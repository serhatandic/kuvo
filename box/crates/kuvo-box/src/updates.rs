//! Connect the keeper's update preparation to live setup and peer facts.

mod mail;
mod progress;
mod released_machines;
mod retry;
mod summary;

use kuvo_os::timer_fires::{self, Timer};
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use kuvo_keeper::{
    fleet::{self, Candidate, FeedRelease, Fetcher},
    manifest::{UPDATE_KEY, local_target},
    prepare_command::is_scratch,
    update_status::{self, Outcome, Step},
};
use kuvo_peers::{
    directory::{PreparedRelease, ReleaseProgress, UpdateFacts},
    reach::UpdatePresence,
    setup_lists::{BridgeAdmission, BridgeUpdate, Reader, ResidentPeers, UpdatePublication, View},
};
use kuvo_setup::SetupHandle;
use kuvo_store::{files, home::Home};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
    time::Instant,
};

const FEED: &str = "https://github.com/serhatandic/kuvo-code/releases/latest/download/boxes.json";
/// Each release's own files, by `v<version>/`, for repairing the one a computer runs.
pub(crate) const RELEASES: &str = "https://github.com/serhatandic/kuvo-code/releases/download";
const CHECK_EVERY_MS: i64 = 6 * 3_600_000;
const CHECK_POLL: Duration = Duration::from_secs(10 * 60);
const CHECK_KEPT: i64 = 24 * 60 * 60_000;
const UPDATING: &str = "It's updating already.";

pub struct Worker {
    checks: mpsc::Sender<Check>,
    step: watch::Receiver<Option<Step>>,
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
    Offered,
    Retry(
        retry::Request,
        oneshot::Sender<Result<retry::Answer, kuvo_server::Refusal>>,
    ),
    PeerRetry(PreparedRelease),
}

enum CheckReply {
    Checking,
    Following(String),
    Busy,
}

struct FeedCheck {
    started: Instant,
    manual: bool,
    round: Option<String>,
    task: JoinHandle<io::Result<Option<FeedRelease>>>,
}

fn check_offset(home: &Home) -> i64 {
    let name = hostname::get().unwrap_or_default();
    let key = Sha256::digest(format!(
        "{} {}",
        name.to_string_lossy(),
        home.root().display()
    ));
    i64::from(u32::from_be_bytes(
        key[..4].try_into().expect("SHA-256 has four bytes"),
    )) % CHECK_EVERY_MS
}

fn release_round(now: i64, offset: i64) -> String {
    now.saturating_add(offset)
        .div_euclid(CHECK_EVERY_MS)
        .to_string()
}

fn feed(home: &Home, profile: &Path) -> Option<(String, String, bool)> {
    let config = std::env::var("KUVO_TEST_RELEASE_FEED")
        .ok()
        .zip(std::env::var("KUVO_TEST_RELEASE_KEY").ok());
    let scratch = is_scratch(home).ok()?;
    match config {
        Some((url, key)) if scratch => Some((url, key, true)),
        _ if !scratch && fleet::checked_home(home, profile) => {
            Some((FEED.into(), UPDATE_KEY.into(), false))
        }
        _ => None,
    }
}

pub fn installed_bridge(home: &Home, profile: &Path) -> Option<BridgeAdmission> {
    let (_, key, _) = feed(home, profile)?;
    let selected = fleet::selected(home).ok()??;
    let running = Candidate::verify(fleet::installed_envelope(home, &selected)?, &key).ok()?;
    if running.local().ok()?.target().ok()? != selected {
        return None;
    }
    let bridge = running.bridge(&key).ok()??.local().ok()?;
    let local_trial = fleet::installed(home).ok()?.1.is_some_and(|working| {
        working != selected && bridge.target().is_ok_and(|target| target == working)
    });
    Some(BridgeAdmission {
        release: PreparedRelease {
            version: bridge.version,
            sha256: bridge.sha256,
        },
        local_trial,
    })
}

