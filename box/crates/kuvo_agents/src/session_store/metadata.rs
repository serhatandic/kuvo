//! Metadata updates preserve fields introduced by other box versions.

use super::{FORMAT, SessionStore, TRANSCRIPT, read_header};
use kuvo_store::{files, sqlite};
use serde_json::{Map, Value};
use std::{
    io::{self, ErrorKind},
    time::{Duration, SystemTime},
};

/// What a client may change on a saved conversation's header.
#[derive(Debug, Default)]
pub struct SessionPatch {
    pub title: Option<String>,
    pub permissions: Option<Value>,
    pub metadata: Option<Map<String, Value>>,
}

/// A new conversation, as the bridge makes one.
#[derive(Debug)]
pub struct NewSession {
    pub id: String,
    pub directory: String,
    pub kind: crate::accounts::AgentKind,
    pub agent: Option<String>,
    pub model: Value,
    pub title: Option<String>,
    pub permissions: Option<Value>,
    pub metadata: Map<String, Value>,
}

/// A replacement for the header's metadata that keeps the bridge's own notes.
fn kept_notes(before: Option<&Value>, mut given: Map<String, Value>) -> Value {
    let agent = before.and_then(|notes| notes.get("kuvoAgent")).cloned();
    let messages = before
        .and_then(|notes| notes.get("kuvo"))
        .and_then(|kuvo| kuvo.get("messages"))
        .cloned();
    match agent {
        Some(agent) => {
            given.insert("kuvoAgent".into(), agent);
        }
        None => {
            given.shift_remove("kuvoAgent");
        }
    }
    if let Some(messages) = messages {
        // Replaced in place, so the key keeps the position the client gave it.
        let kuvo = given
            .entry("kuvo")
            .or_insert_with(|| Value::Object(Map::new()));
        if !kuvo.is_object() {
            *kuvo = Value::Object(Map::new());
        }
        if let Some(kuvo) = kuvo.as_object_mut() {
            kuvo.insert("messages".into(), messages);
        }
    }
    Value::Object(given)
}

impl SessionStore {
    pub fn make_id(prefix: &str) -> io::Result<String> {
        let millis = SystemTime::UNIX_EPOCH
            .elapsed()
            .map_err(io::Error::other)?
            .as_millis();
        let mut random = [0; 9];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        Ok(format!("{prefix}_{millis:012x}{}", hex::encode(random)))
    }

    pub async fn create(&self, header: Map<String, Value>) -> io::Result<()> {
        let id = header
            .get("id")
            .and_then(Value::as_str)
            .ok_or(ErrorKind::InvalidInput)?;
        let folder = self.folder(id).ok_or(ErrorKind::InvalidInput)?;
        tokio::task::spawn_blocking(move || {
            files::write_new(
                &folder.join("session.json"),
                &serde_json::to_vec(&FORMAT.encode(header))?,
            )
        })
        .await?
    }

    /// Save a new conversation's header. Answers it as saved.
    pub async fn begin(&self, new: NewSession) -> io::Result<Map<String, Value>> {
        use sha1::{Digest, Sha1};
        let now = crate::turn_projection::now();
        let project = hex::encode(Sha1::digest(new.directory.as_bytes()));
        let mut header = Map::new();
        header.insert("id".into(), new.id.into());
        header.insert("projectID".into(), format!("prj_{}", &project[..16]).into());
        header.insert(
            "agent".into(),
            new.agent
                .filter(|agent| !agent.is_empty())
                .unwrap_or_else(|| "build".into())
                .into(),
        );
        header.insert("model".into(), new.model);
        header.insert("cost".into(), 0.into());
        header.insert(
            "tokens".into(),
            serde_json::json!({"input":0,"output":0,"reasoning":0,"cache":{"read":0,"write":0}}),
        );
        header.insert(
            "time".into(),
            serde_json::json!({ "created": now, "updated": now }),
        );
        if let Some(title) = new.title {
            header.insert("title".into(), title.into());
        }
        header.insert(
            "location".into(),
            serde_json::json!({ "directory": new.directory }),
        );
        if let Some(permissions) = new.permissions {
            header.insert("permissions".into(), permissions);
        }
        let mut metadata = new.metadata;
        metadata.insert("kuvoAgent".into(), new.kind.name().into());
        header.insert("metadata".into(), Value::Object(metadata));
        self.create(header.clone()).await?;
        Ok(header)
    }

