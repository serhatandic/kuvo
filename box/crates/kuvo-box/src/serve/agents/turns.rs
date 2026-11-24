//! Resident entry points for adapter turns.

use http::StatusCode;
use kuvo_agents::{
    permissions::{Policy, Rule},
    runtime::AdoptedTurn,
    session::{ModelRef, PermissionMode, Prompt, PromptFile, TurnInput, TurnKind},
    session_runs::SessionRuns,
    session_store::SessionStore,
};
use kuvo_protocol::routes::{
    post_kuvo_agents_api_session_id_interrupt as interrupt,
    post_kuvo_agents_api_session_id_prompt as prompt,
};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use serde_json::{Map, Value};
use std::{io, path::PathBuf};

pub(crate) async fn restore(
    runs: &SessionRuns,
    store: &SessionStore,
    home: &std::path::Path,
    adopted: &[AdoptedTurn],
) {
    let mut restoring = tokio::task::JoinSet::new();
    for recovered in adopted {
        let Some(turn) = recovered.note.turn.clone() else {
            continue;
        };
        let runs = runs.clone();
        let store = store.clone();
        let home = home.to_owned();
        let recovered = recovered.clone();
        restoring.spawn(async move {
            let id = recovered.note.session.clone();
            let result = async {
                let Some(session) = store.read(&id).await.map_err(super::refuse)? else {
                    return Ok(());
                };
                let Some(directory) = session.directory().map(PathBuf::from) else {
                    return Ok(());
                };
                let (mut input, policy) = turn_setup(
                    &home,
                    directory,
                    &session.into_fields(),
                    &turn,
                    TurnKind::Recover {
                        note: recovered.note,
                    },
                )?;
                input.model.provider = recovered.provider.to_string();
                let run = runs
                    .run_turn(id.clone(), input, policy)
                    .await
                    .map_err(turn_error)?;
                tokio::time::timeout(std::time::Duration::from_secs(10), run.ready())
                    .await
                    .map_err(|error| turn_error(io::Error::other(error)))?
                    .map_err(turn_error)
            }
            .await;
            if let Err(error) = result {
                eprintln!(
                    "Kuvo could not replay coding-agent turn {id}: {}",
                    error.reply().status
                );
            }
        });
    }
    while restoring.join_next().await.is_some() {}
}

pub(crate) fn register(
    runs: SessionRuns,
    store: SessionStore,
    home: PathBuf,
    routes: &mut Registry,
) {
    let as_turns = runs.clone();
    routes.register::<prompt::Spec, _, _>(Access::DesktopOrPaired, move |call| {
        let runs = as_turns.clone();
        let store = store.clone();
        let home = home.clone();
        async move {
            use kuvo_agents::session::Steer;
            let id = session_id(call.param("id"))?;
            let stored = store
                .read(&id)
                .await
                .map_err(super::refuse)?
                .ok_or_else(|| super::sessions::missing(&id))?;
            let folder = stored.directory().map(PathBuf::from).ok_or_else(|| {
                Refusal::new(
                    StatusCode::BAD_REQUEST,
                    "Give the conversation’s working directory.",
                )
            })?;
            let header = stored.into_fields();
            let message = read_prompt(&call.body)?;
            if store
                .has_message(&id, &message.id)
                .await
                .map_err(super::refuse)?
            {
                return Ok(RouteReply::json::<200>(prompt::Status200 {}));
            }
            if runs.running().contains(&id) && call.body["delivery"] != "queue" {
                match runs
                    .steer(id.clone(), message.clone())
                    .await
                    .map_err(super::refuse)?
                {
                    Steer::Accepted | Steer::InDoubt(_) => {
                        return Ok(RouteReply::json::<200>(prompt::Status200 {}));
                    }
                    Steer::Declined => {}
                }
            }
            let (input, policy) = turn_setup(
                &home,
                folder,
                &header,
                &message.id,
                TurnKind::Prompt(message.clone()),
            )?;
            runs.run_turn(id, input, policy).await.map_err(turn_error)?;
            Ok(RouteReply::json::<200>(prompt::Status200 {}))
        }
    });
    routes.register::<interrupt::Spec, _, _>(Access::DesktopOrPaired, move |call| {
        let runs = runs.clone();
        async move {
            let id = session_id(call.param("id"))?;
            let interrupted = runs.interrupt(id).await.map_err(turn_error)?;
            Ok(RouteReply::json::<200>(interrupt::Status200 {
                interrupted,
            }))
        }
    });
}

