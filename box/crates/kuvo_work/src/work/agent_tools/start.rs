//! start_session from a conversation's agent: here, as its conversation's, through the engine's
//! one start, with the Work Queue item it names linked before it sees its task; or on another of
//! your computers, checked here first as a start is and sent with who's calling as this computer
//! knows it. Both counted, and held to the limits you set for its bot (`limits`).

use kuvo_engine::{ParentLead, StartSteps};
use kuvo_peers::{calls::note_homes, directory::Member};
use serde_json::{Map, Value, json};

use super::{
    Caller, Said,
    calls::{AWAY_WORDS, Keep, Went, asleep, name_of, older_words},
    limits::Counted,
    receipts::Operation,
    refused, session_id,
};
use crate::{
    approval::Approval,
    bots::list::limits_from,
    work::{
        Work,
        starts::started_words,
        watch::{address_in, tools::line_of},
    },
};

/// A start's fields a call carries to another computer (the item, too, apart).
const START_FIELDS: &[&str] = &[
    "task", "title", "project", "folder", "chat", "worktree", "model", "effort", "mode",
    "approval", "wake", "review",
];
/// The most rules a start carries.
const MOST_HELD: usize = 300;

/// What a start here keeps as it goes: its session counted for its bot's limits, and the Work
/// Queue item it names (only from its agent's real call) linked before it's given its task.
struct Steps<'a> {
    work: &'a Work,
    counted: &'a Counted,
    /// The call's operation, which keeps what the start got to.
    operation: Option<&'a Operation>,
    item: Option<Value>,
    task: String,
    starter: Value,
}

impl StartSteps for Steps<'_> {
    fn making(&self, making: &Value) {
        if let Some(operation) = self.operation {
            operation.keep(&json!({ "making": making }));
        }
    }

    fn made(&self, session: &str) {
        self.counted.went(session, false);
    }

    fn prompted(&self) {
        if let Some(operation) = self.operation {
            let mut progress = operation.progress();
            progress["prompted"] = json!(true);
            operation.keep(&progress);
        }
    }

    fn prompting<'a>(
        &'a self,
        session: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if let (Some(link), Some(item)) = (self.work.links.get(), &self.item) {
                link.link(Some(item), &self.task, self.starter.clone(), session)
                    .await;
            }
        })
    }
}

impl Caller {
    /// The new session's depth under the bot its work is for.
    fn depth(&self) -> u64 {
        if self.bot.is_some() {
            1
        } else {
            self.data["metadata"]["kuvo"]["depth"]
                .as_u64()
                .unwrap_or(1)
                .max(1)
                + 1
        }
    }
}

impl Work {
    /// start_session here, for `caller` (none: an agent outside any conversation here).
    pub async fn start_here(
        &self,
        caller: Option<&Caller>,
        args: &Map<String, Value>,
        item: Option<Value>,
        operation: Option<&Operation>,
    ) -> Result<Said, String> {
        let Some(caller) = caller else {
            // (The tool servers here always say which conversation calls.)
            return Ok(refused(
                "Only a conversation on this computer can start sessions here.",
            ));
        };
        // Sent again after it was cut short: the session it made, gone on with.
        if let Some(operation) = operation
            && let Some(said) = self
                .resume_start(caller, operation, item.as_ref(), args)
                .await?
        {
            return Ok(said);
        }
        let depth = caller.depth();
        let counted = match self.count_start(caller, depth).await {
            Ok(counted) => counted,
            Err(why) => return Ok(refused(why)),
        };
        let mut given = args.clone();
        for field in ["computer", "wait", "item"] {
            given.remove(field);
        }
        given.insert("startedBy".into(), json!(caller.session));
        let bot = caller.works_for();
        let works = bot.as_ref().map(|bot| json!(bot));
        let lead = ParentLead {
            name: caller.name.clone(),
            call: operation.map(|operation| operation.key.clone()),
            works,
            own: caller.bot.is_some(),
            depth,
        };
        let task = args
            .get("task")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into();
        let starter = json!({ "session": caller.session, "name": caller.name });
        let steps = Steps {
            work: self,
            counted: &counted,
            operation,
            item,
            task,
            starter,
        };
        match self
            .list
            .engine()
            .start_for_parent(&given, &lead, &steps)
            .await
        {
            Ok(started) => {
                let session = started["session"].as_str().unwrap_or_default();
                counted.went(session, true);
                if let Some(bot) = &bot {
                    self.records.note_changed(&bot.bot);
                    // (Started by the bot itself, for clients to show it looking after it.)
                    if caller.bot.is_some() && started["wakes"].is_array() {
                        self.records.note_oversight(&bot.bot, session, "started");
                    }
                }
                Ok(Said {
                    text: started_words(&started),
                    refused: false,
                    delivery: None,
                })
            }
            Err(refusal) => {
                let reply = refusal.reply();
                let status = reply.status;
                use http_body_util::BodyExt;
                let body = reply.body.collect().await.map(|body| body.to_bytes()).ok();
                let body: Value = body
                    .and_then(|body| serde_json::from_slice(&body).ok())
                    .unwrap_or_default();
                // Made, its task not given, or not known to be made: sent again, it's gone on with.
                if operation.is_some() && status == http::StatusCode::BAD_GATEWAY {
                    return Err(match body["session"].as_str() {
                        Some(session) => {
                            counted.went(session, false);
                            "Waiting for the new session to confirm its task.".into()
                        }
                        None => {
                            counted.back();
                            "Waiting for the engine to confirm the session was created.".into()
                        }
                    });
                }
                counted.back();
                Ok(refused(
                    body["message"]
                        .as_str()
                        .unwrap_or("it couldn't be done now"),
                ))
            }
        }
    }

