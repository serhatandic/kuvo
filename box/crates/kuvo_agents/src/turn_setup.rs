//! A turn's input and permission policy, read from the conversation's header.

use crate::{
    permissions::{Policy, Rule},
    session::{ModelRef, PermissionMode, TurnInput, TurnKind},
};
use serde_json::{Map, Value};
use std::{
    io::{self, ErrorKind},
    path::{Path, PathBuf},
};

/// The turn `turn` of the conversation whose saved header is `header`, run
/// in `folder`.
pub fn turn_setup(
    _home: &Path,
    folder: PathBuf,
    header: &Map<String, Value>,
    turn: &str,
    kind: TurnKind,
) -> io::Result<(TurnInput, Policy)> {
    let model = header.get("model").unwrap_or(&Value::Null);
    let read = |key| {
        model[key]
            .as_str()
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
    };
    let bad_model = || {
        io::Error::new(
            ErrorKind::InvalidInput,
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
    let permissions = if applied == Some("fullAccess") {
        PermissionMode::ApproveAll
    } else {
        PermissionMode::AskFirst
    };
    // Plan mode is gone: a conversation stored with agent "plan" (its `agent-switched` rows still
    // read) runs as Build, as every other conversation does.
    let plan = false;
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
    policy.auto_without_sandbox = applied == Some("auto");
    Ok((
        TurnInput {
            id: turn.to_owned(),
            kind,
            model: selected,
            instructions: String::new(),
            permissions,
            plan,
            auto: false,
            start_cancel: Default::default(),
        },
        policy,
    ))
}
