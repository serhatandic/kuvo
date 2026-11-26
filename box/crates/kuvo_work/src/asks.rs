//! What agents ask you for while they browse here (`kuvo/asks`, PROTOCOL.md, "Per area"): a
//! sign-in, a code or a step only you can do, as the browser keeps them (`run/browser-asks.json`),
//! listed, followed, and answered over the browser's socket.

use kuvo_os::timer_fires::{self, Timer};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use http::{Method, StatusCode};
use kuvo_engine::Engine;
use kuvo_protocol::routes::{
    get_kuvo_asks as list, get_kuvo_asks_events as events, post_kuvo_asks_id as answer,
};
use kuvo_server::{Access, JsonLine, Refusal, Registry, RouteReply};
use serde_json::{Map, Value, json};

/// How long an answer waits on the browser to take it.
const ANSWERED_WITHIN: Duration = Duration::from_secs(5);
/// What these routes answer.
pub const FEATURES: &[&str] = &["asks"];
/// How many conversations' names are kept before they're all read again.
const MOST_KEPT: usize = 200;

/// The asks of the browser on this computer.
#[derive(Clone)]
pub struct Asks {
    run: PathBuf,
    /// This machine's name, as its asks name the machine whose browser it is; none on a Mac, whose
    /// own aren't named.
    machine: Option<String>,
    engine: Engine,
    /// The conversation each asking session goes to, and its name, as the engine said them.
    conversations: Arc<Mutex<HashMap<String, Conversation>>>,
}

#[derive(Clone)]
struct Conversation {
    id: String,
    name: Option<String>,
}

/// What's asked now: none while the browser isn't running (what one that ended, or a restart,
/// left was asked of no one now).
fn read_asks(run: &Path) -> Vec<Map<String, Value>> {
    let runs = std::fs::read_to_string(run.join("browser.pid"))
        .ok()
        .and_then(|pid| pid.trim().parse::<u32>().ok())
        .filter(|pid| *pid > 1)
        .is_some_and(|pid| !matches!(kuvo_os::process::ProcessIdentity::read(pid), Ok(None)));
    if !runs {
        return Vec::new();
    }
    let Some(Value::Object(mut file)) = kuvo_store::files::text_of(&run.join("browser-asks.json"))
        .and_then(|text| serde_json::from_str(&text).ok())
    else {
        return Vec::new();
    };
    let Some(Value::Array(asks)) = file.remove("asks") else {
        return Vec::new();
    };
    asks.into_iter()
        .filter_map(|ask| match ask {
            Value::Object(ask)
                if ask.get("id").is_some_and(Value::is_string)
                    && matches!(
                        ask.get("kind").and_then(Value::as_str),
                        Some("signIn" | "code" | "action")
                    ) =>
            {
                Some(ask)
            }
            _ => None,
        })
        .collect()
}

/// Text as JavaScript's `String` writes a value that may be missing.
fn text(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    }
}

/// One as `kuvo/asks` lists it: this machine's name, who asks (the asking conversation's name),
/// and no saved passwords to pick from.
fn listed(ask: &Map<String, Value>, machine: Option<&str>, asker: Option<&str>) -> Value {
    let kind = ask["kind"].as_str().unwrap_or_default();
    let mut listed = json!({ "id": ask["id"], "kind": kind, "host": text(ask.get("host")) });
    if let Some(machine) = machine {
        listed["machine"] = json!(machine);
    }
    if let Some(session) = ask.get("session").filter(|session| session.is_string()) {
        listed["session"] = session.clone();
    }
    if let Some(asker) = asker.filter(|asker| !asker.is_empty()) {
        listed["asker"] = json!(asker);
    }
    let truthy = |name| {
        ask.get(name).is_some_and(|value| match value {
            Value::Null => false,
            Value::Bool(yes) => *yes,
            Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
            Value::String(text) => !text.is_empty(),
            _ => true,
        })
    };
    match kind {
        "signIn" => {
            listed["username"] = json!(truthy("username"));
            listed["password"] = json!(truthy("password"));
            listed["saved"] = json!([]);
            listed["offersToSave"] = json!(false);
        }
        "code" => listed["prompt"] = json!(text(ask.get("prompt"))),
        _ => listed["instruction"] = json!(text(ask.get("instruction"))),
    }
    if ask.get("passkey") == Some(&Value::Bool(true)) {
        listed["passkey"] = json!(true);
    }
    listed
}

/// A client's answer (`POST kuvo/asks/<id>`) as the browser takes it; none for one that doesn't
/// fit the ask, or names a saved password (the Mac's alone).
fn answer_of(ask: &Map<String, Value>, given: &Value) -> Option<Value> {
    let kind = ask["kind"].as_str();
    let text = |name: &str| given[name].as_str().filter(|text| !text.is_empty());
    match given["answer"].as_str()? {
        "signIn" if kind == Some("signIn") && given.get("saved").is_none_or(Value::is_null) => {
            let mut answer = json!({ "type": "signIn" });
            for name in ["username", "password"] {
                if let Some(given) = text(name) {
                    answer[name] = json!(given);
                }
            }
            (answer.as_object().is_some_and(|answer| answer.len() > 1)).then_some(answer)
        }
        "code" if kind == Some("code") => {
            let code = given["code"].as_str().map(str::trim).unwrap_or_default();
            (!code.is_empty()).then(|| json!({ "type": "code", "code": code }))
        }
        "done" if kind == Some("action") => Some(json!({ "type": "done" })),
        "declined" => Some(json!({ "type": "declined" })),
        _ => None,
    }
}