pub(super) fn turn_setup(
    home: &std::path::Path,
    folder: PathBuf,
    header: &Map<String, Value>,
    turn: &str,
    kind: TurnKind,
) -> Result<(TurnInput, Policy), Refusal> {
    let model = header.get("model").unwrap_or(&Value::Null);
    let read = |key| {
        model[key]
            .as_str()
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
    };
    let bad_model = || {
        Refusal::new(
            StatusCode::BAD_REQUEST,
            "Pick a model of an agent you've connected.",
        )
    };
    let selected = ModelRef {
        provider: read("providerID").ok_or_else(bad_model)?,
        id: read("id").ok_or_else(bad_model)?,
        variant: read("variant"),
    };
    let settings = header
        .get("metadata")
        .and_then(|meta| meta.get("kuvo"))
        .unwrap_or(&Value::Null);
    let applied = settings["applied"].as_str();
    let permissions = if matches!(applied, Some("auto" | "fullAccess")) {
        PermissionMode::ApproveAll
    } else {
        PermissionMode::AskFirst
    };
    let plan = header.get("agent").and_then(Value::as_str) == Some("plan");
    let mut policy = Policy::new(folder, None);
    policy.person_home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    policy.session_rules = header
        .get("permissions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|rule| serde_json::from_value::<Rule>(rule.clone()).ok())
        .collect();
    policy.auto_without_sandbox = applied == Some("auto") && !home.join("run/fence/on").exists();
    Ok((
        TurnInput {
            id: turn.to_owned(),
            kind,
            model: selected,
            instructions: String::new(),
            permissions,
            plan,
            auto: false,
        },
        policy,
    ))
}

fn read_prompt(body: &Value) -> Result<Prompt, Refusal> {
    let text = body["text"].as_str().unwrap_or_default().to_owned();
    let files = body["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|file| {
            Some(PromptFile {
                uri: file["uri"].as_str().unwrap_or_default().to_owned(),
                mime: file["mime"].as_str()?.to_owned(),
                name: file["name"].as_str().map(str::to_owned),
                path: file["path"].as_str().map(PathBuf::from),
                data: file["data"].as_str().map(str::to_owned),
            })
        })
        .collect::<Vec<_>>();
    if text.trim().is_empty() && files.is_empty() {
        return Err(Refusal::new(
            StatusCode::BAD_REQUEST,
            "Give a message or an attachment.",
        ));
    }
    let id = match body["id"].as_str() {
        Some(id)
            if !id.is_empty()
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')) =>
        {
            id.to_owned()
        }
        Some(_) => {
            return Err(Refusal::new(
                StatusCode::BAD_REQUEST,
                "Give the message’s ID.",
            ));
        }
        None => SessionStore::make_id("msg").map_err(super::refuse)?,
    };
    Ok(Prompt {
        id,
        text,
        files,
        synthetic: false,
    })
}

pub(super) fn session_id(raw: Option<&str>) -> Result<String, Refusal> {
    let raw = raw.unwrap_or_default();
    percent_encoding::percent_decode_str(raw)
        .decode_utf8()
        .map(|decoded| decoded.into_owned())
        .map_err(|_| super::sessions::missing(raw))
}

fn turn_error(error: io::Error) -> Refusal {
    if error.kind() == io::ErrorKind::AlreadyExists {
        Refusal::new(StatusCode::CONFLICT, error.to_string())
    } else {
        super::refuse(error)
    }
}
