//! Validate an incoming handoff, then publish its bytes and its live row at release.

use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};
use wire::{Hand, Prepared};

const MAX_FILE: usize = 25_000_000;

impl Writer {
    pub(in crate::queue::items) async fn receive_hand(
        &self,
        sender: &str,
        given: Value,
        now: i64,
    ) -> Result<Option<Answer>, Error> {
        let Ok(hand) = serde_json::from_value::<Hand>(given.clone()) else {
            return Ok(None);
        };
        if !store::session_id(&hand.to) {
            return Ok(None);
        }
        let home = self.store.clone();
        let id = hand.id.clone();
        let to = hand.to.clone();
        let release = hand.release.clone();
        let sender = sender.to_owned();
        let address = Prepared::path(&home, &sender, &id);
        let checkpointed = self.checkpointed();
        let loaded = tokio::task::spawn_blocking(move || {
            if wire::seen(&home, &id)? {
                return Ok::<_, Error>(Loaded::Seen);
            }
            let prepared = Prepared::read(&address)?;
            match &release {
                Some(proof) => Ok(prepared
                    .filter(|stored| stored.proof == *proof && stored.payload["to"] == to)
                    .map_or(Loaded::Ignore, |stored| {
                        Loaded::Pending(address, stored.payload, true)
                    })),
                None => Ok(Loaded::Pending(address, given, false)),
            }
        })
        .await
        .map_err(io::Error::other)??;
        let (address, incoming, released) = match loaded {
            Loaded::Seen => return Ok(Some(Answer::accepted(hand.id))),
            Loaded::Ignore => return Ok(None),
            Loaded::Pending(address, incoming, released) => (address, incoming, released),
        };

        let Some(from) = incoming["from"].as_str().filter(|id| owner_id(id)) else {
            return Ok(Some(Answer::refused(hand.id, "That isn't an item.", false)));
        };
        let Some(item) = Item::read(&hand.to, &incoming["item"]) else {
            return Ok(Some(Answer::refused(hand.id, "That isn't an item.", false)));
        };
        let view = self.peers.view();
        if view.local.ownership.frozen.contains(&hand.to) {
            return Ok(Some(Answer::refused(
                hand.id,
                "That conversation is moving between computers: it comes again once it's there.",
                true,
            )));
        }
        let here = &view.roster.name;
        let owner = match self.engine.tool_session_owner(&hand.to).await {
            Ok(Some((owner, _, false))) => owner,
            Ok(Some((_, _, true))) => {
                return Ok(Some(Answer::refused(
                    hand.id,
                    "That's a group chat's seat: hand it to the bot's own conversation.",
                    false,
                )));
            }
            Ok(None) => {
                return Ok(Some(Answer::refused(
                    hand.id,
                    format!("There's no conversation {} on {here}.", hand.to),
                    true,
                )));
            }
            Err(_) => {
                return Ok(Some(Answer::refused(
                    hand.id,
                    format!("{here} can't look up its conversations now."),
                    true,
                )));
            }
        };
        if checkpointed && self.admit(&owner).is_err() {
            return Ok(Some(Answer::refused(
                hand.id,
                "Waiting for the destination queue’s ownership to settle.",
                true,
            )));
        }
        let guard = self.lock().await.map_err(hand_error)?;
        let writer = self.clone();
        let from = from.to_owned();
        let reply = tokio::task::spawn_blocking(move || {
            let _guard = guard;
            writer.place_handoff(
                Arrival {
                    hand,
                    item,
                    owner,
                    from,
                    incoming,
                    address,
                    released,
                    checkpointed,
                },
                now,
            )
        })
        .await
        .map_err(io::Error::other)??;
        Ok(Some(reply))
    }

