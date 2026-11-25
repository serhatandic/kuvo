//! git as the box runs it in a repository (`shared/git.ts`, PROTOCOL.md "Git"): a repository's own
//! settings (its `.git/config`, and what that includes) never start a program on their own, since
//! whoever could write in the folder could have put one there.
//!
//! Every git the box runs gets [`EVERY`]: no fsmonitor program, pager, external diff or signature
//! check (on a Mac git's own watcher, `core.fsmonitor=true`, stays on where it's set). What the box
//! does by itself ([`own_args`]) also gets [`OWN`]: no hooks, signing or credential helpers, no bare
//! repository found by sitting in its folder, and the filters and diff converters the repository
//! names switched off, but for git-lfs's and git-crypt's as installed. What you ask for yourself
//! ([`asked_args`]) runs its hooks, filters and sign-ins as your terminal would. Neither runs where
//! the repository's `commondir` leads git to another repository's settings and hooks.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::files::{real, resolved};

use super::trees::open_plain;

/// What every git the box runs gets.
pub const EVERY: [&str; 6] = [
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.pager=cat",
    "-c",
    "log.showSignature=false",
];

/// And what the box runs by itself: [`EVERY`] first, then these.
pub const OWN: [&str; 18] = [
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.pager=cat",
    "-c",
    "log.showSignature=false",
    "-c",
    "safe.bareRepository=explicit",
    "-c",
    "core.hooksPath=/dev/null",
    "-c",
    "commit.gpgSign=false",
    "-c",
    "tag.gpgSign=false",
    "-c",
    "credential.helper=",
    "-c",
    "core.askPass=",
];

/// Never asking anything: set on every git the box runs, these included.
pub const GIT_QUIET: (&str, &str) = ("GIT_TERMINAL_PROMPT", "0");

/// The settings naming a program git would run for the box: a file's filter and diff converter (by
/// its attributes), a hook named in settings, and the fsmonitor. (A diff program is never run:
/// diffs get `--no-ext-diff`. A merge program runs only in merges you ask for.)
const DRIVERS: &str =
    r"^(filter\..+\.(clean|smudge|process)|diff\..+\.textconv|hook\..+\.command|core\.fsmonitor)$";

/// git's own watcher, kept on where settings ask for it. Only on a Mac: Linux's git has none, and
/// one before 2.36 reads `true` as a program by that name.
const WATCHER: &[&str] = if cfg!(target_os = "macos") {
    &["-c", "core.fsmonitor=true"]
} else {
    &[]
};

/// The tools whose filters and converters stay on when they're the ones installed.
const TOOLS: [&str; 2] = ["git-lfs", "git-crypt"];

/// Options git takes before its command that are followed by a value.
const VALUED: [&str; 7] = [
    "-C",
    "-c",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--config-env",
    "--exec-path",
];

/// Commands that only read names and objects, so nothing settings name runs and they needn't be
/// read first. (Not `write-tree`, which writes the index: a hook named in settings runs, and a
/// file that looks changed since goes through its filter.)
const PLAIN: [&str; 14] = [
    "rev-parse",
    "for-each-ref",
    "rev-list",
    "config",
    "merge-base",
    "show-ref",
    "check-ignore",
    "ls-tree",
    "ls-files",
    "commit-tree",
    "pack-objects",
    "index-pack",
    "count-objects",
    "var",
];

/// Commands that convert a file for showing and take `--no-textconv`; those that can't be told not to.
const CONVERTS: [&str; 3] = ["blame", "annotate", "grep"];
const CONVERTING: [&str; 3] = ["cat-file", "format-patch", "range-diff"];

/// Commands that make a repository rather than work in one.
const MAKING: [&str; 2] = ["clone", "init"];

/// Commands that reach another repository, each taking it as its first word that isn't an option.
/// What the box runs by itself reaches only a repository at a local path (a backup's store), never
/// a remote its settings name (whose address, upload program or ssh command they choose), and only
/// through git's local transport: a repository's settings can rewrite a path
/// (`url.<x>.insteadOf`) and allow any transport (`protocol.<name>.allow`), which settings on the
/// command line outweigh.
const REACHING: [&str; 7] = [
    "fetch",
    "ls-remote",
    "clone",
    "pull",
    "push",
    "fetch-pack",
    "send-pack",
];
const LOCAL_ONLY: [&str; 14] = [
    "-c",
    "protocol.allow=never",
    "-c",
    "protocol.file.allow=always",
    "-c",
    "protocol.ext.allow=never",
    "-c",
    "protocol.ssh.allow=never",
    "-c",
    "protocol.git.allow=never",
    "-c",
    "protocol.http.allow=never",
    "-c",
    "protocol.https.allow=never",
];

