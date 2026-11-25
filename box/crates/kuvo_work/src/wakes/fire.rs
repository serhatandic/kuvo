//! A round of wake-ups and watches, as `kuvo-wake fire` sends them: each due wake-up to its
//! conversation (a repeating one comes back at its next time, on your clock), what changed in a
//! watch's check, and the folders of conversations that ended, were archived or are a subagent's
//! settled. One round at a time for each root, kept apart by the same `.firing` folder
//! `kuvo-wake` and Cancel use.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use kuvo_engine::{Unreachable, WakeDelivered, WakeEngine};
use serde_json::Value;

use super::{
    check::{self, Fence, Reading},
    clock,
    folder::{self, Kind, Roots, WakeName, names, text},
    inbox,
};

/// What a round needs of the engine; `WakeEngine` in the resident, a stand-in in tests.
pub(crate) trait Reach: Sync {
    /// Whether this round reaches the coding agents' conversations (`agents`), or the engine's.
    fn reaches(&self, agents: bool) -> bool;
    async fn session(&self, id: &str) -> Result<Option<Value>, Unreachable>;
    async fn wait_for_tools(&self, directory: &str);
    async fn stopped(&self, session: &str) -> bool;
    async fn waits_in_inbox(&self, session: &str, note: &str) -> bool;
    async fn waits_on_box(&self, session: &str, note: &str) -> bool;
    async fn deliver(
        &self,
        session: &str,
        note: &str,
        summary: Option<&str>,
    ) -> Result<WakeDelivered, Unreachable>;
}

impl Reach for WakeEngine {
    fn reaches(&self, agents: bool) -> bool {
        WakeEngine::reaches(self, agents)
    }
    async fn session(&self, id: &str) -> Result<Option<Value>, Unreachable> {
        WakeEngine::session(self, id).await
    }
    async fn wait_for_tools(&self, directory: &str) {
        WakeEngine::wait_for_tools(self, directory).await;
    }
    async fn stopped(&self, session: &str) -> bool {
        WakeEngine::stopped(self, session).await
    }
    async fn waits_in_inbox(&self, session: &str, note: &str) -> bool {
        WakeEngine::waits_in_inbox(self, session, note).await
    }
    async fn waits_on_box(&self, session: &str, note: &str) -> bool {
        WakeEngine::waits_on_box(self, session, note).await
    }
    async fn deliver(
        &self,
        session: &str,
        note: &str,
        summary: Option<&str>,
    ) -> Result<WakeDelivered, Unreachable> {
        WakeEngine::deliver(self, session, note, summary).await
    }
}

/// The seconds since 1970 now.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

/// A folder kept as a lock, as `kuvo-wake` keeps one (`mkdir`), let go when dropped; one left by
/// a round killed outright is taken over once it's `stale`.
pub(crate) struct FolderLock(PathBuf);

impl FolderLock {
    pub(crate) fn take(path: PathBuf, stale: Duration) -> Option<Self> {
        let old = fs::metadata(&path)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|at| SystemTime::now().duration_since(at).ok())
            .is_some_and(|age| age > stale);
        if old {
            let _ = fs::remove_dir(&path);
        }
        fs::create_dir(&path).ok().map(|()| Self(path))
    }
}

impl Drop for FolderLock {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.0);
    }
}

/// When the next wake-up waits to go: the earliest of every folder not held, past a retry's wait.
pub fn next_wake(roots: &Roots) -> Option<f64> {
    let mut soonest: Option<f64> = None;
    for (_, folder) in roots.folders() {
        if folder.join("paused").exists() || !folder.join("session").exists() {
            continue;
        }
        let retry = retry_at(&folder).unwrap_or(0) as f64;
        for name in names(&folder.join("wakes")) {
            let Some(wake) = WakeName::parse(&name) else {
                continue;
            };
            let at = wake.due.max(retry);
            soonest = Some(soonest.map_or(at, |soonest: f64| soonest.min(at)));
        }
    }
    soonest
}

