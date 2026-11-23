//! Resident-owned actions on engine sessions.

use http::{Method, StatusCode};
use kuvo_protocol::routes::{
    delete_kuvo_sessions_id as delete, get_kuvo_sessions_id_actions as actions,
    post_kuvo_sessions_id_archive as archive, post_kuvo_sessions_id_pin as pin,
    post_kuvo_sessions_id_rename as rename,
};
use kuvo_server::{Access, Refusal, Registry, RouteCall, RouteReply};
use percent_encoding::percent_decode_str;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{Engine, api, lifetime::Lease};

#[derive(Deserialize)]
pub(crate) struct Session {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(rename = "parentID")]
    pub parent: Option<String>,
    #[serde(default)]
    pub time: Times,
    #[serde(default)]
    pub metadata: Map<String, Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Default, Deserialize)]
pub(crate) struct Times {
    pub created: Option<u64>,
    pub updated: Option<u64>,
    pub activity: Option<u64>,
    pub archived: Option<u64>,
    pub idle: Option<u64>,
    pub viewed: Option<u64>,
}

impl Session {
    pub(crate) fn archived_by_engine(&self) -> bool {
        self.time.archived.is_some_and(|when| when != 0)
    }

    pub(crate) fn archived(&self) -> bool {
        self.archived_by_engine()
            || self.metadata.get("kuvo.archived") == Some(&Value::Bool(true))
            || self.notes().and_then(|notes| notes.get("archived")) == Some(&Value::Bool(true))
    }

    pub(crate) fn pinned(&self) -> bool {
        self.metadata.get("kuvo.pinned") == Some(&Value::Bool(true))
    }

    pub(crate) fn notes(&self) -> Option<&Map<String, Value>> {
        self.metadata.get("kuvo")?.as_object()
    }

    pub(crate) fn directory(&self) -> &str {
        self.extra
            .get("location")
            .and_then(|location| location["directory"].as_str())
            .or_else(|| self.extra.get("directory").and_then(Value::as_str))
            .unwrap_or_default()
    }

    fn check_independent(&self) -> Result<(), Refusal> {
        let marked = |key| {
            self.notes()
                .and_then(|notes| notes.get(key))
                .is_some_and(|value| !value.is_null())
        };
        let why = if self.parent.as_ref().is_some_and(|id| !id.is_empty()) {
            "It’s a subagent’s, part of the conversation that started it: it goes with that one."
        } else if marked("seat") {
            "It’s a bot’s seat in a group chat: change the group chat instead."
        } else if marked("engineFor") || marked("reviewPart") {
            "It’s part of another conversation’s work: it goes with that one."
        } else {
            return Ok(());
        };
        Err(Refusal::new(StatusCode::CONFLICT, why))
    }
}

pub(crate) fn id<T>(call: &RouteCall<T>) -> Result<String, Refusal> {
    let value = percent_decode_str(call.param("id").unwrap_or_default()).decode_utf8();
    match value {
        Ok(value) if valid_id(&value) => Ok(value.into_owned()),
        _ => Err(Refusal::new(
            StatusCode::BAD_REQUEST,
            "That isn't a conversation's id.",
        )),
    }
}

pub(crate) fn valid_id(value: &str) -> bool {
    value.strip_prefix("ses").is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
    })
}

pub(crate) fn path(id: &str) -> String {
    format!("api/session/{}", api::segment(id))
}

pub(crate) fn body<T>(call: &RouteCall<T>) -> Result<Map<String, Value>, Refusal> {
    serde_json::from_slice(&call.request.body)
        .map_err(|_| Refusal::new(StatusCode::BAD_REQUEST, "Send what changes as JSON."))
}

pub(crate) fn typed<T: serde::de::DeserializeOwned>(answer: Value) -> Result<T, Refusal> {
    serde_json::from_value(answer)
        .map_err(|error| Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
}

pub(crate) async fn read(engine: &Engine, lease: &Lease, path: &str) -> Result<Value, Refusal> {
    engine
        .call(lease, Method::GET, path, None)
        .await
        .map_err(|error| {
            if error.missing() {
                Refusal::new(StatusCode::NOT_FOUND, "That conversation isn't here.")
            } else {
                Refusal::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "The engine isn't answering. Try again in a moment.",
                )
            }
        })
}