/// The retained manual engine copies have their own lifetime, including on a
/// machine that never checks a feed and a Mac whose Helper serves it.
pub fn sweep_backups(home: Home, own_mac: bool) -> JoinHandle<()> {
    let delay = if own_mac || is_scratch(&home).unwrap_or(false) {
        Duration::ZERO
    } else {
        Duration::from_secs(60)
    };
    let every = if own_mac {
        Duration::from_secs(30 * 60)
    } else {
        CHECK_POLL
    };
    tokio::spawn(async move {
        let mut ticks = timer_fires::interval_at(Timer::BackupPrune, Instant::now() + delay, every);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticks.tick().await;
            let home = home.clone();
            if let Err(error) =
                tokio::task::spawn_blocking(move || fleet::prune_engine_backups(&home))
                    .await
                    .map_err(io::Error::other)
                    .and_then(|result| result)
            {
                eprintln!("Removing expired engine backup copies: {error}");
            }
        }
    })
}

pub fn start(
    home: Home,
    profile: &Path,
    peers: Arc<ResidentPeers>,
    setup: SetupHandle,
    engine: kuvo_engine::Engine,
) -> Option<Worker> {
    let (url, key, scratch) = feed(&home, profile)?;
    let delay = if scratch {
        Duration::ZERO
    } else {
        Duration::from_secs(60)
    };
    let mail = peers
        .relay_handle()
        .map(|relay| mail::Mail::new(relay, peers.reader(), key.clone()));
    let (checks, mut requests) = mpsc::channel(8);
    let (progress, step) = watch::channel(None);
    let pack_server = mail
        .as_ref()
        .and_then(|mail| mail.serve(home.clone(), checks.clone()));
    let summary = summary::History::load(&home);
    let mut fleet = Fleet {
        check_offset: check_offset(&home),
        step: progress,
        home,
        peers,
        setup,
        engine,
        key,
        url,
        early_fixture: scratch && std::env::var_os("KUVO_TEST_RELEASE_AGE").is_none(),
        next_check: Instant::now() + delay,
        check_now: false,
        fetcher: Fetcher::default(),
        checking: None,
        bridge_fetch: None,
        next_bridge: Instant::now(),
        released_machines: released_machines::Checks::new(scratch),
        last_sent: None,
        summary,
        attempted: None,
        receiving: None,
        pending: None,
        mail,
        pack_server,
    };
    let task = tokio::spawn(async move {
        let mut peers = fleet.peers.reader();
        let mut update_view = peers.view();
        let mut setup = fleet.setup.clone();
        let mut engine = fleet.engine.states();
        let mut files = kuvo_os::watch::FileSet::new(kuvo_os::watch::WatchHub::default());
        let home = &fleet.home;
        files.set_files(vec![
            home.root().join(kuvo_keeper::SELECTOR),
            home.root().join("machines.json"),
            home.updates().join("release/boxes.json"),
            home.run().join("off"),
            home.run().join("paused"),
            home.run().join("stopped"),
            home.run().join("previous"),
            home.run().join("fleet-ready.json"),
            home.run().join("fleet-check.json"),
            home.run().join("engine.json"),
        ]);
        loop {
            if let Err(error) = fleet.round().await {
                eprintln!("Kuvo updates: {error}");
            }
            let deadline = fleet.next_round();
            tokio::select! {
                _ = async { match deadline { Some(at) => timer_fires::sleep_until(Timer::UpdateDeadline, at).await, None => std::future::pending().await } } => {},
                changed = update_peers_changed(&mut peers, &mut update_view) => if changed.is_err() { return },
                changed = setup.changed() => if !changed { return },
                changed = engine.changed() => if changed.is_err() { return },
                _ = files.changed() => {},
                request = requests.recv() => match request {
                    Some(Check::Local(answer)) => { let _ = answer.send(fleet.request_check(true)); }
                    Some(Check::Peer) => { let _ = fleet.request_check(false); }
                    Some(Check::Offered) => {}
                    Some(Check::Retry(given, answer)) => { let _ = answer.send(fleet.retry(given).await); }
                    Some(Check::PeerRetry(release)) => { let _ = fleet.retry_local(&release.version, Some(&release.sha256)); }
                    None => return,
                }
            }
        }
    });
    Some(Worker { checks, step, task })
}

