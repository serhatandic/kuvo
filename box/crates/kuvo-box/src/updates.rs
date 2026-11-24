//! Connect the keeper's update preparation to live setup and peer facts.

use std::{io, path::Path, sync::Arc, time::Duration};

use kuvo_keeper::{
    fleet::{self, Candidate, Fetcher},
    manifest::{UPDATE_KEY, local_target},
    prepare_command::is_scratch,
};
use kuvo_peers::{
    directory::{PreparedRelease, UpdateFacts},
    setup_lists::{ResidentPeers, View},
};
use kuvo_setup::SetupHandle;
use kuvo_store::home::Home;
use tokio::{task::JoinHandle, time::Instant};

const FEED: &str = "https://github.com/serhatandic/kuvo-code/releases/latest/download/boxes.json";
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

pub fn start(
    home: Home,
    profile: &Path,
    peers: Arc<ResidentPeers>,
    setup: SetupHandle,
) -> Option<JoinHandle<()>> {
    let config = std::env::var("KUVO_TEST_RELEASE_FEED")
        .ok()
        .zip(std::env::var("KUVO_TEST_RELEASE_KEY").ok());
    let scratch = is_scratch(&home).ok()?;
    let (url, key, delay) = match config {
        Some((url, key)) if scratch => (url, key, Duration::ZERO),
        _ if !scratch && fleet::checked_home(&home, profile) => {
            (FEED.into(), UPDATE_KEY.into(), Duration::from_secs(60))
        }
        _ => return None,
    };
    let mut fleet = Fleet {
        home,
        peers,
        setup,
        key,
        url,
        test_feed: scratch,
        next_check: Instant::now() + delay,
        fetcher: Fetcher::default(),
        last_sent: None,
        attempted: None,
        pending: None,
    };
    Some(tokio::spawn(async move {
        let mut ticks = tokio::time::interval(Duration::from_secs(2));
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticks.tick().await;
            if let Err(error) = fleet.round().await {
                eprintln!("Kuvo updates: {error}");
            }
        }
    }))
}

struct Fleet {
    home: Home,
    peers: Arc<ResidentPeers>,
    setup: SetupHandle,
    key: String,
    url: String,
    test_feed: bool,
    next_check: Instant,
    fetcher: Fetcher,
    last_sent: Option<UpdateFacts>,
    attempted: Option<PreparedRelease>,
    pending: Option<JoinHandle<io::Result<()>>>,
}

impl Drop for Fleet {
    fn drop(&mut self) {
        if let Some(pending) = &self.pending {
            pending.abort();
        }
    }
}

