//! Work this computer does for a lead that lives on another (PROTOCOL.md, "Peers", `call`): its
//! sessions here listed, stopped and summarized for it, as its own computer's tools do there, and
//! the rows of them told to the computer it lives on. The calls themselves (who sent them, once
//! only, kept for a computer that's away) are the peers' (`kuvo_peers::remote_work`); what's here
//! checks each against the sessions here and does it.

use std::{
    collections::HashSet,
    io,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use kuvo_peers::{
    calls::{
        Admission, Answer, MailedCall, Permission, RemoteWork, Resource, Rule, Tool, WorkFuture,
    },
    directory::ComputerKey,
    outbox::Id,
    remote_work::WorkHandle,
    work_rows::{Row, RowsByLead},
};

use serde_json::{Value, json};

use super::{
    Work,
    watch::{
        started_by,
        tools::{Ask, line_of, working_now},
    },
};

/// How many of each lead's sessions its computer is told of.
const ROWS_EACH: usize = 40;
/// How often the rows are worked out again with nothing said to change.
const AGAIN: Duration = Duration::from_secs(60);
const LOOK: Duration = Duration::from_secs(5);

fn not_now(why: impl Into<String>) -> io::Error {
    io::Error::other(why.into())
}

/// The sessions here a lead on another computer runs, as the peers reach them. Made as the
/// resident starts, before its work is: it takes no call until that's there.
#[derive(Clone, Default)]
pub struct Elsewhere {
    work: Arc<OnceLock<Work>>,
}

impl Elsewhere {
    pub fn new() -> Self {
        Self::default()
    }

    /// The box's work is there: calls are taken from now on, and the rows of what it does for
    /// leads elsewhere told through `peers`.
    pub fn serve(&self, work: Work, peers: WorkHandle) -> tokio::task::JoinHandle<()> {
        let _ = self.work.set(work.clone());
        tokio::spawn(work.tell_rows(peers))
    }

    fn work(&self) -> io::Result<&Work> {
        self.work
            .get()
            .ok_or_else(|| not_now("This computer is still starting."))
    }
}

impl RemoteWork for Elsewhere {
    fn inspect<'a>(&'a self, _: ComputerKey, call: &'a MailedCall) -> WorkFuture<'a, Admission> {
        Box::pin(async move {
            let work = match self.work() {
                Ok(work) => work,
                Err(why) => return Ok(Admission::Retry(why.to_string())),
            };
            let tool = call.request.tool;
            if !matches!(
                tool,
                Tool::ListSessions | Tool::StopSession | Tool::CompactSession | Tool::AnswerAsk
            ) {
                return Ok(Admission::Reject(
                    "That isn't something another computer can do here.".into(),
                ));
            }
            let Some(engine) = work.list.engine().running_bot_engine().await else {
                return Ok(Admission::Retry("The engine isn't running.".into()));
            };
            // Who it's for lives on the computer that called: never one of this computer's own.
            if work.has_session_here(&engine, &call.from).await {
                return Ok(Admission::Reject(
                    "That conversation is on this computer: another can't speak for it.".into(),
                ));
            }
            if let Some(session) = call
                .request
                .session()
                .filter(|_| tool != Tool::ListSessions)
                && !work.has_session_here(&engine, session).await
            {
                return Ok(Admission::Reject(format!(
                    "There's no session {session} on this computer."
                )));
            }
            if tool == Tool::AnswerAsk {
                return Ok(work.admit_answer(&engine, call).await);
            }
            Ok(Admission::Ready)
        })
    }

    fn apply<'a>(&'a self, _: ComputerKey, call: &'a MailedCall) -> WorkFuture<'a, Answer> {
        Box::pin(async move {
            let work = self.work()?;
            let engine = work.list.engine().running_bot_engine().await;
            let engine = engine.ok_or_else(|| not_now("The engine isn't running."))?;
            let (lead, args) = (call.from.as_str(), &call.request.args);
            let said = match call.request.tool {
                Tool::ListSessions => {
                    let rows = work.live_rows(&engine, lead, ROWS_EACH).await;
                    let rows = rows.ok_or_else(|| not_now("Its sessions can't be read now."))?;
                    return Ok(Answer::rows(rows));
                }
                Tool::StopSession => work.stop_session_tool(&engine, lead, args).await,
                Tool::CompactSession => work.compact_session_tool(&engine, lead, args).await,
                Tool::AnswerAsk => {
                    let as_lead = &call.request.lead;
                    let by = match &as_lead.bot {
                        Some(bot) => json!({ "bot": bot.bot, "name": bot.name }),
                        None => json!({ "name": as_lead.name }),
                    };
                    work.answer_ask_tool(&engine, lead, by, args).await
                }
                _ => {
                    return Err(not_now(
                        "That isn't something another computer can do here.",
                    ));
                }
            };
            Ok(Answer {
                text: said.text,
                refused: said.refused,
            })
        })
    }

    fn deliver<'a>(
        &'a self,
        _: &'a Id,
        session: &'a str,
        answer: &'a Answer,
    ) -> WorkFuture<'a, ()> {
        Box::pin(async move {
            // (This computer's own leads call others with the agents' tools, 5.1e: until then no
            // call of its own is answered.)
            let _ = (session, answer);
            Err(not_now("This computer makes no calls of its own yet."))
        })
    }
}

