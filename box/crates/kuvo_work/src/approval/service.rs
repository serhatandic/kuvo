//! `kuvo/approval` and `kuvo/sessions/<id>/approval`: a client reads a conversation's approval
//! (what it is, what it comes to here now, and which it can pick) and sets it; and this computer's
//! Full Access switch. The box keeps each conversation's rules to what it comes to as the switch
//! and Settings › Permissions change (`keep`), adopts conversations a client made without an
//! approval, and a bot's conversation from before there were approvals starts with this
//! computer's default.
//!
//! Auto needs the sandbox, which this box doesn't run yet: it's kept, and asks first (on Windows it
//! isn't offered at all).

use std::{path::PathBuf, time::Duration};

use http::{Method, StatusCode};
use kuvo_engine::{BotEngine, Engine, LocalSessions, NoteEdits};
use kuvo_protocol::routes::{
    get_kuvo_approval as here_get, get_kuvo_sessions_id_approval as session_get,
    patch_kuvo_approval as here_patch, put_kuvo_sessions_id_approval as session_put,
};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use serde_json::{Map, Value, json};

use super::{Approval, Approvals, Kept, Rule, Unavailable};
use crate::bots::{here::BotHere, reach::segment};

const ALL: [Approval; 3] = [
    Approval::AskFirst,
    Approval::AcceptEdits,
    Approval::FullAccess,
];
/// The most pages of 200 conversations the round reads: half a million.
const MOST_PAGES: usize = 2500;
const WAIT: Duration = Duration::from_secs(10);
const LOOK: Duration = Duration::from_secs(2);

fn words(approval: Approval) -> (&'static str, &'static str) {
    match approval {
        Approval::AskFirst => ("Ask First", "Asks before it edits files or runs commands."),
        Approval::AcceptEdits => (
            "Accept Edits",
            "Edits files in the project on its own; asks before commands.",
        ),
        Approval::Auto => unreachable!("unavailable approval is not offered"),
        Approval::FullAccess => (
            "Full Access",
            "Runs everything outside the sandbox and never asks.",
        ),
    }
}

fn why(not: &Unavailable) -> Value {
    json!({"reason": not.reason, "why": not.why})
}

/// This computer's approvals and the conversations kept with them.
#[derive(Clone)]
pub struct ApprovalService {
    approvals: Approvals,
    engine: Engine,
    bots: PathBuf,
    sessions: LocalSessions,
}

/// A conversation as its approval reads it: its data, and whether it's a bot's or its work's.
struct Found {
    data: Value,
    bot: bool,
}

impl ApprovalService {
    pub fn new(
        approvals: Approvals,
        engine: Engine,
        bots: PathBuf,
        sessions: LocalSessions,
    ) -> Self {
        Self {
            approvals,
            engine,
            bots,
            sessions,
        }
    }

    fn found(&self, data: Value) -> Found {
        let id = data["id"].as_str().unwrap_or_default();
        let by = &data["metadata"]["kuvo"]["by"];
        let works = by["bot"].is_string() && by["name"].is_string();
        let bot = works
            || crate::wakes::folder::names(&self.bots)
                .iter()
                .any(|each| BotHere::read(&self.bots, each).is_some_and(|bot| bot.session == id));
        Found { data, bot }
    }

    fn of(&self, at: &Found) -> Kept {
        self.approvals.of(
            &at.data,
            at.bot,
            self.approvals.since(),
            self.approvals.default_approval(),
        )
    }

    fn options(&self) -> Vec<Value> {
        ALL.iter()
            .map(|each| {
                let (title, detail) = words(*each);
                let mut option = json!({"approval": each, "title": title, "detail": detail});
                if let Some(not) = self.approvals.unavailable(*each) {
                    option[if not.reason == "sandbox" {
                        "held"
                    } else {
                        "unavailable"
                    }] = why(&not);
                }
                option
            })
            .collect()
    }

    /// A conversation's approval as clients read it.
    fn shown(&self, at: &Found) -> Value {
        let kept = self.of(at);
        let shown_approval = if kept.approval == Approval::Auto {
            Approval::AskFirst
        } else {
            kept.approval
        };
        let mut shown = json!({
            "approval": shown_approval, "effective": kept.effective, "stored": kept.stored,
            "plan": at.data["agent"] == "plan", "options": self.options(),
        });
        if let Some(held) = &kept.held {
            shown["held"] = why(held);
        }
        shown
    }

    /// This computer's: whether Full Access may be picked, the default, and whether Auto runs.
    fn here(&self) -> Value {
        json!({
            "fullAccess": self.approvals.full_access_allowed(),
            "default": self.approvals.default_approval(),
            "auto": {"engine": {}, "agents": {}},
            "options": {"engine": self.options(), "agents": self.options()},
            "otherComputers": self.approvals.other_computers_allowed(),
        })
    }