impl Fleet {
    async fn round(&mut self) -> io::Result<()> {
        if self.pending.as_ref().is_some_and(JoinHandle::is_finished)
            && let Some(pending) = self.pending.take()
            && let Err(error) = pending.await.map_err(io::Error::other)?
        {
            eprintln!("Preparing Kuvo's release: {error}");
        }
        let (version, working) = fleet::installed(&self.home)?;
        let Some(version) = version else {
            return Ok(());
        };
        let reader = self.peers.reader();
        let view = reader.view();
        let now = reader.now();
        let main = view.roster.main;
        let standalone = main.is_none() && view.roster.members.is_empty();
        let local_main = main.is_some() && main == view.roster.here;
        let mut candidate =
            Candidate::kept(&self.home, &self.key)?.filter(|next| next.newer_than(Some(&version)));
        let mut committed = false;
        if standalone || local_main {
            for peer in &view.roster.members {
                let (away, facts) = view.peer_updates(&peer.key, now);
                if !away {
                    for offer in [facts.release_manifest, facts.running_manifest]
                        .into_iter()
                        .flatten()
                    {
                        self.consider(&mut candidate, offer, &version)?;
                    }
                }
            }
            if Instant::now() >= self.next_check
                && (standalone || self.setup.may_switch_fleet().await)
            {
                self.next_check = Instant::now() + CHECK_EVERY;
                match self.fetcher.candidate(&self.url, &self.key).await {
                    Ok(fetched) if self.test_feed || fetched.old_enough_for_automatic_check() => {
                        self.consider(&mut candidate, fetched.envelope().to_owned(), &version)?
                    }
                    Ok(_) => {}
                    Err(error) => eprintln!("Checking Kuvo's release: {error}"),
                }
            }
        } else if let Some(main) = main {
            candidate = None;
            let (away, facts) = view.peer_updates(&main, now);
            if !away {
                if let (Some(running), Some(envelope)) =
                    (facts.kuvo.as_deref(), facts.running_manifest)
                    && let Ok(offer) = Candidate::verify(envelope, &self.key)
                    && offer.newer_than(Some(&version))
                    && offer.version() == running
                    && peer_installed(&offer, &facts.release_target, &facts.installed_sha256)
                {
                    candidate = Some(offer);
                    committed = true;
                }
                if candidate.is_none()
                    && let Some(envelope) = facts.release_manifest
                {
                    self.consider(&mut candidate, envelope, &version)?;
                }
            }
        }

        let running_manifest = working
            .as_ref()
            .and_then(|working| fleet::installed_envelope(&self.home, working));
        let target = local_target()?;
        let installed_sha256 = running_manifest
            .as_ref()
            .and_then(|body| Candidate::verify(body.clone(), &self.key).ok())
            .and_then(|release| release.local().ok())
            .map(|pack| pack.sha256);
        let mut facts = UpdateFacts {
            kuvo: Some(version),
            release_target: Some(target.into()),
            installed_sha256,
            running_manifest,
            ..UpdateFacts::default()
        };
        if let Some(candidate) = &candidate {
            let local = candidate.local()?;
            candidate.keep(&self.home)?;
            let prepared = PreparedRelease {
                version: local.version,
                sha256: local.sha256,
            };
            facts.candidate = Some(candidate.version().into());
            facts.release_manifest = Some(candidate.envelope().into());
            if candidate.is_prepared(&self.home)? {
                facts.staged = Some(prepared.clone());
            } else if self.pending.is_none()
                && self.attempted.as_ref() != Some(&prepared)
                && !fleet::stopped(&self.home)
            {
                self.attempted = Some(prepared);
                let candidate = candidate.clone();
                let home = self.home.clone();
                let fetcher = self.fetcher.clone();
                self.pending = Some(tokio::spawn(candidate.prepare(home, fetcher)));
            }
        }
        let prepared = facts.staged.is_some();
        if self.last_sent.as_ref() != Some(&facts) {
            self.last_sent = Some(facts.clone());
            self.peers.announce_update(facts).await;
        }

        if let Some(candidate) = candidate {
            if standalone || local_main {
                let (ready, reachable) = barrier(&view, now, &candidate);
                if candidate.primary_may_switch(&self.home, now, ready, reachable)?
                    && (standalone || self.setup.may_switch_fleet().await)
                {
                    candidate.activate(&self.home).await?;
                }
            } else if prepared
                && committed
                && self
                    .setup
                    .rosters()
                    .borrow()
                    .as_ref()
                    .is_some_and(|roster| roster.main == main)
            {
                candidate.activate(&self.home).await?;
            }
        }
        Ok(())
    }

    fn consider(
        &self,
        choice: &mut Option<Candidate>,
        envelope: String,
        installed: &str,
    ) -> io::Result<()> {
        if let Ok(given) = Candidate::verify(envelope, &self.key)
            && given.newer_than(Some(installed))
            && given.newer_than(choice.as_ref().map(Candidate::version))
        {
            given.keep(&self.home)?;
            *choice = Some(given);
        }
        Ok(())
    }
}

fn peer_installed(offer: &Candidate, target: &Option<String>, hash: &Option<String>) -> bool {
    target
        .as_deref()
        .zip(hash.as_deref())
        .is_some_and(|(target, hash)| offer.matches_pack(target, hash))
}

fn barrier(view: &View, now: i64, candidate: &Candidate) -> (usize, usize) {
    let mut ready = 0;
    let mut reachable = 0;
    for computer in &view.roster.members {
        let (away, facts) = view.peer_updates(&computer.key, now);
        if away {
            continue;
        }
        reachable += 1;
        if facts
            .kuvo
            .as_deref()
            .is_some_and(|version| !candidate.newer_than(Some(version)))
            || facts.staged.as_ref().is_some_and(|stage| {
                stage.version == candidate.version()
                    && peer_installed(
                        candidate,
                        &facts.release_target,
                        &Some(stage.sha256.clone()),
                    )
            })
        {
            ready += 1;
        }
    }
    (ready, reachable)
}
