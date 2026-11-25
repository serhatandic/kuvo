//! Keeping each coding agent's own program current: which version you have, the newest one, and
//! the way to update it, through the installer that owns it (a Homebrew keg or cask by that
//! Homebrew, an npm global by the npm of that prefix, an agent's own installer by its own
//! updater). One under someone else's `node_modules` is left to you. Nothing runs until asked, and
//! an update counts only if the version changed.
//!
//! Its update is the box's to keep for every client: under way, updated to a version, already the
//! newest, or why it couldn't be, by program (one program serves every account with it). While it
//! runs, its program's turns are held (`Hold`), and a record of it (`agents/program-draining`)
//! lets the next resident take it up.

mod versions;

use std::{
    collections::HashMap,
    ffi::OsString,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{Arc, Mutex as Plain, OnceLock},
    time::{Duration, Instant},
};

use futures_util::future::{BoxFuture, FutureExt, Shared};
use kuvo_os::process::{Spawn, Stdio};
use kuvo_store::files;
use serde::{Deserialize, Serialize};
pub use versions::{compare_versions, homebrew_keg, npm_global_prefix, version_in};

pub mod notices;

/// How often what you have is read again, unless asked anew.
const CHECKED_FOR: Duration = Duration::from_secs(10 * 60);
/// How long npm's newest is kept.
const NEWEST_FOR: Duration = Duration::from_secs(6 * 60 * 60);
/// An installer's longest run.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// How long what an update brought is news. Why one failed stays until it's seen.
const NEWS_KEPT: u64 = 60 * 60_000;
/// The longest a program's update holds its turns: as long as an installer may run, and a little.
pub const HOLD_MOST: u64 = 15 * 60_000;

/// An agent's program, as its maintenance sees it.
#[derive(Clone, Debug)]
pub struct Definition {
    /// As people read it: "Codex".
    pub name: String,
    pub executable: PathBuf,
    /// Its npm package, where npm has its releases.
    pub npm_package: Option<String>,
    /// The agent's own updater (`codex update`), and how to tell its own installer put it there.
    pub native: Option<Native>,
}

#[derive(Clone, Debug)]
pub struct Native {
    pub args: Vec<String>,
    /// What paths of its own installer's have: `/.codex/`.
    pub markers: Vec<String>,
    /// The npm dist-tag its updater follows, when not `latest`.
    pub dist_tag: Option<String>,
}

impl Native {
    fn is_native(&self, path: &Path) -> bool {
        let path = path.to_string_lossy();
        self.markers.iter().any(|marker| path.contains(marker.as_str()))
    }
}

/// What you have, the newest, and how to update.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Maintenance {
    pub installed: Option<String>,
    pub latest: Option<String>,
    /// What updating runs; none when it's yours to do.
    pub command: Option<Vec<String>>,
    /// That command as you'd type it.
    pub shown: Option<String>,
    pub env: Vec<(String, String)>,
}

impl Maintenance {
    /// A newer version than yours.
    pub fn is_outdated(&self) -> bool {
        matches!((&self.installed, &self.latest), (Some(installed), Some(latest))
            if compare_versions(installed, latest).is_lt())
    }
}

/// How an update went.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpdateResult {
    pub updated: bool,
    pub version: Option<String>,
    /// What went wrong, in the program's words (its last lines).
    pub message: Option<String>,
}

impl UpdateResult {
    fn failed(message: impl Into<String>) -> Self {
        Self {
            message: Some(message.into()),
            ..Self::default()
        }
    }
}

fn lock<T>(plain: &Plain<T>) -> std::sync::MutexGuard<'_, T> {
    plain.lock().unwrap_or_else(|error| error.into_inner())
}

fn now_ms() -> u64 {
    crate::background::now()
}

/// What each program was found to be, and npm's newest, kept a while.
#[derive(Default)]
struct Known {
    checked: HashMap<PathBuf, (Instant, Maintenance)>,
    newest: HashMap<String, (Instant, Option<String>)>,
}