    /// The conversation's mode (`build`, `plan`, or an agent of yours) from its
    /// next message on, with a row that says it changed. False when it isn't here.
    pub async fn select_agent(&self, id: &str, agent: &str) -> io::Result<bool> {
        let mode = if agent.is_empty() { "build" } else { agent }.to_owned();
        let Some(header) = self.read(id).await? else {
            return Ok(false);
        };
        let previous = header.into_fields().get("agent").cloned();
        if previous.as_ref().and_then(Value::as_str) == Some(mode.as_str()) {
            return Ok(true);
        }
        let chosen = mode.clone();
        if !self
            .change_header(id, move |header| {
                header.insert("agent".into(), Value::String(chosen));
            })
            .await?
        {
            return Ok(false);
        }
        let mut row = serde_json::json!({
            "type":"agent-switched", "id":Self::make_id("msg")?,
            "time":{"created":crate::turn_projection::now()}, "agent":mode,
        });
        if let Some(previous) = previous {
            row["previous"] = previous;
        }
        self.write_messages(id, vec![row]).await
    }

    pub(crate) async fn change_header(
        &self,
        id: &str,
        edit: impl FnOnce(&mut Map<String, Value>) + Send + 'static,
    ) -> io::Result<bool> {
        let session = id.to_owned();
        self.mutate_existing(id, move |folder| {
            let mut header = read_header(folder, &session)?
                .ok_or_else(|| {
                    io::Error::new(ErrorKind::NotFound, "The coding-agent session is gone.")
                })?
                .into_fields();
            edit(&mut header);
            files::write_atomic(
                &folder.join("session.json"),
                &serde_json::to_vec(&FORMAT.encode(header))?,
            )
        })
        .await
    }

    /// Keep how much a conversation's model holds, as its agent's list says, until a turn says:
    /// only while it has none and still runs on `model` (a turn or a model change since wins).
    pub async fn keep_listed_window(&self, id: &str, model: Value, window: u64) -> io::Result<()> {
        self.change_header(id, move |header| {
            let current = header.get("contextModel").or_else(|| header.get("model"));
            let same = current.is_some_and(|current| {
                current["providerID"] == model["providerID"] && current["id"] == model["id"]
            });
            if header.contains_key("contextWindow") || !same || window == 0 {
                return;
            }
            header.insert("contextWindow".into(), window.into());
            if let Some(context) = header.get_mut("context").and_then(Value::as_object_mut) {
                context.insert("window".into(), window.into());
            }
        })
        .await
        .map(|_| ())
    }

    /// Apply a client's change to a saved conversation, as the bridge does: its
    /// own notes (whose agent it is, how a subagent is written to) survive a
    /// metadata replacement. `None` when the conversation isn't here.
    pub async fn update(
        &self,
        id: &str,
        patch: SessionPatch,
    ) -> io::Result<Option<Map<String, Value>>> {
        let now = crate::turn_projection::now();
        let (send, updated) = std::sync::mpsc::channel();
        let changed = self
            .change_header(id, move |header| {
                let SessionPatch {
                    title,
                    permissions,
                    metadata,
                } = patch;
                if let Some(title) = title {
                    header.insert("title".into(), title.into());
                }
                if let Some(permissions) = permissions {
                    header.insert("permissions".into(), permissions);
                }
                if let Some(given) = metadata {
                    header.insert("metadata".into(), kept_notes(header.get("metadata"), given));
                }
                let time = header
                    .entry("time")
                    .or_insert_with(|| Value::Object(Map::new()));
                if let Some(time) = time.as_object_mut() {
                    time.insert("updated".into(), now.into());
                }
                let _ = send.send(header.clone());
            })
            .await?;
        Ok(changed.then(|| updated.try_recv().ok()).flatten())
    }

    /// Its rows whose saved words contain `words`, oldest first, read without the rest.
    pub async fn rows_saying(&self, id: &str, words: &str) -> io::Result<Vec<Value>> {
        self.prepare(id).await;
        self.rows_with(id, words).await
    }

