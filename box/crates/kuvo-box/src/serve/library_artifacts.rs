//! Wire Library and Artifacts into the resident (`serve.rs`), including peer snapshots,
//! memory-read answering, bot folder watches, and passive engine event recording.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use kuvo_os::watch::{WatchHub, FileSet};
use sha2::{Digest, Sha256};
use kuvo_peers::{
    memory_read::{Answer, Handle as MemoryReads, Kind as ReadKind, Read},
    relay::{RelayEvent, RelayHandle},
    setup_lists::Reader,
};
use kuvo_server::Registry;
use kuvo_services::{
    artifacts::{
        ArtifactHandle, ArtifactStore, OutputPeerCopy, OutputsFuture, OutputsPeers,
        OutputsSnapshot, PeerOutputRow, PendingComputer, StoreError, StoreOptions,
        folders::{BotFolders, wire_record},
        local_output_copy,
        prepare::{citation_update, load_citation_checkpoints, save_citation_checkpoints},
        recorder::{Event, Recorder, RecorderOptions, SessionDetails, SessionFuture, SessionSource},
    },
    library::{
        Asked, Filter, LibraryHandle, LibraryOptions, LibraryPeers, LibraryService, PartState,
        PeerComputer, ThumbnailPlatform, filter_peer_copy, is_part, keep_part, kept_part,
        library_for_peer,
        peers::{away_hidden_message, last_seen_message, older_message, valid_peer_key},
    },
};
use kuvo_wire::{
    keys::{Host, decode_exact, encode},
    mail::Envelope,
};
use serde_json::{Map, Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::watch,
    task::JoinHandle,
};

const LIBRARY_FEATURE: &str = "library";
const MERGED_LIBRARY_FEATURE: &str = "line.merged-library";
const MERGED_OUTPUTS_FEATURE: &str = "line.merged-outputs";
const CHANGES_FEATURE: &str = "data.library-change-copy";
const LIBRARY_FILE: &str = "library-v1.json";
const OUTPUTS_FILE: &str = "outputs-v1.json";

pub(super) struct ResidentLibraryArtifacts {
    library: LibraryHandle,
    folders: Option<BotFolders>,
    tasks: Vec<JoinHandle<()>>,
}

impl ResidentLibraryArtifacts {
    pub(super) async fn start(
        home: &Path,
        computer_name: String,
        peers: Reader,
        relay: Option<RelayHandle>,
        reads: MemoryReads,
        engine: kuvo_engine::Engine,
        routes: &mut Registry,
    ) -> Self {
        let (artifacts_root, library_root, bots_dir, sessions_dir, run_dir) = (
            home.join("artifacts"),
            home.join("library"),
            home.join("bots"),
            home.join("agents/sessions"),
            home.join("run"),
        );
        let prev = load_citation_checkpoints(&artifacts_root);
        let (shown, checkpoints) = citation_update(&sessions_dir, &prev);
        let store_res = ArtifactStore::open(
            &artifacts_root,
            StoreOptions { bots: Some(bots_dir.clone()), shown, ..Default::default() },
        );
        let mut tasks = Vec::new();
        let (artifact_handle, library_handle, folders) = match store_res {
            Ok(store) => {
                // An unchanged checkpoint stays as it is: a departed computer's
                // released work must not change by starting again.
                if checkpoints != prev {
                    save_citation_checkpoints(&artifacts_root, &checkpoints);
                }
                let lib_res = LibraryService::open(
                    &library_root,
                    LibraryOptions {
                        bots: bots_dir.clone(),
                        sessions: sessions_dir,
                        artifacts: Some(store.clone()),
                        computer: Arc::new(move || computer_name.clone()),
                        platform: ThumbnailPlatform::current(),
                        clock: None,
                    },
                );
                let (library_handle, handed) = match lib_res {
                    Ok(service) => {
                        service.backfill();
                        let svc = service.clone();
                        (LibraryHandle::Ready(service), Some(Arc::new(move |h| svc.handed(h)) as _))
                    }
                    Err(kuvo_services::library::LibraryStoreError::UnsupportedFormat(msg)) => {
                        (LibraryHandle::UnsupportedFormat(msg), None)
                    }
                    Err(err) => {
                        eprintln!("library: could not start: {err}");
                        (LibraryHandle::Unavailable, None)
                    }
                };
                let (recorder, folders) = wire_record(
                    store.clone(),
                    Some(bots_dir),
                    Duration::from_secs(1),
                    RecorderOptions { handed, ..Default::default() },
                );
                if let Some(folders) = &folders {
                    let hub = WatchHub::default();
                    folders.start(Some(&hub)).await;
                }
                tasks.push(tokio::spawn(follow_engine_events(run_dir.clone(), engine.events(), recorder)));
                (ArtifactHandle::Ready(store), library_handle, folders)
            }
            Err(StoreError::UnsupportedFormat(msg)) => {
                (ArtifactHandle::UnsupportedFormat(msg), LibraryHandle::Unavailable, None)
            }
            Err(err) => {
                eprintln!("artifacts: could not start: {err}");
                (ArtifactHandle::Unavailable, LibraryHandle::Unavailable, None)
            }
        };

        let (peer_tx, peer_rx) = watch::channel(0u64);
        if let Some(relay) = relay {
            tasks.push(tokio::spawn(sync_peer_snapshots(
                run_dir.clone(), peers.clone(), relay, artifact_handle.clone(), library_handle.clone(), peer_tx,
            )));
        }
        let adapter = Arc::new(PeerAdapter { run: run_dir, peers, reads, changes: peer_rx });
        kuvo_services::artifacts::register(routes, artifact_handle, Some(adapter.clone()));
        kuvo_services::library::register(routes, library_handle.clone(), Some(adapter));
        Self { library: library_handle, folders, tasks }
    }