fn known() -> &'static Plain<Known> {
    static KNOWN: OnceLock<Plain<Known>> = OnceLock::new();
    KNOWN.get_or_init(Plain::default)
}

/// What you have, the newest, and how to update, asked at most every ten minutes unless `fresh`.
pub async fn maintenance(definition: &Definition, fresh: bool) -> Maintenance {
    if !fresh
        && let Some((at, value)) = lock(known()).checked.get(&definition.executable)
        && at.elapsed() < CHECKED_FOR
    {
        return value.clone();
    }
    let (installed, updater) = tokio::join!(
        installed_version(&definition.executable),
        resolve_updater(definition)
    );
    // Homebrew's own newest, or none known: npm's may not have reached it.
    let latest = match (&updater.homebrew, &updater.latest, &definition.npm_package) {
        (true, latest, _) => latest.clone(),
        (false, Some(latest), _) => Some(latest.clone()),
        (false, None, Some(package)) => {
            npm_latest(package, fresh, updater.dist_tag.as_deref()).await
        }
        _ => None,
    };
    let value = Maintenance {
        installed,
        latest,
        command: updater.command,
        shown: updater.shown,
        env: updater.env,
    };
    lock(known()).checked.insert(
        definition.executable.clone(),
        (Instant::now(), value.clone()),
    );
    value
}

/// What a program says it is (`--version`).
pub async fn installed_version(executable: &Path) -> Option<String> {
    let ran = run(
        executable,
        &["--version".into()],
        &[],
        None,
        Duration::from_secs(15),
    )
    .await?;
    version_in(&format!("{} {}", ran.stdout, ran.stderr))
}

/// The newest release on npm (on a dist-tag: a channel).
async fn npm_latest(package: &str, fresh: bool, tag: Option<&str>) -> Option<String> {
    let tag = tag.unwrap_or("latest");
    let key = format!("{package}@{tag}");
    let kept = lock(known()).newest.get(&key).cloned();
    if !fresh
        && let Some((at, value)) = &kept
        && at.elapsed() < NEWEST_FOR
    {
        return value.clone();
    }
    // (Tests name a registry of their own.)
    let registry = std::env::var("KUVO_TEST_NPM_REGISTRY")
        .ok()
        .filter(|registry| !registry.is_empty())
        .unwrap_or_else(|| "https://registry.npmjs.org".into());
    let url = format!(
        "{}/{}/{tag}",
        registry.trim_end_matches('/'),
        package.replace('/', "%2F")
    );
    let read = async {
        let response = reqwest::Client::new()
            .get(url)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return Some(None);
        }
        let body: serde_json::Value = response.json().await.ok()?;
        Some(body["version"].as_str().map(str::to_owned))
    };
    match read.await {
        Some(value) => {
            lock(known())
                .newest
                .insert(key, (Instant::now(), value.clone()));
            value
        }
        None => kept.and_then(|(_, value)| value),
    }
}

/// How a program is updated, as found from where it really is.
#[derive(Default)]
struct Updater {
    command: Option<Vec<String>>,
    shown: Option<String>,
    env: Vec<(String, String)>,
    /// What that installer can deliver, when it isn't npm's newest (Homebrew trails it).
    latest: Option<String>,
    homebrew: bool,
    dist_tag: Option<String>,
}

