//! Letter delivery state in the box's run directory. The resident owns one Outbox and one
//! Receipts instance, and runs disk operations on its blocking pool.

use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File},
    io,
    path::{Path, PathBuf},
};

use kuvo_wire::{keys, mail};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

const DAY: u64 = 86_400_000;
const LETTER_MEMORY: u64 = 2 * DAY;
const NOTE_MEMORY: u64 = 30 * DAY;
const RETRY_EVERY: u64 = 60_000;
const EXPIRED: &str = "It couldn't get there in two days of trying.";

#[derive(Serialize, Deserialize)]
struct Failure {
    letter: Letter,
    why: Option<String>,
    said: bool,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum FailedRecord {
    Recorded(Failure),
    Legacy(Letter),
}

/// Destination and waiter facts supplied by the owner lookup and tool waiter.
pub struct FailureContext {
    pub conversation: String,
    pub computer: String,
    pub tool_waiting: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Id(String);

impl TryFrom<String> for Id {
    type Error = Error;

    fn try_from(text: String) -> Result<Self, Error> {
        if !(6..=80).contains(&text.len())
            || !text
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            return Err(Error::InvalidId);
        }
        Ok(Self(text))
    }
}

impl From<Id> for String {
    fn from(id: Id) -> Self {
        id.0
    }
}

impl Id {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Letter {
    pub to: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_to: Option<String>,
    #[serde(default)]
    pub wait: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answers: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub refused: bool,
    /// Older writers and later consumers (calls and seats) keep fields here.
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

/// Plain signed letter fields after the peer mail envelope has authenticated its sender.
#[derive(Debug, Clone, Deserialize)]
pub struct IncomingLetter {
    pub id: Id,
    #[serde(flatten)]
    pub letter: Letter,
}

impl IncomingLetter {
    /// Stale or malformed letters are answered without reserving a delivery ID.
    pub fn refusal(&self, now: u64) -> Option<Delivered> {
        let note = self.letter.note.is_some();
        let old = self.letter.at.is_none_or(|at| now.abs_diff(at) > DAY);
        let session = self.letter.to.strip_prefix("ses_").is_some_and(|value| {
            (1..=60).contains(&value.len())
                && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
        });
        let (why, retry) = if old {
            ("It's too old to deliver.", note)
        } else if !session || self.letter.text.trim().is_empty() {
            ("That isn't a letter.", false)
        } else {
            return None;
        };
        Some(Delivered {
            id: self.id.clone(),
            ok: false,
            why: Some(why.into()),
            retry: retry.then_some(true),
            answer: None,
            lives: None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivered {
    pub id: Id,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lives: Option<String>,
}

impl Delivered {
    pub fn envelope(&self, now: u64) -> mail::Envelope {
        envelope(json!({ "delivered": self, "at": now }))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid letter ID")]
    InvalidId,
    #[error("persisting delivery state at {path}: {source}")]
    Storage { path: PathBuf, source: io::Error },
    #[error("reading delivery state at {path}: {source}")]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("letter doesn't fit in peer mail")]
    Size,
    #[error("generating a delivery ID: {0}")]
    Random(getrandom::Error),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Settlement {
    Ignored,
    Retry,
    Delivered,
    WaitingForAnswer,
    Refused,
}

pub struct Pending {
    pub id: Id,
    pub letter: Letter,
}

pub struct Outbox {
    run: PathBuf,
    sending: PathBuf,
    replies: PathBuf,
    tries: HashMap<String, u64>,
    _writer: File,
}

impl Outbox {
    /// `run` is the resident's private run directory. Existing TypeScript records remain readable.
    pub fn open(run: &Path) -> Result<Self, Error> {
        let sending = run.join("outbox/sending");
        let replies = run.join("replies");
        for path in [&sending, &replies] {
            fs::create_dir_all(path).map_err(|source| storage(path, source))?;
        }
        let writer = lock(&run.join("outbox/writer.lock"))?;
        Ok(Self {
            run: run.into(),
            sending,
            replies,
            tries: HashMap::new(),
            _writer: writer,
        })
    }

    /// Leaves a letter only if its ID hasn't already been queued or settled.
    pub fn enqueue(&mut self, id: &Id, letter: &Letter) -> Result<bool, Error> {
        for path in [
            self.pending(id),
            self.failed(id),
            self.waiting(id),
            self.done(id),
        ] {
            if path.try_exists().map_err(|source| storage(&path, source))? {
                return Ok(false);
            }
        }
        write(&self.pending(id), letter)?;
        Ok(true)
    }

    /// A tool stopped waiting. A request for a transcript has no recipient
    /// for its result, while a message can still go on without a waiter.
    pub(crate) fn give_up(&mut self, id: &Id) -> Result<bool, Error> {
        let pending = self.pending(id);
        let letter = match read::<Letter>(&pending) {
            Ok(letter) => letter,
            Err(Error::Json { .. }) => None,
            Err(error) => return Err(error),
        };
        if let Some(mut letter) = letter {
            if letter.other.contains_key("call") || letter.other.contains_key("seat") {
                return Ok(false);
            }
            if letter.read.is_some() {
                remove(&pending)?;
            } else if letter.wait {
                letter.wait = false;
                write(&pending, &letter)?;
            }
        }
        remove(&self.waiting(id))?;
        Ok(true)
    }

    pub fn due(&mut self, now: u64, online: bool) -> Result<Vec<Pending>, Error> {
        let mut due = Vec::new();
        for file in fs::read_dir(&self.sending).map_err(|source| storage(&self.sending, source))? {
            let path = file
                .map_err(|source| storage(&self.sending, source))?
                .path();
            let Some(stem) = path.file_stem().and_then(|name| name.to_str()) else {
                continue;
            };
            let Ok(id) = Id::try_from(stem.to_owned()) else {
                continue;
            };
            if path.extension().is_none_or(|extension| extension != "json")
                || self.reconcile(&id)?
            {
                continue;
            }
            let letter = match read::<Letter>(&path) {
                Ok(Some(letter)) => letter,
                Ok(None) | Err(Error::Json { .. }) => continue,
                Err(error) => return Err(error),
            };
            if letter.other.contains_key("call") || letter.other.contains_key("seat") {
                continue;
            }
            if expired(&letter, now) {
                self.fail(&id, letter, Some(EXPIRED.to_owned()), false)?;
            } else if online
                && self
                    .tries
                    .get(id.as_str())
                    .is_none_or(|last| now.saturating_sub(*last) >= RETRY_EVERY)
            {
                due.push(Pending { id, letter });
            }
        }
        due.sort_by(|a, b| (a.letter.at, a.id.as_str()).cmp(&(b.letter.at, b.id.as_str())));
        Ok(due)
    }

    /// Records which peer a receipt may come from before returning the mail to send. The owner
    /// lookup picks the peer each round so a moved conversation can change destinations.
    pub fn attempt(
        &mut self,
        id: &Id,
        to: &[u8; 32],
        now: u64,
    ) -> Result<Option<mail::Envelope>, Error> {
        if self.reconcile(id)? {
            return Ok(None);
        }
        let path = self.pending(id);
        let Some(mut letter): Option<Letter> = read(&path)? else {
            return Ok(None);
        };
        if expired(&letter, now)
            || self
                .tries
                .get(id.as_str())
                .is_some_and(|last| now.saturating_sub(*last) < RETRY_EVERY)
        {
            return Ok(None);
        }
        if letter.other.contains_key("call") || letter.other.contains_key("seat") {
            return Ok(None);
        }
        let outgoing = letter_mail(id, &letter, now)?;
        let peer = keys::encode(to);
        if letter.sent_to.as_ref() != Some(&peer) {
            letter.sent_to = Some(peer);
            write(&path, &letter)?;
        }
        self.tries.insert(id.0.clone(), now);
        Ok(Some(outgoing))
    }

    /// Accepts a receipt only from the authenticated static key the mail last went to.
    pub fn settle(
        &mut self,
        from: &[u8; 32],
        receipt: &Delivered,
        now: u64,
    ) -> Result<Settlement, Error> {
        if self.reconcile(&receipt.id)? {
            return Ok(Settlement::Ignored);
        }
        let path = self.pending(&receipt.id);
        let Some(mut letter): Option<Letter> = read(&path)? else {
            return Ok(Settlement::Ignored);
        };
        if letter.sent_to.as_deref() != Some(keys::encode(from).as_str())
            || letter.other.contains_key("call")
            || letter.other.contains_key("seat")
        {
            return Ok(Settlement::Ignored);
        }
        if !receipt.ok && receipt.retry == Some(true) && !expired(&letter, now) {
            if letter.note.is_some()
                && let Some(next) = receipt.lives.as_deref().and_then(keys::decode_exact::<32>)
            {
                letter.peer = Some(keys::encode(&next));
                write(&path, &letter)?;
                if let Err(error) =
                    crate::letters::homes::remember(&self.run, [letter.to.as_str()], &next, now)
                {
                    eprintln!("Remembering where a note was redirected: {error}");
                }
            }
            return Ok(Settlement::Retry);
        }
        let id = &receipt.id;
        let result = if !receipt.ok {
            let (why, said) = if receipt.retry == Some(true) {
                (EXPIRED, false)
            } else {
                let words = receipt.why.as_deref().filter(|why| !why.is_empty());
                (
                    words.map_or("It wasn't accepted there.", |why| utf16_prefix(why, 500)),
                    words.is_some(),
                )
            };
            self.fail(id, letter, Some(why.to_owned()), said)?;
            Settlement::Refused
        } else if letter.wait && receipt.answer != Some(false) {
            relocate(&path, &self.waiting(id))?;
            Settlement::WaitingForAnswer
        } else {
            if letter.wait && receipt.answer == Some(false) {
                let reply = self.replies.join(format!("{}.json", id.as_str()));
                let note = receipt.why.as_deref().map_or(
                    "It got your message, but its answer can't come back to you here.",
                    |why| utf16_prefix(why, 500),
                );
                write(&reply, &json!({"note":note}))?;
            }
            // Keep a small tombstone. Enqueuing the same operation again will not bring it back.
            write(&self.done(id), &now)?;
            remove(&path)?;
            Settlement::Delivered
        };
        self.tries.remove(id.as_str());
        Ok(result)
    }

    /// The answer itself is also a receipt if the delivery acknowledgement went unheard.
    pub(crate) fn answer(
        &mut self,
        from: &[u8; 32],
        id: &Id,
        now: u64,
        reply: impl FnOnce(&Letter) -> Option<Value>,
    ) -> Result<bool, Error> {
        self.settle(
            from,
            &Delivered {
                id: id.clone(),
                ok: true,
                why: None,
                retry: None,
                answer: None,
                lives: None,
            },
            now,
        )?;
        let waiting = self.waiting(id);
        let Some(sent) = read::<Letter>(&waiting)? else {
            return Ok(false);
        };
        if sent.sent_to.as_deref() != Some(keys::encode(from).as_str()) {
            return Ok(false);
        }
        let filed = if let Some(fields) = reply(&sent) {
            write(&self.replies.join(format!("{}.json", id.as_str())), &fields)?;
            true
        } else {
            false
        };
        remove(&waiting)?;
        Ok(filed)
    }

    /// Finishes terminal notices even if the previous process stopped between retiring a send
    /// and handing its notice to the local delivery queue.
    pub fn finish_failures(
        &mut self,
        own: &[u8; 32],
        now: u64,
        mut context: impl FnMut(&Id, &Letter) -> FailureContext,
    ) -> Result<(), Error> {
        for file in fs::read_dir(&self.sending).map_err(|source| storage(&self.sending, source))? {
            let path = file
                .map_err(|source| storage(&self.sending, source))?
                .path();
            if path
                .extension()
                .is_none_or(|extension| extension != "failed")
            {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            let Ok(id) = Id::try_from(stem.to_owned()) else {
                continue;
            };
            self.reconcile(&id)?;
            if self
                .done(&id)
                .try_exists()
                .map_err(|source| storage(&path, source))?
            {
                remove(&path)?;
                continue;
            }
            let (letter, why, said) = match read(&path)? {
                Some(FailedRecord::Recorded(failure)) => {
                    (failure.letter, failure.why, failure.said)
                }
                Some(FailedRecord::Legacy(letter)) => (letter, None, false),
                None => continue,
            };
            let context = context(&id, &letter);
            if letter.wait && context.tool_waiting {
                let reply = self.replies.join(format!("{}.json", id.as_str()));
                write(
                    &reply,
                    &json!({"failed":why.as_deref().unwrap_or("It wasn't delivered.")}),
                )?;
            } else if let Some(sender) = letter.from.as_deref()
                && letter.note.is_none()
                && letter.answers.is_none()
                && letter.read.is_none()
                && !letter.other.contains_key("call")
            {
                let notice_id: Id = format!(
                    "ltr_{}",
                    &keys::encode(&Sha256::digest(format!("{} not delivered", id.as_str())))[..22]
                )
                .try_into()?;
                let name = if context.conversation.is_empty() {
                    "that chat"
                } else {
                    &context.conversation
                };
                let computer = if context.computer.is_empty() {
                    "another computer"
                } else {
                    &context.computer
                };
                let line = format!("Your message to {name} on {computer} wasn't delivered");
                let suffix = match why.as_deref() {
                    Some(why) if said => format!(": {computer} said \"{why}\""),
                    Some(why) => format!(". {why}"),
                    None => ".".into(),
                };
                let text = format!("[Not delivered] {line}{suffix}");
                let note = json!({"from":"Your other computers", "words":text, "line":line, "delivery":"queue"});
                self.enqueue(
                    &notice_id,
                    &Letter {
                        to: sender.to_owned(),
                        text,
                        at: Some(now),
                        peer: Some(keys::encode(own)),
                        note: note.as_object().cloned(),
                        from: None,
                        sent_to: None,
                        wait: false,
                        answers: None,
                        read: None,
                        refused: false,
                        other: Map::new(),
                    },
                )?;
            }
            write(&self.done(&id), &now)?;
            remove(&path)?;
        }
        Ok(())
    }

    fn fail(
        &mut self,
        id: &Id,
        letter: Letter,
        why: Option<String>,
        said: bool,
    ) -> Result<(), Error> {
        // A restart between these two operations sees the terminal state before it sends.
        write(&self.failed(id), &Failure { letter, why, said })?;
        remove(&self.pending(id))?;
        self.tries.remove(id.as_str());
        Ok(())
    }

    fn pending(&self, id: &Id) -> PathBuf {
        self.sending.join(format!("{}.json", id.as_str()))
    }
    fn failed(&self, id: &Id) -> PathBuf {
        self.sending.join(format!("{}.failed", id.as_str()))
    }
    fn waiting(&self, id: &Id) -> PathBuf {
        self.replies.join(format!("{}.wait", id.as_str()))
    }
    fn done(&self, id: &Id) -> PathBuf {
        self.sending.join(format!("{}.done", id.as_str()))
    }

    fn reconcile(&self, id: &Id) -> Result<bool, Error> {
        for sibling in [self.done(id), self.failed(id), self.waiting(id)] {
            if sibling
                .try_exists()
                .map_err(|source| storage(&sibling, source))?
            {
                remove(&self.pending(id))?;
                return Ok(true);
            }
        }
        Ok(false)
    }
}

fn expired(letter: &Letter, now: u64) -> bool {
    now.saturating_sub(letter.at.unwrap_or(now)) > memory(letter.note.is_some())
}

fn memory(note: bool) -> u64 {
    if note { NOTE_MEMORY } else { LETTER_MEMORY }
}

fn letter_mail(id: &Id, letter: &Letter, now: u64) -> Result<mail::Envelope, Error> {
    let at = letter.at.unwrap_or(now);
    let text = utf16_prefix(&letter.text, 32_000);
    for max in [24_000, 6_000, 2_000] {
        let mut fields = Map::new();
        fields.insert("id".into(), json!(id));
        if let Some(from) = &letter.from {
            fields.insert("from".into(), json!(from));
        }
        fields.insert("to".into(), json!(letter.to));
        fields.insert("text".into(), json!(utf8_prefix(text, max)));
        fields.insert("wait".into(), json!(letter.wait));
        fields.insert(
            "at".into(),
            json!(if letter.note.is_some() { now } else { at }),
        );
        if let Some(answers) = &letter.answers {
            fields.insert("answers".into(), json!(answers));
        }
        if let Some(read) = &letter.read {
            let read = read
                .get("query")
                .and_then(Value::as_str)
                .map_or_else(|| json!({}), |query| json!({ "query": query }));
            fields.insert("read".into(), read);
        }
        if let Some(note) = &letter.note {
            let mut note = note.clone();
            for field in ["words", "line"] {
                if let Some(value) = note.get(field).and_then(Value::as_str) {
                    note.insert(field.into(), json!(utf8_prefix(value, max)));
                }
            }
            fields.insert("note".into(), json!(note));
            fields.insert("made".into(), json!(at));
        }
        if letter.refused {
            fields.insert("refused".into(), json!(true));
        }
        let mail = envelope(json!({ "letter": fields, "at": now }));
        if mail.encode().len() <= mail::MAX_PLAINTEXT {
            return Ok(mail);
        }
    }
    Err(Error::Size)
}

fn utf8_prefix(text: &str, max: usize) -> &str {
    &text[..text.floor_char_boundary(max.min(text.len()))]
}

fn utf16_prefix(text: &str, max: usize) -> &str {
    let mut units = 0;
    for (at, ch) in text.char_indices() {
        units += ch.len_utf16();
        if units > max {
            return &text[..at];
        }
    }
    text
}

fn envelope(value: Value) -> mail::Envelope {
    let Value::Object(fields) = value else {
        unreachable!("mail fields are an object")
    };
    mail::Envelope(fields)
}

/// The engine/bridge receives the same prompt ID if it already accepted a letter before a
/// restart. It must submit using its normal idempotency boundary. Only an accepted prompt may
/// become a positive delivery receipt.
pub struct Receipts {
    path: PathBuf,
    _writer: File,
    clock: (u64, u64),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Delivery {
    Seen,
    Submit { prompt: String },
}

impl Receipts {
    pub fn open(run: &Path) -> Result<Self, Error> {
        fs::create_dir_all(run).map_err(|source| storage(run, source))?;
        let writer = lock(&run.join("letters-seen.lock"))?;
        Ok(Self {
            path: run.join("letters-seen.json"),
            _writer: writer,
            clock: (0, 0),
        })
    }

    pub fn begin(&mut self, id: &Id, note: bool, now: u64) -> Result<Delivery, Error> {
        let mut seen = self.read(now)?;
        let id = id.as_str();
        if seen.contains_key(id) || seen.contains_key(&format!("note:{id}")) {
            return Ok(Delivery::Seen);
        }
        let prefix = if note {
            format!("prompt:note:{id}:")
        } else {
            format!("prompt:{id}:")
        };
        if let Some(prompt) = seen.keys().find_map(|key| key.strip_prefix(&prefix)) {
            return Ok(Delivery::Submit {
                prompt: prompt.to_owned(),
            });
        }
        // Match the engine's ascending message IDs (48-bit clock plus a per-millisecond counter).
        let mut random = [0; 14];
        getrandom::fill(&mut random).map_err(Error::Random)?;
        self.clock = (
            now,
            if self.clock.0 == now {
                self.clock.1 + 1
            } else {
                1
            },
        );
        let prefix_hex = format!("{:012x}", ((now << 12) + self.clock.1) & 0xffff_ffff_ffff);
        let alphabet = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
        let suffix: String = random
            .into_iter()
            .map(|byte| char::from(alphabet[usize::from(byte) % alphabet.len()]))
            .collect();
        let prompt = format!("msg_{prefix_hex}{suffix}");
        seen.insert(format!("{prefix}{prompt}"), now);
        write(&self.path, &seen)?;
        Ok(Delivery::Submit { prompt })
    }

    pub(crate) fn seen(&self, id: &Id, now: u64) -> Result<bool, Error> {
        let seen = self.read(now)?;
        Ok(seen.contains_key(id.as_str()) || seen.contains_key(&format!("note:{}", id.as_str())))
    }

    pub fn accepted(&mut self, id: &Id, note: bool, now: u64) -> Result<(), Error> {
        let mut seen = self.read(now)?;
        let key = if note {
            format!("note:{}", id.as_str())
        } else {
            id.0.clone()
        };
        seen.insert(key, now);
        write(&self.path, &seen)
    }

    fn read(&self, now: u64) -> Result<BTreeMap<String, u64>, Error> {
        let mut seen: BTreeMap<String, u64> = read(&self.path)?.unwrap_or_default();
        seen.retain(|key, at| {
            now.saturating_sub(*at)
                <= memory(key.starts_with("note:") || key.starts_with("prompt:note:"))
        });
        Ok(seen)
    }
}

fn lock(path: &Path) -> Result<File, Error> {
    kuvo_store::locks::try_file_lock(path).map_err(|source| storage(path, source))
}

fn storage(path: &Path, source: io::Error) -> Error {
    Error::Storage {
        path: path.to_owned(),
        source,
    }
}

fn read<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, Error> {
    match File::open(path) {
        Ok(file) => serde_json::from_reader(file)
            .map(Some)
            .map_err(|source| Error::Json {
                path: path.to_owned(),
                source,
            }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(storage(path, source)),
    }
}

fn write(path: &Path, value: &impl Serialize) -> Result<(), Error> {
    let bytes = serde_json::to_vec(value).map_err(|source| Error::Json {
        path: path.to_owned(),
        source,
    })?;
    kuvo_store::files::write_atomic(path, &bytes).map_err(|source| storage(path, source))
}

fn relocate(from: &Path, to: &Path) -> Result<(), Error> {
    kuvo_store::files::replace_synced(from, to).map_err(|source| storage(from, source))
}

fn remove(path: &Path) -> Result<(), Error> {
    match fs::remove_file(path) {
        Ok(()) => sync_parent(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(storage(path, source)),
    }
}

fn sync_parent(path: &Path) -> Result<(), Error> {
    kuvo_store::files::sync_parent(path).map_err(|source| storage(path, source))
}