/// Gives the browser an answer over its socket, the box's socket key first; whether it took it.
#[cfg(unix)]
async fn give(run: &Path, id: &str, answer: Value) -> bool {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let Some(key) = kuvo_store::files::text_of(&run.join("socket.key")) else {
        return false;
    };
    let socket = run.join("browser.sock");
    let taken = async {
        let mut stream = BufReader::new(tokio::net::UnixStream::connect(&socket).await?);
        let said = [
            json!({ "method": "Kuvo.key", "params": { "key": key.trim() } }),
            json!({ "id": 1, "method": "Kuvo.answer", "params": { "id": id, "answer": answer } }),
        ]
        .map(|line| line.to_string() + "\n")
        .concat();
        stream.get_mut().write_all(said.as_bytes()).await?;
        let mut line = String::new();
        while stream.read_line(&mut line).await? > 0 {
            if serde_json::from_str::<Value>(&line).is_ok_and(|reply| reply["id"] == 1) {
                return Ok::<_, std::io::Error>(true);
            }
            line.clear();
        }
        Ok(false)
    };
    tokio::time::timeout(ANSWERED_WITHIN, taken)
        .await
        .is_ok_and(|taken| taken.unwrap_or(false))
}

/// The browser doesn't run on native Windows yet: nothing is asked there to answer.
#[cfg(not(unix))]
async fn give(_run: &Path, _id: &str, _answer: Value) -> bool {
    false
}

impl Asks {
    pub fn new(home: &Path, machine: Option<String>, engine: Engine) -> Self {
        Self {
            run: home.join("run"),
            machine,
            engine,
            conversations: Arc::default(),
        }
    }

    /// The conversation an ask goes to: a subagent's is the one it works for, which shows it and
    /// waits on you; with its name. From the engine, kept a while.
    async fn conversation_of(&self, session: &str) -> Option<Conversation> {
        if let Some(known) = self.conversations.lock().expect("asks").get(session) {
            return Some(known.clone());
        }
        // (One the engine can't say: as asked, with no name, and asked again next time.)
        let unknown = || Conversation {
            id: session.to_owned(),
            name: None,
        };
        let Some(engine) = self.engine.running_bot_engine().await else {
            return Some(unknown());
        };
        let read = |id: String| {
            let engine = &engine;
            async move {
                let path = format!(
                    "api/session/{}",
                    percent_encoding::utf8_percent_encode(&id, percent_encoding::NON_ALPHANUMERIC)
                );
                engine
                    .call(Method::GET, &path, None, Duration::from_secs(10))
                    .await
                    .ok()
            }
        };
        let Some(mut current) = read(session.to_owned()).await else {
            return Some(unknown());
        };
        for _ in 0..5 {
            let Some(parent) = current["parentID"].as_str().filter(|id| !id.is_empty()) else {
                break;
            };
            let Some(parent) = read(parent.to_owned()).await else {
                return Some(unknown());
            };
            current = parent;
        }
        let found = Conversation {
            id: current["id"].as_str().unwrap_or(session).to_owned(),
            name: current["title"]
                .as_str()
                .filter(|title| !title.is_empty())
                .map(str::to_owned),
        };
        let mut kept = self.conversations.lock().expect("asks");
        if kept.len() > MOST_KEPT {
            kept.clear();
        }
        kept.insert(session.to_owned(), found.clone());
        Some(found)
    }

    /// What's asked now, as `kuvo/asks` lists it.
    async fn said(&self) -> Value {
        let mut asks = Vec::new();
        for mut ask in read_asks(&self.run) {
            let conversation = match ask.get("session").and_then(Value::as_str) {
                Some(session) if !session.is_empty() => self.conversation_of(session).await,
                _ => None,
            };
            if let Some(conversation) = &conversation {
                ask.insert("session".into(), json!(conversation.id));
            }
            let asker = conversation.as_ref().and_then(|each| each.name.as_deref());
            asks.push(listed(&ask, self.machine.as_deref(), asker));
        }
        json!({ "asks": asks })
    }

