//! Your answer to what's asked (`POST kuvo/work/asks/:id`): allow, always or deny, with the
//! projects folder when none is picked here yet. What's asked is done here, on the box; its tool
//! hears of it as it waits, or after, as a message to its conversation. "Always" is kept with the
//! bot (`work.allowed` in its bot.json), so it asks once for each kind; Edit Bot takes it back.

use std::time::Duration;

use kuvo_engine::BoxMessage;
use kuvo_store::locks::{OnBusy, StaleLock};
use serde_json::{Map, Value, json};

use super::{Work, projects};
use crate::bots::here::{BotHere, change_description};

/// What a bot may be allowed Always (kept with it): a project's; a reset's is the account's.
const BOT_ACTIONS: &[&str] = &["makeProject", "cloneProject"];
const GONE: &str = "That's been answered, or it isn't asked any more.";
const MINUTE: i64 = 60_000;
const NO_MORE: &str =
    "Don't ask again for it: go on without it, or ask them in a line what they'd like instead.";
const OVERNIGHT: &str =
    "so it's no for now. Go on without it; it waits for the user in the morning report.";

type Refused = (u16, String);

fn now() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

fn since(at: &Value) -> i64 {
    now() - at.as_f64().map_or(0, |at| at as i64)
}

fn failed(error: impl ToString) -> Refused {
    (500, error.to_string())
}

fn no<T>(status: u16, message: &str) -> Result<T, Refused> {
    Err((status, message.to_owned()))
}

impl Work {
    /// The asks as `change` leaves them, under their lock (`run/work/.asks.lock`, the TS box's
    /// too): answered ones go once their tool has had them (or their conversation, told a week at
    /// most); unanswered ones of a bot that's gone, or asked a week ago and more.
    pub(super) fn change_asks<T>(
        &self,
        change: impl FnOnce(&mut Vec<Value>) -> T,
    ) -> Result<T, Refused> {
        let folder = self.records.folder();
        std::fs::create_dir_all(folder).map_err(failed)?;
        let (stale, wait) = (Duration::from_secs(10), Duration::from_secs(3));
        let _lock = StaleLock::acquire(&folder.join(".asks.lock"), stale, wait, OnBusy::Proceed);
        let mut asks = self.kept_asks();
        let made = change(&mut asks);
        let week = 7 * 24 * 60 * MINUTE;
        let here = |bot: &str| BotHere::read(&self.bots, bot).is_some();
        asks.retain(|each| match &each["answered"] {
            Value::Object(answered) => {
                let (since, untold) = (since(&answered["at"]), each["toldLate"] != true);
                since < 15 * MINUTE || (each["late"] == true && untold && since < week)
            }
            _ => each["bot"].as_str().is_none_or(here) && since(&each["at"]) < week,
        });
        let text = serde_json::to_string_pretty(&json!({ "asks": asks })).map_err(failed)?;
        let file = folder.join("asks.json");
        kuvo_store::files::write_secret(&file, text.as_bytes()).map_err(failed)?;
        Ok(made)
    }

    /// "Always": `action` kept with the bot, for its next time.
    fn allow_always(&self, bot: &str, action: &str) -> std::io::Result<()> {
        let Some(here) = BotHere::read(&self.bots, bot) else {
            return Ok(());
        };
        let allow = |now: Option<Map<String, Value>>| {
            let mut described = now.unwrap_or(here.described.clone());
            let work = described.get("work").and_then(Value::as_object);
            let mut work = work.cloned().unwrap_or_default();
            let had = work.get("allowed").and_then(Value::as_array).cloned();
            let had = had.unwrap_or_default();
            let kept = |each: &&str| *each == action || had.contains(&json!(each));
            let allowed: Vec<&str> = BOT_ACTIONS.iter().copied().filter(kept).collect();
            work.insert("allowed".into(), json!(allowed));
            described.insert("work".into(), Value::Object(work));
            Ok(Some(described))
        };
        change_description(&here.folder, allow, false)?;
        self.list.changed();
        Ok(())
    }

    /// What the asked action does, in its maker's words, and whether it failed; done now.
    pub(super) async fn do_work(&self, ask: &Value, always: bool) -> (String, bool) {
        let here = self.settings.here();
        if ask["action"] == "resetLimits" {
            let Some(row) = ask["given"]["reset"]["row"].as_str() else {
                return ("There's no reset to use in that ask.".into(), true);
            };
            let Some(resets) = &self.resets else {
                return (
                    format!(
                        "The coding agents aren't running on {here} now, so the reset wasn't used."
                    ),
                    true,
                );
            };
            return resets
                .spend_for(row, ask["name"].as_str().unwrap_or("Agent"), always, false)
                .await;
        }
        let mut making = ask["given"].as_object().cloned().unwrap_or_default();
        let clones = ask["action"] == "cloneProject";
        let keep = |key: &String| ["name", "git", "folder"].contains(&key.as_str());
        making.retain(|key, value| !value.is_null() && (keep(key) || (clones && key == "clone")));
        let bot = ask["bot"].as_str();
        let made_by = bot.map(|bot| json!({ "bot": bot, "name": ask["name"], "at": now() }));
        let (settings, home) = (&self.settings, &self.home);
        match projects::make_project(&making, made_by, settings, home).await {
            Ok(made) => {
                bot.inspect(|bot| self.records.note_changed(bot));
                (projects::made_words(&made, settings.account(), here), false)
            }
            Err((500, why)) => (format!("Couldn't do it: {why}"), true),
            Err((_, why)) => (why, true),
        }
    }

