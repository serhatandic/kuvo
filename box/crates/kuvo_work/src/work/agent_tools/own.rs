//! Kuvo Code's own tools that aren't about other agents: a conversation's wake-ups and watches
//! (schedule_wake_up, watch_for_change, list_wake_ups, cancel_wake_up), through this computer's
//! `kuvo-wake` as an agent's own command would run it, and a bot's job (set_job).

use std::{path::Path, time::Duration};

use serde_json::{Map, Value, json};

use super::{Caller, Said, refused, said};
use crate::{bots::here::MOVING, work::Work};

/// What an agent hears instead of a watch made without asking, in a conversation that asks before
/// commands: the same watch as a command, which asks the user with the check written out.
const WATCH_AS_COMMAND: &str = "This conversation asks before running commands, and a watch runs its check by itself, so the user approves it first: start it with the shell command kuvo-wake watch <every> \"<check>\" \"<what to do when it changes>\" instead, which asks them.";

/// Whether a conversation with these rules asks before commands, but nothing asked the user about
/// a watch's check before its tool ran (its rules are from before watches asked).
fn watch_went_unasked(rules: &Value) -> bool {
    let rules: Vec<&Value> = rules.as_array().into_iter().flatten().collect();
    let is = |rule: &&Value, action: &str, effect: &str| {
        let (permission, pattern, effect_of) = (
            rule["permission"].as_str().or(rule["action"].as_str()),
            rule["pattern"].as_str().or(rule["resource"].as_str()),
            rule["action"]
                .as_str()
                .filter(|_| rule["permission"].is_string())
                .or(rule["effect"].as_str()),
        );
        permission == Some(action)
            && (action != "shell" || pattern == Some("*"))
            && effect_of == Some(effect)
    };
    rules.iter().any(|rule| is(rule, "shell", "ask"))
        && !rules
            .iter()
            .any(|rule| is(rule, "kuvo-browser_watch_for_change", "ask"))
}

impl Work {
    /// A wake-up tool's call, for the conversation `caller` is part of (a subagent's, the one that
    /// started it; a bot's, its folder), run from the folder it works in. Never from a group chat's
    /// seat: meetings are the chat's.
    pub(super) async fn wake_tool(
        &self,
        name: &str,
        args: &Map<String, Value>,
        session: Option<&str>,
        own: Option<&Value>,
        caller: Option<&Caller>,
    ) -> Said {
        let Some(session) = session else {
            return refused("This needs a conversation of its own.");
        };
        let seat = |data: Option<&Value>| {
            data.is_some_and(|data| !data["metadata"]["kuvo"]["seat"].is_null())
        };
        if seat(own) || seat(caller.map(|caller| &caller.data)) {
            return refused(
                "Not from a group chat: a chat can meet on a schedule instead (its editor's Meets on a Schedule).",
            );
        }
        let owner = caller.map_or(session, |caller| caller.session.as_str());
        let bot = self.bot_of_conversation(owner);
        if bot.is_none() && caller.is_none() {
            return refused("Kuvo Code doesn't know this conversation yet. Try again in a moment.");
        }
        let given = |key: &str| {
            args.get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
        };
        let verb: Vec<&str> = match name {
            "schedule_wake_up" => {
                let Some(when) = given("when") else {
                    return refused(
                        "Say when (when): +20m, +2h, +1d, or a time like 2026-10-21T09:00.",
                    );
                };
                let Some(note) = given("note") else {
                    return refused("Add a note for yourself (note): what to do when you wake up.");
                };
                let mut verb = vec!["add", when];
                if let Some(every) = given("every") {
                    verb.extend(["--every", every]);
                }
                if let Some(summary) = given("summary") {
                    verb.extend(["--summary", summary]);
                }
                verb.push(note);
                verb
            }
            "watch_for_change" => {
                // A watch runs a command whenever it's due: in Ask First, only once the user was asked.
                if own
                    .or(caller.map(|caller| &caller.data))
                    .is_some_and(|data| watch_went_unasked(&data["permissions"]))
                {
                    return refused(WATCH_AS_COMMAND);
                }
                let (Some(every), Some(check), Some(note)) =
                    (given("every"), given("check"), given("note"))
                else {
                    return refused(
                        "Give how often (every), the check (a shell command) and a note for when it changes.",
                    );
                };
                let mut verb = vec!["watch", every, check];
                if let Some(summary) = given("summary") {
                    verb.extend(["--summary", summary]);
                }
                verb.push(note);
                verb
            }
            "list_wake_ups" => vec!["list"],
            _ => {
                let Some(id) = given("id") else {
                    return refused("Say which one (id), from list_wake_ups.");
                };
                vec!["cancel", id]
            }
        };
        // Its folder: a bot's own, or the session's in the wake-ups folder, which kuvo-wake makes
        // with its first wake-up or watch.
        let folder = bot.as_ref().map_or_else(
            || self.home.join("wakes").join(owner),
            |bot| bot.folder.clone(),
        );
        let exists = folder
            .join(if bot.is_some() { "bot.json" } else { "session" })
            .exists();
        if !exists && name == "cancel_wake_up" {
            return refused(format!(
                "No wake-up \"{}\". See list_wake_ups.",
                given("id").unwrap_or_default()
            ));
        }
        if exists && crate::wakes::folder::moving(&folder) {
            return refused(MOVING);
        }
        let directory = caller.and_then(|caller| {
            caller.data["location"]["directory"]
                .as_str()
                .or(caller.data["directory"].as_str())
        });
        let cwd = directory
            .map(Path::new)
            .filter(|folder| folder.is_dir())
            .map_or_else(|| self.settings.account().to_owned(), Path::to_path_buf);
        let program = std::env::var_os("KUVO_WAKE_COMMAND")
            .map_or_else(|| self.home.join("bin/kuvo-wake"), Into::into);
        let target = bot.as_ref().map_or(owner.to_owned(), |bot| {
            bot.folder.to_string_lossy().into_owned()
        });
        let mut command = tokio::process::Command::new(&program);
        command
            .arg(&target)
            .args(&verb)
            .current_dir(&cwd)
            .env("PWD", &cwd)
            .env("KUVO_BOTS", &self.bots)
            .env("KUVO_WAKES", self.home.join("wakes"))
            .env("KUVO_WAKE_TOOL", "1")
            // (Never as the box's own conversation, nor in a sandbox's folder: it's named.)
            .env_remove("KUVO_WAKE_VIA")
            .env_remove("OPENCODE_SESSION_ID")
            .env_remove("KUVO_WAKE_FOLDER")
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        let ran = tokio::time::timeout(Duration::from_secs(30), command.output()).await;
        if name != "list_wake_ups" {
            self.list.wakes().changed();
        }
        match ran {
            Ok(Ok(output)) => {
                let text = format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                let text = text.trim();
                let failed = !output.status.success();
                let text = if text.is_empty() {
                    if failed { "That didn't work." } else { "Done." }
                } else {
                    text
                };
                Said {
                    text: text.to_owned(),
                    refused: failed,
                }
            }
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => refused(format!(
                "Couldn't run kuvo-wake here: there's none at {}.",
                program.display()
            )),
            _ => refused("That didn't work."),
        }
    }