/// How long one git the layer runs may take, with no budget given.
const LOOK_FOR: Duration = Duration::from_secs(30);
/// The most of a `commondir` that's read.
const MOST_NAMED: u64 = 4096;

/// Why a command isn't to be run at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// The repository's `commondir` leads to another's settings and hooks, or it's a bare one a
    /// folder's own files made.
    ElsewhereCommon,
    /// Its settings name a program that can't be switched off (a name with `=`).
    Unswitchable,
    /// Reading its settings failed or stopped.
    Unread,
    /// The budget ran out before the settings were read.
    OutOfTime,
    /// The command converts files for showing and can't be told not to.
    AlwaysConverts,
    /// The command reaches a repository that isn't at a local path, or a remote settings name.
    Remote,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ElsewhereCommon => "the repository's settings lead to another repository's",
            Self::Unswitchable => "the repository names a program that can't be switched off",
            Self::Unread => "the repository's settings couldn't be read",
            Self::OutOfTime => "no time left to read the repository's settings",
            Self::AlwaysConverts => "the command would run the repository's file converters",
            Self::Remote => "the command would reach a repository that isn't at a local path",
        })
    }
}

impl std::error::Error for Refused {}

/// What the layer goes by from the box's own environment: where `PATH` finds git-lfs and
/// git-crypt, and whether `GIT_DIR` names the repository.
#[derive(Debug, Clone, Default)]
pub(crate) struct Here {
    path: Option<OsString>,
    git_dir_named: bool,
}

impl Here {
    fn from_env() -> Self {
        Self {
            path: std::env::var_os("PATH"),
            git_dir_named: std::env::var_os("GIT_DIR").is_some_and(|dir| !dir.is_empty()),
        }
    }
}

/// `args` as the box runs them by itself: [`OWN`], and the programs settings name switched off
/// (read first, unless the command never runs one). `budget`: how long reading may take in all.
pub fn own_args(
    args: &[String],
    cwd: Option<&Path>,
    budget: Option<Duration>,
) -> Result<Vec<String>, Refused> {
    own_args_in(&Here::from_env(), args, cwd, budget)
}

/// `args` as you asked for them, from `cwd`: [`EVERY`], the repository's hooks, filters and
/// sign-ins as they are.
pub fn asked_args(args: &[String], cwd: Option<&Path>) -> Result<Vec<String>, Refused> {
    asked_args_in(&Here::from_env(), args, cwd)
}

/// Whether a Git operation was requested explicitly or is performed by the box itself.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    Own,
    Asked,
    Yours,
}

