//! Parse and sign with private keys in memory. The agent receives only a public
//! blob and a signature in the SSH agent's length-prefixed encoding.
use ed25519_dalek::{Signer, SigningKey};
use p256::pkcs8::DecodePrivateKey as _;
use rsa::{
    BoxedUint, PrivateKey as RsaPrivateKey,
    pkcs1::DecodeRsaPrivateKey as _,
    pkcs8::DecodePrivateKey as _,
    sha2::{Sha256, Sha512},
    traits::PublicKeyParts,
};
use ssh_key::{Algorithm, PrivateKey, Signature};

use super::{Error, Result, Secret};

pub(super) struct Key {
    pub blob: Vec<u8>,
    key: SignWith,
}

enum SignWith {
    OpenSsh(Box<PrivateKey>),
    Rsa(Box<RsaPrivateKey>),
    Ed25519(SigningKey),
    P256(p256::ecdsa::SigningKey),
    P384(p384::ecdsa::SigningKey),
    P521(p521::ecdsa::SigningKey),
}

impl Key {
    pub fn read(secret: &Secret) -> Result<Self> {
        let text = secret.expose().trim();
        if text.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----") {
            let key = PrivateKey::from_openssh(text).map_err(|_| invalid())?;
            if key.is_encrypted() {
                return Err(passphrase());
            }
            if let Some(rsa) = key.key_data().rsa() {
                let number = |part: &ssh_key::Mpint| {
                    let bytes = part.as_positive_bytes().ok_or_else(invalid)?;
                    BoxedUint::from_be_slice(bytes, (bytes.len() as u32) * 8).map_err(|_| invalid())
                };
                let private = RsaPrivateKey::from_components(
                    number(&rsa.public.n)?,
                    number(&rsa.public.e)?,
                    number(&rsa.private.d)?,
                    vec![number(&rsa.private.p)?, number(&rsa.private.q)?],
                )
                .map_err(|_| invalid())?;
                return Ok(Self::rsa(private));
            }
            if !matches!(
                key.algorithm(),
                Algorithm::Ed25519 | Algorithm::Ecdsa { .. }
            ) {
                return Err(unsupported());
            }
            let blob = key.public_key().to_bytes().map_err(|_| invalid())?;
            return Ok(Self {
                blob,
                key: SignWith::OpenSsh(Box::new(key)),
            });
        }
        if text.contains("ENCRYPTED") {
            return Err(passphrase());
        }
        if let Ok(key) =
            RsaPrivateKey::from_pkcs1_pem(text).or_else(|_| RsaPrivateKey::from_pkcs8_pem(text))
        {
            return Ok(Self::rsa(key));
        }
        if let Ok(key) = SigningKey::from_pkcs8_pem(text) {
            let mut blob = Vec::new();
            put(&mut blob, b"ssh-ed25519");
            put(&mut blob, key.verifying_key().as_bytes());
            return Ok(Self {
                blob,
                key: SignWith::Ed25519(key),
            });
        }
        macro_rules! elliptic {
            ($curve:ident, $variant:ident, $name:literal) => {
                if let Ok(secret) = $curve::SecretKey::from_pkcs8_pem(text)
                    .map_err(|_| ())
                    .or_else(|_| $curve::SecretKey::from_sec1_pem(text).map_err(|_| ()))
                {
                    use $curve::elliptic_curve::sec1::ToEncodedPoint as _;
                    let point = secret.public_key().to_encoded_point(false);
                    let key = $curve::ecdsa::SigningKey::from_slice(secret.to_bytes().as_ref())
                        .map_err(|_| invalid())?;
                    let mut blob = Vec::new();
                    put(&mut blob, concat!("ecdsa-sha2-", $name).as_bytes());
                    put(&mut blob, $name.as_bytes());
                    put(&mut blob, point.as_bytes());
                    return Ok(Self {
                        blob,
                        key: SignWith::$variant(key),
                    });
                }
            };
        }
        elliptic!(p256, P256, "nistp256");
        elliptic!(p384, P384, "nistp384");
        elliptic!(p521, P521, "nistp521");
        Err(invalid())
    }

    fn rsa(key: RsaPrivateKey) -> Self {
        let mut blob = Vec::new();
        put(&mut blob, b"ssh-rsa");
        mpint(&mut blob, &key.e_bytes());
        mpint(&mut blob, &key.n_bytes());
        Self {
            blob,
            key: SignWith::Rsa(Box::new(key)),
        }
    }

    pub fn sign(&self, data: &[u8], flags: u32) -> Result<Vec<u8>> {
        let (algorithm, signature) = match &self.key {
            SignWith::OpenSsh(key) => {
                let signature: Signature = key.try_sign(data).map_err(|_| invalid())?;
                (
                    signature.algorithm().as_str().to_owned(),
                    signature.as_bytes().to_vec(),
                )
            }
            SignWith::Rsa(key) => {
                use sha1::Sha1;
                let (name, signature) = if flags & 4 != 0 {
                    ("rsa-sha2-512", key.pkcs1v15_sign::<Sha512>(data))
                } else if flags & 2 != 0 {
                    ("rsa-sha2-256", key.pkcs1v15_sign::<Sha256>(data))
                } else {
                    ("ssh-rsa", key.pkcs1v15_sign::<Sha1>(data))
                };
                (name.into(), signature.map_err(|_| invalid())?)
            }
            SignWith::Ed25519(key) => (
                "ssh-ed25519".into(),
                key.try_sign(data)
                    .map_err(|_| invalid())?
                    .to_bytes()
                    .to_vec(),
            ),
            SignWith::P256(key) => {
                let signature: p256::ecdsa::Signature =
                    key.try_sign(data).map_err(|_| invalid())?;
                ec_signature("ecdsa-sha2-nistp256", signature.to_bytes().as_slice())
            }
            SignWith::P384(key) => {
                let signature: p384::ecdsa::Signature =
                    key.try_sign(data).map_err(|_| invalid())?;
                ec_signature("ecdsa-sha2-nistp384", signature.to_bytes().as_slice())
            }
            SignWith::P521(key) => {
                let signature: p521::ecdsa::Signature =
                    key.try_sign(data).map_err(|_| invalid())?;
                ec_signature("ecdsa-sha2-nistp521", signature.to_bytes().as_slice())
            }
        };
        let mut packed = Vec::new();
        put(&mut packed, algorithm.as_bytes());
        put(&mut packed, &signature);
        Ok(packed)
    }
}

