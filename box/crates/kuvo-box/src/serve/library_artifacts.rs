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
use kuvo_os::watch::WatchHub;
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
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    sync::watch,
    task::JoinHandle,
};

const LIBRARY_FEATURE: &str = "library";
const MERGED_LIBRARY_FEATURE: &str = "line.merged-library";
const MERGED_OUTPUTS_FEATURE: &str = "line.merged-outputs";
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
        routes: &mut Registry,
    ) -> Self {
        let artifacts_root = home.join("artifacts");
        let library_root = home.join("library");
        let bots_dir = home.join("bots");
        let sessions_dir = home.join("agents").join("sessions");
        let run_dir = home.join("run");

        let prev_checkpoints = load_citation_checkpoints(&artifacts_root);
        let (shown, checkpoints) = citation_update(&sessions_dir, &prev_checkpoints);
        let store_res = ArtifactStore::open(
            &artifacts_root,
            StoreOptions {
                bots: Some(bots_dir.clone()),
                shown,
                ..Default::default()
            },
        );

        let mut tasks = Vec::new();
        let (artifact_handle, library_handle, folders) = match store_res {
            Ok(store) => {
                save_citation_checkpoints(&artifacts_root, &checkpoints);
                let name_for_lib = computer_name.clone();
                let lib_res = LibraryService::open(
                    &library_root,
                    LibraryOptions {
                        bots: bots_dir.clone(),
                        sessions: sessions_dir,
                        artifacts: Some(store.clone()),
                        computer: Arc::new(move || name_for_lib.clone()),
                        platform: ThumbnailPlatform::current(),
                        clock: None,
                    },
                );
                let (library_handle, handed_hook) = match lib_res {
                    Ok(service) => {
                        service.backfill();
                        let svc = service.clone();
                        let hook: kuvo_services::artifacts::recorder::HandedHook =
                            Arc::new(move |handover| svc.handed(handover));
                        (LibraryHandle::Ready(service), Some(hook))
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
                    RecorderOptions {
                        handed: handed_hook,
                        ..Default::default()
                    },
                );
                if let Some(folders) = &folders {
                    let hub = WatchHub::default();
                    folders.start(Some(&hub)).await;
                }
                tasks.push(tokio::spawn(follow_engine_events(
                    run_dir.clone(),
                    recorder,
                )));
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
                run_dir.clone(),
                peers.clone(),
                relay,
                artifact_handle.clone(),
                library_handle.clone(),
                peer_tx,
            )));
        }

        let peer_adapter = Arc::new(PeerAdapter {
            run: run_dir,
            peers,
            reads,
            changes: peer_rx,
        });

        kuvo_services::artifacts::register(
            routes,
            artifact_handle,
            Some(peer_adapter.clone()),
        );
        kuvo_services::library::register(routes, library_handle.clone(), Some(peer_adapter));

        Self {
            library: library_handle,
            folders,
            tasks,
        }
    }

    pub(super) async fn answer_library_read(&self, read: Read) -> Answer {
        let LibraryHandle::Ready(service) = &self.library else {
            return Answer {
                status: 503,
                value: json!({
                    "message": "This computer’s Library can’t be read right now. Try again in a moment."
                }),
            };
        };
        let (status, value) =
            library_for_peer(service, "GET", &read.item, read.body.as_ref()).await;
        Answer { status, value }
    }

    pub(super) async fn stop(self) {
        if let Some(folders) = self.folders {
            folders.stop();
        }
        for task in self.tasks {
            task.abort();
            let _ = task.await;
        }
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
        if !valid_peer_key(key) {
            return None;
        }
        let raw = fs::read_to_string(self.peer_copy_path(key, file)).ok()?;
        let val: Value = serde_json::from_str(&raw).ok()?;
        let seen = val.get("seen").and_then(Value::as_i64)?;
        Some((seen, val))
    }

    fn peer_sessions_children(&self) -> HashMap<String, Vec<String>> {
        let mut parents: HashMap<String, HashSet<String>> = HashMap::new();
        let view = self.peers.view();
        if let Some(local) = &view.local.list {
            for (id, row) in &local.sessions {
                if let Some(parent) = &row.facts.parent {
                    parents.entry(parent.clone()).or_default().insert(id.clone());
                }
                if let Some(line) = &row.facts.line {
                    for p in line {
                        parents.entry(p.clone()).or_default().insert(id.clone());
                    }
                }
            }
        }
        for member in &view.roster.members {
            let key = encode(&member.key);
            let path = self.peer_copy_path(&key, "lists-v1.json");
            let Ok(raw) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(val) = serde_json::from_str::<Value>(&raw) else {
                continue;
            };
            let Some(sessions) = val.get("sessions").and_then(Value::as_array) else {
                continue;
            };
            for entry in sessions {
                let Some(id) = entry.get("id").and_then(Value::as_str) else {
                    continue;
                };
                let Some(facts) = entry.get("facts") else {
                    continue;
                };
                if let Some(parent) = facts.get("parent").and_then(Value::as_str) {
                    parents
                        .entry(parent.to_owned())
                        .or_default()
                        .insert(id.to_owned());
                }
                if let Some(line) = facts.get("line").and_then(Value::as_array) {
                    for p in line.iter().filter_map(Value::as_str) {
                        parents
                            .entry(p.to_owned())
                            .or_default()
                            .insert(id.to_owned());
                    }
                }
            }
        }
        parents
            .into_iter()
            .map(|(k, set)| (k, set.into_iter().collect()))
            .collect()
    }
}