/// Transfer chunks, conversation mail and local publications also advance the shared
/// peer watch. Do not run a fleet round for every chunk; only changes to the inputs
/// of the release barrier and its rows need an immediate round.
async fn update_peers_changed(
    peers: &mut Reader,
    before: &mut Arc<View>,
) -> Result<(), watch::error::RecvError> {
    loop {
        peers.changed().await?;
        let now = peers.now();
        let after = peers.view();
        let changed = before.relay_connected != after.relay_connected
            || before.roster.here != after.roster.here
            || before.roster.main != after.roster.main
            || before.roster.name != after.roster.name
            || before.roster.members.len() != after.roster.members.len()
            || before
                .roster
                .members
                .iter()
                .zip(&after.roster.members)
                .any(|(a, b)| {
                    a.key != b.key
                        || a.address != b.address
                        || a.name != b.name
                        || a.kind != b.kind
                        || before.peer_updates(&a.key, now) != after.peer_updates(&b.key, now)
                });
        *before = after;
        if changed {
            return Ok(());
        }
    }
}

#[derive(Clone)]
struct Status {
    home: Home,
    profile: PathBuf,
    engine: kuvo_engine::Engine,
    step: Option<watch::Receiver<Option<Step>>>,
}

impl Status {
    async fn read(&self) -> io::Result<Value> {
        let mut body = update_status::read(&self.home, &self.profile)?;
        if let Some(step) = self.step.as_ref().and_then(|step| *step.borrow()) {
            body["updating"] = step.words().into();
        }
        if let Some((change, installing)) = self.engine.release_change().await? {
            body["engineChange"] = change;
            if installing {
                body["updating"] = Step::Installing.words().into();
            }
        }
        Ok(body)
    }
}