    /// `rows_saying`, as preparing a conversation reads them: without preparing it first.
    pub(crate) async fn rows_with(&self, id: &str, words: &str) -> io::Result<Vec<Value>> {
        let Some(folder) = self.folder(id) else {
            return Ok(Vec::new());
        };
        let words = words.to_owned();
        tokio::task::spawn_blocking(move || {
            let transcript = folder.join(TRANSCRIPT);
            if transcript.try_exists()? {
                let db = sqlite::open_index(&transcript, Duration::from_secs(1))
                    .map_err(io::Error::other)?;
                let mut query = db
                    .prepare("SELECT body FROM message WHERE instr(body, ?1) > 0 ORDER BY position")
                    .map_err(io::Error::other)?;
                let bodies = query
                    .query_map([&words], |row| row.get::<_, String>(0))
                    .map_err(io::Error::other)?;
                return bodies
                    .map(|body| Ok(serde_json::from_str(&body.map_err(io::Error::other)?)?))
                    .collect();
            }
            let Some(raw) = files::read(&folder.join("messages.json"))? else {
                return Ok(Vec::new());
            };
            let rows: Vec<Value> = serde_json::from_slice(&raw)?;
            Ok(rows
                .into_iter()
                .filter(|row| row.to_string().contains(&words))
                .collect())
        })
        .await?
    }

    pub async fn has_message(&self, id: &str, message: &str) -> io::Result<bool> {
        self.prepare(id).await;
        let Some(folder) = self.folder(id) else {
            return Ok(false);
        };
        let message = message.to_owned();
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || {
            if read_header(&folder, &id)?.is_none() {
                return Ok(false);
            }
            let transcript = folder.join(TRANSCRIPT);
            if transcript.try_exists()? {
                let db = sqlite::open_index(&transcript, Duration::from_secs(1))
                    .map_err(io::Error::other)?;
                return db
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM message WHERE id = ?1)",
                        [&message],
                        |row| row.get(0),
                    )
                    .map_err(io::Error::other);
            }
            let Some(raw) = files::read(&folder.join("messages.json"))? else {
                return Ok(false);
            };
            let rows: Vec<Value> = serde_json::from_slice(&raw)?;
            Ok(rows.iter().any(|row| row["id"] == message))
        })
        .await?
    }

    /// Whether a step of `id`'s conversation names `child`'s session, as a
    /// fork's copied steps do.
    pub(crate) async fn shows_subagent(&self, id: &str, child: &str) -> io::Result<bool> {
        let Some(folder) = self.folder(id) else {
            return Ok(false);
        };
        let child = child.to_owned();
        tokio::task::spawn_blocking(move || {
            let names = |body: &Value| {
                body["type"] == "assistant"
                    && body["content"].as_array().is_some_and(|content| {
                        content.iter().any(|part| {
                            part["type"] == "tool"
                                && part["state"]["metadata"]["sessionID"] == child.as_str()
                        })
                    })
            };
            let transcript = folder.join(TRANSCRIPT);
            if transcript.try_exists()? {
                let db = sqlite::open_index(&transcript, Duration::from_secs(1))
                    .map_err(io::Error::other)?;
                let mut statement = db
                    .prepare("SELECT body FROM message WHERE instr(body, ?1) > 0")
                    .map_err(io::Error::other)?;
                let rows = statement
                    .query_map([&child], |row| row.get::<_, String>(0))
                    .map_err(io::Error::other)?;
                for body in rows {
                    if names(&serde_json::from_str(&body.map_err(io::Error::other)?)?) {
                        return Ok(true);
                    }
                }
                return Ok(false);
            }
            let Some(raw) = files::read(&folder.join("messages.json"))? else {
                return Ok(false);
            };
            let rows: Vec<Value> = serde_json::from_slice(&raw)?;
            Ok(rows.iter().any(names))
        })
        .await?
    }

    pub(crate) async fn turn_messages(&self, id: &str, prefix: String) -> io::Result<Vec<Value>> {
        self.prepare(id).await;
        let folder = self.folder(id).ok_or(ErrorKind::InvalidInput)?;
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || {
            if read_header(&folder, &id)?.is_none() {
                return Err(ErrorKind::NotFound.into());
            }
            let file = folder.join(TRANSCRIPT);
            if !file.try_exists()? {
                return Ok(Vec::new());
            }
            let db = sqlite::open_index(&file, Duration::from_secs(1)).map_err(io::Error::other)?;
            let mut statement = db.prepare(
                "SELECT body FROM message WHERE substr(id, 1, length(?1)) = ?1 ORDER BY position",
            ).map_err(io::Error::other)?;
            let rows = statement
                .query_map([prefix], |row| row.get::<_, String>(0))
                .map_err(io::Error::other)?;
            rows.map(|body| Ok(serde_json::from_str(&body.map_err(io::Error::other)?)?))
                .collect()
        })
        .await?
    }
}