/// Run through the same argument gate as the rest of the box. No terminal prompts.
pub async fn run(
    folder: &Path,
    args: &[&str],
    intent: Intent,
    env: &[(&str, &str)],
    limit: Duration,
) -> Result<std::process::Output, String> {
    let at = folder.to_owned();
    let mut args = args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
    if kuvo_os::platform::Platform::current() == kuvo_os::platform::Platform::Wsl
        && kuvo_os::platform::is_wsl_windows_drive(folder)
    {
        args.splice(..0, ["-c", "core.filemode=false"].map(str::to_owned));
    }
    let args = tokio::task::spawn_blocking(move || match intent {
        Intent::Own => own_args(&args, Some(&at), None),
        Intent::Asked | Intent::Yours => asked_args(&args, Some(&at)),
    })
    .await.map_err(|error| error.to_string())?
    .map_err(|_| "This repository’s own settings name a program Kuvo Code won’t run for it, or lead git to another repository’s.".to_owned())?;
    let mut command = tokio::process::Command::new("git");
    command
        .args(args)
        .current_dir(folder)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let mut environment = crate::vault::command_environment();
    if intent == Intent::Yours
        && cfg!(target_os = "macos")
        && std::env::var_os("KUVO_TEST_LOGIN_ENV").as_deref() != Some(std::ffi::OsStr::new("off"))
    {
        let original = environment.clone();
        let login = tokio::task::spawn_blocking(move || {
            let account = kuvo_os::platform::Account::current().ok()?;
            kuvo_os::platform::login_environment(
                &account,
                &original,
                Duration::from_secs(3),
                Duration::from_secs(3),
            )
            .ok()
            .flatten()
        })
        .await
        .ok()
        .flatten();
        for (key, value) in login.into_iter().flatten() {
            let name = key.to_string_lossy();
            if !["DYLD_", "BUN_", "KUVO_", "OPENCODE_"]
                .iter()
                .any(|prefix| name.starts_with(prefix))
                && !matches!(
                    name.as_ref(),
                    "NODE_OPTIONS" | "PWD" | "OLDPWD" | "SHLVL" | "_"
                )
            {
                environment.insert(key, value);
            }
        }
    }
    command.env_clear().envs(environment);
    command.env("GIT_TERMINAL_PROMPT", "0");
    if intent == Intent::Own {
        command.env("GIT_OPTIONAL_LOCKS", "0");
    }
    command.envs(env.iter().copied());
    if intent == Intent::Own {
        tokio::time::timeout(limit, command.output())
            .await
            .map_err(|_| "git took too long".to_owned())?
            .map_err(|error| error.to_string())
    } else {
        // Hooks may leave a child which inherits the pipes. Git's exit settles the action.
        use tokio::io::AsyncReadExt;
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        let mut stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");
        let mut out = Vec::new();
        let mut err = Vec::new();
        let status;
        {
            let read =
                async { tokio::join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err)) };
            tokio::pin!(read);
            status = tokio::select! {
                status = child.wait() => {
                    let _ = tokio::time::timeout(Duration::from_millis(100), &mut read).await;
                    status
                }
                _ = &mut read => child.wait().await,
            }
            .map_err(|e| e.to_string())?;
        }
        Ok(std::process::Output {
            status,
            stdout: out,
            stderr: err,
        })
    }
}

pub(crate) fn own_args_in(
    here: &Here,
    args: &[String],
    cwd: Option<&Path>,
    budget: Option<Duration>,
) -> Result<Vec<String>, Refused> {
    let (lead, rest) = split(args);
    let local = reaching(rest).ok_or(Refused::Remote)?;
    let found = if plain(rest) {
        Found::default()
    } else {
        let started = Instant::now();
        // (Whole milliseconds left, as the TS box counts them; none left is under one.)
        let remaining = || match budget {
            None => Ok(LOOK_FOR),
            Some(budget) => {
                let left = budget.saturating_sub(started.elapsed()).as_millis();
                match u64::try_from(left) {
                    Ok(0) => Err(Refused::OutOfTime),
                    Ok(left) => Ok(Duration::from_millis(left)),
                    Err(_) => Ok(Duration::MAX),
                }
            }
        };
        if stray_common(here, lead, rest, cwd, remaining()?) {
            return Err(Refused::ElsewhereCommon);
        }
        read_programs(here, lead, cwd, remaining)?
    };
    let shown = with_options(rest, found.converters).ok_or(Refused::AlwaysConverts)?;
    Ok(owned(lead)
        .chain(OWN.iter().map(|each| (*each).to_owned()))
        .chain(local.iter().map(|each| (*each).to_owned()))
        .chain(found.settings)
        .chain(shown)
        .collect())
}

pub(crate) fn asked_args_in(
    here: &Here,
    args: &[String],
    cwd: Option<&Path>,
) -> Result<Vec<String>, Refused> {
    let (lead, rest) = split(args);
    if stray_common(here, lead, rest, cwd, LOOK_FOR) {
        return Err(Refused::ElsewhereCommon);
    }
    let watcher = if making(rest) || WATCHER.is_empty() {
        &[][..]
    } else {
        // (Whatever it said, as it ended.)
        watcher_in(&git(&watcher_read(lead), cwd, LOOK_FOR).out)
    };
    let shown = with_options(rest, false).unwrap_or_else(|| rest.to_vec());
    Ok(owned(lead)
        .chain(EVERY.iter().map(|each| (*each).to_owned()))
        .chain(watcher.iter().map(|each| (*each).to_owned()))
        .chain(shown)
        .collect())
}

