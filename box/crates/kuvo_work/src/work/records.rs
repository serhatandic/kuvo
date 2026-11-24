//! What the box keeps of bots' work in `run/work`: when each bot's work last changed
//! (`changes.json`: a session of its started, stopped, finished, asked or answered), and what each
//! bot just did to its sessions (`oversight.json`: started one, wrote to one, read one, answered
//! what one asked, stopped one, or one finished), the latest minute of it, fifty marks at most.
//! Every process that acts for a bot notes it here.

use std::{path::PathBuf, time::Duration};

use serde_json::{Map, Value, json};

const KEPT_FOR: i64 = 60_000;
const KEPT_AT_MOST: usize = 50;
const KINDS: &[&str] = &["started", "told", "read", "answered", "stopped", "finished"];

/// The `run/work` folder.
#[derive(Clone)]
pub struct Records {
    folder: PathBuf,
}

fn now() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

impl Records {
    pub fn new(run: &std::path::Path) -> Self {
        Self {
            folder: run.join("work"),
        }
    }

    pub fn folder(&self) -> &std::path::Path {
        &self.folder
    }

    fn read(&self, name: &str) -> Value {
        kuvo_store::files::text_of(&self.folder.join(name))
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn locked<T>(&self, name: &str, change: impl FnOnce() -> T) -> Option<T> {
        std::fs::create_dir_all(&self.folder).ok()?;
        let _lock =
            kuvo_store::locks::OwnedLock::acquire(&self.folder.join(name), Duration::from_secs(10))
                .ok()?;
        Some(change())
    }

    /// When each bot's work last changed (ms), by its id.
    pub fn changes(&self) -> Map<String, Value> {
        self.read("changes.json")
            .as_object()
            .map(|kept| {
                kept.iter()
                    .filter(|(_, at)| at.is_number())
                    .map(|(id, at)| (id.clone(), at.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A change to `bot`'s work, noted now (never the same time twice, so a client sees each).
    pub fn note_changed(&self, bot: &str) {
        self.locked(".changes.lock", || {
            let mut changes = self.changes();
            let was = changes.get(bot).and_then(Value::as_i64).unwrap_or(0);
            changes.insert(bot.into(), json!(now().max(was + 1)));
            let text = serde_json::to_string(&changes).expect("JSON");
            let _ =
                kuvo_store::files::write_secret(&self.folder.join("changes.json"), text.as_bytes());
        });
    }

    /// The marks of the latest minute, the oldest first.
    pub fn oversight(&self) -> Vec<Value> {
        let at = now();
        self.read("oversight.json")
            .as_array()
            .into_iter()
            .flatten()
            .filter(|mark| {
                ["id", "bot", "session"]
                    .iter()
                    .all(|key| mark[key].is_string())
                    && mark["kind"]
                        .as_str()
                        .is_some_and(|kind| KINDS.contains(&kind))
                    && mark["at"]
                        .as_f64()
                        .is_some_and(|made| (at as f64) - made < KEPT_FOR as f64)
            })
            .cloned()
            .collect()
    }

    /// `bot` did `kind` to its session `session`, now.
    pub fn note_oversight(&self, bot: &str, session: &str, kind: &str) {
        self.locked(".oversight.lock", || {
            let mut marks = self.oversight();
            let mut id = [0u8; 16];
            let _ = getrandom::fill(&mut id);
            marks.push(json!({"id": hex::encode(id), "bot": bot, "session": session, "kind": kind, "at": now()}));
            let skip = marks.len().saturating_sub(KEPT_AT_MOST);
            let text = serde_json::to_string(&marks[skip..]).expect("JSON");
            let _ = kuvo_store::files::write_secret(&self.folder.join("oversight.json"), text.as_bytes());
        });
    }
}