pub(crate) fn register(engine: &Engine, routes: &mut Registry) {
    let local = engine.clone();
    routes.register::<actions::Spec, _, _>(Access::Any, move |call| {
        let engine = local.clone();
        async move {
            let id = id(&call)?;
            let lease = api::start(&engine).await?;
            let session: Session = typed(read(&engine, &lease, &path(&id)).await?)?;
            session.check_independent()?;
            let mut menu = Vec::new();
            if session.archived() {
                if !session.archived_by_engine() {
                    menu.push(json!({"id":"unarchive","label":"Unarchive"}));
                }
            } else {
                menu.push(if session.pinned() { json!({"id":"unpin","label":"Unpin"}) } else { json!({"id":"pin","label":"Pin"}) });
                menu.push(json!({"id":"rename","label":"Rename"}));
                menu.push(json!({"id":"archive","label":"Archive"}));
            }
            menu.push(json!({"id":"delete","label":"Delete","destructive":true,"confirm":confirmation(&session)}));
            Ok(RouteReply::json::<200>(typed(json!({"session":id,"actions":menu}))?))
        }
    });
    let local = engine.clone();
    routes.register::<rename::Spec, _, _>(Access::Any, move |call| {
        let local = local.clone();
        async move { change(local, call, Change::Rename).await }
    });
    let local = engine.clone();
    routes.register::<archive::Spec, _, _>(Access::Any, move |call| {
        let local = local.clone();
        async move { change(local, call, Change::Archive).await }
    });
    let local = engine.clone();
    routes.register::<pin::Spec, _, _>(Access::Any, move |call| {
        let local = local.clone();
        async move { change(local, call, Change::Pin).await }
    });
    let local = engine.clone();
    routes.register::<delete::Spec, _, _>(Access::Any, move |call| {
        let engine = local.clone();
        async move {
            let id = id(&call)?;
            let given = body(&call).unwrap_or_default();
            let _turn = engine.turn(&id).await;
            let lease = api::start(&engine).await?;
            let path = path(&id);
            match engine
                .call::<Session>(&lease, Method::GET, &path, None)
                .await
            {
                Ok(session) => {
                    session.check_independent()?;
                    check_was(&given, &session, true)?;
                    engine
                        .call::<Value>(&lease, Method::DELETE, &path, None)
                        .await
                        .map_err(|error| {
                            Refusal::new(
                                StatusCode::BAD_GATEWAY,
                                format!("It couldn't be deleted: {error}"),
                            )
                        })?;
                    if let Some(folder) = wakes(&engine, &id) {
                        let _ = tokio::fs::remove_dir_all(folder).await;
                    }
                }
                Err(error) if error.missing() && !given.contains_key("was") => {}
                Err(error) if error.missing() => return Err(stale()),
                Err(_) => {
                    return Err(Refusal::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "The engine isn't answering. Try again in a moment.",
                    ));
                }
            }
            Ok(RouteReply::json::<200>(typed(json!({ "session": id }))?))
        }
    });
}

fn confirmation(session: &Session) -> Value {
    let title = if session.title.is_empty() {
        "New Session"
    } else {
        &session.title
    };
    json!({ "title": format!("Delete “{title}”?"), "message": "This can’t be undone.", "label": "Delete" })
}

fn stale() -> Refusal {
    Refusal::new(StatusCode::CONFLICT, "It changed since you read it.")
}

fn check_was(given: &Map<String, Value>, session: &Session, delete: bool) -> Result<(), Refusal> {
    let Some(was) = given.get("was") else {
        return Ok(());
    };
    let mut expected = json!({ "archived": session.archived() });
    if let Some(updated) = session.time.updated {
        expected["updated"] = json!(updated);
    }
    if delete {
        expected["confirm"] = confirmation(session);
        if *was == expected {
            return Ok(());
        }
    } else if was.get("updated") == expected.get("updated")
        && was.get("archived") == expected.get("archived")
    {
        return Ok(());
    }
    Err(stale())
}

#[derive(Clone, Copy)]
enum Change {
    Rename,
    Archive,
    Pin,
}