    pub(super) async fn answer_library_read(&self, read: Read) -> Answer {
        let LibraryHandle::Ready(service) = &self.library else {
            return Answer {
                status: 503,
                value: json!({ "message": "This computer’s Library can’t be read right now. Try again in a moment." }),
            };
        };
        let (status, value) = library_for_peer(service, "GET", &read.item, read.body.as_ref()).await;
        Answer { status, value }
    }

    pub(super) async fn stop(self) {
        if let Some(folders) = self.folders { folders.stop(); }
        for task in self.tasks { task.abort(); let _ = task.await; }
    }
}

struct PeerAdapter {
    run: PathBuf,
    peers: Reader,
    reads: MemoryReads,
    changes: watch::Receiver<u64>,
}

impl PeerAdapter {
    fn peer_copy_path(&self, key: &str, file: &str) -> PathBuf {
        self.run.join("peers").join(key).join(file)
    }

    fn read_peer_snapshot(&self, key: &str, file: &str) -> Option<(i64, Value)> {
        if !valid_peer_key(key) { return None; }
        let val: Value = serde_json::from_str(&fs::read_to_string(self.peer_copy_path(key, file)).ok()?).ok()?;
        Some((val.get("seen")?.as_i64()?, val))
    }

    fn peer_sessions_children(&self) -> HashMap<String, Vec<String>> {
        let mut parents: HashMap<String, HashSet<String>> = HashMap::new();
        let mut link = |id: &str, parent: Option<&str>, line: Option<&[String]>| {
            if let Some(p) = parent { parents.entry(p.to_owned()).or_default().insert(id.to_owned()); }
            for p in line.into_iter().flatten() { parents.entry(p.clone()).or_default().insert(id.to_owned()); }
        };
        let view = self.peers.view();
        if let Some(local) = &view.local.list {
            for (id, row) in &local.sessions {
                link(id, row.facts.parent.as_deref(), row.facts.line.as_deref());
            }
        }
        for member in &view.roster.members {
            let Ok(raw) = fs::read_to_string(self.peer_copy_path(&encode(&member.key), "lists-v1.json")) else { continue };
            let Ok(val) = serde_json::from_str::<Value>(&raw) else { continue };
            for entry in val.get("sessions").and_then(Value::as_array).into_iter().flatten() {
                let (Some(id), Some(facts)) = (entry.get("id").and_then(Value::as_str), entry.get("facts")) else { continue };
                let line: Option<Vec<String>> = facts.get("line").and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect());
                link(id, facts.get("parent").and_then(Value::as_str), line.as_deref());
            }
        }
        parents.into_iter().map(|(k, s)| (k, s.into_iter().collect())).collect()
    }
}

