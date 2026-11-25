//! What the ACP flavours share of this computer: where an agent's program is, the user's home,
//! a short command of the agent's (its `logout`), and the catalog it last listed.

use std::{
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use kuvo_os::{
    platform::{Account, Environment, Executable, Platform, env_value, which},
    process::Spawn,
};
use kuvo_store::home::Home;
use serde::{Deserialize, Serialize};

pub(super) use crate::codex::fingerprint;

use crate::catalog::{Model, Models};

pub(super) fn environment() -> Environment {
    let mut env: Environment = std::env::vars_os().collect();
    // It reaches the relay's servers on this computer: never through a proxy.
    crate::tool_relay::unproxied_loopback(&mut env);
    env
}

pub(super) fn user_home(env: &Environment) -> Option<PathBuf> {
    Account::current()
        .ok()?
        .home_folder(Platform::current(), env)
        .ok()
}

pub(super) fn variable(env: &Environment, name: &str) -> Option<String> {
    env_value(Platform::current(), env, name)
        .map(|value| value.to_string_lossy().trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// `name` where its maker's installer puts it (`places`, under the user's home), else on the PATH.
pub(super) fn program(name: &str, places: &[&str], env: &Environment) -> Option<Executable> {
    let mut folders: Vec<PathBuf> = Vec::new();
    if let Some(home) = user_home(env) {
        folders.extend(places.iter().map(|place| home.join(place)));
    }
    if env_value(Platform::current(), env, "KUVO_TEST_PATH_ONLY") != Some(OsStr::new("1")) {
        folders.extend(["/opt/homebrew/bin".into(), "/usr/local/bin".into()]);
    }
    which(OsStr::new(name), env, &folders)
}

/// Whether the user lets agents update themselves (Settings › Computer).
pub(super) fn agents_update_themselves(home: &Home) -> bool {
    fs::read(home.settings().join("agents-update-themselves.json"))
        .is_ok_and(|bytes| matches!(serde_json::from_slice::<bool>(&bytes), Ok(true)))
}

/// Runs the agent's own short command (its `logout`), at most 20 seconds: whether it went well.
pub(super) fn run(
    executable: Executable,
    env: Environment,
    args: &[&str],
    cwd: &Path,
) -> io::Result<bool> {
    let mut spawn = Spawn::new(executable.path(), env);
    spawn.executable = executable;
    spawn.args = args.iter().map(Into::into).collect();
    spawn.cwd = Some(cwd.to_owned());
    let mut child = spawn.spawn()?;
    let deadline = Instant::now() + Duration::from_secs(20);
    while !child.has_exited()? {
        if Instant::now() >= deadline {
            child.stop(Duration::from_secs(1), |_| {})?;
            return Err(io::Error::from(io::ErrorKind::TimedOut));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(child.wait()?.success())
}

/// A model as the agent last listed it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct Listed {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub efforts: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

/// The models an account's agent last listed, kept for the sign-in they're for, beside the box's
/// other run files: read to list them without starting the agent.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct Catalog {
    pub credential: Option<String>,
    #[serde(default)]
    pub current: Option<String>,
    pub models: Vec<Listed>,
}

impl Catalog {
    pub fn file(home: &Home, kind: &str, provider: &str) -> PathBuf {
        home.run()
            .join("agents")
            .join(kind)
            .join("catalogs")
            .join(format!("{provider}.json"))
    }

    /// The one kept for `credential`.
    pub fn read(file: &Path, credential: Option<&str>) -> Option<Self> {
        let bytes = fs::read(file).ok()?;
        let catalog: Self = serde_json::from_slice(&bytes).ok()?;
        (catalog.credential.as_deref() == credential && credential.is_some()).then_some(catalog)
    }

    /// Kept, when it says something new: whether it did.
    pub fn write(&self, file: &Path) -> bool {
        if Self::read(file, self.credential.as_deref()).as_ref() == Some(self) {
            return false;
        }
        let Ok(bytes) = serde_json::to_vec(self) else {
            return false;
        };
        file.parent()
            .is_some_and(|folder| kuvo_store::files::private_folder(folder).is_ok())
            && kuvo_store::files::write_atomic(file, &bytes).is_ok()
    }
}

/// The picker's models: those listed (the current one first), each with how much it holds.
pub(super) fn models(
    catalog: &Catalog,
    context: impl Fn(&str) -> u64,
    name: impl Fn(&str) -> String,
) -> Models {
    let mut listed = catalog.models.clone();
    if let Some(current) = &catalog.current
        && let Some(at) = listed.iter().position(|model| &model.id == current)
    {
        let model = listed.remove(at);
        listed.insert(0, model);
    }
    Models {
        generation: None,
        complete: true,
        models: listed
            .into_iter()
            .map(|listed| Model {
                context: Some(context(&listed.id)),
                efforts: listed.efforts,
                name: name(&listed.name),
                id: listed.id,
                ..Model::default()
            })
            .collect(),
    }
}

/// Before it has listed its models here: the one it picks itself, until a conversation's first
/// turn lists them.
pub(super) fn provisional(name: &str) -> Models {
    Models {
        generation: None,
        complete: false,
        models: vec![Model {
            name: format!("{name} (its default model)"),
            id: DEFAULT_MODEL.into(),
            provisional: true,
            ..Model::default()
        }],
    }
}

/// The model the agent picks itself: none is chosen for it.
pub(super) const DEFAULT_MODEL: &str = "default";