    /// A start cut short, sent again: the session its call made (as its mark says), given its task
    /// if it wasn't yet, and what was said of it; none where none was made, so it's started.
    async fn resume_start(
        &self,
        caller: &Caller,
        operation: &Operation,
        item: Option<&Value>,
        args: &Map<String, Value>,
    ) -> Result<Option<Said>, String> {
        let progress = operation.progress();
        let Some(engine) = self.list.engine().running_bot_engine().await else {
            return Err("Waiting for the new session's engine.".into());
        };
        let page = engine
            .call(
                http::Method::GET,
                "api/session?limit=50&order=desc&parentID=null",
                None,
                std::time::Duration::from_secs(10),
            )
            .await
            .map_err(|_| {
                "Waiting for the engine to confirm the new session and its task.".to_owned()
            })?;
        let list = page["data"]
            .as_array()
            .or(page.as_array())
            .cloned()
            .unwrap_or_default();
        let Some(session) = list
            .iter()
            .find(|each| each["metadata"]["kuvo"]["call"] == operation.key.as_str())
            .and_then(|each| each["id"].as_str())
        else {
            return Ok(None);
        };
        let making = &progress["making"];
        if progress["prompted"] != true
            && let Some(text) = making["prompt"]["text"].as_str()
        {
            let task = args.get("task").and_then(Value::as_str).unwrap_or_default();
            if let (Some(link), Some(item)) = (self.links.get(), item) {
                let starter = json!({ "session": caller.session, "name": caller.name });
                link.link(Some(item), task, starter, session).await;
            }
            let mover = self
                .list
                .engine()
                .session_move()
                .await
                .map_err(|error| error.to_string())?;
            let letter = kuvo_engine::LetterPrompt {
                id: making["prompt"]["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                immediate: kuvo_engine::LetterText {
                    text: text.to_owned(),
                    metadata: Some(kuvo_engine::parts_metadata(text)),
                },
                when_queued: None,
                from: caller.name.clone(),
                words: text.to_owned(),
                timing: kuvo_engine::LetterTiming::Legacy { steer: false },
            };
            if mover.deliver_letter(session, letter).await.is_err() {
                return Err("Waiting for the new session to confirm its task.".into());
            }
            let mut progress = progress.clone();
            progress["prompted"] = json!(true);
            operation.keep(&progress);
        }
        let mut started = making["started"].clone();
        if !started.is_object() {
            started = json!({});
        }
        started["session"] = json!(session);
        Ok(Some(Said {
            text: started_words(&started),
            refused: false,
            delivery: None,
        }))
    }