    /// Your answer to the ask `id`: done, and its asker told, as its tool waits or, after, in its
    /// conversation. What was answered, or a status and why not.
    pub async fn answer_work(
        &self,
        id: &str,
        given: &Map<String, Value>,
    ) -> Result<Value, Refused> {
        let open = |each: &&mut Value| each["id"] == id && !each["answered"].is_object();
        let mut kept = self.kept_asks();
        let Some(ask) = kept.iter_mut().find(open).map(Value::take) else {
            return no(404, GONE);
        };
        let answer = given.get("answer").and_then(Value::as_str);
        let Some(answer) = answer.filter(|answer| ["allow", "always", "deny"].contains(answer))
        else {
            return no(400, "Answer allow, always or deny.");
        };
        let action = ask["action"].as_str().unwrap_or_default();
        // The projects folder, picked with the first: only where none is, and kept once it's
        // claimed (not for one with a folder named for it).
        let mut picked = None;
        let unpicked = self.settings.projects_folder().is_none();
        if answer != "deny"
            && action != "resetLimits"
            && unpicked
            && ask["given"]["folder"].is_null()
        {
            let Some(folder) = given.get("folder").filter(|folder| !folder.is_null()) else {
                return no(400, "Pick the folder new projects go in.");
            };
            let folder = self.settings.folder_given(folder);
            picked = Some(folder.map_err(|why| (400, why))?);
        }
        // One answer at a time: another given meanwhile (from a phone, say) finds it taken.
        let claimed = self.change_asks(|asks| {
            let kept = asks.iter_mut().find(open);
            let kept = kept.filter(|kept| since(&kept["answering"]) >= 15 * MINUTE);
            kept.map(|kept| kept["answering"] = json!(now())).is_some()
        })?;
        if !claimed {
            return no(404, GONE);
        }
        let project = ask["project"].as_str().unwrap_or_default();
        let clone = ask["clone"].as_str().unwrap_or_default();
        let what = match action {
            "resetLimits" => format!("using one of {project}'s resets"),
            "cloneProject" => format!("cloning {clone} as {project}"),
            _ => format!("making {project}"),
        };
        let overnight = given.get("overnight") == Some(&json!(true));
        let (text, is_error) = match answer {
            "deny" if overnight => (
                format!("No one could answer {what} overnight, {OVERNIGHT}"),
                true,
            ),
            "deny" => (format!("The user said no to {what}. {NO_MORE}"), true),
            _ => {
                // Whatever goes wrong is its answer, so the ask is never left half answered.
                let mut ready = Ok(());
                if let Some(picked) = picked.filter(|_| self.settings.projects_folder().is_none()) {
                    let set = self.settings.change(&json!({ "projectsFolder": picked }));
                    ready = set.map(drop).map_err(|(_, why)| why);
                }
                let bot = ask["bot"]
                    .as_str()
                    .filter(|_| BOT_ACTIONS.contains(&action));
                if let (Ok(()), "always", Some(bot)) = (&ready, answer, bot) {
                    let allowed = self.allow_always(bot, action);
                    ready = allowed.map_err(|error| error.to_string());
                }
                match ready {
                    Ok(()) => self.do_work(&ask, answer == "always").await,
                    Err(error) => (format!("Couldn't do it: {error}"), true),
                }
            }
        };
        let at = now();
        let answered = json!({ "answer": answer, "text": text, "isError": is_error, "at": at });
        let late = self.change_asks(|asks| {
            let kept = asks.iter_mut().find(|each| each["id"] == id)?;
            kept["answered"] = answered.clone();
            kept.as_object_mut()?.shift_remove("answering");
            Some(kept["late"] == true)
        })?;
        let bot = ask["bot"].as_str();
        bot.inspect(|bot| self.records.note_changed(bot));
        // Its tool stopped waiting: its conversation hears of it as a message.
        if late == Some(true) {
            self.tell_late(id).await;
        }
        Ok(answered)
    }

    /// One answered after its tool stopped waiting, told to its conversation here, and noted once
    /// it is. Not while it moves to or from another computer, nor while it can't be reached (nor
    /// where it isn't this engine's): tried again later.
    pub async fn tell_late(&self, id: &str) {
        let _delivery = self.late_delivery.lock().await;
        let asks = self.kept_asks();
        let Some(ask) = asks.iter().find(|each| each["id"] == id) else {
            return;
        };
        let (text, session) = (ask["answered"]["text"].as_str(), ask["session"].as_str());
        let untold = ask["late"] == true && ask["toldLate"] != true;
        let (Some(text), Some(session), true) = (text, session, untold) else {
            return;
        };
        let Some(engine) = self.list.engine().wake_engine(true).await else {
            return;
        };
        if !matches!(engine.session(session).await, Ok(Some(_))) {
            return;
        }
        let line = format!("You answered: {text}");
        let note = json!({ "kuvo": { "note": { "from": "Your answer", "line": line } } });
        let said = format!("[The user answered] {text}");
        let Ok(message_id) = engine.message_id() else {
            return;
        };
        let (from, metadata, steer) = ("Your answer", Some(note), false);
        let message = BoxMessage {
            id: message_id,
            text: said,
            metadata,
            from,
            words: text,
            steer,
        };
        if engine.tell(session, message).await.is_ok() {
            let _ = self.change_asks(|asks| {
                let kept = asks.iter_mut().filter(|each| each["id"] == id);
                kept.for_each(|kept| kept["toldLate"] = json!(true));
            });
        }
    }
}