/// The settings a command that reaches another repository gets (none for one that doesn't), or
/// none at all: it reaches one that isn't at a local path, or a remote its settings name.
fn reaching(rest: &[String]) -> Option<&'static [&'static str]> {
    let command = word(rest, 0);
    let words = rest.get(1..).unwrap_or_default();
    let has = |names: &[&str]| words.iter().any(|each| names.contains(&each.as_str()));
    let refused = match command {
        "remote" => has(&["update", "show", "prune", "set-head"]),
        "submodule" => has(&["update"]),
        "request-pull" | "maintenance" => true,
        "archive" => words.iter().any(|each| each.starts_with("--remote")),
        _ => false,
    };
    if refused {
        return None;
    }
    if !REACHING.contains(&command) {
        return Some(&[]);
    }
    let remote = words.iter().find(|each| !each.starts_with('-'))?;
    platform::absolute(remote).then_some(&LOCAL_ONLY[..])
}

fn owned(args: &[String]) -> impl Iterator<Item = String> + '_ {
    args.iter().cloned()
}

/// `args` as git's own options (before its command) and the rest.
fn split(args: &[String]) -> (&[String], &[String]) {
    let mut at = 0;
    while let Some(each) = args.get(at).filter(|each| each.starts_with('-')) {
        at += if VALUED.contains(&each.as_str()) {
            2
        } else {
            1
        };
    }
    args.split_at(at.min(args.len()))
}

fn word(rest: &[String], at: usize) -> &str {
    rest.get(at).map_or("", String::as_str)
}

fn making(rest: &[String]) -> bool {
    MAKING.contains(&word(rest, 0))
}

/// Whether the command never runs a program settings name.
fn plain(rest: &[String]) -> bool {
    let (command, sub) = (word(rest, 0), word(rest, 1));
    let after = rest.get(1..).unwrap_or_default();
    match command {
        "cat-file" => !rest.iter().any(|each| {
            ["--filters", "--textconv", "--path"]
                .iter()
                .any(|option| each.starts_with(option))
        }),
        "ls-files" => !rest.iter().any(|each| asks_modified(each)),
        // Read, not written: `symbolic-ref HEAD`, not `symbolic-ref HEAD refs/heads/x` or `-d`.
        "symbolic-ref" => {
            after.iter().filter(|each| !each.starts_with('-')).count() < 2
                && !rest.iter().any(|each| each == "-d" || each == "--delete")
        }
        "worktree" => sub == "list" || sub == "prune",
        "stash" => sub == "list",
        _ => PLAIN.contains(&command),
    }
}

/// `ls-files -m` (among other short options before it) or `--modified`.
fn asks_modified(option: &str) -> bool {
    if option.starts_with("--modified") {
        return true;
    }
    option.strip_prefix('-').is_some_and(|short| {
        short
            .chars()
            .take_while(char::is_ascii_alphabetic)
            .any(|letter| letter == 'm')
    })
}

/// Where a command showing a diff takes its options (after `diff`, `log`, `show`, `stash show`).
fn showing(rest: &[String]) -> Option<usize> {
    match (word(rest, 0), word(rest, 1)) {
        ("diff" | "log" | "show" | "whatchanged", _) => Some(1),
        ("stash", "show") => Some(2),
        _ => None,
    }
}

/// `rest` with the options a diff it shows takes: no diff program, nor (`converters`) converter.
/// None: it converts files and can't be told not to.
fn with_options(rest: &[String], converters: bool) -> Option<Vec<String>> {
    let no_textconv = converters.then(|| "--no-textconv".to_owned());
    if let Some(at) = showing(rest) {
        let (command, options) = rest.split_at(at);
        return Some(
            command
                .iter()
                .cloned()
                .chain(std::iter::once("--no-ext-diff".to_owned()))
                .chain(no_textconv)
                .chain(options.iter().cloned())
                .collect(),
        );
    }
    if !converters {
        return Some(rest.to_vec());
    }
    let command = word(rest, 0);
    if CONVERTS.contains(&command) {
        let mut out = rest.to_vec();
        out.insert(1, "--no-textconv".to_owned());
        return Some(out);
    }
    (!CONVERTING.contains(&command)).then(|| rest.to_vec())
}

/// What switches off what the settings name: `-c` settings and whether diffs need `--no-textconv`.
#[derive(Debug, Default, PartialEq, Eq)]
struct Found {
    settings: Vec<String>,
    converters: bool,
}

