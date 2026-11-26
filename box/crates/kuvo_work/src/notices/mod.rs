//! What this box told you lately (PROTOCOL.md, "kuvo/notices"): each notice it made (a run's news,
//! a question, a permission, an agent's notify_user), kept a little while in
//! `run/notices/told.json` for its clients to show as their own notifications, so the box alone
//! decides what's news and how it's said.

pub mod mask;
pub mod news;
pub mod notify;
pub mod words;

use std::{
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::Duration,
};

use kuvo_store::locks::{OnBusy, StaleLock};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A notice (PROTOCOL.md, "Notifications"): what it's about (`category`, for its buttons), where
/// it opens (`thread`, `data.session` or `data.group`) and what answering takes back
/// (`data.request`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Notice {
    #[serde(default)]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    pub category: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub data: Map<String, Value>,
}

/// A notice told: its id, when (ms since 1970), the notice, and `by: "agent"` for one an agent
/// sent on purpose (notify_user).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Told {
    pub id: String,
    pub at: i64,
    pub notice: Notice,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
}

// How long each is kept: news for clients that follow now; what agents sent, a day, for their limit.
const NEWS_KEPT: i64 = 10 * 60_000;
const SENT_KEPT: i64 = 24 * 60 * 60_000;
const MOST: usize = 300;

pub fn now() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

fn kept_still(entry: &Told, now: i64) -> bool {
    let kept = if entry.by.as_deref() == Some("agent") {
        SENT_KEPT
    } else {
        NEWS_KEPT
    };
    let expires = entry.notice.data.get("expires").and_then(Value::as_str);
    now.saturating_sub(entry.at) < kept
        && (entry.notice.category != "usage"
            || expires.is_none_or(|at| at.is_empty() || at.parse::<i64>().is_ok_and(|at| at > now)))
}

fn new_id() -> String {
    let mut bytes = [0u8; 12];
    let _ = getrandom::fill(&mut bytes);
    format!("ntc_{}", kuvo_wire::keys::encode(&bytes))
}

/// Where notices are kept, and who's told as one is.
#[derive(Clone)]
pub struct Notices {
    folder: PathBuf,
    told: Arc<OnceLock<Arc<dyn Fn() + Send + Sync>>>,
}

impl Notices {
    pub fn new(home: &Path) -> Self {
        Self {
            folder: home.join("run/notices"),
            told: Arc::default(),
        }
    }

    /// Told each time one's kept (the history clients follow).
    pub fn attach_told(&self, told: Arc<dyn Fn() + Send + Sync>) {
        let _ = self.told.set(told);
    }

    /// What's kept, oldest first.
    pub fn read(&self) -> Vec<Told> {
        let kept = kuvo_store::files::text_of(&self.folder.join("told.json"));
        let kept: Vec<Value> = kept
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        kept.into_iter()
            .filter_map(|entry| serde_json::from_value(entry).ok())
            .collect()
    }

    /// `notice` kept as told now (short and in plain words), with what's past its time let go;
    /// refused with why where `unless`, given what's kept, says so; none where it couldn't be kept.
    pub fn keep(
        &self,
        notice: Notice,
        by_agent: bool,
        unless: impl FnOnce(&[Told]) -> Option<String>,
    ) -> Option<Result<Told, String>> {
        let now = now();
        self.keep_at(notice, by_agent, now, unless)
    }

    /// Usage catch-up carries the maker's original time, not the time a
    /// resident received the notice. Never extend its visibility on replay.
    pub fn keep_at(
        &self,
        notice: Notice,
        by_agent: bool,
        at: i64,
        unless: impl FnOnce(&[Told]) -> Option<String>,
    ) -> Option<Result<Told, String>> {
        let now = now();
        let entry = Told {
            id: new_id(),
            at: at.min(now),
            notice: words::short_notice(notice),
            by: by_agent.then(|| "agent".to_owned()),
        };
        if !kept_still(&entry, now) {
            return None;
        }
        kuvo_store::files::create_private_dirs(&self.folder).ok()?;
        let lock = self.folder.join("told.lock");
        let _lock = StaleLock::acquire(
            &lock,
            Duration::from_secs(10),
            Duration::from_secs(3),
            OnBusy::Proceed,
        );
        let mut kept: Vec<Told> = self
            .read()
            .into_iter()
            .filter(|each| kept_still(each, now))
            .collect();
        if let Some(why) = unless(&kept) {
            return Some(Err(why));
        }
        kept.push(entry.clone());
        let from = kept.len().saturating_sub(MOST);
        let bytes = serde_json::to_vec(&kept[from..]).ok()?;
        kuvo_store::files::write_secret(&self.folder.join("told.json"), &bytes).ok()?;
        if let Some(told) = self.told.get() {
            told();
        }
        Some(Ok(entry))
    }

    /// `notice` kept as news.
    pub fn tell(&self, notice: Notice) {
        let _ = self.keep(notice, false, |_| None);
    }
}
