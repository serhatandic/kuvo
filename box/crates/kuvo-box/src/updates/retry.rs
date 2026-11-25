//! A failed preparation is recorded against its signed target.

use std::io;

use http::StatusCode;
use kuvo_keeper::{
    fleet::{self, Candidate},
    update_status,
};
use kuvo_peers::directory::{CannotUpdate, PreparedRelease, UpdateFacts};
use kuvo_protocol::routes::post_kuvo_update_retry as route;
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use kuvo_store::{files, home::Home};
use tokio::sync::{mpsc, oneshot};

use super::{Check, Fleet};

pub(super) type Request = route::Request;
pub(super) type Answer = route::Status202;
const RECORD: &str = "fleet-cannot.json";

pub(super) struct Failure {
    pub(super) record: CannotUpdate,
    attempted: bool,
}

impl Failure {
    pub(super) fn download(release: PreparedRelease, error: io::Error) -> Self {
        Self::new(release, error, true, false)
    }

    pub(super) fn prepare(release: PreparedRelease, error: io::Error) -> Self {
        use io::ErrorKind::{
            NotConnected, PermissionDenied, QuotaExceeded, StorageFull, TimedOut, WouldBlock,
        };
        let fixable = matches!(
            error.kind(),
            NotConnected | PermissionDenied | QuotaExceeded | StorageFull | TimedOut | WouldBlock
        );
        Self::new(release, error, fixable, true)
    }

    pub(super) fn bad_archive(release: PreparedRelease) -> Self {
        Self::new(
            release,
            io::Error::other("The release's files aren't what it says."),
            false,
            false,
        )
    }

    fn new(release: PreparedRelease, error: io::Error, fixable: bool, attempted: bool) -> Self {
        let reason = kuvo_keeper::fleet::release_error_message(&error)
            .chars()
            .take(300)
            .collect();
        Self {
            record: CannotUpdate {
                release,
                reason,
                fixable,
            },
            attempted,
        }
    }

    pub(super) fn save(&self, home: &Home) -> io::Result<()> {
        files::write_secret(&home.run().join(RECORD), &serde_json::to_vec(&self.record)?)
    }

    pub(super) fn outcome(self) -> update_status::Outcome {
        update_status::Outcome::Failed {
            version: self.attempted.then_some(self.record.release.version),
            message: self.record.reason,
        }
    }

    pub(super) fn for_release(
        home: &Home,
        release: &PreparedRelease,
    ) -> io::Result<Option<CannotUpdate>> {
        if let Some(reason) = update_status::trial_failure(home, &release.version, &release.sha256)
        {
            return Ok(Some(CannotUpdate {
                release: release.clone(),
                reason,
                fixable: false,
            }));
        }
        Ok(files::read(&home.run().join(RECORD))?
            .and_then(|bytes| serde_json::from_slice::<CannotUpdate>(&bytes).ok())
            .filter(|failure| &failure.release == release))
    }
}

pub(super) fn register(routes: &mut Registry, checks: Option<mpsc::Sender<Check>>) {
    let off = || {
        Refusal::new(
            StatusCode::CONFLICT,
            "This computer has no update to retry.",
        )
    };
    routes.register_decoded::<route::Spec, _, _, _>(
        Access::Any,
        |body| {
            serde_json::from_slice::<Request>(body)
                .ok()
                .filter(|given| {
                    given.key.as_ref().is_none_or(|key| {
                        key.len() == 43 && kuvo_wire::keys::decode_exact::<32>(key).is_some()
                    })
                })
                .ok_or_else(|| {
                    Refusal::new(StatusCode::BAD_REQUEST, "Send the setup’s target version.")
                })
        },
        move |call| {
            let checks = checks.clone();
            async move {
                let checks = checks.ok_or_else(off)?;
                let (answer, reply) = oneshot::channel();
                checks
                    .try_send(Check::Retry(call.body, answer))
                    .map_err(|_| {
                        Refusal::new(StatusCode::CONFLICT, "A retry is already scheduled.")
                    })?;
                Ok(RouteReply::json::<202>(reply.await.map_err(|_| off())??))
            }
        },
    );
}

fn changed() -> Refusal {
    Refusal::new(StatusCode::CONFLICT, "The setup’s target version changed.")
}