    /// The rules and notes a conversation gets for `wanted`; none where they're already so.
    fn change_for(&self, at: &Found, wanted: Approval) -> Option<NoteEdits> {
        let current: Vec<Rule> = at.data["permissions"]
            .as_array()
            .map(|rules| rules.iter().filter_map(Rule::of).collect())
            .unwrap_or_default();
        let (approval, _) = self.approvals.effective(wanted);
        let held: Vec<Rule> = at.data["metadata"]["kuvo"]["held"]
            .as_array()
            .map(|rules| {
                rules
                    .iter()
                    .filter_map(Rule::of)
                    .filter(Rule::holds_back)
                    .collect()
            })
            .unwrap_or_default();
        let permissions = self.approvals.rules(approval, &current, &held);
        let kuvo = &at.data["metadata"]["kuvo"];
        if json!(permissions) == at.data["permissions"]
            && kuvo["approval"] == json!(wanted)
            && kuvo["applied"] == json!(approval)
        {
            return None;
        }
        Some(NoteEdits {
            kuvo: [
                ("approval".to_owned(), Some(json!(wanted))),
                ("applied".to_owned(), Some(json!(approval))),
            ]
            .into(),
            permissions: Some(
                permissions
                    .iter()
                    .map(crate::bots::tell::permission)
                    .collect(),
            ),
            ..NoteEdits::default()
        })
    }

    /// The conversation changed in its turn, as `wanted` says for it as it is then. Whether it's so.
    async fn write(
        &self,
        engine: &BotEngine,
        id: &str,
        wanted: impl Fn(&Self, &Found) -> Option<Approval>,
    ) -> bool {
        engine
            .rewrite(id, |data| {
                let now = self.found(data.clone());
                wanted(self, &now).and_then(|approval| self.change_for(&now, approval))
            })
            .await
            .is_ok()
    }

    /// What a conversation is to be kept with now, where its rules are made for something else
    /// than it comes to, or it's a bot's from before, or a new one waiting; none where it's so.
    fn to_settle(&self, at: &Found) -> Option<Approval> {
        let now = self.of(at);
        let settled = if now.stored {
            now.applied == Some(now.effective)
                && !self
                    .approvals
                    .made_before(&rules_of(&at.data), now.effective)
        } else {
            !(at.bot || now.pending)
        };
        (!settled).then_some(now.approval)
    }

    async fn settle(&self, engine: &BotEngine, at: &Found) -> bool {
        self.to_settle(at).is_none() || self.write(engine, id_of(&at.data), Self::to_settle).await
    }

    async fn remake(&self, engine: &BotEngine, at: &Found) -> bool {
        let kept = |service: &Self, now: &Found| {
            let is = service.of(now);
            is.stored.then_some(is.approval)
        };
        kept(self, at).is_none() || self.write(engine, id_of(&at.data), kept).await
    }

    /// A conversation's rules made for what its approval comes to now, where they aren't (a bot's
    /// from before there were approvals: this computer's default). Whether it's so.
    pub async fn settle_session(&self, engine: &BotEngine, id: &str) -> bool {
        match self.session(engine, id).await {
            Some(at) => self.settle(engine, &at).await,
            None => false,
        }
    }

    async fn session(&self, engine: &BotEngine, id: &str) -> Option<Found> {
        let data = engine
            .call(
                Method::GET,
                &format!("api/session/{}", segment(id)),
                None,
                WAIT,
            )
            .await
            .ok()?;
        data["id"].is_string().then(|| self.found(data))
    }

    /// Sets a conversation's approval: its answer, or a status and why not.
    async fn set(&self, id: &str, wanted: Approval) -> Result<Value, Refusal> {
        let engine = self.engine.bot_engine().await.ok_or_else(|| {
            Refusal::new(StatusCode::SERVICE_UNAVAILABLE, "The engine isn't running.")
        })?;
        let Some(at) = self.session(&engine, id).await else {
            return Err(Refusal::new(
                StatusCode::NOT_FOUND,
                "That conversation isn't here.",
            ));
        };
        if let Some(not) = self
            .approvals
            .unavailable(wanted)
            .filter(|not| not.reason != "sandbox")
        {
            return Err(Refusal::json(
                StatusCode::CONFLICT,
                json!({"message": not.why, "reason": not.reason}),
            ));
        }
        if !self.write(&engine, id, |_, _| Some(wanted)).await {
            return Err(Refusal::new(
                StatusCode::BAD_GATEWAY,
                "It couldn't be changed: the engine didn't take it. Try again.",
            ));
        }
        // (The next new conversation here starts with it, a bot's pick too: never a subagent's.)
        if !at.data["parentID"]
            .as_str()
            .is_some_and(|parent| !parent.is_empty())
        {
            self.approvals.remember(wanted);
        }
        let at = self.session(&engine, id).await.unwrap_or(at);
        self.settle(&engine, &at).await;
        let at = self.session(&engine, id).await.unwrap_or(at);
        Ok(self.shown(&at))
    }

