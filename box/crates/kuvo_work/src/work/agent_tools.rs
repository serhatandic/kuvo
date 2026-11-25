//! The agents' tools (PROTOCOL.md, "Agents' tools"): `POST kuvo/agent-tools`, which the tool
//! servers on every box pass an agent's call to (`{name, arguments, session}`), and what each
//! says back (`{text, isError}`). A conversation runs the sessions it started with them, here and
//! on your other computers (PROTOCOL.md, "Work"), as a subagent does as its conversation.

mod calls;
mod sessions;

use std::time::Duration;

use http::{Method, StatusCode};
use kuvo_engine::BotEngine;
use kuvo_peers::calls::{Bot, Lead, Rule};
use kuvo_protocol::routes::post_kuvo_agent_tools as route;
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use serde_json::{Map, Value, json};

use super::{
    Work,
    watch::{encoded, tools::Said},
};
use crate::bots::here::BotHere;

/// The tools, by name.
pub const AGENT_TOOLS: &[&str] = &[
    "list_agents",
    "send_message",
    "read_conversation",
    "start_session",
    "list_sessions",
    "stop_session",
    "compact_session",
    "answer_ask",
    "list_models",
    "make_project",
    "list_projects",
    "notify_user",
    "plan_limits",
    "use_limit_reset",
    "schedule_wake_up",
    "watch_for_change",
    "list_wake_ups",
    "cancel_wake_up",
    "set_job",
    "show_in_panel",
    "show_file",
    "mark_final",
    "show_widget",
];

/// The tools this box doesn't answer yet, and the part of the rewrite each comes with.
const NOT_YET: &[(&str, &str)] = &[
    ("notify_user", "5.1c"),
    ("plan_limits", "5.4"),
    ("use_limit_reset", "5.4"),
    ("show_in_panel", "6.4"),
    ("show_file", "6.4"),
    ("mark_final", "6.4"),
    ("show_widget", "6.4"),
];

const WAIT: Duration = Duration::from_secs(10);

pub(super) fn said(text: impl Into<String>) -> Said {
    Said {
        text: text.into(),
        refused: false,
    }
}

pub(super) fn refused(text: impl Into<String>) -> Said {
    Said {
        text: text.into(),
        refused: true,
    }
}

pub(super) fn session_id(given: &str) -> bool {
    given.strip_prefix("ses_").is_some_and(|rest| {
        (1..=60).contains(&rest.len()) && rest.bytes().all(|byte| byte.is_ascii_alphanumeric())
    })
}

/// The conversation a call works for: its root here (a subagent's, the one that started it; a
/// coding agent's engine run, the one it runs for), its name as the others know it, and the bot it
/// is.
pub struct Caller {
    pub session: String,
    pub data: Value,
    pub name: String,
    pub bot: Option<BotHere>,
}

impl Caller {
    /// Who runs its sessions with these tools, as a call carries it to a computer they're on:
    /// its name, its bot, the rules it goes by here (Settings › Permissions, then its own
    /// conversation's), and whether its commands run free of the sandbox (no sandbox here yet:
    /// 6.1).
    pub fn lead(&self, work: &Work) -> Lead {
        let approvals = crate::approval::Approvals::new(&work.home, work.settings.is_mac());
        let here = approvals.permissions_here().unwrap_or_default();
        let here = here.into_iter().map(|rule| Rule {
            action: rule.action,
            resource: rule.resource,
            effect: rule.effect,
        });
        let own = self.data["permissions"].as_array().into_iter().flatten();
        let own = own.filter_map(|rule| {
            let text = |field: &str| rule[field].as_str().map(str::to_owned);
            Some(Rule {
                action: text("permission").or_else(|| text("action"))?,
                resource: text("pattern").or_else(|| text("resource"))?,
                effect: text("action")
                    .filter(|_| rule["permission"].is_string())
                    .or_else(|| text("effect"))?,
            })
        });
        Lead {
            name: self.name.clone(),
            free: true,
            rules: here.chain(own).collect(),
            bot: self.works_for(),
            start: None,
        }
    }

    /// The bot whose work it is: its own, or the one the session works for.
    pub fn works_for(&self) -> Option<Bot> {
        if let Some(bot) = &self.bot {
            let name = bot.described.get("name").and_then(Value::as_str);
            return Some(Bot {
                bot: bot.id.clone(),
                name: name.unwrap_or_default().to_owned(),
            });
        }
        super::watch::works_for(&self.data)?;
        serde_json::from_value(self.data["metadata"]["kuvo"]["by"].clone()).ok()
    }
}