fn file_error(error: io::Error) -> Refusal {
    Refusal::new(
        StatusCode::CONFLICT,
        format!("Couldn’t schedule the retry: {error}"),
    )
}

pub(super) fn signed_failure<'a>(
    facts: &'a UpdateFacts,
    candidate: &Candidate,
) -> Option<&'a CannotUpdate> {
    facts.cannot.as_ref().filter(|failure| {
        failure.release.version == candidate.version()
            && facts.switching.as_ref() != Some(&failure.release)
            && candidate.newer_than(facts.kuvo.as_deref())
            && facts
                .release_target
                .as_deref()
                .is_some_and(|target| candidate.matches_pack(target, &failure.release.sha256))
    })
}

impl Fleet {
    pub(super) async fn retry(&mut self, given: Request) -> Result<Answer, Refusal> {
        let reader = self.peers.reader();
        let view = reader.view();
        let digest = given.target_sha256.as_deref();
        let Some(key) = given
            .key
            .as_deref()
            .and_then(kuvo_wire::keys::decode_exact::<32>)
            .filter(|key| view.roster.here != Some(*key))
        else {
            return self.retry_local(&given.target, digest);
        };
        let peer = view.roster.member(&key).ok_or_else(|| {
            Refusal::new(StatusCode::NOT_FOUND, "That isn’t one of your computers.")
        })?;
        let (_, candidate) = self.retry_target(&given.target)?;
        let (away, facts) = view.peer_updates(&key, reader.now());
        let failure = signed_failure(&facts, &candidate)
            .filter(|failure| !away && failure.fixable)
            .ok_or_else(|| {
                Refusal::new(
                    StatusCode::CONFLICT,
                    format!("{} has no update to retry.", peer.name),
                )
            })?;
        if digest.is_some_and(|hash| hash != failure.release.sha256) {
            return Err(changed());
        }
        let unavailable = || {
            Refusal::new(
                StatusCode::SERVICE_UNAVAILABLE,
                format!(
                    "{} can’t be reached right now. Try again once it’s connected.",
                    peer.name
                ),
            )
        };
        let (mail, host) = self
            .mail
            .as_ref()
            .zip(peer.host())
            .ok_or_else(unavailable)?;
        mail.request_retry(&host, &failure.release)
            .await
            .map_err(|_| unavailable())?;
        Ok(failure.row(facts.kuvo.clone(), facts.installed_sha256.clone(), false))
    }

    pub(super) fn retry_local(
        &mut self,
        version: &str,
        digest: Option<&str>,
    ) -> Result<Answer, Refusal> {
        let (current, candidate) = self.retry_target(version)?;
        if !candidate.newer_than(current.as_deref()) {
            return Err(changed());
        }
        let target = candidate.local().map_err(file_error)?;
        if digest.is_some_and(|hash| hash != target.sha256) {
            return Err(changed());
        }
        let selected = target.target().map_err(file_error)?;
        let on_trial = fleet::trial_release(&self.home)
            .map_err(file_error)?
            .as_ref()
            == Some(&selected);
        let prepared = PreparedRelease {
            version: target.version,
            sha256: target.sha256,
        };
        let failure = Failure::for_release(&self.home, &prepared)
            .map_err(file_error)?
            .filter(|failure| failure.fixable && !on_trial)
            .ok_or_else(|| {
                Refusal::new(
                    StatusCode::CONFLICT,
                    "This computer has no update to retry.",
                )
            })?;
        let answer = failure.row(current, None, false);
        files::remove_file_synced(&self.home.run().join(RECORD)).map_err(file_error)?;
        self.attempted = None;
        Ok(answer)
    }

    fn retry_target(&self, version: &str) -> Result<(Option<String>, Candidate), Refusal> {
        let (current, running) = fleet::installed(&self.home).map_err(file_error)?;
        let candidate = Candidate::kept(&self.home, &self.key)
            .filter(|candidate| {
                candidate.newer_than(current.as_deref())
                    || Some(candidate.version()) == current.as_deref()
            })
            .or_else(|| {
                let envelope = fleet::installed_envelope(&self.home, &running?)?;
                Candidate::verify(envelope, &self.key).ok()
            })
            .filter(|candidate| candidate.version() == version)
            .ok_or_else(changed)?;
        Ok((current, candidate))
    }
}
