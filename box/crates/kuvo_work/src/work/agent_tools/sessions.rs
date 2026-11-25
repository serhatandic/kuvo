//! list_sessions' words, for the caller's sessions here and on your other computers, and
//! make_project: a new project, made once the user allows it (at once for a bot they said Always
//! to), its tool waiting for their answer a while.

use std::{path::Path, time::Duration};

use serde_json::{Map, Value, json};

use super::{Caller, Said, calls::RowsThere, refused, said};
use crate::work::{
    Work,
    asks::{folder_name, project_place},
    projects::{cloneable, is_empty_folder, named_place, shown_path},
};

/// A count as the app writes it, with commas.
pub(super) fn counted(count: u64) -> String {
    let digits = count.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// How long ago `at` was, in words.
fn ago(at: i64, now: i64) -> String {
    let minutes = ((now - at).max(0) as f64 / 60_000.0).round() as i64;
    if minutes < 2 {
        return "a minute ago".into();
    }
    if minutes < 90 {
        return format!("{minutes} minutes ago");
    }
    let hours = (minutes as f64 / 60.0).round() as i64;
    if hours < 36 {
        return format!("{hours} hours ago");
    }
    format!("{} days ago", (hours as f64 / 24.0).round() as i64)
}

impl Work {
    /// Where a session works, in words.
    fn place_words(&self, row: &Value) -> String {
        if let Some(project) = row["project"].as_str() {
            return format!("in {project}");
        }
        let folder = row["folder"].as_str().unwrap_or_default();
        if Path::new(folder) == self.home.join("chats") {
            return "in the chats".into();
        }
        let base = Path::new(folder)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        format!(
            "in {}",
            base.filter(|base| !base.is_empty())
                .as_deref()
                .unwrap_or(folder)
        )
    }

    /// One session in a line (and its last reply under it), as list_sessions says it.
    fn row_line(&self, row: &Value) -> String {
        let text = |field: &str| row[field].as_str().unwrap_or_default();
        let model = &row["model"];
        let model = match (model["providerID"].as_str(), model["id"].as_str()) {
            (Some(provider), Some(id)) => {
                let variant = model["variant"]
                    .as_str()
                    .filter(|variant| !variant.is_empty());
                let variant = variant
                    .map(|variant| format!(" ({variant})"))
                    .unwrap_or_default();
                format!(" · {provider}/{id}{variant}")
            }
            _ => String::new(),
        };
        let state = if text("state") == "asking" {
            let asks = text("asks");
            let asks = asks.strip_prefix("Wants to ").unwrap_or(asks);
            let asks = asks.strip_suffix('.').unwrap_or(asks);
            let id = row["askID"]
                .as_str()
                .map(|id| format!(" (ask {id})"))
                .unwrap_or_default();
            format!("asks to {asks}{id}")
        } else {
            text("state").to_owned()
        };
        let under = if row["depth"].as_u64().unwrap_or(1) > 1 {
            format!(", started by {}", text("startedBy"))
        } else {
            String::new()
        };
        let reply = match row["lastReply"].as_str() {
            Some(reply) if !reply.is_empty() && text("state") != "asking" => {
                let head = if row["replyBeforeStop"] == true {
                    "Last reply, from its turn that ended before it was stopped"
                } else {
                    "Last reply"
                };
                format!("\n  {head}: {reply}")
            }
            _ => String::new(),
        };
        let context = &row["context"];
        let held = if let Some(used) = context["used"].as_u64() {
            format!(" · {} tokens in context now", counted(used))
        } else if context.is_object() {
            " · just summarized, context size known after its next step".into()
        } else {
            String::new()
        };
        let tokens = row["tokens"].as_u64().filter(|tokens| *tokens > 0);
        let tokens = tokens
            .map(|tokens| format!(" · {} tokens used in total", counted(tokens)))
            .unwrap_or_default();
        format!(
            "- {} (address {}), {}: {state}{model}{under}{held}{tokens}{reply}",
            text("title"),
            text("session"),
            self.place_words(row)
        )
    }

    /// list_sessions' words: the caller's sessions here (`rows`), then those on each other
    /// computer, live or as of when it last told.
    pub fn describe_list(&self, rows: &[Value], there: &[RowsThere]) -> Said {
        let others: Vec<&RowsThere> = there.iter().filter(|each| !each.rows.is_empty()).collect();
        if rows.is_empty() && others.is_empty() {
            return said(
                "You haven't started any sessions (or they're archived). Start one with start_session.",
            );
        }
        let lines = |rows: &[Value]| {
            rows.iter()
                .map(|row| self.row_line(row))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let mut parts = Vec::new();
        if !rows.is_empty() {
            let on = if others.is_empty() {
                String::new()
            } else {
                format!(" on {}", self.settings.here())
            };
            parts.push(format!(
                "Your sessions{on}, the latest first:\n{}",
                lines(rows)
            ));
        }
        let now = jiff::Timestamp::now().as_millisecond();
        for each in others {
            let name = &each.name;
            let head = match &each.away {
                Some(away) => {
                    let when = each
                        .as_of
                        .map_or_else(|| "when it last told".into(), |at| ago(at, now));
                    format!(
                        "On {name}, as of {when}. {name} isn't answering ({away}): what it asks waits, and a stop or an answer you give is kept and done when it's back."
                    )
                }
                None => format!("On {name}, the latest first:"),
            };
            parts.push(format!("{head}\n{}", lines(&each.rows)));
        }
        said(parts.join("\n\n"))
    }

    /// Whether the user said Always to `action` for the bot `bot`.
    fn bot_allows(&self, bot: &str, action: &str) -> bool {
        let here = crate::bots::here::BotHere::read(&self.bots, bot);
        here.is_some_and(|here| {
            let allowed = here
                .described
                .get("work")
                .and_then(|work| work["allowed"].as_array().cloned());
            allowed.unwrap_or_default().contains(&json!(action))
        })
    }

    /// make_project: a new project, in the projects folder the user picked (or the folder named
    /// for it), made once they allow it. Waits for their answer a while (not on Autopilot); after
    /// that it stays asked, and the conversation is told when they answer.
    pub async fn make_project_tool(&self, caller: &Caller, args: &Map<String, Value>) -> Said {
        let text = |field: &str| {
            args.get(field)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
        };
        let Some(name) = text("name") else {
            return refused("Say what it's called (name).");
        };
        if folder_name(name).is_none() {
            return refused(format!(
                "{name} can't be a folder's name: use letters or numbers."
            ));
        }
        let clone = text("clone");
        if let Some(clone) = clone.filter(|clone| !cloneable(clone)) {
            return refused(format!(
                "{clone} isn't a repository address git can clone: give an https or ssh one."
            ));
        }
        let account = self.settings.account();
        let named = match text("folder") {
            Some(folder) => match named_place(Some(folder), account) {
                Ok(folder) => Some(folder),
                Err((_, why)) => return refused(why),
            },
            None => None,
        };
        // What would fail anyway isn't asked.
        let root = self.settings.projects_folder();
        let place = named
            .clone()
            .or_else(|| project_place(name, root.as_deref()));
        if let Some(place) = &place {
            let shown = shown_path(&place.to_string_lossy(), account);
            let projects = kuvo_services::git::read_projects(&self.home).unwrap_or_default();
            let listed = projects.iter().find(|each| {
                each.get("directory").and_then(Value::as_str) == Some(&*place.to_string_lossy())
                    && each.get("removed") != Some(&Value::Bool(true))
            });
            if let Some(listed) = listed {
                let base = place
                    .file_name()
                    .map(|base| base.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let called = listed
                    .get("name")
                    .and_then(Value::as_str)
                    .map_or(base, str::to_owned);
                return refused(format!(
                    "There's a project at {shown} already: {called}. Start sessions in it with start_session."
                ));
            }
            if place.exists() && !is_empty_folder(place) {
                let then = if named.is_some() {
                    format!(
                        "To work on what's there, start sessions in it with start_session (folder: \"{shown}\")."
                    )
                } else {
                    "Pick another name.".into()
                };
                return refused(format!(
                    "There's a folder at {shown} already, with files in it. {then}"
                ));
            }
        }
        let bot = caller.bot.as_ref();
        let asker = bot
            .and_then(|bot| bot.described.get("name").and_then(Value::as_str))
            .unwrap_or(&caller.name);
        let action = if clone.is_some() {
            "cloneProject"
        } else {
            "makeProject"
        };
        let mut given = json!({ "name": name });
        if args.get("git") == Some(&Value::Bool(false)) {
            given["git"] = json!(false);
        }
        if let Some(clone) = clone {
            given["clone"] = json!(clone);
        }
        if let Some(named) = &named {
            given["folder"] = json!(named);
        }
        let mut ask = json!({ "action": action, "session": caller.session, "name": asker, "project": name, "given": given });
        if let Some(bot) = bot {
            ask["bot"] = json!(bot.id);
            if self.bot_allows(&bot.id, action) && (named.is_some() || root.is_some()) {
                let (text, failed) = self.do_work(&ask, false).await;
                return Said {
                    text,
                    refused: failed,
                };
            }
        }
        let Ok(id) = self.ask_work(ask, clone, named.as_deref(), root.as_deref()) else {
            return refused("Couldn't ask the user: this computer couldn't keep the ask.");
        };
        // On Autopilot it doesn't wait for the user: it stays asked, for when they're back.
        let away = caller.data["metadata"]["kuvo"]["autopilot"] == true;
        let wait = if away {
            Duration::ZERO
        } else {
            let given = std::env::var("KUVO_WORK_ASK_WAIT")
                .ok()
                .and_then(|wait| wait.parse().ok());
            Duration::from_millis(given.filter(|wait| *wait > 0).unwrap_or(600_000))
        };
        match self.answer_of(&id, wait).await {
            Some(answer) => Said {
                text: answer["text"].as_str().unwrap_or_default().to_owned(),
                refused: answer["isError"] == true,
            },
            None => said(format!(
                "The user hasn't answered yet: it stays asked on {}, and you'll hear when they answer. Carry on with what you can meanwhile.",
                self.settings.here()
            )),
        }
    }

    pub(super) async fn use_reset(
        &self,
        caller: Option<&Caller>,
        args: &Map<String, Value>,
    ) -> Said {
        let Some(caller) = caller else {
            return refused("Only a conversation on this computer can use a reset from here.");
        };
        let Some(wanted) = args
            .get("account")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return refused("Say which account (account), as plan_limits lists it.");
        };
        let Some(resets) = self.resets.as_ref() else {
            return refused("The coding agents aren't running here now, so no reset can be used.");
        };
        let row = match resets.find(wanted).await {
            Ok(row) => row,
            Err(why) => return refused(why),
        };
        let id = row["id"].as_str().unwrap_or_default();
        let name = row["title"].as_str().unwrap_or_default();
        let offer = match resets.offer(id).await {
            Ok(offer) => offer,
            Err(error) => {
                return refused(format!(
                    "{name}'s resets can't be used now: {}",
                    kuvo_services::usage::resets::refusal_words(error).await
                ));
            }
        };
        if offer["always"] == true {
            let (text, error) = resets.spend_for(id, &caller.name, false, true).await;
            return Said {
                text,
                refused: error,
            };
        }
        let mut reset =
            json!({"row":id,"account":name,"service":row["service"],"count":offer["count"]});
        if let Some(refills) = offer.get("refills") {
            reset["refills"] = refills.clone();
        }
        let mut ask = json!({"action":"resetLimits","session":caller.session,"name":caller.name,"project":name,"given":{"name":name,"reset":reset}});
        if let Some(bot) = caller.works_for() {
            ask["bot"] = json!(bot.bot);
        }
        let Ok(id) = self.ask_work(ask, None, None, None) else {
            return refused("Couldn't ask the user: this computer couldn't keep the ask.");
        };
        let wait = if caller.data["metadata"]["kuvo"]["autopilot"] == true {
            Duration::ZERO
        } else {
            Duration::from_millis(
                std::env::var("KUVO_WORK_ASK_WAIT")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .filter(|n| *n > 0)
                    .unwrap_or(600_000),
            )
        };
        match self.answer_of(&id, wait).await {
            Some(answer) => Said {
                text: answer["text"].as_str().unwrap_or_default().to_owned(),
                refused: answer["isError"] == true,
            },
            None => said(format!(
                "The user hasn't answered yet: it stays asked on {}, and you'll hear when they answer. Carry on with what you can meanwhile.",
                self.settings.here()
            )),
        }
    }

    /// What's asked, kept for the user to answer (one open card a conversation for each
    /// project): its id.
    fn ask_work(
        &self,
        mut ask: Value,
        clone: Option<&str>,
        named: Option<&Path>,
        root: Option<&Path>,
    ) -> Result<String, (u16, String)> {
        let project = ask["project"].as_str().unwrap_or_default().to_owned();
        let folder = named
            .map(Path::to_path_buf)
            .or_else(|| project_place(&project, root));
        let id = self.change_asks(|asks| {
            let same = asks.iter_mut().find(|each| {
                !each["answered"].is_object()
                    && each["session"] == ask["session"]
                    && each["action"] == ask["action"]
                    && if ask["action"] == "resetLimits" {
                        each["given"]["reset"]["row"] == ask["given"]["reset"]["row"]
                    } else {
                        each["project"].as_str().unwrap_or_default().to_lowercase()
                            == project.to_lowercase()
                    }
            });
            if let Some(same) = same {
                same.as_object_mut().map(|same| same.shift_remove("late"));
                return same["id"].as_str().unwrap_or_default().to_owned();
            }
            let mut bytes = [0u8; 12];
            let _ = getrandom::fill(&mut bytes);
            let letters = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
            let id: String = bytes
                .iter()
                .map(|byte| letters[usize::from(*byte) % 62] as char)
                .collect();
            let id = format!("wak_{id}");
            let base = named
                .and_then(Path::file_name)
                .map(|base| base.to_string_lossy().into_owned());
            ask["id"] = json!(id);
            if let Some(clone) = clone {
                ask["clone"] = json!(clone);
            }
            ask["folderName"] = json!(
                base.or_else(|| folder_name(&project))
                    .unwrap_or(project.clone())
            );
            ask["folder"] = json!(folder);
            if ask["action"] == "resetLimits" {
                ask["folder"] = Value::Null;
                ask["folderName"] = json!("");
                ask["reset"] = ask["given"]["reset"].clone();
            }
            ask["at"] = json!(jiff::Timestamp::now().as_millisecond());
            asks.push(ask.clone());
            id
        })?;
        if let Some(bot) = ask["bot"].as_str() {
            self.records.note_changed(bot);
        }
        Ok(id)
    }

    /// The answer to ask `id`, once the user gives it, until `wait` goes by: none then, and it
    /// stays asked (its conversation is told when they answer).
    async fn answer_of(&self, id: &str, wait: Duration) -> Option<Value> {
        let until = tokio::time::Instant::now() + wait;
        loop {
            let asks = self.kept_asks();
            let ask = asks.iter().find(|each| each["id"] == id)?;
            // (Its bot gone: there's no one to answer for.)
            if ask["bot"]
                .as_str()
                .is_some_and(|bot| crate::bots::here::BotHere::read(&self.bots, bot).is_none())
            {
                return None;
            }
            if ask["answered"].is_object() {
                return Some(ask["answered"].clone());
            }
            if tokio::time::Instant::now() >= until {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1).min(until - tokio::time::Instant::now()))
                .await;
        }
        // No longer waited for: told to its conversation once answered; one answered just now, here.
        self.change_asks(|asks| {
            let ask = asks.iter_mut().find(|each| each["id"] == id)?;
            if ask["answered"].is_object() {
                return Some(ask["answered"].clone());
            }
            ask["late"] = json!(true);
            None
        })
        .ok()
        .flatten()
    }
}
