//! This box's bots, listed (`kuvo/bots`): each by its conversation, with its name, look, job,
//! personality and model as its description has them, and what its folder says now (paused, its
//! next wake-up, how many things it watches, whether it runs with the coding agents).

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value, json};

use super::{here::BotHere, job::job_of};
use crate::wakes::folder::{self, Detail, names};

/// One line of what deleting a bot takes with it (`bots.deletes`).
#[derive(Clone, Debug, Serialize)]
pub struct Deletes {
    pub what: &'static str,
    pub words: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kept: Option<bool>,
}

/// Where a bot deleted here ends up: Kuvo Code's own trash folder on this computer, as it's
/// written for you (the Mac app's Trash hand-off is the retired app's).
pub struct FolderGoes {
    pub folder: Option<String>,
}

impl FolderGoes {
    pub fn of(home: &Path, account: &Path) -> Self {
        let trash = home.join("trash");
        let shown = match trash.strip_prefix(account) {
            Ok(inside) if !account.as_os_str().is_empty() && inside != Path::new("") => {
                format!("~{}{}", std::path::MAIN_SEPARATOR, inside.display())
            }
            _ => trash.display().to_string(),
        };
        Self {
            folder: Some(shown),
        }
    }
}

fn counted(count: usize, one: &str, many: &str) -> String {
    if count == 1 {
        one.to_owned()
    } else {
        format!("{count} {many}")
    }
}

/// What deleting a bot takes with it, in the order deleting does it, then what stays.
pub fn what_delete_takes(
    computer: &str,
    goes: &FolderGoes,
    wakes: usize,
    watching: usize,
) -> Vec<Deletes> {
    let line = |what, words: String| Deletes {
        what,
        words,
        kept: None,
    };
    let mut lines = vec![line("conversation", "Its conversation is deleted.".into())];
    let place = goes
        .folder
        .as_ref()
        .map(|folder| format!(" ({folder})"))
        .unwrap_or_default();
    lines.push(line(
        "folder",
        format!("Its folder, with its memory and what it made there, goes to Kuvo Code’s trash folder on {computer}{place}."),
    ));
    if wakes > 0 || watching > 0 {
        let mut parts = Vec::new();
        if wakes > 0 {
            parts.push(counted(wakes, "wake-up", "wake-ups"));
        }
        if watching > 0 {
            parts.push(counted(watching, "watch", "watches"));
        }
        let verb = if wakes + watching == 1 { "is" } else { "are" };
        lines.push(line(
            "wakes",
            format!("Its {} {verb} cancelled.", parts.join(" and ")),
        ));
    }
    lines.push(line(
        "groups",
        format!("It leaves the group chats on {computer}."),
    ));
    lines.push(Deletes {
        what: "sessions",
        words: "The sessions it started stay.".into(),
        kept: Some(true),
    });
    lines
}

/// The limits you set for a bot's work, each a whole number of at least one; none for none.
pub fn limits_from(given: Option<&Value>) -> Option<Map<String, Value>> {
    let given = given?.as_object()?;
    let limits: Map<String, Value> = ["atOnce", "perDay", "depth", "tokensPerDay"]
        .into_iter()
        .filter_map(|name| {
            let value = given.get(name)?;
            let whole = value.as_f64().filter(|n| n.fract() == 0.0 && *n >= 1.0)?;
            Some((name.to_owned(), json!(whole as u64)))
        })
        .collect();
    (!limits.is_empty()).then_some(limits)
}

/// What you allowed a bot to do without asking ("Always").
pub fn allowed_of(described: &Map<String, Value>) -> Vec<&'static str> {
    let allowed = described
        .get("work")
        .and_then(|work| work.get("allowed"))
        .and_then(Value::as_array);
    ["makeProject", "cloneProject"]
        .into_iter()
        .filter(|action| allowed.is_some_and(|allowed| allowed.iter().any(|each| each == action)))
        .collect()
}

fn is_model(value: &Value) -> bool {
    value.get("id").is_some_and(Value::is_string)
        && value.get("providerID").is_some_and(Value::is_string)
}

