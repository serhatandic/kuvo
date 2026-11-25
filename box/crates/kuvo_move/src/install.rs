//! The installer's decision about data already on this computer.
use crate::{Disk, Error, app_source, metadata, rehome_status};
use kuvo_os::platform::Platform;
use kuvo_store::{files, format::ObjectFormat, home::Home};
use serde::Serialize;
use serde_json::Value;
use std::{io, path::Path};

pub const TWO_HOMES_MOVE: &str = "This Mac is set up as one of your machines, and has an earlier Kuvo Code's sessions too: putting them together isn't in this version yet.";

/// Match the Mac installer's account check for present and prospective homes.
#[cfg(target_os = "macos")]
pub fn same_mac_home(left: &Path, right: &Path) -> bool {
    use std::{fs, os::unix::fs::MetadataExt as _};

    if let (Ok(left), Ok(right)) = (fs::metadata(left), fs::metadata(right)) {
        return left.dev() == right.dev() && left.ino() == right.ino();
    }
    let placed = |path: &Path| {
        path.canonicalize()
            .ok()
            .or_else(|| Some(path.parent()?.canonicalize().ok()?.join(path.file_name()?)))
            .unwrap_or_else(|| path.to_owned())
    };
    placed(left)
        .as_os_str()
        .eq_ignore_ascii_case(placed(right).as_os_str())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    Fresh,
    Ours,
    #[serde(rename = "app-07")]
    App07,
    AppBox,
    Moving,
    Stuck,
    Waits,
    Back,
    Machine,
    Two,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verb {
    New,
    Move,
}

#[derive(Serialize)]
pub struct Judgement {
    pub verdict: Verdict,
    pub verb: Option<Verb>,
    pub status: Value,
}

impl Judgement {
    pub fn unreadable_move(&self) -> Option<&str> {
        if self.verdict == Verdict::Stuck && self.status["step"].is_null() {
            self.status["failed"]["message"].as_str()
        } else {
            None
        }
    }
}

/// An interrupted making can authorize recovery. Validate both records before
/// deciding whether this installation needs either recovery path.
pub fn check_preflight(home: &Home, installed_bin: &Path) -> Result<(), Error> {
    let format = ObjectFormat::additive("Installed Box receipt");
    if format
        .read(&installed_bin.join(".installed.json"))?
        .is_none()
    {
        format.read(&home.updates().join("installed.json"))?;
    }
    let disk = Disk(home.root().into());
    for marker in ["updates/install/making", "updates/rehome/new"] {
        attempt_at(&disk.path(marker)?)?;
    }
    Ok(())
}

/// Inspect the folders without starting Kuvo. Helper readiness comes from
/// the installer because the same home can be used by a demo copy.
pub fn judge(
    home: &Home,
    legacy_app: Option<&Path>,
    platform: Platform,
    helper_ready: Option<bool>,
) -> Result<Judgement, Error> {
    let disk = Disk(home.root().into());
    let status = rehome_status(home);
    let facts = HomeFacts::read(&disk)?;
    let say = |verdict, verb| {
        Ok(Judgement {
            verdict,
            verb,
            status: status.clone(),
        })
    };
    if crate::status::active_move_lock(&disk, false) {
        return say(Verdict::Waits, None);
    }
    if status["state"] == "clearing" || (status["state"] == "moving" && status["step"].is_null()) {
        return say(Verdict::Stuck, Some(Verb::Move));
    }
    if status["state"] == "moving" {
        return say(Verdict::Moving, Some(Verb::Move));
    }
    if facts.mac_owned(&disk)? {
        return say(
            Verdict::Ours,
            (helper_ready == Some(false)).then_some(Verb::New),
        );
    }
    let state =
        ObjectFormat::additive("Rehome state").read(&disk.path("updates/rehome/state.json")?)?;
    if state
        .is_some_and(|record| record.fields.get("movedBack").and_then(Value::as_bool) == Some(true))
    {
        return say(Verdict::Back, None);
    }
    let source = legacy_app.map(app_source).transpose()?.flatten();
    if facts.machine_owned(&disk, platform)? {
        return say(
            if source.is_some() {
                Verdict::Two
            } else {
                Verdict::Machine
            },
            None,
        );
    }
    if let Some(source) = source {
        return say(source, Some(Verb::Move));
    }
    say(Verdict::Fresh, Some(Verb::New))
}

pub(crate) struct HomeFacts {
    kind: Option<String>,
    identity: bool,
}

impl HomeFacts {
    pub(crate) fn read(disk: &Disk) -> Result<Self, Error> {
        let kind =
            files::read(&disk.path("box/kind")?)?.and_then(|bytes| String::from_utf8(bytes).ok());
        let link = crate::json_at::<Value>(&disk.path("run/link.json")?)
            .ok()
            .flatten();
        let card = crate::json_at::<Value>(&disk.path("run/setup-card.json")?)
            .ok()
            .flatten();
        let identity = metadata(&disk.path("box/keys")?)?.is_some()
            || link.is_some_and(|link| link.is_object())
            || card.is_some_and(|card| card["self"].is_object());
        Ok(Self { kind, identity })
    }

    pub(crate) fn is(&self, kind: &str) -> bool {
        self.kind.as_deref().map(str::trim) == Some(kind)
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn identity(&self) -> bool {
        self.identity
    }

    pub(crate) fn mac_owned(&self, disk: &Disk) -> Result<bool, Error> {
        Ok(self.is("mac") && !self.making_cut_short(disk)?)
    }

    pub(crate) fn making_cut_short(&self, disk: &Disk) -> Result<bool, Error> {
        if !attempt_at(&disk.path("updates/rehome/new")?)? || self.identity || self.is("machine") {
            return Ok(false);
        }
        let recorded = ObjectFormat::additive("Rehome state")
            .read(&disk.path("updates/rehome/state.json")?)?;
        Ok(
            metadata(&disk.path("updates/rehome/journal.json")?)?.is_none()
                && recorded
                    .is_none_or(|record| record.fields.get("state").is_none_or(Value::is_null)),
        )
    }

    pub(crate) fn machine_owned(&self, disk: &Disk, platform: Platform) -> Result<bool, Error> {
        let launcher = if platform == Platform::Windows {
            "bin/kuvo-box.exe"
        } else {
            "bin/kuvo-start"
        };
        if self.is("mac") || !disk.0.join(launcher).exists() || disk.path("run/off")?.exists() {
            return Ok(false);
        }
        if platform != Platform::Mac && attempt_at(&disk.path("updates/install/making")?)? {
            return match crate::check_writers(disk) {
                Err(Error::Busy(_)) => Ok(true),
                other => other.map(|()| false),
            };
        }
        Ok(platform != Platform::Mac || self.is("machine") || self.identity)
    }
}

fn attempt_at(path: &Path) -> Result<bool, Error> {
    let Some(bytes) = files::read(path)? else {
        return Ok(false);
    };
    let text = String::from_utf8_lossy(&bytes);
    let text = text.trim();
    if !text.is_empty() && text.bytes().all(|c| c.is_ascii_digit()) {
        return Ok(true);
    }
    let value = serde_json::from_str(text).unwrap_or(Value::Null);
    let record = ObjectFormat::additive("Interrupted installation marker").decode(value)?;
    if record.fields.get("at").is_some_and(Value::is_number) {
        Ok(true)
    } else {
        Err(io::Error::other("The interrupted installation marker is unreadable.").into())
    }
}
