//! The commands agents left running (the bars over the composer): what a coding agent's printed
//! (`kuvo/agents/api/shell/<id>/output`), and Stop on one, the engine's or a coding agent's
//! (`kuvo/shell/<id>/stop`).

use http::StatusCode;
use kuvo_agents::background::Background;
use kuvo_engine::Engine;
use kuvo_protocol::routes::{
    get_kuvo_agents_api_shell_id_output as output, post_kuvo_shell_id_stop as stop,
};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use kuvo_work::work::watch::{EngineCommand, stop_engine_command};
use percent_encoding::percent_decode_str;
use serde_json::{Value, json};

fn command_of<T>(call: &kuvo_server::RouteCall<T>) -> Result<String, Refusal> {
    let given = call.param("id").unwrap_or_default();
    percent_decode_str(given)
        .decode_utf8()
        .map(|id| id.into_owned())
        .map_err(|_| Refusal::new(StatusCode::NOT_FOUND, "No such command."))
}

/// A query's value, as `URLSearchParams.get` reads it.
fn query<'a>(path: &'a str, name: &str) -> Option<std::borrow::Cow<'a, str>> {
    let (_, query) = path.split_once('?')?;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        (key == name).then(|| percent_decode_str(value).decode_utf8_lossy())
    })
}

fn typed<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, Refusal> {
    serde_json::from_value(value)
        .map_err(|error| Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
}

pub(crate) fn register(commands: Background, engine: Engine, routes: &mut Registry) {
    let printed = commands.clone();
    routes.register::<output::Spec, _, _>(Access::Any, move |call| {
        let commands = printed.clone();
        async move {
            let id = command_of(&call)?;
            let path = &call.request.path;
            let cursor = query(path, "cursor")
                .and_then(|cursor| cursor.parse().ok())
                .unwrap_or(0);
            let limit = query(path, "limit").and_then(|limit| limit.parse().ok());
            let read = commands
                .output(&id, cursor, limit)
                .await
                .ok_or_else(|| Refusal::new(StatusCode::NOT_FOUND, "No such command."))?;
            let directory = std::env::current_dir().unwrap_or_default();
            Ok(RouteReply::json::<200>(typed(json!({
                "location": { "directory": directory.display().to_string() },
                "data": { "output": read.output, "cursor": read.cursor, "size": read.size },
            }))?))
        }
    });
    routes.register::<stop::Spec, _, _>(Access::Any, move |call| {
        let (commands, engine) = (commands.clone(), engine.clone());
        async move {
            let id = command_of(&call)?;
            let folder = call
                .request
                .headers
                .get("x-opencode-directory")
                .and_then(|value| value.to_str().ok())
                .map(|value| percent_decode_str(value).decode_utf8_lossy().into_owned());
            let stopped = |how: &str| typed(json!({ "stopped": how })).map(RouteReply::json::<200>);
            let gone = || Err(Refusal::new(StatusCode::NOT_FOUND, ""));
            // The engine's, else a coding agent's (which runs here, so always answers).
            if let Some(running) = engine.running_bot_engine().await {
                match stop_engine_command(&running, &id, folder.as_deref().unwrap_or_default())
                    .await
                {
                    EngineCommand::Unknown => {}
                    EngineCommand::Ended => return gone(),
                    EngineCommand::Stopped(how) => return stopped(how),
                    EngineCommand::Stuck => {
                        return Err(Refusal::new(
                            StatusCode::UNPROCESSABLE_ENTITY,
                            "It’s still running: neither its signal nor removing it went through.",
                        ));
                    }
                }
            }
            // (A coding agent's command has no process of its own to signal: its agent stops it.)
            match commands.command(&id) {
                Some(command) if command.running && commands.stop(&id).await => stopped("removed"),
                _ => gone(),
            }
        }
    });
}