pub fn register_check(
    routes: &mut kuvo_server::Registry,
    home: Home,
    profile: PathBuf,
    worker: Option<&Worker>,
    engine: kuvo_engine::Engine,
) {
    use kuvo_protocol::routes::{get_kuvo_update as read, post_kuvo_update as update};
    use kuvo_server::{Access, Refusal, RouteReply};

    let checks = worker.map(|worker| worker.checks.clone());
    let status = Status {
        home,
        profile,
        engine,
        step: worker.map(|worker| worker.step.clone()),
    };
    let read_status = status.clone();
    routes.register::<read::Spec, _, _>(Access::Any, move |_| {
        let status = read_status.clone();
        async move {
            let result: io::Result<_> =
                async { Ok(serde_json::from_value(status.read().await?)?) }.await;
            result.map(RouteReply::json::<200>).map_err(|error| {
                Refusal::new(http::StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
            })
        }
    });
    retry::register(routes, checks.clone());
    register_offer(routes, status.clone(), checks.clone());
    routes.register_decoded::<update::Spec, _, _, _>(
        Access::Any,
        |body| {
            serde_json::from_slice(body).map_err(|_| {
                Refusal::new(http::StatusCode::BAD_REQUEST, "Say to update now (now).")
            })
        },
        move |_| {
            let (status, checks) = (status.clone(), checks.clone());
            async move {
                let internal = |error: io::Error| {
                    Refusal::new(http::StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
                };
                let busy = || Refusal::new(http::StatusCode::CONFLICT, UPDATING);
                let mut status = status.read().await.map_err(internal)?;
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

fn register_offer(
    routes: &mut kuvo_server::Registry,
    status: Status,
    checks: Option<mpsc::Sender<Check>>,
) {
    use http::StatusCode;
    use kuvo_protocol::routes::post_kuvo_box_update as offer;
    use kuvo_server::{Access, Caller, LocalOrigin, Refusal, RouteReply, Transport};

    let key =
        feed(&status.home, &status.profile).map_or_else(|| UPDATE_KEY.into(), |(_, key, _)| key);
    let accepting = Arc::new(tokio::sync::Mutex::new(()));
    routes.register::<offer::Spec, _, _>(Access::Any, move |call| {
        let (status, checks, key, accepting) = (
            status.clone(),
            checks.clone(),
            key.clone(),
            accepting.clone(),
        );
        async move {
            if !matches!(
                call.request.caller,
                Caller::Local(Transport::Http {
                    origin: LocalOrigin::App | LocalOrigin::Mac,
                    ..
                })
            ) {
                return Err(Refusal::new(
                    StatusCode::FORBIDDEN,
                    "Only Kuvo Code offers Kuvo on this computer a release.",
                ));
            }
            let body = serde_json::from_slice::<Value>(&call.request.body).unwrap_or_default();
            let source = body["from"]
                .as_str()
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .ok_or_else(|| {
                    Refusal::new(
                        StatusCode::BAD_REQUEST,
                        "Send the full path to the release folder.",
                    )
                })?;
            let guard = accepting.lock_owned().await;
            let offered_home = status.home.clone();
            tokio::task::spawn_blocking(move || {
                let _guard = guard;
                Candidate::receive_offer(&offered_home, &source, &key)
            })
            .await
            .map_err(|_| {
                Refusal::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Couldn’t keep the signed release.",
                )
            })?
            .map_err(|error| Refusal::new(StatusCode::UNPROCESSABLE_ENTITY, error.to_string()))?;
            if let Some(checks) = checks {
                let _ = checks.try_send(Check::Offered);
            }
            let status = status.read().await.map_err(|error| {
                Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
            })?;
            serde_json::from_value(status)
                .map(RouteReply::json::<202>)
                .map_err(|error| Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
        }
    });
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
    engine: kuvo_engine::Engine,
    key: String,
    url: String,
    early_fixture: bool,
    next_check: Instant,
    check_offset: i64,
    step: watch::Sender<Option<Step>>,
    check_now: bool,
    fetcher: Fetcher,
    checking: Option<FeedCheck>,
    bridge_fetch: Option<JoinHandle<io::Result<()>>>,
    next_bridge: Instant,
    released_machines: released_machines::Checks,
    last_sent: Option<Arc<UpdatePublication>>,
    summary: summary::History,
    attempted: Option<PreparedRelease>,
    receiving: Option<(PreparedRelease, mail::PullProgress)>,
    pending: Option<JoinHandle<Result<PreparedRelease, retry::Failure>>>,
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
    /// An idle fleet waits for its next check. An actual release in flight still needs a
    /// short deadline for progress, waiting for the engine to drain and the switch barrier.
    fn next_round(&self) -> Option<Instant> {
        let view = self.peers.reader().view();
        let primary = view.roster.main.is_none() && view.roster.members.is_empty()
            || view.roster.main.is_some() && view.roster.main == view.roster.here;
        let active = self.checking.is_some()
            || self.bridge_fetch.is_some()
            || self.pending.is_some()
            || self
                .last_sent
                .as_ref()
                .is_some_and(|sent| sent.facts.candidate.is_some())
            || self.home.run().join("previous").exists();
        let now = Instant::now();
        if active {
            return Some(now + Duration::from_secs(2));
        }
        let check = primary.then(|| self.next_check.max(now + Duration::from_secs(30)));
        let installed_release = self
            .last_sent
            .as_ref()
            .is_some_and(|sent| sent.facts.running_manifest.is_some());
        let machines =
            (primary && installed_release && self.home.root().join("machines.json").is_file())
                .then(|| self.released_machines.deadline());
        let bridge = self
            .last_sent
            .as_ref()
            .and_then(|sent| sent.bridge.as_ref())
            .filter(|bridge| bridge.facts.kuvo.is_none())
            .map(|_| self.next_bridge);
        [check, machines, bridge]
            .into_iter()
            .flatten()
            .map(|at| at.max(now + Duration::from_secs(2)))
            .min()
    }

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
            let message = if view.kept_peer_is_away(&self.home.run(), &main, now) {
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
            || update_status::legacy_updating(&self.home)
        {
            return Ok(CheckReply::Busy);
        }
        self.check_now = true;
        Ok(CheckReply::Checking)
    }

    fn blocked_check(&mut self, now: i64, message: &str) -> io::Result<()> {
        if self.checking.is_some()
            || self.pending.is_some()
            || self.home.run().join("previous").exists()
            || update_status::legacy_updating(&self.home)
            || (!self.check_now && Instant::now() < self.next_check)
        {
            return Ok(());
        }
        let manual = std::mem::take(&mut self.check_now);
        let round = release_round(now, self.check_offset);
        self.next_check = Instant::now() + CHECK_POLL;
        if !manual && update_status::checked_round(&self.home)?.as_deref() == Some(round.as_str()) {
            return Ok(());
        }
        update_status::note_check(
            &self.home,
            None,
            now,
            (!manual).then_some(round.as_str()),
            Some(Outcome::Blocked {
                message: message.into(),
            }),
        )
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
            if view.kept_peer_is_away(&self.home.run(), &main, now) {
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
        update_status::settle_shell_trial(&self.home)?;
        update_status::settle_legacy_trial(&self.home)?;
        self.engine.recover_release_drain().await?;
        if self.pending.as_ref().is_some_and(JoinHandle::is_finished)
            && let Some(pending) = self.pending.take()
        {
            let outcome = match pending.await {
                Ok(Ok(prepared)) => Outcome::Waiting {
                    version: prepared.version,
                    message: None,
                },
                Ok(Err(error)) => {
                    eprintln!("Preparing Kuvo's release: {}", error.record.reason);
                    if let Err(error) = error.save(&self.home) {
                        eprintln!("Recording the release failure: {error}");
                    }
                    error.outcome()
                }
                Err(error) => Outcome::Failed {
                    version: None,
                    message: error.to_string(),
                },
            };
            let recorded = update_status::note_last(&self.home, self.peers.reader().now(), outcome);
            self.step.send_replace(None);
            recorded?;
        }
        let (version, working) = fleet::installed(&self.home)?;
        let reader = self.peers.reader();
        let view = reader.view();
        let now = reader.now();
        let main = view.roster.main;
        let standalone = main.is_none() && view.roster.members.is_empty();
        let local_main = main.is_some() && main == view.roster.here;
        if let Err(error) = self.send_pending_check(&view, now).await {
            eprintln!("Asking the main to check Kuvo's release: {error}");
        }
        let Some(version) = version else {
            if standalone || local_main {
                self.blocked_check(now, "This computer’s box version is unknown.")?;
            }
            return Ok(());
        };
        let mut candidate =
            Candidate::kept(&self.home, &self.key).filter(|next| next.newer_than(Some(&version)));
        let offered = Candidate::offered(&self.home, &self.key)
            .filter(|next| next.newer_than(Some(&version)));
        let mut committed = false;
        if standalone || local_main {
            if let Some(offer) = &offered
                && !candidate
                    .as_ref()
                    .is_some_and(|chosen| chosen.newer_than(Some(offer.version())))
            {
                candidate = Some(offer.clone());
            }
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
                let result = checking
                    .task
                    .await
                    .map_err(io::Error::other)
                    .and_then(|result| result);
                let recorded = (|| -> io::Result<()> {
                    let mut latest = None;
                    let outcome = match result {
                        Ok(None) => Some(Outcome::Current {
                            version: version.clone(),
                        }),
                        Ok(Some(fetched)) => {
                            latest = Some(fetched.version().to_owned());
                            if !fetched.newer_than(Some(&version)) {
                                Some(Outcome::Current {
                                    version: version.clone(),
                                })
                            } else if !checking.manual
                                && !self.early_fixture
                                && !fetched.old_enough_for_automatic_check()
                            {
                                Some(Outcome::Waiting {
                                    version: fetched.version().into(),
                                    message: Some(
                                        "Automatic checks wait a few days after publication."
                                            .into(),
                                    ),
                                })
                            } else {
                                match fetched.into_shared() {
                                    Ok(fetched) => {
                                        // Retain the signed choice before saving the round.
                                        self.consider(
                                            &mut candidate,
                                            fetched.envelope().to_owned(),
                                            &version,
                                        )?;
                                        if fetched.is_prepared(&self.home)?
                                            || candidate.as_ref().is_some_and(|choice| {
                                                choice.newer_than(Some(fetched.version()))
                                            })
                                        {
                                            Some(Outcome::Waiting {
                                                version: fetched.version().into(),
                                                message: None,
                                            })
                                        } else {
                                            None
                                        }
                                    }
                                    Err(error) => Some(Outcome::Failed {
                                        version: None,
                                        message: fleet::release_error_message(&error),
                                    }),
                                }
                            }
                        }
                        Err(error) => {
                            eprintln!("Checking Kuvo's release: {error}");
                            Some(Outcome::Failed {
                                version: None,
                                message: fleet::release_error_message(&error)
                                    .chars()
                                    .take(300)
                                    .collect(),
                            })
                        }
                    };
                    update_status::note_check(
                        &self.home,
                        latest.as_deref(),
                        now,
                        checking.round.as_deref(),
                        outcome,
                    )
                })();
                self.step.send_replace(None);
                if let Err(error) = recorded {
                    eprintln!("Recording the release check: {error}");
                }
            }
            if fleet::stopped(&self.home) {
                self.blocked_check(now, "This computer’s box is stopped.")?;
            } else if self.checking.is_none()
                && (self.check_now || Instant::now() >= self.next_check)
            {
                let manual = self.check_now;
                let round = release_round(now, self.check_offset);
                if !manual
                    && update_status::checked_round(&self.home)?.as_deref() == Some(round.as_str())
                {
                    self.next_check = Instant::now() + CHECK_POLL;
                } else if self.pending.is_none()
                    && !update_status::legacy_updating(&self.home)
                    && !self.home.run().join("previous").try_exists()?
                    && (standalone || self.setup.may_switch_fleet().await)
                {
                    self.next_check = Instant::now() + CHECK_POLL;
                    self.check_now = false;
                    let (fetcher, url, key) =
                        (self.fetcher.clone(), self.url.clone(), self.key.clone());
                    self.step.send_replace(Some(Step::Checking));
                    self.checking = Some(FeedCheck {
                        started: Instant::now(),
                        manual,
                        round: (!manual).then_some(round),
                        task: tokio::spawn(async move { fetcher.check(&url, &key).await }),
                    });
                }
            }
        } else if let Some(main) = main {
            if let Some(checking) = self.checking.take() {
                checking.task.abort();
                self.step.send_replace(None);
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
        let held_for_released = running
            .as_ref()
            .filter(|_| (local_main || standalone) && candidate.is_none());
        self.released_machines
            .tick(
                &self.home,
                held_for_released,
                &self.fetcher,
                &self.setup,
                standalone,
            )
            .await;
        let installed_sha256 = running
            .as_ref()
            .and_then(|running| running.local().ok())
            .map(|pack| pack.sha256);
        let trial = fleet::trial_release(&self.home)?.and_then(|selected| {
            let envelope = fleet::installed_envelope(&self.home, &selected)?;
            let release = Candidate::verify(envelope, &self.key).ok()?;
            let local = release.local().ok()?;
            (local.target().ok()? == selected).then_some((
                release,
                PreparedRelease {
                    version: local.version,
                    sha256: local.sha256,
                },
            ))
        });
        let mut facts = UpdateFacts {
            kuvo: Some(version),
            release_target: Some(target.into()),
            installed_sha256,
            running_manifest,
            switching: trial.as_ref().map(|(_, local)| local.clone()),
            ..UpdateFacts::default()
        };
        if let Some(candidate) = &candidate {
            let local = candidate.local()?;
            candidate.keep(&self.home)?;
            let prepared = PreparedRelease {
                version: local.version.clone(),
                sha256: local.sha256.clone(),
            };
            let transfer = self
                .receiving
                .get_or_insert_with(|| (prepared.clone(), mail::PullProgress::default()));
            if transfer.0 != prepared {
                *transfer = (prepared.clone(), mail::PullProgress::default());
            }
            let progress = &transfer.1;
            facts.candidate = Some(candidate.version().into());
            facts.release_manifest = Some(candidate.envelope().into());
            facts.cannot = retry::Failure::for_release(&self.home, &prepared)?;
            let failed = facts.cannot.is_some();
            if !failed && candidate.is_prepared(&self.home)? {
                facts.staged = Some(prepared.clone());
            } else if !failed
                && self.pending.is_none()
                && self.checking.is_none()
                && !update_status::legacy_updating(&self.home)
                && !self.home.run().join("previous").exists()
                && self.attempted.as_ref() != Some(&prepared)
                && !fleet::stopped(&self.home)
            {
                self.attempted = Some(prepared.clone());
                let candidate = candidate.clone();
                let home = self.home.clone();
                let fetcher = self.fetcher.clone();
                let mail = self.mail.clone();
                let key = self.key.clone();
                let progress = progress.clone();
                let prepared = prepared.clone();
                let step = self.step.clone();
                step.send_replace(Some(Step::Downloading));
                self.pending = Some(tokio::spawn(async move {
                    let pack = match candidate.download_or_held(&home, &key, &fetcher).await {
                        Ok(pack) => pack,
                        Err(error) => {
                            let Some(mail) = mail else {
                                return Err(retry::Failure::download(prepared, error));
                            };
                            mail.pull(&local, &progress).await.map_err(|peer_error| {
                                eprintln!("Peer release fallback: {peer_error}");
                                retry::Failure::download(prepared.clone(), error)
                            })?
                        }
                    };
                    if !local.matches_bytes(&pack) {
                        return Err(retry::Failure::bad_archive(prepared));
                    }
                    if let Some(bridge) = candidate
                        .bridge(&key)
                        .map_err(|error| retry::Failure::prepare(prepared.clone(), error))?
                    {
                        bridge
                            .retain_bridge(&home, &fetcher)
                            .await
                            .map_err(|error| retry::Failure::download(prepared.clone(), error))?;
                    }
                    candidate
                        .prepare(home, pack, move |phase| {
                            step.send_replace(Some(phase));
                        })
                        .await
                        .map_err(|error| retry::Failure::prepare(prepared.clone(), error))?;
                    Ok(prepared)
                }));
            }
            if !failed && facts.staged.is_none() && facts.switching.is_none() {
                facts.receiving = progress.fraction().map(|fill| ReleaseProgress {
                    release: prepared,
                    fill,
                });
            }
        }
        if !standalone
            && !local_main
            && let Some(offered) = offered
            && (facts.staged.is_none()
                || !candidate
                    .as_ref()
                    .is_some_and(|chosen| chosen.newer_than(Some(offered.version()))))
        {
            facts.candidate = Some(offered.version().into());
            facts.release_manifest = Some(offered.envelope().into());
        }
        let prepared = facts.staged.is_some();
        let bridge = if local_main && let Some(running) = &running {
            self.bridge_offer(running, &view, now).await?
        } else {
            None
        };
        let signed_release = trial
            .as_ref()
            .map(|(release, _)| release)
            .or(candidate.as_ref())
            .or(running.as_ref());
        let (local_row, peer_rows, summary) = signed_release
            .map(|release| {
                let local =
                    progress::row(release, Some(&facts), UpdatePresence::default(), true, now);
                let members = view.roster.members_by_name();
                let peers: BTreeMap<_, _> = members
                    .iter()
                    .map(|peer| {
                        let (presence, facts) = view.peer_update(&peer.key, now);
                        let can_retry = self.mail.is_some() && peer.host().is_some();
                        let row = progress::row(release, facts.as_ref(), presence, can_retry, now);
                        (peer.key, row)
                    })
                    .collect();
                let here = view.roster.local();
                let named = std::iter::once((here.name.as_str(), &local)).chain(
                    members
                        .iter()
                        .filter_map(|peer| Some((peer.name.as_str(), peers.get(&peer.key)?))),
                );
                let digest = release.pack_digest(target).map(str::to_owned);
                let summary = self.summary.summarize(release, digest, named, now);
                (Some(local), peers, Some(summary))
            })
            .unwrap_or_default();
        let publication = UpdatePublication {
            facts,
            bridge,
            local_row,
            peer_rows,
            summary,
        };
        if self.last_sent.as_deref() != Some(&publication) {
            let publication = Arc::new(publication);
            self.last_sent = Some(Arc::clone(&publication));
            self.peers.announce_update(publication).await;
        }

        if let Some(candidate) = candidate {
            // Preparation publishes fleet-ready just before its task returns. A native
            // file notification can get here before the task's completion has been
            // recorded, particularly on a newly elected main. Settle that result
            // before activation so the same round can start its due feed check.
            if prepared && self.pending.is_some() {
                return Ok(());
            }
            // A promoted main can finish preparation and start its feed check in
            // the same event round. Give that actual request one old-round grace
            // to get on the wire before replacing this resident; don't wait for
            // its response (the feed may be unavailable).
            if (standalone || local_main)
                && prepared
                && self
                    .checking
                    .as_ref()
                    .is_some_and(|check| check.started.elapsed() < Duration::from_secs(2))
            {
                return Ok(());
            }
            let authorized = if standalone || local_main {
                let (ready, reachable) = barrier(&view, now, &candidate);
                candidate.primary_may_switch(&self.home, now, ready, reachable)?
                    && prepared
                    && (standalone || self.setup.may_switch_fleet().await)
            } else {
                prepared
                    && committed
                    && self
                        .setup
                        .rosters()
                        .borrow()
                        .as_ref()
                        .is_some_and(|roster| roster.main == main)
            };
            if authorized
                && !update_status::legacy_updating(&self.home)
                && !fleet::stopped(&self.home)
                && !self.home.run().join("previous").exists()
            {
                let local = candidate.local()?;
                let release = format!("{}:{}", local.version, local.sha256);
                if self
                    .engine
                    .drain_for_release(&release, candidate.engine())
                    .await?
                {
                    let fresh = reader.view();
                    let now = reader.now();
                    let still_authorized = if fresh.roster.main != main {
                        false
                    } else if standalone || local_main {
                        let (ready, reachable) = barrier(&fresh, now, &candidate);
                        candidate.primary_may_switch(&self.home, now, ready, reachable)?
                            && if standalone {
                                fresh.roster.members.is_empty()
                            } else {
                                self.setup.may_switch_fleet().await
                            }
                    } else if let Some(main) = main {
                        let (away, facts) = fresh.peer_updates(&main, now);
                        !away
                            && facts.kuvo.as_deref() == Some(candidate.version())
                            && peer_installed(
                                &candidate,
                                &facts.release_target,
                                &facts.installed_sha256,
                            )
                            && self
                                .setup
                                .rosters()
                                .borrow()
                                .as_ref()
                                .is_some_and(|roster| roster.main == Some(main))
                    } else {
                        false
                    };
                    if still_authorized && !fleet::stopped(&self.home) {
                        self.step.send_replace(Some(Step::Installing));
                        let result = candidate.activate(&self.home).await;
                        self.step.send_replace(None);
                        if let Err(error) = result {
                            self.engine.cancel_release_drain().await?;
                            return Err(error);
                        }
                        return Ok(());
                    }
                    self.engine.cancel_release_drain().await?;
                }
                return Ok(());
            }
        }
        self.engine.cancel_release_drain().await?;
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
