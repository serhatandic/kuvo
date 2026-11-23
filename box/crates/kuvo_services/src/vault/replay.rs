//! Keep an operation's identity and the masked answer across resident restarts.
use std::{
    collections::HashMap,
    fs,
    future::Future,
    io,
    path::{Path, PathBuf},
    sync::Mutex,
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tokio::sync::watch;

use super::{Error, Result, helper::millis, signed, tools::Answer};

pub(super) struct Ledger {
    folder: PathBuf,
    running: Mutex<HashMap<String, Live>>,
}

struct Live {
    digest: String,
    received: watch::Receiver<Option<Answer>>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Claim {
    #[serde(default, skip_serializing_if = "is_legacy")]
    format_version: u32,
    digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    answer: Option<Recorded>,
    #[serde(skip_serializing_if = "Option::is_none")]
    at: Option<u64>,
}

fn is_legacy(version: &u32) -> bool {
    *version == 0
}

#[derive(Serialize, Deserialize)]
struct Recorded {
    status: u16,
    value: Answer,
}

enum Admission {
    Recorded(Answer),
    Wait(watch::Receiver<Option<Answer>>),
    Run {
        key: String,
        claim: Claim,
        publisher: watch::Sender<Option<Answer>>,
        received: watch::Receiver<Option<Answer>>,
    },
}

impl Ledger {
    pub fn new(home: &Path) -> Self {
        Self {
            folder: home.join("run/owned-data/mutations"),
            running: Mutex::default(),
        }
    }

    fn admit(&self, path: &str, given: &Map<String, Value>, nonce: &str) -> Result<Admission> {
        let scope = serde_json::json!(["tool", path, given.get("session")]);
        let key = hash(&serde_json::json!([signed::stringify(&scope), nonce]));
        let digest = hash(&ordered(&Value::Object(given.clone())));
        let mut running = self.running.lock().map_err(|_| stopped())?;
        if let Some(live) = running.get(&key) {
            if live.digest != digest {
                return Err(reused());
            }
            return Ok(Admission::Wait(live.received.clone()));
        }
        let file = self.file(&key);
        let claim = Claim {
            format_version: 1,
            digest,
            answer: None,
            at: None,
        };
        let bytes = serde_json::to_vec(&claim).expect("vault operation serializes");
        match kuvo_store::files::write_new(&file, &bytes) {
            Ok(()) => {
                let (publisher, received) = watch::channel(None);
                running.insert(
                    key.clone(),
                    Live {
                        digest: claim.digest.clone(),
                        received: received.clone(),
                    },
                );
                Ok(Admission::Run {
                    key,
                    claim,
                    publisher,
                    received,
                })
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let bytes = fs::read(file)
                    .map_err(|error| Error::disk("Couldn't read the vault's operation.", error))?;
                let previous: Claim = serde_json::from_slice(&bytes).map_err(|_| stopped())?;
                if previous.format_version > 1 {
                    return Err(stopped());
                }
                if previous.digest != claim.digest {
                    return Err(reused());
                }
                if let Some(answer) = previous.answer.filter(|answer| answer.status == 200) {
                    return Ok(Admission::Recorded(answer.value));
                }
                Err(Error::refused(
                    409,
                    "The owner has claimed this operation. Check the record before trying again.",
                ))
            }
            Err(error) => Err(Error::disk("Couldn't keep the vault's operation.", error)),
        }
    }

    pub async fn answer(
        self: &std::sync::Arc<Self>,
        path: &str,
        given: &Map<String, Value>,
        nonce: &str,
        run: impl Future<Output = Answer> + Send + 'static,
    ) -> Result<Answer> {
        let decision = tokio::task::block_in_place(|| self.admit(path, given, nonce))?;
        let mut received = match decision {
            Admission::Recorded(answer) => return Ok(answer),
            Admission::Wait(received) => received,
            Admission::Run {
                key,
                mut claim,
                publisher,
                received,
            } => {
                let ledger = self.clone();
                tokio::spawn(async move {
                    let answer = run.await;
                    claim.answer = Some(Recorded {
                        status: 200,
                        value: answer.clone(),
                    });
                    claim.at = Some(millis());
                    tokio::task::block_in_place(|| {
                        let file = ledger.file(&key);
                        let bytes = serde_json::to_vec(&claim).expect("vault operation serializes");
                        if kuvo_store::files::write_secret(&file, &bytes).is_ok() {
                            publisher.send_replace(Some(answer));
                        }
                        if let Ok(mut running) = ledger.running.lock() {
                            running.remove(&key);
                        }
                    });
                });
                received
            }
        };
        let found = received
            .wait_for(Option::is_some)
            .await
            .map_err(|_| stopped())?;
        found.clone().ok_or_else(stopped)
    }

    fn file(&self, key: &str) -> PathBuf {
        self.folder.join(format!("{key}.json"))
    }
}

fn ordered(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut keys: Vec<_> = object.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            Value::Object(
                keys.into_iter()
                    .map(|key| (key.clone(), ordered(&object[key])))
                    .collect(),
            )
        }
        Value::Array(values) => Value::Array(values.iter().map(ordered).collect()),
        other => other.clone(),
    }
}

fn hash(value: &Value) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(signed::stringify(value).as_bytes()))
}

fn stopped() -> Error {
    Error::refused(503, "Couldn't keep the vault's operation.")
}

fn reused() -> Error {
    Error::refused(409, "That operation id is already in use.")
}