    /// Each conversation kept with an approval that comes to something else now, its rules made
    /// for what it comes to; with `settings` (Settings › Permissions changed), every one kept with
    /// an approval whose rules aren't made from them as they are now, too. Whether every one is so.
    async fn reapply(&self, engine: &BotEngine, settings: bool) -> bool {
        let mut done = true;
        let mut query = Some("limit=200&order=desc".to_owned());
        let mut pages = 0;
        while let Some(asked) = query.take() {
            if pages >= MOST_PAGES {
                return false;
            }
            pages += 1;
            let Ok(page) = engine
                .call_whole(Method::GET, &format!("api/session?{asked}"), WAIT)
                .await
            else {
                return false;
            };
            let listed = page["data"].as_array().cloned().unwrap_or_default();
            let mut rewrites = Vec::new();
            for data in &listed {
                let Some(id) = data["id"].as_str() else {
                    continue;
                };
                let kept = &data["metadata"]["kuvo"]["approval"];
                let parent = data["parentID"]
                    .as_str()
                    .is_some_and(|parent| !parent.is_empty());
                if parent && kept.is_null() {
                    continue;
                }
                let at = self.found(data.clone());
                // (A bot's from before there were approvals is the bots' to adopt, in its turn.)
                if at.bot && kept.is_null() {
                    continue;
                }
                let now = self.of(&at);
                let remade = settings && now.stored && self.change_for(&at, now.approval).is_some();
                let special = kept == "auto" || kept == "fullAccess";
                if !remade && !special && !now.pending {
                    continue;
                }
                if !remade
                    && (if now.stored {
                        now.applied == Some(now.effective)
                            && !self.approvals.made_before(&rules_of(data), now.effective)
                    } else {
                        !(at.bot || now.pending)
                    })
                {
                    continue;
                }
                rewrites.push((id.to_owned(), remade));
            }
            for (id, remade) in rewrites {
                // (As it is now: what a client set since the list was read stays.)
                let Some(fresh) = self.session(engine, &id).await else {
                    done = false;
                    continue;
                };
                let ok = if remade {
                    self.remake(engine, &fresh).await
                } else {
                    self.settle(engine, &fresh).await
                };
                done &= ok;
            }
            if listed.len() == 200
                && let Some(next) = page["cursor"]["next"].as_str()
            {
                query = Some(format!("limit=200&cursor={}", segment(next)));
            }
        }
        done
    }

    /// The newest conversations a client made that still wait for their approval, each adopted.
    async fn adopt_pending(&self, engine: &BotEngine) {
        let Ok(page) = engine
            .call_whole(
                Method::GET,
                "api/session?limit=20&order=desc&parentID=null",
                WAIT,
            )
            .await
        else {
            return;
        };
        for data in page["data"].as_array().into_iter().flatten() {
            let Some(id) = data["id"].as_str() else {
                continue;
            };
            if !self.of(&self.found(data.clone())).pending {
                continue;
            }
            // (Read again just before it's written: one a client set meanwhile wins.)
            let Some(again) = self.session(engine, id).await else {
                continue;
            };
            let asked = &data["metadata"]["kuvo"]["approval"];
            if &again.data["metadata"]["kuvo"]["approval"] != asked
                || again.data["metadata"]["kuvo"].get("applied").is_some()
            {
                continue;
            }
            if let Some(asked) = Approval::of(asked) {
                self.approvals.remember(asked);
            }
            self.settle(engine, &again).await;
        }
    }

