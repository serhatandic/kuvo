//! A rebuildable address index over durable receipts. Each read still verifies
//! the record and its current issuer before disclosing any evidence.

use std::{
    collections::{BTreeMap, VecDeque},
    fs, io,
    path::{Path, PathBuf},
};

use kuvo_store::files::{FileStamp, file_stamp};
use serde::Deserialize;
use tokio::sync::{mpsc, oneshot};

const FAMILIES: [&str; 3] = ["submissions", "outbox", "message-receipts"];
const SCAN_BATCH: usize = 128;
type Addresses = BTreeMap<(String, String), BTreeMap<String, String>>;

struct Input {
    owner: bool,
    key: String,
    path: PathBuf,
}

#[derive(Clone)]
pub(crate) struct ReceiptIndex(mpsc::Sender<Query>);

struct Query {
    setup: String,
    session: String,
    after: String,
    reply: oneshot::Sender<io::Result<Keys>>,
}

pub(crate) struct Keys {
    pub owner: Vec<String>,
    pub listing: Vec<String>,
    pub complete: bool,
}

#[derive(Default)]
struct Index {
    stamp: Vec<Option<FileStamp>>,
    owner: Addresses,
    listing: Addresses,
    unread: VecDeque<Input>,
    damaged: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Claim {
    #[serde(default)]
    format_version: u32,
    submission: Address,
}

#[derive(Deserialize)]
struct Address {
    setup: String,
    session: String,
    id: String,
    origin: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Outgoing {
    #[serde(default)]
    format_version: u32,
    #[serde(default)]
    scoped: bool,
    submission: Option<String>,
    setup: Option<String>,
    record: Option<String>,
    receipt: Option<Receipt>,
}

#[derive(Deserialize)]
struct Receipt {
    owner: String,
    operation: String,
}

impl ReceiptIndex {
    pub(crate) fn start(home: &Path) -> Self {
        let root = home.join("run/owned-data");
        let (send, mut queries) = mpsc::channel::<Query>(8);
        tokio::spawn(async move {
            let mut index = Index::default();
            while let Some(query) = queries.recv().await {
                let root = root.clone();
                match tokio::task::spawn_blocking(move || {
                    let result = index.query(&root, &query.setup, &query.session, &query.after);
                    let _ = query.reply.send(result);
                    index
                })
                .await
                {
                    Ok(updated) => index = updated,
                    Err(_) => index = Index::default(),
                }
            }
        });
        Self(send)
    }

    pub(crate) async fn keys(&self, setup: &str, session: &str, after: &str) -> io::Result<Keys> {
        let failed = || io::Error::other("The submission receipt index stopped.");
        let (reply, answer) = oneshot::channel();
        self.0
            .send(Query {
                setup: setup.into(),
                session: session.into(),
                after: after.into(),
                reply,
            })
            .await
            .map_err(|_| failed())?;
        answer.await.map_err(|_| failed())?
    }
}

impl Index {
    fn query(&mut self, root: &Path, setup: &str, session: &str, after: &str) -> io::Result<Keys> {
        let before = stamps(root)?;
        if self.stamp != before {
            *self = Self {
                stamp: before,
                unread: inputs(root)?,
                ..Self::default()
            };
        }
        for _ in 0..SCAN_BATCH {
            let Some(input) = self.unread.pop_front() else {
                break;
            };
            self.damaged |= self.scan(input).is_err();
        }
        let scope = (setup.to_owned(), session.to_owned());
        let candidates = |rows: &Addresses| {
            rows.get(&scope)
                .into_iter()
                .flat_map(|rows| rows.iter())
                .filter(|(handle, _)| handle.as_str() > after)
                .map(|(_, key)| key.clone())
                .collect()
        };
        Ok(Keys {
            owner: candidates(&self.owner),
            listing: candidates(&self.listing),
            complete: self.unread.is_empty() && !self.damaged && stamps(root)? == self.stamp,
        })
    }

    fn scan(&mut self, input: Input) -> io::Result<()> {
        let Input { owner, key, path } = input;
        let invalid = || io::Error::other("A receipt has invalid address evidence.");
        let bytes = fs::read(path)?;
        if owner {
            let claim = serde_json::from_slice::<Claim>(&bytes)?;
            if claim.format_version > 1 || !address_matches(&claim.submission, &key) {
                return Err(invalid());
            }
            insert(&mut self.owner, claim.submission, &key);
        } else {
            let saved = serde_json::from_slice::<Outgoing>(&bytes)?;
            if !saved.scoped {
                return Ok(());
            }
            let (Some(setup), Some(session), Some(id), Some(receipt)) =
                (saved.setup, saved.record, saved.submission, saved.receipt)
            else {
                return Err(invalid());
            };
            let address = Address {
                setup,
                session,
                id,
                origin: receipt.owner,
            };
            if saved.format_version > 1
                || receipt.operation != key
                || !address_matches(&address, &key)
            {
                return Err(invalid());
            }
            insert(&mut self.listing, address, &key);
        }
        Ok(())
    }
}

fn inputs(root: &Path) -> io::Result<VecDeque<Input>> {
    let mut unread = VecDeque::new();
    for family in FAMILIES {
        let entries = match fs::read_dir(root.join(family)) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            let filename = entry.file_name();
            let Some(key) = filename
                .to_str()
                .and_then(|name| name.strip_suffix(".json"))
                .filter(|key| {
                    key.len() == 43
                        && key
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
                })
            else {
                continue;
            };
            if entry.file_type()?.is_file() {
                unread.push_back(Input {
                    owner: family == "submissions",
                    key: key.into(),
                    path: entry.path(),
                });
            }
        }
    }
    Ok(unread)
}

fn stamps(root: &Path) -> io::Result<Vec<Option<FileStamp>>> {
    FAMILIES
        .iter()
        .map(|family| file_stamp(&root.join(family)))
        .collect()
}

fn address_matches(address: &Address, key: &str) -> bool {
    crate::sessions::valid_id(&address.session)
        && crate::submissions::listing_id(&address.id, &address.setup)
            .is_some_and(|(origin, _)| origin == address.origin)
        && crate::submissions::fingerprint(serde_json::json!([
            "draft-submission",
            address.setup,
            address.session,
            address.id
        ])) == key
}

fn insert(rows: &mut Addresses, row: Address, key: &str) {
    rows.entry((row.setup, row.session))
        .or_default()
        .insert(row.id, key.into());
}
