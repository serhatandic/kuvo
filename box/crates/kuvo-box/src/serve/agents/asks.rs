//! The native requests a coding agent is waiting for this resident to answer.

use http::StatusCode;
use kuvo_agents::{
    feed::Feed,
    live_asks::{Ask, AskKey, Ended, Reply},
    permissions::remember,
    session::{Answer, Decision, Field, FieldKind},
    session_runs::SessionRuns,
    session_store::SessionStore,
};
use kuvo_protocol::routes::{
    delete_kuvo_agents_api_session_id_form_request as dismiss,
    get_kuvo_agents_api_session_id_form as forms,
    get_kuvo_agents_api_session_id_permission as permission,
    post_kuvo_agents_api_session_id_form_request_reply as answer_form,
    post_kuvo_agents_api_session_id_permission_request_reply as answer_permission,
};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use serde_json::{Map, Value, json};
use std::{collections::BTreeSet, io};

pub(crate) fn register(runs: SessionRuns, store: SessionStore, routes: &mut Registry) {
    let ask_runs = runs.clone();
    let ask_store = store.clone();
    routes.register::<permission::Spec, _, _>(Access::Any, move |call| {
        let runs = ask_runs.clone();
        let store = ask_store.clone();
        async move {
            let id = read_session(&store, call.param("id")).await?;
            // After an update, a turn taken up shows again what it asked:
            // an ordinary read waits for that rather than show none.
            let live = call
                .request
                .path
                .split_once('?')
                .is_some_and(|(_, query)| query.split('&').any(|pair| pair == "live=1"));
            if !live {
                tokio::select! {
                    () = runs.asks_restored(&id) => {}
                    () = call.cancel.cancelled() => {
                        return Err(Refusal::new(
                            StatusCode::SERVICE_UNAVAILABLE,
                            "Kuvo is restoring this session’s approvals. Try again in a moment.",
                        ));
                    }
                }
            }
            let mut cards = cards(&runs, &id, false);
            with_sources(&store, &id, &mut cards).await;
            Ok(RouteReply::json::<200>(permission::Status200 {
                data: cards,
            }))
        }
    });
    let ask_runs = runs.clone();
    let ask_store = store.clone();
    routes.register::<forms::Spec, _, _>(Access::Any, move |call| {
        let runs = ask_runs.clone();
        let store = ask_store.clone();
        async move {
            let id = read_session(&store, call.param("id")).await?;
            let cards = cards(&runs, &id, true);
            Ok(RouteReply::json::<200>(forms::Status200 { data: cards }))
        }
    });
    let ask_runs = runs.clone();
    routes.register::<answer_permission::Spec, _, _>(Access::DesktopOrPaired, move |call| {
        let runs = ask_runs.clone();
        async move {
            let (key, _) = pending(&runs, call.param("id"), call.param("request"), false)?;
            let decision = match call.body["decision"].as_str() {
                Some("once") => Decision::Once,
                Some("always") => Decision::Always,
                Some("reject") => Decision::Reject,
                _ => {
                    return Err(Refusal::new(
                        StatusCode::BAD_REQUEST,
                        "Answer once, always or reject.",
                    ));
                }
            };
            runs.reply_permission(key, decision, || call.cancel.is_cancelled())
                .await
                .map_err(refuse)?;
            Ok(RouteReply::json::<200>(answer_permission::Status200 {}))
        }
    });
    let ask_runs = runs.clone();
    routes.register::<answer_form::Spec, _, _>(Access::DesktopOrPaired, move |call| {
        let runs = ask_runs.clone();
        async move {
            let (key, _) = pending(&runs, call.param("id"), call.param("request"), true)?;
            let fields = call.body["answer"].as_object().cloned().unwrap_or_default();
            runs.asks()
                .reply(key, Reply::Question(Answer::Submitted(fields)))
                .await
                .map_err(refuse)?;
            Ok(RouteReply::json::<200>(answer_form::Status200 {}))
        }
    });
    routes.register::<dismiss::Spec, _, _>(Access::DesktopOrPaired, move |call| {
        let runs = runs.clone();
        async move {
            let (key, _) = pending(&runs, call.param("id"), call.param("request"), true)?;
            // Turned down by its box with words (Autopilot): those are what the agent hears.
            let reason = call.request.path.split_once('?').and_then(|(_, query)| {
                query.split('&').find_map(|pair| {
                    let words = pair.strip_prefix("message=")?.replace('+', " ");
                    percent_encoding::percent_decode_str(&words)
                        .decode_utf8()
                        .ok()
                        .map(|words| words.into_owned())
                })
            });
            runs.asks()
                .reply(key, Reply::Question(Answer::Dismissed { reason }))
                .await
                .map_err(refuse)?;
            Ok(RouteReply::json::<200>(dismiss::Status200 {}))
        }
    });
}