async fn change<R: kuvo_protocol::JsonStatus<200>>(
    engine: Engine,
    call: RouteCall<<R as kuvo_protocol::RouteSpec>::Body>,
    change: Change,
) -> Result<RouteReply<R>, Refusal>
where
    <R as kuvo_protocol::JsonStatus<200>>::Body: serde::de::DeserializeOwned,
{
    let id = id(&call)?;
    let given = body(&call)?;
    let field = match change {
        Change::Rename => "title",
        Change::Archive => "archived",
        Change::Pin => "pinned",
    };
    let value = given
        .get(field)
        .and_then(|value| match change {
            Change::Rename => value
                .as_str()
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .map(|title| json!(title)),
            Change::Archive | Change::Pin => value.as_bool().map(|flag| json!(flag)),
        })
        .ok_or_else(|| {
            Refusal::new(
                StatusCode::BAD_REQUEST,
                match change {
                    Change::Rename => "Give it a name.",
                    Change::Archive => "Say whether it's archived: {\"archived\": true} or false.",
                    Change::Pin => "Say whether it's pinned: {\"pinned\": true} or false.",
                },
            )
        })?;

    let _turn = engine.turn(&id).await;
    let lease = api::start(&engine).await?;
    let path = path(&id);
    let mut info = read(&engine, &lease, &path).await?;
    let mut session: Session = typed(info.clone())?;
    session.check_independent()?;

    let patch = match change {
        Change::Rename => (session.title != value).then(|| json!({ "title": value })),
        Change::Archive | Change::Pin => {
            let archive = matches!(change, Change::Archive);
            if archive && value == false && session.archived_by_engine() {
                return Err(Refusal::new(
                    StatusCode::CONFLICT,
                    "It was archived by the engine, which keeps it so. Start a new session to carry on.",
                ));
            }
            check_was(&given, &session, false)?;
            let previous = if archive {
                session.archived()
            } else {
                session.pinned()
            };
            if previous == value {
                None
            } else {
                if archive
                    && value == false
                    && let Some(notes) = session
                        .metadata
                        .get_mut("kuvo")
                        .and_then(Value::as_object_mut)
                {
                    notes.remove("archived");
                }
                session
                    .metadata
                    .insert(format!("kuvo.{field}"), value.clone());
                Some(json!({ "metadata": session.metadata }))
            }
        }
    };
    if let Some(patch) = patch {
        let updated = engine
            .call::<Value>(&lease, Method::PATCH, &path, Some(&patch))
            .await
            .map_err(|error| {
                let action = if matches!(change, Change::Rename) {
                    "renamed"
                } else {
                    "changed"
                };
                Refusal::new(
                    StatusCode::BAD_GATEWAY,
                    format!("It couldn't be {action}: {error}"),
                )
            })?;
        if updated.is_object() {
            info = updated;
        } else if let Some(patch) = patch.as_object() {
            for (key, value) in patch {
                info[key] = value.clone();
            }
        }
        if matches!(change, Change::Archive) {
            after_archive(&engine, &lease, &id, value == true).await;
        }
    }
    Ok(RouteReply::json::<200>(typed(
        json!({ "session": id, "info": info }),
    )?))
}

fn wakes(engine: &Engine, id: &str) -> Option<std::path::PathBuf> {
    id.strip_prefix("ses_")
        .filter(|suffix| !suffix.is_empty() && suffix.bytes().all(|c| c.is_ascii_alphanumeric()))?;
    Some(engine.home.join("wakes").join(id))
}

pub(crate) async fn after_archive(engine: &Engine, lease: &Lease, id: &str, archived: bool) {
    hold_wakes(engine, id, archived).await;
    if archived {
        crate::asks::stop_waiting(engine, lease, id).await;
    }
}

async fn hold_wakes(engine: &Engine, id: &str, archived: bool) {
    if let Some(folder) = wakes(engine, id) {
        let marker = folder.join("paused");
        let held = tokio::fs::read_to_string(&marker).await;
        if archived {
            if matches!(held.as_deref(), Ok("moving"))
                || held.is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            {
                let _ = tokio::fs::write(marker, "archived").await;
            }
        } else if matches!(held.as_deref(), Ok("archived" | "")) {
            let _ = tokio::fs::remove_file(marker).await;
        }
    }
}
