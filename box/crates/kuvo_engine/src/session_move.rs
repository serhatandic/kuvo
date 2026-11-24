//! OpenCode's part of a move. The peer worker owns admission, the journal and
//! the activation decision. This handle pins the engine for the lifetime of the lease.

use std::{
    collections::HashSet,
    fmt, fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use http::{Method, StatusCode};
use kuvo_os::process::ProcessIdentity;
use kuvo_server::Refusal;
use kuvo_store::{files, format::ObjectFormat};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::time::Instant;

use crate::{
    Engine, api,
    lifetime::{Demand, Lease},
    queue,
};

const READ_WAIT: Duration = Duration::from_secs(120);
const EXPORT_WAIT: Duration = Duration::from_secs(600);
const IMPORTED_BY: &str = "kuvo.moveImport";
const JOURNAL: ObjectFormat = ObjectFormat::additive("Move journal");
const DEPARTURE: ObjectFormat = ObjectFormat::additive("Moved conversation");
const RECENT_DEPARTURE: u64 = 2 * 24 * 60 * 60_000;
const HELD: &str = "This record is held while its owner changes.";
pub(crate) const MOVING: &str = "It’s moving to another computer. Try again once it’s there.";

pub(crate) struct ActionFence {
    pub held: bool,
    pub elsewhere: Option<(String, String)>,
}

pub(crate) fn moved_elsewhere(computer: &str, name: &str) -> Refusal {
    let clean = name
        .replace(
            |c: char| {
                matches!(
                    c,
                    '\u{0000}'..='\u{001f}'
                        | '\u{007f}'
                        | '\u{200e}'
                        | '\u{200f}'
                        | '\u{202a}'..='\u{202e}'
                        | '\u{2066}'..='\u{2069}'
                )
            },
            " ",
        )
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let shown = if clean.is_empty() {
        "another of your computers"
    } else {
        clean.as_str()
    };
    Refusal::json(
        StatusCode::CONFLICT,
        json!({
            "message": format!("It’s on {shown} now. Change it there."),
            "computer": computer,
        }),
    )
}

pub(crate) fn check_action(fence: &ActionFence) -> Result<(), Refusal> {
    if let Some((computer, name)) = &fence.elsewhere {
        return Err(moved_elsewhere(computer, name));
    }
    if fence.held {
        return Err(Refusal::new(StatusCode::CONFLICT, HELD));
    }
    Ok(())
}

pub(crate) async fn action_state(engine: &Engine, session: &str) -> Result<ActionFence, Refusal> {
    let mut elsewhere = None;
    let mut held = false;
    if let Some(authority) = &engine.ownership
        && let Ok(scope) = authority.scope(session, None)
    {
        held |= scope.held;
        if let Some(owner) = scope.owner
            && !owner.local
            && let Some(key) = owner.key
        {
            let computer = kuvo_wire::keys::encode(&key);
            let name = if owner.name == computer {
                String::new()
            } else {
                owner.name
            };
            elsewhere = Some((computer, name));
        }
    }
    let run = engine.home.join("run");
    let target = session.to_owned();
    let disk = tokio::task::spawn_blocking(move || {
        Ok::<_, io::Error>((
            journal_held(&run, &target, None, false)?,
            recent_departure(&run, &target)?,
        ))
    })
    .await;
    let (disk_held, departure) = match disk {
        Ok(Ok(found)) => found,
        Ok(Err(error)) => return Err(fence_failure(error)),
        Err(error) => return Err(fence_failure(error)),
    };
    held |= disk_held;
    if elsewhere.is_none()
        && let Some(computer) = departure
    {
        let name = engine
            .ownership
            .as_ref()
            .and_then(|authority| {
                let key = kuvo_wire::keys::decode_exact(&computer)?;
                authority
                    .members()
                    .into_iter()
                    .find(|member| member.key == Some(key))
                    .map(|member| member.name)
            })
            .filter(|name| name != &computer)
            .unwrap_or_default();
        elsewhere = Some((computer, name));
    }
    Ok(ActionFence { held, elsewhere })
}

pub(crate) fn refused(status: StatusCode, body: &Value) -> bool {
    status == StatusCode::CONFLICT && body["message"].as_str() == Some(HELD)
}

pub(crate) fn prompt_target(method: &Method, path: &str) -> Option<String> {
    let mut tail = path.strip_prefix("api/session/")?.split('/');
    let id = percent_encoding::percent_decode_str(tail.next()?)
        .decode_utf8()
        .ok()?;
    if !crate::sessions::valid_id(&id) {
        return None;
    }
    let guarded = match (method, tail.next()?, tail.next()) {
        (&Method::POST, "prompt" | "command" | "shell" | "compact" | "summarize", None) => true,
        (&Method::POST | &Method::DELETE, "permission" | "form", Some(token)) => {
            !token.is_empty()
                && matches!(tail.next(), None | Some("reply"))
                && tail.next().is_none()
        }
        _ => false,
    };
    guarded.then(|| id.into_owned())
}

/// A write registers before looking for a fence. A snapshot taken after a
/// durable fence then either waits for the write or knows it was refused.
pub(crate) struct PromptWrite(PathBuf);

impl Drop for PromptWrite {
    fn drop(&mut self) {
        let _ = files::remove_file_synced(&self.0);
    }
}

#[derive(Deserialize)]
struct Fence {
    id: String,
    thing: String,
    role: String,
    stage: Option<String>,
    sessions: Vec<String>,
    step: Option<String>,
    failed: Option<String>,
    activation: Option<serde::de::IgnoredAny>,
    #[serde(default, rename = "queueSettled")]
    queue_settled: bool,
}

#[derive(Deserialize)]
struct Departure {
    session: String,
    key: String,
    at: u64,
    arrived: Option<bool>,
    queue: Option<String>,
}

pub(crate) async fn admit(
    engine: &Engine,
    session: &str,
    activating: Option<&str>,
) -> Result<PromptWrite, Refusal> {
    admit_write(engine, session, activating, false).await
}

pub(crate) async fn admit_queue_hold(
    engine: &Engine,
    session: &str,
) -> Result<PromptWrite, Refusal> {
    admit_write(engine, session, None, true).await
}

async fn admit_write(
    engine: &Engine,
    session: &str,
    activating: Option<&str>,
    queue_hold: bool,
) -> Result<PromptWrite, Refusal> {
    let run = engine.home.join("run");
    let session = session.to_owned();
    let activating = activating.map(str::to_owned);
    let admission = tokio::task::spawn_blocking(move || {
        let mut nonce = [0; 16];
        getrandom::fill(&mut nonce).map_err(io::Error::other)?;
        let marker = run.join("move-prompt-writes").join(format!(
            "{}-{}-{}",
            write_prefix(&session),
            std::process::id(),
            hex::encode(nonce),
        ));
        files::write_new(&marker, &[])?;
        let write = PromptWrite(marker);
        if frozen(&run, &session, activating.as_deref(), queue_hold)? {
            Ok::<_, io::Error>(None)
        } else {
            Ok(Some(write))
        }
    })
    .await;
    match admission {
        Ok(Ok(Some(write))) => Ok(write),
        Ok(Ok(None)) => Err(Refusal::new(StatusCode::CONFLICT, HELD)),
        Ok(Err(error)) => Err(fence_failure(error)),
        Err(error) => Err(fence_failure(error)),
    }
}

fn fence_failure(error: impl fmt::Display) -> Refusal {
    eprintln!("Reading move admission: {error}");
    Refusal::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "The computer couldn’t check whether this conversation is moving.",
    )
}