async fn resolve_updater(definition: &Definition) -> Updater {
    let executable = &definition.executable;
    let real = std::fs::canonicalize(executable).unwrap_or_else(|_| executable.clone());
    let name = executable
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    let native = definition.native.as_ref().map(|native| Updater {
        command: Some(
            std::iter::once(executable.to_string_lossy().into_owned())
                .chain(native.args.iter().cloned())
                .collect(),
        ),
        shown: Some(
            std::iter::once(name.clone())
                .chain(native.args.iter().cloned())
                .collect::<Vec<_>>()
                .join(" "),
        ),
        dist_tag: native.dist_tag.clone(),
        ..Updater::default()
    });
    if let Some(found) = &definition.native
        && (found.is_native(&real) || found.is_native(executable))
    {
        return native.unwrap_or_default();
    }
    // npm names the package, so it outranks a Homebrew keg the path passes through.
    if let Some(package) = &definition.npm_package
        && let Some(prefix) = npm_global_prefix(&real.to_string_lossy(), package)
    {
        let prefix = PathBuf::from(prefix);
        let own = prefix.join("bin").join("npm");
        let npm = if own.exists() {
            own.to_string_lossy().into_owned()
        } else {
            "npm".into()
        };
        let path = std::env::var("PATH").unwrap_or_default();
        return Updater {
            command: Some(vec![
                npm,
                "install".into(),
                "-g".into(),
                "--prefix".into(),
                prefix.to_string_lossy().into_owned(),
                format!("--allow-scripts={package}"),
                format!("{package}@latest"),
            ]),
            shown: Some(format!("npm install -g {package}@latest")),
            // That npm's own Node first.
            env: vec![(
                "PATH".into(),
                format!("{}:{path}", prefix.join("bin").display()),
            )],
            ..Updater::default()
        };
    }
    if let Some(keg) = homebrew_keg(&real.to_string_lossy())
        && !keg.name.eq_ignore_ascii_case("mise")
        && let Some(updater) = homebrew(&keg).await
    {
        return updater;
    }
    // Under someone else's node_modules (a project's, a version manager's): yours to update.
    let shown = real.to_string_lossy();
    if shown.contains("/node_modules/") || shown.contains("/mise/") {
        return Updater::default();
    }
    native.unwrap_or_default()
}

/// The Homebrew that owns `keg`, its upgrade and the newest it has.
async fn homebrew(keg: &versions::Keg) -> Option<Updater> {
    let own = Path::new(&keg.prefix).join("bin").join("brew");
    let brew = if own.exists() { own } else { which("brew")? };
    let said = run(
        &brew,
        &["--prefix".into()],
        &[],
        None,
        Duration::from_secs(15),
    )
    .await?;
    let said = said.stdout.trim().to_owned();
    let prefix =
        std::fs::canonicalize(&said).map_or(said, |path| path.to_string_lossy().into_owned());
    if prefix != keg.prefix {
        return None;
    }
    let cask = keg.cask;
    let mut args: Vec<OsString> = vec!["info".into(), "--json=v2".into()];
    if cask {
        args.push("--cask".into());
    }
    args.push(keg.name.clone().into());
    let env = [("HOMEBREW_NO_AUTO_UPDATE".to_owned(), "1".to_owned())];
    let info = run(&brew, &args, &env, None, Duration::from_secs(30)).await;
    let mut command = vec![brew.to_string_lossy().into_owned(), "upgrade".into()];
    if cask {
        command.push("--cask".into());
    }
    command.push(keg.name.clone());
    Some(Updater {
        command: Some(command),
        shown: Some(format!(
            "brew upgrade {}{}",
            if cask { "--cask " } else { "" },
            keg.name
        )),
        latest: info.and_then(|info| versions::homebrew_latest(&info.stdout, cask)),
        homebrew: true,
        ..Updater::default()
    })
}

fn which(name: &str) -> Option<PathBuf> {
    let env: kuvo_os::platform::Environment = std::env::vars_os().collect();
    kuvo_os::platform::which(name.as_ref(), &env, &[]).map(|found| found.path().to_owned())
}

/// What a program printed.
struct Ran {
    stdout: String,
    stderr: String,
}

/// Runs `program` as a group of its own, at most `timeout` (then its group is stopped). None when
/// it couldn't start.
async fn run(
    program: &Path,
    args: &[OsString],
    env: &[(String, String)],
    cwd: Option<&Path>,
    timeout: Duration,
) -> Option<Ran> {
    run_recorded(program, args, env, cwd, timeout, |_| {}).await
}

