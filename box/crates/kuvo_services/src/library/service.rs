//! This computer's Library service: bot metadata lookup, artifact copy fallback,
//! thumbnails, and one-time backfill (`library/backfill.json`), matching `library/service.ts` 1:1.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use jiff::Timestamp;
use serde_json::{Map, Value};
use url::Url;

use super::{
    store::{Filter, Handover, How, Item, LibraryStore, LibraryStoreError},
    thumbnails::{Thumbnail, ThumbnailPlatform, Thumbnails},
};
use crate::artifacts::{
    kinds::strip_folder_prefix,
    prepare::{read_saved_session_headers, saved_messages},
    recorder::{attached_paths, tool_name},
    store::{ArtifactStore, COPY_LIMIT, Clock, Scope, parse_seconds},
};

const BACKFILL_VERSION: u64 = 1;
const BOT_CACHE_TTL: Duration = Duration::from_secs(30);

#[derive(Clone, Default)]
pub struct BotDescription {
    pub name: Option<String>,
    pub look: Option<Map<String, Value>>,
}

struct CachedBot {
    desc: BotDescription,
    at: Instant,
}

#[derive(Clone)]
pub struct LibraryOptions {
    pub bots: PathBuf,
    pub sessions: PathBuf,
    pub artifacts: Option<ArtifactStore>,
    pub computer: Arc<dyn Fn() -> String + Send + Sync>,
    pub platform: ThumbnailPlatform,
    pub clock: Option<Clock>,
}

#[derive(Clone)]
pub struct LibraryService {
    root: PathBuf,
    store: LibraryStore,
    thumbnails: Thumbnails,
    bots: PathBuf,
    sessions: PathBuf,
    artifacts: Option<ArtifactStore>,
    computer: Arc<dyn Fn() -> String + Send + Sync>,
    described: Arc<Mutex<HashMap<String, CachedBot>>>,
}