fn asked_with_kept(computer: PeerComputer, kept: Option<kuvo_services::library::peers::KeptEntry>, away: bool, fallback: String) -> Asked {
    match kept {
        Some(k) => Asked {
            state: if away { PartState::Away(last_seen_message(&computer.name)) } else { PartState::Failed(last_seen_message(&computer.name)) },
            computer,
            part: Some(k.part),
            seen: Some(k.seen),
        },
        None => Asked {
            state: if away { PartState::Away(fallback) } else { PartState::Failed(fallback) },
            computer,
            part: None,
            seen: None,
        },
    }
}

impl OutputsPeers for PeerAdapter {
    fn snapshot(&self) -> OutputsFuture<'_> {
        Box::pin(async move {
            let (view, now) = (self.peers.view(), self.peers.now());
            let local = view.roster.local();
            let children = self.peer_sessions_children();
            let (mut copies, mut have_copy) = (Vec::new(), HashSet::new());
            for member in &view.roster.members {
                let key_str = encode(&member.key);
                let Some((seen, val)) = self.read_peer_snapshot(&key_str, OUTPUTS_FILE) else { continue };
                let Ok(artifacts) = serde_json::from_value::<Vec<PeerOutputRow>>(val.get("artifacts").cloned().unwrap_or(json!([]))) else { continue };
                have_copy.insert(member.key);
                let standing = view.standing(member.key, now);
                copies.push(OutputPeerCopy {
                    computer_id: key_str,
                    computer_name: member.name.clone(),
                    away: standing.away(),
                    last_seen: standing.last_seen().or(Some(seen)),
                    artifacts,
                });
            }
            let pending = view.pending(MERGED_OUTPUTS_FEATURE, |k| have_copy.contains(k))
                .into_iter().map(|c| PendingComputer { id: c.id, name: c.name }).collect();
            Some(OutputsSnapshot { local_id: local.id, local_name: local.name, children, peers: copies, pending })
        })
    }

    fn changes(&self) -> watch::Receiver<u64> { self.changes.clone() }
}

impl LibraryPeers for PeerAdapter {
    fn local(&self) -> PeerComputer {
        let local = self.peers.view().roster.local();
        PeerComputer { key: local.id, name: local.name, here: true }
    }

    fn peers_dir(&self) -> PathBuf { self.run.join("library/peers") }
    fn changes(&self) -> watch::Receiver<u64> { self.changes.clone() }

    fn kept_peers(&self, query: &str) -> Vec<Asked> {
        let (view, now, dir) = (self.peers.view(), self.peers.now(), self.peers_dir());
        view.roster.members.iter().filter_map(|member| {
            let key_str = encode(&member.key);
            let computer = PeerComputer { key: key_str.clone(), name: member.name.clone(), here: false };
            if !view.advertises(&member.key, LIBRARY_FEATURE) {
                return Some(Asked { state: PartState::Older(older_message(&member.name)), computer, part: None, seen: None });
            }
            let kept = kept_part(&dir, &key_str, query);
            if !view.relay_connected || view.standing(member.key, now).away() {
                return Some(asked_with_kept(computer, kept, true, away_hidden_message(&member.name)));
            }
            kept.map(|k| Asked { computer, state: PartState::There, part: Some(k.part), seen: Some(k.seen) })
        }).collect()
    }

