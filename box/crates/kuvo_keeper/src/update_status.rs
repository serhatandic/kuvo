//! The installed version, the last check, and any refusal left by the keeper.

use std::{fs, io, path::Path, time::Duration};

use kuvo_store::{
    files,
    format::{Fields, ObjectFormat},
    home::Home,
};
use serde::Serialize;
use serde_json::{Value, json};

use crate::{Release, SELECTOR, fleet, prepare_command::version_order};

const BLOCKED: &str = "Only a machine's own Kuvo Code folder updates itself.";
const STATE: ObjectFormat = ObjectFormat::additive("Box update state");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Checking,
    Downloading,
    DownloadingEngine,
    CheckingIt,
    Installing,
}

impl Step {
    pub fn words(self) -> &'static str {
        match self {
            Self::Checking => "checking",
            Self::Downloading => "downloading",
            Self::DownloadingEngine => "downloading the engine",
            Self::CheckingIt => "checking it",
            Self::Installing => "installing",
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "result", rename_all = "lowercase")]
pub enum Outcome {
    Current {
        version: String,
    },
    Waiting {
        version: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    Failed {
        #[serde(skip_serializing_if = "Option::is_none")]
        version: Option<String>,
        message: String,
    },
    Blocked {
        message: String,
    },
    Updated {
        from: String,
        to: String,
    },
}

#[derive(Serialize)]
struct LastUpdate {
    at: i64,
    #[serde(flatten)]
    outcome: Outcome,
}

pub fn checked_round(home: &Home) -> io::Result<Option<String>> {
    Ok(STATE
        .read(&home.run().join("update.json"))
        .map_err(io::Error::other)?
        .and_then(|state| {
            state
                .fields
                .get("round")
                .and_then(Value::as_str)
                .map(str::to_owned)
        }))
}

pub fn note_check(
    home: &Home,
    latest: Option<&str>,
    checked: i64,
    round: Option<&str>,
    outcome: Option<Outcome>,
) -> io::Result<()> {
    change(home, |fields| {
        if let Some(latest) = latest {
            fields.insert("latest".into(), latest.into());
            fields.insert("checked".into(), checked.into());
        }
        if let Some(round) = round {
            fields.insert("round".into(), round.into());
        }
        if let Some(outcome) = outcome {
            if matches!(outcome, Outcome::Updated { .. }) {
                fields.remove("trial");
            }
            fields.insert(
                "last".into(),
                serde_json::to_value(LastUpdate {
                    at: checked,
                    outcome,
                })?,
            );
        }
        Ok(())
    })
}

fn change<T>(home: &Home, apply: impl FnOnce(&mut Fields) -> io::Result<T>) -> io::Result<T> {
    let path = home.run().join("update.json");
    // Keeper confirmation and the resident's checks both update this record.
    // The lock protects only the atomic local read/modify/write, never a task.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(home.run().join(".update-state.lock"))?;
    lock.lock()?;
    let mut fields = STATE
        .read(&path)
        .map_err(io::Error::other)?
        .map_or_else(Default::default, |state| state.fields);
    let result = apply(&mut fields)?;
    files::write_secret(&path, &serde_json::to_vec(&STATE.encode(fields))?)?;
    Ok(result)
}

pub fn note_last(home: &Home, at: i64, outcome: Outcome) -> io::Result<()> {
    note_check(home, None, at, None, Some(outcome))
}

/// The shell guard and its foreground resident are separate processes. Remember
/// the exact selected trial before starting it, then settle only after the
/// guard removes `previous` and the selected release still matches.
pub(crate) fn mark_shell_trial(home: &Home, from: &Release, to: &Release) -> io::Result<()> {
    change(home, |fields| {
        fields.insert(
            "shellTrial".into(),
            json!({"from":from.to_string(),"to":to.to_string()}),
        );
        Ok(())
    })
}

pub fn settle_shell_trial(home: &Home) -> io::Result<()> {
    if home.run().join("previous").try_exists()?
        || !STATE
            .read(&home.run().join("update.json"))
            .map_err(io::Error::other)?
            .is_some_and(|state| state.fields.contains_key("shellTrial"))
    {
        return Ok(());
    }
    change(home, |fields| {
        if home.run().join("previous").try_exists()? {
            return Ok(());
        }
        let Some(trial) = fields.remove("shellTrial") else {
            return Ok(());
        };
        let (Some(from), Some(to)) = (
            trial["from"]
                .as_str()
                .and_then(|value| value.parse::<Release>().ok()),
            trial["to"]
                .as_str()
                .and_then(|value| value.parse::<Release>().ok()),
        ) else {
            return Ok(());
        };
        if fleet::selected(home)?.as_ref() == Some(&to) {
            fields.remove("trial");
            fields.insert(
                "last".into(),
                serde_json::to_value(LastUpdate {
                    at: jiff::Timestamp::now().as_millisecond(),
                    outcome: Outcome::Updated {
                        from: from.version().into(),
                        to: to.version().into(),
                    },
                })?,
            );
        }
        Ok(())
    })
}

/// A released updater can leave its old process marker in a migrated home.
/// Only a live updater (or the just-created lock before it records its PID)
/// is active; a stale phase is not an update in progress.
pub fn legacy_updating(home: &Home) -> bool {
    let lock = home.run().join(".updating");
    if let Some(pid) = files::text_of(&lock.join("pid"))
        .and_then(|text| text.trim().parse::<u32>().ok())
        .filter(|pid| *pid != 0)
    {
        return kuvo_os::process::legacy_process_runs(pid, "kuvo-link");
    }
    fs::metadata(lock)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.elapsed().ok())
        .is_some_and(|age| age < Duration::from_secs(60))
}