fn entries(path: &Path) -> io::Result<impl Iterator<Item = io::Result<fs::DirEntry>>> {
    let read = match fs::read_dir(path) {
        Ok(read) => Some(read),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    Ok(read.into_iter().flatten())
}

fn frozen(
    run: &Path,
    session: &str,
    activating: Option<&str>,
    queue_hold: bool,
) -> io::Result<bool> {
    Ok(journal_held(run, session, activating, queue_hold)?
        || recent_departure(run, session)?.is_some())
}

fn journal_held(
    run: &Path,
    session: &str,
    activating: Option<&str>,
    queue_hold: bool,
) -> io::Result<bool> {
    let root = run.join("moves");
    for entry in entries(&root)? {
        let entry = entry?;
        let name = entry.file_name();
        if !name
            .to_str()
            .and_then(|name| name.strip_prefix("mov_")?.strip_suffix(".json"))
            .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric()))
        {
            continue;
        }
        let Some(stored) = JOURNAL.read(&entry.path()).map_err(io::Error::other)? else {
            continue;
        };
        let row: Fence = serde_json::from_value(stored.fields.into())?;
        if !matches!(row.role.as_str(), "from" | "to") {
            return Err(io::Error::other("An ownership record could not be read"));
        }
        if row.step.as_deref() == Some("check")
            || row.failed.as_deref().is_some_and(|text| !text.is_empty())
        {
            continue;
        }
        let member = row.sessions.iter().any(|id| id == session);
        let own_activation = activating == Some(row.id.as_str())
            && row.role == "to"
            && row.activation.is_some()
            && member;
        let settling = queue_hold
            && row.role == "from"
            && row.stage.as_deref() == Some("holding")
            && !row.queue_settled;
        if !own_activation && !settling && (row.thing == session || member) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn recent_departure(run: &Path, session: &str) -> io::Result<Option<String>> {
    let Some(raw) = files::read(&run.join("moves/moved.json"))? else {
        return Ok(None);
    };
    let Ok(Value::Array(rows)) = serde_json::from_slice(&raw) else {
        return Ok(None);
    };
    let now = crate::disk::now();
    for row in rows.into_iter().filter(Value::is_object) {
        let fields = DEPARTURE.decode(row).map_err(io::Error::other)?.fields;
        if let Ok(row) = serde_json::from_value::<Departure>(fields.into())
            && row.session == session
            && !row.key.is_empty()
            && row.arrived != Some(true)
            && row.queue.is_none()
            && now.saturating_sub(row.at) < RECENT_DEPARTURE
        {
            return Ok(Some(row.key));
        }
    }
    Ok(None)
}

fn write_prefix(session: &str) -> String {
    hex::encode(Sha256::digest(session.as_bytes()))
}

fn in_flight(folder: &Path, prefixes: &HashSet<String>) -> io::Result<bool> {
    for entry in entries(folder)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some((hash, pid)) = name.to_str().and_then(|name| {
            let (hash, rest) = name.split_once('-')?;
            let (pid, _) = rest.split_once('-')?;
            let pid: u32 = pid.parse().ok()?;
            (pid > 0).then_some((hash, pid))
        }) else {
            continue;
        };
        if !prefixes.contains(hash) {
            continue;
        }
        if ProcessIdentity::read(pid)?.is_some() {
            return Ok(true);
        }
        files::remove_file_synced(&entry.path())?;
    }
    Ok(false)
}