/// A bot as `kuvo/bots` lists it, without what it's doing now.
pub struct Listed {
    pub here: BotHere,
    pub created: String,
    pub value: Map<String, Value>,
    pub paused: bool,
    pub wakes: Option<f64>,
}

/// Where listing reads from, and how this computer is named.
pub struct Place<'a> {
    pub bots: &'a Path,
    /// This machine's name in each bot (none on a Mac).
    pub machine: Option<&'a str>,
    /// This computer in a sentence ("lab", "this Mac").
    pub computer: &'a str,
    pub goes: &'a FolderGoes,
}

/// The bots that live here, the first made first.
pub fn list(place: &Place<'_>) -> Vec<Listed> {
    let mut bots: Vec<Listed> = names(place.bots)
        .into_iter()
        .filter_map(|id| BotHere::read(place.bots, &id))
        .map(|here| listed(place, here))
        .collect();
    bots.sort_by(|a, b| a.created.cmp(&b.created));
    bots
}

pub fn listed(place: &Place<'_>, here: BotHere) -> Listed {
    let listing = folder::read(&here.folder, Detail::Line);
    let job = job_of(&here, true);
    let described = &here.described;
    let wakes = listing.wakes.iter().map(|wake| wake.due).reduce(f64::min);
    let mut value = Map::new();
    value.insert("id".into(), json!(here.id));
    value.insert("session".into(), json!(here.session));
    value.insert("name".into(), json!(here.name()));
    if let Some(look) = described.get("look") {
        value.insert("look".into(), look.clone());
    }
    if let Some(machine) = place.machine {
        value.insert("machine".into(), json!(machine));
    }
    value.insert("job".into(), json!(job.line));
    value.insert("jobDetails".into(), json!(job.details));
    value.insert("paused".into(), json!(listing.paused));
    if let Some(wakes) = wakes {
        value.insert("wakes".into(), json!(wakes));
    }
    value.insert("watching".into(), json!(listing.watches.len()));
    value.insert("agents".into(), json!(here.agents));
    if let Some(personality) = here.field("personality") {
        value.insert("personality".into(), json!(personality));
    }
    if let Some(model) = described.get("model").filter(|model| is_model(model)) {
        value.insert("model".into(), model.clone());
    }
    value.insert("folder".into(), json!(here.folder.display().to_string()));
    if let Some(project) = here.field("project") {
        value.insert("project".into(), json!(project));
    }
    if let Some(created) = here.field("created") {
        value.insert("created".into(), json!(created));
    }
    if let Some(limits) = limits_from(described.get("work").and_then(|work| work.get("limits"))) {
        value.insert("limits".into(), Value::Object(limits));
    }
    let allowed = allowed_of(described);
    if !allowed.is_empty() {
        value.insert("allowed".into(), json!(allowed));
    }
    value.insert(
        "deletes".into(),
        json!(what_delete_takes(
            place.computer,
            place.goes,
            listing.wakes.len(),
            listing.watches.len()
        )),
    );
    Listed {
        created: described
            .get("created")
            .map(|created| match created {
                Value::String(text) => text.clone(),
                Value::Null => String::new(),
                other => other.to_string(),
            })
            .unwrap_or_default(),
        paused: listing.paused,
        wakes,
        here,
        value,
    }
}

/// The bots a move is bringing here, not this box's to show yet.
pub fn arriving(run: &Path) -> Vec<String> {
    let folder: PathBuf = run.join("moves");
    names(&folder)
        .into_iter()
        .filter(|name| name.starts_with("mov_") && name.ends_with(".json"))
        .filter_map(|name| {
            let record: Value =
                serde_json::from_str(&kuvo_store::files::text_of(&folder.join(name))?).ok()?;
            let live = record["failed"].as_str().is_none_or(str::is_empty);
            let held = record["kind"] == "bot"
                && (record["role"] == "to" || record["stage"] == "finishing");
            (live && held)
                .then(|| record["thing"].as_str().map(str::to_owned))
                .flatten()
        })
        .collect()
}