/// The `git config` read of the programs settings name, with each one's scope (`scoped`; git
/// before 2.26 can't say).
fn reading(lead: &[String], scoped: bool) -> Vec<String> {
    let scope: &[&str] = if scoped { &["--show-scope"] } else { &[] };
    owned(lead)
        .chain(std::iter::once("config".to_owned()))
        .chain(scope.iter().map(|each| (*each).to_owned()))
        .chain(["-z", "--get-regexp", DRIVERS].map(str::to_owned))
        .collect()
}

/// Reads what the repository's settings name (with scopes, then without where git can't say
/// them) and what switches it off.
fn read_programs(
    here: &Here,
    lead: &[String],
    cwd: Option<&Path>,
    remaining: impl Fn() -> Result<Duration, Refused>,
) -> Result<Found, Refused> {
    for scoped in [true, false] {
        let ran = git(&reading(lead, scoped), cwd, remaining()?);
        match ran.exit {
            Exit::Code(0) => return switched_off(here, &ran.out, scoped),
            Exit::Code(1) | Exit::NoGit => return Ok(Found::default()),
            Exit::Code(129) if scoped => continue,
            _ => return Err(Refused::Unread),
        }
    }
    Err(Refused::Unread)
}

/// A `config -z` entry as its name and value (none for a name alone).
fn entry(text: &str) -> (&str, Option<&str>) {
    match text.split_once('\n') {
        Some((name, value)) => (name, Some(value)),
        None => (text, None),
    }
}

/// Whether a `core.fsmonitor` value means git's own watcher, as git reads a yes.
fn watches(value: Option<&str>) -> bool {
    let Some(value) = value.map(str::trim) else {
        return true;
    };
    let number = value.strip_prefix('-').unwrap_or(value);
    ["true", "yes", "on"]
        .iter()
        .any(|yes| value.eq_ignore_ascii_case(yes))
        || (number.starts_with(|first: char| ('1'..='9').contains(&first))
            && number.chars().all(|digit| digit.is_ascii_digit()))
}

/// [`WATCHER`] when the last `core.fsmonitor` of a `config -z --get-regexp` read says git's own.
fn watcher_in(read: &str) -> &'static [&'static str] {
    match read.split('\0').rfind(|part| !part.is_empty()).map(entry) {
        Some(("core.fsmonitor", value)) if watches(value) => WATCHER,
        _ => &[],
    }
}

/// (`config` starts nothing; without [`EVERY`], whose own `core.fsmonitor=false` it would read last.)
fn watcher_read(lead: &[String]) -> Vec<String> {
    owned(lead)
        .chain(["config", "-z", "--get-regexp", r"^core\.fsmonitor$"].map(str::to_owned))
        .collect()
}

/// The part of a setting's name before its key (`filter.x` of `filter.x.clean`).
fn section(name: &str) -> &str {
    match name.rfind('.') {
        Some(at) => &name[..at],
        // (Never for what DRIVERS reads; as the TS box cuts it.)
        None => &name[..name.len().saturating_sub(1)],
    }
}

/// What switches off what the read found: each hook, and the repository's own filters and
/// converters but git-lfs's and git-crypt's.
fn switched_off(here: &Here, read: &str, scoped: bool) -> Result<Found, Refused> {
    let parts: Vec<&str> = read.split('\0').filter(|part| !part.is_empty()).collect();
    let mut found = Found::default();
    let mut watcher: &[&str] = &[];
    for at in (0..parts.len()).step_by(if scoped { 2 } else { 1 }) {
        let (scope, text) = if scoped {
            (parts[at], parts.get(at + 1).copied())
        } else {
            ("local", Some(parts[at]))
        };
        let Some((name, value)) = text.map(entry) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        if name == "core.fsmonitor" {
            watcher = if watches(value) { WATCHER } else { &[] };
            continue;
        }
        let place = section(name);
        if name.starts_with("hook.") {
            if place.contains('=') {
                return Err(Refused::Unswitchable);
            }
            found
                .settings
                .extend(["-c".to_owned(), format!("{place}.enabled=false")]);
            continue;
        }
        if matches!(scope, "system" | "global" | "command") || known(here, value) {
            continue;
        }
        if name.starts_with("diff.") {
            found.converters = true;
            continue;
        }
        if name.contains('=') {
            return Err(Refused::Unswitchable);
        }
        found.settings.extend([
            "-c".to_owned(),
            format!("{name}="),
            "-c".to_owned(),
            format!("{place}.required=false"),
        ]);
    }
    found
        .settings
        .extend(watcher.iter().map(|each| (*each).to_owned()));
    Ok(found)
}

