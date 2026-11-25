//! Authenticated calls for work that lives on another computer.

use std::{
    collections::BTreeMap,
    future::Future,
    io,
    path::{Path, PathBuf},
    pin::Pin,
};

use kuvo_wire::{
    keys::{decode_exact, encode},
    mail::{Envelope, MAX_PLAINTEXT},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{
    directory::ComputerKey,
    kept,
    outbox::{Delivered, Id, IncomingLetter},
    reach::Millis,
};

const DAY: Millis = 86_400_000;
const RETRY: Millis = 60_000;
const ANSWER_LIFE: Millis = 2 * DAY;
const MAX_RULES: usize = 300;
pub const FEATURE: &str = "work.elsewhere";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tool {
    ListSessions,
    StopSession,
    CompactSession,
    AnswerAsk,
    StartSession,
    ListModels,
    ListProjects,
    StopWork,
}

impl Tool {
    fn name(self) -> &'static str {
        match self {
            Self::ListSessions => "list_sessions",
            Self::StopSession => "stop_session",
            Self::CompactSession => "compact_session",
            Self::AnswerAsk => "answer_ask",
            Self::StartSession => "start_session",
            Self::ListModels => "list_models",
            Self::ListProjects => "list_projects",
            Self::StopWork => "stop_work",
        }
    }

    pub(crate) fn feature(self) -> &'static str {
        match self {
            Self::StartSession | Self::ListModels | Self::ListProjects | Self::StopWork => {
                "work.elsewhere.start"
            }
            _ => FEATURE,
        }
    }

    pub(crate) fn waits(self) -> bool {
        matches!(
            self,
            Self::ListSessions | Self::StartSession | Self::ListModels | Self::ListProjects
        )
    }

    fn is_list(self) -> bool {
        matches!(
            self,
            Self::ListSessions | Self::ListModels | Self::ListProjects
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rule {
    pub action: String,
    pub resource: String,
    pub effect: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Lead {
    pub name: String,
    pub free: bool,
    pub rules: Vec<Rule>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot: Option<Bot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Bot {
    pub bot: String,
    pub name: String,
}

impl Lead {
    fn outgoing(&mut self) {
        if self.rules.len() > MAX_RULES {
            self.rules = vec![Rule {
                action: "*".into(),
                resource: "*".into(),
                effect: "ask".into(),
            }];
        }
    }

    fn received(&mut self) -> bool {
        if self.rules.len() > MAX_RULES || self.rules.iter().any(|rule| rule.effect.is_empty()) {
            return false;
        }
        for rule in &mut self.rules {
            rule.action = prefix(&rule.action, 200);
            rule.resource = prefix(&rule.resource, 2000);
        }
        self.name = plain(&self.name, 100);
        if self.name.is_empty() {
            self.name = "another agent".into();
        }
        self.bot = self
            .bot
            .take()
            .filter(|bot| identifier(&bot.bot, "bot_", 1, 60))
            .map(|bot| Bot {
                name: plain(&bot.name, 100),
                ..bot
            });
        true
    }
}

pub fn plain(value: &str, limit: usize) -> String {
    let stripped = value.replace(
        |c: char| {
            matches!(c,
        '\0'..='\u{1f}' | '\u{7f}' | '\u{200e}'..='\u{200f}'
        | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        },
        " ",
    );
    prefix(
        &stripped.split_whitespace().collect::<Vec<_>>().join(" "),
        limit,
    )
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub tool: Tool,
    pub args: Map<String, Value>,
    #[serde(rename = "as")]
    pub lead: Lead,
}

impl Request {
    fn allows(&self) -> bool {
        self.tool == Tool::AnswerAsk
            && self.args.get("decision").and_then(Value::as_str) == Some("allow")
    }

    fn lifetime(&self) -> Millis {
        match self.tool {
            Tool::ListSessions => 900_000,
            Tool::StartSession | Tool::ListModels | Tool::ListProjects => 120_000,
            _ if self.allows() => DAY,
            _ => 30 * DAY,
        }
    }

    pub fn session(&self) -> Option<&str> {
        self.args
            .get("session")
            .and_then(Value::as_str)
            .map(str::trim)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MailedCall {
    pub id: Id,
    pub from: String,
    pub made: Millis,
    pub at: Millis,
    #[serde(flatten)]
    pub request: Request,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pending {
    call: Request,
    from: String,
    to: String,
    peer: String,
    at: Millis,
    #[serde(default)]
    wait: bool,
    #[serde(default)]
    quiet: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    sent_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    abandoned: Option<Millis>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Answer {
    pub text: String,
    pub refused: bool,
}

impl Answer {
    pub fn refused(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            refused: true,
        }
    }

    /// Keep a machine-readable rows answer within the mail budget without cutting JSON.
    pub fn rows(mut rows: Vec<Value>) -> Self {
        loop {
            let text = json!({"rows":rows}).to_string();
            if text.len() <= 22_000 {
                return Self {
                    text,
                    refused: false,
                };
            }
            if rows.len() > 1 {
                rows.truncate((rows.len() * 7 / 10).max(1));
            } else if let Some(row) = rows[0].as_object_mut() {
                for field in ["lastReply", "replyBeforeStop"] {
                    row.remove(field);
                }
                for (field, max) in [("title", 100), ("asks", 200)] {
                    if let Some(value) = row.get(field).and_then(Value::as_str) {
                        row.insert(field.into(), Value::String(prefix(value, max)));
                    }
                }
                if json!({"rows":rows}).to_string().len() > 22_000 {
                    rows.clear();
                }
            } else {
                rows.clear();
            }
        }
    }
}

#[derive(Clone, Debug)]
pub enum Admission {
    Ready,
    Reject(String),
    Permission(Permission),
    Refused(Answer),
    Retry(String),
}

/// Computed by the local session owner from the current ask and the complete tool input.
/// Include derived command parts and both spellings of any outside-project paths.
#[derive(Clone, Debug)]
pub struct Permission {
    pub title: String,
    pub ask_first: bool,
    pub sandboxed: bool,
    pub checks: Vec<Resource>,
    pub local_rules: Vec<Rule>,
}

#[derive(Clone, Debug)]
pub struct Resource {
    pub action: String,
    pub value: String,
}

impl Permission {
    pub fn refusal(&self, lead: &Lead, made: Millis, now: Millis) -> Option<Answer> {
        if self.ask_first {
            return Some(Answer::refused(format!(
                "You can't allow that: \"{}\" is in Ask First, so the user answers every ask it makes, whatever mode you're in. The user sees the ask on \"{}\" and answers it there. Tell them in a line why it's needed if it matters, and carry on with the rest. You can still deny it.",
                self.title, self.title
            )));
        }
        if now.saturating_sub(made) > DAY {
            return Some(Answer::refused(format!(
                "Your allow for \"{}\" wasn't given: this computer was away for over a day after you gave it, so it's out of date. Look at what it asks now (list_sessions), and answer again if it still should go ahead.",
                self.title
            )));
        }
        if self.checks.is_empty()
            || self.checks.iter().any(|check| {
                let shell = matches!(check.action.as_str(), "shell" | "bash");
                let file = matches!(
                    check.action.as_str(),
                    "external_directory"
                        | "edit"
                        | "write"
                        | "patch"
                        | "multiedit"
                        | "read"
                        | "list"
                        | "glob"
                        | "grep"
                );
                (shell && !lead.free && !self.sandboxed)
                    || (file && check.value.contains('\\'))
                    || !allows(&lead.rules, check)
                    || !allows(&self.local_rules, check)
            })
        {
            return Some(Answer::refused(format!(
                "You can't allow that: you'd have to ask the user about it yourself. The user sees the ask on \"{}\" and answers it there. Tell them in a line why it's needed, and carry on with the rest.",
                self.title
            )));
        }
        None
    }
}

fn allows(rules: &[Rule], check: &Resource) -> bool {
    if let Some(rule) = rules.iter().rev().find(|rule| {
        (rule.action == "*" || rule.action == check.action) && matches(&rule.resource, &check.value)
    }) {
        return rule.effect == "allow";
    }
    match check.action.as_str() {
        "external_directory" | "doom_loop" => false,
        "read" => {
            check.value.ends_with(".env.example")
                || !(matches("*.env", &check.value) || matches("*.env.*", &check.value))
        }
        _ => true,
    }
}

fn matches(pattern: &str, text: &str) -> bool {
    let text: Vec<_> = text.encode_utf16().collect();
    let mut possible = vec![false; text.len() + 1];
    possible[0] = true;
    for token in pattern.encode_utf16() {
        if token == u16::from(b'*') {
            for i in 1..possible.len() {
                possible[i] |= possible[i - 1];
            }
        } else {
            for (i, c) in text.iter().copied().enumerate().rev() {
                // The TypeScript matcher uses a plain dot for '?'.
                let hit = if token == u16::from(b'?') {
                    !matches!(c, 0x000a | 0x000d | 0x2028 | 0x2029)
                } else {
                    token == c
                };
                possible[i + 1] = hit && possible[i];
            }
            possible[0] = false;
        }
    }
    possible.last().copied().unwrap_or_default()
}

pub(crate) fn prefix(value: &str, units: usize) -> String {
    let mut consumed = 0;
    value
        .chars()
        .take_while(|c| {
            consumed += c.len_utf16();
            consumed <= units
        })
        .collect()
}

fn identifier(value: &str, prefix: &str, min: usize, max: usize) -> bool {
    value.strip_prefix(prefix).is_some_and(|suffix| {
        (min..=max).contains(&suffix.len()) && suffix.bytes().all(|c| c.is_ascii_alphanumeric())
    })
}

pub(crate) fn session(id: &str) -> bool {
    identifier(id, "ses_", 1, 60)
}

fn call_id(id: &str) -> bool {
    id.strip_prefix("cal_").is_some_and(|s| {
        (16..=40).contains(&s.len())
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Home {
    Unknown,
    Local,
    Frozen,
    Peer(ComputerKey),
}

/// Engine/session boundary (3.2). Inspection checks the session's ancestry, resolves its current
/// ask, and returns the derived policy checks. The open-work switch also belongs to inspection.
/// Effects recheck session authority, use the call ID as a durable idempotency key, and return
/// the same answer on replay. Queue delivery uses the stable answer ID and follows the lead's
/// current home when it has moved (2.4).
pub trait RemoteWork: Send + Sync + 'static {
    fn inspect<'a>(
        &'a self,
        sender: ComputerKey,
        call: &'a MailedCall,
    ) -> WorkFuture<'a, Admission>;
    fn apply<'a>(&'a self, sender: ComputerKey, call: &'a MailedCall) -> WorkFuture<'a, Answer>;
    fn deliver<'a>(
        &'a self,
        id: &'a Id,
        session: &'a str,
        answer: &'a Answer,
    ) -> WorkFuture<'a, ()>;
}

pub type WorkFuture<'a, T> = Pin<Box<dyn Future<Output = io::Result<T>> + Send + 'a>>;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Completed {
    #[serde(default)]
    at: Millis,
    #[serde(default)]
    acknowledged: bool,
    sender: ComputerKey,
    lead: String,
    tool: Tool,
    session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    answer: Option<Answer>,
}

pub struct Calls {
    run: PathBuf,
    tries: BTreeMap<String, Millis>,
    answers_sent: BTreeMap<String, Millis>,
    heard: BTreeMap<ComputerKey, Millis>,
    last_prune: Option<Millis>,
}

impl Calls {
    /// The resident owns one writer for call records. Letter delivery skips call records.
    pub fn open(run: &Path) -> Self {
        Self {
            run: run.into(),
            tries: BTreeMap::new(),
            answers_sent: BTreeMap::new(),
            heard: BTreeMap::new(),
            last_prune: None,
        }
    }

    fn outgoing(&self, id: &Id) -> PathBuf {
        self.run
            .join("outbox/sending")
            .join(format!("{}.json", id.as_str()))
    }

    fn result(&self, id: &Id) -> PathBuf {
        self.run
            .join("work/call-results")
            .join(format!("{}.json", id.as_str()))
    }

    pub fn enqueue(
        &mut self,
        peer: ComputerKey,
        from: String,
        mut call: Request,
        wait: bool,
        quiet: bool,
        now: Millis,
    ) -> io::Result<Id> {
        if !session(&from) || (!wait && call.tool.waits()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid remote call",
            ));
        }
        call.lead.outgoing();
        let mut random = [0; 16];
        getrandom::fill(&mut random).map_err(|err| io::Error::other(err.to_string()))?;
        let id = Id::try_from(format!("cal_{}", encode(&random))).map_err(io::Error::other)?;
        let queued = Pending {
            call,
            from,
            to: String::new(),
            peer: encode(&peer),
            at: now,
            wait,
            quiet,
            sent_to: None,
            abandoned: None,
        };
        kept::write_durable(&self.outgoing(&id), &queued)?;
        Ok(id)
    }

    pub fn sent(&self, id: &Id) -> bool {
        kept::read::<Pending>(&self.outgoing(id)).is_some_and(|call| call.sent_to.is_some())
    }

    pub fn heard(&mut self, key: ComputerKey, now: Millis) {
        self.heard.insert(key, now);
    }

    fn pending(&self) -> io::Result<Vec<(Id, Pending)>> {
        let dir = self.run.join("outbox/sending");
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
            Err(error) => return Err(error),
        };
        let mut list = vec![];
        for entry in entries {
            let file = entry?.path();
            let Some(id) = file
                .file_stem()
                .and_then(|name| name.to_str())
                .filter(|id| call_id(id))
            else {
                continue;
            };
            if file.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            if let Some(call) = kept::read::<Pending>(&file) {
                list.push((Id::try_from(id.to_owned()).map_err(io::Error::other)?, call));
            }
        }
        list.sort_by(|(id, a), (other, b)| a.at.cmp(&b.at).then(id.as_str().cmp(other.as_str())));
        Ok(list)
    }

    pub fn cancel_wait(&mut self, id: &Id, keep: bool, now: Millis) -> io::Result<()> {
        let path = self.outgoing(id);
        if let Some(mut call) = kept::read::<Pending>(&path) {
            if !keep && call.sent_to.is_none() {
                return self.remove(id);
            }
            call.wait = false;
            call.abandoned = (!keep).then_some(now);
            kept::write_durable(&path, &call)?;
        }
        Ok(())
    }

    fn remove(&mut self, id: &Id) -> io::Result<()> {
        match std::fs::remove_file(self.outgoing(id)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        self.tries.remove(id.as_str());
        Ok(())
    }

    /// Start with the oldest call for an away computer, even where it is still waiting to retry.
    /// A mail is returned only after its destination has been durably recorded.
    pub fn round(
        &mut self,
        now: Millis,
        online: bool,
        peer_state: impl Fn(ComputerKey) -> PeerState,
        home: impl Fn(&str) -> Home,
    ) -> io::Result<Vec<Outgoing>> {
        let mut away = std::collections::BTreeSet::new();
        let mut work = vec![];
        for (id, mut call) in self.pending()? {
            let Some(key) = decode_exact::<32>(&call.peer) else {
                continue;
            };
            let state = peer_state(key);
            if let Some(abandoned) = call.abandoned {
                let life = if call.call.tool.is_list() {
                    ANSWER_LIFE
                } else {
                    30 * DAY
                };
                if now.saturating_sub(abandoned) > life || state == PeerState::Removed {
                    self.remove(&id)?;
                }
                continue;
            }
            let moved = matches!(home(&call.from), Home::Peer(_));
            let reason = if now.saturating_sub(call.at) > call.call.lifetime() {
                Some(FailureReason::Expired {
                    allow: call.call.allows(),
                })
            } else if moved {
                Some(FailureReason::Moved)
            } else if state == PeerState::Removed {
                self.remove(&id)?;
                continue;
            } else {
                None
            };
            if let Some(reason) = reason {
                if call.quiet && !call.wait {
                    self.remove(&id)?;
                } else {
                    work.push(Outgoing::Failed(Failure::new(id, key, call, reason)));
                }
                continue;
            }
            if state == PeerState::Unknown
                || (state == PeerState::Away && !away.insert(key))
                || !online
                || self.tries.get(id.as_str()).is_some_and(|at| {
                    now.saturating_sub(*at) < RETRY
                        && self.heard.get(&key).is_none_or(|heard| *heard <= *at)
                })
            {
                continue;
            }
            let envelope = packet(json!({"at":now, "call":MailedCall {
                id: id.clone(), from: call.from.clone(), made: call.at, at: now, request:call.call.clone(),
            }}));
            if envelope.encode().len() > MAX_PLAINTEXT {
                if call.quiet && !call.wait {
                    self.remove(&id)?;
                } else {
                    work.push(Outgoing::Failed(Failure::new(
                        id,
                        key,
                        call,
                        FailureReason::TooLong,
                    )));
                }
                continue;
            }
            if call.sent_to.as_deref() != Some(&call.peer) {
                let path = self.outgoing(&id);
                let Some(mut current) = kept::read::<Pending>(&path) else {
                    continue;
                };
                current.sent_to = Some(std::mem::take(&mut call.peer));
                kept::write_durable(&path, &current)?;
            }
            self.tries.insert(id.as_str().into(), now);
            work.push(Outgoing::Mail { key, envelope });
        }
        Ok(work)
    }

    pub fn finish(&mut self, id: &Id) -> io::Result<()> {
        self.remove(id)
    }

    pub fn receipt(
        &mut self,
        key: ComputerKey,
        receipt: &Delivered,
    ) -> io::Result<Option<Outgoing>> {
        let Some(call) = kept::read::<Pending>(&self.outgoing(&receipt.id)) else {
            return Ok(None);
        };
        if call.sent_to.as_deref() != Some(&encode(&key)) || receipt.retry == Some(true) {
            return Ok(None);
        }
        if receipt.ok && call.call.tool != Tool::StopWork {
            return Ok(None);
        }
        if receipt.ok
            || (!call.wait && (call.quiet || call.call.tool.waits() || call.abandoned.is_some()))
        {
            self.remove(&receipt.id)?;
            return Ok(None);
        }
        let why = receipt
            .why
            .as_deref()
            .filter(|why| !why.is_empty())
            .map_or_else(
                || "It couldn't be done there.".into(),
                |why| prefix(why, 500),
            );
        Ok(Some(Outgoing::Failed(Failure::new(
            receipt.id.clone(),
            key,
            call,
            FailureReason::Refused(why),
        ))))
    }

    pub async fn receive(
        &mut self,
        key: ComputerKey,
        given: Value,
        now: Millis,
        here: &str,
        home: impl Fn(&str) -> Home,
        work: &dyn RemoteWork,
    ) -> io::Result<Vec<Envelope>> {
        let Some(id) = given
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| call_id(id))
            .and_then(|id| Id::try_from(id.to_owned()).ok())
        else {
            return Ok(vec![]);
        };
        let no = |why: &str, retry| vec![receipt(&id, Some((why, retry)), now)];
        if given
            .get("at")
            .and_then(Value::as_i64)
            .is_none_or(|at| now.abs_diff(at) > DAY as u64)
        {
            return Ok(no("It's too old to deliver.", true));
        }
        if given
            .get("tool")
            .and_then(|tool| serde_json::from_value::<Tool>(tool.clone()).ok())
            .is_none()
        {
            return Ok(no(
                "That isn't something another computer can do here.",
                false,
            ));
        }
        let Ok(mut call) = serde_json::from_value::<MailedCall>(given) else {
            return Ok(no("That isn't a call.", false));
        };
        if !session(&call.from) || !call.request.lead.received() {
            return Ok(no("That isn't a call.", false));
        }
        if let Some(prior) = kept::read::<Completed>(&self.result(&id)) {
            if prior.sender != key || prior.lead != call.from {
                return Ok(no(
                    "This call id already belongs to another conversation.",
                    false,
                ));
            }
            return Ok(reply(&id, &prior, here, now));
        }
        match home(&call.from) {
            Home::Local => {
                return Ok(no(
                    "That conversation is on this computer: another can't speak for it.",
                    false,
                ));
            }
            Home::Frozen => {
                return Ok(no(
                    "That conversation is moving between computers: it comes again once it's there.",
                    true,
                ));
            }
            Home::Peer(peer) if peer != key => {
                return Ok(no(
                    "That conversation lives on another computer, as this one knows it: it comes again.",
                    true,
                ));
            }
            _ => {}
        }
        if call.request.tool != Tool::ListSessions && now.saturating_sub(call.made) > 31 * DAY {
            return Ok(no("It's too old to deliver.", false));
        }
        if call.request.tool == Tool::StartSession && now.abs_diff(call.made) > DAY as u64 {
            return Ok(no("It's too old to start now.", false));
        }
        if matches!(
            call.request.tool,
            Tool::StopSession | Tool::CompactSession | Tool::AnswerAsk
        ) {
            let Some(target) = call.request.session().filter(|target| session(target)) else {
                return Ok(no("Say which session.", false));
            };
            if home(target) == Home::Frozen {
                return Ok(no(
                    "It's moving between computers: it comes again once it's there.",
                    true,
                ));
            }
        }
        let admission = work.inspect(key, &call).await;
        if let Ok(Admission::Reject(why)) = &admission {
            return Ok(no(&prefix(why, 500), false));
        }
        let answer = match self.apply(key, &call, now, work, admission).await {
            Ok(answer) => answer,
            Err(reason) => return Ok(no(&prefix(&reason, 500), true)),
        };
        if home(&call.from) == Home::Unknown {
            note_homes(&self.run, key, now, std::iter::once(call.from.as_str()))?;
        }
        let done = Completed {
            at: now,
            acknowledged: false,
            sender: key,
            lead: call.from,
            tool: call.request.tool,
            session: call.request.session().map(str::to_owned),
            answer: (call.request.tool != Tool::StopWork).then_some(answer),
        };
        kept::write_durable(&self.result(&id), &done)?;
        if done.tool != Tool::StopWork {
            self.answers_sent.insert(id.as_str().into(), now);
        }
        Ok(reply(&id, &done, here, now))
    }

    async fn apply(
        &self,
        key: ComputerKey,
        call: &MailedCall,
        now: Millis,
        work: &dyn RemoteWork,
        admission: io::Result<Admission>,
    ) -> Result<Answer, String> {
        let temporary = |error| format!("It couldn't be done now: {error}");
        match admission.map_err(temporary)? {
            Admission::Retry(why) | Admission::Reject(why) => return Err(why),
            Admission::Refused(answer) => return Ok(answer),
            Admission::Permission(check) if call.request.allows() => {
                if let Some(answer) = check.refusal(&call.request.lead, call.made, now) {
                    return Ok(answer);
                }
            }
            Admission::Ready if call.request.allows() => {
                return Ok(Answer::refused(
                    "You can't allow that: the current ask couldn't be checked.",
                ));
            }
            _ => {}
        }
        work.apply(key, call).await.map_err(temporary)
    }

    pub(crate) fn answer_receipt(
        &mut self,
        key: ComputerKey,
        receipt: &Delivered,
    ) -> io::Result<()> {
        if !receipt.ok && receipt.retry == Some(true) {
            return Ok(());
        }
        let Some(id) = receipt
            .id
            .as_str()
            .strip_suffix("-a")
            .filter(|id| call_id(id))
        else {
            return Ok(());
        };
        let path = self.result(&Id::try_from(id.to_owned()).map_err(io::Error::other)?);
        if let Some(mut done) = kept::read::<Completed>(&path)
            && done.sender == key
            && !done.acknowledged
        {
            done.acknowledged = true;
            done.answer = None;
            kept::write_durable(&path, &done)?;
            self.answers_sent.remove(id);
        }
        Ok(())
    }

    pub(crate) fn retry_answers(
        &mut self,
        now: Millis,
        online: bool,
        here: &str,
    ) -> io::Result<Vec<(ComputerKey, Envelope)>> {
        if self
            .last_prune
            .is_none_or(|last| now.saturating_sub(last) >= 60 * RETRY)
        {
            for (id, path, done) in self.records::<Completed>("work/call-results")? {
                if now.saturating_sub(done.at) > 31 * DAY {
                    self.answers_sent.remove(id.as_str());
                    remove(&path)?;
                } else if !done.acknowledged && done.tool != Tool::StopWork {
                    self.answers_sent
                        .entry(id.into())
                        .or_insert(now.saturating_sub(RETRY));
                }
            }
            for (_, path, answer) in self.records::<AnswerFrom>("work/call-answers")? {
                if now.saturating_sub(answer.at) > 31 * DAY {
                    remove(&path)?;
                }
            }
            self.last_prune = Some(now);
        }
        let mut messages = vec![];
        if !online {
            return Ok(messages);
        }
        let due: Vec<_> = self
            .answers_sent
            .iter()
            .filter(|(_, at)| now.saturating_sub(**at) >= RETRY)
            .map(|(id, _)| Id::try_from(id.clone()).expect("validated call id"))
            .collect();
        for id in due {
            let Some(done) = kept::read::<Completed>(&self.result(&id)).filter(|done| {
                !done.acknowledged
                    && now.saturating_sub(done.at)
                        <= if done.tool.is_list() {
                            ANSWER_LIFE
                        } else {
                            30 * DAY
                        }
            }) else {
                self.answers_sent.remove(id.as_str());
                continue;
            };
            if let Some(letter) = answer_letter(&id, &done, here, now) {
                messages.push((done.sender, letter));
                self.answers_sent.insert(id.into(), now);
            } else {
                self.answers_sent.remove(id.as_str());
            }
        }
        Ok(messages)
    }

    fn records<T: serde::de::DeserializeOwned>(
        &self,
        directory: &str,
    ) -> io::Result<Vec<(Id, PathBuf, T)>> {
        let listing = match std::fs::read_dir(self.run.join(directory)) {
            Ok(listing) => listing,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
            Err(error) => return Err(error),
        };
        let mut records = vec![];
        for entry in listing {
            let path = entry?.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let Some(id) = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .filter(|id| call_id(id))
                .and_then(|id| Id::try_from(id.to_owned()).ok())
            else {
                continue;
            };
            if let Some(value) = kept::read::<T>(&path) {
                records.push((id, path, value));
            }
        }
        Ok(records)
    }

    fn received_answer(&self, id: &Id) -> PathBuf {
        self.run
            .join("work/call-answers")
            .join(format!("{}.json", id.as_str()))
    }

    pub(crate) fn answer(
        &self,
        key: ComputerKey,
        mail: IncomingLetter,
        now: Millis,
    ) -> Option<ReceivedAnswer> {
        let call = mail.letter.answers.as_ref()?;
        if !call_id(call.as_str()) || mail.id.as_str() != format!("{}-a", call.as_str()) {
            return None;
        }
        let ack = |message| ReceivedAnswer::Ack(receipt(&mail.id, message, now));
        if let Some(prior) = kept::read::<AnswerFrom>(&self.received_answer(call)) {
            return (prior.key == key && prior.lead == mail.letter.to).then(|| ack(None));
        }
        let queued = kept::read::<Pending>(&self.outgoing(call))?;
        if queued.sent_to.as_deref() != Some(&encode(&key))
            || queued.from != mail.letter.to
            || queued.call.tool == Tool::StopWork
            || mail.letter.read.is_some() != queued.call.tool.is_list()
        {
            return None;
        }
        if mail.refusal(now.max(0) as u64).is_some() {
            return Some(ack(Some(("That isn't a current call answer.", true))));
        }
        Some(ReceivedAnswer::New {
            call: call.clone(),
            letter: mail.id,
            lead: mail.letter.to,
            words: mail
                .letter
                .note
                .as_ref()
                .and_then(|note| note.get("words"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            answer: Answer {
                text: mail.letter.text,
                refused: mail.letter.refused,
            },
            note: !queued.call.tool.is_list(),
        })
    }

    pub(crate) fn accept_answer(
        &mut self,
        key: ComputerKey,
        call: &Id,
        lead: String,
        now: Millis,
    ) -> io::Result<()> {
        kept::write_durable(
            &self.received_answer(call),
            &AnswerFrom { key, lead, at: now },
        )?;
        self.remove(call)
    }
}

#[derive(Serialize, Deserialize)]
struct AnswerFrom {
    key: ComputerKey,
    lead: String,
    at: Millis,
}

pub(crate) enum ReceivedAnswer {
    Ack(Envelope),
    New {
        call: Id,
        letter: Id,
        lead: String,
        answer: Answer,
        note: bool,
        words: Option<String>,
    },
}

pub enum Outgoing {
    Mail {
        key: ComputerKey,
        envelope: Envelope,
    },
    Failed(Failure),
}

pub struct Failure {
    pub id: Id,
    pub key: ComputerKey,
    pub lead: String,
    tool: Tool,
    target: String,
    reason: FailureReason,
    notify: bool,
}

enum FailureReason {
    Expired { allow: bool },
    Moved,
    Refused(String),
    TooLong,
}

impl Failure {
    fn new(id: Id, key: ComputerKey, call: Pending, reason: FailureReason) -> Self {
        Self {
            id,
            key,
            lead: call.from,
            tool: call.call.tool,
            target: call.call.session().unwrap_or_default().into(),
            reason,
            notify: !call.quiet && !call.call.tool.waits(),
        }
    }

    pub(crate) fn notifies(&self) -> bool {
        self.notify
    }

    pub fn why(&self) -> &str {
        match &self.reason {
            FailureReason::Expired { .. } => "that computer wasn't back in time.",
            FailureReason::Moved => {
                "you moved to another computer before it could go. Ask again from there."
            }
            FailureReason::Refused(why) => why,
            FailureReason::TooLong => "It's too long to send.",
        }
    }

    pub fn queued_words(&self, computer: &str) -> String {
        let tool = self.tool.name();
        let session = &self.target;
        let why = self.why();
        match self.reason {
            FailureReason::Expired { allow } => {
                let further = if allow {
                    " An allow waits a day at most, as what it was for may have changed. Look at what it asks with list_sessions once it's back."
                } else {
                    ""
                };
                format!(
                    "[Not done] Your {tool} for {session} on {computer} wasn't done: {why}{further}"
                )
            }
            FailureReason::Moved => {
                format!("[Not done] Your {tool} for {session} wasn't done: {why}")
            }
            FailureReason::Refused(_) | FailureReason::TooLong => {
                format!("[Not done] Your {tool} for {session} on {computer}: {why}")
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeerState {
    Online,
    Away,
    Unknown,
    Removed,
}

fn packet(value: Value) -> Envelope {
    Envelope(value.as_object().expect("call mail object").clone())
}

pub(crate) fn receipt(id: &Id, refused: Option<(&str, bool)>, now: Millis) -> Envelope {
    let (why, retry) = refused.unzip();
    Delivered {
        id: id.clone(),
        ok: why.is_none(),
        why: why.map(str::to_owned),
        retry: retry.filter(|retry| *retry),
        answer: None,
        lives: None,
    }
    .envelope(now.max(0) as u64)
}

fn reply(id: &Id, done: &Completed, here: &str, now: Millis) -> Vec<Envelope> {
    let mut messages = vec![];
    if let Some(letter) = answer_letter(id, done, here, now) {
        messages.push(letter);
    }
    messages.push(receipt(id, None, now));
    messages
}

fn answer_letter(id: &Id, done: &Completed, here: &str, now: Millis) -> Option<Envelope> {
    let answer = done.answer.as_ref()?;
    let here = plain(here, 100);
    let mut letter = json!({
        "id":format!("{}-a", id.as_str()), "to":done.lead, "answers":id,
        "at":now, "text":answer.text, "refused":answer.refused,
    });
    if done.tool.is_list() {
        letter["read"] = json!({});
        if packet(json!({"letter":letter, "at":now})).encode().len() > MAX_PLAINTEXT {
            letter["text"] = json!(r#"{"rows":[]}"#);
        }
    } else {
        let words = prefix(&answer.text, 4000);
        letter["text"] = json!(format!("[On {here}] {words}"));
        letter["note"] = json!({
            "from":here, "words":words,
            "line":prefix(&answer.text, 300), "delivery":"queue",
        });
        if let Some(session) = &done.session {
            letter["note"]["about"] = json!([session]);
        }
        letter["made"] = done.at.into();
    }
    Some(packet(json!({"letter":letter, "at":now})))
}

#[derive(Serialize, Deserialize)]
struct KeptHome {
    key: String,
    at: Millis,
}

pub fn known_home(run: &Path, session: &str) -> Option<ComputerKey> {
    let homes = kept::read::<BTreeMap<String, KeptHome>>(&run.join("work/homes.json"))?;
    decode_exact(&homes.get(session)?.key)
}

pub fn note_homes<'a>(
    run: &Path,
    key: ComputerKey,
    at: Millis,
    sessions: impl Iterator<Item = &'a str>,
) -> io::Result<()> {
    let path = run.join("work/homes.json");
    let mut homes = kept::read::<BTreeMap<String, KeptHome>>(&path).unwrap_or_default();
    let key = encode(&key);
    let mut changed = false;
    for session in sessions {
        if homes
            .get(session)
            .is_none_or(|home| home.at <= at && home.key != key)
        {
            homes.insert(
                session.to_owned(),
                KeptHome {
                    key: key.clone(),
                    at,
                },
            );
            changed = true;
        }
    }
    if changed {
        kept::write(&path, &homes)?;
    }
    Ok(())
}

fn remove(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}
