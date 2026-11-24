//! The agents' SDKs from npm, installed the first time one is needed: each
//! agent's in a folder of its own (`runtime/<agent>`), by the engine's Bun,
//! at the version this box was written for. Bun's download cache is the
//! box's own and is emptied after an install.
//!
//! An install is staged beside the agent's folder and published by a rename,
//! and an old folder is renamed aside before it is deleted, so an interrupted
//! install or reset never leaves a folder that only looks complete.

use kuvo_os::{
    platform::Environment,
    process::{Spawn, Stdio},
};
use serde_json::{Map, Value, json};
use std::{
    collections::HashMap,
    ffi::OsString,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub version: String,
}

/// The program that installs: the engine's Bun (`BUN_BE_BUN=1`).
#[derive(Clone, Debug)]
pub struct Installer {
    pub program: PathBuf,
    /// Arguments before Bun's own (none for Bun itself).
    pub leading: Vec<OsString>,
    /// The environment it runs with, before the cache settings.
    pub env: Environment,
}

#[derive(Clone)]
pub struct RuntimePackages {
    root: PathBuf,
    cache: PathBuf,
    installer: Installer,
    /// One install at a time per agent.
    agents: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
    running: Arc<AtomicUsize>,
}

impl RuntimePackages {
    pub fn new(root: PathBuf, cache: PathBuf, installer: Installer) -> Self {
        Self {
            root,
            cache,
            installer,
            agents: Arc::default(),
            running: Arc::default(),
        }
    }

    pub fn folder(&self, agent: &str) -> PathBuf {
        self.root.join(agent)
    }

    /// Whether an agent's packages are installed at their versions.
    pub fn has(&self, agent: &str, packages: &[Package]) -> bool {
        let folder = self.folder(agent);
        packages
            .iter()
            .all(|package| installed_version(&folder, &package.name).as_deref() == Some(&package.version))
    }

    /// Install an agent's packages unless they are already there. Answers
    /// the agent's folder.
    pub async fn ensure(
        &self,
        agent: &str,
        packages: &[Package],
        optional: bool,
    ) -> io::Result<PathBuf> {
        let folder = self.folder(agent);
        if self.has(agent, packages) {
            return Ok(folder);
        }
        let turn = {
            let mut agents = self.agents.lock().expect("the install table is never poisoned");
            agents.entry(agent.to_owned()).or_default().clone()
        };
        let _turn = turn.lock().await;
        // Installed while this one waited for its turn.
        if self.has(agent, packages) {
            return Ok(folder);
        }
        self.running.fetch_add(1, Ordering::SeqCst);
        let packages_owned = packages.to_vec();
        let this = self.clone();
        let name = agent.to_owned();
        let installed =
            tokio::task::spawn_blocking(move || this.install(&name, &packages_owned, optional))
                .await
                .map_err(io::Error::other)
                .and_then(|result| result);
        // The download cache goes once nothing else installs from it: it holds
        // what the install left out and would grow with every update.
        if self.running.fetch_sub(1, Ordering::SeqCst) == 1 {
            let _ = fs::remove_dir_all(self.cache.join("install"));
        }
        installed.map(|()| folder)
    }

    /// An agent's install, gone, to install again.
    pub fn reset(&self, agent: &str) -> io::Result<()> {
        retire(&self.root, &self.folder(agent))
    }

    fn install(&self, agent: &str, packages: &[Package], optional: bool) -> io::Result<()> {
        fs::create_dir_all(&self.root)?;
        sweep(&self.root, agent);
        let staging = tempfile::Builder::new()
            .prefix(&format!(".{agent}-installing-"))
            .tempdir_in(&self.root)?;
        let dependencies = packages
            .iter()
            .map(|package| (package.name.clone(), Value::from(package.version.clone())))
            .collect::<Map<_, _>>();
        let manifest = json!({
            "name": "kuvo-agents-runtime",
            "private": true,
            "dependencies": dependencies,
        });
        fs::write(
            staging.path().join("package.json"),
            serde_json::to_string_pretty(&manifest)?,
        )?;
        let mut env = self.installer.env.clone();
        env.insert("BUN_BE_BUN".into(), "1".into());
        env.insert("BUN_INSTALL".into(), self.cache.clone().into_os_string());
        env.insert(
            "BUN_INSTALL_CACHE_DIR".into(),
            self.cache.join("install").into_os_string(),
        );
        let mut spawn = Spawn::new(self.installer.program.clone(), env);
        spawn.args = self.installer.leading.clone();
        spawn
            .args
            .extend(["install", "--no-save", "--no-progress", "--silent"].map(OsString::from));
        if !optional {
            spawn.args.extend(["--omit", "optional"].map(OsString::from));
        }
        spawn.cwd = Some(staging.path().to_owned());
        spawn.stderr = Stdio::Pipe;
        let mut child = spawn.spawn()?;
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            stderr = String::from_utf8_lossy(&bytes).into_owned();
        }
        let status = child.wait()?;
        if !status.success() {
            let names = packages
                .iter()
                .map(|package| package.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let said = stderr.trim();
            let tail = &said[said.len() - last_chars(said, 400)..];
            return Err(io::Error::other(format!(
                "Couldn't download what the agent needs ({names}): {tail}"
            )));
        }
        let folder = self.folder(agent);
        retire(&self.root, &folder)?;
        fs::rename(staging.keep(), &folder)
    }
}

/// The byte length of the last `count` characters of `text`.
fn last_chars(text: &str, count: usize) -> usize {
    text.char_indices()
        .rev()
        .nth(count.saturating_sub(1))
        .map_or(text.len(), |(at, _)| text.len() - at)
}

fn installed_version(folder: &Path, name: &str) -> Option<String> {
    let file = folder.join("node_modules").join(name).join("package.json");
    let bytes = fs::read(file).ok()?;
    let manifest: Value = serde_json::from_slice(&bytes).ok()?;
    manifest["version"].as_str().map(str::to_owned)
}

/// Move a folder aside, then delete it.
fn retire(root: &Path, folder: &Path) -> io::Result<()> {
    let name = folder
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("agent");
    let aside = match tempfile::Builder::new()
        .prefix(&format!(".{name}-retired-"))
        .tempdir_in(root)
    {
        Ok(aside) => aside.keep(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let target = aside.join(name);
    match fs::rename(folder, &target) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let _ = fs::remove_dir(&aside);
            return Ok(());
        }
        Err(error) => {
            let _ = fs::remove_dir(&aside);
            return Err(error);
        }
    }
    fs::remove_dir_all(aside)
}

/// What an interrupted install or reset of `agent` left behind.
fn sweep(root: &Path, agent: &str) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with(&format!(".{agent}-installing-"))
            || name.starts_with(&format!(".{agent}-retired-"))
        {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}
