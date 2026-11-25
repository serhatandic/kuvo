//! Signed box/1 release admission, before any pack bytes are opened.

use std::{collections::BTreeMap, io};

use base64::{
    Engine as _, alphabet,
    engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::Release;

pub const UPDATE_KEY: &str = "iLSFoygG8wg+r2yj+/qnoMHpZIhPKRNLlf9vpEKmx0A=";
const DOMAIN: &[u8] = b"kuvo-box v1\n";
const INVALID: &str = "That isn't a release.";

#[derive(Clone, Debug, Deserialize)]
pub struct Pack {
    pub url: String,
    pub sha256: String,
}

impl Pack {
    fn validate(&self) -> io::Result<()> {
        if is_sha256(&self.sha256) {
            Ok(())
        } else {
            Err(invalid())
        }
    }

    pub fn filename(&self) -> io::Result<String> {
        let url = reqwest::Url::parse(&self.url).map_err(|_| invalid())?;
        let basename = url.path().rsplit('/').find(|part| !part.is_empty());
        let name = basename
            .and_then(|name| {
                percent_encoding::percent_decode_str(name)
                    .decode_utf8()
                    .ok()
            })
            .ok_or_else(invalid)?;
        if !name.starts_with(|c: char| c.is_ascii_alphanumeric())
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            return Err(invalid());
        }
        Ok(name.into_owned())
    }
}

#[derive(Clone, Debug, Deserialize)]
struct Manifest {
    kuvo: String,
    version: String,
    engine: String,
    spreads: Option<Value>,
    released: Option<Value>,
    launch: Option<Value>,
    bridge: Option<String>,
    pack: Pack,
    #[serde(default)]
    packs: BTreeMap<String, Pack>,
    #[serde(flatten)]
    engines: EngineCatalogue,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct EngineCatalogue {
    darwin: Option<Value>,
    linux: Option<Value>,
    windows: Option<Value>,
}

#[derive(Deserialize)]
pub(crate) struct EngineBuild {
    pub url: String,
    pub integrity: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReleaseRef {
    pub version: String,
    pub sha256: String,
}

impl ReleaseRef {
    pub fn matches_bytes(&self, bytes: &[u8]) -> bool {
        self.sha256 == sha256(bytes)
    }

    pub fn target(&self) -> io::Result<Release> {
        if !is_sha256(&self.sha256) {
            return Err(invalid());
        }
        format!("versions/{}-{}/bin", self.version, &self.sha256[..12]).parse()
    }
}

/// Only signature verification can create this value.
#[derive(Clone, Debug)]
pub struct SignedRelease {
    manifest: Manifest,
}

impl SignedRelease {
    pub fn verify(envelope: &[u8], keys: &[&str]) -> io::Result<Self> {
        #[derive(Deserialize)]
        struct Envelope {
            manifest: String,
            signature: String,
            key: String,
        }
        let envelope: Envelope = serde_json::from_slice(envelope).map_err(|_| invalid())?;
        let claimed = decode_base64(&envelope.key);
        let key = keys
            .iter()
            .filter_map(|key| <[u8; 32]>::try_from(decode_base64(key)).ok())
            .find(|key| key.as_slice() == claimed)
            .ok_or_else(|| denied("It isn't signed with a key this machine trusts."))?;
        let mismatch = || denied("Its signature doesn't match.");
        let key = VerifyingKey::from_bytes(&key).map_err(|_| mismatch())?;
        let signature =
            Signature::from_slice(&decode_base64(&envelope.signature)).map_err(|_| mismatch())?;
        let decoded = decode_base64(&envelope.manifest);
        let mut signed = Vec::with_capacity(DOMAIN.len() + decoded.len());
        signed.extend_from_slice(DOMAIN);
        signed.extend_from_slice(&decoded);
        key.verify(&signed, &signature).map_err(|_| mismatch())?;

        let value: Value = serde_json::from_slice(&decoded).map_err(|_| invalid())?;
        let integer = |field: &str, minimum: f64| {
            value.get(field).is_none_or(|value| {
                value.as_f64().is_some_and(|number| {
                    number.fract() == 0.0 && (minimum..=9_007_199_254_740_991.0).contains(&number)
                })
            })
        };
        if !integer("layout", 0.0) || !integer("clientProtocol", 1.0) {
            return Err(invalid());
        }
        let manifest: Manifest = serde_json::from_value(value).map_err(|_| invalid())?;
        if manifest.kuvo != "box/1" || !valid_engine(&manifest.engine) {
            return Err(invalid());
        }
        manifest.pack.validate()?;
        let release = Self { manifest };
        release
            .reference(&release.manifest.pack)
            .target()
            .map_err(|_| invalid())?;
        for pack in release.manifest.packs.values() {
            pack.validate()?;
        }
        Ok(release)
    }