    /// Kept so as the box runs: when the engine starts, the Full Access switch or Settings ›
    /// Permissions change, every conversation kept with an approval made again until every one is;
    /// and as conversations come and go, the new ones a client made adopted. Only while the engine runs: this never starts it, nor keeps it up.
    pub fn keep(&self) -> tokio::task::JoinHandle<()> {
        let service = self.clone();
        tokio::spawn(async move {
            // (From now on, a new conversation a client doesn't say gets the default.)
            service.approvals.since();
            let mut states = service.engine.states();
            let mut sessions = service.sessions.clone();
            let mut last: Option<String> = None;
            let mut settings_made: Option<String> = None;
            loop {
                // (A round at most every two seconds, the first two seconds in, as TypeScript's
                // interval runs them: what a client does as the engine starts goes first.)
                tokio::time::sleep(LOOK).await;
                let started = match &*states.borrow_and_update() {
                    kuvo_engine::State::Running { started, .. } => Some(format!("{started:?}")),
                    _ => None,
                };
                if let Some(started) = started {
                    let now = format!("{} {started}", service.approvals.full_access_allowed());
                    let settings = json!(service.approvals.permissions_here()).to_string();
                    let changed =
                        last.as_ref() != Some(&now) || settings_made.as_ref() != Some(&settings);
                    if let Some(engine) = service.engine.running_bot_engine().await {
                        if changed
                            && service
                                .reapply(&engine, settings_made.as_ref() != Some(&settings))
                                .await
                        {
                            last = Some(now);
                            settings_made = Some(settings);
                        }
                        service.adopt_pending(&engine).await;
                    }
                }
                // (Looked at again as the engine or its list changes, and every two seconds for
                // the switch and Settings › Permissions, which needs no engine to read.)
                loop {
                    tokio::select! {
                        changed = states.changed() => if changed.is_err() { return } else { break },
                        _ = sessions.changed() => break,
                        () = tokio::time::sleep(LOOK) => {
                            let now = format!("{} ", service.approvals.full_access_allowed());
                            let settings = json!(service.approvals.permissions_here()).to_string();
                            if last.as_ref().is_some_and(|last| !last.starts_with(&now))
                                || settings_made.as_ref().is_some_and(|made| made != &settings)
                            {
                                break;
                            }
                        }
                    }
                }
            }
        })
    }

    pub fn register(&self, routes: &mut Registry) {
        let service = self.clone();
        routes.register::<here_get::Spec, _, _>(Access::Any, move |_| {
            let service = service.clone();
            async move { Ok(RouteReply::json::<200>(typed(service.here())?)) }
        });
        let service = self.clone();
        routes.register::<here_patch::Spec, _, _>(Access::Any, move |call| {
            let service = service.clone();
            async move {
                let given = body(&call.request.body);
                let other = given.get("otherComputers").and_then(Value::as_bool);
                // Whether your other computers may start work here: only that, when it's all that's said.
                if let (Some(other), None) = (other, given.get("fullAccess")) {
                    service.approvals.allow_other_computers(other);
                    return Ok(RouteReply::json::<200>(typed(service.here())?));
                }
                let Some(full) = given.get("fullAccess").and_then(Value::as_bool) else {
                    return Err(Refusal::new(
                        StatusCode::BAD_REQUEST,
                        "Say whether you confirmed Full Access on this computer (fullAccess, true or false), or whether your other computers may start work here (otherComputers, true or false).",
                    ));
                };
                if let Some(other) = other {
                    service.approvals.allow_other_computers(other);
                }
                service.approvals.allow_full_access(full);
                // What had it, or was held from it, comes to what it can be now at once.
                if let Some(engine) = service.engine.running_bot_engine().await {
                    service.reapply(&engine, false).await;
                }
                Ok(RouteReply::json::<200>(typed(service.here())?))
            }
        });
        let service = self.clone();
        routes.register::<session_get::Spec, _, _>(Access::Any, move |call| {
            let service = service.clone();
            async move {
                let id = id_from(call.param("id"))?;
                let engine = service.engine.bot_engine().await.ok_or_else(|| {
                    Refusal::new(StatusCode::SERVICE_UNAVAILABLE, "The engine isn't running.")
                })?;
                let at = service.session(&engine, &id).await.ok_or_else(|| {
                    Refusal::new(StatusCode::NOT_FOUND, "That conversation isn't here.")
                })?;
                Ok(RouteReply::json::<200>(typed(service.shown(&at))?))
            }
        });
        let service = self.clone();
        routes.register::<session_put::Spec, _, _>(Access::Any, move |call| {
            let service = service.clone();
            async move {
                let id = id_from(call.param("id"))?;
                let given = body(&call.request.body);
                let Some(wanted) = given.get("approval").and_then(Approval::of) else {
                    return Err(Refusal::new(
                        StatusCode::BAD_REQUEST,
                        "approval is one of askFirst, acceptEdits, auto, fullAccess.",
                    ));
                };
                Ok(RouteReply::json::<200>(typed(
                    service.set(&id, wanted).await?,
                )?))
            }
        });
    }
}

fn id_of(data: &Value) -> &str {
    data["id"].as_str().unwrap_or_default()
}

fn rules_of(data: &Value) -> Vec<Rule> {
    data["permissions"]
        .as_array()
        .map(|rules| rules.iter().filter_map(Rule::of).collect())
        .unwrap_or_default()
}

fn body(bytes: &[u8]) -> Map<String, Value> {
    serde_json::from_slice::<Value>(bytes)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

fn id_from(param: Option<&str>) -> Result<String, Refusal> {
    param
        .and_then(|id| percent_encoding::percent_decode_str(id).decode_utf8().ok())
        .map(|id| id.into_owned())
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Refusal::new(StatusCode::NOT_FOUND, "That conversation isn't here."))
}

fn typed<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, Refusal> {
    serde_json::from_value(value)
        .map_err(|error| Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
}
