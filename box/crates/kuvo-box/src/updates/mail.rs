//! Target packs over the existing authenticated peer release mail.

use std::{io, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use kuvo_keeper::{
    fleet::{Candidate, HeldPack, PACK_PART},
    manifest::ReleaseRef,
    staging::MAX_PACK,
};
use kuvo_peers::{
    relay::{RelayEvent, RelayHandle},
    setup_lists::{Reader, system_clock},
};
use kuvo_store::home::Home;
use kuvo_wire::{keys::Host, mail::Envelope};
use serde_json::{Value, json};
use tokio::{sync::broadcast, task::JoinHandle, time::timeout};

const WINDOW: usize = 16;
const QUIET: Duration = Duration::from_secs(60);
const MOST: Duration = Duration::from_secs(300);

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

    pub(super) fn serve(&self, home: Home) -> Option<JoinHandle<()>> {
        let mut events = self.relay.subscribe()?;
        let mail = self.clone();
        Some(tokio::spawn(async move {
            let mut held = None;
            loop {
                let event = match events.recv().await {
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
                    match answer(value, &home, &mail.key, &mut held) {
                        Ok(Some(value)) => {
                            let _ = mail.send(&peer, value).await;
                        }
                        Ok(None) => {}
                        Err(error) => eprintln!("Answering a release request: {error}"),
                    }
                }
            }
        }))
    }

    pub(super) async fn pull(&self, release: &ReleaseRef) -> io::Result<Vec<u8>> {
        let mut events = self.relay.subscribe().ok_or_else(unavailable)?;
        let view = self.reader.view();
        let now = self.reader.now();
        let peers = view.roster.members.iter().filter_map(|member| {
            let (away, facts) = view.peer_updates(&member.key, now);
            let installing = facts.staged.as_ref().is_some_and(|stage| {
                stage.version == release.version && stage.sha256 == release.sha256
            });
            let installed = facts.kuvo.as_deref() == Some(&release.version)
                && facts.installed_sha256.as_deref() == Some(&release.sha256);
            (!away && (installing || installed))
                .then(|| member.host())
                .flatten()
        });
        for peer in peers {
            if let Ok(Ok(pack)) = timeout(MOST, self.pull_from(&mut events, &peer, release)).await {
                return Ok(pack);
            }
        }
        Err(unavailable())
    }

    async fn pull_from(
        &self,
        events: &mut broadcast::Receiver<std::sync::Arc<RelayEvent>>,
        peer: &Host,
        release: &ReleaseRef,
    ) -> io::Result<Vec<u8>> {
        self.send(
            peer,
            json!({"want":release.version, "sha256": release.sha256}),
        )
        .await?;
        let header = timeout(
            QUIET,
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
                && hash.is_none_or(|hash| hash == pack.reference.sha256)
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