impl Work {
    pub(super) fn register_tools(&self, routes: &mut Registry) {
        let work = self.clone();
        routes.register::<route::Spec, _, _>(Access::Any, move |call| {
            let work = work.clone();
            async move {
                let given: Value = serde_json::from_slice(&call.request.body).unwrap_or_default();
                let name = given["name"].as_str().unwrap_or_default();
                if !AGENT_TOOLS.contains(&name) {
                    let message = format!(
                        "Send {{name, arguments, session}}: name one of {}.",
                        AGENT_TOOLS.join(", ")
                    );
                    return Err(Refusal::json(
                        StatusCode::BAD_REQUEST,
                        json!({ "message": message }),
                    ));
                }
                let operation = given.get("operation");
                if operation.is_some_and(|operation| {
                    !operation.as_str().is_some_and(|id| {
                        (1..=80).contains(&id.len())
                            && id
                                .bytes()
                                .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
                    })
                }) {
                    return Err(Refusal::json(
                        StatusCode::BAD_REQUEST,
                        json!({ "message": "Invalid tool operation id." }),
                    ));
                }
                let args = given["arguments"].as_object().cloned().unwrap_or_default();
                let session = given["session"].as_str().filter(|id| session_id(id));
                let answer = work.agent_tool(name, &args, session).await;
                Ok(RouteReply::json::<200>(
                    serde_json::from_value(
                        json!({ "text": answer.text, "isError": answer.refused }),
                    )
                    .map_err(|_| {
                        Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, "Couldn't say it.")
                    })?,
                ))
            }
        });
    }

    /// The conversation `session` works for, here: itself, or up its starters' (a subagent's) and
    /// the conversations coding agents' engine runs are for, six steps at most.
    async fn root_of(&self, engine: &BotEngine, session: Option<&str>) -> Option<(String, Value)> {
        let mut id = session?.to_owned();
        let mut found = None;
        for _ in 0..6 {
            let path = format!("api/session/{}", encoded(&id));
            let Some(data) = engine
                .call(Method::GET, &path, None, WAIT)
                .await
                .ok()
                .filter(|data| data["id"] == id.as_str())
            else {
                break;
            };
            let above = data["parentID"]
                .as_str()
                .or_else(|| data["metadata"]["kuvo"]["engineFor"].as_str())
                .map(str::to_owned);
            found = Some((id, data));
            match above {
                Some(above) => id = above,
                None => break,
            }
        }
        found
    }

    /// A conversation here, as the tools name it: its bot's name, else its title.
    fn caller_of(&self, session: String, data: Value) -> Caller {
        let bot = self.bot_of_conversation(&session);
        let title = data["title"].as_str().filter(|title| !title.is_empty());
        let name = match &bot {
            Some(bot) => bot.described.get("name").and_then(Value::as_str),
            None => title,
        };
        let name = name.unwrap_or("New session").to_owned();
        Caller {
            session,
            data,
            name,
            bot,
        }
    }

    /// A tool's call (`name`, its `args`), from the conversation `session` (none from outside
    /// one): what it says back.
    pub async fn agent_tool(
        &self,
        name: &str,
        args: &Map<String, Value>,
        session: Option<&str>,
    ) -> Said {
        if let Some((_, part)) = NOT_YET.iter().find(|(tool, _)| *tool == name) {
            return refused(format!(
                "{name} isn't in this computer's Kuvo Code yet (part {part} of its rewrite). Ask the user to do it in the app."
            ));
        }
        let engine = self.list.engine().running_bot_engine().await;
        let root = match &engine {
            Some(engine) => self.root_of(engine, session).await,
            None => None,
        };
        let caller = root.map(|(session, data)| self.caller_of(session, data));
        if caller
            .as_ref()
            .is_some_and(|caller| super::watch::truthy(&caller.data["metadata"]["kuvo"]["seat"]))
            && matches!(name, "send_message" | "start_session")
        {
            return refused(
                "Not from a group chat: talk it through here; the work is handed out after the wrap-up.",
            );
        }
        let here = self.own_name_here();
        match name {
            "list_models" | "list_projects" => match self.computer_of(args.get("computer")) {
                Err(why) => refused(why),
                Ok(Some(peer)) => match &caller {
                    None => refused("Only a conversation on this computer can look at another."),
                    Some(caller) => self.read_elsewhere(name, peer, caller).await,
                },
                Ok(None) if name == "list_projects" => self.list_projects_tool(&here).await,
                Ok(None) => self.list_models_tool(&here).await,
            },
            "make_project" => match &caller {
                None => refused("Only a conversation on this computer makes projects here."),
                Some(caller) => self.make_project_tool(caller, args).await,
            },
            "list_sessions" | "stop_session" | "compact_session" | "answer_ask" => {
                let (Some(caller), Some(engine)) = (&caller, &engine) else {
                    return refused(
                        "Only a conversation on this computer has sessions of its own.",
                    );
                };
                if name == "list_sessions" {
                    let rows = self.live_rows(engine, &caller.session, 40).await;
                    let there = self.rows_everywhere(caller).await;
                    return self.describe_list(&rows.unwrap_or_default(), &there);
                }
                let session = args
                    .get("session")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .unwrap_or_default();
                if session_id(session) && !self.has_session_here(engine, session).await {
                    return self.elsewhere_tool(name, caller, args, session).await;
                }
                match name {
                    "stop_session" => self.stop_session_tool(engine, &caller.session, args).await,
                    "compact_session" => {
                        self.compact_session_tool(engine, &caller.session, args)
                            .await
                    }
                    _ => self.answer_here(engine, caller, args).await,
                }
            }
            _ => refused(format!(
                "{name} isn't in this computer's Kuvo Code yet (part 5.1e of its rewrite). Ask the user to do it in the app."
            )),
        }
    }

    /// answer_ask for one of the caller's sessions here: an allow judged as one from another
    /// computer is, by the rules the caller goes by.
    async fn answer_here(
        &self,
        engine: &BotEngine,
        caller: &Caller,
        args: &Map<String, Value>,
    ) -> Said {
        use kuvo_peers::calls::Admission;
        let allows = args.get("decision").and_then(Value::as_str) == Some("allow");
        match self.admit_answer(engine, &caller.session, args).await {
            Admission::Refused(answer) => return refused(answer.text),
            Admission::Permission(check) if allows => {
                let now = jiff::Timestamp::now().as_millisecond();
                if let Some(answer) = check.refusal(&caller.lead(self), now, now) {
                    return refused(answer.text);
                }
            }
            _ => {}
        }
        let by = match caller.works_for() {
            Some(bot) => json!(bot),
            None => json!({ "name": caller.name }),
        };
        self.answer_ask_tool(engine, &caller.session, by, args)
            .await
    }

    /// This computer, as its tools' words name it ("this Mac", "this machine").
    fn own_name_here(&self) -> String {
        self.settings.here().to_owned()
    }
}
