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
use serde_json::{Map, Value, json};
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

struct CachedBot { desc: BotDescription, at: Instant }

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
    pub fn open(root: impl AsRef<Path>, options: LibraryOptions) -> Result<Self, LibraryStoreError> {
        let root = root.as_ref().to_path_buf();
        let store = LibraryStore::open(&root, Some(options.bots.clone()), options.clock)?;
        let thumbnails = Thumbnails::new(root.join("thumbnails"), options.platform);
        Ok(Self {
            root, store, thumbnails,
            bots: options.bots,
            sessions: options.sessions,
            artifacts: options.artifacts,
            computer: options.computer,
            described: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn bots_folder(&self) -> &Path { &self.bots }
    pub fn clear_bot_descriptions(&self) { self.described.lock().unwrap().clear(); }
    pub fn store(&self) -> &LibraryStore { &self.store }
    pub fn artifacts_ready(&self) -> bool { self.artifacts.as_ref().is_none_or(|store| store.ensure_history().is_ok()) }
    pub fn handed(&self, handover: Handover) { let _ = self.store.hand(handover); }
    pub fn computer_name(&self) -> String { (self.computer)() }
    pub fn thumbnailable(&self, item: &Item) -> bool { self.thumbnails.can(item) }
    pub async fn thumbnail(&self, item: &Item) -> Option<Thumbnail> { self.thumbnails.of(item, || self.artifact_copy(item)).await }

    pub fn bot(&self, id: &str) -> BotDescription {
        let now = Instant::now();
        if let Some(known) = self.described.lock().unwrap().get(id) && now.duration_since(known.at) < BOT_CACHE_TTL {
            return known.desc.clone();
        }
        let mut desc = BotDescription::default();
        if let Some(val) = kuvo_store::files::text_of(&self.bots.join(id).join("bot.json"))
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        {
            desc.name = val.get("name").and_then(Value::as_str).map(str::to_owned);
            desc.look = val.get("look").and_then(Value::as_object).map(|look| {
                ["color", "shape", "face"].into_iter()
                    .filter_map(|k| Some((k.to_owned(), Value::String(look.get(k)?.as_str()?.to_owned()))))
                    .collect()
            });
        }
        self.described.lock().unwrap().insert(id.to_owned(), CachedBot { desc: desc.clone(), at: now });
        desc
    }

    pub fn artifact_copy(&self, item: &Item) -> Option<Vec<u8>> {
        let artifacts = self.artifacts.as_ref()?;
        let artifact = artifacts.at(&item.path)?;
        let latest = artifact.versions.iter().rev().find(|v| v.kept)?;
        artifacts.bytes(&artifact.id, latest.n).flatten()
    }

    pub fn bytes(&self, item: &Item) -> Option<Vec<u8>> {
        if let Ok(meta) = fs::metadata(&item.path) && meta.is_file() {
            return (meta.len() <= COPY_LIMIT).then(|| fs::read(&item.path).ok()).flatten();
        }
        self.artifact_copy(item)
    }

    pub fn there(&self, item: &Item) -> bool {
        fs::metadata(&item.path).is_ok_and(|m| m.is_file())
            || self.artifacts.as_ref().and_then(|a| a.at(&item.path)).is_some_and(|a| a.versions.iter().any(|v| v.kept))
    }

    pub fn client_item(&self, item: &Item) -> Value {
        let there = self.there(item);
        let mut val = serde_json::to_value(item).unwrap();
        let obj = val.as_object_mut().unwrap();
        obj.insert("at".into(), json!(parse_seconds(&item.at).unwrap_or(0.0)));
        if let Some(secs) = item.hidden.as_deref().and_then(parse_seconds) {
            obj.insert("hidden".into(), json!(secs));
        } else {
            obj.shift_remove("hidden");
        }
        obj.insert("there".into(), json!(there));
        obj.insert("thumbnail".into(), json!(there && self.thumbnailable(item)));
        val
    }

    pub fn sources_json(&self) -> Value {
        let (bot_ids, you) = self.store.sources();
        let bots: Vec<Value> = bot_ids.into_iter().map(|id| {
            let desc = self.bot(&id);
            let mut v = json!({ "id": id, "name": desc.name });
            if let Some(look) = desc.look { v["look"] = Value::Object(look); }
            v
        }).collect();
        json!({ "bots": bots, "you": you })
    }

    pub fn listed_local(&self, filter: &Filter, limit: usize) -> Value {
        let computer = self.computer_name();
        let all = self.store.list(filter, |id| self.bot(id).name, &computer);
        let more = all.len() > limit;
        let items: Vec<Value> = all.into_iter().take(limit).map(|item| self.client_item(&item)).collect();
        json!({ "items": items, "more": more, "sources": self.sources_json() })
    }

    pub fn peer_copy_json(&self) -> Value {
        let computer = self.computer_name();
        let mut items = self.store.list(&Filter::default(), |id| self.bot(id).name, &computer);
        items.extend(self.store.list(&Filter { hidden: true, ..Default::default() }, |id| self.bot(id).name, &computer));
        let mut bot_names = Map::new();
        let client_items: Vec<Value> = items.iter().map(|item| {
            if let Some(bot) = &item.bot && !bot_names.contains_key(bot) {
                bot_names.insert(bot.clone(), json!(self.bot(bot).name.unwrap_or_default()));
            }
            self.client_item(item)
        }).collect();
        json!({ "items": client_items, "sources": self.sources_json(), "botNames": Value::Object(bot_names) })
    }

    /// One-time backfill from saved conversations and artifact timeline.
    pub fn backfill(&self) {
        let done = kuvo_store::files::text_of(&self.root.join("backfill.json"))
            .and_then(|r| serde_json::from_str::<Value>(&r).ok())
            .and_then(|v| v.get("done")?.as_u64())
            .is_some_and(|d| d >= BACKFILL_VERSION);
        if done || !self.artifacts_ready() { return; }
        let mut found = library_handovers(&self.sessions, &self.bots);
        if let Some(artifacts) = &self.artifacts {
            for (artifact, version) in artifacts.timeline(&Scope::default(), 100_000, None).0 {
                let how = if version.via.iter().any(|v| v == "show_file") { How::Shown }
                    else if version.via.iter().any(|v| v == "generate_image") { How::Generated }
                    else { continue };
                let group = version.group.as_ref().map(|g| g.id.clone());
                let session = version.turn.as_ref().map(|t| t.session.clone()).or_else(|| artifact.where_.sessions.first().cloned());
                found.push(Handover {
                    path: artifact.path,
                    how,
                    bot: version.by.bot.or_else(|| artifact.where_.bots.first().cloned()),
                    session: group.is_none().then_some(session).flatten(),
                    group,
                    message: version.turn.and_then(|t| t.message),
                    part: None,
                    at: version.at.parse::<Timestamp>().ok(),
                });
            }
        }
        found.sort_by_key(|h| h.at.map_or(0, Timestamp::as_millisecond));
        for handover in found {
            let exists = Path::new(&handover.path).exists()
                || self.artifacts.as_ref().and_then(|a| a.at(&handover.path)).is_some_and(|a| a.versions.iter().any(|v| v.kept));
            if exists { self.store.hand(handover); }
        }
        let _ = fs::write(self.root.join("backfill.json"), json!({ "done": BACKFILL_VERSION }).to_string());
    }
}

pub fn library_handovers(sessions_dir: &Path, bots_dir: &Path) -> Vec<Handover> {
    let headers = read_saved_session_headers(sessions_dir);
    let parents: HashMap<String, Option<String>> = headers.iter().map(|h| (h.id.clone(), h.parent_id.clone())).collect();
    let root_of = |id: &str| {
        let mut at = id.to_owned();
        for _ in 0..8 {
            if let Some(Some(parent)) = parents.get(&at) { at = parent.clone(); } else { break; }
        }
        at
    };
    let bots_str = bots_dir.to_string_lossy().into_owned();
    let mut found = Vec::new();
    for session in headers {
        let dir = session.directory.as_deref();
        let kuvo = session.metadata.as_ref().and_then(|m| m.get("kuvo"));
        let seat = kuvo.and_then(|k| k.get("seat"));
        let bot = seat.and_then(|s| s.get("bot")).or_else(|| kuvo.and_then(|k| k.get("bot"))).and_then(Value::as_str).map(str::to_owned)
            .or_else(|| {
                let first = strip_folder_prefix(dir?, &bots_str)?.split(std::path::MAIN_SEPARATOR).next()?;
                first.starts_with("bot_").then(|| first.to_owned())
            });
        let top = root_of(&session.id);
        let own = top == session.id;
        let group = seat.and_then(|s| s.get("group")).and_then(Value::as_str).map(str::to_owned);
        let target_session = group.is_none().then_some(top);
        let absolute = |raw: &str| {
            let p = Path::new(raw);
            if p.is_absolute() { raw.to_owned() } else { Path::new(dir.unwrap_or("/")).join(p).to_string_lossy().into_owned() }
        };
        let make = |path: String, how: How, message: Option<String>, part: Option<String>, at: Option<Timestamp>| Handover {
            path, how, bot: bot.clone(), session: target_session.clone(), group: group.clone(), message, part, at,
        };
        for message in saved_messages(&session.folder) {
            let at = message.pointer("/time/completed").or_else(|| message.pointer("/time/created"))
                .and_then(Value::as_i64).and_then(|ms| Timestamp::from_millisecond(ms).ok());
            let msg_id = message.get("id").and_then(Value::as_str).map(str::to_owned);
            match message.get("type").and_then(Value::as_str) {
                Some("user") if own => {
                    for path in attached_paths(&message) { found.push(make(path, How::Attached, msg_id.clone(), None, at)); }
                }
                Some("assistant") => {
                    let cited_prefix = format!("cited-{}-", msg_id.as_deref().unwrap_or(""));
                    for part in message.get("content").and_then(Value::as_array).into_iter().flatten() {
                        if part.get("type").and_then(Value::as_str) != Some("tool") { continue; }
                        let part_id = part.get("id").and_then(Value::as_str).unwrap_or("");
                        let name = tool_name(part.get("name").and_then(Value::as_str).unwrap_or(""));
                        let anchor_msg = own.then(|| msg_id.clone()).flatten();
                        let anchor_part = (own && !part_id.is_empty()).then(|| part_id.to_owned());
                        if (name == "show_file" || part_id.starts_with(&cited_prefix))
                            && let Some(raw) = part.pointer("/state/input/path").and_then(Value::as_str)
                        {
                            found.push(make(absolute(raw), How::Shown, anchor_msg.clone(), anchor_part.clone(), at));
                        }
                        if name == "generate_image" {
                            for out in part.pointer("/state/content").and_then(Value::as_array).into_iter().flatten() {
                                if out.get("type").and_then(Value::as_str) == Some("file")
                                    && let Some(uri) = out.get("uri").and_then(Value::as_str).filter(|u| u.starts_with("file:"))
                                    && let Ok(fp) = Url::parse(uri).ok().and_then(|u| u.to_file_path().ok()).ok_or(())
                                {
                                    found.push(make(fp.to_string_lossy().into_owned(), How::Generated, anchor_msg.clone(), anchor_part.clone(), at));
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
