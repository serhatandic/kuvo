//! When you last saw each bot here (PROTOCOL.md, "What a bot is doing now", Seen): a client says so
//! as it shows you a bot (`POST kuvo/bots/<id>/seen`), and from then on what its sessions did
//! before is no longer Done, nor a hard stop Blocked, on any client. Kept in
//! `run/bots-seen/seen.json` (`{"bots": {"<id>": <ms>}}`, written whole); a bot that no longer
//! lives here is let go as it's next written. Its own conversation is read up to where its last
//! turn ended, as a client reads the conversation it shows.

use std::time::Duration;

use http::Method;
use serde_json::{Map, Value, json};

use super::{Bots, here::BotHere, reach::segment};

const GONE: &str = "That bot isn’t there any more.";

impl Bots {
    /// `id` seen at `at` (ms; a time before the one kept leaves that), kept.
    fn note_seen(&self, id: &str, at: i64) -> std::io::Result<()> {
        let _lock = kuvo_store::locks::OwnedLock::acquire(
            &self.inner.run.join(".bots-seen.lock"),
            Duration::from_secs(10),
        )?;
        let mut seen: Map<String, Value> = self
            .seen()
            .into_iter()
            .filter(|(each, _)| each == id || BotHere::read(&self.inner.bots, each).is_some())
            .map(|(each, at)| (each, json!(at)))
            .collect();
        let was = seen.get(id).and_then(Value::as_i64).unwrap_or(0);
        seen.insert(id.to_owned(), json!(was.max(at)));
        let text = serde_json::to_string(&json!({ "bots": seen })).expect("JSON");
        kuvo_store::files::write_secret(&self.inner.run.join("bots-seen/seen.json"), text.as_bytes())
    }

    /// `POST kuvo/bots/<id>/seen`.
    pub(super) async fn seen_now(&self, id: &str) -> Result<(), (u16, String)> {
        let Some(bot) = BotHere::read(&self.inner.bots, id) else {
            return Err((404, GONE.into()));
        };
        let bots = self.clone();
        let noted = {
            let id = id.to_owned();
            tokio::task::spawn_blocking(move || {
                bots.note_seen(&id, jiff::Timestamp::now().as_millisecond())
            })
            .await
        };
        if !matches!(noted, Ok(Ok(()))) {
            return Err((
                500,
                "It couldn’t be marked as seen just now. Try again in a moment.".into(),
            ));
        }
        self.changed();
        let Some(list) = self.inner.sessions.latest_record() else {
            return Err((503, "The engine isn’t running.".into()));
        };
        let own = &list["sessions"][&bot.session];
        let unread = &own["facts"]["unread"];
        // (Nothing new in it, or not this box's own copy: nothing to read.)
        if own.is_null() || unread.is_null() || own["facts"]["away"] == true {
            return Ok(());
        }
        // (It lives with the coding agents, whose bridge isn't in this box yet: nothing to read it on.)
        if own["place"] == "bridge" {
            return Ok(());
        }
        let Some(engine) = self.inner.engine.running_bot_engine().await else {
            return Ok(());
        };
        engine
            .call(
                Method::POST,
                &format!("api/session/{}/view", segment(&bot.session)),
                Some(&json!({ "idle": unread })),
                Duration::from_secs(10),
            )
            .await
            .map_err(|_| {
                (
                    502,
                    "Its conversation couldn’t be marked as read just now. Try again in a moment."
                        .into(),
                )
            })?;
        Ok(())
    }
}
