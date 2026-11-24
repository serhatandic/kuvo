//! Connect the keeper's update preparation to live setup and peer facts.

mod mail;

use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use kuvo_keeper::{
    fleet::{self, Candidate, Fetcher},
    manifest::{UPDATE_KEY, local_target},
    prepare_command::is_scratch,
    update_status,
};
use kuvo_peers::{
    directory::{PreparedRelease, UpdateFacts},
    setup_lists::{BridgeUpdate, ResidentPeers, View},
};
use kuvo_setup::SetupHandle;
use kuvo_store::{files, home::Home};
use serde_json::{Value, json};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::Instant,
};

const FEED: &str = "https://github.com/serhatandic/kuvo-code/releases/latest/download/boxes.json";
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const CHECK_KEPT: i64 = 24 * 60 * 60_000;
const UPDATING: &str = "It's updating already.";

pub struct Worker {
    checks: mpsc::Sender<Check>,
    task: JoinHandle<()>,
}

impl Worker {
    pub async fn stop(mut self) {
        self.task.abort();
        let _ = (&mut self.task).await;
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.task.abort();
    }
}

enum Check {
    Local(oneshot::Sender<io::Result<CheckReply>>),
    Peer,
}

enum CheckReply {
    Checking,
    Following(String),
    Busy,
}

struct FeedCheck {
    manual: bool,
    task: JoinHandle<io::Result<Candidate>>,
}

pub fn start(
    home: Home,
    profile: &Path,
    peers: Arc<ResidentPeers>,
    setup: SetupHandle,
) -> Option<Worker> {
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
    let (checks, mut requests) = mpsc::channel(8);
    let pack_server = mail
        .as_ref()
        .and_then(|mail| mail.serve(home.clone(), checks.clone()));
    let mut fleet = Fleet {
        home,
        peers,
        setup,
        key,
        url,
        test_feed: scratch,
        next_check: Instant::now() + delay,
        check_now: false,
        fetcher: Fetcher::default(),
        checking: None,
        bridge_fetch: None,
        next_bridge: Instant::now(),
        last_sent: None,
        attempted: None,
        pending: None,
        mail,
        pack_server,
    };
    let task = tokio::spawn(async move {
        let mut ticks = tokio::time::interval(Duration::from_secs(2));
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = ticks.tick() => {},
                request = requests.recv() => match request {
                    Some(Check::Local(answer)) => { let _ = answer.send(fleet.request_check(true)); }
                    Some(Check::Peer) => { let _ = fleet.request_check(false); }
                    None => return,
                }
            }
            if let Err(error) = fleet.round().await {
                eprintln!("Kuvo updates: {error}");
            }
        }
    });
    Some(Worker { checks, task })
}

