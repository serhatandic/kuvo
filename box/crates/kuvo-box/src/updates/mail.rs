//! Target packs over the existing authenticated peer release mail.

use std::{
    io,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use kuvo_keeper::{
    fleet::{Candidate, Fetcher, HeldPack, PACK_PART},
    manifest::ReleaseRef,
    staging::MAX_PACK,
};
use kuvo_peers::{
    directory::UpdateFacts,
    relay::{RelayEvent, RelayHandle},
    setup_lists::{Reader, system_clock},
};
use kuvo_store::home::Home;
use kuvo_wire::{keys::Host, mail::Envelope};
use serde_json::{Number, Value, json};
use tokio::{
    sync::{Semaphore, broadcast, mpsc},
    task::{JoinHandle, JoinSet},
    time::timeout,
};

use super::Check;

const WINDOW: usize = 16;
const MIRROR_REQUESTS: usize = 4;
const QUIET: Duration = Duration::from_secs(60);

#[derive(Clone, Default)]
pub(super) struct PullProgress(Arc<Mutex<Option<f64>>>);

impl PullProgress {
    fn note(&self, received: usize, parts: usize) {
        let mut fill = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let next = received as f64 / parts as f64;
        *fill = Some(fill.map_or(next, |previous| previous.max(next)));
    }

    pub(super) fn fraction(&self) -> Option<Number> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .and_then(Number::from_f64)
    }
}

#[derive(Clone)]
pub(super) struct Mail {
    relay: RelayHandle,
    reader: Reader,
    key: String,
}

impl Mail {
    pub(super) fn new(relay: RelayHandle, reader: Reader, key: String) -> Self {
        Self { relay, reader, key }
    }

    pub(super) fn serve(&self, home: Home, checks: mpsc::Sender<Check>) -> Option<JoinHandle<()>> {
        let mut events = self.relay.subscribe()?;
        let mail = self.clone();
        Some(tokio::spawn(async move {
            let mut held = None;
            let mut mirrors = JoinSet::new();
            let gate = Arc::new(Semaphore::new(1));
            let fetcher = Fetcher::default();
            loop {
                let incoming = tokio::select! {
                    _ = mirrors.join_next(), if !mirrors.is_empty() => continue,
                    event = events.recv() => event,
                };
                let event = match incoming {
                    Ok(event) => event,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return,
                };
                let RelayEvent::Mail {
                    from,
                    key,
                    envelope,
                } = &*event
                else {
                    continue;
                };
                let view = mail.reader.view();
                let Some(peer) = view.roster.member(key).and_then(|peer| peer.host()) else {
                    continue;
                };
                if peer.address != *from {
                    continue;
                }
                if let Some(value) = envelope.0.get("release") {
                    if value["check"] == true {
                        let _ = checks.try_send(Check::Peer);
                        continue;
                    }
                    if let Some(retry) = value.get("retry")
                        && let Ok(release) = serde_json::from_value(retry.clone())
                    {
                        if view
                            .peer_updates(key, mail.reader.now())
                            .1
                            .release_target
                            .is_some()
                        {
                            let _ = checks.try_send(Check::PeerRetry(release));
                        }
                        continue;
                    }
                    match answer(value, &home, &mail.key, &mut held) {
                        Ok(Some(reply)) => {
                            if reply["none"] == true
                                && mirrors.len() < MIRROR_REQUESTS
                                && let (Some(version), Some(hash)) =
                                    (value["want"].as_str(), value["sha256"].as_str())
                            {
                                let (mail, home, gate, fetcher) =
                                    (mail.clone(), home.clone(), gate.clone(), fetcher.clone());
                                let release = ReleaseRef {
                                    version: version.into(),
                                    sha256: hash.into(),
                                };
                                mirrors.spawn(async move {
                                    let mirrored = timeout(kuvo_keeper::fleet::peer_transfer_limit(), async {
                                        let _permit = gate.acquire_owned().await.ok()?;
                                        HeldPack::mirror(&home, &mail.key, &release.version, &release.sha256, &fetcher).await.ok()?
                                    }).await.ok().flatten();
                                    let reply = match mirrored {
                                        Some(pack) => json!({"version":release.version,"sha256":release.sha256,"boxes":pack.candidate.envelope(),"size":pack.size}),
                                        None => reply,
                                    };
                                    if mail.reader.view().roster.member(&peer.key).and_then(|member| member.host()).as_ref() == Some(&peer) {
                                        let _ = mail.send(&peer, reply).await;
                                    }
                                });
                            } else {
                                let _ = mail.send(&peer, reply).await;
                            }
                        }
                        Ok(None) => {}
                        Err(error) => eprintln!("Answering a release request: {error}"),
                    }
                }
            }
        }))
    }