    fn place_handoff(&self, arrival: Arrival, now: i64) -> Result<Answer, Error> {
        let Arrival {
            hand,
            item,
            owner,
            from,
            incoming,
            address,
            released,
            checkpointed,
        } = arrival;
        let id = &hand.id;
        let (all, compaction) = self.changes(now).map_err(hand_error)?;
        if self
            .fence(Fence::Deleted, &item.id)
            .map_err(hand_error)?
            .is_some()
        {
            return Ok(Answer::refused(
                hand.id,
                "That Work Queue item has been deleted.",
                checkpointed && released,
            ));
        }
        let existing = all.iter().find(|row| row.id == item.id);
        if existing.is_some_and(|row| row.saved.get("handing").is_some()) {
            let here = &self.peers.view().roster.name;
            return Ok(Answer::refused(
                hand.id,
                format!("{here} is still handing that item on: it comes again once that's done."),
                true,
            ));
        }
        if let Some(twin) = item.source().and_then(|source| {
            let reference = self.refs.issue(source["ref"].as_str()?)?.key();
            all.iter().find(|other| {
                other.id != item.id
                    && other.removed.is_none()
                    && other
                        .source()
                        .and_then(|source| {
                            self.refs
                                .issue(source["ref"].as_str()?)
                                .map(|issue| issue.key())
                        })
                        .as_ref()
                        == Some(&reference)
            })
        }) {
            let reference = twin
                .source()
                .and_then(|source| source["ref"].as_str())
                .unwrap_or_default();
            let sessions = crate::queue::sessions::Sessions::read(self.sessions.latest_record());
            let title = sessions.title(&twin.owner);
            let queue = if title.is_empty() {
                twin.owner.clone()
            } else {
                format!("\"{title}\"")
            };
            return Ok(Answer::refused(
                hand.id,
                format!(
                    "{reference} is in the Work Queue there already, in the queue of {queue}: one item for each issue"
                ),
                checkpointed && released,
            ));
        }
        if existing.is_none_or(|row| row.removed.is_some())
            && all
                .iter()
                .filter(|row| {
                    row.owner == owner && row.removed.is_none() && !status::closed(&row.status)
                })
                .count()
                >= 2000
        {
            return Ok(Answer::refused(
                hand.id,
                "That queue has 2000 open items already.",
                checkpointed && released,
            ));
        }
        let carried = match carried_files(&item, &incoming["files"], checkpointed) {
            Some(files) => files,
            None => {
                return Ok(Answer::refused(
                    hand.id,
                    "A Work Queue attachment could not be verified.",
                    false,
                ));
            }
        };
        if checkpointed && !released {
            if let Some(prepared) = Prepared::read(&address)? {
                return Ok(Answer::prepared(hand.id, prepared.proof));
            }
            let proof = key(incoming.clone());
            Prepared {
                proof: proof.clone(),
                payload: incoming,
            }
            .save(&self.store, &address)?;
            return Ok(Answer::prepared(hand.id, proof));
        }
        if existing.is_none_or(|other| other.removed.is_some()) {
            let run = self.store.root.parent().expect("Work Queue under run");
            let who = store::who(&incoming["by"]).unwrap_or_else(|| json!({"name":"someone else"}));
            let setup = self.setup.queue_setup();
            let mut row = item.clone().normalized();
            let additions = store::array(incoming.get("labels"))
                .iter()
                .filter_map(|label| {
                    let given = labels::clean_label(
                        label["id"].as_str()?,
                        label["name"].as_str()?,
                        label["color"].as_str().unwrap_or_default(),
                    )?;
                    item.labels.contains(&given.id).then_some(given)
                })
                .collect::<Vec<_>>();
            let choices = labels::keep_handed(&self.store, setup.as_ref(), additions)?;
            row["labels"] = json!(
                item.labels
                    .iter()
                    .filter(|label| choices.iter().any(|choice| &choice.id == *label))
                    .collect::<Vec<_>>()
            );
            row["files"] = json!(
                item.files
                    .iter()
                    .filter(|file| file["name"]
                        .as_str()
                        .is_some_and(|name| carried.contains_key(name)))
                    .collect::<Vec<_>>()
            );
            let fields = row.as_object_mut().expect("Work Queue item");
            for name in ["removed", "relations", "handing"] {
                fields.remove(name);
            }
            let status = if STATUSES.iter().any(|status| status.id == item.status) {
                item.status.as_str()
            } else {
                row["status"] = "todo".into();
                "todo"
            };
            row["order"] = place(&all, Some(&item.id), status, false).into();
            row["owner"] = owner.clone().into();
            row["updated"] = json!({"at":now,"by":who});
            note_hand(&mut row, &from, &owner, &who, now);
            if !status::closed(status) {
                let mut sessions = store::array(row.get("sessions")).to_vec();
                if !sessions.iter().any(|link| link["session"] == owner) {
                    sessions.push(json!({"session":owner,"since":now}));
                }
                row["sessions"] = sessions.into();
            }
            self.observe_known(&mut row, &item.id);
            attachments::place_handoff(&self.store, &item.id, &carried)?;
            if checkpointed && let Some(own) = self.peers.view().roster.here {
                kuvo_peers::moves::note_queue_placement(
                    run,
                    &item.id,
                    &owner,
                    &kuvo_wire::keys::encode(&own),
                    now,
                    true,
                )?;
            }
            let mut destination = self.read_owner(&owner, now).map_err(hand_error)?;
            destination.items.retain(|row| row["id"] != item.id);
            destination.items.push(row);
            destination.save(&self.store)?;
            if let Some(old) = existing.filter(|old| old.owner != owner) {
                self.retire_owner_row(&old.owner, &old.id, now)
                    .map_err(hand_error)?;
            }
        }
        compaction.finish(self, now).map_err(hand_error)?;
        wire::remember(&self.store, id, now)?;
        if checkpointed {
            files::remove_file_synced(&address)?;
        }
        Ok(Answer::accepted(hand.id))
    }

