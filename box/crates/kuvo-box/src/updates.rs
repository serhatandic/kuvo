//! Connect the keeper's update preparation to live setup and peer facts.

mod mail;

use std::{io, path::Path, sync::Arc, time::Duration};

use kuvo_keeper::{
    fleet::{self, Candidate, Fetcher},
    manifest::{UPDATE_KEY, local_target},
    prepare_command::is_scratch,
    update_status,
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
    let mail = peers
        .relay_handle()
        .map(|relay| mail::Mail::new(relay, peers.reader(), key.clone()));
    let pack_server = mail.as_ref().and_then(|mail| mail.serve(home.clone()));
    let mut fleet = Fleet {
        home,
        peers,
        setup,
        key,
        url,
        test_feed: scratch,
        next_check: Instant::now() + delay,
        fetcher: Fetcher::default(),
        checking: None,
        last_sent: None,
        attempted: None,
        pending: None,
        mail,
        pack_server,
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

pub fn register_helper_stop(routes: &mut kuvo_server::Registry, home: Home, mac: bool) {
    use kuvo_protocol::routes::post_kuvo_box_stop as stop;
    use kuvo_server::{Access, Caller, Refusal, RouteReply, Transport};

    routes.register::<stop::Spec, _, _>(Access::Any, move |call| {
        let home = home.clone();
        async move {
            if !matches!(call.request.caller, Caller::Local(Transport::Http { .. })) {
                return Err(Refusal::new(
                    http::StatusCode::FORBIDDEN,
                    "Kuvo on this Mac is stopped only on this Mac.",
                ));
            }
            if !cfg!(target_os = "macos") || !mac {
                return Err(Refusal::new(
                    http::StatusCode::CONFLICT,
                    "Only Kuvo on a Mac, run by Kuvo Helper, is stopped this way.",
                ));
            }
            #[cfg(target_os = "macos")]
            kuvo_keeper::mac_helper::pause(&home).map_err(|error| {
                Refusal::new(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Kuvo on this Mac couldn't be stopped: {error}"),
                )
            })?;
            #[cfg(not(target_os = "macos"))]
            let _ = home;
            Ok(RouteReply::json::<202>(stop::Status202 {
                stopping: Default::default(),
            }))
        }
    });
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
    checking: Option<JoinHandle<io::Result<Candidate>>>,
    last_sent: Option<UpdateFacts>,
    attempted: Option<PreparedRelease>,
    pending: Option<JoinHandle<io::Result<()>>>,
    mail: Option<mail::Mail>,
    pack_server: Option<JoinHandle<()>>,
}

impl Drop for Fleet {
    fn drop(&mut self) {
        if let Some(checking) = &self.checking {
            checking.abort();
        }
        if let Some(pending) = &self.pending {
            pending.abort();
        }
        if let Some(server) = &self.pack_server {
            server.abort();
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
            if self.checking.as_ref().is_some_and(JoinHandle::is_finished)
                && let Some(checking) = self.checking.take()
            {
                match checking.await.map_err(io::Error::other)? {
                    Ok(fetched) => {
                        if let Err(error) =
                            update_status::note_check(&self.home, fetched.version(), now)
                        {
                            eprintln!("Recording the release check: {error}");
                        }
                        if self.test_feed || fetched.old_enough_for_automatic_check() {
                            self.consider(&mut candidate, fetched.envelope().to_owned(), &version)?;
                        }
                    }
                    Err(error) => eprintln!("Checking Kuvo's release: {error}"),
                }
            }
            if self.checking.is_none()
                && Instant::now() >= self.next_check
                && (standalone || self.setup.may_switch_fleet().await)
            {
                self.next_check = Instant::now() + CHECK_EVERY;
                let (fetcher, url, key) =
                    (self.fetcher.clone(), self.url.clone(), self.key.clone());
                self.checking = Some(tokio::spawn(
                    async move { fetcher.candidate(&url, &key).await },
                ));
            }
        } else if let Some(main) = main {
            if let Some(checking) = self.checking.take() {
                checking.abort();
                self.next_check = Instant::now();
            }
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
                let mail = self.mail.clone();
                self.pending = Some(tokio::spawn(async move {
                    let pack = match candidate.download(&fetcher).await {
                        Ok(pack) => pack,
                        Err(error) => {
                            let Some(mail) = mail else {
                                return Err(error);
                            };
                            mail.pull(&candidate.local()?).await.map_err(|_| error)?
                        }
                    };
                    candidate.prepare(home, pack).await
                }));
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
