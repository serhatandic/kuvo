//! Ed25519 document envelopes. Verification keeps the original signed bytes for forwarding.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::keys::{decode, decode_exact, encode};

const DOMAIN: &[u8] = b"kuvo-link doc v1\0";
pub const MAX_BYTES: usize = 512 * 1024;
pub const MAX_SEQUENCE: u64 = (1 << 53) - 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Peers,
    Phones,
    Personalities,
    Leadership,
    SetupShared,
    SetupCheckpoint,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub d: String,
    pub s: String,
}

#[derive(Debug, Clone)]
pub struct Document {
    pub origin: [u8; 32],
    pub kind: Kind,
    pub n: u64,
    pub body: Map<String, Value>,
    pub hash: [u8; 32],
    pub envelope: Envelope,
}

impl Document {
    pub fn digest(&self) -> String {
        encode(&self.hash[..8])
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Size,
    Encoding,
    Shape,
    Signature,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Size => "signed document exceeds 512 KiB",
            Self::Encoding => "invalid signed document encoding",
            Self::Shape => "invalid signed document fields",
            Self::Signature => "invalid document signature",
        })
    }
}

impl std::error::Error for Error {}

pub fn sign(
    seed: &[u8; 32],
    kind: Kind,
    n: u64,
    body: Map<String, Value>,
) -> Result<Envelope, Error> {
    if !(1..=MAX_SEQUENCE).contains(&n) {
        return Err(Error::Shape);
    }
    let key = SigningKey::from_bytes(seed);
    let origin = encode(key.verifying_key().as_bytes());
    let bytes = crate::json::ordinary(
        &json!({ "v": 1, "kind": kind, "origin": origin, "n": n, "body": body }),
    );
    if bytes.len() > MAX_BYTES {
        return Err(Error::Size);
    }
    let signing = [DOMAIN, bytes.as_bytes()].concat();
    Ok(Envelope {
        d: encode(bytes.as_bytes()),
        s: encode(&key.sign(&signing).to_bytes()),
    })
}

pub fn verify(envelope: Envelope) -> Result<Document, Error> {
    if envelope.d.len() > (MAX_BYTES * 4).div_ceil(3) {
        return Err(Error::Size);
    }
    let bytes = decode(&envelope.d).ok_or(Error::Encoding)?;
    if bytes.len() > MAX_BYTES || encode(&bytes) != envelope.d {
        return Err(Error::Encoding);
    }
    #[derive(Deserialize)]
    struct Fields {
        v: f64,
        kind: Kind,
        origin: String,
        n: f64,
        body: Map<String, Value>,
    }
    let fields: Fields = serde_json::from_slice(&bytes).map_err(|_| Error::Shape)?;
    if fields.v != 1.0
        || fields.n.fract() != 0.0
        || !(1.0..=MAX_SEQUENCE as f64).contains(&fields.n)
    {
        return Err(Error::Shape);
    }
    let origin = decode_exact(&fields.origin).ok_or(Error::Shape)?;
    if fields.origin.len() != 43 {
        return Err(Error::Shape);
    }
    let key = VerifyingKey::from_bytes(&origin).map_err(|_| Error::Signature)?;
    let sig = Signature::from_bytes(&decode_exact(&envelope.s).ok_or(Error::Signature)?);
    key.verify_strict(&[DOMAIN, &bytes].concat(), &sig)
        .map_err(|_| Error::Signature)?;
    Ok(Document {
        origin,
        kind: fields.kind,
        n: fields.n as u64,
        body: fields.body,
        hash: Sha256::digest(&bytes).into(),
        envelope,
    })
}

pub const PART_SIZE: usize = 40_000;
const MAX_PARTS: usize = (MAX_BYTES * 4).div_ceil(3).div_ceil(PART_SIZE) + 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Part {
    pub whole: String,
    pub i: usize,
    pub of: usize,
    pub d: String,
    pub s: String,
}

pub fn split(envelope: &Envelope) -> Result<Vec<Part>, Error> {
    if envelope.d.len() > (MAX_BYTES * 4).div_ceil(3) {
        return Err(Error::Size);
    }
    if !envelope.d.is_ascii() {
        return Err(Error::Encoding);
    }
    let whole = encode(&Sha256::digest(&envelope.d));
    let of = envelope.d.len().div_ceil(PART_SIZE);
    Ok((0..of)
        .map(|i| Part {
            whole: whole.clone(),
            i,
            of,
            d: envelope.d[i * PART_SIZE..envelope.d.len().min((i + 1) * PART_SIZE)].to_owned(),
            s: envelope.s.clone(),
        })
        .collect())
}

#[derive(Default)]
pub struct Parts {
    sources: std::collections::HashMap<[u8; 32], std::collections::VecDeque<Pending>>,
}

struct Pending {
    whole: String,
    signature: String,
    pieces: Vec<Option<String>>,
}

impl Parts {
    /// Keeps four incomplete documents per authenticated sender. Different senders' pieces
    /// never mix. The assembled bytes must match the announced hash and signature.
    pub fn receive(&mut self, from: [u8; 32], part: Part) -> Result<Option<Document>, Error> {
        if !(2..=MAX_PARTS).contains(&part.of)
            || part.i >= part.of
            || part.d.len() > PART_SIZE
            || decode_exact::<32>(&part.whole).is_none()
            || decode_exact::<64>(&part.s).is_none()
        {
            return Err(Error::Shape);
        }
        let list = self.sources.entry(from).or_default();
        let at = match list.iter().position(|entry| entry.whole == part.whole) {
            Some(at) if list[at].pieces.len() == part.of => at,
            old => {
                if let Some(at) = old {
                    list.remove(at);
                }
                if list.len() == 4 {
                    list.pop_front();
                }
                list.push_back(Pending {
                    whole: part.whole,
                    signature: part.s,
                    pieces: vec![None; part.of],
                });
                list.len() - 1
            }
        };
        list[at].pieces[part.i] = Some(part.d);
        if list[at].pieces.iter().any(Option::is_none) {
            return Ok(None);
        }
        let pending = list.remove(at).expect("the completed document is pending");
        if list.is_empty() {
            self.sources.remove(&from);
        }
        let d: String = pending.pieces.into_iter().flatten().collect();
        if encode(&Sha256::digest(&d)) != pending.whole {
            return Err(Error::Encoding);
        }
        verify(Envelope {
            d,
            s: pending.signature,
        })
        .map(Some)
    }

    pub fn forget(&mut self, from: &[u8; 32]) {
        self.sources.remove(from);
    }
}