impl LibraryService {
    pub fn open(
        root: impl AsRef<Path>,
        options: LibraryOptions,
    ) -> Result<Self, LibraryStoreError> {
        let root = root.as_ref().to_path_buf();
        let store = LibraryStore::open(&root, Some(options.bots.clone()), options.clock)?;
        let thumbnails = Thumbnails::new(root.join("thumbnails"), options.platform);
        Ok(Self {
            root,
            store,
            thumbnails,
            bots: options.bots,
            sessions: options.sessions,
            artifacts: options.artifacts,
            computer: options.computer,
            described: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn store(&self) -> &LibraryStore {
        &self.store
    }

    pub fn handed(&self, handover: Handover) {
        let _ = self.store.hand(handover);
    }

    pub fn computer_name(&self) -> String {
        (self.computer)()
    }

    pub fn bot(&self, id: &str) -> BotDescription {
        let now = Instant::now();
        if let Some(known) = self.described.lock().unwrap().get(id)
            && now.duration_since(known.at) < BOT_CACHE_TTL
        {
            return known.desc.clone();
        }
        let mut desc = BotDescription::default();
        if let Ok(raw) = fs::read_to_string(self.bots.join(id).join("bot.json"))
            && let Ok(val) = serde_json::from_str::<Value>(&raw)
        {
            if let Some(name) = val.get("name").and_then(Value::as_str) {
                desc.name = Some(name.to_owned());
            }
            if let Some(look) = val.get("look").and_then(Value::as_object) {
                let mut filtered = Map::new();
                for key in ["color", "shape", "face"] {
                    if let Some(v) = look.get(key).and_then(Value::as_str) {
                        filtered.insert(key.to_owned(), Value::String(v.to_owned()));
                    }
                }
                desc.look = Some(filtered);
            }
        }
        self.described.lock().unwrap().insert(
            id.to_owned(),
            CachedBot {
                desc: desc.clone(),
                at: now,
            },
        );
        desc
    }

    pub fn artifact_copy(&self, item: &Item) -> Option<Vec<u8>> {
        let artifacts = self.artifacts.as_ref()?;
        let artifact = artifacts.at(&item.path)?;
        let latest = artifact.versions.iter().rev().find(|v| v.kept)?;
        artifacts.bytes(&artifact.id, latest.n).flatten()
    }

    pub fn bytes(&self, item: &Item) -> Option<Vec<u8>> {
        if let Ok(meta) = fs::metadata(&item.path)
            && meta.is_file()
        {
            return if meta.len() <= COPY_LIMIT {
                fs::read(&item.path).ok()
            } else {
                None
            };
        }
        self.artifact_copy(item)
    }

    pub fn there(&self, item: &Item) -> bool {
        if let Ok(meta) = fs::metadata(&item.path)
            && meta.is_file()
        {
            return true;
        }
        self.artifacts
            .as_ref()
            .and_then(|a| a.at(&item.path))
            .is_some_and(|a| a.versions.iter().any(|v| v.kept))
    }

    pub fn thumbnailable(&self, item: &Item) -> bool {
        self.thumbnails.can(item)
    }

    pub async fn thumbnail(&self, item: &Item) -> Option<Thumbnail> {
        self.thumbnails.of(item, || self.artifact_copy(item)).await
    }

    pub fn client_item(&self, item: &Item) -> Value {
        let there = self.there(item);
        let thumbnail = there && self.thumbnailable(item);
        let mut obj = Map::new();
        obj.insert("id".into(), Value::String(item.id.clone()));
        obj.insert("name".into(), Value::String(item.name.clone()));
        obj.insert("kind".into(), serde_json::to_value(item.kind).unwrap());
        obj.insert("type".into(), Value::String(item.mime.clone()));
        obj.insert("how".into(), serde_json::to_value(item.how).unwrap());
        obj.insert("path".into(), Value::String(item.path.clone()));
        if let Some(bot) = &item.bot {
            obj.insert("bot".into(), Value::String(bot.clone()));
        }
        if let Some(session) = &item.session {
            obj.insert("session".into(), Value::String(session.clone()));
        }
        if let Some(group) = &item.group {
            obj.insert("group".into(), Value::String(group.clone()));
        }
        if let Some(message) = &item.message {
            obj.insert("message".into(), Value::String(message.clone()));
        }
        if let Some(part) = &item.part {
            obj.insert("part".into(), Value::String(part.clone()));
        }
        obj.insert(
            "at".into(),
            Value::from(parse_seconds(&item.at).unwrap_or(0.0)),
        );
        if let Some(hidden) = &item.hidden
            && let Some(secs) = parse_seconds(hidden)
        {
            obj.insert("hidden".into(), Value::from(secs));
        }
        obj.insert("there".into(), Value::Bool(there));
        obj.insert("thumbnail".into(), Value::Bool(thumbnail));
        Value::Object(obj)
    }

    pub fn sources_json(&self) -> Value {
        let (bot_ids, you) = self.store.sources();
        let bots: Vec<Value> = bot_ids
            .into_iter()
            .map(|id| {
                let desc = self.bot(&id);
                let mut obj = Map::new();
                obj.insert("id".into(), Value::String(id));
                obj.insert("name".into(), desc.name.map_or(Value::Null, Value::String));
                if let Some(look) = desc.look {
                    obj.insert("look".into(), Value::Object(look));
                }
                Value::Object(obj)
            })
            .collect();
        serde_json::json!({ "bots": bots, "you": you })
    }

    pub fn listed_local(&self, filter: &Filter, limit: usize) -> Value {
        let computer = self.computer_name();
        let all = self.store.list(filter, |id| self.bot(id).name, &computer);
        let more = all.len() > limit;
        let items: Vec<Value> = all
            .into_iter()
            .take(limit)
            .map(|item| self.client_item(&item))
            .collect();
        serde_json::json!({
            "items": items,
            "more": more,
            "sources": self.sources_json(),
        })
    }

    pub fn peer_copy_json(&self) -> Value {
        let computer = self.computer_name();
        let mut items = self
            .store
            .list(&Filter::default(), |id| self.bot(id).name, &computer);
        items.extend(self.store.list(
            &Filter {
                hidden: true,
                ..Default::default()
            },
            |id| self.bot(id).name,
            &computer,
        ));
        let mut bot_names = Map::new();
        let client_items: Vec<Value> = items
            .iter()
            .map(|item| {
                if let Some(bot) = &item.bot
                    && !bot_names.contains_key(bot)
                {
                    bot_names.insert(
                        bot.clone(),
                        Value::String(self.bot(bot).name.unwrap_or_default()),
                    );
                }
                self.client_item(item)
            })
            .collect();
        serde_json::json!({
            "items": client_items,
            "sources": self.sources_json(),
            "botNames": Value::Object(bot_names),
        })
    }

    /// One-time backfill from saved conversations and artifact timeline.
    pub fn backfill(&self) {
        if is_backfilled(&self.root) {
            return;
        }
        let mut found = library_handovers(&self.sessions, &self.bots);
        if let Some(artifacts) = &self.artifacts {
            let (entries, _) = artifacts.timeline(&Scope::default(), 100_000, None);
            for (artifact, version) in entries {
                let how = if version.via.iter().any(|v| v == "show_file") {
                    How::Shown
                } else if version.via.iter().any(|v| v == "generate_image") {
                    How::Generated
                } else {
                    continue;
                };
                let bot = version
                    .by
                    .bot
                    .clone()
                    .or_else(|| artifact.where_.bots.first().cloned());
                let session = version
                    .turn
                    .as_ref()
                    .map(|t| t.session.clone())
                    .or_else(|| artifact.where_.sessions.first().cloned());
                let group = version.group.as_ref().map(|g| g.id.clone());
                let at = version.at.parse::<Timestamp>().ok();
                found.push(Handover {
                    path: artifact.path,
                    how,
                    bot,
                    session: if group.is_some() { None } else { session },
                    group,
                    message: version.turn.and_then(|t| t.message),
                    part: None,
                    at,
                });
            }
        }
        found.sort_by_key(|h| h.at.map_or(0, Timestamp::as_millisecond));
        for handover in found {
            let exists = Path::new(&handover.path).exists()
                || self
                    .artifacts
                    .as_ref()
                    .and_then(|a| a.at(&handover.path))
                    .is_some_and(|a| a.versions.iter().any(|v| v.kept));
            if exists {
                self.store.hand(handover);
            }
        }
        let _ = fs::write(
            self.root.join("backfill.json"),
            serde_json::json!({ "done": BACKFILL_VERSION }).to_string(),
        );
    }
}

fn is_backfilled(root: &Path) -> bool {
    fs::read_to_string(root.join("backfill.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v| v.get("done").and_then(Value::as_u64))
        .is_some_and(|done| done >= BACKFILL_VERSION)
}

pub fn library_handovers(sessions_dir: &Path, bots_dir: &Path) -> Vec<Handover> {
    let headers = read_saved_session_headers(sessions_dir);
    let parents: HashMap<String, Option<String>> = headers
        .iter()
        .map(|h| (h.id.clone(), h.parent_id.clone()))
        .collect();
    let root_of = |id: &str| -> String {
        let mut at = id.to_owned();
        for _ in 0..8 {
            if let Some(Some(parent)) = parents.get(&at) {
                at = parent.clone();
            } else {
                break;
            }
        }
        at
    };
    let bots_str = bots_dir.to_string_lossy().into_owned();
    let mut found = Vec::new();
    for session in headers {
        let directory = session.directory.as_deref();
        let kuvo = session.metadata.as_ref().and_then(|m| m.get("kuvo"));
        let seat = kuvo.and_then(|k| k.get("seat"));
        let bot = seat
            .and_then(|s| s.get("bot"))
            .and_then(Value::as_str)
            .or_else(|| kuvo.and_then(|k| k.get("bot")).and_then(Value::as_str))
            .map(str::to_owned)
            .or_else(|| {
                directory.and_then(|dir| {
                    let rel = strip_folder_prefix(dir, &bots_str)?;
                    let first = rel.split(std::path::MAIN_SEPARATOR).next()?;
                    first.starts_with("bot_").then(|| first.to_owned())
                })
            });
        let top = root_of(&session.id);
        let own = top == session.id;
        let group = seat
            .and_then(|s| s.get("group"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let target_session = if group.is_some() { None } else { Some(top) };
        let absolute = |raw: &str| -> String {
            let p = Path::new(raw);
            if p.is_absolute() {
                raw.to_owned()
            } else {
                Path::new(directory.unwrap_or("/"))
                    .join(p)
                    .to_string_lossy()
                    .into_owned()
            }
        };

        for message in saved_messages(&session.folder) {
            let at_ms = message
                .pointer("/time/completed")
                .or_else(|| message.pointer("/time/created"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let at = Timestamp::from_millisecond(at_ms).ok();
            let msg_id = message
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_owned);
            match message.get("type").and_then(Value::as_str) {
                Some("user") if own => {
                    for path in attached_paths(&message) {
                        found.push(Handover {
                            path,
                            how: How::Attached,
                            bot: bot.clone(),
                            session: target_session.clone(),
                            group: group.clone(),
                            message: msg_id.clone(),
                            part: None,
                            at,
                        });
                    }
                }
                Some("assistant") => {
                    let Some(content) = message.get("content").and_then(Value::as_array) else {
                        continue;
                    };
                    let cited_prefix = format!("cited-{}-", msg_id.as_deref().unwrap_or(""));
                    for part in content {
                        if part.get("type").and_then(Value::as_str) != Some("tool") {
                            continue;
                        }
                        let part_id = part.get("id").and_then(Value::as_str).unwrap_or("");
                        let name = tool_name(part.get("name").and_then(Value::as_str).unwrap_or(""));
                        let anchor_msg = if own { msg_id.clone() } else { None };
                        let anchor_part = if own && !part_id.is_empty() {
                            Some(part_id.to_owned())
                        } else {
                            None
                        };
                        if (name == "show_file" || part_id.starts_with(&cited_prefix))
                            && let Some(raw) =
                                part.pointer("/state/input/path").and_then(Value::as_str)
                        {
                            found.push(Handover {
                                path: absolute(raw),
                                how: How::Shown,
                                bot: bot.clone(),
                                session: target_session.clone(),
                                group: group.clone(),
                                message: anchor_msg.clone(),
                                part: anchor_part.clone(),
                                at,
                            });
                        }
                        if name == "generate_image"
                            && let Some(outputs) =
                                part.pointer("/state/content").and_then(Value::as_array)
                        {
                            for output in outputs {
                                if output.get("type").and_then(Value::as_str) != Some("file") {
                                    continue;
                                }
                                let Some(uri) = output.get("uri").and_then(Value::as_str) else {
                                    continue;
                                };
                                if !uri.starts_with("file:") {
                                    continue;
                                }
                                if let Ok(url) = Url::parse(uri)
                                    && let Ok(file_path) = url.to_file_path()
                                {
                                    found.push(Handover {
                                        path: file_path.to_string_lossy().into_owned(),
                                        how: How::Generated,
                                        bot: bot.clone(),
                                        session: target_session.clone(),
                                        group: group.clone(),
                                        message: anchor_msg.clone(),
                                        part: anchor_part.clone(),
                                        at,
                                    });
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    found
}