async fn run_recorded(
    program: &Path,
    args: &[OsString],
    env: &[(String, String)],
    cwd: Option<&Path>,
    timeout: Duration,
    started: impl FnOnce(u32) + Send + 'static,
) -> Option<Ran> {
    let mut environment: kuvo_os::platform::Environment = std::env::vars_os().collect();
    for (name, value) in env {
        environment.insert(name.into(), value.into());
    }
    // (A bare name, `npm`, as your PATH finds it.)
    let executable = kuvo_os::platform::which(program.as_os_str(), &environment, &[])?;
    let mut spawn = Spawn::new(program, environment);
    spawn.executable = executable;
    spawn.args = args.to_vec();
    spawn.cwd = cwd.map(Path::to_path_buf);
    spawn.stdout = Stdio::Pipe;
    spawn.stderr = Stdio::Pipe;
    tokio::task::spawn_blocking(move || {
        let mut child = spawn.spawn().ok()?;
        started(child.identity().pid);
        let read = |pipe: Option<io::PipeReader>| {
            std::thread::spawn(move || {
                let mut text = String::new();
                if let Some(mut pipe) = pipe {
                    let mut bytes = Vec::new();
                    let _ = pipe.read_to_end(&mut bytes);
                    text = String::from_utf8_lossy(&bytes).into_owned();
                }
                text
            })
        };
        let (stdout, stderr) = (read(child.stdout.take()), read(child.stderr.take()));
        let deadline = Instant::now() + timeout;
        while !child.has_exited().unwrap_or(true) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = child.stop(Duration::from_secs(3), |_| {});
        Some(Ran {
            stdout: stdout.join().unwrap_or_default(),
            stderr: stderr.join().unwrap_or_default(),
        })
    })
    .await
    .ok()
    .flatten()
}

/// The last lines of what an installer printed, as words.
fn last_lines(ran: &Ran) -> String {
    let text = format!("{}\n{}", ran.stderr, ran.stdout);
    let plain = versions::plain(&text);
    let lines: Vec<&str> = plain.trim().lines().collect();
    lines[lines.len().saturating_sub(6)..]
        .join("\n")
        .trim()
        .to_owned()
}

/// Where an installer under way says which group runs it, for whom.
fn install_marker(agents: &Path, program: &str) -> PathBuf {
    agents.join("program-installing").join(program)
}

fn drain_file(agents: &Path, program: &str) -> PathBuf {
    agents
        .join("program-draining")
        .join(format!("{program}.json"))
}

/// Whether the installer for `attempt` had started (its group is recorded).
fn installer_started(agents: &Path, program: &str, attempt: &str) -> bool {
    std::fs::read_to_string(install_marker(agents, program)).is_ok_and(|text| {
        let parts: Vec<&str> = text.split(' ').collect();
        parts.get(2) == Some(&attempt) && parts[0].parse::<u32>().is_ok_and(|pid| pid > 0)
    })
}

/// Retires only the marker of `attempt`.
fn finish_installation(agents: &Path, program: &str, attempt: &str) {
    let marker = install_marker(agents, program);
    if std::fs::read_to_string(&marker).is_ok_and(|text| text.split(' ').nth(2) == Some(attempt)) {
        let _ = std::fs::remove_file(marker);
    }
}

/// One update at a time for each program, by its name.
fn one_at_a_time(name: &str) -> Arc<tokio::sync::Mutex<()>> {
    static RUNNING: OnceLock<Plain<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    lock(RUNNING.get_or_init(Plain::default))
        .entry(name.to_owned())
        .or_default()
        .clone()
}

/// How an installer an earlier resident started stands, by its record.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Installation {
    Running,
    Ended,
}

/// A detached installer can outlive the resident that started it: its record holds turns until
/// it exits (or, one stopped before its group was recorded, for the installer's longest run).
pub fn installation(agents: &Path, program: &str) -> Option<Installation> {
    let named = program.starts_with(|c: char| c.is_ascii_lowercase())
        && program
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !named {
        return None;
    }
    let text = std::fs::read_to_string(install_marker(agents, program)).ok()?;
    let mut parts = text.split(' ').map(|part| part.parse::<u64>().ok());
    let (pid, until) = (parts.next().flatten()?, parts.next().flatten()?);
    if pid == 0 {
        return Some(if now_ms() < until {
            Installation::Running
        } else {
            Installation::Ended
        });
    }
    let pid = u32::try_from(pid).ok().filter(|pid| *pid >= 2)?;
    // (Its leader, as the record names it.)
    Some(match kuvo_os::process::ProcessIdentity::read(pid) {
        Ok(None) => Installation::Ended,
        _ => Installation::Running,
    })
}