impl OutputsPeers for PeerAdapter {
    fn snapshot(&self) -> OutputsFuture<'_> {
        Box::pin(async move {
            let view = self.peers.view();
            let now = self.peers.now();
            let local = view.roster.local();
            let children = self.peer_sessions_children();
            let mut copies = Vec::new();
            let mut have_copy = HashSet::new();
            for member in &view.roster.members {
                let key_str = encode(&member.key);
                let Some((seen, val)) = self.read_peer_snapshot(&key_str, OUTPUTS_FILE) else {
                    continue;
                };
                let Ok(artifacts) = serde_json::from_value::<Vec<PeerOutputRow>>(
                    val.get("artifacts").cloned().unwrap_or(Value::Array(vec![])),
                ) else {
                    continue;
                };
                have_copy.insert(member.key);
                let standing = view.standing(member.key, now);
                let away = standing.away();
                let last_seen = standing.last_seen().or(Some(seen));
                copies.push(OutputPeerCopy {
                    computer_id: key_str,
                    computer_name: member.name.clone(),
                    away,
                    last_seen,
                    artifacts,
                });
            }
            let pending = view
                .pending(MERGED_OUTPUTS_FEATURE, |k| have_copy.contains(k))
                .into_iter()
                .map(|c| PendingComputer {
                    id: c.id,
                    name: c.name,
                })
                .collect();
            Some(OutputsSnapshot {
                local_id: local.id,
                local_name: local.name,
                children,
                peers: copies,
                pending,
            })
        })
    }

    fn changes(&self) -> watch::Receiver<u64> {
        self.changes.clone()
    }
}

impl LibraryPeers for PeerAdapter {
    fn local(&self) -> PeerComputer {
        let local = self.peers.view().roster.local();
        PeerComputer {
            key: local.id,
            name: local.name,
            here: true,
        }
    }

    fn peers_dir(&self) -> PathBuf {
        self.run.join("library").join("peers")
    }