fn legacy_trial_busy(home: &Home) -> bool {
    home.run().join("update-trial").exists()
        || home.run().join("previous").exists()
        || legacy_updating(home)
}

/// Read the result of a predecessor's interrupted in-place trial. The Rust
/// updater never restores that data itself; it reports what the old keeper left.
pub fn settle_legacy_trial(home: &Home) -> io::Result<()> {
    if legacy_trial_busy(home)
        || !STATE
            .read(&home.run().join("update.json"))
            .map_err(io::Error::other)?
            .is_some_and(|state| state.fields.contains_key("trial"))
    {
        return Ok(());
    }
    let event = change(home, |fields| {
        if legacy_trial_busy(home) {
            return Ok(None);
        }
        let Some(trial) = fields.get("trial") else {
            return Ok(None);
        };
        let Some(to) = trial["to"].as_str().map(str::to_owned) else {
            return Ok(None);
        };
        let from = trial["from"].as_str().map(str::to_owned);
        let before = from
            .as_deref()
            .filter(|value| !value.is_empty())
            .unwrap_or("what it had");
        let message = if home.updates().join("rollback").exists() {
            format!(
                "{to} didn't run well here, and not all of it could go back to {before}: Kuvo updates again later. Your conversations stay as they are."
            )
        } else {
            format!(
                "{to} didn't run well here, so it went back to {before}. Your conversations stay as they are."
            )
        };
        let at = jiff::Timestamp::now().as_millisecond();
        let mut last = json!({"at":at,"result":"failed","to":to,"message":message});
        if let Some(from) = from {
            last["from"] = from.into();
        }
        let event = json!({"at":at,"version":to,"step":"rollback","message":message});
        fields.remove("trial");
        fields.insert("last".into(), last);
        Ok(Some(event))
    })?;
    if let Some(event) = event
        && let Err(error) = append_event(home, &event)
    {
        eprintln!("Recording the predecessor's rollback: {error}");
    }
    Ok(())
}

fn append_event(home: &Home, event: &Value) -> io::Result<()> {
    let path = home.run().join("fleet-update.log");
    let length = match fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_file() && meta.len() >= 512_000 => {
            files::replace_synced(&path, &home.run().join("fleet-update.log.1"))?;
            0
        }
        Ok(meta) if meta.is_file() => files::read(&path)?
            .unwrap_or_default()
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |end| end as u64 + 1),
        Ok(_) => return Err(io::Error::other("The update log is not a regular file.")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error),
    };
    let mut line = serde_json::to_vec(event)?;
    line.push(b'\n');
    files::append_staged_line(&path, length, &line)
}

pub fn read(home: &Home, profile: &Path) -> io::Result<Value> {
    if let Err(error) = settle_shell_trial(home).and_then(|_| settle_legacy_trial(home)) {
        eprintln!("Recording the completed trial: {error}");
    }
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
        "updating":legacy_updating(home).then(|| state.get("running").and_then(Value::as_str).unwrap_or("checking")),
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