async fn read_session(store: &SessionStore, given: Option<&str>) -> Result<String, Refusal> {
    let id = super::turns::session_id(given)?;
    if id != "global" && store.read(&id).await.map_err(super::refuse)?.is_none() {
        return Err(super::sessions::missing(&id));
    }
    Ok(id)
}

/// Where each card's tool call is in the conversation: its step and call.
async fn with_sources(store: &SessionStore, session: &str, cards: &mut [Value]) {
    if !cards.iter().any(|card| card.get("tool").is_some()) {
        return;
    }
    let steps = store
        .message_page(
            session,
            None,
            200,
            kuvo_agents::session_store::MessageOrder::NewestFirst,
        )
        .await
        .ok()
        .flatten()
        .map(|page| page.messages)
        .unwrap_or_default();
    for card in cards.iter_mut() {
        let Some(tool) = card.as_object_mut().and_then(|card| card.remove("tool")) else {
            continue;
        };
        let step = steps.iter().find(|row| {
            row["type"] == "assistant"
                && row["content"].as_array().is_some_and(|parts| {
                    parts
                        .iter()
                        .any(|part| part["type"] == "tool" && part["id"] == tool)
                })
        });
        if let Some(step) = step.and_then(|step| step["id"].as_str()) {
            card["source"] = json!({"type":"tool","messageID":step,"id":tool});
        }
    }
}

fn cards(runs: &SessionRuns, session: &str, questions: bool) -> Vec<Value> {
    let pending = runs.asks().pending();
    let current = pending.borrow();
    current
        .iter()
        .filter(|(key, ask)| key.session == session && matches!(ask, Ask::Question(_)) == questions)
        .map(|(key, ask)| card(key, ask))
        .collect()
}

/// One ask as clients see it: a permission's card, or a question's form.
fn card(key: &AskKey, ask: &Ask) -> Value {
    let session = &key.session;
    match ask {
        Ask::Permission(ask) => {
            let mut card = json!({
                "id":key.client_id(false),"sessionID":session,"action":ask.action,"resources":ask.resources,
                "save":remember(ask).patterns,
            });
            if let Some(message) = &ask.message {
                card["message"] = json!(message);
            }
            if !ask.metadata.is_empty() {
                card["metadata"] = json!(ask.metadata);
            }
            if ask.sandboxed == Some(true) {
                card["sandboxed"] = true.into();
            }
            // (Its place in the conversation, filled in by `with_sources`.)
            if let Some(tool) = &ask.tool_call_id {
                card["tool"] = json!(tool);
            }
            card
        }
        Ask::Question(ask) => {
            let mut context = Map::<String, Value>::new();
            context.insert("kind".into(), "question".into());
            context.extend(ask.metadata.clone());
            if let Some(tool) = &ask.tool_call_id {
                context.insert("tool".into(), json!({"id":tool}));
            }
            json!({
                "id":key.client_id(true),"sessionID":session,"title":ask.title,"metadata":context,
                "fields":ask.fields.iter().map(field).collect::<Vec<_>>()
            })
        }
    }
}

