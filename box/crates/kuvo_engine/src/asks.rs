//! Live questions and permissions from a conversation and its local descendants.

use kuvo_os::timer_fires::{self, Timer};
use std::{collections::BTreeSet, io, time::Duration};

use http::{Method, StatusCode};
use kuvo_protocol::routes::{
    get_kuvo_sessions_id_asks as read, get_kuvo_sessions_id_asks_events as events,
    post_kuvo_sessions_id_asks_ask as reply,
};
use http_body_util::BodyExt;
use kuvo_server::{Access, DispatchRequest, Dispatcher, JsonLine, Refusal, Registry, RouteReply};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, json};
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    Engine, ExternalAskKind, ExternalCard, ExternalDecision, ExternalReply, LocalSessions, api,
    lifetime::{Demand, Lease},
    sessions::{Session, id, typed},
};

#[derive(Deserialize)]
struct Permission {
    id: String,
    #[serde(default)]
    action: String,
    #[serde(default)]
    resources: Vec<String>,
    save: Option<Vec<String>>,
    message: Option<String>,
    #[serde(default)]
    metadata: Value,
    #[serde(default)]
    source: Value,
    #[serde(default)]
    time: Value,
}

#[derive(Deserialize)]
struct Form {
    id: String,
    title: Option<String>,
    #[serde(default)]
    metadata: Value,
    #[serde(default)]
    time: Value,
    #[serde(default)]
    fields: Vec<Field>,
}

#[derive(Deserialize)]
struct Field {
    key: Option<String>,
    #[serde(rename = "type", default)]
    kind: String,
    title: Option<String>,
    description: Option<String>,
    format: Option<String>,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    when: Vec<Value>,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    custom: bool,
    #[serde(default)]
    options: Vec<OptionText>,
    #[serde(default)]
    default: Value,
}

#[derive(Deserialize)]
struct OptionText {
    label: Option<String>,
    value: Option<String>,
    description: Option<String>,
}

#[derive(Serialize)]
struct OptionShown {
    value: String,
    label: String,
    description: Option<String>,
}

#[derive(Serialize)]
struct Question {
    key: String,
    title: Option<String>,
    question: String,
    options: Vec<OptionShown>,
    multiple: bool,
    custom: bool,
    required: bool,
}

enum Answer {
    Permission,
    Questions(Vec<Field>, Vec<Question>),
    Dismiss,
    External(ExternalAskKind),
}

struct Ask {
    id: String,
    session: String,
    at: u64,
    card: Value,
    answer: Answer,
    shared_folder: Option<String>,
}

/// Where a conversation runs, its asks read and answered the same way there: the engine's API, or
/// the coding agents' same routes (`kuvo/agents/…`), through this box's own dispatcher as asked.
enum Target<'a> {
    Engine(&'a Lease),
    Agents(&'a Dispatcher, &'a DispatchRequest),
}

impl Target<'_> {
    async fn raw(
        &self,
        engine: &Engine,
        method: Method,
        path: &str,
        body: Option<&Value>,
        headers: http::HeaderMap,
        wait: Duration,
    ) -> Result<Value, api::Error> {
        let (dispatcher, asked) = match self {
            Self::Engine(lease) => {
                return engine.call_with_headers(lease, method, path, body, wait, headers).await;
            }
            Self::Agents(dispatcher, asked) => (dispatcher, asked),
        };
        let request = DispatchRequest {
            method,
            path: format!("kuvo/agents/{path}"),
            headers,
            body: body.map(|body| body.to_string().into()).unwrap_or_default(),
            caller: asked.caller.clone(),
        };
        let reply = dispatcher.dispatch(request, Default::default()).await;
        let bytes = reply.body.collect().await.map(|body| body.to_bytes()).unwrap_or_default();
        let value = serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null);
        if !reply.status.is_success() {
            let message = value["message"].as_str().map_or_else(
                || format!("It answered {}.", reply.status.as_u16()),
                str::to_owned,
            );
            return Err(api::Error::Status(reply.status, message));
        }
        Ok(value)
    }

    async fn call<T: DeserializeOwned>(&self, engine: &Engine, path: &str) -> Result<T, api::Error> {
        let mut value = self.raw(engine, Method::GET, path, None, Default::default(), api::WAIT).await?;
        if let Some(data) = value.get_mut("data").filter(|data| !data.is_null()) {
            value = data.take();
        }
        serde_json::from_value(value).map_err(api::Error::Invalid)
    }

    /// A coding agent's conversation: its Always lasts for it, not the project.
    fn for_conversation(&self) -> bool {
        matches!(self, Self::Agents(..))
    }
}