#[derive(Debug)]
pub enum MoveEngineError {
    Unavailable,
    InvalidTree,
    MissingOwnership,
    Busy,
    WaitingQueue,
    UnreadableQueue(Option<io::Error>),
    Storage(io::Error),
    Upstream(Box<dyn std::error::Error + Send + Sync>),
}

pub struct LetterText {
    pub text: String,
    pub metadata: Option<Value>,
}

pub struct LetterPrompt {
    pub id: String,
    pub immediate: LetterText,
    pub when_queued: Option<LetterText>,
    pub from: String,
    pub words: String,
    pub steer: bool,
}

pub enum LetterDelivery {
    Accepted,
    Queued(String),
}

impl fmt::Display for MoveEngineError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => out.write_str("The engine isn’t running."),
            Self::InvalidTree => out.write_str("Its conversations aren’t its own."),
            Self::MissingOwnership => {
                out.write_str("The conversation copy has no import ownership receipt")
            }
            Self::Busy => out.write_str("Its conversation is still running."),
            Self::WaitingQueue => {
                out.write_str("The conversation still has messages on their way.")
            }
            Self::UnreadableQueue(_) => {
                out.write_str("A message waiting for the conversation couldn’t be read.")
            }
            Self::Upstream(error) => write!(out, "{error}"),
            Self::Storage(error) => write!(out, "{error}"),
        }
    }
}

impl std::error::Error for MoveEngineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Upstream(error) => Some(error.as_ref()),
            Self::Storage(error) => Some(error),
            Self::UnreadableQueue(Some(error)) => Some(error),
            _ => None,
        }
    }
}

impl From<api::Error> for MoveEngineError {
    fn from(error: api::Error) -> Self {
        Self::Upstream(Box::new(error))
    }
}

pub struct SessionMove {
    engine: Engine,
    lease: Lease,
}

impl Engine {
    pub async fn session_move(&self) -> Result<SessionMove, MoveEngineError> {
        let lease = self
            .acquire(Demand::Existing)
            .await
            .ok_or(MoveEngineError::Unavailable)?;
        Ok(SessionMove {
            engine: self.clone(),
            lease,
        })
    }
}