pub fn register_check(
    routes: &mut kuvo_server::Registry,
    home: Home,
    profile: PathBuf,
    worker: Option<&Worker>,
) {
    use kuvo_protocol::routes::post_kuvo_update as update;
    use kuvo_server::{Access, Refusal, RouteReply};

    let checks = worker.map(|worker| worker.checks.clone());
    routes.register_decoded::<update::Spec, _, _, _>(
        Access::Any,
        |body| {
            serde_json::from_slice(body).map_err(|_| {
                Refusal::new(http::StatusCode::BAD_REQUEST, "Say to update now (now).")
            })
        },
        move |_| {
            let (home, profile, checks) = (home.clone(), profile.clone(), checks.clone());
            async move {
                let internal = |error: io::Error| {
                    Refusal::new(http::StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
                };
                let busy = || Refusal::new(http::StatusCode::CONFLICT, UPDATING);
                let mut status = update_status::read(&home, &profile).map_err(internal)?;
                if let Some(blocked) = status["blocked"].as_str() {
                    return Err(Refusal::new(http::StatusCode::CONFLICT, blocked));
                }
                let checks = checks.ok_or_else(busy)?;
                let (send, reply) = oneshot::channel();
                checks.try_send(Check::Local(send)).map_err(|_| busy())?;
                match reply.await.map_err(|_| busy())?.map_err(internal)? {
                    CheckReply::Busy => return Err(busy()),
                    CheckReply::Checking => status["updating"] = "checking".into(),
                    CheckReply::Following(message) => status["message"] = message.into(),
                }
                serde_json::from_value(status)
                    .map(RouteReply::json::<202>)
                    .map_err(|error| internal(error.into()))
            }
        },
    );
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
    check_now: bool,
    fetcher: Fetcher,
    checking: Option<FeedCheck>,
    bridge_fetch: Option<JoinHandle<io::Result<()>>>,
    next_bridge: Instant,
    last_sent: Option<(UpdateFacts, Option<BridgeUpdate>)>,
    attempted: Option<PreparedRelease>,
    pending: Option<JoinHandle<io::Result<()>>>,
    mail: Option<mail::Mail>,
    pack_server: Option<JoinHandle<()>>,
}

impl Drop for Fleet {
    fn drop(&mut self) {
        if let Some(checking) = &self.checking {
            checking.task.abort();
        }
        if let Some(bridge) = &self.bridge_fetch {
            bridge.abort();
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
    fn request_check(&mut self, local: bool) -> io::Result<CheckReply> {
        let reader = self.peers.reader();
        let view = reader.view();
        let now = reader.now();
        if let Some(main) = view
            .roster
            .main
            .filter(|main| Some(*main) != view.roster.here)
        {
            if !local {
                return Ok(CheckReply::Busy);
            }
            files::write_secret(
                &self.home.run().join("fleet-check.json"),
                &serde_json::to_vec(&json!({ "at": now }))?,
            )?;
            let named = view
                .roster
                .member(&main)
                .map(|member| member.name.as_str())
                .filter(|name| !name.is_empty());
            let name = named.unwrap_or("your main computer");
            let (away, _) = view.peer_updates(&main, now);
            let message = if away {
                let place = named.map_or_else(
                    || name.to_owned(),
                    |name| format!("{name} (your main computer)"),
                );
                let (installed, _) = fleet::installed(&self.home)?;
                match Candidate::kept(&self.home, &self.key)
                    .filter(|held| held.newer_than(installed.as_deref()))
                {
                    Some(held) => {
                        let version = held.version();
                        let short = version
                            .split_once("-beta.")
                            .map_or(version.to_owned(), |(_, beta)| format!("beta.{beta}"));
                        format!(
                            "{short} is ready. It installs on all your computers when {place} is back."
                        )
                    }
                    None => format!("Kuvo checks for updates when {place} is back."),
                }
            } else {
                format!("{name} is checking for updates.")
            };
            return Ok(CheckReply::Following(message));
        }
        if self.check_now
            || self.checking.is_some()
            || self.pending.is_some()
            || self.home.run().join("previous").try_exists()?
            || fleet::installed(&self.home)?.0.is_none()
        {
            return Ok(CheckReply::Busy);
        }
        self.check_now = true;
        Ok(CheckReply::Checking)
    }

    async fn send_pending_check(&self, view: &View, now: i64) -> io::Result<()> {
        let path = self.home.run().join("fleet-check.json");
        let Some(bytes) = files::read(&path)? else {
            return Ok(());
        };
        let kept = serde_json::from_slice::<Value>(&bytes).ok();
        if let (Some(kept), Some(main)) = (
            kept.as_ref().and_then(|kept| kept["at"].as_i64()),
            view.roster
                .main
                .filter(|main| Some(*main) != view.roster.here),
        ) && now.saturating_sub(kept) <= CHECK_KEPT
        {
            let (away, _) = view.peer_updates(&main, now);
            if away {
                return Ok(());
            }
            let Some((mail, host)) = self
                .mail
                .as_ref()
                .zip(view.roster.member(&main).and_then(|member| member.host()))
            else {
                return Ok(());
            };
            mail.request_check(&host).await?;
        }
        files::remove_file_synced(&path)
    }

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
        if let Err(error) = self.send_pending_check(&view, now).await {
            eprintln!("Asking the main to check Kuvo's release: {error}");
        }
        let mut candidate =
            Candidate::kept(&self.home, &self.key).filter(|next| next.newer_than(Some(&version)));
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
            if self
                .checking
                .as_ref()
                .is_some_and(|checking| checking.task.is_finished())
                && let Some(checking) = self.checking.take()
            {
                match checking.task.await.map_err(io::Error::other)? {
                    Ok(fetched) => {
                        if let Err(error) =
                            update_status::note_check(&self.home, fetched.version(), now)
                        {
                            eprintln!("Recording the release check: {error}");
                        }
                        if checking.manual
                            || self.test_feed
                            || fetched.old_enough_for_automatic_check()
                        {
                            self.consider(&mut candidate, fetched.envelope().to_owned(), &version)?;
                        }
                    }
                    Err(error) => eprintln!("Checking Kuvo's release: {error}"),
                }
            }
            if self.checking.is_none()
                && (self.check_now || Instant::now() >= self.next_check)
                && (standalone || self.setup.may_switch_fleet().await)
            {
                self.next_check = Instant::now() + CHECK_EVERY;
                let (fetcher, url, key) =
                    (self.fetcher.clone(), self.url.clone(), self.key.clone());
                self.checking = Some(FeedCheck {
                    manual: std::mem::take(&mut self.check_now),
                    task: tokio::spawn(async move { fetcher.candidate(&url, &key).await }),
                });
            }
        } else if let Some(main) = main {
            if let Some(checking) = self.checking.take() {
                checking.task.abort();
                self.next_check = Instant::now();
            }
            self.check_now = false;
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
        let running = running_manifest
            .as_ref()
            .and_then(|body| Candidate::verify(body.clone(), &self.key).ok());
        let installed_sha256 = running
            .as_ref()
            .and_then(|running| running.local().ok())
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
                let key = self.key.clone();
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
                    if let Some(bridge) = candidate.bridge(&key)? {
                        bridge.retain_bridge(&home, &fetcher).await?;
                    }
                    candidate.prepare(home, pack).await
                }));
            }
        }
        let prepared = facts.staged.is_some();
        let bridge = if local_main && let Some(running) = &running {
            self.bridge_offer(running, &view, now).await?
        } else {
            None
        };
        let publication = (facts, bridge);
        if self.last_sent.as_ref() != Some(&publication) {
            self.last_sent = Some(publication.clone());
            self.peers
                .announce_update(publication.0, publication.1)
                .await;
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

    async fn bridge_offer(
        &mut self,
        running: &Candidate,
        view: &View,
        now: i64,
    ) -> io::Result<Option<BridgeUpdate>> {
        let Some(bridge) = running.bridge(&self.key)? else {
            return Ok(None);
        };
        let recipients = view
            .roster
            .members
            .iter()
            .filter_map(|peer| {
                let (_, offered) = view.peer_updates(&peer.key, now);
                (offered.release_target.is_none() && bridge.newer_than(offered.kuvo.as_deref()))
                    .then_some(peer.key)
            })
            .collect();
        if self
            .bridge_fetch
            .as_ref()
            .is_some_and(JoinHandle::is_finished)
            && let Some(fetch) = self.bridge_fetch.take()
            && let Err(error) = fetch.await.map_err(io::Error::other)?
        {
            eprintln!("Keeping the TypeScript bridge: {error}");
        }
        let reference = bridge.local()?;
        let facts = if bridge.held_bridge(&self.home)? {
            UpdateFacts {
                kuvo: Some(reference.version),
                installed_sha256: Some(reference.sha256),
                running_manifest: Some(bridge.envelope().into()),
                ..UpdateFacts::default()
            }
        } else {
            if self.bridge_fetch.is_none() && Instant::now() >= self.next_bridge {
                self.next_bridge = Instant::now() + Duration::from_secs(60);
                let (home, fetcher) = (self.home.clone(), self.fetcher.clone());
                self.bridge_fetch = Some(tokio::spawn(async move {
                    bridge.retain_bridge(&home, &fetcher).await
                }));
            }
            UpdateFacts::default()
        };
        Ok(Some(BridgeUpdate { recipients, facts }))
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
