//! What the work watcher keeps in `run/work`: what waits to be told each conversation
//! (`wakes.json`, gathered until it's sent, so none is lost to the box starting again), what each
//! followed session's starter was told and when the box last heard of any (`told.json`), and which
//! sessions you stopped (`stopped.json`). Each under its lock, shared with the TS box.

use std::{path::PathBuf, time::Duration};

use kuvo_store::locks::{OnBusy, StaleLock};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// What waits to be told a conversation: its lines, whether one is an ask (read at its next step,
/// not after its turn), and since when (ms). `tried`: its first lines went once already under this
/// id, the answer lost, so they go again alone under the same id, which a conversation takes once.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Waiting {
    pub lines: Vec<String>,
    pub asks: bool,
    pub since: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tried: Option<Tried>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Tried {
    pub id: String,
    pub lines: usize,
    pub asks: bool,
}

/// What a starter was told of one session: its last reply, the asks told, when its finish was told,
/// its newest message as a failed turn was told, what its reply said (a hash), whether it was told
/// early (while what it left running went on), why its last failed turn failed, when it had last
/// changed as its finish was told, and whether its reply couldn't be read then.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SessionTold {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub asks: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub said: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub early: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upto: Option<u64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unread: bool,
}

/// What's noted of a session as it's told (each field only where it's given).
#[derive(Default)]
pub struct Note {
    pub reply: Option<String>,
    pub ask: Option<String>,
    pub at: Option<u64>,
    pub turn: Option<String>,
    pub said: Option<String>,
    pub early: Option<bool>,
    pub failure: Option<String>,
    pub upto: Option<u64>,
    pub unread: bool,
}

/// The engine's place, as `told.json` names it: its time is `since`; the others' are in `sinces`.
pub const ENGINE: &str = "engine";
const OTHERS: &[&str] = &["bridge"];
const KEPT: usize = 400;

/// The oldest let go of, past the latest few hundred.
fn drop_oldest(kept: &mut Map<String, Value>) {
    let over = kept.len().saturating_sub(KEPT);
    let oldest: Vec<String> = kept.keys().take(over).cloned().collect();
    for key in oldest {
        kept.shift_remove(&key);
    }
}

pub fn now() -> u64 {
    jiff::Timestamp::now().as_millisecond().max(0) as u64
}

fn read(file: &std::path::Path) -> Option<Value> {
    kuvo_store::files::text_of(file).and_then(|text| serde_json::from_str(&text).ok())
}

/// The watcher's files.
#[derive(Clone)]
pub struct Kept {
    folder: PathBuf,
}

impl Kept {
    pub fn new(run: &std::path::Path) -> Self {
        Self {
            folder: run.join("work"),
        }
    }

    fn locked<T>(&self, lock: &str, change: impl FnOnce() -> T) -> T {
        let _ = std::fs::create_dir_all(&self.folder);
        let (stale, wait) = (Duration::from_secs(10), Duration::from_secs(3));
        let _lock = StaleLock::acquire(&self.folder.join(lock), stale, wait, OnBusy::Proceed);
        change()
    }

    fn write(&self, name: &str, value: &impl Serialize) {
        let text = serde_json::to_string(value).expect("JSON");
        let _ = kuvo_store::files::write_secret(&self.folder.join(name), text.as_bytes());
    }

    /// What waits for each conversation.
    pub fn waiting(&self) -> Map<String, Value> {
        read(&self.folder.join("wakes.json"))
            .and_then(|kept| kept.as_object().cloned())
            .unwrap_or_default()
    }

    pub fn waiting_for(&self, starter: &str) -> Option<Waiting> {
        serde_json::from_value(self.waiting().get(starter)?.clone()).ok()
    }

    /// What waits, as `change` leaves it.
    pub fn change_waiting<T>(&self, change: impl FnOnce(&mut Map<String, Value>) -> T) -> T {
        self.locked(".wakes.lock", || {
            let mut waiting = self.waiting();
            let made = change(&mut waiting);
            self.write("wakes.json", &waiting);
            made
        })
    }

    /// What `starter` was still to hear of its sessions, dropped (Stop All).
    pub fn forget_wakes(&self, starter: &str) {
        self.change_waiting(|all| all.shift_remove(starter));
    }

