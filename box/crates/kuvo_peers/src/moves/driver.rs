//! The resident transfer owner. An accepted command outlives its caller.
//! The durable journal separates a complete copy from an ownership decision.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use http::StatusCode;
use kuvo_engine::{Engine, SessionMove};
use kuvo_server::Refusal;
use kuvo_store::{
    files,
    locks::{OnBusy, StaleLock},
};
use kuvo_wire::{
    keys::{decode_exact, encode},
    mail::Envelope,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::{
    sync::{broadcast, mpsc, oneshot, watch},
    task::JoinHandle,
    time::{self, Instant},
};

use super::{
    Journal, Kind, Role, Stage, archive, forward_evidence, handoff, journals, left, listing,
    manifest::{Manifest, Origin},
    move_id, read_json,
    receipt::{self, Bundle, Decided, Decision, Payloads, Receipts},
    session_files, session_notes,
    transfer::{
        BundleSize, Flight, IncomingBundle, MAX_BUNDLE_BYTES, OutgoingBundle, PART_BYTES, Receiving,
    },
    wake_folder,
};
use crate::{
    directory::ComputerKey,
    ownership,
    reach::Millis,
    relay::{RelayEvent, RelayHandle},
    setup_lists::{Reader, View},
};

pub(super) const MOVING: &str = "It’s moving to another computer. Try again once it’s there.";
const CHECK_WAIT: Millis = 45_000;
const STARTING_ON_RESTART: Millis = 30_000;
const STEP_AGAIN: Millis = 60_000;
const NO_OFFER_ANSWER: Duration = Duration::from_secs(30);
const QUIET_BEFORE_SWITCH: Duration = Duration::from_secs(10 * 60);
const SET_UP_AT_MOST: Duration = Duration::from_secs(30 * 60);
const WAITING_AFTER: Duration = Duration::from_secs(90);
const WAITING_FOR_SWITCH: Duration = Duration::from_secs(150);
const HELD_AT_MOST: Millis = 7 * 24 * 60 * 60_000;
const DROP_AGAIN_LATE: Millis = 60 * 60_000;
const DROP_AT_MOST: Millis = 24 * 3_600_000;
const ORPHAN_AFTER: Duration = Duration::from_secs(20 * 60);

#[derive(Deserialize)]
pub(super) struct Request {
    kind: Kind,
    id: String,
    to: String,
    #[serde(default)]
    pub(super) receipt: bool,
}

#[derive(Clone)]
pub(crate) struct Handle(mpsc::Sender<Begin>);

struct Begin {
    request: Request,
    result: oneshot::Sender<Result<Journal, Refusal>>,
}

impl Handle {
    pub(super) async fn begin(&self, request: Request) -> Result<Journal, Refusal> {
        let (result, receive) = oneshot::channel();
        self.0
            .send(Begin { request, result })
            .await
            .map_err(|_| unavailable())?;
        receive.await.map_err(|_| unavailable())?
    }
}

pub(crate) fn start(
    reader: Reader,
    relay: RelayHandle,
    engine: Engine,
    ownership: ownership::Handle,
    waiting: watch::Sender<Arc<BTreeSet<String>>>,
) -> io::Result<(Handle, JoinHandle<()>)> {
    let events = relay
        .subscribe()
        .ok_or_else(|| io::Error::other("The resident relay stopped"))?;
    files::create_private_dirs(&reader.run.join("moves"))?;
    let (tx, commands) = mpsc::channel(16);
    let driver = Driver {
        reader,
        relay,
        engine,
        ownership,
        copies: Receiving::default(),
        sending: HashMap::new(),
        retry: HashMap::new(),
        logged_stuck: HashSet::new(),
        orphans: HashMap::new(),
        owned: HashSet::new(),
        heard: HashMap::new(),
        looking_since: Instant::now(),
        waiting,
        asked_trash: None,
    };
    Ok((Handle(tx), tokio::spawn(driver.run(events, commands))))
}

struct Sending {
    manifest: Manifest,
    bundle: OutgoingBundle,
    flight: Flight,
    since: Instant,
}

struct RetryClock {
    wall: Millis,
    since: Instant,
}

struct Driver {
    reader: Reader,
    relay: RelayHandle,
    engine: Engine,
    ownership: ownership::Handle,
    copies: Receiving,
    sending: HashMap<String, Sending>,
    retry: HashMap<String, RetryClock>,
    logged_stuck: HashSet<String>,
    orphans: HashMap<String, Instant>,
    owned: HashSet<String>,
    heard: HashMap<String, Instant>,
    looking_since: Instant,
    waiting: watch::Sender<Arc<BTreeSet<String>>>,
    asked_trash: Option<Instant>,
}

impl Driver {
    async fn run(
        mut self,
        mut events: broadcast::Receiver<std::sync::Arc<RelayEvent>>,
        mut commands: mpsc::Receiver<Begin>,
    ) {
        if let Err(error) = wake_folder::release_orphans(self.run_dir()) {
            eprintln!("Keeping orphaned wake-up holds: {error}");
        }
        let mut tick = time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
        loop {
            let result = tokio::select! {
                command = commands.recv() => {
                    let Some(Begin { request, result }) = command else { return };
                    let _ = result.send(self.begin(request).await);
                    Ok(())
                }
                event = events.recv() => match event {
                    Ok(event) => {
                        let view = self.reader.view();
                        if let RelayEvent::Mail { key, envelope, .. } = event.as_ref()
                            && view.roster.member(key).is_some()
                            && let Some(movement) = envelope.0.get("move")
                        {
                            self.receive(&view, *key, movement).await
                        } else if matches!(event.as_ref(), RelayEvent::Connected(_)) {
                            self.round().await
                        } else { Ok(()) }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => Ok(()),
                    Err(broadcast::error::RecvError::Closed) => return,
                },
                _ = tick.tick() => self.round().await,
            };
            if let Err(error) = result {
                eprintln!("Keeping the move: {error}");
            }
        }
    }

    fn run_dir(&self) -> &Path {
        self.reader.run.as_ref()
    }

    fn staged(&self, id: &str) -> PathBuf {
        self.run_dir().join("moves").join(id)
    }

    async fn begin(&mut self, request: Request) -> Result<Journal, Refusal> {
        let view = self.reader.view();
        let destination = decode_exact(&request.to).and_then(|key| view.roster.member(&key));
        let Some(here) = view
            .roster
            .here
            .filter(|here| destination.is_some_and(|peer| peer.key != *here))
        else {
            return Err(Refusal::new(
                StatusCode::NOT_FOUND,
                "That computer isn’t one of yours.",
            ));
        };
        if request.kind != Kind::Chat || !session_id(&request.id) {
            return Err(gone());
        }
        if !view.relay_connected {
            return Err(unavailable());
        }
        if journals(self.run_dir())
            .map_err(|error| Refusal::new(StatusCode::SERVICE_UNAVAILABLE, error.to_string()))?
            .iter()
            .any(|row| row.live() && row.covers(&request.id))
        {
            return Err(Refusal::new(StatusCode::CONFLICT, MOVING));
        }
        let session = chat(&self.engine, self.run_dir(), &request.id).await?;
        let scope = self
            .ownership
            .scope(&request.id, None)
            .map_err(|error| Refusal::new(StatusCode::SERVICE_UNAVAILABLE, error.to_string()))?;
        if scope.held || scope.owner.is_some_and(|owner| !owner.local) {
            return Err(Refusal::new(
                StatusCode::CONFLICT,
                "This record is held while its owner changes.",
            ));
        }
        if let Some(peer) = destination
            && !listing::supports_moves(&view, &request.to, listing::checkpointed(self.run_dir()))
        {
            let name = listing::plain_name(&peer.name);
            let place = if name.is_empty() {
                "That computer"
            } else {
                &name
            };
            return Err(Refusal::new(
                StatusCode::CONFLICT,
                listing::needs_update(place),
            ));
        }
        let mut random = [0; 12];
        getrandom::fill(&mut random).map_err(|_| unavailable())?;
        // The durable name is also a cross-resident identifier.
        let id = format!("mov_{}", encode(&random).replace(['-', '_'], ""));
        let name = chat_name(&session);
        let now = self.reader.now();
        let peer = &request.to;
        let check = view
            .announced
            .get(peer)
            .and_then(|entry| entry.features.as_ref())
            .is_some_and(|features| features.iter().any(|feature| feature == "moves.check"));
        let mut row = new_journal(
            id,
            Kind::Chat,
            request.id,
            name,
            encode(&here),
            request.to,
            Role::From,
            now,
        );
        row.sessions.push(row.thing.clone());
        row.step = check.then(|| "check".to_owned());
        row.extra.insert("twoComputers".into(), true.into());
        if let Some(model) = model(&session) {
            row.extra.insert(
                "model".into(),
                serde_json::to_value(model).expect("a model serializes"),
            );
        }
        self.reserve(&row)
            .map_err(|_| Refusal::new(StatusCode::CONFLICT, MOVING))?;
        self.owned.insert(row.id.clone());
        Ok(row)
    }

    fn reserve(&self, row: &Journal) -> io::Result<()> {
        let lock = self.run_dir().join("moves/.admission.lock");
        let _guard = StaleLock::acquire(
            &lock,
            Duration::from_secs(10),
            Duration::from_secs(3),
            OnBusy::Refuse,
        )?;
        let rows = journals(self.run_dir()).map_err(io::Error::other)?;
        if rows.iter().any(|other| {
            other.id == row.id
                || other.live()
                    && (other.covers(&row.thing) || row.sessions.iter().any(|id| other.covers(id)))
        }) {
            return Err(io::Error::other(MOVING));
        }
        row.save(self.run_dir())
    }

    async fn round(&mut self) -> io::Result<()> {
        let view = self.reader.view();
        let now = self.reader.now();
        let rows = journals(self.run_dir()).map_err(io::Error::other)?;
        if let Err(error) =
            remove_orphaned_bundles(&self.reader.run, &mut self.orphans, Instant::now())
        {
            eprintln!("Removing an orphaned move bundle: {error}");
        }
        self.refresh_waiting(&rows);
        self.check_trash(&view, &rows).await?;
        for mut row in rows {
            let result = if row.role == Role::From {
                self.source_step(&view, &mut row).await
            } else if row.dropping && row.activation.is_none() {
                self.retry_drop(&view, &mut row, now).await;
                Ok(())
            } else if row.activation.is_some()
                || row.set_up_since.is_some()
                || Receipts::open(self.run_dir())
                    .for_peer(&row.id, &row.peer)
                    .map_err(io::Error::other)?
                    .is_some_and(|decision| {
                        decision.decided == Decided::Dropped && decision.bundle == row.bundle
                    })
            {
                self.resume_destination(&view, &mut row).await
            } else if now.saturating_sub(row.started) > HELD_AT_MOST
                && let Some(peer) = decode_exact(&row.peer)
            {
                self.drop_copy(&view, peer, &mut row, false).await
            } else {
                Ok(())
            };
            if let Err(error) = result {
                eprintln!("Recovering move {}: {error}", row.id);
            }
        }
        Ok(())
    }

    async fn retry_drop(&mut self, view: &View, row: &mut Journal, now: Millis) {
        let drop_since = row
            .extra
            .get("dropSince")
            .and_then(Value::as_i64)
            .unwrap_or(row.started);
        let late = now.saturating_sub(drop_since) > HELD_AT_MOST;
        let every = if late { DROP_AGAIN_LATE } else { STEP_AGAIN };
        if self.retry.contains_key(&row.id) && !self.retry_due(row, every) {
            return;
        }
        if let Err(error) = self.resume_destination(view, row).await
            && late
            && self.logged_stuck.insert(row.id.clone())
        {
            eprintln!(
                "kuvo-link: a copy of {} that came here couldn’t be let go of in a week, so it stays held: {error}",
                row.display_name()
            );
        }
    }

    fn refresh_waiting(&mut self, rows: &[Journal]) {
        let ticks = Instant::now();
        let wall = self.reader.now();
        self.heard
            .retain(|_, time| ticks.duration_since(*time) <= QUIET_BEFORE_SWITCH);
        let waiting: BTreeSet<_> = rows
            .iter()
            .filter(|row| {
                let last = self
                    .heard
                    .get(&row.id)
                    .copied()
                    .unwrap_or(self.looking_since);
                let sent = self
                    .sending
                    .get(&row.id)
                    .map(|sending| ticks.duration_since(sending.since));
                waiting_on_peer(row, wall, sent, ticks.duration_since(last))
            })
            .map(|row| row.id.clone())
            .collect();
        self.waiting.send_if_modified(|before| {
            if **before == waiting {
                return false;
            }
            *before = Arc::new(waiting);
            true
        });
    }

    async fn source_step(&mut self, view: &View, row: &mut Journal) -> io::Result<()> {
        let now = self.reader.now();
        let Some(key) = decode_exact(&row.peer) else {
            return Ok(());
        };
        if !row.live() {
            return self.settle_failure(view, key, row).await;
        }
        if row.stage == Stage::Finishing {
            if self.retry.contains_key(&row.id) && !self.retry_due(row, STEP_AGAIN) {
                return Ok(());
            }
            return self.finish(row).await;
        }
        if row.stage == Stage::Switching {
            let op = if row.bundle.generation.is_some() {
                "switch"
            } else {
                "decision?"
            };
            if self.retry_due(row, STEP_AGAIN) {
                row.step = Some("switch".into());
                row.extra.insert("sentAt".into(), now.into());
                row.extra.shift_remove("againAt");
                row.save(self.run_dir())?;
                self.tell(view, key, row, op, json!({"safeSwitch":true}))
                    .await;
            }
            return Ok(());
        }
        if !self.owned.contains(&row.id) {
            let just_requested = row.stage == Stage::Holding
                && matches!(row.step.as_deref(), None | Some("check"))
                && now.saturating_sub(row.started) < STARTING_ON_RESTART;
            if !just_requested {
                return self
                    .fail_source(
                        view,
                        key,
                        row,
                        format!("{} didn’t get there. Try again.", row.display_name()),
                        409,
                    )
                    .await;
            }
            if !view.relay_connected {
                return Ok(());
            }
            self.owned.insert(row.id.clone());
            row.extra.shift_remove("sentAt");
            row.extra.shift_remove("againAt");
        }
        if row.step.as_deref() == Some("check") {
            if now.saturating_sub(row.started) >= CHECK_WAIT {
                let place = listing::place_name(listing::name_of(&view.roster, &row.peer));
                return self
                    .fail_source(
                        view,
                        key,
                        row,
                        format!("{place} isn’t connected. Connect to it, then try again."),
                        503,
                    )
                    .await;
            }
            if self.retry_due(row, STEP_AGAIN) {
                let mut check = json!({"kind":row.kind,"name":row.name});
                if let Some(model) = row.extra.get("model") {
                    check["model"] = model.clone();
                }
                self.tell(view, key, row, "check", check).await;
            }
        } else if row.stage == Stage::Holding {
            if let Err(error) = self.pack(view, row).await {
                return self
                    .fail_source(view, key, row, error.to_string(), 409)
                    .await;
            }
            self.offer(view, key, row).await;
        } else {
            let step = row.step.as_deref();
            let tries = row
                .extra
                .get("tries")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            if let Some(sending) = self.sending.get(&row.id) {
                let place = listing::place_name(listing::name_of(&view.roster, &row.peer));
                let no_answer = step == Some("offer")
                    && tries == 0
                    && sending.since.elapsed() > NO_OFFER_ANSWER
                    && sending.flight.acknowledged() == 0;
                let quiet = (matches!(step, Some("parts" | "arrive"))
                    || step == Some("offer") && tries > 0)
                    && sending.since.elapsed() > QUIET_BEFORE_SWITCH
                    && self
                        .heard
                        .get(&row.id)
                        .is_none_or(|last| last.elapsed() > QUIET_BEFORE_SWITCH);
                if no_answer || quiet {
                    let message = if no_answer {
                        format!("{place} isn’t connected. Connect to it, then try again.")
                    } else {
                        format!(
                            "{place} stopped answering, so {} stayed here. Try again once it’s back.",
                            row.display_name()
                        )
                    };
                    return self.fail_source(view, key, row, message, 503).await;
                }
            }
            match row.step.as_deref() {
                Some("offer") if self.retry_due(row, STEP_AGAIN) => {
                    row.extra.insert("againAt".into(), now.into());
                    row.save(self.run_dir())?;
                    self.offer(view, key, row).await
                }
                Some("parts") => self.send_parts(view, key, row).await?,
                Some("arrive") if self.retry_due(row, STEP_AGAIN) => {
                    row.extra.insert("againAt".into(), now.into());
                    row.save(self.run_dir())?;
                    self.tell(view, key, row, "arrive", json!({})).await;
                }
                _ => {}
            }
        }
        Ok(())
    }

    async fn receive(&mut self, view: &View, key: ComputerKey, given: &Value) -> io::Result<()> {
        let Some(id) = given["id"]
            .as_str()
            .filter(|id| move_id(id).is_some_and(|tail| tail.len() <= 40))
        else {
            return Ok(());
        };
        let Some(op) = given["op"].as_str() else {
            return Ok(());
        };
        let peer = encode(&key);
        match op {
            "ran" => {
                if let Some(at) = given["at"].as_i64() {
                    left::note_ran(self.run_dir(), id, &peer, at)?;
                }
                return Ok(());
            }
            "ran?" if given["lane"] != true => {
                if let Some(session) = given["session"].as_str().filter(|id| session_id(id)) {
                    self.report_run(view, key, id, session).await?;
                }
                return Ok(());
            }
            _ => {}
        }
        let stored = Journal::read(self.run_dir(), id)?;
        if op != "offer" && stored.as_ref().is_some_and(|row| row.peer != peer) {
            return Ok(());
        }
        if stored
            .as_ref()
            .is_some_and(|row| row.peer == peer && (op == "checked" || row.bundle.matches(given)))
        {
            self.heard.insert(id.into(), Instant::now());
            self.waiting.send_if_modified(|waiting| {
                waiting.contains(id) && Arc::make_mut(waiting).remove(id)
            });
        }
        if op == "check" {
            let answer = match self.check_destination(view, given).await {
                Ok(()) => json!({"ok":true}),
                Err(message) => json!({"ok":false,"message":message}),
            };
            self.wire(view, key, id, &Bundle::default(), "checked", answer)
                .await;
            return Ok(());
        }
        if op == "offer" {
            return self.accept_offer(view, key, given, id, stored).await;
        }
        let Some(mut row) = stored else {
            if op == "arrive" {
                let bundle = Bundle {
                    generation: given["generation"].as_u64(),
                    hash: given["hash"].as_str().map(str::to_owned),
                };
                self.wire(
                    view,
                    key,
                    id,
                    &bundle,
                    "failed",
                    json!({"message":self.could_not_set_up(view)}),
                )
                .await;
                return Ok(());
            }
            return self.replay(view, key, &peer, given).await;
        };
        if op != "checked" && !row.bundle.matches(given) {
            return Ok(());
        }
        if row.role == Role::To {
            return self.destination_mail(view, key, &mut row, op, given).await;
        }
        if !row.live() {
            if op == "dropped" && row.step.as_deref() == Some("drop") {
                self.retire_memory(&row.id);
                handoff::remove_completed(self.run_dir(), &row)?;
            }
            return Ok(());
        }
        match op {
            "checked" if row.stage == Stage::Holding && row.step.as_deref() == Some("check") => {
                if given["ok"] != true {
                    let message = given["message"]
                        .as_str()
                        .unwrap_or("It couldn’t go there.")
                        .to_owned();
                    return self.fail_source(view, key, &mut row, message, 409).await;
                }
                row.note = given["note"].as_str().map(str::to_owned);
                row.step = None;
                row.save(self.run_dir())?;
                self.source_step(view, &mut row).await?;
            }
            "ready" if row.step.as_deref() == Some("offer") && self.sending.contains_key(id) => {
                if given["safeSwitch"] != true
                    || (row.verify_payloads && given["verifyPayloads"] != true)
                    || (row.extra.get("typedAttachmentPaths") == Some(&Value::Bool(true))
                        && given["typedAttachmentPaths"] != true)
                {
                    return self
                        .fail_source(
                            view,
                            key,
                            &mut row,
                            "Update Kuvo on the destination computer before trying the move again."
                                .into(),
                            409,
                        )
                        .await;
                }
                row.extra.insert("safeSwitch".into(), true.into());
                self.step(&mut row, "parts")?;
                if let Some(sending) = self.sending.get_mut(id) {
                    sending.since = Instant::now();
                }
                self.send_parts(view, key, &row).await?;
            }
            "got" if row.step.as_deref() == Some("parts") => {
                let Some(index) = index(given) else {
                    return Ok(());
                };
                let Some(sending) = self.sending.get_mut(id) else {
                    return Ok(());
                };
                if !sending.flight.acknowledge(index) {
                    return Ok(());
                }
                let size = row.size.unwrap_or_default();
                if sending.flight.complete() {
                    sending.since = Instant::now();
                    row.sent = Some(size);
                    self.step(&mut row, "arrive")?;
                    self.mark_retry(&row);
                } else {
                    if sending.flight.acknowledged().is_multiple_of(32) {
                        row.sent =
                            Some(size.min(sending.flight.acknowledged() as u64 * PART_BYTES));
                        row.save(self.run_dir())?;
                    }
                    self.send_parts(view, key, &row).await?;
                }
            }
            "arrived"
                if row.stage == Stage::Copying
                    && row.extra.get("safeSwitch") == Some(&Value::Bool(true)) =>
            {
                if (row.verify_payloads && given["verifiedPayloads"] != true)
                    || (row.extra.get("typedAttachmentPaths") == Some(&Value::Bool(true))
                        && given["typedAttachmentPaths"] != true)
                {
                    return self
                        .fail_source(
                            view,
                            key,
                            &mut row,
                            "The destination could not verify the move’s files.".into(),
                            409,
                        )
                        .await;
                }
                if let Err(error) = self.switch(view, key, &mut row).await {
                    if row.stage == Stage::Switching {
                        return Err(error);
                    }
                    return self
                        .fail_source(view, key, &mut row, error.to_string(), 409)
                        .await;
                }
            }
            "refused" | "failed" if row.stage == Stage::Copying => {
                let message = given["message"]
                    .as_str()
                    .unwrap_or("It couldn’t go there.")
                    .to_owned();
                self.fail_source(view, key, &mut row, message, 409).await?;
            }
            "switched"
                if row.stage.switched()
                    && (!row.verify_payloads || given["verifiedPayloads"] == true) =>
            {
                if given["verifiedPayloads"] == true {
                    row.extra.insert("verifiedPayloads".into(), true.into());
                }
                self.finish(&mut row).await?;
            }
            "dropped" if row.stage == Stage::Switching && given["neverSwitched"] == true => {
                let reason = format!(
                    "{} couldn’t be set up there, so it stayed here. Try again.",
                    row.display_name()
                );
                self.fail_source(view, key, &mut row, reason, 409).await?;
            }
            _ => {}
        }
        Ok(())
    }

    async fn pack(&mut self, view: &View, row: &mut Journal) -> io::Result<()> {
        wake_folder::hold(self.run_dir(), row, self.reader.now())?;
        let session = self.engine.session_move().await.map_err(io::Error::other)?;
        session
            .settle(&row.sessions)
            .await
            .map_err(io::Error::other)?;
        let exports = session
            .export_tree(&row.thing)
            .await
            .map_err(io::Error::other)?;
        let ids = tree(&exports, &row.thing)?;
        row.sessions = ids;
        row.save(self.run_dir())?;
        session
            .settle(&row.sessions)
            .await
            .map_err(io::Error::other)?;
        let (exports, queues) = snapshot(&session, row).await?;
        self.check_payloads(row)?;
        let notes = session_notes::pack(self.run_dir(), &row.sessions)?;
        if notes.is_some()
            && !view
                .announced
                .get(&row.peer)
                .and_then(|entry| entry.features.as_ref())
                .is_some_and(|features| {
                    features
                        .iter()
                        .any(|feature| feature == "data.session-notes")
                })
        {
            return Err(io::Error::other(
                "Update the destination computer before moving these ordinary session notes.",
            ));
        }
        let setup = self
            .ownership
            .scope(&row.thing, None)
            .map_err(io::Error::other)?
            .setup;
        let evidence = forward_evidence::pack(self.run_dir(), setup.as_deref(), &row.sessions)?;
        if evidence.is_some()
            && !view
                .announced
                .get(&row.peer)
                .and_then(|entry| entry.features.as_ref())
                .is_some_and(|features| {
                    features
                        .iter()
                        .any(|feature| feature == "data.forward-evidence")
                })
        {
            return Err(io::Error::other(
                "Update the destination computer before moving these forwarded decisions.",
            ));
        }
        let home = self.run_dir().parent().expect("resident has a home");
        let location = chats_dir(home);
        let wake = wake_folder::original(self.run_dir(), row)?;
        let mut manifest = Manifest {
            kind: Kind::Chat,
            thing: row.thing.clone(),
            name: row.display_name().into(),
            exports,
            queues: (!queues.is_empty()).then_some(queues),
            session_files: None,
            was_paused: Some(row.extra.get("wasPaused") == Some(&Value::Bool(true))),
            was: Some(Origin {
                place: view.roster.name.clone(),
                folder: location.to_str().map(str::to_owned),
            }),
            folder: wake.as_ref().map(|_| row.thing.clone()),
            lane: None,
            group: None,
            decided: None,
            work_queue: None,
            forward_evidence: evidence,
            session_notes: notes,
            extra: Default::default(),
        };
        let directory = self.staged(&row.id);
        let home = home.to_owned();
        let id = row.id.clone();
        let sessions = row.sessions.clone();
        let (manifest, path, size, hash) = tokio::task::spawn_blocking(move || {
            let stage = directory.join("sending-files");
            let path = directory.join("bundle.tgz");
            files::remove_tree_synced(&stage)?;
            let mut queues = manifest.queues.take().unwrap_or_default();
            let packed = (|| {
                let payloads = session_files::stage(
                    &home,
                    &id,
                    &sessions,
                    &mut manifest.exports,
                    &mut queues,
                    &stage,
                    super::transfer::MAX_BUNDLE_BYTES,
                )?;
                manifest.queues = (!queues.is_empty()).then_some(queues);
                manifest.session_files = Some(payloads);
                archive::write_chat(&path, &manifest, Some(&stage), wake.as_deref()).map_err(
                    |error| match error {
                        archive::Error::TooLarge => io::ErrorKind::FileTooLarge.into(),
                        other => io::Error::other(other),
                    },
                )
            })();
            let cleaned = files::remove_tree_synced(&stage);
            packed?;
            cleaned?;
            let (size, hash) = BundleSize::measure(&path)?;
            Ok::<_, io::Error>((manifest, path, size, hash))
        })
        .await?
        .map_err(|error| {
            if error.kind() == io::ErrorKind::FileTooLarge {
                io::Error::new(error.kind(), too_large(row.display_name()))
            } else {
                error
            }
        })?;
        let identity = row.bundle.generation.map_or(1, |generation| generation + 1);
        let bytes = BundleSize::new(size, &hash)?;
        let now = Instant::now();
        let sender = Sending {
            manifest,
            bundle: OutgoingBundle::open(&path, bytes.clone())?,
            flight: Flight::new(&bytes),
            since: now,
        };
        row.bundle = Bundle {
            generation: Some(identity),
            hash: Some(hash),
        };
        row.size = Some(size);
        row.verify_payloads = sender
            .manifest
            .session_files
            .as_ref()
            .is_some_and(|files| !files.is_empty());
        if row.verify_payloads {
            row.extra.insert("typedAttachmentPaths".into(), true.into());
        } else {
            row.extra.shift_remove("typedAttachmentPaths");
        }
        row.sent = Some(0);
        row.stage = Stage::Copying;
        row.extra.insert("offered".into(), true.into());
        self.step(row, "offer")?;
        self.sending.insert(row.id.clone(), sender);
        Ok(())
    }

    async fn offer(&mut self, view: &View, key: ComputerKey, row: &Journal) {
        self.mark_retry(row);
        let mut payload = json!({
            "safeSwitch":true, "twoComputers":true, "wasPaused":row.extra.get("wasPaused") == Some(&Value::Bool(true)), "kind":row.kind, "thing":row.thing,
            "name":row.name, "size":row.size, "from":row.from,
        });
        if row.verify_payloads {
            payload["verifyPayloads"] = true.into();
        }
        if let Some(paused) = row.extra.get("pausedAs") {
            payload["pausedAs"] = paused.clone();
        }
        self.tell(view, key, row, "offer", payload).await;
    }

    async fn send_parts(&mut self, view: &View, key: ComputerKey, row: &Journal) -> io::Result<()> {
        let now = Instant::now();
        let Some(sending) = self.sending.get_mut(&row.id) else {
            return Ok(());
        };
        let due: Vec<_> = sending.flight.due(now).collect();
        for index in due {
            let part = sending.bundle.part(index)?;
            if !send(
                &self.relay,
                view,
                key,
                packet(
                    &row.id,
                    &row.bundle,
                    "part",
                    serde_json::to_value(part)?,
                    self.reader.now(),
                ),
            )
            .await
            {
                break;
            }
            sending.flight.sent(index, now);
        }
        Ok(())
    }

    async fn switch(&mut self, view: &View, key: ComputerKey, row: &mut Journal) -> io::Result<()> {
        let Some(sending) = self.sending.get(&row.id) else {
            return Ok(());
        };
        let session = self.engine.session_move().await.map_err(io::Error::other)?;
        session
            .settle(&row.sessions)
            .await
            .map_err(io::Error::other)?;
        let (exports, queues) = snapshot(&session, row).await?;
        let before = sending.manifest.queues.clone().unwrap_or_default();
        let setup = self
            .ownership
            .scope(&row.thing, None)
            .map_err(io::Error::other)?
            .setup;
        self.check_payloads(row)?;
        if conversation(&exports) != conversation(&sending.manifest.exports)
            || without_clocks(&queues)? != without_clocks(&before)?
            || session_notes::pack(self.run_dir(), &row.sessions)? != sending.manifest.session_notes
            || forward_evidence::pack(self.run_dir(), setup.as_deref(), &row.sessions)?
                != sending.manifest.forward_evidence
        {
            if row.bundle.generation.unwrap_or_default() >= 3 {
                return Err(io::Error::other(format!(
                    "{} kept getting messages while it moved, so it stayed where it is. Try again in a moment.",
                    row.display_name()
                )));
            }
            row.extra.insert(
                "tries".into(),
                row.bundle.generation.unwrap_or_default().into(),
            );
            row.stage = Stage::Holding;
            row.step = None;
            row.save(self.run_dir())?;
            return Ok(());
        }
        let home = self.run_dir().parent().expect("resident has a home");
        session_files::source_unchanged(
            home,
            sending
                .manifest
                .session_files
                .as_deref()
                .unwrap_or_default(),
        )?;
        row.stage = Stage::Switching;
        row.extra
            .insert("switchSince".into(), self.reader.now().into());
        self.step(row, "switch")?;
        if let Some(sending) = self.sending.get_mut(&row.id) {
            sending.since = Instant::now();
        }
        self.mark_retry(row);
        self.tell(view, key, row, "switch", json!({"safeSwitch":true}))
            .await;
        Ok(())
    }

    async fn fail_source(
        &mut self,
        view: &View,
        key: ComputerKey,
        row: &mut Journal,
        message: String,
        status: u16,
    ) -> io::Result<()> {
        row.failed = Some(message);
        row.extra.insert("failedStatus".into(), status.into());
        row.extra
            .insert("dropSince".into(), self.reader.now().into());
        let step = if row.bundle.generation.is_some() && row.stage != Stage::Switching {
            "drop"
        } else {
            "done"
        };
        row.step = Some(step.into());
        row.extra.shift_remove("sentAt");
        row.extra.shift_remove("againAt");
        self.retry.remove(&row.id);
        row.save(self.run_dir())?;
        self.sending.remove(&row.id);
        self.settle_failure(view, key, row).await
    }

    async fn settle_failure(
        &mut self,
        view: &View,
        key: ComputerKey,
        row: &mut Journal,
    ) -> io::Result<()> {
        wake_folder::release(self.run_dir(), row)?;
        let status = row
            .extra
            .get("failedStatus")
            .and_then(Value::as_u64)
            .unwrap_or(409) as u16;
        listing::save_outcome(
            self.run_dir(),
            row,
            None,
            Some((status, row.failed.as_deref().unwrap_or_default())),
        )?;
        let since = row
            .extra
            .get("dropSince")
            .and_then(Value::as_i64)
            .unwrap_or(row.started);
        if row.step.as_deref() == Some("drop")
            && self.reader.now().saturating_sub(since) < DROP_AT_MOST
        {
            if self.retry_due(row, STEP_AGAIN) {
                row.extra.insert("sentAt".into(), self.reader.now().into());
                row.save(self.run_dir())?;
                self.tell(view, key, row, "drop", json!({})).await;
                files::remove_tree_synced(&self.staged(&row.id))?;
            }
            return Ok(());
        }
        self.retire_memory(&row.id);
        handoff::remove_completed(self.run_dir(), row)
    }

    async fn finish(&mut self, row: &mut Journal) -> io::Result<()> {
        handoff::retire_source(self.run_dir(), row, self.reader.now())?;
        self.retire_memory(&row.id);
        let result = self.clean_source(row).await;
        row.cleanup_pending = Some(result.is_err());
        row.save(self.run_dir())?;
        let note = if result.is_err() {
            Some(listing::CLEANUP)
        } else {
            row.note.as_deref()
        };
        listing::save_outcome(self.run_dir(), row, note, None)?;
        if let Err(error) = result {
            self.retry.insert(
                row.id.clone(),
                RetryClock {
                    wall: self.reader.now(),
                    since: Instant::now(),
                },
            );
            return Err(error);
        }
        handoff::remove_completed(self.run_dir(), row)
    }

    async fn clean_source(&self, row: &mut Journal) -> io::Result<()> {
        let session = self.engine.session_move().await.map_err(io::Error::other)?;
        let home = self.run_dir().parent().expect("resident has a home");
        let backup = wake_folder::original(self.run_dir(), row)?
            .unwrap_or_else(|| home.join(format!("moved-{}", row.thing)));
        if row.extra.get("kept") != Some(&Value::Bool(true)) {
            let exports = session
                .export_tree(&row.thing)
                .await
                .map_err(io::Error::other)?;
            files::write_atomic_through(
                &backup.join("conversation.json"),
                &serde_json::to_vec_pretty(&exports)?,
                if backup.starts_with(home) {
                    home
                } else {
                    &backup
                },
            )?;
            row.extra.insert("kept".into(), true.into());
            row.save(self.run_dir())?;
        }
        for id in row.sessions.iter().rev() {
            session.remove_retired(id).await.map_err(io::Error::other)?;
        }
        session_notes::discard(self.run_dir(), &row.sessions, None)?;
        left::keep(self.run_dir(), row, &backup, self.reader.now())
    }

    async fn check_trash(&mut self, view: &View, active: &[Journal]) -> io::Result<()> {
        let rows = left::read(self.run_dir())?;
        let ticks = Instant::now();
        let ask = view.relay_connected
            && self
                .asked_trash
                .is_none_or(|at| ticks - at >= Duration::from_millis(left::ASK_EVERY as u64));
        if ask && !rows.is_empty() {
            self.asked_trash = Some(ticks);
        }
        let now = self.reader.now();
        for row in rows {
            if row
                .ran_at
                .is_some_and(|at| now.saturating_sub(at) >= left::KEEP_FOR)
                && !active
                    .iter()
                    .any(|active| active.live() && active.thing == row.thing)
            {
                if let Err(error) = left::expire(self.run_dir(), &row) {
                    eprintln!("Removing an expired move backup {}: {error}", row.id);
                }
            } else if ask
                && row.ran_at.is_none()
                && let (Some(peer), Some(session)) = (decode_exact(&row.to), row.sessions.first())
            {
                self.wire(
                    view,
                    peer,
                    &row.id,
                    &Bundle::default(),
                    "ran?",
                    json!({"session":session}),
                )
                .await;
            }
        }
        Ok(())
    }

    async fn report_run(
        &self,
        view: &View,
        key: ComputerKey,
        id: &str,
        session: &str,
    ) -> io::Result<()> {
        let Some(decision) = Receipts::open(self.run_dir())
            .for_peer(id, &encode(&key))
            .map_err(io::Error::other)?
            .filter(|decision| decision.decided == Decided::Switched)
        else {
            return Ok(());
        };
        let engine = self.engine.session_move().await.map_err(io::Error::other)?;
        let at = match engine
            .recent_messages(session, 20)
            .await
            .map_err(io::Error::other)?
        {
            None => Some(self.reader.now()),
            Some(messages) => {
                let index = decision
                    .last
                    .as_ref()
                    .and_then(|last| messages.iter().position(|message| message["id"] == *last));
                let after = index.map_or(messages.as_slice(), |index| &messages[..index]);
                let ordered = decision.last.is_some() && (index.is_some() || messages.len() >= 20);
                after
                    .iter()
                    .filter(|message| message["type"] == "assistant")
                    .filter_map(|message| message["time"]["created"].as_i64())
                    .filter(|at| ordered || *at > decision.at)
                    .map(|at| at.max(decision.at))
                    .min()
            }
        };
        if let Some(at) = at {
            self.wire(view, key, id, &Bundle::default(), "ran", json!({"at":at}))
                .await;
        }
        Ok(())
    }

    fn check_payloads(&self, row: &Journal) -> io::Result<()> {
        let run = self.run_dir();
        let decided = read_json(&run.join("work/decided.json"))?;
        for session in &row.sessions {
            if decided
                .as_ref()
                .and_then(|rows| rows[session].as_array())
                .is_some_and(|entries| !entries.is_empty())
            {
                return Err(io::Error::other(
                    "The move’s Autopilot decisions could not be prepared.",
                ));
            }
            let queue = read_json(
                &run.join("work-queue/queues")
                    .join(format!("{session}.json")),
            )?;
            if queue.is_some_and(|queue| {
                queue["items"]
                    .as_array()
                    .is_none_or(|items| !items.is_empty())
            }) {
                return Err(io::Error::other(
                    "The session’s Work Queue could not be prepared for the move.",
                ));
            }
        }
        Ok(())
    }

    fn step(&mut self, row: &mut Journal, step: &str) -> io::Result<()> {
        row.step = Some(step.to_owned());
        row.extra.insert("sentAt".into(), self.reader.now().into());
        row.extra.shift_remove("againAt");
        self.retry.remove(&row.id);
        row.save(self.run_dir())
    }

    fn retry_due(&mut self, row: &Journal, every: Millis) -> bool {
        let wall = self.reader.now();
        let ticks = Instant::now();
        let interval = Duration::from_millis(every as u64);
        let clock = self.retry.entry(row.id.clone()).or_insert_with(|| {
            let stamp = row
                .extra
                .get("againAt")
                .or_else(|| row.extra.get("sentAt"))
                .and_then(Value::as_i64)
                .unwrap_or_else(|| wall.saturating_sub(every));
            let elapsed = Duration::from_millis(wall.saturating_sub(stamp).clamp(0, every) as u64);
            RetryClock {
                wall: stamp,
                since: ticks.checked_sub(elapsed).unwrap_or(ticks),
            }
        });
        if clock.wall <= wall && ticks.duration_since(clock.since) < interval {
            return false;
        }
        *clock = RetryClock { wall, since: ticks };
        true
    }

    fn mark_retry(&mut self, row: &Journal) {
        self.retry.insert(
            row.id.clone(),
            RetryClock {
                wall: self.reader.now(),
                since: Instant::now(),
            },
        );
    }

    fn retire_memory(&mut self, id: &str) {
        self.logged_stuck.remove(id);
        self.retry.remove(id);
        self.owned.remove(id);
        self.sending.remove(id);
        self.heard.remove(id);
    }

    async fn accept_offer(
        &mut self,
        view: &View,
        key: ComputerKey,
        given: &Value,
        id: &str,
        before: Option<Journal>,
    ) -> io::Result<()> {
        let bundle = Bundle {
            generation: given["generation"]
                .as_u64()
                .filter(|generation| (1..=9_007_199_254_740_991).contains(generation)),
            hash: given["hash"].as_str().map(str::to_owned),
        };
        let clock = self.reader.clone();
        let refused = |message: &str| {
            packet(
                id,
                &bundle,
                "refused",
                json!({"message":message}),
                clock.now(),
            )
        };
        let thing = given["thing"].as_str().unwrap_or_default();
        let size = given["size"].as_u64().unwrap_or_default();
        let hash_valid = bundle.hash.as_ref().is_some_and(|hash| {
            decode_exact::<32>(hash).is_some_and(|digest| encode(&digest) == *hash)
        });
        if given["kind"] != "chat"
            || given["lane"] == true
            || !session_id(thing)
            || !hash_valid
            || size == 0
        {
            send(&self.relay, view, key, refused("That isn’t a move.")).await;
            return Ok(());
        }
        if given["safeSwitch"] != true || bundle.generation.is_none() {
            send(
                &self.relay,
                view,
                key,
                refused("Update Kuvo on the source computer before trying the move again."),
            )
            .await;
            return Ok(());
        }
        if listing::checkpointed(self.run_dir()) && given["twoComputers"] != true {
            send(
                &self.relay,
                view,
                key,
                refused("Update both computers and wait for their current setup before trying the move again."),
            )
            .await;
            return Ok(());
        }
        let ready = || {
            packet(
                id,
                &bundle,
                "ready",
                json!({"safeSwitch":true,"verifyPayloads":true,"typedAttachmentPaths":true}),
                clock.now(),
            )
        };
        let name = given["name"].as_str().unwrap_or("It");
        let sender = encode(&key);
        if let Some(row) = before.as_ref() {
            if row.role != Role::To
                || row.peer != sender
                || row.thing != thing
                || row.kind != Some(Kind::Chat)
            {
                send(&self.relay, view, key, refused(MOVING)).await;
                return Ok(());
            }
            if row.bundle.generation > bundle.generation {
                return Ok(());
            }
            if row.dropping {
                let place = listing::place_name(listing::name_of(&view.roster, &sender));
                send(&self.relay, view, key, refused(&old_copy(name, &place))).await;
                return Ok(());
            }
        }
        if before.as_ref().is_some_and(|row| row.activation.is_some())
            || Receipts::open(self.run_dir())
                .is_decided(id)
                .map_err(io::Error::other)?
        {
            send(&self.relay, view, key, refused(MOVING)).await;
            return Ok(());
        }
        if let Some(row) = before
            .as_ref()
            .filter(|row| row.bundle.generation == bundle.generation)
        {
            if row.bundle == bundle {
                send(&self.relay, view, key, ready()).await;
            }
            return Ok(());
        }
        let place = local_place(self.run_dir(), view);
        if size > MAX_BUNDLE_BYTES {
            send(&self.relay, view, key, refused(&too_large(name))).await;
            return Ok(());
        }
        let engine = match self.engine.session_move().await {
            Ok(engine) if engine.ready().await.is_ok() => engine,
            _ => {
                send(&self.relay, view, key, refused(&self.no_engine(view))).await;
                return Ok(());
            }
        };
        let Some(here) = view.roster.here else {
            return Ok(());
        };
        let now = self.reader.now();
        let started = before.as_ref().map_or(now, |row| row.started);
        let mut row = new_journal(
            id.to_owned(),
            Kind::Chat,
            thing.into(),
            name.to_owned(),
            encode(&key),
            encode(&here),
            Role::To,
            started,
        );
        if let Some(before) = before {
            drop(engine);
            if let Err(error) = self.clear_copy(&before).await {
                send(&self.relay, view, key, refused(&old_copy(name, &place))).await;
                return Err(error);
            }
            handoff::remove_completed(self.run_dir(), &before)?;
        } else {
            let earlier = journals(self.run_dir())
                .map_err(io::Error::other)?
                .into_iter()
                .find(|row| row.live() && row.covers(thing));
            if let Some(mut earlier) = earlier {
                if earlier.thing == thing
                    && earlier.dropping
                    && earlier.role == Role::To
                    && earlier.peer == sender
                    && let Err(error) = self.drop_copy(view, key, &mut earlier, false).await
                {
                    send(&self.relay, view, key, refused(&old_copy(name, &place))).await;
                    return Err(error);
                }
                if Journal::read(self.run_dir(), &earlier.id)?.is_some() {
                    send(&self.relay, view, key, refused(MOVING)).await;
                    return Ok(());
                }
            }
            if wake_folder::at(self.run_dir(), thing)?.try_exists()?
                || engine
                    .lookup(thing)
                    .await
                    .map_err(io::Error::other)?
                    .is_some()
            {
                send(
                    &self.relay,
                    view,
                    key,
                    refused(&format!("{} is on {place} already.", row.display_name())),
                )
                .await;
                return Ok(());
            }
        }
        row.bundle = bundle.clone();
        row.size = Some(size);
        row.sent = Some(0);
        row.verify_payloads = given["verifyPayloads"] == true;
        row.extra.insert("safeSwitch".into(), true.into());
        row.extra.insert("createdSessions".into(), json!([]));
        row.extra
            .insert("wasPaused".into(), (given["wasPaused"] == true).into());
        if let Some(paused) = given["pausedAs"].as_str() {
            row.extra.insert("pausedAs".into(), paused.into());
        }
        if given["twoComputers"] == true {
            row.extra.insert("twoComputers".into(), true.into());
        }
        if let Err(error) = self.reserve(&row) {
            send(&self.relay, view, key, refused(&error.to_string())).await;
            return Ok(());
        }
        send(&self.relay, view, key, ready()).await;
        Ok(())
    }

    async fn destination_mail(
        &mut self,
        view: &View,
        key: ComputerKey,
        row: &mut Journal,
        op: &str,
        given: &Value,
    ) -> io::Result<()> {
        match op {
            "part" => {
                if let Some(reply) = self.copies.take(
                    self.reader.run.as_ref(),
                    &row.peer,
                    given,
                    self.reader.now(),
                )? {
                    send(&self.relay, view, key, reply).await;
                    if !row.complete
                        && !row.dropping
                        && let Some(mut current) = Journal::read(self.run_dir(), &row.id)?
                        && current.sent == current.size
                        && current.size.is_some()
                    {
                        self.set_up(view, key, &mut current).await?;
                    }
                }
            }
            "arrive" => {
                if row.set_up_at.is_some() && !row.dropping && row.activation.is_none() {
                    self.took(view, key, row).await;
                } else if row.activation.is_some() || row.set_up_since.is_some() {
                    self.tell(view, key, row, "working", json!({})).await;
                } else if row.sent == row.size && row.size.is_some() && !row.dropping {
                    self.set_up(view, key, row).await?;
                } else {
                    self.tell(
                        view,
                        key,
                        row,
                        "failed",
                        json!({"message":self.could_not_set_up(view)}),
                    )
                    .await;
                }
            }
            "switch" if given["safeSwitch"] == true && row.bundle.generation.is_some() => {
                if row.activation.is_none()
                    && Receipts::open(self.run_dir())
                        .is_decided(&row.id)
                        .map_err(io::Error::other)?
                {
                    return self.replay(view, key, &row.peer, given).await;
                }
                let last = if row.activation.is_none() && row.complete && row.set_up_since.is_none()
                {
                    match self.engine.session_move().await {
                        Ok(engine) => engine
                            .recent_messages(&row.thing, 1)
                            .await
                            .ok()
                            .flatten()
                            .and_then(|messages| {
                                messages.first()?.get("id")?.as_str().map(str::to_owned)
                            }),
                        Err(_) => None,
                    }
                } else {
                    None
                };
                if handoff::begin_activation(self.run_dir(), row, self.reader.now(), last)? {
                    self.activate(view, key, row).await?;
                } else if row.set_up_since.is_none()
                    && (!row.complete || row.extra.get("twoComputers") != Some(&Value::Bool(true)))
                {
                    self.drop_copy(view, key, row, false).await?;
                }
            }
            "decision?" | "drop" if row.activation.is_some() => {
                self.activate(view, key, row).await?
            }
            "drop" => self.drop_copy(view, key, row, true).await?,
            _ => {}
        }
        Ok(())
    }

    async fn resume_destination(&mut self, view: &View, row: &mut Journal) -> io::Result<()> {
        let Some(key) = decode_exact(&row.peer) else {
            return Ok(());
        };
        if row.activation.is_some() {
            self.activate(view, key, row).await
        } else {
            let interrupted = !row.dropping;
            if interrupted {
                if !view.relay_connected {
                    return Ok(());
                }
                let message = self.could_not_set_up(view);
                self.tell(view, key, row, "failed", json!({"message":message}))
                    .await;
            }
            self.drop_copy(view, key, row, false).await
        }
    }

    async fn set_up(&mut self, view: &View, key: ComputerKey, row: &mut Journal) -> io::Result<()> {
        row.set_up_since = Some(self.reader.now());
        row.complete = true;
        row.save(self.run_dir())?;
        self.tell(view, key, row, "working", json!({})).await;
        if let Err(error) = self.place_with_progress(view, key, row).await {
            let message = format!(
                "{}: {error}",
                self.could_not_set_up(view).trim_end_matches('.')
            );
            self.tell(view, key, row, "failed", json!({"message":message}))
                .await;
            self.drop_copy(view, key, row, false).await?;
            return Ok(());
        }
        row.set_up_since = None;
        row.set_up_at = Some(self.reader.now());
        row.save(self.run_dir())?;
        self.took(view, key, row).await;
        Ok(())
    }

    async fn place_with_progress(
        &self,
        view: &View,
        key: ComputerKey,
        row: &mut Journal,
    ) -> io::Result<()> {
        let id = row.id.clone();
        let identity = row.bundle.clone();
        let every = Duration::from_millis(STEP_AGAIN as u64);
        let mut beat = time::interval_at(Instant::now() + every, every);
        beat.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
        let placement = time::timeout(SET_UP_AT_MOST, self.place(row));
        tokio::pin!(placement);
        loop {
            tokio::select! {
                result = &mut placement => {
                    return result.map_err(|_| io::Error::new(
                        io::ErrorKind::TimedOut, "It took too long to set up."
                    ))?;
                }
                _ = beat.tick() => {
                    self.wire(view, key, &id, &identity, "working", json!({})).await;
                }
            }
        }
    }

    async fn place(&self, row: &mut Journal) -> io::Result<()> {
        let size = BundleSize::new(
            row.size.unwrap_or_default(),
            row.bundle.hash.as_deref().unwrap_or_default(),
        )?;
        let staged = IncomingBundle::open(&self.staged(&row.id), size)?;
        let bundle = staged.verify()?;
        let mut manifest = archive::read_manifest(bundle).map_err(io::Error::other)?;
        if manifest.thing != row.thing
            || manifest.kind != Kind::Chat
            || manifest.group.is_some()
            || manifest.decided.is_some()
            || (row.verify_payloads && manifest.session_files.is_none())
            || manifest
                .folder
                .as_deref()
                .is_some_and(|folder| folder != row.thing)
            || manifest.lane.is_some()
            || manifest.work_queue.is_some()
        {
            return Err(io::Error::other("What came isn’t a move."));
        }
        let sessions = tree(&manifest.exports, &row.thing)?;
        let home = self.run_dir().parent().expect("resident has a home");
        let directory = chats_dir(home);
        let payloads = manifest.session_files.take().unwrap_or_default();
        session_files::validate(&row.id, &sessions, &payloads)?;
        let staged_files = self.staged(&row.id).join("received-files");
        let staged_wake = self.staged(&row.id).join("received-wake");
        let wake = manifest
            .folder
            .as_deref()
            .map(|folder| (folder, staged_wake.as_path()));
        archive::unpack_chat_files(bundle, &staged_files, &payloads, wake)
            .map_err(io::Error::other)?;
        let mut queues = manifest.queues.take().unwrap_or_default();
        session_files::rewrite(home, &payloads, &mut manifest.exports, &mut queues)?;
        let engine = self.engine.session_move().await.map_err(io::Error::other)?;
        for id in &sessions {
            if engine.lookup(id).await.map_err(io::Error::other)?.is_some() {
                return Err(io::Error::other(
                    "Its conversations are on this computer already.",
                ));
            }
        }
        session_files::require_unused_paths(home, &payloads)?;
        row.sessions = sessions;
        row.extra.insert("createdSessions".into(), json!([]));
        if !payloads.is_empty() {
            row.extra.insert("typedAttachmentPaths".into(), true.into());
        }
        row.payloads = Some(Payloads {
            sessions: payloads,
            queue: vec![],
        });
        if let Some(was) = manifest.was {
            row.extra.insert("was".into(), serde_json::to_value(was)?);
        }
        row.save(self.run_dir())?;
        if let Some(payloads) = row.payloads.as_ref() {
            session_files::install(home, &staged_files, &payloads.sessions)?;
        }
        if wake.is_some() {
            wake_folder::place(self.run_dir(), row, &staged_wake, &directory)?;
        }
        files::create_private_dirs(&directory)?;
        let receipt = row.import_receipt()?;
        let mut created = Vec::new();
        for exported in &manifest.exports {
            let id = exported["info"]["id"].as_str().expect("validated its tree");
            row.extra.insert("importingSession".into(), json!(id));
            row.save(self.run_dir())?;
            engine
                .import(exported, &directory, &receipt)
                .await
                .map_err(io::Error::other)?;
            created.push(id);
            row.extra.remove("importingSession");
            row.extra.insert("createdSessions".into(), json!(created));
            row.save(self.run_dir())?;
        }
        let unpaused = engine
            .place_queues(&row.sessions, queues)
            .await
            .map_err(io::Error::other)?;
        if !unpaused.is_empty() {
            row.extra.insert("queuesGoOn".into(), json!(unpaused));
        }
        if let Some(notes) = manifest.session_notes {
            session_notes::place(self.run_dir(), &row.sessions, notes, &receipt)?;
        }
        if let Some(evidence) = manifest.forward_evidence {
            let setup = self
                .ownership
                .scope(&row.thing, None)
                .map_err(io::Error::other)?
                .setup;
            forward_evidence::place(self.run_dir(), setup.as_deref(), &row.sessions, evidence)?;
        }
        Ok(())
    }

    async fn took(&self, view: &View, key: ComputerKey, row: &Journal) {
        if let Some(payloads) = row.payloads.as_ref() {
            let home = self.run_dir().parent().expect("resident has a home");
            if session_files::validate(&row.id, &row.sessions, &payloads.sessions)
                .and_then(|()| session_files::verify(home, &payloads.sessions))
                .is_err()
            {
                self.tell(
                    view,
                    key,
                    row,
                    "failed",
                    json!({"message":self.could_not_set_up(view)}),
                )
                .await;
                return;
            }
        }
        let mut extra = if row.verify_payloads {
            json!({"verifiedPayloads":true})
        } else {
            json!({})
        };
        if row
            .payloads
            .as_ref()
            .is_some_and(|payloads| !payloads.sessions.is_empty())
        {
            extra["typedAttachmentPaths"] = true.into();
        }
        self.tell(view, key, row, "arrived", extra).await;
    }

    async fn activate(&mut self, view: &View, key: ComputerKey, row: &Journal) -> io::Result<()> {
        let receipts = Receipts::open(self.run_dir());
        if let Some(decision) = receipts
            .for_peer(&row.id, &row.peer)
            .map_err(io::Error::other)?
        {
            if decision.decided != Decided::Switched || decision.bundle != row.bundle {
                return Err(io::Error::other(
                    "Move ownership conflicts with its activation record",
                ));
            }
            receipts.seal(&row.id).map_err(io::Error::other)?;
        } else {
            let here = view
                .roster
                .here
                .ok_or_else(|| io::Error::other("This computer has no setup identity"))?;
            if row.verify_payloads && row.payloads.is_none() {
                return Err(io::Error::other(
                    "The move’s file inventory is unavailable.",
                ));
            }
            if let Some(payloads) = row.payloads.as_ref() {
                let home = self.run_dir().parent().expect("resident has a home");
                session_files::validate(&row.id, &row.sessions, &payloads.sessions)?;
                session_files::verify(home, &payloads.sessions)?;
            }
            let engine = self.engine.session_move().await.map_err(io::Error::other)?;
            let needed = row
                .extra
                .get("queuesGoOn")
                .cloned()
                .unwrap_or_else(|| json!([]));
            let unpaused: Vec<String> = serde_json::from_value(needed)?;
            handoff::claim_destination(self.run_dir(), row, &encode(&here))?;
            for prior in left::read(self.run_dir())?
                .into_iter()
                .filter(|prior| prior.thing == row.thing)
            {
                if let Err(error) = left::forget(self.run_dir(), &prior.id) {
                    eprintln!("Removing a prior move’s retired copy: {error}");
                }
            }
            self.tell_moved(&engine, row).await;
            engine
                .activate_queues(&row.id, &unpaused)
                .await
                .map_err(io::Error::other)?;
            wake_folder::release(self.run_dir(), row)?;
            handoff::commit_destination(self.run_dir(), row)?;
        }
        handoff::remove_completed(self.run_dir(), row)?;
        self.retire_memory(&row.id);
        self.answer_decision(view, key, row).await?;
        Ok(())
    }

    async fn tell_moved(&self, engine: &SessionMove, row: &Journal) {
        let Some(was) = row.extra.get("was") else {
            return;
        };
        let Some(place) = was["place"].as_str() else {
            return;
        };
        let home = self.run_dir().parent().expect("resident has a home");
        let folder = chats_dir(home);
        let mut words = format!(
            "You moved to this computer from {place}. What you did before that happened there."
        );
        if let (Some(from), Some(to)) = (was["folder"].as_str(), folder.to_str())
            && from != to
        {
            words.push_str(&format!(
                " Paths in your conversation under {from} are under {to} now."
            ));
        }
        if let Some(payloads) = row.payloads.as_ref() {
            let root = self
                .run_dir()
                .parent()
                .expect("resident has a home")
                .join("attachments");
            for file in &payloads.sessions {
                if let Ok(target) = session_files::checked_path(&root, Path::new(&file.relative)) {
                    words.push_str(&format!(
                        " The attachment at {} is at {} now.",
                        file.from,
                        target.display()
                    ));
                }
            }
        }
        words.push_str(&format!(" Other paths on {place} aren't here, and the sessions you started there stay there: reach them with list_agents, send_message and read_conversation."));
        if let Err(error) = engine.moved_instruction(&row.thing, &words).await {
            eprintln!("Telling the moved conversation where it is: {error}");
        }
    }

    async fn drop_copy(
        &mut self,
        view: &View,
        key: ComputerKey,
        row: &mut Journal,
        explicit: bool,
    ) -> io::Result<()> {
        let receipts = Receipts::open(self.run_dir());
        if let Some(decision) = receipts
            .for_peer(&row.id, &row.peer)
            .map_err(io::Error::other)?
            && decision.decided != Decided::Dropped
        {
            return self.answer_decision(view, key, row).await;
        }
        if row.complete
            && row.set_up_since.is_none()
            && !row.dropping
            && (row.extra.get("safeSwitch") != Some(&Value::Bool(true))
                || !explicit && row.extra.get("twoComputers") == Some(&Value::Bool(true)))
        {
            return Ok(());
        }
        receipts
            .record(
                &row.id,
                &Decision {
                    decided: Decided::Dropped,
                    at: self.reader.now(),
                    bundle: row.bundle.clone(),
                    payloads: None,
                    last: None,
                    peer: Some(row.peer.clone()),
                },
                &row.sessions,
            )
            .map_err(io::Error::other)?;
        if !row.dropping {
            row.dropping = true;
            row.extra
                .insert("dropSince".into(), self.reader.now().into());
            row.save(self.run_dir())?;
        }
        self.mark_retry(row);
        if explicit {
            self.answer_decision(view, key, row).await?;
        }
        self.clear_copy(row).await?;
        handoff::remove_completed(self.run_dir(), row)?;
        self.retire_memory(&row.id);
        if !explicit {
            self.answer_decision(view, key, row).await?;
        }
        Ok(())
    }

    async fn clear_copy(&self, row: &Journal) -> io::Result<()> {
        if row.sessions.is_empty() {
            return Ok(());
        }
        if let Some(payloads) = row.payloads.as_ref() {
            let home = self.run_dir().parent().expect("resident has a home");
            session_files::validate(&row.id, &row.sessions, &payloads.sessions)?;
            session_files::discard(home, &payloads.sessions)?;
        }
        let created = row.extra.get("createdSessions").and_then(Value::as_array);
        let importing = row.extra.get("importingSession").and_then(Value::as_str);
        if created.is_none() && importing.is_none() {
            return Err(io::Error::other(
                "The conversation copy has no import ownership receipt",
            ));
        }
        let receipt = row.import_receipt()?;
        session_notes::discard(self.run_dir(), &row.sessions, Some(&receipt))?;
        if created.is_none_or(Vec::is_empty) && importing.is_none() {
            return wake_folder::discard(self.run_dir(), row);
        }
        let engine = self.engine.session_move().await.map_err(io::Error::other)?;
        for id in row.sessions.iter().rev() {
            let placed = created.is_some_and(|ids| ids.iter().any(|entry| entry == id));
            if placed {
                engine.forget_queues(id).await.map_err(io::Error::other)?;
            }
            if placed || importing == Some(id.as_str()) {
                engine
                    .discard_import(id, &receipt)
                    .await
                    .map_err(io::Error::other)?;
            }
        }
        wake_folder::discard(self.run_dir(), row)
    }

    async fn answer_decision(
        &self,
        view: &View,
        key: ComputerKey,
        row: &Journal,
    ) -> io::Result<()> {
        self.replay(view, key, &row.peer, &json!({"op":"decision?","id":row.id,"generation":row.bundle.generation,"hash":row.bundle.hash})).await
    }

    async fn replay(
        &self,
        view: &View,
        key: ComputerKey,
        peer: &str,
        given: &Value,
    ) -> io::Result<()> {
        if let Some(reply) = receipt::replay(self.run_dir(), peer, given, self.reader.now())
            .map_err(io::Error::other)?
        {
            send(&self.relay, view, key, reply).await;
        }
        Ok(())
    }

    async fn tell(
        &self,
        view: &View,
        key: ComputerKey,
        row: &Journal,
        op: &str,
        extra: Value,
    ) -> bool {
        self.wire(view, key, &row.id, &row.bundle, op, extra).await
    }

    async fn wire(
        &self,
        view: &View,
        key: ComputerKey,
        id: &str,
        bundle: &Bundle,
        op: &str,
        extra: Value,
    ) -> bool {
        send(
            &self.relay,
            view,
            key,
            packet(id, bundle, op, extra, self.reader.now()),
        )
        .await
    }

    fn could_not_set_up(&self, view: &View) -> String {
        format!("It couldn’t be set up on {}.", view.roster.name)
    }

    async fn check_destination(&self, view: &View, given: &Value) -> Result<(), String> {
        let place = local_place(self.run_dir(), view);
        if given["lane"] == true {
            return Err(format!(
                "The coding agents on {place} aren’t running. Try again once they are."
            ));
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        let engine = tokio::time::timeout_at(deadline, async {
            let engine = self.engine.session_move().await?;
            engine.ready().await?;
            Ok::<_, kuvo_engine::MoveEngineError>(engine)
        })
        .await
        .ok()
        .and_then(Result::ok)
        .ok_or_else(|| self.no_engine(view))?;
        if given["kind"].as_str() != Some("group")
            && let Some(model) = model(given)
            && matches!(
                tokio::time::timeout_at(deadline, engine.sign_in_needed(&model.provider)).await,
                Ok(Ok(true))
            )
        {
            let name = given["name"].as_str().unwrap_or("It");
            return Err(format!(
                "{name}’s model needs a sign-in on {place}. Sign in to it in Settings › Accounts there, or choose another model, then move {name}."
            ));
        }
        Ok(())
    }

    fn no_engine(&self, view: &View) -> String {
        format!(
            "The engine on {} isn’t running. Try again once it is.",
            local_place(self.run_dir(), view)
        )
    }
}

#[derive(Deserialize, serde::Serialize)]
struct Model {
    #[serde(rename = "providerID")]
    provider: String,
    id: String,
}

fn model(row: &Value) -> Option<Model> {
    serde_json::from_value(row.get("model")?.clone()).ok()
}

fn local_place(run: &Path, view: &View) -> String {
    if listing::is_mac(run) {
        return "this Mac".into();
    }
    let name = listing::plain_name(&view.roster.name);
    if name.is_empty() {
        "this machine".into()
    } else {
        name
    }
}

async fn snapshot(
    engine: &SessionMove,
    row: &Journal,
) -> io::Result<(Vec<Value>, std::collections::BTreeMap<String, Value>)> {
    let mut exports = engine
        .export_tree(&row.thing)
        .await
        .map_err(io::Error::other)?;
    if tree(&exports, &row.thing)?
        .into_iter()
        .collect::<HashSet<_>>()
        != row.sessions.iter().cloned().collect()
    {
        return Err(io::Error::other(
            "Its conversations changed during the move.",
        ));
    }
    exports.sort_by_key(|export| {
        let id = export["info"]["id"].as_str().unwrap_or_default();
        row.sessions.iter().position(|session| session == id)
    });
    let queues = engine
        .queue_snapshot(&row.sessions)
        .await
        .map_err(io::Error::other)?;
    Ok((exports, queues))
}

fn tree(exports: &[Value], root: &str) -> io::Result<Vec<String>> {
    let invalid = || io::Error::other("Its conversations aren’t its own.");
    let mut seen = HashSet::new();
    let mut sessions = Vec::new();
    for export in exports {
        let id = export["info"]["id"]
            .as_str()
            .filter(|id| session_id(id))
            .ok_or_else(invalid)?;
        let parent = &export["info"]["parentID"];
        if (!export["messages"].is_array())
            || (seen.is_empty() && (id != root || !parent.is_null()))
            || (!seen.is_empty() && !parent.as_str().is_some_and(|parent| seen.contains(parent)))
            || !seen.insert(id)
        {
            return Err(invalid());
        }
        sessions.push(id.into());
    }
    if sessions.is_empty() {
        return Err(invalid());
    }
    Ok(sessions)
}

fn without_clocks(queues: &std::collections::BTreeMap<String, Value>) -> io::Result<Value> {
    let mut value = serde_json::to_value(queues)?;
    if let Value::Object(rows) = &mut value {
        for queue in rows.values_mut() {
            if let Some(waiting) = queue["waiting"].as_array_mut() {
                for item in waiting {
                    if let Some(shown) = item["shown"].as_object_mut() {
                        shown.remove("at");
                    }
                }
            }
        }
    }
    Ok(value)
}

impl Journal {
    fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or("It")
    }

    fn import_receipt(&self) -> io::Result<String> {
        let generation = self
            .bundle
            .generation
            .ok_or_else(|| io::Error::other("The move has no bundle generation"))?;
        let hash = self
            .bundle
            .hash
            .as_deref()
            .ok_or_else(|| io::Error::other("The move has no bundle digest"))?;
        Ok(format!("{}:{generation}:{hash}", self.id))
    }
}

pub(super) async fn chat(engine: &Engine, run: &Path, id: &str) -> Result<Value, Refusal> {
    if !session_id(id) {
        return Err(gone());
    }
    let session = engine
        .session_move()
        .await
        .map_err(|_| gone())?
        .lookup(id)
        .await
        .map_err(|_| gone())?
        .ok_or_else(gone)?;
    let home = run.parent().expect("the run folder is in the home");
    let chats = chats_dir(home);
    let directory = session
        .pointer("/location/directory")
        .or_else(|| session.get("directory"))
        .and_then(Value::as_str)
        .map(Path::new);
    let is_chat = directory.is_some_and(|folder| same_path(folder, &chats))
        || session
            .pointer("/metadata/kuvo/chat")
            .and_then(Value::as_str)
            .is_some_and(|folder| same_path(Path::new(folder), &chats));
    if !is_chat || session["parentID"].as_str().is_some() {
        return Err(Refusal::new(
            StatusCode::CONFLICT,
            "It isn’t a chat that moves on its own.",
        ));
    }
    Ok(session)
}

fn chats_dir(home: &Path) -> PathBuf {
    std::env::var_os("KUVO_CHATS")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            if std::env::var("KUVO_BOX").as_deref() == Ok("mac") {
                if let Some(support) = home.parent() {
                    return support.join("Chats");
                }
            }
            home.join("chats")
        })
}