impl Work {
    /// What an answer from a lead elsewhere may give: refused here as its own tool would refuse
    /// it, or, for an allow, what the peers judge it by (the lead's own rules where it lives and
    /// this computer's, each on its own).
    async fn admit_answer(&self, engine: &kuvo_engine::BotEngine, call: &MailedCall) -> Admission {
        let refused = |text: String| Admission::Refused(Answer::refused(text));
        let args = &call.request.args;
        let (data, permission) = match self.ask_to_answer(engine, &call.from, args).await {
            Err(said) => return refused(said.text),
            Ok((_, Ask::Question(_))) => return Admission::Ready,
            Ok((data, Ask::Permission(permission))) => (data, permission),
        };
        match args.get("decision").and_then(Value::as_str) {
            Some("deny") => return Admission::Ready,
            Some("allow") => {}
            _ => return refused("Say allow or deny (decision).".into()),
        }
        if let Some(why) = self.ask_first_refusal(&data) {
            return refused(why);
        }
        let checks = self.allow_checks(engine, &data, &permission).await;
        let approvals = crate::approval::Approvals::new(&self.home, self.settings.is_mac());
        let local_rules = approvals.permissions_here().unwrap_or_default();
        let title = data["title"].as_str().filter(|title| !title.is_empty());
        Admission::Permission(Permission {
            title: title.unwrap_or("New Session").to_owned(),
            ask_first: false,
            // (The sandbox's state is 6.1's: until then nothing here counts as run in it.)
            sandboxed: false,
            checks: checks
                .into_iter()
                .map(|(action, value)| Resource { action, value })
                .collect(),
            local_rules: local_rules
                .into_iter()
                .map(|rule| Rule {
                    action: rule.action,
                    resource: rule.resource,
                    effect: rule.effect,
                })
                .collect(),
        })
    }

    /// The rows of the sessions here whose lead lives elsewhere, by lead: each session's leads are
    /// the first conversation above it, up its starters, that isn't here, and those in its line
    /// that aren't. Which computer each lives on is the peers' to say. None where the engine
    /// can't say now.
    async fn rows_elsewhere(&self) -> Option<RowsByLead> {
        let engine = self.list.engine().running_bot_engine().await?;
        let all = super::watch::tools::all_work(&engine).await?;
        let here: HashSet<&str> = all.iter().filter_map(|each| each["id"].as_str()).collect();
        let by_id = |id: &str| all.iter().find(|each| each["id"] == id);
        let working = working_now(&engine).await;
        let mut leads = RowsByLead::new();
        for each in &all {
            let mut above: Vec<&str> = Vec::new();
            let mut at = Some(each);
            for _ in 0..20 {
                let Some(data) = at else { break };
                for session in line_of(data) {
                    if !here.contains(session) && !above.contains(&session) {
                        above.push(session);
                    }
                }
                let Some(starter) = started_by(data) else {
                    break;
                };
                if !here.contains(starter) && !above.contains(&starter) {
                    above.push(starter);
                }
                at = by_id(starter);
            }
            let mut row: Option<Row> = None;
            for lead in above {
                let rows = leads.entry(lead.to_owned()).or_default();
                if rows.len() >= ROWS_EACH {
                    continue;
                }
                if row.is_none() {
                    let described = self.describe_work(&engine, each, &working, false).await;
                    row = serde_json::from_value(described).ok();
                }
                rows.extend(row.clone());
            }
        }
        leads.retain(|_, rows| !rows.is_empty());
        Some(leads)
    }

    /// The rows of leads elsewhere, told as they change (worked out again when this box's work or
    /// moves change, and each minute otherwise); the peers send each to the computer its lead
    /// lives on.
    async fn tell_rows(self, peers: WorkHandle) {
        let marks = |work: &Work| {
            let run = work.home.join("run");
            [
                run.join("work/changes.json"),
                run.join("moves/moved.json"),
                run.join("work/homes.json"),
            ]
            .iter()
            .map(|file| {
                std::fs::metadata(file)
                    .and_then(|meta| meta.modified())
                    .ok()
            })
            .collect::<Vec<_>>()
        };
        let (mut seen, mut looked) = (None, None::<Instant>);
        loop {
            let now = marks(&self);
            if seen.as_ref() != Some(&now) || looked.is_none_or(|at| at.elapsed() >= AGAIN) {
                // (After an engine that can't say now: as they were, until it can.)
                if let Some(rows) = self.rows_elsewhere().await {
                    if peers.publish(rows).await.is_err() {
                        return;
                    }
                    seen = Some(now);
                    looked = Some(Instant::now());
                }
            }
            tokio::time::sleep(LOOK).await;
        }
    }
}