/// Runs the update and says whether the version changed. `install`: where it says its group runs
/// (the agents' folder, its program and attempt).
pub async fn update(
    definition: &Definition,
    temporary: &Path,
    install: Option<(&Path, &str, &str)>,
) -> UpdateResult {
    let one = one_at_a_time(&definition.name);
    let _one = one.lock().await;
    let before = maintenance(definition, true).await;
    let Some(command) = before.command.clone().filter(|command| !command.is_empty()) else {
        return UpdateResult {
            version: before.installed,
            ..UpdateResult::failed(format!(
                "Update {} the way you installed it.",
                definition.name
            ))
        };
    };
    let marker = install.map(|(agents, program, _)| install_marker(agents, program));
    if let Some(marker) = &marker {
        let until = now_ms() + INSTALL_TIMEOUT.as_millis() as u64 + 3_000;
        let attempt = install.map_or("", |(_, _, attempt)| attempt);
        let _ = files::write_secret(marker, format!("0 {until} {attempt}").as_bytes());
    }
    let (recorded, attempt) = (
        marker.clone(),
        install.map_or(String::new(), |(_, _, attempt)| attempt.to_owned()),
    );
    let args: Vec<OsString> = command[1..].iter().map(OsString::from).collect();
    // (Run where Kuvo keeps its scratch, as you'd run it: none of your projects.)
    let _ = files::private_folder(temporary);
    let ran = run_recorded(
        Path::new(&command[0]),
        &args,
        &before.env,
        Some(temporary),
        INSTALL_TIMEOUT,
        move |pid| {
            if let Some(marker) = recorded {
                let _ = files::write_secret(&marker, format!("{pid} 0 {attempt}").as_bytes());
            }
        },
    )
    .await;
    if let Some(marker) = &marker
        && install.is_some_and(|(_, _, attempt)| attempt.is_empty())
    {
        let _ = std::fs::remove_file(marker);
    }
    let after = maintenance(definition, true).await;
    let changed = after.installed.is_some() && after.installed != before.installed;
    // Unchanged, it's up to date only if the newest is known to be what it has.
    if changed || (after.installed.is_some() && after.latest.is_some() && !after.is_outdated()) {
        return UpdateResult {
            updated: changed,
            version: after.installed,
            message: None,
        };
    }
    let output = ran.as_ref().map(last_lines).unwrap_or_default();
    let still = after.installed.as_deref().unwrap_or("the same version");
    let shown = before.shown.as_deref().unwrap_or_default();
    let mut message = format!("{} is still {still} after “{shown}”.", definition.name);
    if !output.is_empty() {
        message.push_str(&format!("\n\n{output}"));
    }
    UpdateResult {
        updated: false,
        version: after.installed,
        message: Some(message),
    }
}

/// How an install went.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct InstallResult {
    pub installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// What went wrong, in the installer's words (its last lines).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Installs an agent's program with its maker's own installer (`curl … | sh`), run as you'd run
/// it in Terminal: it goes where that installer puts it. Once at a time per program, and not while
/// it updates; then whether `found` finds it. One that's there already isn't installed again.
pub async fn install(
    name: &str,
    command: &str,
    temporary: &Path,
    found: impl Fn() -> Option<PathBuf>,
) -> InstallResult {
    let one = one_at_a_time(name);
    let _one = one.lock().await;
    if let Some(there) = found() {
        return InstallResult {
            installed: true,
            version: installed_version(&there).await,
            message: None,
        };
    }
    let _ = files::private_folder(temporary);
    let ran = run(
        Path::new("/bin/sh"),
        &["-c".into(), command.into()],
        &[],
        Some(temporary),
        INSTALL_TIMEOUT,
    )
    .await;
    if let Some(executable) = found() {
        return InstallResult {
            installed: true,
            version: installed_version(&executable).await,
            message: None,
        };
    }
    let output = ran.as_ref().map(last_lines).unwrap_or_default();
    let mut message = format!("{name} isn't installed after “{command}”.");
    if !output.is_empty() {
        message.push_str(&format!("\n\n{output}"));
    }
    InstallResult {
        installed: false,
        version: None,
        message: Some(message),
    }
}