    pub fn register(self, routes: &mut Registry) {
        let here = self.clone();
        routes.register::<list::Spec, _, _>(Access::Any, move |_| {
            let asks = here.clone();
            async move { Ok(RouteReply::json::<200>(typed(asks.said().await)?)) }
        });
        let here = self.clone();
        routes.register::<events::Spec, _, _>(Access::Any, move |call| {
            let asks = here.clone();
            async move {
                Ok(RouteReply::ndjson::<200>(async_stream::try_stream! {
                    let mut last: Option<Value> = None;
                    let mut files = kuvo_os::watch::FileSet::new(kuvo_os::watch::WatchHub::default());
                    files.set_files(vec![asks.run.join("browser.pid"), asks.run.join("browser-asks.json")]);
                    let mut states = asks.engine.states();
                    let mut watched = None;
                    let mut exited = None;
                    loop {
                        let said = asks.said().await;
                        let has_asks = said["asks"].as_array().is_some_and(|rows| !rows.is_empty());
                        let id = if has_asks {
                            std::fs::read_to_string(asks.run.join("browser.pid")).ok()
                                .and_then(|pid| pid.trim().parse::<u32>().ok()).filter(|pid| *pid > 1)
                                .and_then(|pid| kuvo_os::process::ProcessIdentity::read(pid).ok().flatten())
                        } else { None };
                        if watched != id {
                            exited = id.as_ref().and_then(|id| id.watch_exit().ok());
                            watched = id;
                        }
                        if last.as_ref() != Some(&said) {
                            yield JsonLine::new(serde_json::from_value(said.clone())?)?;
                            last = Some(said);
                        }
                        let fallback = has_asks && exited.is_none();
                        tokio::select! {
                            () = call.cancel.cancelled() => break,
                            _ = files.changed() => {},
                            changed = states.changed() => if changed.is_err() { break; },
                            _ = async { match &mut exited {
                                Some(exit) => { let _ = exit.await; }, None => std::future::pending().await,
                            } } => { exited = None; },
                            _ = timer_fires::sleep(Timer::WorkAsksFollow, Duration::from_secs(1)), if fallback => {},
                        }
                    }
                }))
            }
        });
        let here = self;
        routes.register::<answer::Spec, _, _>(Access::Any, move |call| {
            let asks = here.clone();
            async move {
                let id = call
                    .param("id")
                    .and_then(|id| percent_encoding::percent_decode_str(id).decode_utf8().ok())
                    .map(|id| id.into_owned())
                    .unwrap_or_default();
                let Some(ask) = read_asks(&asks.run).into_iter().find(|ask| ask["id"] == id) else {
                    return Err(Refusal::new(StatusCode::NOT_FOUND, ""));
                };
                let given: Value =
                    serde_json::from_slice(&call.request.body).unwrap_or(Value::Null);
                let Some(answer) = answer_of(&ask, &given) else {
                    let saved = given.get("saved").is_some_and(|saved| !saved.is_null());
                    return Err(Refusal::new(
                        StatusCode::BAD_REQUEST,
                        if saved {
                            "Saved passwords are on your Mac: type this one."
                        } else {
                            ""
                        },
                    ));
                };
                if !give(&asks.run, &id, answer).await {
                    return Err(Refusal::new(StatusCode::SERVICE_UNAVAILABLE, ""));
                }
                Ok(RouteReply::json::<200>(typed(json!({}))?))
            }
        });
    }
}

fn typed<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, Refusal> {
    serde_json::from_value(value)
        .map_err(|error| Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(value: Value) -> Map<String, Value> {
        value.as_object().cloned().expect("an object")
    }

    #[test]
    fn an_ask_is_listed_as_a_mac_lists_one() {
        let sign_in = ask(
            json!({ "id": "a1", "kind": "signIn", "host": "example.com", "session": "ses_x", "password": 1 }),
        );
        assert_eq!(
            listed(&sign_in, Some("Lab"), Some("Trips")),
            json!({ "id": "a1", "kind": "signIn", "host": "example.com", "machine": "Lab", "session": "ses_x",
                    "asker": "Trips", "username": false, "password": true, "saved": [], "offersToSave": false })
        );
        let code = ask(json!({ "id": "a2", "kind": "code", "passkey": true }));
        assert_eq!(
            listed(&code, None, None),
            json!({ "id": "a2", "kind": "code", "host": "", "prompt": "", "passkey": true })
        );
    }

    #[test]
    fn an_answer_fits_its_ask_or_isnt_one() {
        let sign_in = ask(json!({ "id": "a1", "kind": "signIn" }));
        let code = ask(json!({ "id": "a2", "kind": "code" }));
        let step = ask(json!({ "id": "a3", "kind": "action" }));
        assert_eq!(
            answer_of(
                &sign_in,
                &json!({ "answer": "signIn", "username": "me", "password": "" })
            ),
            Some(json!({ "type": "signIn", "username": "me" }))
        );
        assert_eq!(
            answer_of(&sign_in, &json!({ "answer": "signIn", "saved": "me" })),
            None
        );
        assert_eq!(answer_of(&sign_in, &json!({ "answer": "signIn" })), None);
        assert_eq!(
            answer_of(&code, &json!({ "answer": "code", "code": " 123 " })),
            Some(json!({ "type": "code", "code": "123" }))
        );
        assert_eq!(answer_of(&code, &json!({ "answer": "done" })), None);
        assert_eq!(
            answer_of(&step, &json!({ "answer": "done" })),
            Some(json!({ "type": "done" }))
        );
        assert_eq!(
            answer_of(&step, &json!({ "answer": "declined" })),
            Some(json!({ "type": "declined" }))
        );
        assert_eq!(answer_of(&step, &json!({})), None);
    }
}