    fn kept_peers(&self, query: &str) -> Vec<Asked> {
        let view = self.peers.view();
        let now = self.peers.now();
        let dir = self.peers_dir();
        let mut out = Vec::new();
        for member in &view.roster.members {
            let key_str = encode(&member.key);
            let computer = PeerComputer {
                key: key_str.clone(),
                name: member.name.clone(),
                here: false,
            };
            if !view.advertises(&member.key, LIBRARY_FEATURE) {
                out.push(Asked {
                    computer,
                    state: PartState::Older(older_message(&member.name)),
                    part: None,
                    seen: None,
                });
                continue;
            }
            let kept = kept_part(&dir, &key_str, query);
            let away = !view.relay_connected || view.standing(member.key, now).away();
            if away {
                let asked = match kept {
                    Some(k) => Asked {
                        computer,
                        state: PartState::Away(last_seen_message(&member.name)),
                        part: Some(k.part),
                        seen: Some(k.seen),
                    },
                    None => Asked {
                        computer,
                        state: PartState::Away(away_hidden_message(&member.name)),
                        part: None,
                        seen: None,
                    },
                };
                out.push(asked);
            } else if let Some(k) = kept {
                out.push(Asked {
                    computer,
                    state: PartState::There,
                    part: Some(k.part),
                    seen: Some(k.seen),
                });
            }
        }
        out
    }