    pub(super) async fn request_check(&self, peer: &Host) -> io::Result<()> {
        self.send(peer, json!({"check":true})).await
    }

    pub(super) async fn request_retry(
        &self,
        peer: &Host,
        release: &kuvo_peers::directory::PreparedRelease,
    ) -> io::Result<()> {
        self.send(peer, json!({"retry":release})).await
    }

    pub(super) async fn pull(
        &self,
        release: &ReleaseRef,
        progress: &PullProgress,
    ) -> io::Result<Vec<u8>> {
        let mut events = self.relay.subscribe().ok_or_else(unavailable)?;
        let view = self.reader.view();
        let now = self.reader.now();
        let mut peers: Vec<_> = view
            .roster
            .members
            .iter()
            .filter_map(|member| {
                let (away, facts) = view.peer_updates(&member.key, now);
                if away {
                    return None;
                }
                let installing = facts.staged.as_ref().is_some_and(|stage| {
                    stage.version == release.version && stage.sha256 == release.sha256
                });
                let installed = facts.kuvo.as_deref() == Some(&release.version)
                    && facts.installed_sha256.as_deref() == Some(&release.sha256);
                let exact = installing || installed;
                if exact || self.can_mirror(&facts, release) {
                    Some((member.host()?, !exact))
                } else {
                    None
                }
            })
            .collect();
        peers.sort_by_key(|(_, mirror)| *mirror);
        for (peer, mirror) in peers {
            if let Ok(Ok(pack)) = timeout(
                kuvo_keeper::fleet::peer_transfer_limit(),
                self.pull_from(&mut events, &peer, release, progress, mirror),
            )
            .await
            {
                return Ok(pack);
            }
        }
        Err(unavailable())
    }

    fn can_mirror(&self, facts: &UpdateFacts, release: &ReleaseRef) -> bool {
        let Some(target) = &facts.release_target else {
            return false;
        };
        for (envelope, version, digest) in [
            (
                facts.running_manifest.as_ref(),
                facts.kuvo.as_ref(),
                facts.installed_sha256.as_ref(),
            ),
            (
                facts.release_manifest.as_ref(),
                facts.staged.as_ref().map(|stage| &stage.version),
                facts.staged.as_ref().map(|stage| &stage.sha256),
            ),
        ] {
            if let (Some(envelope), Some(version), Some(digest)) = (envelope, version, digest)
                && version == &release.version
                && let Ok(candidate) = Candidate::verify(envelope.clone(), &self.key)
                && candidate.local().ok().as_ref() == Some(release)
                && candidate.matches_pack(target, digest)
            {
                return true;
            }
        }
        false
    }