impl SessionMove {
    pub async fn recent_messages(
        &self,
        session: &str,
        limit: u8,
    ) -> Result<Option<Vec<Value>>, MoveEngineError> {
        if !valid_session(session) {
            return Err(MoveEngineError::InvalidTree);
        }
        let route = format!("api/session/{session}/message?limit={limit}&order=desc");
        match self
            .engine
            .call(&self.lease, Method::GET, &route, None)
            .await
        {
            Ok(messages) => Ok(Some(messages)),
            Err(error) if error.missing() => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub async fn moved_instruction(
        &self,
        session: &str,
        words: &str,
    ) -> Result<(), MoveEngineError> {
        if !valid_session(session) {
            return Err(MoveEngineError::InvalidTree);
        }
        let route = format!("api/experimental/session/{session}/instructions/entries/kuvo.moved");
        self.engine
            .call_raw(
                &self.lease,
                Method::PUT,
                &route,
                Some(&json!({"value": words})),
                READ_WAIT,
            )
            .await?;
        Ok(())
    }

    /// Deliver the already composed letter through the same session turn and
    /// lookup used to reconcile an accepted prompt whose reply was lost.
    pub async fn deliver_letter(
        &self,
        session: &str,
        letter: LetterPrompt,
    ) -> Result<LetterDelivery, Refusal> {
        let _turn = self.engine.turn(session).await;
        let _admitted = admit(&self.engine, session, None).await?;
        let queue = self
            .engine
            .queues
            .get(session)
            .await
            .map_err(fence_failure)?;
        if queue.contains(&letter.id) {
            return Ok(LetterDelivery::Queued(
                queue.letter_wait_reason(&self.engine.home),
            ));
        }
        if queue::reached(&self.engine, &self.lease, session, &letter.id)
            .await
            .map_err(fence_failure)?
        {
            return Ok(LetterDelivery::Accepted);
        }
        let mut prompt = queue::Prompt {
            id: letter.id,
            text: letter.immediate.text,
            delivery: queue::Delivery::Queue,
            metadata: letter.immediate.metadata,
            files: vec![],
        };
        let queued = letter.when_queued.map(|body| queue::Prompt {
            text: body.text,
            metadata: body.metadata,
            ..prompt.clone()
        });
        if self
            .engine
            .queues
            .keep(
                session,
                queued.as_ref().unwrap_or(&prompt),
                Some(&letter.words),
                Some(letter.from),
                None,
                &[],
            )
            .await
            .map_err(fence_failure)?
        {
            let reason = self
                .engine
                .queues
                .get(session)
                .await
                .unwrap_or(queue)
                .letter_wait_reason(&self.engine.home);
            return Ok(LetterDelivery::Queued(reason));
        }
        if letter.steer {
            prompt.delivery = queue::Delivery::Steer;
        }
        crate::send::post(&self.engine, &self.lease, session, &prompt).await?;
        Ok(LetterDelivery::Accepted)
    }

    pub async fn ready(&self) -> Result<(), MoveEngineError> {
        self.active().await.map(|_| ())
    }

    pub async fn sign_in_needed(&self, provider: &str) -> Result<bool, MoveEngineError> {
        let listed = self
            .engine
            .call::<Value>(&self.lease, Method::GET, "api/provider", None)
            .await?;
        Ok(listed.as_array().is_some_and(|accounts| {
            !accounts.is_empty()
                && !accounts
                    .iter()
                    .any(|row| row["id"].as_str() == Some(provider))
        }))
    }

    pub async fn running(&self, session: &str) -> Result<bool, MoveEngineError> {
        Ok(self
            .active()
            .await?
            .as_object()
            .is_some_and(|active| active.contains_key(session)))
    }

    pub async fn lookup(&self, session: &str) -> Result<Option<Value>, MoveEngineError> {
        if !valid_session(session) {
            return Err(MoveEngineError::InvalidTree);
        }
        let path = format!("api/session/{session}");
        match self
            .engine
            .call::<Value>(&self.lease, Method::GET, &path, None)
            .await
        {
            Ok(row) if row["id"].as_str() == Some(session) => Ok(Some(row)),
            Ok(_) => Err(MoveEngineError::InvalidTree),
            Err(error) if error.missing() => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// The journals already cover this set. Let turns already running end
    /// and let existing prompt writes finish before reading the move's data.
    pub async fn settle(&self, sessions: &[String]) -> Result<(), MoveEngineError> {
        self.settle_writes(sessions).await?;
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let active = self.active().await?;
            let busy = sessions.iter().any(|session| {
                active.get(session).is_some()
                    || active
                        .as_array()
                        .is_some_and(|rows| rows.iter().any(|id| id == session))
            });
            if !busy {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(MoveEngineError::Busy);
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    pub async fn remove_retired(&self, session: &str) -> Result<(), MoveEngineError> {
        if !valid_session(session) {
            return Err(MoveEngineError::InvalidTree);
        }
        let _turn = self.engine.turn(session).await;
        let path = format!("api/session/{session}");
        match self
            .engine
            .call_raw(&self.lease, Method::DELETE, &path, None, READ_WAIT)
            .await
        {
            Ok(_) => {}
            Err(error) if error.missing() => {}
            Err(error) => return Err(error.into()),
        }
        queue::forget_moved(&self.engine, session)
            .await
            .map_err(MoveEngineError::Storage)
    }

    pub async fn queue_snapshot(
        &self,
        sessions: &[String],
    ) -> Result<std::collections::BTreeMap<String, Value>, MoveEngineError> {
        queue::pack_move(&self.engine, &self.lease, sessions).await
    }

    pub async fn place_queues(
        &self,
        sessions: &[String],
        queues: std::collections::BTreeMap<String, Value>,
    ) -> Result<Vec<String>, MoveEngineError> {
        queue::place_move(&self.engine, sessions, queues)
            .await
            .map_err(MoveEngineError::Storage)
    }

    pub async fn forget_queues(&self, session: &str) -> Result<(), MoveEngineError> {
        queue::forget_moved(&self.engine, session)
            .await
            .map_err(MoveEngineError::Storage)
    }

    pub async fn activate_queues(
        &self,
        move_id: &str,
        unpaused: &[String],
    ) -> Result<(), MoveEngineError> {
        for session in unpaused {
            queue::resume_move(&self.engine, &self.lease, session, move_id)
                .await
                .map_err(|error| {
                    MoveEngineError::Storage(io::Error::other(format!(
                        "Activating the move queue: {}",
                        error.reply().status
                    )))
                })?;
        }
        Ok(())
    }

    async fn active(&self) -> Result<Value, MoveEngineError> {
        Ok(self
            .engine
            .call::<Value>(&self.lease, Method::GET, "api/session/active", None)
            .await?)
    }

    /// Call after the move journal fences this set of sessions, before
    /// exporting or comparing with a copy that has arrived.
    pub async fn settle_writes(&self, sessions: &[String]) -> Result<(), MoveEngineError> {
        let folder = std::sync::Arc::new(self.engine.home.join("run/move-prompt-writes"));
        let prefixes = std::sync::Arc::new(
            sessions
                .iter()
                .map(|id| write_prefix(id))
                .collect::<HashSet<_>>(),
        );
        let deadline = Instant::now() + Duration::from_secs(125);
        loop {
            let folder = folder.clone();
            let prefixes = prefixes.clone();
            let busy = tokio::task::spawn_blocking(move || in_flight(&folder, &prefixes))
                .await
                .map_err(|error| MoveEngineError::Storage(io::Error::other(error)))?
                .map_err(MoveEngineError::Storage)?;
            if !busy {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(MoveEngineError::Storage(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "A prompt write has not finished",
                )));
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Export the entire session tree, including paginated child lists. Each
    /// export's own ID and parent must agree with the listing that led to it.
    pub async fn export_tree(&self, root: &str) -> Result<Vec<Value>, MoveEngineError> {
        if !valid_session(root) {
            return Err(MoveEngineError::InvalidTree);
        }
        let mut exports = vec![self.export(root, None).await?];
        let mut seen = HashSet::from([root.to_owned()]);
        let mut parents = vec![root.to_owned()];
        while let Some(parent) = parents.pop() {
            for child in self.children(&parent).await? {
                if !seen.insert(child.clone()) {
                    return Err(MoveEngineError::InvalidTree);
                }
                exports.push(self.export(&child, Some(&parent)).await?);
                parents.push(child);
            }
        }
        Ok(exports)
    }

    /// Import with the move's own receipt in the engine's metadata. The
    /// caller has already journaled the import and reserved the destination.
    pub async fn import(
        &self,
        exported: &Value,
        directory: &Path,
        receipt: &str,
    ) -> Result<(), MoveEngineError> {
        let mut info = exported["info"]
            .as_object()
            .cloned()
            .ok_or(MoveEngineError::InvalidTree)?;
        let id = info
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| valid_session(id))
            .ok_or(MoveEngineError::InvalidTree)?;
        if receipt.is_empty() {
            return Err(MoveEngineError::MissingOwnership);
        }
        let _turn = self.engine.turn(id).await;
        let directory_text = directory.to_str().ok_or(MoveEngineError::InvalidTree)?;
        let mut metadata = info
            .remove("metadata")
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        let chats = self.engine.home.join("chats");
        if directory == chats
            || directory
                .canonicalize()
                .is_ok_and(|actual| chats.canonicalize().is_ok_and(|chats| chats == actual))
        {
            let mut kuvo = metadata
                .remove("kuvo")
                .and_then(|value| value.as_object().cloned())
                .unwrap_or_default();
            kuvo.insert("chat".into(), json!(directory_text));
            metadata.insert("kuvo".into(), kuvo.into());
        } else if let Some(kuvo) = metadata.get_mut("kuvo").and_then(Value::as_object_mut) {
            kuvo.remove("chat");
        }
        metadata.insert(IMPORTED_BY.into(), json!(receipt));
        info.insert("metadata".into(), metadata.into());
        let body = json!({"info": info, "messages": exported["messages"], "location": {"directory": directory_text}});
        self.engine
            .call_raw(
                &self.lease,
                Method::POST,
                "api/experimental/session/import",
                Some(&body),
                READ_WAIT,
            )
            .await?;
        Ok(())
    }

    /// Roll back an uncertain import only after OpenCode proves that this
    /// move created the copy. Returns whether this call removed the import.
    pub async fn discard_import(
        &self,
        session: &str,
        receipt: &str,
    ) -> Result<bool, MoveEngineError> {
        if !valid_session(session) {
            return Err(MoveEngineError::InvalidTree);
        }
        if receipt.is_empty() {
            return Err(MoveEngineError::MissingOwnership);
        }
        let _turn = self.engine.turn(session).await;
        let route = format!("api/session/{session}");
        let page = match self
            .engine
            .call_raw(&self.lease, Method::GET, &route, None, READ_WAIT)
            .await
        {
            Ok(found) => found,
            Err(error) if error.missing() => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        let found = page.get("data").unwrap_or(&page);
        if found["id"].as_str() != Some(session)
            || found["metadata"][IMPORTED_BY].as_str() != Some(receipt)
        {
            return Ok(false);
        }
        match self
            .engine
            .call_raw(&self.lease, Method::DELETE, &route, None, READ_WAIT)
            .await
        {
            Ok(_) => Ok(true),
            Err(error) if error.missing() => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    async fn export(&self, session: &str, parent: Option<&str>) -> Result<Value, MoveEngineError> {
        let route = format!("api/experimental/session/{session}/export");
        let page = self
            .engine
            .call_raw(&self.lease, Method::GET, &route, None, EXPORT_WAIT)
            .await?;
        let export = page.get("data").unwrap_or(&page);
        let info = &export["info"];
        let ancestry = &info["parentID"];
        if info["id"].as_str() != Some(session)
            || !match parent {
                Some(parent) => ancestry.as_str() == Some(parent),
                None => ancestry.is_null(),
            }
        {
            return Err(MoveEngineError::InvalidTree);
        }
        Ok(match page {
            Value::Object(mut fields) => fields.remove("data").unwrap_or_else(|| fields.into()),
            _ => return Err(MoveEngineError::InvalidTree),
        })
    }

    async fn children(&self, parent: &str) -> Result<Vec<String>, MoveEngineError> {
        let mut route = format!("api/session?parentID={parent}&limit=100&order=desc");
        let mut children = Vec::new();
        let mut cursors = HashSet::new();
        loop {
            let page = self
                .engine
                .call_raw(&self.lease, Method::GET, &route, None, READ_WAIT)
                .await?;
            let rows = page["data"]
                .as_array()
                .ok_or(MoveEngineError::InvalidTree)?;
            for row in rows {
                let id = row["id"]
                    .as_str()
                    .filter(|id| valid_session(id))
                    .ok_or(MoveEngineError::InvalidTree)?;
                children.push(id.to_owned());
            }
            let Some(next) = page["cursor"]["next"]
                .as_str()
                .filter(|value| !value.is_empty())
            else {
                return Ok(children);
            };
            if rows.is_empty() || !cursors.insert(next.to_owned()) {
                return Err(MoveEngineError::InvalidTree);
            }
            route = format!(
                "api/session?parentID={parent}&limit=100&cursor={}",
                api::segment(next)
            );
        }
    }
}

fn valid_session(id: &str) -> bool {
    id.strip_prefix("ses_").is_some_and(|suffix| {
        (1..=60).contains(&suffix.len()) && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
    })
}
