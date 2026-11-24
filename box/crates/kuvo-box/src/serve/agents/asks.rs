//! The native requests a coding agent is waiting for this resident to answer.

use http::StatusCode;
use kuvo_agents::{
    live_asks::{Ask, AskKey, Reply},
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
use std::io;

pub(crate) fn register(runs: SessionRuns, store: SessionStore, routes: &mut Registry) {
    let ask_runs = runs.clone();
    let ask_store = store.clone();
    routes.register::<permission::Spec, _, _>(Access::Any, move |call| {
        let runs = ask_runs.clone();
        let store = ask_store.clone();
        async move {
            let id = read_session(&store, call.param("id")).await?;
            let cards = cards(&runs, &id, false);
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
            runs.asks()
                .reply(key, Reply::Question(Answer::Dismissed { reason: None }))
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

fn cards(runs: &SessionRuns, session: &str, questions: bool) -> Vec<Value> {
    let pending = runs.asks().pending();
    let current = pending.borrow();
    current
        .iter()
        .filter(|(key, _)| key.session == session)
        .filter_map(|(key, ask)| {
            let id = key.client_id(questions);
            match ask {
                Ask::Permission(ask) if !questions => {
                    let mut card = json!({
                        "id":id,"sessionID":session,"action":ask.action,"resources":ask.resources,
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
                    Some(card)
                }
                Ask::Question(ask) if questions => {
                    let mut context = Map::<String, Value>::new();
                    context.insert("kind".into(), "question".into());
                    context.extend(ask.metadata.clone());
                    if let Some(tool) = &ask.tool_call_id {
                        context.insert("tool".into(), json!({"id":tool}));
                    }
                    Some(json!({
                        "id":id,"sessionID":session,"title":ask.title,"metadata":context,
                        "fields":ask.fields.iter().map(field).collect::<Vec<_>>()
                    }))
                }
                _ => None,
            }
        })
        .collect()
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