    fn ask_peers<'a>(&'a self, query: &'a str) -> kuvo_services::library::peers::PeerFuture<'a, Vec<Asked>> {
        Box::pin(async move {
            let view = self.peers.view();
            let now = self.peers.now();
            let dir = self.peers_dir();
            let mut out = Vec::new();
            for member in &view.roster.members {
                let key_str = encode(&member.key);
                let computer = PeerComputer {
                    key: key_str.clone(),
                    name: member.name.clone(),
                    here: false,
                };
                if !view.advertises(&member.key, LIBRARY_FEATURE) {
                    out.push(Asked {
                        computer,
                        state: PartState::Older(older_message(&member.name)),
                        part: None,
                        seen: None,
                    });
                    continue;
                }
                let as_kept = |state_away: bool, msg: String| -> Asked {
                    match kept_part(&dir, &key_str, query) {
                        Some(k) => Asked {
                            computer: computer.clone(),
                            state: if state_away {
                                PartState::Away(last_seen_message(&member.name))
                            } else {
                                PartState::Failed(last_seen_message(&member.name))
                            },
                            part: Some(k.part),
                            seen: Some(k.seen),
                        },
                        None => Asked {
                            computer: computer.clone(),
                            state: if state_away {
                                PartState::Away(msg)
                            } else {
                                PartState::Failed(msg)
                            },
                            part: None,
                            seen: None,
                        },
                    }
                };
                if !view.relay_connected || view.standing(member.key, now).away() {
                    out.push(as_kept(true, away_hidden_message(&member.name)));
                    continue;
                }
                let read = Read {
                    area: Some("library".to_owned()),
                    item: "list".to_owned(),
                    kind: ReadKind::Record,
                    body: Some([("query".to_owned(), Value::String(query.to_owned()))].into_iter().collect()),
                };
                match self.reads.request(member.key, read).await {
                    Ok(answer) if answer.status == 200 && is_part(&answer.value) => {
                        keep_part(&dir, &key_str, query, answer.value.clone(), now);
                        out.push(Asked {
                            computer,
                            state: PartState::There,
                            part: Some(answer.value),
                            seen: None,
                        });
                    }
                    Ok(answer) if answer.status == 409 => {
                        let msg = answer
                            .value
                            .get("message")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                            .unwrap_or_else(|| older_message(&member.name));
                        out.push(Asked {
                            computer,
                            state: PartState::Older(msg),
                            part: None,
                            seen: None,
                        });
                    }
                    Ok(_) => {
                        out.push(as_kept(
                            false,
                            format!(
                                "{}’s Library couldn’t be read. Try again in a moment.",
                                member.name
                            ),
                        ));
                    }
                    Err(err) => {
                        out.push(as_kept(true, err.to_string()));
                    }
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
            let Some(key) = decode_exact::<32>(computer_key) else {
                return Err((404, "That computer isn’t one of yours any more.".to_owned()));
            };
            let view = self.peers.view();
            let Some(member) = view.roster.member(&key) else {
                return Err((404, "That computer isn’t one of yours any more.".to_owned()));
            };
            if !view.advertises(&key, LIBRARY_FEATURE) {
                return Err((409, older_message(&member.name)));
            }
            if method != "GET" {
                return Err((
                    503,
                    format!("{} isn’t answering now. Try again in a moment.", member.name),
                ));
            }
            let read = Read {
                area: Some("library".to_owned()),
                item: item.to_owned(),
                kind: ReadKind::Record,
                body,
            };
            match self.reads.request(key, read).await {
                Ok(answer) => Ok((answer.status, answer.value)),
                Err(err) => Err((503, err.to_string())),
            }
        })
    }

    fn merged_snapshot<'a>(
        &'a self,
        filter: &'a Filter,
        query: &'a str,
    ) -> kuvo_services::library::peers::PeerFuture<'a, (Vec<Asked>, Vec<PendingComputer>)> {
        Box::pin(async move {
            let view = self.peers.view();
            let now = self.peers.now();
            let dir = self.peers_dir();
            let mut known = Vec::new();
            let mut known_keys = HashSet::new();
            for member in &view.roster.members {
                let key_str = encode(&member.key);
                let computer = PeerComputer {
                    key: key_str.clone(),
                    name: member.name.clone(),
                    here: false,
                };
                let copy = self.read_peer_snapshot(&key_str, LIBRARY_FILE);
                let legacy = if copy.is_none() {
                    kept_part(&dir, &key_str, query)
                } else {
                    None
                };
                let seen = copy
                    .as_ref()
                    .map(|(s, _)| *s)
                    .or_else(|| legacy.as_ref().map(|k| k.seen));
                let standing = view.standing(member.key, now);
                let aged = seen.is_some_and(|s| now - s > 180_000);
                let away = standing.away() || (seen.is_some() && (!view.relay_connected || aged));
                if copy.is_none() && legacy.is_none() {
                    if away {
                        known_keys.insert(member.key);
                        known.push(Asked {
                            computer,
                            state: PartState::Away(away_hidden_message(&member.name)),
                            part: None,
                            seen: None,
                        });
                    } else if !view.advertises(&member.key, MERGED_LIBRARY_FEATURE)
                        && view.advertises(&member.key, "info")
                    {
                        known_keys.insert(member.key);
                        known.push(Asked {
                            computer,
                            state: PartState::Older(older_message(&member.name)),
                            part: None,
                            seen: None,
                        });
                    }
                    continue;
                }
                let part = match (&copy, &legacy) {
                    (Some((_, val)), _) => filter_peer_copy(val, filter, &member.name, away),
                    (None, Some(leg)) => Some(leg.part.clone()),
                    (None, None) => None,
                };
                let Some(part) = part else {
                    continue;
                };
                known_keys.insert(member.key);
                if away {
                    known.push(Asked {
                        computer,
                        state: PartState::Away(last_seen_message(&member.name)),
                        part: Some(part),
                        seen,
                    });
                } else {
                    known.push(Asked {
                        computer,
                        state: PartState::There,
                        part: Some(part),
                        seen: None,
                    });
                }
            }
            let pending = view
                .pending(MERGED_LIBRARY_FEATURE, |k| known_keys.contains(k))
                .into_iter()
                .map(|c| PendingComputer {
                    id: c.id,
                    name: c.name,
                })
                .collect();
            (known, pending)
        })
    }

    fn changes(&self) -> watch::Receiver<u64> {
        self.changes.clone()
    }
}

