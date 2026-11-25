//! The coding agents' update notices: with "Update coding agents automatically" off, "Codex
//! 0.161.0 is available" as soon as the box knows of a newer version than the one installed (with
//! Update where Kuvo Code can update it); with it on, "Codex updated to 0.161.0" once the version
//! moved on. One per program and version, kept in `agents/update-notices.json` across restarts:
//! served until a client says it was seen, or until it no longer holds. A seen one is kept as
//! seen, so the same version is never raised twice. The box decides and words them; clients draw
//! them.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex as Plain,
};

use kuvo_store::files;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{Maintenance, compare_versions, lock, now_ms};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Available,
    Updated,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Updated => "updated",
        }
    }
}

/// A notice as it's kept.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Notice {
    /// `<program>-<kind>-<version>`: the same notice whenever it's decided again.
    id: String,
    program: String,
    name: String,
    kind: Kind,
    /// The version that's out, or that it updated to.
    version: String,
    /// The version it had: installed then, or before it updated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    from: Option<String>,
    at: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    seen: bool,
}

/// A notice as a client gets it: the box's words, and what can be done about it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Served {
    pub id: String,
    pub program: String,
    /// The agent's own sign-in, whose update route updates its program.
    pub provider: String,
    pub kind: Kind,
    pub version: String,
    pub title: String,
    pub body: String,
    pub at: u64,
    /// `update`: `POST kuvo/agents/api/kuvo/updates/<provider>`. None where Kuvo Code can't.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<Action>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Action {
    pub id: &'static str,
    pub label: &'static str,
}

/// What a program is now, as the box reads it to decide.
#[derive(Clone, Debug)]
pub struct ProgramNow {
    pub program: String,
    pub name: String,
    pub provider: String,
    pub maintenance: Maintenance,
    /// Kuvo Code can update it (its installer is known).
    pub updates: bool,
}

#[derive(Default, Serialize)]
struct Kept {
    notices: Vec<Notice>,
    /// Each program's version when last read, to tell an update that happened since.
    versions: Map<String, Value>,
}

