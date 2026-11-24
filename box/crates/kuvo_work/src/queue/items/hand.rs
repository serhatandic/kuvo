//! Hand an item and its files to its new keeper. A checkpointed setup
//! stages the incoming item and records release before either side retires it.

mod receive;
mod wire;

use super::*;
use crate::queue::attachments;
use kuvo_peers::directory::{ComputerKey, Member};
pub(super) use receive::note_hand;
use std::collections::HashMap;
use wire::Answer;
pub(super) use wire::{HAND_LIFE, HandId, Handing, MOST_HANDED_FILES, MOVE_FEATURE};

#[derive(Default)]
pub(super) struct Delivery {
    sent: HashMap<String, (ComputerKey, i64)>,
}

impl Writer {
    pub(super) fn hand_peer(&self, owner: &str) -> Option<Member> {
        let view = self.peers.view();
        let location = view.owner_of(RecordScope::Session, owner, self.peers.now())?;
        if location.local {
            return None;
        }
        view.roster.member(&location.key?).cloned()
    }

    pub(super) fn hand_away(
        &self,
        item: &Item,
        all: &[Item],
        peer: Member,
        owner: String,
        name: String,
        who: Value,
        operation: Option<String>,
        now: i64,
    ) -> Result<Value, Refusal> {
        let view = self.peers.view();
        if !view.advertises(&peer.key, told::FEATURE) {
            return Err(bad(format!(
                "{name} is on {}, which runs an older Kuvo Code: items can't be handed to it until it's updated. Ask that conversation to add it with queue_add.",
                peer.name
            )));
        }
        let display = tools::item_name(&Value::Object(item.saved.clone()));
        if view.local.ownership.frozen.contains(&owner) {
            return Err(bad(
                "That conversation is moving to another computer, with its Work Queue: try again once it's there.",
            ));
        }
        edit::refuse_handing(item)?;
        let peer_key = kuvo_wire::keys::encode(&peer.key);
        if let Some(source) = item.source()
            && let Some(reference) = source["ref"].as_str()
        {
            let far = told::items_elsewhere(
                &self.store,
                all,
                &view,
                "all",
                &crate::queue::sessions::Sessions::default(),
                &[],
                now,
            )
            .map_err(storage_refusal)?;
            if let Some(twin) = far.items.iter().find(|twin| {
                twin["id"] != item.id
                    && twin["computer"]["key"] == peer_key
                    && source["ref"]
                        .as_str()
                        .zip(twin["source"]["ref"].as_str())
                        .is_some_and(|(same, other)| same.eq_ignore_ascii_case(other))
            }) {
                let queue = twin["owner"].as_str().unwrap_or_default();
                return Err(bad(format!(
                    "{reference} is in the Work Queue on {} already, in the queue of {queue}: one item for each issue. Ask that conversation (send_message), or the user.",
                    peer.name
                )));
            }
        }
        if self.checkpointed() && !view.advertises(&peer.key, MOVE_FEATURE) {
            return Err(bad(format!(
                "Update {} before handing over a Work Queue item.",
                peer.name
            )));
        }
        let bytes = item
            .files
            .iter()
            .filter_map(|file| {
                let path = self
                    .store
                    .root
                    .join("files")
                    .join(&item.id)
                    .join(file["name"].as_str()?);
                fs::metadata(path).ok().map(|size| size.len())
            })
            .fold(0_u64, u64::saturating_add);
        if bytes > MOST_HANDED_FILES as u64 {
            let size = (bytes as f64 / 1_000_000.0).round();
            let here = &view.roster.name;
            return Err(bad(format!(
                "{display} keeps {size} MB of files: too much to hand to another computer (4 MB at most). Take some away first (queue_update remove), or hand it to a session on {here}."
            )));
        }
        let away = if view.peer_updates(&peer.key, now).0 {
            " (it isn't answering now: it goes once it's back)"
        } else {
            ""
        };
        let answer = tools::said(format!(
            "Handing {display} to {name} on {}: it goes into its queue there, linked to it, as soon as that computer has it{away}, and stays in your view. Until then it's unchanged here.",
            peer.name
        ));
        let to = Handing {
            id: HandId::new().map_err(internal)?,
            to: owner,
            key: peer_key,
            name,
            at: now,
            by: who,
            missing: None,
            released: None,
        };
        let mut source = self.read_owner(&item.owner, now)?;
        let row = source
            .items
            .iter_mut()
            .find(|row| row["id"] == item.id)
            .expect("loaded queue item");
        row["handing"] = json!(to);
        tools::keep_reply(row, operation, &answer);
        source.save(&self.store).map_err(storage_refusal)?;
        Ok(answer)
    }