/// The program a git-lfs or git-crypt setting runs: `"?prog"? (filter-process|clean|smudge|diff)`
/// then any of ` --skip`, ` --key-name=<word>`, ` -- %f`.
fn known_program(value: &str) -> Option<&str> {
    let unquoted = value.strip_prefix('"').unwrap_or(value);
    let end = unquoted
        .find(|each: char| each == '"' || each.is_whitespace())
        .unwrap_or(unquoted.len());
    let (program, after) = unquoted.split_at(end);
    let after = after.strip_prefix('"').unwrap_or(after);
    let mut tail = ["filter-process", "clean", "smudge", "diff"]
        .iter()
        .find_map(|verb| after.strip_prefix(' ')?.strip_prefix(verb))?;
    while !tail.is_empty() {
        tail = option_after(tail)?;
    }
    (!program.is_empty()).then_some(program)
}

/// What's left after a git-lfs or git-crypt option at the start of `tail`; none where there's none.
fn option_after(tail: &str) -> Option<&str> {
    if let Some(more) = tail
        .strip_prefix(" --skip")
        .or_else(|| tail.strip_prefix(" -- %f"))
    {
        return Some(more);
    }
    let name = tail.strip_prefix(" --key-name=")?;
    let end = name
        .find(|each: char| !(each.is_ascii_alphanumeric() || each == '_' || each == '-'))
        .unwrap_or(name.len());
    (end > 0).then(|| &name[end..])
}

/// Whether `value` runs git-lfs or git-crypt as installed here, to do what they set up.
fn known(here: &Here, value: Option<&str>) -> bool {
    let Some(program) = value.and_then(known_program) else {
        return false;
    };
    let Some(tool) = Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
    else {
        return false;
    };
    if !TOOLS.contains(&tool) {
        return false;
    }
    let Some(place) = installed(here, tool) else {
        return false;
    };
    !program.contains('/') || real(Path::new(program)).is_some_and(|found| found == place)
}

/// Where `PATH` finds `tool`, as its real place; none where it isn't installed.
fn installed(here: &Here, tool: &str) -> Option<PathBuf> {
    std::env::split_paths(here.path.as_deref()?)
        .filter(|folder| !folder.as_os_str().is_empty())
        .map(|folder| folder.join(tool))
        .filter(|file| platform::executable(file))
        .find_map(|file| real(&file))
}

/// How a git the layer ran ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Exit {
    Code(i32),
    /// Ended by a signal: no code.
    Signalled,
    /// No git here.
    NoGit,
    /// Stopped at its time, or it couldn't start.
    Failed,
}

struct Ran {
    exit: Exit,
    out: String,
}