/// How a program's update stands, by when it last changed (`at`, its id for a client's "seen").
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum ProgramUpdate {
    Updating {
        at: u64,
    },
    /// Updated just now.
    Updated {
        version: String,
        at: u64,
    },
    /// Asked to, and already on the newest.
    Current {
        version: String,
        at: u64,
    },
    /// Why it couldn't be updated, and the version it was left on.
    Failed {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        version: Option<String>,
        at: u64,
    },
}

impl ProgramUpdate {
    fn at(&self) -> u64 {
        match self {
            Self::Updating { at }
            | Self::Updated { at, .. }
            | Self::Current { at, .. }
            | Self::Failed { at, .. } => *at,
        }
    }

    fn version(&self) -> Option<&str> {
        match self {
            Self::Updated { version, .. } | Self::Current { version, .. } => Some(version),
            Self::Failed { version, .. } => version.as_deref(),
            Self::Updating { .. } => None,
        }
    }

    fn of(result: &UpdateResult) -> Self {
        let at = now_ms();
        match (&result.message, result.updated, &result.version) {
            (Some(message), false, version) => Self::Failed {
                message: message.clone(),
                version: version.clone(),
                at,
            },
            (_, updated, Some(version)) => {
                let version = version.clone();
                if updated {
                    Self::Updated { version, at }
                } else {
                    Self::Current { version, at }
                }
            }
            _ => Self::Failed {
                message: "The update finished without saying which version it left.".into(),
                version: None,
                at,
            },
        }
    }
}

/// A program's update as its record keeps it, for a next resident.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Drain {
    pub attempt: String,
    pub provider: String,
    pub deadline: u64,
}

/// A program an update was asked for: what it's called, how to update it, and what's done around
/// it (the box's half, its agents' and its sessions').
pub trait Program: Send + Sync {
    /// One program serves every account with it: `codex`.
    fn program(&self) -> &str;
    /// The account the update was asked from.
    fn provider(&self) -> String;
    /// As people read it: "Codex".
    fn name(&self) -> String;
    /// An account added to its agent, not its own sign-in: asked after it in Update All.
    fn added(&self) -> bool {
        false
    }
    fn definition(&self) -> Option<Definition>;
    /// Each account with it runs the new program.
    fn updated(&self) {}
}

/// Holds a program's turns while it updates; the release lets them go.
pub trait Hold: Send + Sync {
    fn hold<'a>(
        &'a self,
        program: &'a str,
        deadline: u64,
    ) -> BoxFuture<'a, Result<Box<dyn FnOnce() + Send>, String>>;
}

type Running = Shared<BoxFuture<'static, UpdateResult>>;
/// Told a program's update started (`None`) or how it ended.
pub type Changed = Arc<dyn Fn(&str, Option<&UpdateResult>) + Send + Sync>;
/// Every account here, as each update asks.
pub type Accounts = Arc<dyn Fn() -> Vec<Arc<dyn Program>> + Send + Sync>;

/// The updates of the agents' programs here.
#[derive(Clone)]
pub struct ProgramUpdates {
    agents: PathBuf,
    temporary: PathBuf,
    kept: Arc<Plain<HashMap<String, ProgramUpdate>>>,
    running: Arc<Plain<HashMap<String, Running>>>,
    hold: Option<Arc<dyn Hold>>,
    accounts: Accounts,
    /// Told that a program's update changed (started, or ended so): its accounts' rows say so.
    changed: Changed,
}

impl ProgramUpdates {
    /// The updates for a box at `root`.
    pub fn at(root: &Path, accounts: Accounts, changed: Changed) -> Self {
        Self {
            agents: root.join("agents"),
            temporary: root.join("tmp"),
            kept: Arc::default(),
            running: Arc::default(),
            hold: None,
            accounts,
            changed,
        }
    }