impl Form {
    fn questions(&self) -> Option<Vec<Question>> {
        if self.metadata["kind"] != "question"
            || self.fields.iter().any(|field| {
                field.kind != "multiselect"
                    && (field.kind != "string" || field.format.as_deref() == Some("password"))
            })
        {
            return None;
        }
        let shown = self
            .fields
            .iter()
            .filter(|field| !field.hidden)
            .map(|field| {
                let key = field.key.clone().filter(|_| field.when.is_empty())?;
                let options = field
                    .options
                    .iter()
                    .map(|option| OptionShown {
                        value: option
                            .value
                            .clone()
                            .or_else(|| option.label.clone())
                            .unwrap_or_default(),
                        label: option
                            .label
                            .clone()
                            .or_else(|| option.value.clone())
                            .unwrap_or_default(),
                        description: option.description.clone(),
                    })
                    .collect();
                Some(Question {
                    key,
                    title: field.title.clone(),
                    question: field
                        .description
                        .clone()
                        .or_else(|| field.title.clone())
                        .or_else(|| self.title.clone())
                        .unwrap_or_default(),
                    options,
                    multiple: field.kind == "multiselect",
                    custom: field.custom || (field.kind == "string" && field.options.is_empty()),
                    required: field.required,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        (!shown.is_empty()).then_some(shown)
    }

    fn describe(&self) -> &str {
        match self.metadata["kind"].as_str() {
            Some("mcp-elicitation") => self.metadata["message"]
                .as_str()
                .or(self
                    .fields
                    .first()
                    .and_then(|field| field.description.as_deref().or(field.title.as_deref())))
                .unwrap_or("Needs your answer."),
            Some("question") => {
                let mut shown = self
                    .fields
                    .iter()
                    .filter(|field| !field.hidden && field.when.is_empty());
                match (shown.next(), shown.next()) {
                    (Some(field), None) if field.kind == "string" => field
                        .description
                        .as_deref()
                        .or(field.title.as_deref())
                        .or(self.title.as_deref())
                        .unwrap_or("Has a question for you."),
                    _ => "Has a question for you.",
                }
            }
            _ => "Needs your answer.",
        }
    }

    fn card(self, session: &str) -> Ask {
        let title = self
            .title
            .as_deref()
            .filter(|title| !title.is_empty())
            .unwrap_or("Question");
        let (card, answer) = if let Some(questions) = self.questions() {
            (
                json!({"kind":"question","title":title,"questions":questions}),
                Answer::Questions(self.fields, questions),
            )
        } else {
            (
                json!({"kind":"other","title":title,"message":self.describe(),"dismiss":true}),
                Answer::Dismiss,
            )
        };
        Ask::new(self.id, session, self.time, card, answer)
    }
}

impl Ask {
    fn new(id: String, session: &str, time: Value, mut card: Value, answer: Answer) -> Self {
        card["id"] = json!(id);
        card["session"] = json!(session);
        Self {
            id,
            session: session.to_owned(),
            at: time["created"].as_u64().unwrap_or_default(),
            card,
            answer,
            shared_folder: None,
        }
    }

    fn response(&self, given: &Value) -> Result<(Method, String, Option<Value>), Refusal> {
        let at = if self.shared_folder.is_some() {
            "api/session/global".to_owned()
        } else {
            format!("api/session/{}", api::segment(&self.session))
        };
        let ask = api::segment(&self.id);
        let form = format!("{at}/form/{ask}");
        let answer = match &self.answer {
            Answer::Permission => {
                let decision = match given["answer"].as_str() {
                    Some(answer @ ("once" | "always" | "reject")) => json!({"decision":answer}),
                    Some("instead") => {
                        let words = given["message"]
                            .as_str()
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .ok_or_else(|| invalid("Say what it should do instead."))?;
                        json!({"decision":"reject","message":words})
                    }
                    _ => return Err(invalid("Answer once, always, reject or instead.")),
                };
                return Ok((
                    Method::POST,
                    format!("{at}/permission/{ask}/reply"),
                    Some(decision),
                ));
            }
            _ if given["dismiss"] == true => return Ok((Method::DELETE, form, None)),
            Answer::Questions(fields, questions) => {
                question_answer(fields, questions, &given["answers"])?
            }
            Answer::Dismiss => {
                return Err(invalid(
                    "This one is answered where it was asked: only dismissing it goes from here.",
                ));
            }
            Answer::External(_) => return Err(gone()),
        };
        Ok((
            Method::POST,
            format!("{form}/reply"),
            Some(json!({"answer":answer})),
        ))
    }
}

fn question_answer(
    fields: &[Field],
    questions: &[Question],
    given: &Value,
) -> Result<Map<String, Value>, Refusal> {
    let given = given
        .as_object()
        .ok_or_else(|| invalid("Send the answers, by each question's key."))?;
    let mut answer = fields
        .iter()
        .filter_map(|field| {
            let key = field.key.as_ref()?;
            (!field.default.is_null()).then(|| (key.clone(), field.default.clone()))
        })
        .collect::<Map<_, _>>();
    for (key, value) in given {
        let question = questions
            .iter()
            .find(|question| question.key == *key)
            .ok_or_else(|| invalid(format!("It doesn't ask \"{key}\".")))?;
        let title = question.title.as_deref().unwrap_or(key);
        let picked = match (question.multiple, value) {
            (true, Value::String(_)) => json!([value]),
            _ => value.clone(),
        };
        let words = if question.multiple {
            picked.as_array().map(Vec::as_slice)
        } else {
            Some(std::slice::from_ref(&picked))
        };
        let words = words
            .filter(|words| words.iter().all(Value::is_string))
            .ok_or_else(|| {
                let want = if question.multiple {
                    "a list of words"
                } else {
                    "words"
                };
                invalid(format!("Answer \"{title}\" with {want}."))
            })?;
        if !question.custom
            && !question.options.is_empty()
            && !words
                .iter()
                .all(|word| question.options.iter().any(|option| *word == option.value))
        {
            return Err(invalid(format!("Pick one of the choices for \"{title}\".")));
        }
        answer.insert(key.clone(), picked);
    }
    for question in questions.iter().filter(|question| question.required) {
        let answered = answer.get(&question.key).is_some_and(|value| match value {
            Value::Array(values) => !values.is_empty(),
            Value::String(words) => !words.trim().is_empty(),
            _ => true,
        });
        if !answered {
            let title = question.title.as_deref().unwrap_or(&question.question);
            return Err(invalid(format!("Answer \"{title}\": it needs an answer.")));
        }
    }
    Ok(answer)
}

fn invalid(message: impl Into<String>) -> Refusal {
    Refusal::new(StatusCode::BAD_REQUEST, message)
}

fn unavailable() -> Refusal {
    Refusal::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "The engine isn't answering.",
    )
}

fn gone() -> Refusal {
    Refusal::new(StatusCode::NOT_FOUND, "It isn't waiting any more.")
}

fn relative<'a>(folder: &str, file: &'a str) -> &'a str {
    if folder.is_empty() {
        file
    } else {
        file.strip_prefix(folder.trim_end_matches('/'))
            .and_then(|rest| rest.strip_prefix('/'))
            .unwrap_or(file)
    }
}

fn outside(pattern: &str, home: &str) -> String {
    let path = pattern.trim();
    if matches!(path, "*" | "**") {
        return "Any folder".into();
    }
    let path = path
        .strip_suffix("/**")
        .or_else(|| path.strip_suffix("/*"))
        .unwrap_or(path);
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".into();
    }
    if trimmed == home {
        return "~".into();
    }
    if trimmed == "~" || trimmed.starts_with("~/") {
        return trimmed.into();
    }
    let short = relative(home, trimmed);
    if short == trimmed {
        short.into()
    } else {
        format!("~/{short}")
    }
}