/// git with `args` from `cwd`, never asking anything, stopped after `within`.
fn git(args: &[String], cwd: Option<&Path>, within: Duration) -> Ran {
    let mut command = Command::new("git");
    command
        .args(args)
        .env(GIT_QUIET.0, GIT_QUIET.1)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ran {
                exit: Exit::NoGit,
                out: String::new(),
            };
        }
        Err(_) => {
            return Ran {
                exit: Exit::Failed,
                out: String::new(),
            };
        }
    };
    let reader = child.stdout.take().map(|mut stdout| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stdout.read_to_end(&mut bytes);
            bytes
        })
    });
    let deadline = Instant::now().checked_add(within);
    let exit = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code().map_or(Exit::Signalled, Exit::Code),
            Ok(None) if deadline.is_some_and(|deadline| Instant::now() >= deadline) => {
                let _ = child.kill();
                let _ = child.wait();
                break Exit::Failed;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(2)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break Exit::Failed;
            }
        }
    };
    let bytes = reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    Ran {
        exit,
        out: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

/// What a `commondir` says: nothing there, something that can't be read as a small regular file,
/// or its text.
enum Named {
    Nothing,
    Unreadable,
    Says(String),
}

/// A `commondir` read only as a small regular file, never waiting on one (a FIFO in its place).
fn read_named(file: &Path) -> Named {
    let opened = match open_plain(file) {
        Ok(opened) => opened,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Named::Nothing,
        Err(_) => return Named::Unreadable,
    };
    match opened.metadata() {
        Ok(kind) if kind.is_file() && kind.len() <= MOST_NAMED => {}
        _ => return Named::Unreadable,
    }
    let mut bytes = Vec::new();
    match opened.take(MOST_NAMED).read_to_end(&mut bytes) {
        Ok(_) => Named::Says(String::from_utf8_lossy(&bytes).into_owned()),
        Err(_) => Named::Unreadable,
    }
}

/// Node's `dirname` and `basename`: the root is its own parent and has no name.
fn above(path: &Path) -> &Path {
    path.parent().unwrap_or(path)
}

fn name_of(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
}

/// Whether a `commondir` in `folder` (the repository's or worktree's own folder git found) leads
/// where it should: itself, or for a worktree's own folder (`<repository>/worktrees/<name>`) the
/// repository it's one of.
fn leads_home(folder: &Path, named: &str) -> bool {
    let named = named.trim_end_matches(['\r', '\n']);
    let (Some(own), Some(common)) = (real(folder), real(&resolved(&folder.join(named)))) else {
        return false;
    };
    common == own
        || (name_of(above(&own)) == "worktrees"
            && above(above(&own)) == above(above(folder))
            && real(above(above(folder))).is_some_and(|repository| repository == common))
}

/// Whether the repository git would work in, with `lead` from `cwd`, has a `commondir` leading
/// anywhere but where it should, or is a bare one a folder's own files made (one with a `.git` of
/// its own, which no repository's own folder has). Not in one: no. git not run: yes.
fn stray_common(
    here: &Here,
    lead: &[String],
    rest: &[String],
    cwd: Option<&Path>,
    within: Duration,
) -> bool {
    if making(rest) {
        return false;
    }
    let asked: Vec<String> = owned(lead)
        .chain(EVERY.iter().map(|each| (*each).to_owned()))
        .chain(["rev-parse", "--absolute-git-dir"].map(str::to_owned))
        .collect();
    let ran = git(&asked, cwd, within);
    let folder = match ran.exit {
        Exit::NoGit | Exit::Failed => return true,
        Exit::Code(0) => ran.out.trim().to_owned(),
        Exit::Code(_) | Exit::Signalled => String::new(),
    };
    if folder.is_empty() {
        return false;
    }
    let folder = PathBuf::from(folder);
    let git_dir_given = lead
        .iter()
        .any(|each| each == "--git-dir" || each.starts_with("--git-dir="));
    if !git_dir_given
        && !here.git_dir_named
        && std::fs::symlink_metadata(folder.join(".git")).is_ok()
    {
        return true;
    }
    match read_named(&folder.join("commondir")) {
        Named::Nothing => false,
        Named::Unreadable => true,
        Named::Says(named) => !leads_home(&folder, &named),
    }
}

#[cfg(unix)]
mod platform {
    use std::os::unix::ffi::OsStrExt;

    /// A full path, as Node's `path.isAbsolute` reads it.
    pub(super) fn absolute(path: &str) -> bool {
        path.starts_with('/')
    }

    /// Whether this user may run `file`, as `access(X_OK)` says.
    pub(super) fn executable(file: &std::path::Path) -> bool {
        let Ok(name) = std::ffi::CString::new(file.as_os_str().as_bytes()) else {
            return false;
        };
        // SAFETY: `name` is a NUL-terminated string that lives through the call, which only reads it.
        unsafe { libc::access(name.as_ptr(), libc::X_OK) == 0 }
    }
}

#[cfg(not(unix))]
mod platform {
    /// A full path as Node's `path.isAbsolute` reads it on Windows: a drive's, a share's, or one
    /// from the top of the current drive.
    pub(super) fn absolute(path: &str) -> bool {
        std::path::Path::new(path).is_absolute() || path.starts_with(['/', '\\'])
    }

    /// Windows has no execute bit: there is as runnable (as Node's `access(X_OK)` says there).
    pub(super) fn executable(file: &std::path::Path) -> bool {
        file.exists()
    }
}

#[cfg(test)]
#[path = "safe_tests.rs"]
mod tests;