    /// set_job: a bot keeping its job in its record, as Edit Bot changes it, for the bot whose
    /// conversation this is (a subagent's, the one that started it).
    pub(super) async fn set_job_tool(
        &self,
        args: &Map<String, Value>,
        session: Option<&str>,
        caller: Option<&Caller>,
    ) -> Said {
        let Some(session) = session else {
            return refused("This needs a conversation of its own.");
        };
        let Some(bot) =
            self.bot_of_conversation(caller.map_or(session, |caller| caller.session.as_str()))
        else {
            return refused("Only a bot keeps a job, and this conversation isn't one.");
        };
        let clear = args.get("clear") == Some(&Value::Bool(true));
        let line = args
            .get("summary")
            .or(args.get("job"))
            .and_then(Value::as_str);
        let details = args.get("details").and_then(Value::as_str);
        let job = match (clear, line, details) {
            (true, ..) => json!({ "line": "", "details": "" }),
            (false, Some(line), Some(details)) if !line.trim().is_empty() => {
                json!({ "line": line, "details": details })
            }
            _ => {
                return refused(
                    "Provide a non-empty summary and details, or clear: true to clear your job.",
                );
            }
        };
        match self.list.change(&bot.id, json!({ "job": job })).await {
            Err((status, message)) => refused(if message.is_empty() {
                format!("Couldn't keep it ({status}).")
            } else {
                message
            }),
            Ok(answer) => match answer["bot"]["job"].as_str() {
                None => refused(
                    "Kuvo on this computer returned an unreadable job response. Check the job before trying again.",
                ),
                Some("") => said("Kept: you have no job now."),
                Some(kept) => said(format!("Kept. Kuvo Code shows \"{kept}\" under your name.")),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::watch_went_unasked;
    use serde_json::json;

    #[test]
    fn a_watch_goes_unasked_only_where_commands_ask_and_the_tool_itself_doesnt() {
        let asks = json!([{ "permission": "shell", "pattern": "*", "action": "ask" }]);
        assert!(watch_went_unasked(&asks));
        let both = json!([{ "permission": "shell", "pattern": "*", "action": "ask" }, { "permission": "kuvo-browser_watch_for_change", "pattern": "*", "action": "ask" }]);
        assert!(!watch_went_unasked(&both));
        assert!(!watch_went_unasked(
            &json!([{ "action": "shell", "resource": "*", "effect": "allow" }])
        ));
        assert!(!watch_went_unasked(&json!(null)));
    }
}