fn spoken_folder(shown: &str) -> String {
    match shown {
        "/" => "The whole disk".into(),
        "~" => "Home".into(),
        "Any folder" => shown.into(),
        _ => shown
            .split('/')
            .filter(|part| !part.is_empty())
            .map(|part| if part == "~" { "Home" } else { part })
            .collect::<Vec<_>>()
            .join(", "),
    }
}

fn short(word: &str) -> String {
    let mut letters = word.graphemes(true);
    let head = letters.by_ref().take(23).collect::<String>();
    if letters.nth(1).is_some() {
        format!("{head}…")
    } else {
        word.to_owned()
    }
}

fn choice(
    answer: &str,
    title: &str,
    named: Option<String>,
    placeholder: Option<&str>,
    help: Option<String>,
    spoken: Option<String>,
) -> Value {
    json!({"answer":answer,"title":title,"named":named,"placeholder":placeholder,"help":help,"spoken":spoken})
}

impl Permission {
    fn always(&self, home: &str, for_conversation: bool) -> Value {
        let saved = self.save.as_ref().unwrap_or(&self.resources);
        let kinds = saved
            .iter()
            .filter(|name| !self.resources.contains(name))
            .collect::<Vec<_>>();
        let patterns = if kinds.is_empty() {
            saved.iter().collect()
        } else {
            kinds
        };
        let bare = patterns
            .iter()
            .map(|word| {
                word.strip_suffix(" *")
                    .or_else(|| word.strip_suffix("/*"))
                    .unwrap_or(word)
            })
            .collect::<Vec<_>>();
        let whole = patterns.len() == 1 && patterns[0] == "*";
        let here = if for_conversation {
            "in this conversation"
        } else {
            "in this project"
        };
        let (title, named, help, spoken) = match self.action.as_str() {
            "edit" | "patch" | "write" => (
                "Always Allow Edits",
                None,
                format!("Don’t ask again before changing files {here}"),
                format!("Always allow edits {here}"),
            ),
            "shell" | "bash" if bare.is_empty() || whole => (
                "Always Allow Commands",
                None,
                format!("Don’t ask again before running commands {here}"),
                format!("Always allow commands {here}"),
            ),
            "shell" | "bash" if bare.len() == 1 => {
                let kind = bare[0];
                let words = if patterns[0].ends_with(" *") {
                    format!("{kind} commands")
                } else {
                    kind.to_owned()
                };
                (
                    "Always Allow",
                    Some(short(kind)),
                    format!("Don’t ask again before running {words} {here}"),
                    format!("Always allow {words}"),
                )
            }
            "shell" | "bash" => {
                let words = bare.join(", ");
                (
                    "Always Allow These Commands",
                    None,
                    format!("Don’t ask again {here} before running: {words}"),
                    format!("Always allow these commands: {words}"),
                )
            }
            "external_directory" => {
                let folders = patterns
                    .iter()
                    .map(|pattern| outside(pattern, home))
                    .collect::<Vec<_>>();
                let spoken = folders
                    .iter()
                    .map(|folder| spoken_folder(folder))
                    .collect::<Vec<_>>()
                    .join("; ");
                let named = (folders.len() == 1).then(|| {
                    let name = &folders[0];
                    if name.graphemes(true).count() > 24 {
                        short(&format!("…/{}", name.rsplit('/').next().unwrap_or(name)))
                    } else {
                        name.clone()
                    }
                });
                let title = if named.is_some() {
                    "Always Allow"
                } else {
                    "Always Allow These Folders"
                };
                (
                    title,
                    named,
                    format!("Don’t ask again before working in {}", folders.join(", ")),
                    format!("Always allow working in {spoken}"),
                )
            }
            _ if bare.len() == 1 && !whole => {
                let kind = bare[0];
                (
                    "Always Allow",
                    Some(short(kind)),
                    format!("Don’t ask again for {kind} {here}"),
                    format!("Always allow {kind}"),
                )
            }
            _ => (
                "Always Allow",
                None,
                format!("Don’t ask again for this kind of request {here}"),
                format!("Always allow this kind of request {here}"),
            ),
        };
        choice("always", title, named, None, Some(help), Some(spoken))
    }

