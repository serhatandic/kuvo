//! The resident supervisor and its installed-release selector.
//!
//! The updater stages complete packs before requesting a selection. The keeper
//! only switches executable code and records the trial. It never restores a data snapshot.

#[cfg(unix)]
pub mod crontab;
mod engine_pack;
pub mod fleet;
#[cfg(target_os = "macos")]
pub mod mac_helper;
pub mod manifest;
pub mod prepare_command;
mod readiness;
mod release;
mod retirement;
pub mod staging;
pub mod update_status;
mod worker;
#[cfg(target_os = "linux")]
mod wsl_task;

#[cfg(windows)]
pub mod windows;

pub use engine_pack::provision_pinned_engine;
pub use readiness::resident_info;
pub use release::Release;
pub use retirement::lease_running_release;
pub use worker::{next_resident_coming, wait_for_stop};

use std::{
    fs::{self, File},
    future::Future,
    io,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use kuvo_os::{
    platform::Environment,
    process::{ProcessIdentity, ProcessState},
};
use kuvo_store::{files, home::Home, locks};
use serde::{Deserialize, Serialize};
use tokio::time::{self, Instant};

use worker::Worker;

/// Native installations use a plain file. It can be replaced even while a reader
/// opened with FILE_SHARE_DELETE still holds the old contents.
pub const SELECTOR: &str = "active-release";

/// Rust packs keep a second signed keeper that released Macs never replace.
#[cfg(unix)]
pub fn shell_keeper(bin: &std::path::Path) -> PathBuf {
    let saved = bin.join(".kuvo-start-keeper");
    if bin.join("kuvo-box").is_file() && saved.is_file() {
        saved
    } else {
        bin.join("kuvo-start")
    }
}

#[derive(Debug)]
pub enum Command {
    Run,
    Handover,
    Select(Release),
    Pause,
    Off,
}

const ROUND: Duration = Duration::from_secs(1);
const RESTART: Duration = Duration::from_secs(15);

struct Installation {
    home: PathBuf,
    run: PathBuf,
    selector: PathBuf,
    previous: PathBuf,
    reverting: PathBuf,
    worker: PathBuf,
}

#[derive(Deserialize, Serialize, PartialEq)]
struct Handover {
    from: ProcessIdentity,
    to: ProcessIdentity,
    resident: ProcessIdentity,
}

fn read_record<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> io::Result<Option<T>> {
    files::read(path)?
        .map(|bytes| serde_json::from_slice(&bytes).map_err(io::Error::from))
        .transpose()
}

enum Trial {
    Checking(Release),
    Reverting(Release),
}

impl Trial {
    fn previous_to_restore(&self, deadline: Instant) -> Option<&Release> {
        match self {
            Self::Reverting(previous) => Some(previous),
            Self::Checking(previous) if Instant::now() >= deadline => Some(previous),
            Self::Checking(_) => None,
        }
    }
}

impl Installation {
    fn open(home: &Home) -> io::Result<Self> {
        let run = home.run();
        files::create_private_dirs(&run)?;
        Ok(Self {
            home: home.root().to_owned(),
            selector: home.root().join(SELECTOR),
            previous: run.join("previous"),
            reverting: run.join("keeper-rollback"),
            worker: run.join("keeper-resident.json"),
            run,
        })
    }

    fn selected(&self) -> io::Result<Release> {
        Release::read(&self.selector)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "No Kuvo release has been selected")
        })
    }

    async fn lock_selection(&self) -> io::Result<File> {
        let lock = self.run.join(".keeper-selection.lock");
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            match locks::try_file_lock(&lock) {
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < until =>
                {
                    time::sleep(Duration::from_millis(20)).await;
                }
                result => return result,
            }
        }
    }

    async fn select(&self, next: Release) -> io::Result<()> {
        let _lock = self.lock_selection().await?;
        next.validate(&self.home)?;
        let current = Release::read(&self.selector)?;
        let previous = Release::read(&self.previous)?;
        if self.reverting.try_exists()? || (previous.is_some() && previous != current) {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "A Kuvo release is on trial",
            ));
        }
        if self.pending_handover()?.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Kuvo's keeper is handing over",
            ));
        }
        if current.as_ref() == Some(&next) {
            return Ok(());
        }
        if let Some(current) = current {
            current.write(&self.previous)?;
        }
        next.write(&self.selector)
    }

    async fn selection(&self) -> io::Result<(Release, Option<Trial>)> {
        let _lock = self.lock_selection().await?;
        let current = self.selected()?;
        let previous = Release::read(&self.previous)?;
        let reverting = Release::read(&self.reverting)?;
        if let Some(failed) = reverting {
            if failed == current {
                return previous
                    .filter(|previous| previous != &current)
                    .map(|previous| (current, Some(Trial::Reverting(previous))))
                    .ok_or_else(|| io::Error::other("A failed trial has no previous release"));
            }
            if previous
                .as_ref()
                .is_some_and(|previous| previous != &current)
            {
                return Err(io::Error::other(
                    "Unexpected release selected during rollback",
                ));
            }
            self.finish_rollback()?;
            return Ok((current, None));
        }
        if previous.as_ref() == Some(&current) {
            // The selector never moved before the updater stopped.
            files::remove_file_synced(&self.previous)?;
            return Ok((current, None));
        }
        Ok((current, previous.map(Trial::Checking)))
    }

    async fn confirm(&self, selected: &Release) -> io::Result<()> {
        let _lock = self.lock_selection().await?;
        if &self.selected()? == selected {
            if let Some(previous) = Release::read(&self.previous)? {
                let home = Home::new(self.home.clone())?;
                if let Err(error) = update_status::note_last(
                    &home,
                    jiff::Timestamp::now().as_millisecond(),
                    update_status::Outcome::Updated {
                        from: previous.version().into(),
                        to: selected.version().into(),
                    },
                ) {
                    eprintln!("Recording the confirmed release: {error}");
                }
            }
            files::remove_file_synced(&self.run.join("update-cannot"))?;
            files::remove_file_synced(&self.previous)?;
        }
        Ok(())
    }

    async fn rollback(&self, failed: &Release, previous: &Release) -> io::Result<()> {
        let _lock = self.lock_selection().await?;
        if &self.selected()? != failed || Release::read(&self.previous)?.as_ref() != Some(previous)
        {
            return Err(io::Error::other(
                "The selected release changed during rollback",
            ));
        }
        previous.validate(&self.home)?;
        // Publish the rollback decision before touching the selector or the public refusal.
        // A crash at any later step must finish this rollback, even if the bad release
        // is able to answer on a subsequent launch.
        failed.write(&self.reverting)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?;
        files::write_secret(
            &self.run.join("update-cannot"),
            failed.refusal(now.as_secs()).as_bytes(),
        )?;
        previous.write(&self.selector)?;
        self.finish_rollback()
    }

    fn finish_rollback(&self) -> io::Result<()> {
        files::remove_file_synced(&self.run.join("update-engine-hold.json"))?;
        files::remove_file_synced(&self.previous)?;
        files::remove_file_synced(&self.reverting)
    }

    fn prepare_start(&self) -> io::Result<()> {
        if self.home.join("backup/restoring").try_exists()? {
            return Err(io::Error::other(
                "Finish the Kuvo backup restore before starting workers.",
            ));
        }
        Ok(())
    }

    fn pending_handover(&self) -> io::Result<Option<Handover>> {
        Ok(read_record::<Handover>(&self.run.join("keeper-handover"))?
            .filter(|handover| handover.to.state() != ProcessState::Stale))
    }

    async fn request_handover(
        &self,
        successor: &ProcessIdentity,
    ) -> io::Result<(Handover, Worker)> {
        let _selection = self.lock_selection().await?;
        if self.previous.try_exists()?
            || self.reverting.try_exists()?
            || self.run.join("off").try_exists()?
            || self.run.join("paused").try_exists()?
            || self.pending_handover()?.is_some()
        {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Kuvo's keeper is busy",
            ));
        }
        let predecessor = read_record::<ProcessIdentity>(&self.run.join("keeper-owner"))?
            .filter(|owner| owner.state() == ProcessState::Live)
            .ok_or_else(|| io::Error::other("No keeper is running to hand over"))?;
        // Hold the resident (and on Windows its job) before asking the old keeper
        // to drop its handle. A failed open must leave the old keeper in charge.
        let worker = Worker::adopt(&self.home, &self.worker)?
            .filter(|resident| resident.identity().state() == ProcessState::Live)
            .ok_or_else(|| io::Error::other("No live resident to hand over"))?;
        if worker.release != self.selected()? {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Kuvo is switching releases",
            ));
        }
        let handover = Handover {
            from: predecessor,
            to: successor.clone(),
            resident: worker.identity().clone(),
        };
        files::write_secret(
            &self.run.join("keeper-handover"),
            &serde_json::to_vec(&handover)?,
        )?;
        Ok((handover, worker))
    }

    async fn supervise(
        &self,
        env: Environment,
        shutdown: impl Future<Output = ()>,
        handing_over: bool,
    ) -> io::Result<()> {
        self.prepare_start()?;
        let identity = ProcessIdentity::read(std::process::id())?
            .ok_or_else(|| io::Error::other("Couldn't identify the keeper"))?;
        let transfer = if handing_over {
            Some(self.request_handover(&identity).await?)
        } else {
            None
        };
        let until = Instant::now() + Duration::from_secs(15);
        let owner = self.run.join(".keeper.lock");
        let _owner = loop {
            match locks::try_file_lock(&owner) {
                Ok(lock) => break lock,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock && transfer.is_none() => {
                    return Ok(());
                }
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < until =>
                {
                    time::sleep(Duration::from_millis(100)).await;
                }
                Err(error) => return Err(error),
            }
        };
        let owner_path = self.run.join("keeper-owner");
        let pid_path = self.run.join("box.pid");
        let worker;
        {
            let _selection = self.lock_selection().await?;
            let pending = self.pending_handover()?;
            worker = if let Some((handover, resident)) = transfer {
                if pending.as_ref() != Some(&handover) {
                    return Err(io::Error::other(
                        "Keeper handover changed before it finished",
                    ));
                }
                Some(resident)
            } else {
                if pending.is_some() {
                    return Ok(());
                }
                if !self.run.join("off").try_exists()? {
                    files::remove_file_synced(&self.run.join("paused"))?;
                }
                Worker::adopt(&self.home, &self.worker)?
            };
            files::write_secret(&owner_path, &serde_json::to_vec(&identity)?)?;
            files::write_secret(&pid_path, format!("{}\n", identity.pid).as_bytes())?;
            files::remove_file_synced(&self.run.join("keeper-handover"))?;
        }
        let result = self.rounds(env, shutdown, &identity, worker).await;
        let cleanup = files::remove_file_synced(&pid_path)
            .and_then(|()| files::remove_file_synced(&owner_path));
        result.and(cleanup)
    }

    async fn rounds(
        &self,
        env: Environment,
        shutdown: impl Future<Output = ()>,
        identity: &ProcessIdentity,
        mut worker: Option<Worker>,
    ) -> io::Result<()> {
        let client = readiness::probe_client()?;
        let mut trial_deadline = Instant::now();
        let mut selected = None;
        let mut next_start = Instant::now();
        let mut interval = time::interval(ROUND);
        interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                () = &mut shutdown => return Ok(()),
                _ = interval.tick() => {}
            }
            if self.run.join("off").try_exists()? || self.run.join("paused").try_exists()? {
                self.stop(&mut worker, false).await?;
                return Ok(());
            }
            if let Some(handover) = self.pending_handover()?
                && &handover.from == identity
                && handover.to.state() == ProcessState::Live
                && worker.as_ref().is_some_and(|resident| {
                    resident.identity() == &handover.resident
                        && resident.identity().state() == ProcessState::Live
                })
            {
                return Ok(());
            }
            let (current, trial) = match self.selection().await {
                // No prior release exists until the installer publishes the very first selector.
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    self.stop(&mut worker, false).await?;
                    return Err(error);
                }
                result => result?,
            };
            if selected.as_ref() != Some(&current) {
                selected = Some(current.clone());
                trial_deadline = Instant::now() + fleet::keeper_trial_limit();
                next_start = Instant::now();
            }
            if worker
                .as_ref()
                .is_some_and(|worker| worker.release != current)
            {
                self.stop(&mut worker, true).await?;
            }
            if let Some(resident) = &mut worker
                && resident.finished()?
            {
                worker = None;
                files::remove_file_synced(&self.worker)?;
                next_start = Instant::now() + RESTART;
            }
            if let Some(trial) = &trial {
                if let Some(previous) = trial.previous_to_restore(trial_deadline) {
                    self.stop(&mut worker, true).await?;
                    self.rollback(&current, previous).await?;
                    continue;
                }
                if let Some(resident) = &worker
                    && readiness::probe(&client, &self.run, resident.identity().pid)
                        .await?
                        .is_some()
                {
                    self.confirm(&current).await?;
                }
            }
            if worker.is_none() && Instant::now() >= next_start {
                self.prepare_start()?;
                match Worker::start(&self.home, &self.worker, current, env.clone()) {
                    Ok(resident) => worker = Some(resident),
                    Err(error) => {
                        eprintln!("Starting the selected Kuvo release: {error}");
                        next_start = Instant::now() + RESTART;
                    }
                }
            }
        }
    }

    /// Stop the resident it keeps: for good, or (`next`) for another to take its place.
    async fn stop(&self, worker: &mut Option<Worker>, next: bool) -> io::Result<()> {
        if let Some(worker) = worker.take() {
            worker.stop(&self.run, next).await?;
        }
        files::remove_file_synced(&self.worker)
    }

    async fn stop_when_unowned(&self) -> io::Result<()> {
        let _owner = match locks::try_file_lock(&self.run.join(".keeper.lock")) {
            Ok(lock) => lock,
            // The active keeper sees the marker and shuts down its own child.
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) => return Err(error),
        };
        let mut worker = Worker::adopt(&self.home, &self.worker)?;
        // (Off or paused: it stops for good.)
        self.stop(&mut worker, false).await?;
        files::remove_file_synced(&self.run.join("box.pid"))
    }

    async fn pause(&self) -> io::Result<()> {
        if cfg!(unix)
            && !fs::read_to_string(self.home.join("box/kind"))
                .is_ok_and(|kind| kind.trim() == "mac")
        {
            return Err(io::Error::other(
                "Only a Mac's own box is stopped until it next starts.",
            ));
        }
        files::write_secret(&self.run.join("paused"), b"")?;
        self.stop_when_unowned().await
    }

    async fn off(&self) -> io::Result<()> {
        files::write_secret(&self.run.join("off"), b"")?;
        for path in ["link.needed", "serve.needed"] {
            files::remove_file_synced(&self.run.join(path))?;
        }
        self.stop_when_unowned().await
    }
}

pub async fn execute(
    home: &Home,
    env: Environment,
    command: Command,
    shutdown: impl Future<Output = ()>,
) -> io::Result<()> {
    let installation = Installation::open(home)?;
    match command {
        Command::Run => installation.supervise(env, shutdown, false).await,
        Command::Handover => installation.supervise(env, shutdown, true).await,
        Command::Select(release) => installation.select(release).await,
        Command::Pause => installation.pause().await,
        Command::Off => installation.off().await,
    }
}