fn retry_at(folder: &Path) -> Option<i64> {
    text(&folder.join(".retry"))
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// Whether a round now has anything for the engine: a wake-up due, or a watch, in a folder that's
/// neither held nor waiting to try again, of a conversation the engine runs.
pub fn wanted(roots: &Roots, now: i64) -> bool {
    wanted_by(roots, now, false)
}

/// As `wanted`, for the coding agents' conversations.
pub fn agents_wanted(roots: &Roots, now: i64) -> bool {
    wanted_by(roots, now, true)
}

fn wanted_by(roots: &Roots, now: i64, agents: bool) -> bool {
    roots.folders().any(|(_, folder)| {
        if folder.join("paused").exists() || !folder.join("session").exists() {
            return false;
        }
        if retry_at(&folder).is_some_and(|at| at > now) {
            return false;
        }
        let due = !names(&folder.join("watches")).is_empty()
            || names(&folder.join("wakes"))
                .iter()
                .filter_map(|name| WakeName::parse(name))
                .any(|wake| wake.due <= now as f64);
        due && via_agents(&folder) == agents
    })
}

/// A conversation the coding agents run (its folder says `via` bridge): theirs to reach.
fn via_agents(folder: &Path) -> bool {
    text(&folder.join("via")).trim() == "bridge"
}

fn archived(session: &Value) -> bool {
    session["metadata"]["kuvo.archived"] == Value::Bool(true)
        || session["time"]["archived"].is_number()
}

/// How a folder stands once its conversation is looked at: it goes on, or not this round (gone to
/// the trash, held as archived, or handed to the conversation that started it).
enum Settled {
    GoOn,
    Stop,
}

pub(crate) struct Round<'a, R: Reach> {
    pub roots: &'a Roots,
    pub reach: &'a R,
    pub now: i64,
}