/// The box's setting, as Settings › Computer saves it: the agents update themselves.
pub fn agents_update_themselves(home: &Path) -> bool {
    std::fs::read(home.join("box/agents-update-themselves.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        == Some(Value::Bool(true))
}

/// What `note` decided: a notice made just now (to be told once), and whether what's served
/// changed.
#[derive(Debug, Default)]
pub struct Decided {
    pub told: Option<Served>,
    pub changed: bool,
}

pub struct UpdateNotices {
    file: PathBuf,
    /// What Kuvo Code knows of each program now, for the words and the action.
    programs: Plain<HashMap<String, ProgramNow>>,
}

impl UpdateNotices {
    /// Kept in the agents' folder, `update-notices.json`.
    pub fn new(agents: &Path) -> Self {
        Self {
            file: agents.join("update-notices.json"),
            programs: Plain::default(),
        }
    }

    fn read(&self) -> Kept {
        let Some(kept) = std::fs::read(&self.file)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        else {
            return Kept::default();
        };
        let notices = kept["notices"]
            .as_array()
            .map(|all| {
                all.iter()
                    .filter_map(|each| serde_json::from_value(each.clone()).ok())
                    .collect()
            })
            .unwrap_or_default();
        let versions = kept["versions"].as_object().cloned().unwrap_or_default();
        Kept { notices, versions }
    }

    fn write(&self, kept: &Kept) -> std::io::Result<()> {
        files::write_secret(&self.file, &serde_json::to_vec(kept)?)
    }

    /// `now` as just read, with the agents updating themselves (`automatic`) or not.
    pub fn note(&self, now: ProgramNow, automatic: bool) -> Decided {
        lock(&self.programs).insert(now.program.clone(), now.clone());
        let Some(installed) = now.maintenance.installed.clone() else {
            return Decided::default();
        };
        let mut kept = self.read();
        let before = serde_json::to_value(&kept).ok();
        let was = kept.versions.get(&now.program).and_then(Value::as_str).map(str::to_owned);
        kept.versions.insert(now.program.clone(), Value::String(installed.clone()));
        let mine = |notice: &Notice| notice.program == now.program;
        // What it's updated to (or past) holds no more; nor does an earlier update, once moved on.
        kept.notices.retain(|notice| {
            !mine(notice)
                || match notice.kind {
                    Kind::Available => compare_versions(&installed, &notice.version).is_lt(),
                    Kind::Updated => notice.version == installed,
                }
        });
        let made = |kind: Kind, version: &str, from: &str| Notice {
            id: format!("{}-{}-{version}", now.program, kind.name()),
            program: now.program.clone(),
            name: now.name.clone(),
            kind,
            version: version.to_owned(),
            from: Some(from.to_owned()),
            at: now_ms(),
            seen: false,
        };
        let mut new = None;
        let latest = now.maintenance.latest.as_deref();
        if automatic {
            // It updates itself: what's out isn't yours to do, so it's let be (and not raised again).
            for notice in kept.notices.iter_mut().filter(|notice| mine(notice)) {
                if notice.kind == Kind::Available {
                    notice.seen = true;
                }
            }
            if let Some(was) = &was
                && compare_versions(&installed, was).is_gt()
                && !kept.notices.iter().any(|notice| {
                    mine(notice) && notice.kind == Kind::Updated && notice.version == installed
                })
            {
                new = Some(made(Kind::Updated, &installed, was));
            }
        } else if let Some(latest) = latest
            && compare_versions(&installed, latest).is_lt()
        {
            // A newer one than an earlier notice's replaces it; the same one is never raised twice.
            kept.notices.retain(|notice| {
                !mine(notice)
                    || notice.kind != Kind::Available
                    || compare_versions(&notice.version, latest).is_ge()
            });
            if !kept.notices.iter().any(|notice| {
                mine(notice) && notice.kind == Kind::Available && notice.version == latest
            }) {
                new = Some(made(Kind::Available, latest, &installed));
            }
        }
        if let Some(notice) = &new {
            kept.notices.push(notice.clone());
        }
        let changed = serde_json::to_value(&kept).ok() != before;
        // (Not kept, so not told: it's decided again the next time it's read.)
        if changed && self.write(&kept).is_err() {
            return Decided::default();
        }
        Decided {
            told: new.as_ref().map(|notice| self.served(notice)),
            changed: new.is_some() || changed,
        }
    }

    /// What clients show now, newest first.
    pub fn list(&self) -> Vec<Served> {
        let mut notices: Vec<_> = self.read().notices.into_iter().filter(|notice| !notice.seen).collect();
        notices.sort_by(|a, b| b.at.cmp(&a.at));
        notices.iter().map(|notice| self.served(notice)).collect()
    }

    /// It was seen (for every client): no longer served, and not raised again. Whether it was.
    pub fn seen(&self, id: &str) -> bool {
        let mut kept = self.read();
        let Some(notice) = kept.notices.iter_mut().find(|each| each.id == id && !each.seen) else {
            return false;
        };
        notice.seen = true;
        self.write(&kept).is_ok()
    }

    fn served(&self, notice: &Notice) -> Served {
        let programs = lock(&self.programs);
        let program = programs.get(&notice.program);
        let name = program.map_or(notice.name.as_str(), |program| program.name.as_str());
        let mut served = Served {
            id: notice.id.clone(),
            program: notice.program.clone(),
            provider: program.map_or_else(|| notice.program.clone(), |program| program.provider.clone()),
            kind: notice.kind,
            version: notice.version.clone(),
            title: format!("{name} {} is available", notice.version),
            body: String::new(),
            at: notice.at,
            action: None,
        };
        if notice.kind == Kind::Updated {
            served.title = format!("{name} updated to {}", notice.version);
            served.body = match &notice.from {
                Some(from) => format!("It was {from}. It updates itself, as set in Settings › Computer."),
                None => "It updates itself, as set in Settings › Computer.".into(),
            };
            return served;
        }
        let installed = program
            .and_then(|program| program.maintenance.installed.as_deref())
            .or(notice.from.as_deref());
        let have = installed.map(|installed| format!("You have {installed}.")).unwrap_or_default();
        if program.is_some_and(|program| program.updates) {
            served.body = have;
            served.action = Some(Action { id: "update", label: "Update" });
            return served;
        }
        let how = match program.and_then(|program| program.maintenance.shown.as_deref()) {
            Some(shown) => format!("Update it with “{shown}”."),
            None => format!("Update {name} the way you installed it."),
        };
        served.body = [have, how].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" ");
        served
    }
}
