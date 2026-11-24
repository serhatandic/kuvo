use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use kuvo_store::files;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use super::{
    Error, LOCKED, Result, Secret, Values, check_id,
    crypto::{self, Key},
};

const WRAP_AAD: &str = "kuvo-vault-key";
const CHECK: &[u8] = b"kuvo-vault-check";

pub struct Encrypted {
    folder: PathBuf,
    keys: Option<Keys>,
}

struct Keys {
    store: Key,
    unlock: Key,
}

#[derive(Serialize, Deserialize)]
struct Wrapped {
    #[serde(default)]
    v: u8,
    wrapped: String,
    check: String,
}

impl Encrypted {
    pub fn new(home: &Path) -> Self {
        Self {
            folder: home.join("box/vault"),
            keys: None,
        }
    }

    pub fn made(&self) -> bool {
        self.folder.join("key.json").exists()
    }

    pub fn locked(&self) -> bool {
        self.made() && self.keys.is_none()
    }

    /// The resident makes a vault under its writer lock.
    pub fn make(&mut self) -> Result<Secret> {
        if self.made() {
            return Err(Error::refused(409, "This computer has a vault already."));
        }
        let store = crypto::random()?;
        let unlock = crypto::random()?;
        let wrapped = Wrapped {
            v: 1,
            wrapped: crypto::seal(&unlock, Zeroizing::new(store.to_vec()), WRAP_AAD)?,
            check: STANDARD.encode(&*crypto::hmac(&*unlock, &[CHECK])),
        };
        let mut text = serde_json::to_vec(&wrapped).expect("wrapped keys serialize");
        text.push(b'\n');
        files::write_secret(&self.folder.join("key.json"), &text)
            .map_err(|error| Error::disk("Couldn't write the vault's wrapped key.", error))?;
        let result = Secret::new(URL_SAFE_NO_PAD.encode(&*unlock));
        self.keys = Some(Keys { store, unlock });
        Ok(result)
    }

    pub fn unlock(&mut self, supplied: &str) -> bool {
        let supplied = Zeroizing::new(crypto::decode_base64(supplied));
        if supplied.len() != 32 {
            return false;
        }
        let mut unlock = Zeroizing::new([0; 32]);
        unlock.copy_from_slice(&supplied);
        if let Some(keys) = &self.keys {
            return bool::from(keys.unlock.ct_eq(&*unlock));
        }
        let Ok(bytes) = fs::read(self.folder.join("key.json")) else {
            return false;
        };
        let Ok(wrapped) = serde_json::from_slice::<Wrapped>(&bytes) else {
            return false;
        };
        if !bool::from(
            STANDARD
                .encode(&*crypto::hmac(&*unlock, &[CHECK]))
                .as_bytes()
                .ct_eq(wrapped.check.as_bytes()),
        ) {
            return false;
        }
        let Some(plain) = crypto::open(&unlock, &wrapped.wrapped, WRAP_AAD) else {
            return false;
        };
        if plain.len() != 32 {
            return false;
        }
        let mut store = Zeroizing::new([0; 32]);
        store.copy_from_slice(&plain);
        self.keys = Some(Keys { store, unlock });
        true
    }

    pub fn unlock_key(&self) -> Option<Secret> {
        self.keys
            .as_ref()
            .map(|keys| Secret::new(URL_SAFE_NO_PAD.encode(&*keys.unlock)))
    }

    pub fn unlock_proof(&self, tag: &str, nonce: &str) -> Option<String> {
        let keys = self.keys.as_ref()?;
        Some(URL_SAFE_NO_PAD.encode(&*crypto::hmac(
            &*keys.unlock,
            &[b"kuvo-vault-", tag.as_bytes(), b"\0", nonce.as_bytes()],
        )))
    }

    pub fn proves_unlock(&self, tag: &str, nonce: &str, proof: &str) -> bool {
        self.unlock_proof(tag, nonce)
            .is_some_and(|expected| bool::from(expected.as_bytes().ct_eq(proof.as_bytes())))
    }

    pub(crate) fn file_key(&self) -> Option<Key> {
        let keys = self.keys.as_ref()?;
        Some(crypto::hmac(&*keys.store, &[b"kuvo-vault-files"]))
    }

    pub fn values(&self, id: &str) -> Result<Values> {
        check_id(id)?;
        let key = &self.require_keys()?.store;
        let path = self.folder.join("values").join(id);
        let text = match fs::read_to_string(path) {
            Ok(text) if text.is_empty() => return Ok(Values::new()),
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Values::new()),
            Err(error) => {
                return Err(Error::disk(
                    "Couldn't read the vault's encrypted values.",
                    error,
                ));
            }
        };
        let parsed = crypto::open(key, &text, id)
            .and_then(|plaintext| serde_json::from_slice::<Values>(&plaintext).ok());
        parsed.ok_or_else(|| {
            Error::refused(
                409,
                "This item's values can't be read: they were changed outside Kuvo Code.",
            )
        })
    }

    pub fn put_values(&self, id: &str, values: &Values) -> Result<()> {
        check_id(id)?;
        let key = &self.require_keys()?.store;
        let exposed: BTreeMap<_, _> = values
            .iter()
            .map(|(field, value)| (field, value.expose()))
            .collect();
        let mut plain = Zeroizing::new(Vec::new());
        serde_json::to_writer(&mut *plain, &exposed).expect("string fields serialize");
        let encoded = crypto::seal(key, plain, id)?;
        files::write_secret(&self.folder.join("values").join(id), encoded.as_bytes())
            .map_err(|error| Error::disk("Couldn't write the vault's encrypted values.", error))
    }

    pub fn delete_values(&self, id: &str) -> Result<()> {
        check_id(id)?;
        match fs::remove_file(self.folder.join("values").join(id)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(Error::disk(
                "Couldn't remove the vault's encrypted values.",
                error,
            )),
        }
    }

    fn require_keys(&self) -> Result<&Keys> {
        self.keys
            .as_ref()
            .ok_or_else(|| Error::refused(423, LOCKED))
    }
}