impl<R: Reach> Round<'_, R> {
    /// Each due wake-up sent, root by root; then each root's watches, apart, so a slow check
    /// never holds a wake-up up.
    pub async fn wakes(&self) {
        // An interrupted restore must finish before its records can fire.
        if self
            .roots
            .run
            .parent()
            .is_some_and(|home| home.join("backup/restoring").exists())
        {
            return;
        }
        expire_trash(&self.roots.trash);
        for (root, kind) in [
            (&self.roots.bots, Kind::Bot),
            (&self.roots.sessions, Kind::Session),
        ] {
            if !root.is_dir() {
                continue;
            }
            let Some(_lock) = FolderLock::take(root.join(".firing"), Duration::from_secs(600))
            else {
                continue;
            };
            for name in names(root) {
                let folder = root.join(name);
                if folder.is_dir() {
                    self.folder(&folder, kind).await;
                }
            }
        }
    }

    async fn folder(&self, folder: &Path, kind: Kind) {
        let session = text(&folder.join("session"));
        if let Some(hold) = folder::hold(folder) {
            let moving = folder::moving(folder) || hold.trim() == "moving";
            if !moving && !session.is_empty() && self.reach.reaches(via_agents(folder)) {
                self.settle(folder, kind, &session, true).await;
            }
            if folder.join("paused").exists() || !folder.is_dir() {
                return;
            }
        }
        if retry_at(folder).is_some_and(|at| at > self.now) {
            return;
        }
        inbox::take(folder, self.now as u64);
        if names(&folder.join("wakes")).is_empty() && names(&folder.join("watches")).is_empty() {
            return;
        }
        if session.is_empty() || !self.reach.reaches(via_agents(folder)) {
            return;
        }
        if matches!(
            self.settle(folder, kind, &session, false).await,
            Settled::Stop
        ) {
            return;
        }
        let stopped = self.reach.stopped(&session).await;
        // (A coding agent's tools are its own, not the engine's to wait for.)
        let mut tools = via_agents(folder);
        let mut failed = false;
        for name in names(&folder.join("wakes")) {
            let path = folder.join("wakes").join(&name);
            let Some(wake) = WakeName::parse(&name) else {
                continue;
            };
            let Some(due) = wake.due_seconds() else {
                continue;
            };
            if due > self.now || !path.is_file() {
                continue;
            }
            let note = text(&path).trim_end_matches('\n').to_owned();
            let summary = text(&folder.join("summaries").join(&wake.letters));
            let summary = summary.trim_end_matches('\n');
            let summary = (!summary.is_empty()).then_some(summary);
            let repeating = wake.repeat.is_some();
            let waits = repeating
                && (self.reach.waits_in_inbox(&session, &note).await
                    || (stopped && self.reach.waits_on_box(&session, &note).await));
            let went = waits || {
                if !stopped && !tools {
                    tools = true;
                    let directory = text(&folder.join("directory"));
                    if folder.join("directory").is_file() {
                        self.reach.wait_for_tools(&directory).await;
                    }
                }
                self.reach.deliver(&session, &note, summary).await.is_ok()
            };
            if !went {
                failed = true;
                continue;
            }
            let next = wake
                .repeat
                .as_deref()
                .filter(|repeat| clock::repeats(repeat.split('@').next().unwrap_or_default()))
                .map(|repeat| {
                    let zone = clock::zone_of(folder);
                    let repeat = clock::kept(&zone, due, repeat);
                    let at = clock::next(&zone, due, &repeat, self.now);
                    format!("{at}-{}-{repeat}", wake.letters)
                });
            match next {
                Some(next) => {
                    let _ = fs::rename(&path, folder.join("wakes").join(next));
                }
                None => {
                    let _ = fs::remove_file(&path);
                    let _ = fs::remove_file(folder.join("summaries").join(&wake.letters));
                }
            }
        }
        if failed {
            let waited: i64 = text(&folder.join(".retry"))
                .split_whitespace()
                .nth(1)
                .and_then(|wait| wait.parse().ok())
                .unwrap_or(30);
            let wait = (waited * 2).min(1_800);
            let _ = fs::write(
                folder.join(".retry"),
                format!("{} {wait}\n", self.now + wait),
            );
        } else {
            let _ = fs::remove_file(folder.join(".retry"));
        }
    }

    /// A conversation's own folder as its engine has the conversation now: one that's gone goes
    /// to the trash rather than being tried for ever; one archived is held as archiving holds it;
    /// a subagent's goes to the conversation that started it; and it works where its conversation
    /// does. A bot's folder always goes on. `held`: only looked at, as a held one is let go once
    /// its conversation is gone or (held as archived) back.
    async fn settle(&self, folder: &Path, kind: Kind, session: &str, held: bool) -> Settled {
        if kind == Kind::Bot {
            return if held { Settled::Stop } else { Settled::GoOn };
        }
        let found = match self.reach.session(session).await {
            Ok(found) => found,
            Err(_) => return if held { Settled::Stop } else { Settled::GoOn },
        };
        let Some(found) = found else {
            let trash = self.roots.trash.clone();
            if fs::create_dir_all(&trash).is_ok() {
                let _ = fs::rename(folder, trash.join(format!("wakes-{session}-{}", self.now)));
            }
            return Settled::Stop;
        };
        if held {
            if folder::hold(folder).as_deref() == Some("archived") && !archived(&found) {
                let _ = fs::remove_file(folder.join("paused"));
            }
            return Settled::Stop;
        }
        if archived(&found) {
            let _ = fs::write(folder.join("paused"), "archived");
            return Settled::Stop;
        }
        if let Some(directory) = found["location"]["directory"].as_str()
            && !directory.is_empty()
            && directory != text(&folder.join("directory"))
        {
            let _ = fs::write(folder.join("directory"), directory);
        }
        let Some(parent) = found["parentID"]
            .as_str()
            .filter(|id| folder::session_id(id))
        else {
            return Settled::GoOn;
        };
        self.hand_to(folder, parent);
        Settled::Stop
    }

    /// A subagent's wake-ups and watches handed to the conversation that started it (its bot's
    /// folder, if it's a bot's).
    fn hand_to(&self, folder: &Path, parent: &str) {
        let to = names(&self.roots.bots)
            .into_iter()
            .map(|id| self.roots.bots.join(id))
            .find(|bot| text(&bot.join("session")) == parent)
            .unwrap_or_else(|| self.roots.sessions.join(parent));
        if !to.is_dir() {
            if kuvo_store::files::create_private_dirs(&to).is_err()
                || fs::write(to.join("session"), parent).is_err()
            {
                return;
            }
            let _ = fs::copy(folder.join("directory"), to.join("directory"));
            if folder.join("via").is_file() {
                let _ = fs::copy(folder.join("via"), to.join("via"));
            }
        }
        let _ = fs::create_dir_all(to.join("wakes"));
        let _ = fs::create_dir_all(to.join("watches"));
        for name in names(&folder.join("wakes")) {
            let _ = fs::rename(
                folder.join("wakes").join(&name),
                to.join("wakes").join(&name),
            );
        }
        for name in names(&folder.join("summaries")) {
            let _ = fs::create_dir_all(to.join("summaries"));
            let _ = fs::rename(
                folder.join("summaries").join(&name),
                to.join("summaries").join(&name),
            );
        }
        for name in names(&folder.join("watches")) {
            let _ = fs::rename(
                folder.join("watches").join(&name),
                to.join("watches").join(&name),
            );
        }
        let _ = fs::remove_dir_all(folder);
    }

    /// Each root's watches that are due, one round of them at a time: what changed wakes its
    /// conversation with what its check prints now; one that can't go is told at the next check.
    pub async fn watches(&self, fence: &Fence, env: &kuvo_os::platform::Environment) {
        for (root, kind) in [
            (&self.roots.bots, Kind::Bot),
            (&self.roots.sessions, Kind::Session),
        ] {
            if !root.is_dir() {
                continue;
            }
            let Some(_lock) = FolderLock::take(root.join(".watching"), Duration::from_secs(3_600))
            else {
                continue;
            };
            for name in names(root) {
                let folder = root.join(name);
                if folder.join("paused").exists() || !folder.join("watches").is_dir() {
                    continue;
                }
                let session = text(&folder.join("session"));
                if session.is_empty() || !self.reach.reaches(via_agents(&folder)) {
                    continue;
                }
                if kind == Kind::Session
                    && matches!(self.reach.session(&session).await, Ok(Some(found)) if archived(&found))
                {
                    continue;
                }
                if self.reach.stopped(&session).await {
                    continue;
                }
                let changed = {
                    let (folder, fence, env, now) =
                        (folder.clone(), fence.clone(), env.clone(), self.now);
                    tokio::task::spawn_blocking(move || checks(&folder, now, &fence, &env))
                        .await
                        .unwrap_or_default()
                };
                for letters in changed {
                    let watch = folder.join("watches").join(&letters);
                    let message = text(&watch.join("message"));
                    if !watch.join("message").is_file() {
                        continue;
                    }
                    let summary = text(&watch.join("summary"));
                    let summary = summary.trim_end_matches('\n');
                    let went = self
                        .reach
                        .deliver(
                            &session,
                            message.trim_end_matches('\n'),
                            (!summary.is_empty()).then_some(summary),
                        )
                        .await
                        .is_ok();
                    if went {
                        delivered(&watch);
                    }
                }
            }
        }
    }
}

