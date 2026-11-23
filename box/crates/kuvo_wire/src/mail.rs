//! Peer mail v1. A single envelope can carry several fields, including fields from newer boxes.

use serde_json::{Map, Value};

use crate::frames::{Kind, MAX_RELAY_PAYLOAD, frame};
use crate::noise::{self, MAIL_PROLOGUE};

pub const MAX_FRAME: usize = MAX_RELAY_PAYLOAD - 17;
const REMOVAL: &[u8] = b"kuvo-removal-v1\0";
const KEY_BYTES: usize = 32;
const NOISE_OVERHEAD: usize = KEY_BYTES + 16;
const FRAME_HEADER: usize = 5;
pub const MAX_PLAINTEXT: usize = MAX_FRAME - FRAME_HEADER - NOISE_OVERHEAD;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope(pub Map<String, Value>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Shape,
    Version,
    Size,
    Noise,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Shape => "invalid peer mail JSON",
            Self::Version => "unsupported peer mail version",
            Self::Size => "peer mail exceeds the relay limit",
            Self::Noise => "peer mail failed authentication",
        })
    }
}

impl std::error::Error for Error {}

impl Envelope {
    pub fn encode(&self) -> Vec<u8> {
        let mut fields = self.0.clone();
        fields.insert("formatVersion".into(), 1.into());
        crate::json::ordinary(&Value::Object(fields)).into_bytes()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut fields: Map<String, Value> =
            serde_json::from_slice(bytes).map_err(|_| Error::Shape)?;
        if let Some(version) = fields.shift_remove("formatVersion")
            && !matches!(version.as_f64(), Some(0.0 | 1.0))
        {
            return Err(Error::Version);
        }
        Ok(Self(fields))
    }

    pub fn seal(&self, local: &[u8; 32], remote: &[u8; 32]) -> Result<Vec<u8>, Error> {
        self.seal_with(local, remote, false, None)
    }

    pub fn seal_removal(&self, local: &[u8; 32], remote: &[u8; 32]) -> Result<Vec<u8>, Error> {
        self.seal_with(local, remote, true, None)
    }

    pub fn seal_with_ephemeral(
        &self,
        local: &[u8; 32],
        remote: &[u8; 32],
        ephemeral: &[u8; 32],
        removal: bool,
    ) -> Result<Vec<u8>, Error> {
        self.seal_with(local, remote, removal, Some(ephemeral))
    }

    fn seal_with(
        &self,
        local: &[u8; 32],
        remote: &[u8; 32],
        removal: bool,
        ephemeral: Option<&[u8; 32]>,
    ) -> Result<Vec<u8>, Error> {
        let plaintext = self.encode();
        let hint = if removal {
            REMOVAL.len() + KEY_BYTES
        } else {
            0
        };
        if plaintext.len() > MAX_PLAINTEXT - hint {
            return Err(Error::Size);
        }
        let (message, _) = match ephemeral {
            Some(key) => {
                noise::seal_one_way_with_ephemeral(&plaintext, local, remote, MAIL_PROLOGUE, key)
            }
            None => noise::seal_one_way(&plaintext, local, remote, MAIL_PROLOGUE),
        }
        .map_err(|_| Error::Noise)?;
        let body = if removal {
            [REMOVAL, &noise::public_of(local), &message].concat()
        } else {
            message
        };
        Ok(frame(Kind::Mail, 0, &body))
    }

    /// Opens a link frame's body with a trusted peer's key.
    pub fn open(body: &[u8], local: &[u8; 32], remote: &[u8; 32]) -> Result<Self, Error> {
        if body.len() > MAX_FRAME - FRAME_HEADER {
            return Err(Error::Size);
        }
        let (plaintext, _) =
            noise::open_one_way(body, local, remote, MAIL_PROLOGUE).map_err(|_| Error::Noise)?;
        Self::decode(&plaintext)
    }

    /// Authentication here proves only the key in the hint. The caller must validate the signed
    /// removal chain before acting on the contents. This path grants no ordinary mail access.
    pub fn open_removal(body: &[u8], local: &[u8; 32]) -> Option<([u8; 32], Self)> {
        let hinted = body.strip_prefix(REMOVAL)?;
        let (key, message) = hinted.split_at_checked(KEY_BYTES)?;
        let key = key.try_into().ok()?;
        let mail = Self::open(message, local, &key).ok()?;
        Some((key, mail))
    }
}