async fn sync_peer_snapshots(
    run: PathBuf,
    peers: Reader,
    relay: RelayHandle,
    artifacts: ArtifactHandle,
    library: LibraryHandle,
    notify: watch::Sender<u64>,
) {
    let Some(mut events) = relay.subscribe() else {
        return;
    };
    let mut seq = 0u64;
    let mut art_rx = match &artifacts {
        ArtifactHandle::Ready(store) => Some(store.subscribe()),
        _ => None,
    };
    let mut lib_rx = match &library {
        LibraryHandle::Ready(service) => Some(service.store().subscribe()),
        _ => None,
    };
    let mut tick = tokio::time::interval(Duration::from_secs(60));
    loop {
        tokio::select! {
            res = events.recv() => {
                match res {
                    Ok(event) => {
                        if let RelayEvent::Mail { key, envelope, .. } = event.as_ref() {
                            let payload = &envelope.0;
                            let key_str = encode(key);
                            let now = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .map_or(0, |d| d.as_millis() as i64);
                            let mut updated = false;
                            if let Some(lib_copy) = payload.get("libraryCopy").and_then(Value::as_object) {
                                let mut doc = lib_copy.clone();
                                doc.insert("at".into(), Value::from(now));
                                doc.insert("seen".into(), Value::from(now));
                                if let Ok(bytes) = serde_json::to_vec(&Value::Object(doc)) {
                                    let _ = kuvo_store::files::write_secret(
                                        &run.join("peers").join(&key_str).join(LIBRARY_FILE),
                                        &bytes,
                                    );
                                    updated = true;
                                }
                            }
                            if let Some(out_copy) = payload.get("outputCopy").and_then(Value::as_object) {
                                let mut doc = out_copy.clone();
                                doc.insert("at".into(), Value::from(now));
                                doc.insert("seen".into(), Value::from(now));
                                if let Ok(bytes) = serde_json::to_vec(&Value::Object(doc)) {
                                    let _ = kuvo_store::files::write_secret(
                                        &run.join("peers").join(&key_str).join(OUTPUTS_FILE),
                                        &bytes,
                                    );
                                    updated = true;
                                }
                            }
                            if updated {
                                seq = seq.wrapping_add(1);
                                let _ = notify.send(seq);
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            _ = recv_broadcast(&mut art_rx) => {
                push_snapshot(&peers, &relay, &artifacts, &library).await;
            }
            _ = recv_broadcast(&mut lib_rx) => {
                push_snapshot(&peers, &relay, &artifacts, &library).await;
            }
            _ = tick.tick() => {
                push_snapshot(&peers, &relay, &artifacts, &library).await;
            }
        }
    }
}

async fn recv_broadcast<T: Clone>(rx: &mut Option<tokio::sync::broadcast::Receiver<T>>) {
    match rx {
        Some(r) => match r.recv().await {
            Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            Err(tokio::sync::broadcast::error::RecvError::Closed) => *rx = None,
        },
        None => std::future::pending().await,
    }
}

async fn push_snapshot(
    peers: &Reader,
    relay: &RelayHandle,
    artifacts: &ArtifactHandle,
    library: &LibraryHandle,
) {
    let view = peers.view();
    if !view.relay_connected || view.roster.members.is_empty() {
        return;
    }
    let mut payload = Map::new();
    if let LibraryHandle::Ready(service) = library {
        payload.insert("libraryCopy".into(), service.peer_copy_json());
    }
    if let ArtifactHandle::Ready(store) = artifacts {
        payload.insert(
            "outputCopy".into(),
            json!({ "artifacts": local_output_copy(store, None) }),
        );
    }
    if payload.is_empty() {
        return;
    }
    let Ok(bytes) = serde_json::to_vec(&Value::Object(payload.clone())) else {
        return;
    };
    if bytes.len() > kuvo_wire::mail::MAX_PLAINTEXT {
        return;
    }
    for member in &view.roster.members {
        let Some(address) = member.address else {
            continue;
        };
        let host = Host {
            address,
            key: member.key,
        };
        let env = Envelope(payload.clone());
        let _ = relay.sender.mail(host, env).await;
    }
}

#[derive(Clone, serde::Deserialize)]
struct EngineRecord {
    port: u16,
    password: String,
}

struct LoopbackEngineSource {
    run: PathBuf,
}

impl LoopbackEngineSource {
    fn read_record(&self) -> Option<EngineRecord> {
        let raw = fs::read_to_string(self.run.join("engine.json")).ok()?;
        serde_json::from_str(&raw).ok()
    }
}

impl SessionSource for LoopbackEngineSource {
    fn name(&self) -> &'static str {
        "engine"
    }

    fn session<'a>(&'a self, id: &'a str) -> SessionFuture<'a> {
        Box::pin(async move {
            let record = self.read_record()?;
            let encoded = percent_encoding::utf8_percent_encode(
                id,
                percent_encoding::NON_ALPHANUMERIC,
            );
            let body = http_get_json(record.port, &record.password, &format!("/api/session/{encoded}")).await?;
            let data = body.get("data").unwrap_or(&body);
            Some(SessionDetails {
                parent_id: data
                    .get("parentID")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                directory: data
                    .pointer("/location/directory")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                metadata: data.get("metadata").cloned(),
            })
        })
    }

    fn agent(&self, _id: &str) -> Option<String> {
        Some("engine".to_owned())
    }
}

async fn http_get_json(port: u16, password: &str, path: &str) -> Option<Value> {
    let mut stream = tokio::time::timeout(
        Duration::from_secs(2),
        TcpStream::connect(("127.0.0.1", port)),
    )
    .await
    .ok()?
    .ok()?;
    let auth = STANDARD.encode(format!("opencode:{password}"));
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Basic {auth}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).await.ok()?;
    let mut buf = Vec::new();
    tokio::time::timeout(Duration::from_secs(3), stream.read_to_end(&mut buf))
        .await
        .ok()?
        .ok()?;
    let split = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&buf[..split]);
    if !head.starts_with("HTTP/1.1 200") && !head.starts_with("HTTP/1.0 200") {
        return None;
    }
    let body = &buf[split + 4..];
    serde_json::from_slice(body).ok()
}

async fn follow_engine_events(run: PathBuf, recorder: Recorder) {
    let source = LoopbackEngineSource { run: run.clone() };
    loop {
        let Some(record) = source.read_record() else {
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        };
        let _ = read_sse_stream(&record, &source, &recorder).await;
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn read_sse_stream(
    record: &EngineRecord,
    source: &LoopbackEngineSource,
    recorder: &Recorder,
) -> Option<()> {
    let mut stream = tokio::time::timeout(
        Duration::from_secs(2),
        TcpStream::connect(("127.0.0.1", record.port)),
    )
    .await
    .ok()?
    .ok()?;
    let auth = STANDARD.encode(format!("opencode:{}", record.password));
    let req = format!(
        "GET /api/event?passive=1 HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Basic {auth}\r\nAccept: text/event-stream\r\n\r\n",
        record.port
    );
    stream.write_all(req.as_bytes()).await.ok()?;
    let mut reader = BufReader::new(stream);
    let mut status = String::new();
    reader.read_line(&mut status).await.ok()?;
    if !status.contains(" 200") {
        return None;
    }
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await.ok()?;
        if n == 0 || line == "\r\n" || line == "\n" {
            break;
        }
    }
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await.ok()?;
        if n == 0 {
            break;
        }
        let trimmed = line.trim();
        let Some(payload) = trimmed.strip_prefix("data:") else {
            continue;
        };
        let Ok(val) = serde_json::from_str::<Value>(payload.trim()) else {
            continue;
        };
        let Some(event_type) = val.get("type").and_then(Value::as_str) else {
            continue;
        };
        let event = Event {
            event_type: event_type.to_owned(),
            data: val.get("data").cloned().unwrap_or(Value::Object(Map::new())),
            directory: val
                .pointer("/location/directory")
                .and_then(Value::as_str)
                .map(str::to_owned),
        };
        recorder.heard(source, event).await;
    }
    Some(())
}