pub(super) fn chat_name(session: &Value) -> String {
    let title = session["title"].as_str().unwrap_or_default().trim();
    if title.is_empty() {
        "This chat".into()
    } else {
        format!("“{title}”")
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    a == b
        || a.canonicalize()
            .is_ok_and(|a| b.canonicalize().is_ok_and(|b| b == a))
}

fn conversation(exports: &[Value]) -> Vec<(&Value, &Value)> {
    exports
        .iter()
        .map(|export| (&export["info"]["id"], &export["messages"]))
        .collect()
}

pub(super) fn session_id(id: &str) -> bool {
    id.strip_prefix("ses_").is_some_and(|suffix| {
        (1..=120).contains(&suffix.len())
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    })
}

fn index(given: &Value) -> Option<u32> {
    let value = given["i"].as_f64()?;
    (value >= 0.0 && value <= f64::from(u32::MAX) && value.fract() == 0.0).then_some(value as u32)
}

fn too_large(name: &str) -> String {
    format!(
        "{name} is more than 1 GB, too big to move to another computer, so it stays where it is."
    )
}

fn old_copy(name: &str, place: &str) -> String {
    format!(
        "An earlier copy of {name} on {place} couldn’t be let go of yet. Try again in a moment."
    )
}

fn waiting_on_peer(row: &Journal, now: Millis, sent: Option<Duration>, heard: Duration) -> bool {
    if !row.live() {
        return false;
    }
    let stamp = |key| row.extra.get(key).and_then(Value::as_i64);
    let (asked, delay) = match row.role {
        Role::To if row.dropping || row.set_up_since.is_some() => return false,
        Role::To if row.complete => (
            Some(row.set_up_at.unwrap_or(row.started)),
            WAITING_FOR_SWITCH,
        ),
        Role::To => (Some(row.started), WAITING_AFTER),
        Role::From => {
            let asked = match row.step.as_deref() {
                Some("switch") => stamp("switchSince"),
                Some("drop") if row.stage == Stage::Switching => stamp("switchSince"),
                Some("parts" | "arrive") => stamp("sentAt"),
                Some("offer") if stamp("tries").is_some_and(|tries| tries > 0) => stamp("sentAt"),
                _ => None,
            };
            (asked, WAITING_AFTER)
        }
    };
    heard > delay
        && asked.is_some_and(|asked| {
            sent.map_or_else(
                || asked > now || now.saturating_sub(asked) > delay.as_millis() as Millis,
                |sent| sent > delay,
            )
        })
}

fn new_journal(
    id: String,
    kind: Kind,
    thing: String,
    name: String,
    from: String,
    to: String,
    role: Role,
    started: Millis,
) -> Journal {
    Journal {
        id,
        thing,
        name: Some(name),
        kind: Some(kind),
        peer: match role {
            Role::From => to.clone(),
            Role::To => from.clone(),
        },
        from: Some(from),
        to: Some(to),
        role,
        stage: match role {
            Role::From => Stage::Holding,
            Role::To => Stage::Copying,
        },
        sessions: Vec::new(),
        started,
        step: None,
        failed: None,
        dropping: false,
        complete: false,
        cleanup_pending: None,
        note: None,
        sent: None,
        size: None,
        activation: None,
        set_up_since: None,
        set_up_at: None,
        verify_payloads: false,
        payloads: None,
        bundle: Bundle::default(),
        extra: Map::from_iter([
            ("wasPaused".into(), false.into()),
            ("tries".into(), 0.into()),
        ]),
    }
}

async fn send(relay: &RelayHandle, view: &View, key: ComputerKey, message: Envelope) -> bool {
    if let Some(to) = view.roster.member(&key).and_then(|member| member.host()) {
        return relay.sender.mail(to, message).await.is_ok();
    }
    false
}

fn packet(id: &str, bundle: &Bundle, op: &str, extra: Value, now: Millis) -> Envelope {
    let mut movement = extra.as_object().cloned().unwrap_or_default();
    movement.insert("op".into(), op.into());
    movement.insert("id".into(), id.into());
    if let Some(generation) = bundle.generation {
        movement.insert("generation".into(), generation.into());
    }
    if let Some(hash) = bundle.hash.as_deref() {
        movement.insert("hash".into(), hash.into());
    }
    Envelope(Map::from_iter([
        ("at".into(), now.into()),
        ("move".into(), movement.into()),
    ]))
}

fn unavailable() -> Refusal {
    Refusal::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "This computer isn’t connected.",
    )
}