    async fn pull_from(
        &self,
        events: &mut broadcast::Receiver<std::sync::Arc<RelayEvent>>,
        peer: &Host,
        release: &ReleaseRef,
        progress: &PullProgress,
        mirror: bool,
    ) -> io::Result<Vec<u8>> {
        self.send(
            peer,
            json!({"want":release.version, "sha256": release.sha256}),
        )
        .await?;
        let header = timeout(
            if mirror {
                kuvo_keeper::fleet::peer_transfer_limit()
            } else {
                QUIET
            },
            reply(events, peer, release, |value| {
                value["none"] == true || value["boxes"].is_string()
            }),
        )
        .await
        .map_err(io::Error::other)??;
        let size = header["size"]
            .as_u64()
            .filter(|size| *size > 0 && *size <= MAX_PACK)
            .ok_or_else(unavailable)? as usize;
        let envelope = header["boxes"].as_str().ok_or_else(unavailable)?;
        if Candidate::verify(envelope.to_owned(), &self.key)?.local()? != *release {
            return Err(unavailable());
        }
        let parts = size.div_ceil(PACK_PART);
        let mut received = 0;
        progress.note(received, parts);
        let mut pack = vec![0; size];
        for first in (0..parts).step_by(WINDOW) {
            let end = (first + WINDOW).min(parts);
            let mut missing: Vec<_> = (first..end).collect();
            for _ in 0..2 {
                for part in &missing {
                    self.send(
                        peer,
                        json!({"version":release.version,"sha256": release.sha256,"part":part}),
                    )
                    .await?;
                }
                let received = timeout(QUIET, async {
                    while !missing.is_empty() {
                        let value = reply(events, peer, release, |value| {
                            value["i"]
                                .as_u64()
                                .is_some_and(|index| missing.contains(&(index as usize)))
                                && value["d"].is_string()
                        })
                        .await?;
                        let index = value["i"].as_u64().ok_or_else(unavailable)? as usize;
                        let start = index * PACK_PART;
                        let slice = &mut pack[start..(start + PACK_PART).min(size)];
                        let data = value["d"].as_str().ok_or_else(unavailable)?;
                        let bytes = STANDARD.decode(data).map_err(io::Error::other)?;
                        if bytes.len() != slice.len() {
                            return Err(unavailable());
                        }
                        slice.copy_from_slice(&bytes);
                        missing.retain(|part| *part != index);
                        received += 1;
                        progress.note(received, parts);
                    }
                    Ok(())
                })
                .await;
                match received {
                    Ok(result) => {
                        result?;
                        break;
                    }
                    Err(_) => continue,
                }
            }
            if !missing.is_empty() {
                return Err(unavailable());
            }
        }
        if !release.matches_bytes(&pack) {
            return Err(unavailable());
        }
        Ok(pack)
    }

    async fn send(&self, peer: &Host, value: Value) -> io::Result<()> {
        let fields = json!({ "at": system_clock(), "release": value });
        self.relay
            .sender
            .mail(
                peer.clone(),
                Envelope(fields.as_object().expect("mail is an object").clone()),
            )
            .await
            .map_err(io::Error::other)
    }
}

fn answer(
    value: &Value,
    home: &Home,
    key: &str,
    held: &mut Option<HeldPack>,
) -> io::Result<Option<Value>> {
    let hash = value["sha256"].as_str();
    if let Some(version) = value["want"].as_str() {
        *held = HeldPack::find(home, key, version, hash)?;
        return Ok(Some(match held.as_ref() {
            Some(pack) => {
                json!({"version":version,"boxes":pack.candidate.envelope(),"size":pack.size})
            }
            None => json!({"version":version,"none":true}),
        }));
    }
    if let (Some(version), Some(part), true) = (
        value["version"].as_str(),
        value["part"].as_u64(),
        value["d"].is_null(),
    ) && let Ok(part) = usize::try_from(part)
    {
        if !held.as_ref().is_some_and(|pack| {
            pack.reference.version == version
                && hash.map_or_else(
                    || pack.candidate.local().ok().as_ref() == Some(&pack.reference),
                    |hash| hash == pack.reference.sha256,
                )
        }) {
            *held = HeldPack::find(home, key, version, hash)?;
        }
        if let Some(pack) = held.as_mut()
            && let Some(bytes) = pack.part(part)?
        {
            let mut reply = json!({"version":version,"i":part,"d":STANDARD.encode(bytes)});
            if hash.is_some() {
                reply["sha256"] = json!(pack.reference.sha256);
            }
            return Ok(Some(reply));
        }
    }
    Ok(None)
}

async fn reply(
    events: &mut broadcast::Receiver<std::sync::Arc<RelayEvent>>,
    peer: &Host,
    release: &ReleaseRef,
    matches: impl Fn(&Value) -> bool,
) -> io::Result<Value> {
    loop {
        let event = match events.recv().await {
            Ok(event) => event,
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return Err(unavailable()),
        };
        if let RelayEvent::Mail {
            from,
            key,
            envelope,
        } = &*event
            && *from == peer.address
            && *key == peer.key
            && let Some(value) = envelope.0.get("release")
            && value["version"] == release.version
            && value["sha256"]
                .as_str()
                .is_none_or(|hash| hash == release.sha256)
            && matches(value)
        {
            return Ok(value.clone());
        }
    }
}

fn unavailable() -> io::Error {
    io::Error::other("The other computers couldn't send this signed release.")
}