    fn card(
        self,
        session: &str,
        folder: &str,
        home: &str,
        command: Option<String>,
        for_conversation: bool,
    ) -> Ask {
        let mut subject = self
            .resources
            .first()
            .map_or("this", |first| relative(folder, first))
            .to_owned();
        if self.resources.len() > 1 {
            subject.push_str(&format!(" and {} more", self.resources.len() - 1));
        }
        let title = match self.action.as_str() {
            "edit" | "patch" => format!("Allow editing {subject}?"),
            "write" => format!("Allow writing {subject}?"),
            "read" => format!("Allow reading {subject}?"),
            "shell" | "bash" if self.resources.len() > 1 && command.is_none() => {
                "Allow running these commands?".into()
            }
            "shell" | "bash" => "Allow running this command?".into(),
            "external_directory" => "Allow working outside the project?".into(),
            "subagent" => format!("Allow starting the {subject} agent?"),
            "webfetch" => format!("Allow fetching {subject}?"),
            action => format!("Allow {}?", action.replace('_', " ")),
        };
        let files = self.metadata["files"].as_array().into_iter().flatten().filter_map(|row| {
            let file = row["file"].as_str()?;
            let status = row["status"].as_str().filter(|status| matches!(*status, "added" | "deleted" | "modified"));
            Some(json!({"file":file,"patch":row["patch"].as_str().unwrap_or_default(),
                "additions":row["additions"].as_i64().unwrap_or_default(), "deletions":row["deletions"].as_i64().unwrap_or_default(),"status":status}))
        }).collect::<Vec<_>>();
        let resources = self
            .resources
            .iter()
            .map(|each| {
                if self.action == "external_directory" {
                    outside(each, home)
                } else {
                    relative(folder, each).into()
                }
            })
            .collect::<Vec<_>>();
        let choices = [
            choice("once", "Allow Once", None, None, None, None),
            self.always(home, for_conversation),
            choice("reject", "Don’t Allow", None, None, None, None),
            choice(
                "instead",
                "Tell It What to Do Instead…",
                None,
                Some("Tell it what to do instead…"),
                None,
                None,
            ),
        ];
        Ask::new(
            self.id,
            session,
            self.time,
            json!({"kind":"permission","title":title,"message":self.message,
            "command":command,"files":files,"resources":resources,"choices":choices}),
            Answer::Permission,
        )
    }

    async fn command(&self, engine: &Engine, target: &Target<'_>, at: &str) -> Option<String> {
        if !matches!(self.action.as_str(), "bash" | "shell") {
            return None;
        }
        let id = self.source["id"].as_str()?;
        let message = self.source["messageID"].as_str();
        let messages = target
            .call::<Vec<Value>>(engine, &format!("{at}/message?limit=30&order=desc"))
            .await
            .ok()?;
        let part = messages
            .iter()
            .filter(|row| message.is_none_or(|id| row["id"] == id))
            .filter_map(|row| row["content"].as_array())
            .flatten()
            .find(|part| part["type"] == "tool" && part["id"] == id)?;
        part["state"]["input"]["command"]
            .as_str()
            .map(str::to_owned)
    }
}

/// Where `session` runs here: with the coding agents, else on the engine while it runs.
enum Place {
    Engine(Lease),
    Agents(Dispatcher, DispatchRequest),
}

impl Place {
    async fn of(
        engine: &Engine,
        session: &str,
        dispatcher: Dispatcher,
        viewer: &DispatchRequest,
    ) -> Result<Self, Refusal> {
        if let Some(agents) = &engine.agent_sessions
            && agents.session(session).await.ok().flatten().is_some()
        {
            return Ok(Self::Agents(dispatcher, viewer.clone()));
        }
        let lease = engine.acquire(Demand::Passive).await.ok_or_else(|| {
            Refusal::new(StatusCode::SERVICE_UNAVAILABLE, "The engine isn't running.")
        })?;
        Ok(Self::Engine(lease))
    }

