//! Compatibility with Git preferences held by pre-0.8 Macs and boxes.
use crate::config::{
    ConfigPaths, SettingChange, SettingsChanged, SettingsRead, UNOWNED_SETTINGS,
    write_settings_file,
};
use http::StatusCode;
use kuvo_protocol::routes::post_kuvo_git_settings_move;
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use kuvo_store::{
    files,
    format::{Error, ObjectFormat},
    locks::OwnedLock,
};
use serde_json::{Map, Value, json};
use std::{path::Path, time::Duration};

const FORMAT: ObjectFormat = ObjectFormat::additive("Git preferences migration record");
const KEYS: [(&str, &str); 3] = [
    ("branchPrefix", "branch_prefix"),
    ("commitInstructions", "commit_instructions"),
    ("pullRequestInstructions", "pull_request_instructions"),
];

fn failed(error: impl std::fmt::Display) -> Refusal {
    Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}
fn read(path: &Path) -> Result<Map<String, Value>, Refusal> {
    match FORMAT.read(path) {
        Ok(Some(record)) => Ok(record.fields),
        Err(error @ Error::Unsupported { .. }) => Err(failed(error)),
        _ => Ok(Map::new()),
    }
}
fn save(path: &Path, fields: Map<String, Value>) -> Result<(), Refusal> {
    files::write_secret(path, FORMAT.encode(fields).to_string().as_bytes()).map_err(failed)
}
fn settings(paths: &ConfigPaths) -> Result<Map<String, Value>, Refusal> {
    match paths.read_settings() {
        SettingsRead::Ok(settings) => Ok(settings),
        SettingsRead::Problem(problem) => Err(Refusal::new(
            StatusCode::CONFLICT,
            format!("The engine's settings file can't be changed from here: {problem}"),
        )),
    }
}
fn change(key: &str, value: Value) -> SettingChange {
    SettingChange {
        path: vec![key.to_owned()],
        value,
        was: None,
    }
}
fn write(
    paths: &ConfigPaths,
    current: &Map<String, Value>,
    changes: &[SettingChange],
) -> Result<(), Refusal> {
    match write_settings_file(current, changes, &paths.config_file, &paths.keys_named) {
        SettingsChanged::Ok(_) => Ok(()),
        SettingsChanged::Err {
            status,
            message,
            problem,
        } => Err(SettingsChanged::into_refusal(status, message, problem)),
    }
}
fn take(paths: &ConfigPaths, path: &Path) -> Result<(), Refusal> {
    let mut kept = read(path)?;
    let Some(prefix) = kept.remove("branchPrefix") else {
        return Ok(());
    };
    let current = settings(paths)?;
    if prefix.is_string() && !current.contains_key("branch_prefix") {
        write(paths, &current, &[change("branch_prefix", prefix)])?;
    }
    save(path, kept)
}
pub fn keep_old_branch_prefix(paths: &ConfigPaths) {
    if !paths.owns_settings() {
        return;
    }
    let path = paths.home.join("box/git.json");
    if !path.exists() {
        return;
    }
    if let Ok(_guard) = OwnedLock::acquire(&paths.lock_file, Duration::from_secs(15)) {
        let _ = take(paths, &path);
    }
}
fn move_given(paths: &ConfigPaths, given: Value) -> Result<bool, Refusal> {
    let obj = given.as_object().ok_or_else(|| {
        Refusal::new(
            StatusCode::BAD_REQUEST,
            "Send {branchPrefix?, commitInstructions?, pullRequestInstructions?}.",
        )
    })?;
    let mut offered = Vec::new();
    for (key, target) in KEYS {
        let Some(value) = obj.get(key).filter(|value| !value.is_null()) else {
            continue;
        };
        let text = value
            .as_str()
            .ok_or_else(|| Refusal::new(StatusCode::BAD_REQUEST, "Each is text."))?;
        let text = if key == "branchPrefix" {
            text
        } else {
            text.trim()
        };
        if !text.is_empty() || key == "branchPrefix" {
            offered.push(change(target, json!(text)));
        }
    }
    if !paths.owns_settings() {
        return Err(Refusal::new(StatusCode::CONFLICT, UNOWNED_SETTINGS));
    }
    let _guard = OwnedLock::acquire(&paths.lock_file, Duration::from_secs(15)).map_err(failed)?;
    let current = settings(paths)?;
    let path = paths.home.join("box/git.json");
    let mut kept = read(&path)?;
    let moved = kept.get("moved") == Some(&Value::Bool(true));
    let real = !offered.is_empty();
    let changes: Vec<_> = offered
        .into_iter()
        .filter(|change| !moved && !current.contains_key(&change.path[0]))
        .collect();
    if !changes.is_empty() {
        write(paths, &current, &changes)?;
    }
    if !moved && real {
        kept.insert("moved".into(), json!(true));
        save(&path, kept)?;
    }
    take(paths, &path)?;
    Ok(!changes.is_empty())
}
pub(crate) fn register(routes: &mut Registry, paths: ConfigPaths) {
    keep_old_branch_prefix(&paths);
    routes.register::<post_kuvo_git_settings_move::Spec, _, _>(Access::Any, move |call| {
        let paths = paths.clone();
        let given = serde_json::from_slice(&call.request.body).unwrap_or(Value::Null);
        async move {
            let moved = tokio::task::spawn_blocking(move || move_given(&paths, given))
                .await
                .map_err(failed)??;
            Ok(RouteReply::json_as_kept::<200>(json!({"moved":moved})))
        }
    });
}
