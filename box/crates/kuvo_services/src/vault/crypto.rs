use std::{collections::BTreeMap, io};

use aes_gcm::{Aes256Gcm, KeyInit, Nonce, Tag, aead::AeadInPlace};
use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use zeroize::Zeroizing;

use super::{Error, Result};

/// Text owned by the vault. Serialization and formatting require an explicit
/// decision at the boundary where the plaintext is consumed.
#[derive(serde::Deserialize)]
#[serde(transparent)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(text: String) -> Self {
        Self(Zeroizing::new(text))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

pub type Values = BTreeMap<String, Secret>;
pub(crate) type Key = Zeroizing<[u8; 32]>;

pub(crate) fn random<const N: usize>() -> Result<Zeroizing<[u8; N]>> {
    let mut bytes = Zeroizing::new([0; N]);
    getrandom::fill(bytes.as_mut()).map_err(|error| {
        Error::disk(
            "Couldn't generate vault randomness.",
            io::Error::other(error),
        )
    })?;
    Ok(bytes)
}

pub(crate) fn hmac(key: &[u8], parts: &[&[u8]]) -> Zeroizing<[u8; 32]> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC supports any key length");
    for part in parts {
        mac.update(part);
    }
    Zeroizing::new(mac.finalize().into_bytes().into())
}

/// The file's original format is base64(nonce || tag || ciphertext).
pub(crate) fn seal(key: &Key, mut body: Zeroizing<Vec<u8>>, aad: &str) -> Result<String> {
    let nonce = random::<12>()?;
    let tag = Aes256Gcm::new((&**key).into())
        .encrypt_in_place_detached(
            Nonce::from_slice(&*nonce),
            aad.as_bytes(),
            body.as_mut_slice(),
        )
        .map_err(|_| Error::refused(503, "Couldn't encrypt the vault's values."))?;
    let mut envelope = Vec::with_capacity(28 + body.len());
    envelope.extend_from_slice(&*nonce);
    envelope.extend_from_slice(&tag);
    envelope.extend_from_slice(&body);
    Ok(STANDARD.encode(envelope))
}

pub(crate) fn open(key: &Key, text: &str, aad: &str) -> Option<Zeroizing<Vec<u8>>> {
    let mut envelope = Zeroizing::new(STANDARD.decode(text.trim()).ok()?);
    if envelope.len() < 28 {
        return None;
    }
    let (header, body) = envelope.split_at_mut(28);
    Aes256Gcm::new((&**key).into())
        .decrypt_in_place_detached(
            Nonce::from_slice(&header[..12]),
            aad.as_bytes(),
            body,
            Tag::from_slice(&header[12..]),
        )
        .ok()?;
    envelope.copy_within(28.., 0);
    let end = envelope.len() - 28;
    use zeroize::Zeroize as _;
    envelope[end..].zeroize();
    envelope.truncate(end);
    Some(envelope)
}

// Node's base64 decoder accepts either alphabet, padding and ignorable bytes.
pub(crate) fn decode_base64(text: &str) -> Vec<u8> {
    use base64::{
        alphabet,
        engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
    };
    let ascii = Zeroizing::new(
        text.bytes()
            .take_while(|byte| *byte != b'=')
            .filter_map(|byte| match byte {
                b'-' => Some(b'+'),
                b'_' => Some(b'/'),
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'+' | b'/' => Some(byte),
                _ => None,
            })
            .collect::<Vec<_>>(),
    );
    let len = ascii.len() - usize::from(ascii.len() % 4 == 1);
    GeneralPurpose::new(
        &alphabet::STANDARD,
        GeneralPurposeConfig::new()
            .with_decode_padding_mode(DecodePaddingMode::Indifferent)
            .with_decode_allow_trailing_bits(true),
    )
    .decode(&ascii[..len])
    .unwrap_or_default()
}