    fn ask_peers<'a>(&'a self, query: &'a str) -> kuvo_services::library::peers::PeerFuture<'a, Vec<Asked>> {
        Box::pin(async move {
            let (view, now, dir) = (self.peers.view(), self.peers.now(), self.peers_dir());
            let mut out = Vec::new();
            for member in &view.roster.members {
                let key_str = encode(&member.key);
                let computer = PeerComputer { key: key_str.clone(), name: member.name.clone(), here: false };
                if !view.advertises(&member.key, LIBRARY_FEATURE) {
                    out.push(Asked { state: PartState::Older(older_message(&member.name)), computer, part: None, seen: None });
                    continue;
                }
                if !view.relay_connected || view.standing(member.key, now).away() {
                    out.push(asked_with_kept(computer, kept_part(&dir, &key_str, query), true, away_hidden_message(&member.name)));
                    continue;
                }
                let read = Read {
                    area: Some("library".into()),
                    item: "list".into(),
                    kind: ReadKind::Record,
                    body: Some([("query".into(), Value::String(query.into()))].into_iter().collect()),
                };
                match self.reads.request(member.key, read).await {
                    Ok(ans) if ans.status == 200 && is_part(&ans.value) => {
                        keep_part(&dir, &key_str, query, ans.value.clone(), now);
                        out.push(Asked { computer, state: PartState::There, part: Some(ans.value), seen: None });
                    }
                    Ok(ans) if ans.status == 409 => {
                        let msg = ans.value.get("message").and_then(Value::as_str)
                            .map_or_else(|| older_message(&member.name), str::to_owned);
                        out.push(Asked { computer, state: PartState::Older(msg), part: None, seen: None });
                    }
                    Ok(_) => out.push(asked_with_kept(
                        computer,
                        kept_part(&dir, &key_str, query),
                        false,
                        format!("{}’s Library couldn’t be read. Try again in a moment.", member.name),
                    )),
                    Err(err) => out.push(asked_with_kept(computer, kept_part(&dir, &key_str, query), true, err.to_string())),
                }
            }
            out
        })
    }

    fn ask_item<'a>(
        &'a self,
        computer_key: &'a str,
        method: &'a str,
        item: &'a str,
        body: Option<Map<String, Value>>,
    ) -> kuvo_services::library::peers::PeerFuture<'a, Result<(u16, Value), (u16, String)>> {
        Box::pin(async move {
            let gone = || (404, "That computer isn’t one of yours any more.".to_owned());
            let key = decode_exact::<32>(computer_key).ok_or_else(gone)?;
            let view = self.peers.view();
            let member = view.roster.member(&key).ok_or_else(gone)?;
            if !view.advertises(&key, LIBRARY_FEATURE) { return Err((409, older_message(&member.name))); }
            if method != "GET" {
                return Err((503, format!("{} isn’t answering now. Try again in a moment.", member.name)));
            }
            let read = Read { area: Some("library".into()), item: item.into(), kind: ReadKind::Record, body };
            self.reads.request(key, read).await
                .map(|a| (a.status, a.value))
                .map_err(|e| (503, e.to_string()))
        })
    }

    fn merged_snapshot<'a>(
        &'a self,
        filter: &'a Filter,
        query: &'a str,
    ) -> kuvo_services::library::peers::PeerFuture<'a, (Vec<Asked>, Vec<PendingComputer>)> {
        Box::pin(async move {
            let (view, now, dir) = (self.peers.view(), self.peers.now(), self.peers_dir());
            let (mut known, mut known_keys) = (Vec::new(), HashSet::new());
            for member in &view.roster.members {
                let key_str = encode(&member.key);
                let computer = PeerComputer { key: key_str.clone(), name: member.name.clone(), here: false };
                let copy = self.read_peer_snapshot(&key_str, LIBRARY_FILE);
                let legacy = copy.is_none().then(|| kept_part(&dir, &key_str, query)).flatten();
                let seen = copy.as_ref().map(|(s, _)| *s).or_else(|| legacy.as_ref().map(|k| k.seen));
                let aged = !view.advertises(&member.key, CHANGES_FEATURE) && seen.is_some_and(|s| now - s > 180_000);
                let away = view.standing(member.key, now).away() || (seen.is_some() && (!view.relay_connected || aged));
                if copy.is_none() && legacy.is_none() {
                    if away {
                        known_keys.insert(member.key);
                        known.push(Asked { state: PartState::Away(away_hidden_message(&member.name)), computer, part: None, seen: None });
                    } else if !view.advertises(&member.key, MERGED_LIBRARY_FEATURE) && view.advertises(&member.key, "info") {
                        known_keys.insert(member.key);
                        known.push(Asked { state: PartState::Older(older_message(&member.name)), computer, part: None, seen: None });
                    }
                    continue;
                }
                let part = match (&copy, &legacy) {
                    (Some((_, val)), _) => filter_peer_copy(val, filter, &member.name, away),
                    (None, Some(leg)) => Some(leg.part.clone()),
                    (None, None) => None,
                };
                let Some(part) = part else { continue };
                known_keys.insert(member.key);
                let (state, out_seen) = if away { (PartState::Away(last_seen_message(&member.name)), seen) } else { (PartState::There, None) };
                known.push(Asked { computer, state, part: Some(part), seen: out_seen });
            }
            let pending = view.pending(MERGED_LIBRARY_FEATURE, |k| known_keys.contains(k))
                .into_iter().map(|c| PendingComputer { id: c.id, name: c.name }).collect();
            (known, pending)
        })
    }
}