    pub(super) fn checkpointed(&self) -> bool {
        self.setup
            .queue_setup()
            .is_some_and(|setup| setup.checkpointed)
    }

    pub(super) async fn send_handings(
        &self,
        relay: Option<&RelayHandle>,
        delivery: &mut Delivery,
        now: i64,
    ) -> Result<(), Error> {
        let view = self.peers.view();
        let Some(own) = view.roster.here else {
            return Ok(());
        };
        let store = self.store.clone();
        let items = tokio::task::spawn_blocking(move || store.items())
            .await
            .map_err(io::Error::other)??;
        let setup = self.setup.queue_setup();
        let checkpointed = setup.as_ref().is_some_and(|setup| setup.checkpointed);
        for item in items.into_iter().filter(|item| item.removed.is_none()) {
            let Some(hand) = Handing::read(&item) else {
                continue;
            };
            let Some(before) = hand.peer() else { continue };
            if before == own {
                continue;
            }
            let transfer = if hand.released.is_none() {
                self.hand_peer(&hand.to)
            } else {
                None
            };
            let peer = transfer.or_else(|| view.roster.member(&before).cloned());
            let expiry = if hand.released.is_some() {
                None
            } else if now - hand.at > HAND_LIFE {
                Some("it couldn't be reached for 30 days")
            } else if peer.is_none()
                && setup
                    .as_ref()
                    .is_some_and(|setup| setup.removed_computers.contains(&hand.key))
            {
                Some("you removed that computer")
            } else if peer.is_none()
                && hand
                    .missing
                    .is_some_and(|since| now - since > 3 * 86_400_000)
            {
                Some("that computer hasn't been one of yours for 3 days")
            } else {
                None
            };
            if let Some(why) = expiry {
                self.return_hand(&item, &hand, why, now)
                    .await
                    .map_err(hand_error)?;
                delivery.sent.remove(hand.id.as_str());
                continue;
            }
            match (peer.is_some(), hand.missing) {
                (true, Some(_)) | (false, None) => {
                    let kept = peer.is_none().then_some(now);
                    self.change_hand(
                        &item,
                        &hand,
                        move |row, mut hand| {
                            hand.missing = kept;
                            row["handing"] = json!(hand);
                        },
                        now,
                    )
                    .await
                    .map_err(hand_error)?;
                }
                _ => (),
            }
            let (Some(relay), Some(peer)) = (relay, peer) else {
                continue;
            };
            if delivery
                .sent
                .get(hand.id.as_str())
                .is_some_and(|(_, sent)| now - sent < 60_000)
                || (checkpointed && !view.advertises(&peer.key, MOVE_FEATURE))
                || view.peer_updates(&peer.key, now).0
            {
                continue;
            }
            let Some(host) = peer.host() else { continue };
            let store = self.store.clone();
            let setup = self.setup.queue_setup();
            let pending = hand.clone();
            let snapshot = item.clone();
            let payload = tokio::task::spawn_blocking(move || {
                payload(&store, setup.as_ref(), snapshot, pending)
            })
            .await
            .map_err(io::Error::other)??;
            let Some(mails) = kuvo_peers::work_queue_mails(payload, now) else {
                self.return_hand(&item, &hand, "it's too big to send", now)
                    .await
                    .map_err(hand_error)?;
                continue;
            };
            let mut sent = true;
            for mail in mails {
                if relay.sender.mail(host.clone(), mail).await.is_err() {
                    sent = false;
                    break;
                }
            }
            if sent {
                delivery
                    .sent
                    .insert(hand.id.as_str().into(), (peer.key, now));
            }
        }
        Ok(())
    }

