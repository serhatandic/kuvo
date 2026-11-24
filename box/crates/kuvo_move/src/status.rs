//! Readable receipts shared by the move and its command.
use crate::{Disk, metadata};
use kuvo_store::{files, format, home::Home};
use serde_json::{Value, json};
use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

pub(crate) const UNREADABLE_JOURNAL: &str = "Kuvo was stopped part way through moving your sessions and can't tell how far it got, so it's changing nothing more. Try again; if this stays, send ~/.kuvo-code/updates/rehome/run.log to Kuvo's support.";
const STEPS: &[(&str, &str)] = &[
    ("plan", "Checking this Mac"),
    ("install", "Getting ready"),
    ("keys", "Handing over your keys"),
    ("clone", "Copying your sessions and settings"),
    ("upgrade", "Bringing your settings along"),
    ("database", "Bringing your sessions up to date"),
    ("rewrite", "Pointing everything at its new place"),
    ("check", "Checking nothing is missing"),
    ("place", "Putting everything in place"),
    ("fix", "Reconnecting your worktrees"),
    ("clear", "Finishing up"),
];
pub(crate) fn status(state: Option<&Value>) -> Value {
    let state = state.unwrap_or(&Value::Null);
    let overdue = matches!(state["state"].as_str(), Some("waiting" | "failed"))
        && state["waitingSince"]
            .as_u64()
            .is_some_and(|since| now_ms().saturating_sub(since) >= 3 * 86_400_000);
    json!({
        "state":state["state"],"way":state["way"],"step":null,"tries":state["tries"].as_u64().unwrap_or_default(),"nextTry":state["nextTry"],
        "waitingSince":state["waitingSince"],"overdue":overdue,"failed":state["failed"],"movedAt":state["movedAt"],"keptUntil":state["keptUntil"],
        "owed":state["owed"].as_array().cloned().unwrap_or_default(),"asked":state["asked"],"from":state["from"],
        "to":state["to"],"progress":null,"dropped":state["dropped"].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>(),
        "steps":STEPS.iter().map(|&(step,title)|json!({"step":step,"title":title})).collect::<Vec<_>>(),"signIns":sign_ins_receipt(state)
    })
}

/// A status query reads stopped or live receipts without taking the move lock.
pub fn rehome_status(home: &Home) -> Value {
    let disk = Disk(home.root().into());
    let read = || -> Result<_, crate::Error> {
        let kept = format::ObjectFormat::additive("Rehome state")
            .read(&disk.path("updates/rehome/state.json")?)?
            .map(|object| Value::Object(object.fields));
        let path = disk.path("updates/rehome/journal.json")?;
        let journal = match metadata(&path)? {
            None => None,
            Some(meta) if meta.is_file() => crate::json_at::<Value>(&path)?
                .filter(|journal| journal["v"] == 1 && journal["step"].is_string()),
            Some(_) => None,
        };
        if journal.is_none() && metadata(&path)?.is_some() {
            return Err(std::io::Error::other(UNREADABLE_JOURNAL).into());
        }
        Ok((kept, journal))
    };

    let (state, journal) = match read() {
        Ok(records) => records,
        Err(_) => {
            let mut status = status(None);
            status["state"] = json!("moving");
            status["failed"] =
                json!({ "at": now_ms(), "step": null, "message": UNREADABLE_JOURNAL });
            return status;
        }
    };
    let mut status = status(state.as_ref());
    if sign_ins_running(&disk) {
        status["signIns"] = json!({"state":"asking"});
    }
    if let Some(journal) = journal.filter(|journal| journal["step"] != "done") {
        let step = journal["step"].as_str().expect("validated move step");
        status["state"] = json!(if step == "clear" {
            "clearing"
        } else {
            "moving"
        });
        status["step"] = json!(step);
        status["way"] = journal["way"].clone();
        for name in ["from", "to"] {
            if status[name].is_null() {
                status[name] = journal[name].clone();
            }
        }
        if journal["way"] == "move" && journal["run"].is_object() {
            status["progress"] = progress(step);
        }
    } else {
        if status["state"] == "moving" {
            status["state"] = json!("failed");
        }
        if let Some(step) = live_phase(home.root()) {
            status["progress"] = progress(&step);
        }
    }
    status
}