    pub fn with_hold(mut self, hold: Arc<dyn Hold>) -> Self {
        self.hold = Some(hold);
        self
    }

    /// Whether its program is updating now.
    pub fn is_updating(&self, program: &str) -> bool {
        lock(&self.running).contains_key(program)
    }

    /// Whether any is: a newer resident waits for it.
    pub fn any_updating(&self) -> bool {
        !lock(&self.running).is_empty()
    }

    /// How its update stands, on `installed`: one about an older version, or an hour old (unless
    /// it failed), is no news.
    pub fn update_of(&self, program: &str, installed: Option<&str>) -> Option<ProgramUpdate> {
        let mut kept = lock(&self.kept);
        let update = kept.get(program)?.clone();
        if matches!(update, ProgramUpdate::Updating { .. }) {
            return Some(update);
        }
        let moved_on = matches!((installed, update.version()), (Some(installed), Some(version))
            if compare_versions(installed, version).is_gt());
        let old = !matches!(update, ProgramUpdate::Failed { .. })
            && now_ms().saturating_sub(update.at()) > NEWS_KEPT;
        if !moved_on && !old {
            return Some(update);
        }
        kept.remove(program);
        None
    }

    /// Its outcome seen (`at`: that one), for every client. Whether there was one.
    pub fn forget(&self, program: &str, at: Option<u64>) -> bool {
        let mut kept = lock(&self.kept);
        let Some(update) = kept.get(program) else {
            return false;
        };
        if matches!(update, ProgramUpdate::Updating { .. })
            || at.is_some_and(|at| update.at() != at)
        {
            return false;
        }
        kept.remove(program);
        true
    }

    /// Updates `agent`'s program, or joins the update of it under way: its accounts say it's
    /// updating until it's done, then how it went. Never fails: what went wrong is its outcome.
    pub fn update(&self, agent: Arc<dyn Program>, saved: Option<Drain>) -> Running {
        let program = agent.program().to_owned();
        if let Some(running) = lock(&self.running).get(&program) {
            return running.clone();
        }
        let drain = saved.clone().unwrap_or_else(|| Drain {
            attempt: uuid::Uuid::new_v4().to_string(),
            provider: agent.provider(),
            deadline: now_ms() + HOLD_MOST,
        });
        let file = drain_file(&self.agents, &program);
        if saved.is_none()
            && let Ok(bytes) = serde_json::to_vec(&drain)
        {
            let _ = files::write_secret(&file, &bytes);
        }
        lock(&self.kept).insert(
            program.clone(),
            ProgramUpdate::Updating {
                at: drain.deadline - HOLD_MOST,
            },
        );
        let this = self.clone();
        let (name, attempt) = (program.clone(), drain.attempt.clone());
        let done = async move {
            let release = match &this.hold {
                Some(hold) => match hold.hold(&name, drain.deadline).await {
                    Ok(release) => Some(release),
                    Err(message) => {
                        let result = UpdateResult::failed(message);
                        this.finish(&name, &attempt, &result, None);
                        return result;
                    }
                },
                None => None,
            };
            let result = match agent.definition() {
                Some(definition) => {
                    let result = update(
                        &definition,
                        &this.temporary,
                        Some((&this.agents, &name, &attempt)),
                    )
                    .await;
                    // Every account with it runs that program.
                    if result.updated {
                        for each in (this.accounts)() {
                            if each.program() == name {
                                each.updated();
                            }
                        }
                    }
                    result
                }
                None => UpdateResult::failed(format!("{} isn't installed.", agent.name())),
            };
            this.finish(&name, &attempt, &result, release);
            result
        }
        .boxed()
        .shared();
        lock(&self.running).insert(program.clone(), done.clone());
        (self.changed)(&program, None);
        tokio::spawn(done.clone());
        done
    }