fn gone() -> Refusal {
    Refusal::new(StatusCode::NOT_FOUND, "It isn’t on this computer any more.")
}

fn remove_orphaned_bundles(
    run: &Path,
    seen: &mut HashMap<String, Instant>,
    now: Instant,
) -> io::Result<()> {
    let moves = run.join("moves");
    let entries = match std::fs::read_dir(&moves) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            seen.clear();
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    let mut absent = HashSet::new();
    for entry in entries {
        let entry = entry?;
        let Ok(id) = entry.file_name().into_string() else {
            continue;
        };
        if move_id(&id).is_none()
            || !entry.file_type()?.is_dir()
            || moves.join(format!("{id}.json")).try_exists()?
        {
            continue;
        }
        let since = seen.entry(id.clone()).or_insert(now);
        if now.duration_since(*since) > ORPHAN_AFTER {
            *since = now;
            files::remove_tree_synced(&entry.path())?;
        } else {
            absent.insert(id);
        }
    }
    seen.retain(|id, _| absent.contains(id));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_orphaned_bundle_is_removed_only_after_the_grace_period() {
        let home = tempfile::tempdir().unwrap();
        let run = home.path().join("run");
        let moves = run.join("moves");
        let orphan = moves.join("mov_orphan");
        let active = moves.join("mov_active");
        let other = moves.join("other");
        for bundle in [&orphan, &active, &other] {
            files::write_atomic(&bundle.join("bundle.tgz"), b"a staged copy").unwrap();
        }
        files::write_atomic(&moves.join("mov_active.json"), b"{}").unwrap();
        let mut seen = HashMap::new();
        let now = Instant::now();
        for offset in [Duration::ZERO, ORPHAN_AFTER] {
            remove_orphaned_bundles(&run, &mut seen, now + offset).unwrap();
            assert!(orphan.is_dir());
        }
        remove_orphaned_bundles(&run, &mut seen, now + ORPHAN_AFTER + Duration::from_secs(1))
            .unwrap();
        assert!(!orphan.exists());
        assert!(active.join("bundle.tgz").is_file());
        assert!(other.join("bundle.tgz").is_file());
    }
}
