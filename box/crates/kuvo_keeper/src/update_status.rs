//! The installed version, the last check, and any refusal left by the keeper.

use std::{io, path::Path};

use kuvo_store::{files, format::ObjectFormat, home::Home};
use serde_json::{Value, json};

use crate::{Release, SELECTOR, prepare_command::version_order};

const BLOCKED: &str = "Only a machine's own Kuvo Code folder updates itself.";
const STATE: ObjectFormat = ObjectFormat::additive("Box update state");

pub fn note_check(home: &Home, latest: &str, checked: i64) -> io::Result<()> {
    let path = home.run().join("update.json");
    let mut fields = STATE
        .read(&path)
        .map_err(io::Error::other)?
        .map_or_else(Default::default, |state| state.fields);
    fields.insert("latest".into(), latest.into());
    fields.insert("checked".into(), checked.into());
    files::write_secret(&path, &serde_json::to_vec(&STATE.encode(fields))?)
}

pub fn read(home: &Home, profile: &Path) -> io::Result<Value> {
    let blocked = (home.root() != profile.join(".kuvo-code")).then_some(BLOCKED);
    let run = home.run();
    let version = if let Some(target) = Release::read(&home.root().join(SELECTOR))? {
        Some(target.version().to_owned())
    } else {
        files::text_of(&home.bin().join("box-version"))
            .map(|value| value.trim().to_owned())
            .filter(|value| version_order(value).is_some())
    };
    let state = STATE
        .read(&run.join("update.json"))
        .map_err(io::Error::other)?
        .map_or_else(Default::default, |state| state.fields);
    let engine = files::text_of(&run.join("engine.json"))
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|entry| entry["version"].as_str().map(str::to_owned));
    let mut last = state.get("last").cloned().unwrap_or(Value::Null);
    if let Some(failed) = switch_failure(home)
        && last["at"]
            .as_u64()
            .is_none_or(|time| failed.at.saturating_add(1000) >= time)
    {
        last = json!({
            "result":"failed",
            "at":failed.at,
            "to":failed.version,
            "message":failed.message(),
        });
    }
    let mut answer = json!({
        "setUp":true,
        "version":version,
        "engine":engine,
        "latest":state.get("latest"),
        "last":last,
        "updating":null,
        "blocked":blocked,
    });
    if let Some(engine) = state.get("engine") {
        answer["engineLast"] = engine.clone();
    }
    Ok(answer)
}

struct Failed {
    at: u64,
    version: String,
    digest_prefix: String,
}

impl Failed {
    fn message(&self) -> String {
        format!("{} didn't start here", self.version)
    }
}

fn switch_failure(home: &Home) -> Option<Failed> {
    refusal(&files::text_of(&home.run().join("update-cannot"))?)
}

/// The keeper's refusal applies only to the trial it rejected.
pub fn trial_failure(home: &Home, version: &str, sha256: &str) -> Option<String> {
    let failed = switch_failure(home)?;
    (failed.version == version && sha256.starts_with(&failed.digest_prefix))
        .then(|| failed.message())
}

fn refusal(record: &str) -> Option<Failed> {
    let mut parts = record.split_whitespace();
    let at = parts.next()?.parse::<u64>().ok()?.checked_mul(1000)?;
    let version = parts.next()?;
    let digest = parts.next()?;
    version_order(version)?;
    if digest.len() != 12
        || !digest
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return None;
    }
    Some(Failed {
        at,
        version: version.into(),
        digest_prefix: digest.into(),
    })
}