    pub(super) async fn handed_back(
        &self,
        key: ComputerKey,
        answer: &Value,
        delivery: &mut Delivery,
        now: i64,
    ) -> Result<(), Error> {
        let Ok(answer) = serde_json::from_value::<Answer>(answer.clone()) else {
            return Ok(());
        };
        let store = self.store.clone();
        let id = answer.id.clone();
        let Some((item, hand)) = tokio::task::spawn_blocking(move || {
            Ok::<_, Error>(store.items()?.into_iter().find_map(|item| {
                let hand = Handing::read(&item)?;
                (id == hand.id).then_some((item, hand))
            }))
        })
        .await
        .map_err(io::Error::other)??
        else {
            return Ok(());
        };
        let sent = delivery.sent.get(hand.id.as_str()).map(|(to, _)| *to);
        if sent.or_else(|| hand.peer()) != Some(key) {
            return Ok(());
        }
        if let Some(proof) = answer
            .prepared
            .filter(|proof| kuvo_wire::keys::decode_exact::<32>(proof).is_some())
        {
            self.change_hand(
                &item,
                &hand,
                move |row, mut hand| {
                    if hand.released.is_none() {
                        hand.released = Some(proof);
                        hand.key = kuvo_wire::keys::encode(&key);
                        row["handing"] = json!(hand);
                    }
                },
                now,
            )
            .await
            .map_err(hand_error)?;
            delivery.sent.remove(hand.id.as_str());
        } else if answer.retry {
            delivery.sent.insert(hand.id.as_str().into(), (key, now));
        } else if answer.ok != Some(true) {
            let why = answer
                .why
                .map(|why| limit(&plain_name(&why), 300))
                .filter(|why| !why.is_empty());
            self.return_hand(
                &item,
                &hand,
                why.as_deref().unwrap_or("it was refused"),
                now,
            )
            .await
            .map_err(hand_error)?;
            delivery.sent.remove(hand.id.as_str());
        } else if !self.checkpointed() || hand.released.is_some() {
            self.retire_hand(item, hand.clone(), key, now)
                .await
                .map_err(hand_error)?;
            delivery.sent.remove(hand.id.as_str());
        }
        Ok(())
    }

    async fn change_hand(
        &self,
        item: &Item,
        hand: &Handing,
        edit: impl FnOnce(&mut Value, Handing) + Send + 'static,
        now: i64,
    ) -> Result<(), Refusal> {
        let guard = self.lock().await?;
        let writer = self.clone();
        let owner = item.owner.clone();
        let id = item.id.clone();
        let prepared = hand.id.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            let mut document = writer.read_owner(&owner, now)?;
            let Some(row) = document.items.iter_mut().find(|row| row["id"] == id) else {
                return Ok(());
            };
            let Some(hand) = Item::read(&owner, row)
                .as_ref()
                .and_then(Handing::read)
                .filter(|hand| hand.id == prepared)
            else {
                return Ok(());
            };
            let before = row.clone();
            edit(row, hand);
            if *row != before {
                document.save(&writer.store).map_err(storage_refusal)?;
            }
            Ok(())
        })
        .await
        .map_err(internal)?
    }

    async fn return_hand(
        &self,
        item: &Item,
        hand: &Handing,
        why: &str,
        now: i64,
    ) -> Result<(), Refusal> {
        let why = why.trim_end_matches(['.', ' ']).to_owned();
        self.change_hand(item, hand, move |row, hand| {
            if hand.released.is_some() {
                return;
            }
            row.as_object_mut().expect("Work Queue item").remove("handing");
            let name = if hand.name.is_empty() { hand.to } else { hand.name };
            let mut notes = store::array(row.get("notes")).to_vec();
            notes.push(json!({"at":now,"by":hand.by,"text":format!("Couldn't be handed to {name} on another computer: {why}. It's still here.")}));
            row["notes"] = notes.into();
        }, now).await
    }
}

pub(super) fn hand_error(error: Refusal) -> Error {
    io::Error::other(format!(
        "Could not save the Work Queue handoff ({})",
        error.reply().status
    ))
    .into()
}

fn payload(
    store: &Store,
    setup: Option<&kuvo_setup::QueueSetup>,
    item: Item,
    hand: Handing,
) -> Result<Value, Error> {
    if let Some(released) = hand.released {
        return Ok(json!({"hand":{"id":hand.id,"to":hand.to,"release":released}}));
    }
    let mut row = item.clone().normalized();
    let saved = row.as_object_mut().expect("Work Queue item");
    saved.remove("handing");
    saved.remove("relations");
    let mut carried = Vec::new();
    for file in &item.files {
        let Some(name) = file["name"].as_str() else {
            continue;
        };
        if let Some(download) = attachments::read(store, &item, name) {
            use base64::{Engine as _, engine::general_purpose::STANDARD};
            carried.push(json!({"name":name,"data":STANDARD.encode(download.bytes)}));
        }
    }
    let mut hand = json!({"id":hand.id,"to":hand.to,"from":item.owner,"by":hand.by,"at":hand.at,"item":row,"files":carried});
    let attached = labels::shown(store, setup)?
        .into_iter()
        .filter(|label| item.labels.contains(&label.id))
        .collect::<Vec<_>>();
    if !attached.is_empty() {
        hand["labels"] = json!(attached);
    }
    Ok(json!({"hand":hand}))
}