fn ec_signature(algorithm: &str, bytes: &[u8]) -> (String, Vec<u8>) {
    let (r, s) = bytes.split_at(bytes.len() / 2);
    let mut signature = Vec::new();
    mpint(&mut signature, r);
    mpint(&mut signature, s);
    (algorithm.into(), signature)
}

fn invalid() -> Error {
    Error::refused(400, "That isn't a private key.")
}

fn passphrase() -> Error {
    Error::refused(
        400,
        "This key has a passphrase: Kuvo Code's agent uses keys without one (ssh-keygen -p takes it off a copy).",
    )
}

fn unsupported() -> Error {
    Error::refused(
        400,
        "This kind of key isn't one SSH signs with here: use an Ed25519, RSA or ECDSA key.",
    )
}

fn mpint(out: &mut Vec<u8>, bytes: &[u8]) {
    let bytes = bytes
        .iter()
        .position(|byte| *byte != 0)
        .map_or(&[][..], |at| &bytes[at..]);
    let pad = bytes.first().is_some_and(|byte| byte & 0x80 != 0);
    out.extend_from_slice(&((bytes.len() + usize::from(pad)) as u32).to_be_bytes());
    if pad {
        out.push(0);
    }
    out.extend_from_slice(bytes);
}

pub(super) fn put(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
}
