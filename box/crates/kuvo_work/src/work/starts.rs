//! A lead on another computer starting a session here (PROTOCOL.md, "Peers", `call`,
//! `start_session`): started once, kept as it goes (`run/calls/<id>.json`, as the TypeScript box
//! keeps it), so one cut short is finished from the step it got to when its call comes again, never
//! made twice; and what it said.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use http::Method;
use kuvo_engine::{RemoteLead, StartSteps};
use kuvo_peers::{
    calls::{Answer, MailedCall, known_home, note_homes, plain},
    directory::ComputerKey,
};
use kuvo_wire::keys::{decode_exact, encode};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{Work, watch::tools::refusal_words};

/// What a start's call may ask for, as start_session takes it here.
const FIELDS: &[&str] = &[
    "task", "title", "project", "folder", "chat", "worktree", "model", "effort",
    "approval", "wake", "wait", "review", "item",
];
const APPROVALS: &[&str] = &["askFirst", "acceptEdits", "auto", "fullAccess"];
const LIMITS: &[&str] = &["atOnce", "perDay", "depth", "tokensPerDay"];
const WAIT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Done {
    text: String,
    refused: bool,
}

/// A start's call, as it's kept here: from whom, what it asked and who it is (`given`, `start`),
/// when it was made, where it's got to, and once it's done, what it said.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Journal {
    id: String,
    from: String,
    sender: String,
    computer: String,
    name: String,
    given: Map<String, Value>,
    start: Value,
    made: i64,
    at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    making: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prompted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    done: Option<Done>,
}

/// A journal kept on disk as each step is done.
struct Kept {
    file: PathBuf,
    journal: Mutex<Journal>,
}

impl Kept {
    fn change(&self, change: impl FnOnce(&mut Journal)) {
        let mut journal = self.journal.lock().expect("journal");
        change(&mut journal);
        let text = serde_json::to_string(&*journal).expect("JSON");
        if let Some(folder) = self.file.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        let _ = kuvo_store::files::write_secret(&self.file, text.as_bytes());
    }

    fn now(&self) -> Journal {
        self.journal.lock().expect("journal").clone()
    }
}

impl StartSteps for Kept {
    fn making(&self, making: &Value) {
        self.change(|journal| journal.making = Some(making.clone()));
    }

    fn made(&self, session: &str) {
        self.change(|journal| journal.session = Some(session.to_owned()));
    }

    fn prompted(&self) {
        self.change(|journal| journal.prompted = Some(true));
    }
}

fn session_id(id: &str) -> bool {
    id.strip_prefix("ses_").is_some_and(|rest| {
        (1..=60).contains(&rest.len()) && rest.bytes().all(|byte| byte.is_ascii_alphanumeric())
    })
}

fn cut(value: &Value, most: usize) -> String {
    value
        .as_str()
        .unwrap_or_default()
        .chars()
        .take(most)
        .collect()
}