    pub(super) async fn retire_hand(
        &self,
        item: Item,
        hand: Handing,
        peer: ComputerKey,
        now: i64,
    ) -> Result<(), Refusal> {
        let guard = self.lock().await?;
        let writer = self.clone();
        let checkpointed = self.checkpointed();
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            let have = writer.read_owner(&item.owner, now)?;
            let Some(found) = have
                .items
                .iter()
                .find(|row| row["id"] == item.id)
                .and_then(|row| Item::read(&item.owner, row))
                .as_ref()
                .and_then(Handing::read)
            else {
                return Ok(());
            };
            if found.id != hand.id || (checkpointed && found.released.is_none()) {
                return Ok(());
            }
            let run = writer.store.root.parent().expect("Work Queue under run");
            let peer = kuvo_wire::keys::encode(&peer);
            if checkpointed {
                kuvo_peers::moves::note_queue_placement(run, &item.id, &hand.to, &peer, now, false)
                    .map_err(internal)?;
            }
            writer.note_departed(&item, &hand.to, &peer)?;
            writer.retire_owner_row(&item.owner, &item.id, now)?;
            files::remove_tree_synced(&writer.store.root.join("files").join(&item.id))
                .map_err(internal)?;
            Ok::<_, Refusal>(())
        })
        .await
        .map_err(internal)?
    }

    fn retire_owner_row(&self, owner: &str, id: &str, now: i64) -> Result<(), Refusal> {
        let mut source = self.read_owner(owner, now)?;
        source.items.retain(|row| row["id"] != id);
        if source.items.is_empty() {
            files::remove_file_synced(&source.path).map_err(internal)
        } else {
            source.save(&self.store).map_err(storage_refusal)
        }
    }
}

enum Loaded {
    Seen,
    Ignore,
    Pending(PathBuf, Value, bool),
}

struct Arrival {
    hand: Hand,
    item: Item,
    owner: String,
    from: String,
    incoming: Value,
    address: PathBuf,
    released: bool,
    checkpointed: bool,
}

fn carried_files(
    item: &Item,
    incoming: &Value,
    checkpointed: bool,
) -> Option<std::collections::BTreeMap<String, Vec<u8>>> {
    let mut present = std::collections::BTreeMap::new();
    for file in store::array(Some(incoming)) {
        let (Some(name), Some(base64)) = (
            file["name"].as_str().filter(|name| store::file_name(name)),
            file["data"].as_str(),
        ) else {
            continue;
        };
        if let Ok(bytes) = STANDARD.decode(base64)
            && bytes.len() <= MAX_FILE
        {
            present.insert(name.to_owned(), bytes);
        }
    }
    if checkpointed
        && item.files.iter().any(|file| {
            file["name"]
                .as_str()
                .and_then(|name| present.get(name))
                .is_none_or(|bytes| {
                    file["size"].as_u64() != Some(bytes.len() as u64)
                        || file["sha256"].as_str().is_some_and(|given| {
                            given != kuvo_wire::keys::encode(&Sha256::digest(bytes))
                        })
                })
        })
    {
        return None;
    }
    present.retain(|name, _| item.files.iter().any(|entry| entry["name"] == *name));
    Some(present)
}

pub(in crate::queue::items) fn note_hand(
    row: &mut Value,
    from: &str,
    to: &str,
    who: &Value,
    now: i64,
) {
    if row.get("handedBy").is_none() {
        row["handedBy"] = from.into();
    }
    let mut handed = store::array(row.get("handed")).to_vec();
    handed.push(json!({"from":from,"to":to,"at":now}));
    handed.drain(..handed.len().saturating_sub(20));
    row["handed"] = handed.into();
    append_event(row, json!({"at":now,"by":who,"kind":"handed","to":to}));
}