    fn target(&self) -> Target<'_> {
        match self {
            Self::Engine(lease) => Target::Engine(lease),
            Self::Agents(dispatcher, viewer) => Target::Agents(dispatcher, viewer),
        }
    }
}

async fn found(
    engine: &Engine,
    session: &str,
    viewer: &DispatchRequest,
    dispatcher: Dispatcher,
) -> Result<Vec<Ask>, Refusal> {
    let place = Place::of(engine, session, dispatcher, viewer).await?;
    found_on(engine, &place.target(), session, false, Some(viewer)).await
}

async fn found_on(
    engine: &Engine,
    target: &Target<'_>,
    session: &str,
    live: bool,
    viewer: Option<&DispatchRequest>,
) -> Result<Vec<Ask>, Refusal> {
    let path = format!("api/session/{}", api::segment(session));
    let top = target
        .call::<Session>(engine, &path)
        .await
        .map_err(|error| {
            if error.missing() {
                Refusal::new(StatusCode::NOT_FOUND, "There's no such conversation here.")
            } else {
                unavailable()
            }
        })?;
    let mut askers = vec![session.to_owned()];
    let mut seen = BTreeSet::from([session.to_owned()]);
    // (A coding agent's subagents aren't conversations of their own here.)
    let depth = if target.for_conversation() { 0 } else { 40 };
    for index in 0..depth {
        if index > 0 && askers.len() > 30 {
            break;
        }
        let Some(parent) = askers.get(index).cloned() else {
            break;
        };
        let route = format!(
            "api/session?limit=50&order=desc&parentID={}",
            api::segment(&parent)
        );
        let rows = target
            .call::<Vec<Session>>(engine, &route)
            .await
            .map_err(|_| unavailable())?;
        askers.extend(
            rows.into_iter()
                .filter(|row| row.parent.as_deref() == Some(&parent) && seen.insert(row.id.clone()))
                .map(|row| row.id),
        );
    }

    let home = engine.account_home.to_string_lossy();
    let mut found = Vec::new();
    for asker in &askers {
        let at = format!("api/session/{}", api::segment(asker));
        let permission = format!("{at}/permission{}", if live { "?live=1" } else { "" });
        let form = format!("{at}/form");
        let (permissions, forms) = tokio::join!(
            target.call::<Vec<Permission>>(engine, &permission),
            target.call::<Vec<Form>>(engine, &form),
        );
        let (permissions, forms) = match (permissions, forms) {
            (Ok(permissions), Ok(forms)) => (permissions, forms),
            (Err(error), _) | (_, Err(error)) if asker != session && error.missing() => continue,
            _ => return Err(unavailable()),
        };
        for permission in permissions {
            let command = permission.command(engine, target, &at).await;
            let conversation = target.for_conversation();
            found.push(permission.card(asker, top.directory(), &home, command, conversation));
        }
        found.extend(forms.into_iter().map(|form| form.card(asker)));
    }
    found.extend(shared_forms(engine, target, session, top.directory(), &askers).await?);
    if let (Some(source), Some(viewer)) = (&engine.external_asks, viewer) {
        for ask in source.list(&askers, viewer).await? {
            if !askers.iter().any(|session| session == ask.card.session()) {
                continue;
            }
            let kind = match &ask.card {
                ExternalCard::Sandbox(_) => ExternalAskKind::Sandbox,
                ExternalCard::Vault(_) => ExternalAskKind::Vault,
            };
            found.push(Ask {
                id: ask.card.id().to_owned(),
                session: ask.card.session().to_owned(),
                at: ask.at,
                card: ask.card.encode(),
                answer: Answer::External(kind),
                shared_folder: None,
            });
        }
    }
    if !found.is_empty()
        && let Target::Engine(lease) = target
    {
        let heard = restore(engine, (lease.record.pid, lease.record.port)).await;
        for ask in &mut found {
            if let Some(pending) = heard.get(&ask.id) {
                ask.at = pending.at;
            }
        }
    }
    found.sort_by_key(|ask| {
        (
            matches!(ask.answer, Answer::Dismiss)
                || matches!(ask.answer, Answer::External(ExternalAskKind::Vault))
                    && ask.card["choices"].as_array().is_some_and(Vec::is_empty),
            ask.at,
        )
    });
    Ok(found)
}

async fn shared_forms(
    engine: &Engine,
    target: &Target<'_>,
    session: &str,
    folder: &str,
    askers: &[String],
) -> Result<Vec<Ask>, Refusal> {
    if folder.is_empty() {
        return Ok(Vec::new());
    }
    let mut forms = target
        .raw(engine, Method::GET, "api/session/global/form", None, api::folder_header(folder), api::WAIT)
        .await
        .map_err(|_| unavailable())?;
    if let Some(data) = forms.get_mut("data").filter(|data| !data.is_null()) {
        forms = data.take();
    }
    let forms = serde_json::from_value::<Vec<Value>>(forms).map_err(|_| unavailable())?;
    let mut found = Vec::new();
    let mut owners = std::collections::BTreeMap::new();
    for form in forms
        .into_iter()
        .filter_map(|form| serde_json::from_value::<Form>(form).ok())
    {
        let Some(server) = form.metadata["server"]
            .as_str()
            .map(str::trim)
            .filter(|name| !name.is_empty())
        else {
            continue;
        };
        if !owners.contains_key(server) {
            let owner = server_owner(engine, target, folder, server).await;
            owners.insert(server.to_owned(), owner);
        }
        let Some(owner) = owners[server]
            .as_ref()
            .filter(|owner| askers.contains(owner))
        else {
            continue;
        };
        let title = form
            .title
            .as_deref()
            .filter(|title| !title.is_empty())
            .unwrap_or(server);
        let card = json!({"kind":"other","title":title,"message":form.describe(),"dismiss":true});
        let mut ask = Ask::new(form.id, owner, form.time, card, Answer::Dismiss);
        ask.card["session"] = json!(session);
        ask.shared_folder = Some(folder.to_owned());
        found.push(ask);
    }
    Ok(found)
}