/// `as.start` as it came, cleaned, for a call from `from`; none for one that isn't.
fn start_of(given: &Value, from: &str) -> Option<Value> {
    let approval = given["approval"]
        .as_str()
        .filter(|each| APPROVALS.contains(each))?;
    let held = given["held"].as_array().filter(|held| held.len() <= 300)?;
    let line = given["line"].as_array()?;
    let rule = |rule: &Value| -> Option<Value> {
        let text = |field: &str| rule[field].as_str();
        let effect = text("effect").filter(|effect| !effect.is_empty())?;
        // (Only what holds back: an allow or an ask a lead's computer can't bring.)
        (effect != "allow" && effect != "ask").then(|| {
            json!({ "action": cut(&rule["action"], 200), "resource": cut(&rule["resource"], 2000), "effect": cut(&rule["effect"], 40) })
        })
    };
    let held: Vec<Value> = held.iter().map(rule).collect::<Option<_>>()?;
    let line: Vec<&str> = line
        .iter()
        .filter_map(Value::as_str)
        .filter(|id| session_id(id))
        .collect();
    if line.len() != given["line"].as_array()?.len() || line.is_empty() || line.len() > 20 {
        return None;
    }
    // (Its line ends with the lead itself.)
    if line.last() != Some(&from) {
        return None;
    }
    let model = &given["model"];
    let model = (model["providerID"].is_string() && model["id"].is_string()).then(|| {
        let mut shown =
            json!({ "providerID": cut(&model["providerID"], 120), "id": cut(&model["id"], 200) });
        if model["variant"].is_string() {
            shown["variant"] = json!(cut(&model["variant"], 60));
        }
        shown
    });
    let works = &given["works"];
    let bot = works["bot"].as_str().filter(|bot| {
        bot.strip_prefix("bot_").is_some_and(|rest| {
            (1..=60).contains(&rest.len()) && rest.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
    });
    let works = bot.filter(|_| works["name"].is_string()).map(|bot| {
        let name = plain(works["name"].as_str().unwrap_or_default(), 100);
        json!({ "bot": bot, "name": if name.is_empty() { "a bot".into() } else { name } })
    });
    let depth = given["depth"]
        .as_u64()
        .filter(|depth| *depth >= 1)
        .map_or(1, |depth| depth.min(50));
    let homes: Map<String, Value> = given["homes"]
        .as_object()
        .into_iter()
        .flatten()
        .take(20)
        .filter(|(session, key)| {
            session_id(session) && key.as_str().is_some_and(|key| key.len() <= 100)
        })
        .map(|(session, key)| (session.clone(), key.clone()))
        .collect();
    let limits: Map<String, Value> = LIMITS
        .iter()
        .filter_map(|name| {
            let value = given["limits"][name].as_u64().filter(|value| *value >= 1)?;
            Some(((*name).to_owned(), json!(value)))
        })
        .collect();
    Some(json!({
        "approval": approval, "held": held,
        "model": model, "own": given["own"] == true && works.is_some(),
        "works": works, "depth": depth, "line": line, "homes": homes,
        "limits": if limits.is_empty() { Value::Null } else { Value::Object(limits) },
    }))
}

/// Whether a start's call is one: its `as.start` is there and whole, and its line ends with its lead.
pub fn start_call(call: &MailedCall) -> bool {
    let start = call.request.lead.start.as_ref();
    start
        .and_then(|start| start_of(start, &call.from))
        .is_some()
}

/// What a session started with less than it was asked for does on its own.
fn got_words(got: &str) -> &'static str {
    match got {
        "askFirst" => "It asks before it edits or runs commands.",
        "acceptEdits" => "It edits on its own, and asks before it runs commands.",
        "auto" => "It works on its own inside the sandbox, and asks for anything beyond it.",
        _ => "",
    }
}

/// How a start went, in the words the tool gives its agent.
pub fn started_words(started: &Value) -> String {
    let text = |field: &str| started[field].as_str().filter(|text| !text.is_empty());
    let name = text("title").map_or("a new session".to_owned(), |title| format!("\"{title}\""));
    let place = text("project")
        .map(|project| format!(" in {project}"))
        .unwrap_or_default();
    let what = text("picked").map_or(".".to_owned(), |picked| format!(": {picked}."));
    let model = text("model")
        .map(|model| format!(" {model}"))
        .unwrap_or_default();
    let got = text("got").map(|got| {
        let held = text("held")
            .map(|held| format!(" {held}"))
            .unwrap_or_default();
        format!(" {}{held}", got_words(got))
    });
    let wakes: Vec<&str> = started["wakes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let hears = match wakes.as_slice() {
        [] => " Write to it with send_message; it can write back to you the same way.".to_owned(),
        [_, _] => {
            " You'll be woken when it finishes or asks for something, so don't check on it.".into()
        }
        ["finished"] => " You'll be woken when it finishes, so don't check on it.".into(),
        _ => " You'll be woken when it asks for something, so don't check on it.".into(),
    };
    let warned = text("warning")
        .map(|warning| format!(" {warning}"))
        .unwrap_or_default();
    let session = text("session").unwrap_or_default();
    format!(
        "Started {name}{place} (address {session}){what}{model}{} It's on it now.{hears}{warned}",
        got.unwrap_or_default()
    )
}

impl Work {
    /// A start's call from `sender` (checked who it's from, and that this computer takes work from
    /// your other computers): done once, from the step it got to, and what it said.
    pub async fn take_start(
        &self,
        sender: ComputerKey,
        call: &MailedCall,
    ) -> std::io::Result<Answer> {
        let file = self
            .home
            .join("run/calls")
            .join(format!("{}.json", call.id.as_str()));
        let kept = kuvo_store::files::text_of(&file)
            .and_then(|text| serde_json::from_str::<Journal>(&text).ok());
        let key = encode(&sender);
        if let Some(kept) = &kept
            && (kept.sender != key || kept.from != call.from)
        {
            return Ok(Answer::refused(
                "This call id already belongs to another conversation.",
            ));
        }
        if let Some(done) = kept.as_ref().and_then(|kept| kept.done.clone()) {
            return Ok(Answer {
                text: done.text,
                refused: done.refused,
            });
        }
        let start = call
            .request
            .lead
            .start
            .as_ref()
            .and_then(|start| start_of(start, &call.from));
        let start = start.ok_or_else(|| std::io::Error::other("That isn't a call."))?;
        let view = self.peers.view();
        let computer = view
            .roster
            .member(&sender)
            .map(|member| member.name.clone());
        let computer = plain(computer.as_deref().unwrap_or_default(), 100);
        let given = FIELDS
            .iter()
            .filter_map(|field| Some(((*field).to_owned(), call.request.args.get(*field)?.clone())))
            .collect();
        let journal = kept.unwrap_or_else(|| Journal {
            id: call.id.as_str().to_owned(),
            from: call.from.clone(),
            sender: key.clone(),
            computer: if computer.is_empty() {
                "another computer".into()
            } else {
                computer
            },
            name: call.request.lead.name.clone(),
            given,
            start,
            made: call.made,
            at: jiff::Timestamp::now().as_millisecond(),
            ..Journal::default()
        });
        let kept = Arc::new(Kept {
            file,
            journal: Mutex::new(journal),
        });
        kept.change(|_| {});
        let done = self.finish_start(&kept).await?;
        if !done.refused {
            self.note_line_homes(&kept.now());
        }
        kept.change(|journal| journal.done = Some(done.clone()));
        Ok(Answer {
            text: done.text,
            refused: done.refused,
        })
    }

    // The start, from where it got to: made and told its task as far as it wasn't, or started.
    async fn finish_start(&self, kept: &Kept) -> std::io::Result<Done> {
        let journal = kept.now();
        let engine = self.list.engine();
        if let (Some(session), Some(making)) = (&journal.session, &journal.making) {
            if journal.prompted != Some(true) {
                let bot = engine
                    .running_bot_engine()
                    .await
                    .ok_or_else(|| std::io::Error::other("The engine isn't running."))?;
                let prompt = &making["prompt"];
                let message =
                    json!({ "id": prompt["id"], "text": prompt["text"], "delivery": "steer" });
                let path = format!("api/session/{session}/prompt");
                bot.call(Method::POST, &path, Some(&message), WAIT)
                    .await
                    .map_err(|error| std::io::Error::other(error.0))?;
                kept.prompted();
            }
            return Ok(Done {
                text: started_words(&making["started"]),
                refused: false,
            });
        }
        let start = &journal.start;
        let strings = |value: &Value| {
            value
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        let lead = RemoteLead {
            session: journal.from.clone(),
            name: journal.name.clone(),
            call: journal.id.clone(),
            computer: journal.computer.clone(),
            key: journal.sender.clone(),
            approval: start["approval"].clone(),
            held: start["held"].as_array().cloned().unwrap_or_default(),
            model: Some(start["model"].clone()).filter(|model| model.is_object()),
            works: Some(start["works"].clone()).filter(|works| works.is_object()),
            own: start["own"] == true,
            depth: start["depth"].as_u64().unwrap_or(1),
            line: strings(&start["line"]),
            limits: Some(start["limits"].clone()).filter(|limits| limits.is_object()),
        };
        match engine.start_for_lead(&journal.given, &lead, kept).await {
            Ok(started) => Ok(Done {
                text: started_words(&started),
                refused: false,
            }),
            Err(refusal) => Ok(Done {
                text: refusal_words(refusal).await,
                refused: true,
            }),
        }
    }

    /// Word from the computer that called that the conversations above its lead live where it says,
    /// kept only for those this one has no word on yet, and only on computers it knows.
    fn note_line_homes(&self, journal: &Journal) {
        let run = self.home.join("run");
        let view = self.peers.view();
        let line = &journal.start["line"];
        let homes = journal.start["homes"].as_object().into_iter().flatten();
        for (session, key) in homes {
            let on_line = line
                .as_array()
                .is_some_and(|line| line.iter().any(|each| each == session));
            let Some(key): Option<ComputerKey> = key.as_str().and_then(decode_exact::<32>) else {
                continue;
            };
            if on_line && view.roster.member(&key).is_some() && known_home(&run, session).is_none()
            {
                let at = jiff::Timestamp::now().as_millisecond();
                let _ = note_homes(&run, key, at, std::iter::once(session.as_str()));
            }
        }
    }
}