    fn finish(
        &self,
        program: &str,
        attempt: &str,
        result: &UpdateResult,
        release: Option<Box<dyn FnOnce() + Send>>,
    ) {
        lock(&self.kept).insert(program.to_owned(), ProgramUpdate::of(result));
        lock(&self.running).remove(program);
        let _ = std::fs::remove_file(drain_file(&self.agents, program));
        finish_installation(&self.agents, program, attempt);
        if let Some(release) = release {
            release();
        }
        (self.changed)(program, Some(result));
    }

    /// The updates an earlier resident left under way: taken up, before turns are let in; one
    /// whose installer had started is its own (its group holds turns until it exits).
    pub fn resume(&self) {
        let programs = (self.accounts)();
        let folder = self.agents.join("program-draining");
        let Ok(names) = std::fs::read_dir(&folder) else {
            return;
        };
        for entry in names.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(program) = name.strip_suffix(".json").filter(|program| {
                program
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                    && !program.is_empty()
            }) else {
                continue;
            };
            let Ok(drain) = std::fs::read(entry.path())
                .map_err(drop)
                .and_then(|bytes| serde_json::from_slice::<Drain>(&bytes).map_err(drop))
            else {
                continue;
            };
            let attempt = &drain.attempt;
            if attempt.len() != 36
                || !attempt
                    .bytes()
                    .all(|b| b == b'-' || b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                continue;
            }
            if installer_started(&self.agents, program, &drain.attempt) {
                let _ = std::fs::remove_file(entry.path());
                continue;
            }
            let candidates: Vec<_> = programs
                .iter()
                .filter(|each| each.program() == program && each.definition().is_some())
                .collect();
            let agent = candidates
                .iter()
                .find(|each| each.provider() == drain.provider)
                .or(candidates.first());
            match agent {
                Some(agent) => {
                    // (It runs on its own: nothing here waits for it.)
                    drop(self.update((*agent).clone(), Some(drain)));
                }
                None => {
                    let failed =
                        UpdateResult::failed("This program can no longer be updated here.");
                    lock(&self.kept).insert(program.to_owned(), ProgramUpdate::of(&failed));
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }

    /// Update All: each program a newer version is out for that its installer can update, once,
    /// asked of an agent's own sign-in before the accounts added to it. Started, not waited for:
    /// the programs it started.
    pub async fn update_all(&self) -> Vec<String> {
        let programs = (self.accounts)();
        let mut by_program: Vec<(String, Vec<Arc<dyn Program>>)> = Vec::new();
        // An agent's own sign-in ahead of the accounts added to it.
        let (own, added): (Vec<_>, Vec<_>) = programs.iter().partition(|agent| !agent.added());
        for agent in own.into_iter().chain(added) {
            if agent.definition().is_none() {
                continue;
            }
            match by_program
                .iter_mut()
                .find(|(name, _)| name == agent.program())
            {
                Some((_, each)) => each.push(agent.clone()),
                None => by_program.push((agent.program().to_owned(), vec![agent.clone()])),
            }
        }
        let mut started = Vec::new();
        for (program, candidates) in by_program {
            if self.is_updating(&program) {
                continue;
            }
            for agent in candidates {
                let Some(definition) = agent.definition() else {
                    continue;
                };
                let found = maintenance(&definition, false).await;
                if found.command.is_some() && found.is_outdated() && !self.is_updating(&program) {
                    drop(self.update(agent, None));
                    started.push(program.clone());
                    break;
                }
            }
        }
        started
    }
}

/// An account here, and its agent's program.
impl Program for crate::roster::Entry {
    fn program(&self) -> &str {
        self.account.kind.name()
    }

    fn provider(&self) -> String {
        self.account.provider().to_string()
    }

    fn name(&self) -> String {
        self.account.kind.display_name().into()
    }

    fn added(&self) -> bool {
        self.account.provider().account != "default"
    }

    fn definition(&self) -> Option<Definition> {
        self.agent.program()
    }

    fn updated(&self) {
        let agent = self.agent.clone();
        tokio::spawn(async move {
            if let Err(error) = agent.program_updated().await {
                eprintln!("Kuvo could not restart an updated agent: {error}");
            }
        });
    }
}

#[cfg(test)]
mod tests;
