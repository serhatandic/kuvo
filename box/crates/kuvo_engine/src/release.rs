//! A differing engine stays put until its turns and prompt writers finish. The
//! saved hold outlives either keeper outcome, so its messages can be released.

use std::{io, path::Path, sync::LazyLock};

use http::StatusCode;
use kuvo_server::Refusal;
use serde_json::json;

use kuvo_os::process::{ProcessIdentity, ProcessState};
use serde::{Deserialize, Serialize};

use crate::{Engine, disk, lifetime::Demand};

const HOLD: &str = "fleet-engine-hold.json";
pub(crate) const HOLD_LINE: &str = "Kuvo’s engine is updating once the running sessions finish. New messages wait and go right after.";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Timing {
    release_drain_ms: u64,
    release_drain_retry_ms: u64,
}

static TIMING: LazyLock<Timing> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../src/shared/fleet-timing.json"))
        .expect("checked-in fleet timing")
});

pub(crate) fn turn_refusal() -> Refusal {
    Refusal::json(
        StatusCode::SERVICE_UNAVAILABLE,
        json!({"message":HOLD_LINE,"engineChanging":true}),
    )
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Stage {
    Holding,
    Releasing,
    Deferred,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Hold {
    format_version: u32,
    release: String,
    from: String,
    to: String,
    owner: ProcessIdentity,
    since: u64,
    stage: Stage,
    retry_at: Option<u64>,
}

impl Hold {
    fn owned(&self) -> bool {
        self.owner.pid == std::process::id() && self.owner.state() == ProcessState::Live
    }
}

fn stage(run: &Path) -> Option<Stage> {
    match kuvo_store::files::read(&run.join(HOLD)) {
        Ok(None) => None,
        Ok(Some(bytes)) => Some(
            serde_json::from_slice::<Hold>(&bytes)
                .ok()
                .filter(|hold| hold.format_version == 1)
                .map_or(Stage::Holding, |hold| hold.stage),
        ),
        Err(_) => Some(Stage::Holding),
    }
}

pub(crate) fn holding(run: &Path) -> bool {
    stage(run) == Some(Stage::Holding)
}

pub(crate) fn releasing(run: &Path) -> bool {
    stage(run) == Some(Stage::Releasing)
}

impl Engine {
    async fn release_hold(&self) -> io::Result<Option<Hold>> {
        let hold: Option<Hold> = disk::read(&self.home.join("run").join(HOLD)).await?;
        if hold.as_ref().is_some_and(|hold| hold.format_version != 1) {
            return Err(io::Error::other("The saved engine hold is unreadable."));
        }
        Ok(hold)
    }

    async fn save_release_hold(&self, hold: &Hold) -> io::Result<()> {
        disk::durable(self.home.join("run").join(HOLD), hold.clone()).await
    }

    /// Advance this once per fleet round, keeping the worker responsive while
    /// a turn is still running. An unknown or unowned worker cannot be retired.
    pub async fn drain_for_release(&self, release: &str, next: &str) -> io::Result<bool> {
        let mut hold = match self.release_hold().await? {
            Some(hold) if hold.release != release => {
                if hold.stage == Stage::Deferred {
                    kuvo_store::files::remove_file_synced(&self.home.join("run").join(HOLD))?;
                } else {
                    self.cancel_release_drain().await?;
                }
                return Ok(false);
            }
            Some(hold) if hold.stage != Stage::Holding => return Ok(false),
            Some(hold) => hold,
            None => {
                let record =
                    disk::read::<crate::launch::Record>(&self.home.join("run/engine.json")).await?;
                // A parked engine may start on demand between preparation and
                // selection. The old resident's pin still needs the hold.
                let from = record.as_ref().map_or(self.version.as_str(), |record| {
                    record.version.as_deref().unwrap_or_default()
                });
                if from == next {
                    return Ok(true);
                }
                let hold = Hold {
                    format_version: 1,
                    release: release.into(),
                    from: from.into(),
                    to: next.into(),
                    owner: ProcessIdentity::read(std::process::id())?
                        .ok_or_else(|| io::Error::other("The resident cannot identify itself."))?,
                    since: disk::now(),
                    stage: Stage::Holding,
                    retry_at: None,
                };
                self.save_release_hold(&hold).await?;
                hold
            }
        };
        if !hold.owned() {
            return Ok(false);
        }
        if disk::now().saturating_sub(hold.since) >= TIMING.release_drain_ms {
            hold.stage = Stage::Releasing;
            hold.retry_at = Some(disk::now() + TIMING.release_drain_retry_ms);
            self.save_release_hold(&hold).await?;
            return Ok(false);
        }
        if crate::session_move::writes_in_flight(&self.home).await?
            || !self.quiet_for_release().await?
        {
            return Ok(false);
        }
        self.keep_inboxes_with(Demand::Owned).await?;
        if crate::session_move::writes_in_flight(&self.home).await? {
            return Ok(false);
        }
        self.stop_for_release().await
    }

    pub async fn cancel_release_drain(&self) -> io::Result<()> {
        let Some(mut hold) = self.release_hold().await? else {
            return Ok(());
        };
        if hold.stage == Stage::Holding && !self.home.join("run/previous").exists() {
            hold.stage = Stage::Releasing;
            self.save_release_hold(&hold).await?;
        }
        Ok(())
    }

    /// Once the keeper confirms or rolls back, the new resident resumes the
    /// engine belonging to that release and lets the saved queue go in order.
    pub async fn recover_release_drain(&self) -> io::Result<()> {
        let Some(mut hold) = self.release_hold().await? else {
            return Ok(());
        };
        let path = self.home.join("run").join(HOLD);
        if self.home.join("run/previous").exists() || (hold.stage == Stage::Holding && hold.owned())
        {
            return Ok(());
        }
        if hold.stage == Stage::Deferred {
            if hold.retry_at.is_none_or(|at| disk::now() >= at) {
                kuvo_store::files::remove_file_synced(&path)?;
            }
            return Ok(());
        }
        if hold.stage == Stage::Holding {
            hold.stage = Stage::Releasing;
            self.save_release_hold(&hold).await?;
        }
        if self.queues.release_pending(self, None).await? {
            let Some(lease) = self.acquire(Demand::Start).await else {
                return Ok(());
            };
            self.release_waiting().await?;
            if self.queues.release_pending(self, Some(&lease)).await? {
                return Ok(());
            }
        }
        if crate::session_move::writes_in_flight(&self.home).await? {
            return Ok(());
        }
        if hold.retry_at.is_some_and(|at| disk::now() < at) {
            hold.stage = Stage::Deferred;
            self.save_release_hold(&hold).await
        } else {
            kuvo_store::files::remove_file_synced(&path)
        }
    }
}