    pub fn version(&self) -> &str {
        &self.manifest.version
    }

    pub fn engine(&self) -> &str {
        &self.manifest.engine
    }

    pub fn spreads(&self) -> bool {
        self.manifest.spreads == Some(Value::Bool(true))
    }

    pub fn launches_rust(&self) -> bool {
        self.manifest.launch.as_ref().and_then(Value::as_str) == Some("rust")
    }

    pub(crate) fn bridge(&self) -> Option<&str> {
        self.manifest.bridge.as_deref()
    }

    pub(crate) fn released(&self) -> Option<jiff::Timestamp> {
        self.manifest.released.as_ref()?.as_str()?.parse().ok()
    }

    pub fn pack_for(&self, target: &str) -> io::Result<&Pack> {
        if self.launches_rust() {
            self.manifest.packs.get(target).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "The release has no pack for this machine.",
                )
            })
        } else {
            Ok(&self.manifest.pack)
        }
    }

    pub(crate) fn rust_pack_by_hash(&self, digest: &str) -> Option<&Pack> {
        self.launches_rust().then_some(())?;
        self.manifest
            .packs
            .values()
            .find(|pack| pack.sha256 == digest)
    }

    pub fn reference(&self, pack: &Pack) -> ReleaseRef {
        ReleaseRef {
            version: self.manifest.version.clone(),
            sha256: pack.sha256.clone(),
        }
    }

    pub(crate) fn engine_build(&self, target: &str) -> Option<EngineBuild> {
        self.manifest.engines.build_for(target)
    }
}

impl EngineCatalogue {
    pub(crate) fn build_for(&self, target: &str) -> Option<EngineBuild> {
        let (arch, system) = target.split_once('-')?;
        let catalogue = match system {
            "apple-darwin" => self.darwin.as_ref(),
            "unknown-linux-musl" => self.linux.as_ref(),
            "pc-windows-gnullvm" => self.windows.as_ref(),
            _ => None,
        }?;
        serde_json::from_value(catalogue.get(arch)?.clone()).ok()
    }
}

// These names identify release packs, so WSL selects the Linux pack.
pub fn local_target() -> io::Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-musl"),
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-musl"),
        ("windows", "x86_64") => Ok("x86_64-pc-windows-gnullvm"),
        _ => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "The release has no pack for this machine.",
        )),
    }
}

pub(crate) fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn is_sha256(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn valid_engine(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_digit())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
}

// The envelope is encoded like Node's Buffer: either alphabet, optional padding.
fn decode_base64(text: &str) -> Vec<u8> {
    let mut clean: Vec<_> = text
        .bytes()
        .take_while(|&byte| byte != b'=')
        .filter_map(|byte| match byte {
            b'-' => Some(b'+'),
            b'_' => Some(b'/'),
            b'+' | b'/' | b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => Some(byte),
            _ => None,
        })
        .collect();
    if clean.len() % 4 == 1 {
        clean.pop();
    }
    GeneralPurpose::new(
        &alphabet::STANDARD,
        GeneralPurposeConfig::new()
            .with_decode_padding_mode(DecodePaddingMode::Indifferent)
            .with_decode_allow_trailing_bits(true),
    )
    .decode(clean)
    .unwrap_or_default()
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, INVALID)
}

fn denied(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}
