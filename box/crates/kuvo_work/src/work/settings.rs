//! Where new projects go on this box (PROTOCOL.md, "Work"): a folder you pick once per computer,
//! kept in `box/work.json` as `projectsFolder`; none until you pick one. And Keep This Mac Awake:
//! a Mac's box keeps its Mac from falling asleep by itself while agents work there, as its setting
//! says (`keepsAwake`, on unless you turn it off).

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use serde_json::{Map, Value, json};

/// What's said when `keepsAwake` isn't true or false.
pub const KEEPS_AWAKE_REFUSED: &str =
    "Say whether to keep this Mac awake while agents work: keepsAwake, true or false.";
const CHANGED_SINCE: &str = "It changed since you read it.";

/// This box's work settings (`box/work.json`), and whether it keeps its Mac awake itself.
#[derive(Clone)]
pub struct WorkSettings {
    file: PathBuf,
    account: PathBuf,
    is_mac: bool,
    /// Whether this box keeps its Mac awake while agents work (a Mac's, on macOS).
    pub keeps_mac_awake: bool,
    awake_file: PathBuf,
}

fn is_folder(folder: &Path) -> bool {
    folder.is_dir()
}

impl WorkSettings {
    pub fn new(home: &Path, account: &Path, is_mac: bool) -> Self {
        Self {
            file: home.join("box/work.json"),
            account: account.to_owned(),
            is_mac,
            keeps_mac_awake: is_mac && cfg!(target_os = "macos"),
            awake_file: home.join("run/work-awake.json"),
        }
    }

    fn kept(&self) -> Map<String, Value> {
        kuvo_store::files::text_of(&self.file)
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default()
    }

    /// Your home folder here.
    pub fn account(&self) -> &Path {
        &self.account
    }