/// What the agents ask, told on their feed as it's asked and as it ends: `permission.asked` and
/// `permission.replied`, `form.created`, and `form.replied` or `form.cancelled`.
pub(crate) fn follow(runs: SessionRuns, store: SessionStore, feed: Feed) {
    tokio::spawn(async move {
        let asks = runs.asks().clone();
        let mut pending = asks.pending();
        let mut ended = asks.ended();
        let mut known: BTreeSet<AskKey> = pending.borrow().keys().cloned().collect();
        let directory = |session: String| {
            let store = store.clone();
            async move {
                let header = store.read(&session).await.ok()??;
                header.directory().map(str::to_owned)
            }
        };
        loop {
            tokio::select! {
                changed = pending.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    let now = pending.borrow_and_update().clone();
                    for (key, ask) in now.iter().filter(|(key, _)| !known.contains(*key)) {
                        let mut card = card(key, ask);
                        with_sources(&store, &key.session, std::slice::from_mut(&mut card)).await;
                        let at = directory(key.session.clone()).await;
                        match ask {
                            Ask::Permission(_) => feed.emit_from("permission.asked", card, at),
                            Ask::Question(_) => feed.emit_from("form.created", json!({ "form": card }), at),
                        }
                    }
                    known = now.keys().cloned().collect();
                }
                gone = ended.recv() => match gone {
                    Ok(Ended { key, ask, reply }) => {
                        let at = directory(key.session.clone()).await;
                        match ask {
                            Ask::Permission(_) => feed.emit_from(
                                "permission.replied",
                                json!({ "sessionID": key.session, "requestID": key.client_id(false) }),
                                at,
                            ),
                            Ask::Question(_) => {
                                let answered = matches!(reply, Some(Reply::Question(answer)) if !matches!(answer, Answer::Dismissed { .. }));
                                feed.emit_from(
                                    if answered { "form.replied" } else { "form.cancelled" },
                                    json!({ "id": key.client_id(true), "sessionID": key.session }),
                                    at,
                                );
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                },
            }
        }
    });
}

fn pending(
    runs: &SessionRuns,
    session: Option<&str>,
    request: Option<&str>,
    questions: bool,
) -> Result<(AskKey, Ask), Refusal> {
    let id = super::turns::session_id(session)?;
    let request = request.ok_or_else(gone)?;
    let pending = runs.asks().pending();
    let current = pending.borrow();
    current
        .iter()
        .find(|(key, ask)| {
            key.session == id
                && matches!(ask, Ask::Question(_)) == questions
                && key.client_id(questions) == request
        })
        .map(|(key, ask)| (key.clone(), ask.clone()))
        .ok_or_else(gone)
}

fn field(field: &Field) -> Value {
    let (kind, options, custom) = match &field.kind {
        FieldKind::String {
            options, custom, ..
        } => ("string", Some(options), *custom),
        FieldKind::MultiSelect { options, custom } => ("multiselect", Some(options), *custom),
        FieldKind::Boolean => ("boolean", None, false),
        FieldKind::External { .. } => ("external", None, false),
    };
    let mut object = json!({"key":field.key,"type":kind,"required":field.required});
    if let Some(title) = &field.title {
        object["title"] = json!(title);
    }
    if let Some(description) = &field.description {
        object["description"] = json!(description);
    }
    if let Some(options) = options.filter(|options| !options.is_empty()) {
        object["options"] = json!(
            options
                .iter()
                .map(|option| {
                    let mut item = json!({"value":option.value,"label":option.label});
                    if let Some(description) = &option.description {
                        item["description"] = json!(description);
                    }
                    item
                })
                .collect::<Vec<_>>()
        );
    }
    if custom {
        object["custom"] = true.into();
    }
    if let FieldKind::String {
        format: Some(format),
        ..
    } = &field.kind
    {
        object["format"] = json!(format);
    }
    if let FieldKind::External { url: Some(url) } = &field.kind {
        object["url"] = json!(url);
    }
    object
}

fn refuse(error: io::Error) -> Refusal {
    match error.kind() {
        io::ErrorKind::NotFound => gone(),
        io::ErrorKind::PermissionDenied => Refusal::new(
            StatusCode::FORBIDDEN,
            "This computer’s permissions no longer allow that action.",
        ),
        _ => super::refuse(error),
    }
}

fn gone() -> Refusal {
    Refusal::new(StatusCode::NOT_FOUND, "That request was answered already.")
}