/// A watch's wake-up went: its reading is what its conversation heard.
fn delivered(watch: &Path) {
    if watch.join("pending").is_file() {
        let _ = fs::rename(watch.join("pending"), watch.join("last"));
    }
    let _ = fs::remove_file(watch.join("message"));
}

fn period(every: &str) -> Option<i64> {
    let (count, unit) = every.split_at(every.len().checked_sub(1)?);
    if count.is_empty()
        || count.len() > 6
        || count.starts_with('0')
        || !count.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let count: i64 = count.parse().ok()?;
    match unit {
        "m" if count >= 5 => Some(count * 60),
        "h" => Some(count * 3_600),
        "d" => Some(count * 86_400),
        _ => None,
    }
}

fn number(path: &Path) -> Option<i64> {
    let value = text(path);
    (!value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()))
        .then(|| value.parse().ok())
        .flatten()
}

/// A folder's due watches, each run once and compared with what its conversation last heard
/// (`last`). A run that's no reading is never a change. A change counts when two readings in a
/// row differ from `last`: the one that saw it (`seen`) and a second look two minutes later. The
/// wake-up's words go to `message` and its reading's sum to `pending`; the letters of each
/// returned. Blocking.
pub(crate) fn checks(
    folder: &Path,
    now: i64,
    fence: &Fence,
    env: &kuvo_os::platform::Environment,
) -> Vec<String> {
    let mut changed = Vec::new();
    for letters in names(&folder.join("watches")) {
        let watch = folder.join("watches").join(&letters);
        if !watch.join("check").is_file() {
            continue;
        }
        if number(&watch.join("next")).unwrap_or(0) > now {
            continue;
        }
        let Some(period) = period(text(&watch.join("every")).trim_end_matches('\n')) else {
            continue;
        };
        let seen = number(&watch.join("seen"));
        let mut next = seen.unwrap_or(now) + period;
        if next <= now {
            next = now + period;
        }
        let _ = fs::write(watch.join("next"), next.to_string());
        let Reading::Printed(output) = check::run(&watch, folder, fence, env) else {
            continue;
        };
        let sum = check::sum(&output);
        let last = text(&watch.join("last"));
        let forget = || {
            for name in ["pending", "message", "seen"] {
                let _ = fs::remove_file(watch.join(name));
            }
        };
        if !last.starts_with("2/") {
            let _ = fs::write(watch.join("last"), &sum);
            forget();
            continue;
        }
        if last == sum {
            forget();
            continue;
        }
        if seen.is_none() && !watch.join("pending").is_file() {
            let _ = fs::write(watch.join("seen"), now.to_string());
            let _ = fs::write(watch.join("next"), (now + 120).to_string());
            continue;
        }
        let _ = fs::remove_file(watch.join("seen"));
        let prints = if output.is_empty() {
            "now prints nothing.".to_owned()
        } else {
            format!("now prints:\n{output}")
        };
        let check_line = text(&watch.join("check"));
        let check_line = check_line.split('\n').next().unwrap_or_default();
        let message = format!(
            "{}\n\nYour watch ({letters}, every {}) saw a change. Its check, {check_line}, {prints}\n",
            text(&watch.join("note")).trim_end_matches('\n'),
            text(&watch.join("every")).trim_end_matches('\n'),
        );
        let _ = fs::write(watch.join("message"), message);
        let _ = fs::write(watch.join("pending"), &sum);
        changed.push(letters);
    }
    changed
}

/// Wake-ups of conversations that ended are kept in the trash a month, then go for good.
fn expire_trash(trash: &Path) {
    for name in names(trash) {
        if !name.starts_with("wakes-") {
            continue;
        }
        let path = trash.join(&name);
        let old = fs::metadata(&path)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|at| SystemTime::now().duration_since(at).ok())
            .is_some_and(|age| age.as_secs() / 86_400 > 30);
        if old {
            let _ = fs::remove_dir_all(&path);
        }
    }
}

#[cfg(test)]
mod tests;
