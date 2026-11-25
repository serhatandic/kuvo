//! What each session wrote to another with send_message lately (`run/work/sent.json`, the
//! TypeScript box's file), so a lead isn't told the same again (#768): a session that sends its
//! starter its report and then finishes saying it doesn't wake the lead a second time with it.

use std::time::Duration;

use kuvo_store::locks::{OnBusy, StaleLock};
use serde::{Deserialize, Serialize};

use super::Work;

/// How much of each is kept, how many to each it wrote to, of how many sessions, for how long.
const KEPT_TEXT: usize = 8000;
const EACH: usize = 4;
const MOST_KEPT: usize = 16;
const SESSIONS: usize = 100;
const KEPT_FOR: i64 = 86_400_000;
/// How long what a reply says must be to count as said already when what was sent holds it.
const SAID_ALREADY: usize = 200;

#[derive(Clone, Deserialize, Serialize)]
struct Sent {
    to: String,
    text: String,
    at: i64,
}

/// Words as they're compared: whitespace aside.
fn plain(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn cut(text: &str) -> String {
    text.chars().take(KEPT_TEXT).collect()
}

impl Work {
    fn read_sent(&self) -> Vec<(String, Vec<Sent>)> {
        let file = self.records.folder().join("sent.json");
        let kept: serde_json::Map<String, serde_json::Value> = std::fs::read(file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        kept.into_iter()
            .map(|(session, list)| (session, serde_json::from_value(list).unwrap_or_default()))
            .collect()
    }

    /// `session` wrote `text` to `to` (send_message), now.
    pub fn note_sent(&self, session: &str, to: &str, text: &str) {
        let words = cut(&plain(text));
        if words.is_empty() {
            return;
        }
        let folder = self.records.folder();
        let _ = std::fs::create_dir_all(folder);
        let _lock = StaleLock::acquire(
            &folder.join(".sent.lock"),
            Duration::from_secs(10),
            Duration::from_secs(3),
            OnBusy::Proceed,
        );
        let now = jiff::Timestamp::now().as_millisecond();
        let mut all = self.read_sent();
        let mut kept: Vec<Sent> = all
            .iter()
            .find(|(id, _)| id == session)
            .map(|(_, list)| list.clone())
            .unwrap_or_default();
        kept.retain(|each| now - each.at < KEPT_FOR);
        kept.push(Sent {
            to: to.into(),
            text: words,
            at: now,
        });
        // (Its latest few to each: writing to others doesn't push out what it sent its starter.)
        let its: Vec<Sent> = kept
            .iter()
            .enumerate()
            .filter(|(index, each)| {
                kept[index + 1..]
                    .iter()
                    .filter(|later| later.to == each.to)
                    .count()
                    < EACH
            })
            .map(|(_, each)| each.clone())
            .collect();
        let its = its[its.len().saturating_sub(MOST_KEPT)..].to_vec();
        // Last, as the latest: the oldest are let go first.
        all.retain(|(id, _)| id != session);
        all.push((session.into(), its));
        let all = &all[all.len().saturating_sub(SESSIONS)..];
        let map: serde_json::Map<String, serde_json::Value> = all
            .iter()
            .map(|(id, list)| (id.clone(), serde_json::json!(list)))
            .collect();
        if let Ok(bytes) = serde_json::to_vec(&map) {
            let _ = kuvo_store::files::write_secret(&folder.join("sent.json"), &bytes);
        }
    }

    /// Whether `session` has written `to` what `reply` says, since `since`: the same words, a long
    /// enough reply that's part of what it sent, or one that holds a long enough message it sent
    /// with little else ("I sent Pixel this: …").
    pub fn said_already(&self, session: &str, to: &str, reply: &str, since: i64) -> bool {
        let words = plain(reply);
        if words.is_empty() {
            return false;
        }
        let shown = cut(&words);
        let (count, kept_text) = (words.chars().count(), |each: &Sent| {
            each.text.chars().count()
        });
        self.read_sent()
            .into_iter()
            .filter(|(id, _)| id == session)
            .flat_map(|(_, list)| list)
            .any(|each| {
                each.to == to
                    && each.at >= since
                    && (each.text == shown
                        || (count >= SAID_ALREADY && each.text.contains(&shown))
                        || (kept_text(&each) >= SAID_ALREADY
                            && kept_text(&each) < KEPT_TEXT
                            && words.contains(&each.text)
                            && count - kept_text(&each) < SAID_ALREADY))
            })
    }
}