    /// start_session on another of your computers (`peer`), from `caller` here.
    pub async fn start_elsewhere(
        &self,
        peer: Member,
        caller: &Caller,
        args: &Map<String, Value>,
        item: Option<Value>,
    ) -> Said {
        let name = name_of(&peer);
        let text = |field: &str| {
            args.get(field)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
        };
        let Some(task) = text("task") else {
            return refused("Say what it's to do (task).");
        };
        let older = older_words(&name);
        if !self
            .peers
            .view()
            .advertises(&peer.key, "work.elsewhere.start")
        {
            return refused(older);
        }
        let data = &caller.data;
        let given = |field: &str| args.get(field).filter(|value| !value.is_null());
        if given("mode").is_some_and(|mode| mode != "build" && mode != "plan") {
            return refused("mode is build or plan.");
        }
        if given("mode").is_some_and(|mode| mode == "build") && data["agent"] == "plan" {
            return refused(
                "This conversation is in Plan mode, so the sessions it starts plan too. Start it in plan mode, or ask the user to switch to Build.",
            );
        }
        if given("approval").is_some_and(|approval| Approval::of(approval).is_none()) {
            return refused("approval is askFirst, acceptEdits, auto or fullAccess.");
        }
        // (A folder here means nothing there.)
        if args.get("chat") != Some(&Value::Bool(true))
            && text("project").is_none()
            && text("folder").is_none()
        {
            return refused(format!(
                "Say where it works on {name}: one of its projects (project; list_projects with computer \"{name}\" lists them), a folder there (folder, a full path), or chat."
            ));
        }
        // Who's calling, as this computer knows it: its approval as it comes to here now, and its
        // Don't Allows.
        let approvals = crate::approval::Approvals::new(&self.home, self.settings.is_mac());
        let bot = caller.works_for();
        let approval = approvals
            .of(
                data,
                bot.is_some(),
                approvals.since(),
                approvals.default_approval(),
            )
            .effective;
        let lead = caller.lead(self);
        let held: Vec<Value> = lead
            .rules
            .iter()
            .filter(|rule| rule.effect != "allow" && rule.effect != "ask")
            .map(|rule| json!(rule))
            .collect();
        if held.len() > MOST_HELD {
            return refused(format!(
                "Your rules are too many to carry to {name}. Start it here, or ask the user."
            ));
        }
        let depth = caller.depth();
        let counted = match self.count_start(caller, depth).await {
            Ok(counted) => counted,
            Err(why) => return refused(why),
        };
        let failed = |words: String| {
            counted.back();
            refused(words)
        };
        let run = self.home.join("run");
        let own = self
            .peers
            .view()
            .roster
            .here
            .map(|key| kuvo_wire::keys::encode(&key));
        let mut line: Vec<String> = line_of(data).into_iter().map(str::to_owned).collect();
        line.push(caller.session.clone());
        let line = line[line.len().saturating_sub(20)..].to_vec();
        let homes: Map<String, Value> = line
            .iter()
            .filter_map(|session| {
                let known = kuvo_peers::calls::known_home(&run, session)
                    .map(|key| kuvo_wire::keys::encode(&key));
                Some((session.clone(), json!(known.or(own.clone())?)))
            })
            .collect();
        let model = &data["model"];
        let model = match (model["providerID"].as_str(), model["id"].as_str()) {
            (Some(provider), Some(id)) if !provider.is_empty() && !id.is_empty() => {
                let mut model = json!({ "providerID": provider, "id": id });
                if let Some(variant) = data["model"]["variant"]
                    .as_str()
                    .filter(|variant| !variant.is_empty())
                {
                    model["variant"] = json!(variant);
                }
                model
            }
            _ => Value::Null,
        };
        let limits = bot
            .as_ref()
            .and(limits_from(data["metadata"]["kuvo"].get("limits")));
        let limits = bot
            .as_ref()
            .and_then(|bot| {
                let here = crate::bots::here::BotHere::read(&self.bots, &bot.bot)?;
                limits_from(here.described.get("work")?.get("limits"))
            })
            .or(limits);
        let plan = (data["agent"] == "plan").then_some("plan");
        let start = json!({
            "approval": approval, "held": held, "agent": plan, "model": model, "autopilot": data["metadata"]["kuvo"]["autopilot"] == true,
            "works": bot, "own": caller.bot.is_some(), "depth": depth, "line": line, "limits": limits, "homes": homes,
        });
        let mut sent: Map<String, Value> = START_FIELDS
            .iter()
            .filter_map(|field| Some(((*field).to_owned(), given(field)?.clone())))
            .collect();
        sent.insert("task".into(), json!(task));
        if let Some(item) = item.filter(|item| !item.is_null()) {
            sent.insert("item".into(), item);
        }
        match self
            .call_peer(&peer, "start_session", sent, caller, Keep::No, Some(start))
            .await
        {
            Went::Answer(answer) => {
                // (Its words are that computer's: said so, as they would be had they come as a
                // message.)
                let text = if answer.text.starts_with("[On ") {
                    answer.text
                } else {
                    format!("[On {name}] {}", answer.text)
                };
                match address_in(&text).filter(|id| session_id(id)) {
                    // Refused there, with nothing made: it didn't start.
                    None if answer.refused => failed(text),
                    None => Said {
                        text,
                        refused: false,
                        delivery: None,
                    },
                    Some(session) => {
                        // Made there: it lives there, so a stop or an answer for it goes there at
                        // once, before its first rows come.
                        let _ = note_homes(
                            &run,
                            peer.key,
                            jiff::Timestamp::now().as_millisecond(),
                            std::iter::once(session),
                        );
                        self.note_started_on(&caller.session, &peer.key);
                        counted.went(session, !answer.refused);
                        Said {
                            text,
                            refused: answer.refused,
                            delivery: None,
                        }
                    }
                }
            }
            Went::Failed(why) => failed(format!("On {name}: {why}")),
            Went::Older => failed(older),
            Went::Offline { .. } => failed(self.offline_words(&name)),
            Went::Away {
                why, sent: false, ..
            } if why == AWAY_WORDS => failed(asleep(&name, "nothing was started")),
            Went::Away { why, .. } => {
                // It went, and no answer came: it may have started. Not tried again, so it's never
                // started twice.
                counted.unknown();
                let (why, back) = if why == AWAY_WORDS {
                    (
                        format!("it stopped answering ({AWAY_WORDS}) after the start went"),
                        format!(" once {name} is back"),
                    )
                } else {
                    ("it didn't answer in time".to_owned(), String::new())
                };
                refused(format!(
                    "Couldn't confirm the session started on {name}: {why}. It may still have started, and if it did you'll hear as a message{back}. Check with list_sessions before you start it again."
                ))
            }
        }
    }
}