async fn sync_peer_snapshots(
    run: PathBuf,
    mut peers: Reader,
    relay: RelayHandle,
    artifacts: ArtifactHandle,
    library: LibraryHandle,
    notify: watch::Sender<u64>,
) {
    let Some(mut events) = relay.subscribe() else { return };
    let mut seq = 0u64;
    let mut art_rx = match &artifacts { ArtifactHandle::Ready(s) => Some(s.subscribe()), _ => None };
    let mut lib_rx = match &library { LibraryHandle::Ready(s) => Some(s.store().subscribe()), _ => None };
    let mut sent = HashMap::<([u8; 32], &'static str), SentCopy>::new();
    // A peer can announce agents before its Library receiver is installed. Modern
    // recipients explicitly say they are ready, so an initial copy cannot be lost.
    let mut requested = HashSet::new();
    let mut ready = HashSet::new();
    let mut cached = None;
    let mut source_watches = FileSet::new(WatchHub::default());
    loop {
        let next_legacy = push_snapshot(&peers, &relay, &artifacts, &library, &mut cached, &mut sent, &mut requested, &mut ready, &mut source_watches).await;

        tokio::select! {
            res = events.recv() => match res {
                Ok(event) => match event.as_ref() {
                    RelayEvent::Mail { key, envelope, .. } => {
                        let key_str = encode(key);
                        if envelope.0.get("ask").and_then(Value::as_str) == Some("library-copy") {
                            ready.insert(*key);
                            sent.retain(|(recipient, _), _| recipient != key);
                        }
                        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
                        let mut updated = false;
                        for (field, file) in [("libraryCopy", LIBRARY_FILE), ("outputCopy", OUTPUTS_FILE)] {
                            if let Some(copy) = envelope.0.get(field).and_then(Value::as_object) {
                                let mut doc = copy.clone();
                                doc.insert("at".into(), Value::from(now));
                                doc.insert("seen".into(), Value::from(now));
                                if let Ok(bytes) = serde_json::to_vec(&Value::Object(doc)) {
                                    let _ = kuvo_store::files::write_secret(&run.join("peers").join(&key_str).join(file), &bytes);
                                    updated = true;
                                }
                            }
                        }
                        if updated {
                            ready.insert(*key);
                            seq = seq.wrapping_add(1); let _ = notify.send(seq);
                        }
                    }
                    RelayEvent::Connected(_) | RelayEvent::Disconnected(_) => {
                        sent.clear(); requested.clear(); ready.clear();
                    },
                    RelayEvent::Gone { address, replaced: true } => {
                        if let Some(member) = peers.view().roster.at_address(*address) {
                            sent.retain(|(key, _), _| *key != member.key);
                            requested.remove(&member.key); ready.remove(&member.key);
                        }
                    }
                    RelayEvent::Unreachable(address) | RelayEvent::Gone { address, replaced: false } => {
                        if let Some(member) = peers.view().roster.at_address(*address) {
                            sent.retain(|(key, _), _| *key != member.key);
                            ready.remove(&member.key);
                            // The peer view will clear the request when it learns the peer is away.
                        }
                    }
                    _ => {}
                },
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => { sent.clear(); requested.clear(); ready.clear(); cached = None; }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            },
            _ = recv_broadcast(&mut art_rx) => cached = None,
            _ = recv_broadcast(&mut lib_rx) => cached = None,
            _ = source_watches.changed() => {
                if let LibraryHandle::Ready(service) = &library { service.clear_bot_descriptions(); }
                cached = None;
                seq = seq.wrapping_add(1); let _ = notify.send(seq);
            },
            result = peers.changed() => {
                if result.is_err() { break; }
                seq = seq.wrapping_add(1); let _ = notify.send(seq);
            },
            _ = legacy_due(next_legacy) => {},
        }
    }
}

async fn recv_broadcast<T: Clone>(rx: &mut Option<tokio::sync::broadcast::Receiver<T>>) {
    match rx {
        Some(r) => if matches!(r.recv().await, Err(tokio::sync::broadcast::error::RecvError::Closed)) { *rx = None; },
        None => std::future::pending().await,
    }
}

struct SentCopy {
    digest: [u8; 32],
    at: tokio::time::Instant,
}

type Content = Vec<(&'static str, Value, [u8; 32])>;

async fn legacy_due(at: Option<tokio::time::Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// Build a snapshot only after a real store/source change. Each stream has its own digest and
/// recipient ledger: an unrelated artifact change doesn't send Library data again. A legacy
/// recipient expects a periodic copy for its standing; only while such a peer is connected is
/// there a timer at all.
async fn push_snapshot(
    peers: &Reader, relay: &RelayHandle, artifacts: &ArtifactHandle, library: &LibraryHandle,
    cached: &mut Option<Content>, sent: &mut HashMap<([u8; 32], &'static str), SentCopy>,
    requested: &mut HashSet<[u8; 32]>, ready: &mut HashSet<[u8; 32]>,
    sources: &mut FileSet,
) -> Option<tokio::time::Instant> {
    let view = peers.view();
    let recipients = view.roster.members.iter().filter(|member| member.address.is_some() && !view.away(&member.key)).collect::<Vec<_>>();
    sent.retain(|(key, _), _| recipients.iter().any(|member| member.key == *key));
    requested.retain(|key| recipients.iter().any(|member| member.key == *key));
    // Relay mail and the peer projection are independently scheduled. A ready
    // request may precede the new roster or Connected view; keep it until a
    // real disconnect/unreachable rather than dropping it on a stale view.
    if !view.relay_connected || recipients.is_empty() {
        *cached = None;
        sources.clear();
        return None;
    }
    for member in &recipients {
        if view.advertises(&member.key, CHANGES_FEATURE) && !requested.contains(&member.key) {
            let payload = Map::from_iter([("ask".into(), json!("library-copy"))]);
            if relay.sender.mail(Host { address: member.address.expect("filtered"), key: member.key }, Envelope(payload)).await.is_ok() {
                requested.insert(member.key);
            }
        }
    }
    let recipients = recipients.into_iter().filter(|member| {
        if view.advertises(&member.key, CHANGES_FEATURE) {
            ready.contains(&member.key)
        } else {
            // An unknown peer may be modern; wait for its first announcement
            // instead of treating startup as an old client and mailing too early.
            view.has_announcement(&member.key) || ready.contains(&member.key)
        }
    }).collect::<Vec<_>>();
    if recipients.is_empty() { return None; }
    let content = cached.get_or_insert_with(|| {
        let mut content = Vec::new();
        let mut add = |field, value: Value| {
            if let Ok(bytes) = serde_json::to_vec(&value) {
                content.push((field, value, Sha256::digest(bytes).into()));
            }
        };
        if let LibraryHandle::Ready(service) = library {
            let copy = service.peer_copy_json();
            let paths = copy["items"].as_array().into_iter().flatten().flat_map(|item| {
                let file = item["path"].as_str().map(PathBuf::from);
                let bot = item["bot"].as_str().map(|bot| service.bots_folder().join(bot).join("bot.json"));
                file.into_iter().chain(bot)
            }).collect();
            sources.set_files(paths);
            add("libraryCopy", copy);
        }
        if let ArtifactHandle::Ready(store) = artifacts {
            add("outputCopy", json!({ "artifacts": local_output_copy(store, None) }));
        }
        content
    });
    let now = tokio::time::Instant::now();
    let mut next = None;
    for member in recipients {
        let legacy = !view.advertises(&member.key, CHANGES_FEATURE);
        for (field, value, digest) in content.iter() {
            let key = (member.key, *field);
            let previous = sent.get(&key);
            let due = previous.map(|copy| copy.at + Duration::from_secs(60));
            if previous.is_none_or(|copy| copy.digest != *digest) || legacy && due.is_some_and(|at| at <= now) {
                let mut doc = value.clone();
                doc["at"] = json!(peers.now());
                let payload = Map::from_iter([((*field).into(), doc)]);
                if serde_json::to_vec(&payload).map_or(true, |bytes| bytes.len() > kuvo_wire::mail::MAX_PLAINTEXT) {
                    continue;
                }
                if relay.sender.mail(Host { address: member.address.expect("filtered"), key: member.key }, Envelope(payload)).await.is_ok() {
                    sent.insert(key, SentCopy { digest: *digest, at: now });
                }
            }
            if legacy {
                let at = sent.get(&key).map_or(now, |copy| copy.at) + Duration::from_secs(60);
                next = Some(next.map_or(at, |prior: tokio::time::Instant| prior.min(at)));
            }
        }
    }
    next
}

#[derive(Clone, serde::Deserialize)]
struct EngineRecord { port: u16, password: String }

struct LoopbackEngineSource { run: PathBuf }

impl LoopbackEngineSource {
    fn read_record(&self) -> Option<EngineRecord> {
        serde_json::from_str(&fs::read_to_string(self.run.join("engine.json")).ok()?).ok()
    }
}

impl SessionSource for LoopbackEngineSource {
    fn name(&self) -> &'static str { "engine" }
    fn agent(&self, _id: &str) -> Option<String> { Some("engine".to_owned()) }

    fn session<'a>(&'a self, id: &'a str) -> SessionFuture<'a> {
        Box::pin(async move {
            let record = self.read_record()?;
            let encoded = percent_encoding::utf8_percent_encode(id, percent_encoding::NON_ALPHANUMERIC);
            let body = http_get_json(&record, &format!("/api/session/{encoded}")).await?;
            let data = body.get("data").unwrap_or(&body);
            Some(SessionDetails {
                parent_id: data.get("parentID").and_then(Value::as_str).map(str::to_owned),
                directory: data.pointer("/location/directory").and_then(Value::as_str).map(str::to_owned),
                metadata: data.get("metadata").cloned(),
            })
        })
    }
}

async fn connect_engine(record: &EngineRecord, path: &str, extra: &str) -> Option<TcpStream> {
    let mut stream = tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(("127.0.0.1", record.port))).await.ok()?.ok()?;
    let auth = STANDARD.encode(format!("opencode:{}", record.password));
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Basic {auth}\r\n{extra}\r\n", record.port);
    stream.write_all(req.as_bytes()).await.ok()?;
    Some(stream)
}

async fn http_get_json(record: &EngineRecord, path: &str) -> Option<Value> {
    let mut stream = connect_engine(record, path, "Connection: close\r\n").await?;
    let mut buf = Vec::new();
    tokio::time::timeout(Duration::from_secs(3), stream.read_to_end(&mut buf)).await.ok()?.ok()?;
    let split = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&buf[..split]);
    (head.starts_with("HTTP/1.1 200") || head.starts_with("HTTP/1.0 200"))
        .then(|| serde_json::from_slice(&buf[split + 4..]).ok())
        .flatten()
}

async fn follow_engine_events(run: PathBuf, mut events: tokio::sync::broadcast::Receiver<Arc<kuvo_engine::EngineEvent>>, recorder: Recorder) {
    let source = LoopbackEngineSource { run };
    loop {
        let event = match events.recv().await {
            Ok(event) => event,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        };
        recorder.heard(&source, Event {
            event_type: event.kind.clone(),
            data: event.data.clone(),
            directory: event.location.get("directory").and_then(Value::as_str).map(str::to_owned),
        }).await;
    }
}