async fn server_owner(
    engine: &Engine,
    target: &Target<'_>,
    folder: &str,
    server: &str,
) -> Option<String> {
    let active = target
        .raw(engine, Method::GET, "api/session/active", None, Default::default(), api::WAIT)
        .await
        .ok()?;
    let mut prefix = server
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || matches!(ch, '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    prefix.push('_');
    for id in api::active_ids(&active["data"])? {
        let route = crate::sessions::path(id);
        let Ok(candidate) = target.call::<Value>(engine, &route).await else {
            continue;
        };
        if candidate
            .pointer("/location/directory")
            .and_then(Value::as_str)
            != Some(folder)
        {
            continue;
        }
        let Ok(messages) = target
            .call::<Vec<Value>>(engine, &format!("{route}/message?limit=8&order=desc"))
            .await
        else {
            continue;
        };
        let running = messages
            .iter()
            .filter(|message| message["type"] == "assistant")
            .flat_map(|message| message["content"].as_array().into_iter().flatten())
            .any(|part| {
                part["type"] == "tool"
                    && part["state"]["status"] == "running"
                    && part["name"].as_str().is_some_and(|name| {
                        name.starts_with(&prefix)
                            || (name == "execute"
                                && part
                                    .pointer("/state/input/code")
                                    .and_then(Value::as_str)
                                    .is_some_and(|code| code.contains(server)))
                    })
            });
        if running {
            return Some(id.to_owned());
        }
    }
    None
}

pub(crate) async fn stop_waiting(engine: &Engine, lease: &Lease, session: &str) {
    let Ok(asks) = found_on(engine, &Target::Engine(lease), session, false, None).await else {
        return;
    };
    if asks.is_empty() {
        return;
    }
    if crate::queue::hold_on(
        engine,
        crate::queue::Target::Engine(lease),
        session,
        Vec::new(),
        crate::queue::Hold::Queue,
    )
    .await
    .is_ok()
    {
        let _ = crate::queue::interrupt(engine, crate::queue::Target::Engine(lease), session, false).await;
    }
    let children = asks
        .into_iter()
        .map(|ask| ask.session)
        .filter(|id| id != session)
        .collect::<BTreeSet<_>>();
    for child in children {
        let _ = crate::queue::interrupt(engine, crate::queue::Target::Engine(lease), &child, false).await;
    }
}

async fn reading(
    engine: &Engine,
    session: &str,
    viewer: &DispatchRequest,
    dispatcher: Dispatcher,
) -> Result<Value, Refusal> {
    let asks = found(engine, session, viewer, dispatcher)
        .await?
        .into_iter()
        .map(|ask| ask.card)
        .collect::<Vec<_>>();
    Ok(json!({"asks": asks}))
}

pub(crate) fn register(engine: &Engine, list: LocalSessions, routes: &mut Registry) {
    let here = engine.clone();
    routes.register::<read::Spec, _, _>(Access::Any, move |call| {
        let here = here.clone();
        async move {
            Ok(RouteReply::json::<200>(typed(
                reading(&here, &id(&call)?, &call.request, call.dispatcher()).await?,
            )?))
        }
    });
    let here = engine.clone();
    let updates = list.clone();
    routes.register::<events::Spec, _, _>(Access::Any, move |call| {
        let here = here.clone();
        let mut changes = updates.ask_changes();
        let mut sessions = updates.clone();
        let mut states = here.states();
        let mut external = here.external_asks.as_ref().map(|source| source.waiting());
        async move {
            let session = id(&call)?;
            let mut current = reading(&here, &session, &call.request, call.dispatcher()).await?;
            Ok(RouteReply::ndjson::<200>(async_stream::try_stream! {
                let mut retry = false;
                loop {
                    let answer: events::Status200 = serde_json::from_value(current.clone()).map_err(io::Error::other)?;
                    yield JsonLine::new(answer)?;
                    loop {
                        tokio::select! {
                            changed = changes.changed() => if changed.is_err() { return; },
                            changed = sessions.changed() => if !changed { return; },
                            changed = states.changed() => if changed.is_err() { return; },
                            changed = async {
                                match &mut external {
                                    Some(external) => external.changed().await,
                                    None => std::future::pending().await,
                                }
                            } => if changed.is_err() { external = None; },
                            () = call.cancel.cancelled() => return,
                            () = timer_fires::sleep(Timer::EngineAsksFollow, Duration::from_secs(2)), if retry => {},
                        }
                        let next = match reading(&here, &session, &call.request, call.dispatcher()).await {
                            Ok(next) => { retry = false; next },
                            Err(error) => {
                                if error.reply().status == StatusCode::NOT_FOUND { retry = false; json!({"asks":[]}) } else {
                                    retry = matches!(*states.borrow(), crate::State::Running { .. });
                                    continue;
                                }
                            },
                        };
                        if next != current {
                            current = next;
                            break;
                        }
                    }
                }
            }))
        }
    });
    let here = engine.clone();
    routes.register::<reply::Spec, _, _>(Access::DesktopOrPaired, move |call| {
        let here = here.clone();
        async move {
            let session = id(&call)?;
            let ask = percent_encoding::percent_decode_str(call.param("ask").unwrap_or_default())
                .decode_utf8()
                .map_err(|_| gone())?;
            let given = serde_json::from_slice::<Value>(&call.request.body).unwrap_or_default();
            let place = Place::of(&here, &session, call.dispatcher(), &call.request).await?;
            let target = place.target();
            let found = found_on(&here, &target, &session, true, Some(&call.request))
                .await?
                .into_iter()
                .find(|item| item.id == ask)
                .ok_or_else(gone)?;
            if let Answer::External(kind) = found.answer {
                let source = here.external_asks.as_ref().ok_or_else(gone)?;
                crate::consent::live(&here, &call.request.caller, &call.cancel, [&session, &found.session])?;
                let item = given["item"].as_str();
                let choices = found.card["choices"].as_array().map(Vec::as_slice).unwrap_or_default();
                if kind == ExternalAskKind::Vault && choices.is_empty() {
                    return Err(Refusal::new(StatusCode::FORBIDDEN, "Answer it on your iPhone: only a device this computer’s vault knows can answer it."));
                }
                let answer = given["answer"].as_str();
                let offered = choices.iter().any(|choice| {
                    choice["answer"].as_str() == answer && choice["item"].as_str() == item
                });
                let decision = match (offered, answer) {
                    (true, Some("once")) => ExternalDecision::Once,
                    (true, Some("always")) => ExternalDecision::Always,
                    (true, Some("reject")) => ExternalDecision::Reject,
                    _ => {
                        let offered = choices.iter().filter_map(|choice| choice["answer"].as_str()).collect::<Vec<_>>().join(", ");
                        let message = if kind == ExternalAskKind::Vault && found.card["need"] == "item"
                            && item.is_none() && answer != Some("reject")
                        {
                            "Say which item to give it.".to_owned()
                        } else {
                            format!("Answer {offered}.")
                        };
                        return Err(invalid(message));
                    }
                };
                source.reply(ExternalReply { kind, session: &found.session, id: &ask, decision, item,
                    request: &call.request, cancel: &call.cancel }).await?;
                return Ok(RouteReply::json::<200>(reply::Status200 {}));
            }
            let (method, path, body) = found.response(&given)?;
            crate::consent::live(&here, &call.request.caller, &call.cancel, [&session, &found.session])?;
            let headers = found
                .shared_folder
                .as_deref()
                .map(api::folder_header)
                .unwrap_or_default();
            // (A coding agent keeps what Always allows itself.)
            if matches!(found.answer, Answer::Permission)
                && let Some(decision) = body.as_ref().and_then(|body| body["decision"].as_str())
                && let Target::Engine(lease) = target
            {
                crate::consent::grant(&here, lease, &found.session, &ask, decision, &headers)
                    .await?;
                crate::consent::live(&here, &call.request.caller, &call.cancel, [&session, &found.session])?;
            }
            target
                .raw(&here, method, &path, body.as_ref(), headers, Duration::from_secs(15))
                .await
            .map_err(|error| match error {
                api::Error::Status(StatusCode::NOT_FOUND, _) => gone(),
                api::Error::Status(
                    status @ (StatusCode::FORBIDDEN | StatusCode::SERVICE_UNAVAILABLE),
                    message,
                ) => Refusal::new(status, message),
                api::Error::Status(_, message) => Refusal::new(StatusCode::BAD_GATEWAY, message),
                _ => unavailable(),
            })?;
            Ok(RouteReply::json::<200>(reply::Status200 {}))
        }
    });
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Pending {
    pub session: String,
    pub at: u64,
    place: String,
    pub asks: Words,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Words {
    kind: String,
    summary: String,
}

pub(crate) type Book = std::collections::BTreeMap<String, Pending>;

/// The coding agents' asks waiting on you, beside the engine's.
pub(crate) fn include_agents(book: &mut Book, asks: &[crate::AgentWaiting]) {
    for ask in asks {
        let (kind, summary) = match &ask.ask {
            crate::AgentAsk::Permission { action, resources } => {
                let files = resources.iter().map(String::as_str).collect::<Vec<_>>();
                ("permission", action_words(Some(action), &files))
            }
            crate::AgentAsk::Question => ("question", "has a question for you".into()),
        };
        book.insert(
            format!("agents:{}", ask.id),
            Pending {
                session: ask.session.clone(),
                at: ask.at,
                place: "agents".into(),
                asks: Words {
                    kind: kind.into(),
                    summary,
                },
            },
        );
    }
}

pub(crate) fn include_external(book: &mut Book, asks: &[crate::ExternalWaiting]) {
    for ask in asks {
        let (kind, summary) = match ask.kind {
            ExternalAskKind::Sandbox => ("sandbox", "asks to run a command outside the sandbox"),
            ExternalAskKind::Vault => ("vault", "asks for something from the vault"),
        };
        book.insert(
            format!("{kind}:{}", ask.id),
            Pending {
                session: ask.session.clone(),
                at: ask.at,
                place: kind.into(),
                asks: Words {
                    kind: kind.into(),
                    summary: summary.into(),
                },
            },
        );
    }
}

#[derive(Deserialize, Serialize)]
struct Checkpoint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    engine: Option<(u32, u16)>,
    asks: Book,
}

#[derive(Deserialize, Serialize)]
struct CheckpointSource {
    engine: (u32, u16),
    fingerprint: String,
}

pub(crate) async fn restore(engine: &Engine, source: (u32, u16)) -> Book {
    let kept = crate::disk::read::<Checkpoint>(&engine.home.join("run/glance/asks.json"))
        .await
        .ok()
        .flatten();
    let Some(kept) = kept else {
        return Book::new();
    };
    let matches = if let Some(earlier) = kept.engine {
        earlier == source
    } else {
        crate::disk::read::<CheckpointSource>(&engine.home.join("run/glance-source.json"))
            .await
            .ok()
            .flatten()
            .is_some_and(|earlier| {
                earlier.engine == source
                    && earlier.fingerprint == crate::submissions::fingerprint(json!(kept.asks))
            })
    };
    if matches { kept.asks } else { Book::new() }
}

pub(crate) async fn checkpoint(engine: &Engine, source: (u32, u16), asks: &Book) {
    let tag = CheckpointSource {
        engine: source,
        fingerprint: crate::submissions::fingerprint(json!(asks)),
    };
    let checkpoint = Checkpoint {
        engine: None,
        asks: asks.clone(),
    };
    let tagged = crate::disk::durable(engine.home.join("run/glance-source.json"), tag).await;
    if tagged.is_ok() {
        let _ = crate::disk::durable(engine.home.join("run/glance/asks.json"), checkpoint).await;
    }
}

pub(crate) fn note(book: &mut Book, kind: &str, data: &Value) -> bool {
    match kind {
        "permission.asked" | "form.created" => {
            let item = data
                .get("form")
                .filter(|form| form.is_object())
                .unwrap_or(data);
            let (Some(id), Some(session)) = (item["id"].as_str(), item["sessionID"].as_str())
            else {
                return false;
            };
            if !crate::sessions::valid_id(session) || book.contains_key(id) {
                return false;
            }
            let (kind, summary) = match kind {
                "permission.asked" => ("permission", permission_words(item)),
                _ if item["metadata"]["kind"] == "question" => {
                    ("question", "has a question for you".into())
                }
                _ => ("other", "needs your answer".into()),
            };
            book.insert(
                id.to_owned(),
                Pending {
                    session: session.to_owned(),
                    at: crate::disk::now(),
                    place: "engine".into(),
                    asks: Words {
                        kind: kind.into(),
                        summary,
                    },
                },
            );
            true
        }
        "permission.replied" | "permission.rejected" | "form.replied" | "form.cancelled" => {
            data["requestID"]
                .as_str()
                .or_else(|| data["id"].as_str())
                .is_some_and(|id| book.remove(id).is_some())
        }
        "session.execution.succeeded"
        | "session.execution.failed"
        | "session.execution.interrupted"
        | "session.deleted" => {
            let Some(id) = data["sessionID"].as_str() else {
                return false;
            };
            let before = book.len();
            book.retain(|_, item| item.session != id);
            before != book.len()
        }
        _ => false,
    }
}

fn permission_words(item: &Value) -> String {
    let files = item["resources"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    action_words(item["action"].as_str(), &files)
}

fn action_words(action: Option<&str>, files: &[&str]) -> String {
    let file = match files {
        [] => "a file".to_owned(),
        [file] => clip(
            file.trim_end_matches('/')
                .rsplit('/')
                .next()
                .filter(|part| !part.is_empty())
                .unwrap_or(file),
        ),
        _ => format!("{} files", files.len()),
    };
    match action {
        Some("edit" | "patch" | "write") => format!("asks to edit {file}"),
        Some("read") => format!("asks to read {file}"),
        Some("shell" | "bash") => "asks to run a command".into(),
        Some("external_directory") => "asks to work outside the project".into(),
        Some("subagent") => "asks to start an agent".into(),
        Some("webfetch") => "asks to fetch a page".into(),
        Some(action) if !action.is_empty() => {
            format!("asks to {}", clip(&action.replace('_', " ")))
        }
        _ => "asks for your OK".into(),
    }
}

fn clip(words: &str) -> String {
    if words.encode_utf16().count() <= 60 {
        return words.to_owned();
    }
    let prefix = String::from_utf16_lossy(&words.encode_utf16().take(59).collect::<Vec<_>>());
    format!("{prefix}…")
}