    /// This computer in a sentence: "this Mac" or "this machine".
    pub fn here(&self) -> &'static str {
        if self.is_mac {
            "this Mac"
        } else {
            "this machine"
        }
    }

    /// The folder new projects go in here, or none until you pick one (or once it's gone).
    pub fn projects_folder(&self) -> Option<PathBuf> {
        let folder = PathBuf::from(self.kept().get("projectsFolder")?.as_str()?);
        (folder.is_absolute() && is_folder(&folder)).then_some(folder)
    }

    /// The folder to offer when none is picked, if there's one.
    pub fn suggested(&self) -> Option<PathBuf> {
        ["Developer", "Projects", "projects", "dev", "code"]
            .iter()
            .map(|name| self.account.join(name))
            .find(|folder| is_folder(folder))
    }

    /// `given` (written with `~` or in full) as a projects folder, or why it can't be one.
    pub fn folder_given(&self, given: &Value) -> Result<PathBuf, String> {
        let Some(given) = given
            .as_str()
            .map(str::trim)
            .filter(|given| !given.is_empty())
        else {
            return Err("Say which folder (projectsFolder), as a full path.".into());
        };
        let written = match given.strip_prefix('~') {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => {
                format!("{}{rest}", self.account.display())
            }
            _ => given.to_owned(),
        };
        let written = PathBuf::from(written);
        if !written.is_absolute() {
            return Err("Give a full path to the folder.".into());
        }
        let folder = normalized(&written);
        if !is_folder(&folder) {
            return Err(format!("There's no folder at {given}."));
        }
        Ok(folder)
    }

    /// Keep This Mac Awake, as this box keeps it: on unless you turned it off.
    pub fn keeps_awake(&self) -> bool {
        self.kept().get("keepsAwake") != Some(&Value::Bool(false))
    }

    /// One change at a time, from any of this box's processes; `was`, only while the fields named
    /// are still as they were read.
    fn write(
        &self,
        change: Map<String, Value>,
        was: Option<&Map<String, Value>>,
    ) -> Result<(), (u16, String)> {
        let lock = self.file.with_extension("json.lock");
        let _lock = kuvo_store::locks::OwnedLock::acquire(&lock, Duration::from_secs(10))
            .map_err(|error| (500, error.to_string()))?;
        if let Some(was) = was {
            let now = |key: &str| match key {
                "projectsFolder" => json!(
                    self.projects_folder()
                        .map(|folder| folder.display().to_string())
                ),
                _ => json!(self.keeps_awake()),
            };
            if change.keys().any(|key| was.get(key) != Some(&now(key))) {
                return Err((409, CHANGED_SINCE.into()));
            }
        }
        let mut kept = self.kept();
        kept.extend(change);
        let text = serde_json::to_string_pretty(&Value::Object(kept)).expect("JSON");
        kuvo_store::files::write_secret(&self.file, text.as_bytes())
            .map_err(|error| (500, error.to_string()))
    }

    /// The settings as clients read them.
    pub fn shown(&self) -> Value {
        let folder = self.projects_folder();
        let mut shown =
            json!({ "projectsFolder": folder.as_ref().map(|folder| folder.display().to_string()) });
        if folder.is_none()
            && let Some(suggested) = self.suggested()
        {
            shown["suggested"] = json!(suggested.display().to_string());
        }
        if self.keeps_mac_awake {
            shown["keepsAwake"] = json!(self.keeps_awake());
            shown["awakeNow"] = json!(self.awake_now());
        }
        shown
    }

    /// `PATCH kuvo/work/settings`: both checked before either is kept.
    pub fn change(&self, given: &Value) -> Result<Value, (u16, String)> {
        let given = given.as_object().cloned().unwrap_or_default();
        let awake = self.keeps_mac_awake && given.contains_key("keepsAwake");
        if !given.contains_key("projectsFolder") && !awake {
            return Err((
                400,
                if self.keeps_mac_awake {
                    "Send {projectsFolder}: a full path, or null; or {keepsAwake}: true or false."
                } else {
                    "Send {projectsFolder}: a full path, or null."
                }
                .into(),
            ));
        }
        let mut change = Map::new();
        if awake {
            let Some(on) = given["keepsAwake"].as_bool() else {
                return Err((400, KEEPS_AWAKE_REFUSED.into()));
            };
            change.insert("keepsAwake".into(), json!(on));
        }
        if let Some(folder) = given.get("projectsFolder") {
            let folder = match folder {
                Value::Null => Value::Null,
                given => json!(
                    self.folder_given(given)
                        .map_err(|refused| (400, refused))?
                        .display()
                        .to_string()
                ),
            };
            change.insert("projectsFolder".into(), folder);
        }
        match given.get("was") {
            None => self.write(change, None)?,
            Some(Value::Object(was)) => self.write(change, Some(was))?,
            Some(_) => return Err((400, "Send was as the fields you read.".into())),
        }
        Ok(self.shown())
    }

    /// Whether the Mac is held awake for agents' work now, by whichever process holds it.
    pub fn awake_now(&self) -> bool {
        let said: Value = kuvo_store::files::text_of(&self.awake_file)
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        let running = |pid: &Value| {
            pid.as_u64()
                .filter(|pid| *pid > 1)
                .and_then(|pid| u32::try_from(pid).ok())
                .is_some_and(|pid| {
                    !matches!(kuvo_os::process::ProcessIdentity::read(pid), Ok(None))
                })
        };
        running(&said["by"]) && running(&said["hold"])
    }

    /// Looked at every ten seconds while the box runs: held while the setting's on and agents work
    /// here (`busy`), let go otherwise. Only on a Mac.
    pub fn keep_awake(
        &self,
        busy: impl Fn() -> bool + Send + 'static,
    ) -> Option<tokio::task::JoinHandle<()>> {
        if !self.keeps_mac_awake {
            return None;
        }
        let settings = self.clone();
        Some(tokio::spawn(async move {
            let mut held: Option<tokio::process::Child> = None;
            let mut noted = String::new();
            loop {
                if let Some(child) = held.as_mut()
                    && child.try_wait().ok().flatten().is_some()
                {
                    held = None;
                }
                let wanted = settings.keeps_awake() && busy();
                if wanted && held.is_none() {
                    held = tokio::process::Command::new("/usr/bin/caffeinate")
                        .args(["-i", "-w", &std::process::id().to_string()])
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .kill_on_drop(true)
                        .spawn()
                        .ok();
                } else if !wanted && let Some(mut child) = held.take() {
                    let _ = child.kill().await;
                }
                let now = json!({ "by": std::process::id(), "hold": held.as_ref().and_then(tokio::process::Child::id) }).to_string();
                if (now != noted || !settings.awake_file.exists())
                    && kuvo_store::files::write_secret(&settings.awake_file, now.as_bytes()).is_ok()
                {
                    noted = now;
                }
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        }))
    }
}

/// A path with `.` and `..` worked out, as `path.resolve` does.
pub(super) fn normalized(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}