fn sign_ins_receipt(state: &Value) -> Value {
    let receipt = &state["signInsLast"];
    if !receipt["at"].is_number() {
        return Value::Null;
    }
    match receipt["state"].as_str() {
        Some("done") => {}
        Some("failed")
            if state["owed"]
                .as_array()
                .is_some_and(|owed| owed.iter().any(|what| what == "passwords")) => {}
        _ => return Value::Null,
    }
    let mut answer = json!({"state":receipt["state"],"at":receipt["at"]});
    if receipt["message"].is_string() {
        answer["message"] = receipt["message"].clone();
    }
    for field in ["added", "kept"] {
        if receipt[field].is_number() {
            answer[field] = receipt[field].clone();
        }
    }
    answer
}

pub fn progress(step: &str) -> Value {
    let count = STEPS.len();
    if step == "settle" {
        return json!({ "step": "done", "title": "Starting Kuvo on this Mac", "index": count, "of": count });
    }
    STEPS.iter().enumerate().find(|(_, (name, _))| *name == step)
        .map_or(Value::Null, |(index, (_, title))| json!({ "step": step, "title": title, "index": index + 1, "of": count }))
}

fn live_phase(home: &Path) -> Option<String> {
    let rehome = Disk(home.into());
    let phase = files::read(&rehome.path("updates/rehome/run.phase").ok()?).ok()??;
    let phase = String::from_utf8(phase).ok()?;
    active_move_lock(&rehome, true).then(|| phase.trim().into())
}

pub(crate) fn active_move_lock(disk: &Disk, include_self: bool) -> bool {
    move_lock(disk, include_self).unwrap_or(false)
}

fn move_lock(disk: &Disk, include_self: bool) -> Option<bool> {
    let pid = recorded_pid(disk, "updates/rehome/lock/pid")?;
    if !include_self && pid == std::process::id() {
        return Some(false);
    }
    #[cfg(target_os = "macos")]
    let role = legacy_role(pid);
    #[cfg(target_os = "macos")]
    if matches!(role, Some(LegacyRole::SignIns | LegacyRole::Settle)) {
        return Some(false);
    }
    let folder = disk.path("updates/rehome/lock").ok()?;
    let prefix = format!("owner-{pid}-");
    let owned = crate::entries(&folder)
        .ok()?
        .into_iter()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .any(|entry| {
            crate::json_at::<kuvo_os::process::ProcessIdentity>(&entry.path())
                .ok()
                .flatten()
                .is_some_and(|owner| {
                    owner.pid == pid && owner.state() == kuvo_os::process::ProcessState::Live
                })
        });
    #[cfg(target_os = "macos")]
    let owned = owned || role == Some(LegacyRole::Moving);
    Some(owned)
}

fn recorded_pid(disk: &Disk, name: &str) -> Option<u32> {
    let bytes = files::read(&disk.path(name).ok()?).ok()??;
    let pid = std::str::from_utf8(&bytes).ok()?.trim().parse().ok()?;
    (pid > 1).then_some(pid)
}

fn sign_ins_running(_disk: &Disk) -> bool {
    #[cfg(target_os = "macos")]
    return ["updates/rehome/sign-ins.pid", "updates/rehome/lock/pid"]
        .into_iter()
        .filter_map(|path| recorded_pid(_disk, path))
        .any(|pid| legacy_role(pid) == Some(LegacyRole::SignIns));
    #[cfg(not(target_os = "macos"))]
    false
}

#[cfg(target_os = "macos")]
#[derive(PartialEq)]
enum LegacyRole {
    Moving,
    SignIns,
    Settle,
}

#[cfg(target_os = "macos")]
fn legacy_role(pid: u32) -> Option<LegacyRole> {
    let owner = kuvo_os::process::ProcessIdentity::read(pid).ok()??;
    let command = crate::mac::processes::command_line(pid).ok()?;
    if owner.state() != kuvo_os::process::ProcessState::Live {
        return None;
    }
    let mut words = command.split_ascii_whitespace();
    if !words.any(|word| word == "rehome") {
        let install = command
            .split_ascii_whitespace()
            .any(|word| word == "install");
        let predecessor = command.split_ascii_whitespace().any(|word| {
            Path::new(word)
                .file_name()
                .is_some_and(|name| matches!(name.to_str(), Some("kuvo-link.js" | "main.ts")))
        });
        return (install && predecessor).then_some(LegacyRole::Moving);
    }
    Some(
        words
            .find_map(|word| match word {
                "--sign-ins" => Some(LegacyRole::SignIns),
                "--settle" => Some(LegacyRole::Settle),
                _ => None,
            })
            .unwrap_or(LegacyRole::Moving),
    )
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