    /// Lines kept for conversations from outside a watcher (a stop), each with what else waits for
    /// it; one waiting already isn't added again. The starters to send to.
    pub fn add_lines(&self, told: &[(String, String)]) -> Vec<String> {
        self.change_waiting(|all| {
            for (starter, line) in told {
                let kept = all
                    .get(starter)
                    .and_then(|kept| serde_json::from_value(kept.clone()).ok());
                let mut kept: Waiting = kept.unwrap_or(Waiting {
                    since: now(),
                    ..Waiting::default()
                });
                if !kept.lines.contains(line) {
                    kept.lines.push(line.clone());
                }
                all.insert(starter.clone(), json!(kept));
            }
        });
        let mut starters: Vec<String> = told.iter().map(|(starter, _)| starter.clone()).collect();
        starters.dedup();
        starters
    }

    /// What's been told, or none for a box that hasn't kept it yet.
    pub fn told(&self) -> Option<Value> {
        read(&self.folder.join("told.json")).filter(|told| told["since"].is_number())
    }

    pub fn session_told(&self, session: &str) -> SessionTold {
        let told = self.told().unwrap_or_default();
        serde_json::from_value(told["sessions"][session].clone()).unwrap_or_default()
    }

    /// What's been told, as `change` leaves it; with `from`, the box has heard of everything of its
    /// place until `until`. The latest few hundred sessions are kept.
    pub fn change_told(&self, change: impl FnOnce(&mut Value), from: Option<&str>, until: u64) {
        self.locked(".wakes.lock-told", || {
            let mut told = self
                .told()
                .unwrap_or_else(|| json!({ "since": until, "sessions": {} }));
            if !told["sessions"].is_object() {
                told["sessions"] = json!({});
            }
            change(&mut told);
            let since = told["since"].as_u64().unwrap_or(0);
            match from {
                Some(ENGINE) => {
                    // (Each other place keeps the time the engine had, which was theirs too until now.)
                    for other in OTHERS {
                        if !told["sinces"][other].is_number() {
                            told["sinces"][other] = json!(since);
                        }
                    }
                    told["since"] = json!(since.max(until));
                }
                Some(place) => {
                    let was = told["sinces"][place].as_u64().unwrap_or(since);
                    told["sinces"][place] = json!(was.max(until));
                }
                None => {}
            }
            let sessions = told["sessions"].as_object_mut().expect("sessions");
            drop_oldest(sessions);
            self.write("told.json", &told);
        });
    }

    /// What's noted of `session` as it's told, as the latest told of.
    pub fn note_told(&self, session: &str, note: Note, from: Option<&str>) {
        self.change_told(
            |told| {
                let sessions = told["sessions"].as_object_mut().expect("sessions");
                let was = sessions.shift_remove(session).unwrap_or_default();
                let mut its: SessionTold = serde_json::from_value(was).unwrap_or_default();
                if let Some(reply) = note.reply {
                    its.reply = Some(reply);
                    its.unread = false;
                    if let Some(held) = told["held"].as_object_mut() {
                        held.shift_remove(session);
                        if held.is_empty() {
                            told.as_object_mut().expect("told").shift_remove("held");
                        }
                    }
                }
                its.upto = note.upto.or(its.upto);
                its.unread |= note.unread;
                its.at = note.at.or(its.at);
                its.turn = note.turn.or(its.turn);
                its.said = note.said.or(its.said);
                its.early = note.early.unwrap_or(its.early);
                its.failure = note.failure.or(its.failure);
                if let Some(ask) = note.ask {
                    its.asks.retain(|each| *each != ask);
                    its.asks.push(ask);
                    let over = its.asks.len().saturating_sub(20);
                    its.asks.drain(..over);
                }
                let sessions = told["sessions"].as_object_mut().expect("sessions");
                sessions.insert(session.to_owned(), json!(its));
            },
            from,
            now(),
        );
    }

    /// When each session you stopped was stopped (ms).
    pub fn stopped(&self) -> Map<String, Value> {
        let kept =
            read(&self.folder.join("stopped.json")).and_then(|kept| kept.as_object().cloned());
        let mut kept = kept.unwrap_or_default();
        kept.retain(|_, at| at.is_number());
        kept
    }

    fn change_stopped(&self, change: impl FnOnce(&mut Map<String, Value>)) {
        self.locked(".background.lock-stopped", || {
            let mut stopped = self.stopped();
            change(&mut stopped);
            drop_oldest(&mut stopped);
            self.write("stopped.json", &stopped);
        });
    }

    /// `session` was stopped, now: nothing comes of it until someone writes to it.
    pub fn note_stopped(&self, session: &str) {
        self.change_stopped(|stopped| {
            stopped.shift_remove(session);
            stopped.insert(session.to_owned(), json!(now()));
        });
    }

    /// `session` was written to since it was stopped: it's at work again.
    pub fn forget_stopped(&self, session: &str) {
        if self.stopped().contains_key(session) {
            self.change_stopped(|stopped| {
                stopped.shift_remove(session);
            });
        }
    }
}
