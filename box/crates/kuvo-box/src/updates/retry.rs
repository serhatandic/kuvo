//! A failed preparation is kept against its signed target until explicitly retried.

use std::io;

use http::StatusCode;
use kuvo_keeper::fleet::{self, Candidate};
use kuvo_peers::directory::PreparedRelease;
use kuvo_protocol::routes::post_kuvo_update_retry as route;
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use kuvo_store::{files, home::Home};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{mpsc, oneshot};

use super::{Check, Fleet};

pub(super) type Request = route::Request;
pub(super) type Answer = route::Status202;
const RECORD: &str = "fleet-cannot.json";

#[derive(Deserialize, Serialize)]
pub(super) struct Failure {
    #[serde(flatten)]
    release: PreparedRelease,
    pub reason: String,
    fixable: bool,
}

impl Failure {
    pub(super) fn download(release: PreparedRelease, error: io::Error) -> Self {
        Self::new(release, error, true)
    }

    pub(super) fn prepare(release: PreparedRelease, error: io::Error) -> Self {
        use io::ErrorKind::{PermissionDenied, QuotaExceeded, StorageFull, TimedOut, WouldBlock};
        let fixable = matches!(
            error.kind(),
            PermissionDenied | QuotaExceeded | StorageFull | TimedOut | WouldBlock
        );
        Self::new(release, error, fixable)
    }

    fn new(release: PreparedRelease, error: io::Error, fixable: bool) -> Self {
        let reason = error.to_string().chars().take(300).collect();
        Self {
            release,
            reason,
            fixable,
        }
    }

    pub(super) fn save(&self, home: &Home) -> io::Result<()> {
        if self.fixable {
            files::write_secret(&home.run().join(RECORD), &serde_json::to_vec(self)?)?;
        }
        Ok(())
    }

    pub(super) fn for_release(home: &Home, release: &PreparedRelease) -> io::Result<Option<Self>> {
        Ok(files::read(&home.run().join(RECORD))?
            .and_then(|bytes| serde_json::from_slice::<Self>(&bytes).ok())
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

impl Fleet {
    pub(super) fn retry(&mut self, given: Request) -> Result<Answer, Refusal> {
        let conflict = |message| Refusal::new(StatusCode::CONFLICT, message);
        let file_error = |error: io::Error| {
            Refusal::new(
                StatusCode::CONFLICT,
                format!("Couldn’t schedule the retry: {error}"),
            )
        };
        let view = self.peers.reader().view();
        let peer = if let Some(key) = given
            .key
            .as_deref()
            .and_then(kuvo_wire::keys::decode_exact::<32>)
            && view.roster.here != Some(key)
        {
            Some(view.roster.member(&key).ok_or_else(|| {
                Refusal::new(StatusCode::NOT_FOUND, "That isn’t one of your computers.")
            })?)
        } else {
            None
        };
        let changed = || conflict("The setup’s target version changed.");
        let (current, _) = fleet::installed(&self.home).map_err(file_error)?;
        let target = Candidate::kept(&self.home, &self.key)
            .filter(|candidate| candidate.newer_than(current.as_deref()))
            .ok_or_else(changed)?
            .local()
            .map_err(file_error)?;
        if given.target != target.version
            || given
                .target_sha256
                .as_deref()
                .is_some_and(|hash| hash != target.sha256)
        {
            return Err(changed());
        }
        if let Some(peer) = peer {
            return Err(Refusal::new(
                StatusCode::CONFLICT,
                format!("{} has no update to retry.", peer.name),
            ));
        }
        let prepared = PreparedRelease {
            version: target.version,
            sha256: target.sha256,
        };
        let failure = Failure::for_release(&self.home, &prepared)
            .map_err(file_error)?
            .filter(|failure| failure.fixable)
            .ok_or_else(|| conflict("This computer has no update to retry."))?;
        let answer = serde_json::from_value(json!({
            "current":current, "target":prepared.version, "targetSha256":prepared.sha256,
            "canRetry":false, "state":"cannot", "reason":failure.reason, "fixable":true
        }))
        .map_err(|error| file_error(error.into()))?;
        files::remove_file_synced(&self.home.run().join(RECORD)).map_err(file_error)?;
        self.attempted = None;
        Ok(answer)
    }
}
