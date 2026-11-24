//! The Work Queue records and file proofs carried by a conversation move.

use super::{
    attachments, labels, migration, status,
    store::{self, Item, QUEUE, Store},
};
use kuvo_peers::moves::{
    manifest::{FileProof, QueueFile},
    work_queue::{
        ExpectedQueueFile, PlacedItem, QueuePlacement, QueueSnapshot, StagedQueue,
        WorkQueueTransfer,
    },
};
use kuvo_setup::SetupHandle;
use kuvo_store::{
    files,
    format::{Fields, ObjectFormat},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

const DELETION: ObjectFormat = ObjectFormat::additive("Owned data");
const IMPORT_MARKER: &str = ".kuvo-move-import";

pub fn transfer(home: &Path, setup: SetupHandle) -> Arc<dyn WorkQueueTransfer> {
    Arc::new(QueueKeeper {
        store: Store::new(home),
        setup,
    })
}

struct QueueKeeper {
    store: Store,
    setup: SetupHandle,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Carried {
    items: Vec<Value>,
    #[serde(default)]
    labels: Vec<Value>,
    #[serde(default)]
    with_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    deletions: Vec<Value>,
}

impl Carried {
    fn fingerprint(&self) -> io::Result<String> {
        Ok(kuvo_wire::keys::encode(&kuvo_wire::json::digest(
            &serde_json::to_value(self)?,
        )))
    }

    fn records(&self) -> Vec<String> {
        self.items
            .iter()
            .chain(&self.deletions)
            .filter_map(|row| row["id"].as_str().map(str::to_owned))
            .collect()
    }
}

impl QueueKeeper {
    fn lock(&self) -> io::Result<kuvo_store::locks::OwnedLock> {
        files::create_private_dirs(&self.store.root)?;
        migration::migrate(&self.store.root).map_err(io::Error::other)?;
        store::exclusive(&self.store.root.join(".lock"), Duration::from_secs(3))
    }

    fn run(&self) -> &Path {
        self.store.root.parent().expect("Work Queue under run")
    }

    fn deleted_folder(&self) -> PathBuf {
        self.run().join("owned-data").join("edit-deletions")
    }

    fn deleted_file(&self, item: &str) -> PathBuf {
        let key = kuvo_wire::keys::encode(&kuvo_wire::json::digest(&json!([item])));
        self.deleted_folder().join(format!("{key}.json"))
    }

    fn deletions(&self, owners: &[String]) -> io::Result<Vec<Value>> {
        store::queue_files(&self.deleted_folder())
            .into_iter()
            .filter_map(|file| match DELETION.read(&file) {
                Ok(Some(row))
                    if row
                        .fields
                        .get("queue")
                        .and_then(Value::as_str)
                        .is_some_and(|owner| owners.iter().any(|each| each == owner)) =>
                {
                    Some(Ok(Value::Object(row.fields)))
                }
                Ok(_) => None,
                Err(error) => Some(Err(io::Error::other(error))),
            })
            .collect()
    }

    fn read_owner(&self, owner: &str) -> io::Result<Vec<Value>> {
        let Some(row) = QUEUE
            .read(&self.store.root.join("queues").join(format!("{owner}.json")))
            .map_err(io::Error::other)?
        else {
            return Ok(Vec::new());
        };
        let rows = row
            .fields
            .get("items")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("A Work Queue owner file has no items."))?;
        if rows
            .iter()
            .any(|row| Item::read(owner, row).is_none() || !valid_files(row.get("files")))
        {
            return Err(invalid(
                "A Work Queue owner file has an invalid item or file list.",
            ));
        }
        Ok(rows.clone())
    }

    fn save_owner(&self, owner: &str, items: Vec<Value>) -> io::Result<()> {
        let path = self.store.root.join("queues").join(format!("{owner}.json"));
        if items.is_empty() {
            files::remove_file_synced(&path)
        } else {
            let fields = Fields::from_iter([
                ("owner".into(), owner.into()),
                ("items".into(), items.into()),
            ]);
            let bytes = serde_json::to_vec(&QUEUE.encode(fields))?;
            files::write_atomic_through(&path, &bytes, self.run())
        }
    }
    fn collect(&self, owners: &[String], own: Option<&str>) -> io::Result<Option<Carried>> {
        for owner in owners.iter().filter(|owner| store::owner_id(owner)) {
            self.read_owner(owner)?;
        }
        let state = self.setup.queue_setup();
        let all = self.store.items().map_err(io::Error::other)?;
        if let Some(own) = own
            && all.iter().any(|item| {
                item.removed.is_none()
                    && item.saved.get("handing").is_some_and(|hand| {
                        hand["key"] == own
                            && (owners.contains(&item.owner)
                                || hand["to"]
                                    .as_str()
                                    .is_some_and(|to| owners.iter().any(|owner| owner == to)))
                    })
            })
        {
            return Err(invalid(
                "A Work Queue handoff is still in progress. Try the move again once it finishes.",
            ));
        }
        let items = all
            .into_iter()
            .filter(|item| owners.contains(&item.owner))
            .collect::<Vec<_>>();
        let deletions = self.deletions(owners)?;
        if items.is_empty() && deletions.is_empty() {
            return Ok(None);
        }
        let travelling = items
            .iter()
            .map(|item| item.id.clone())
            .collect::<BTreeSet<_>>();
        let used = items
            .iter()
            .flat_map(|item| &item.labels)
            .cloned()
            .collect::<BTreeSet<_>>();
        let labels = labels::shown(&self.store, state.as_ref())
            .map_err(io::Error::other)?
            .into_iter()
            .filter(|label| used.contains(&label.id))
            .map(|label| json!(label))
            .collect();
        let mut carried = Carried {
            items: vec![],
            labels,
            with_files: vec![],
            deletions,
        };
        for item in items {
            let removed = item.removed.is_some();
            let id = item.id.clone();
            let has_files = !item.files.is_empty();
            let mut row = item.normalized();
            let fields = row.as_object_mut().expect("Work Queue item");
            if removed {
                fields.remove("handing");
            }
            if let Some(links) = fields.get_mut("relations").and_then(Value::as_array_mut) {
                links.retain(|link| {
                    link["item"]
                        .as_str()
                        .is_some_and(|id| travelling.contains(id))
                });
                if links.is_empty() {
                    fields.remove("relations");
                }
            }
            if has_files {
                carried.with_files.push(id);
            }
            carried.items.push(row);
        }
        Ok(Some(carried))
    }
}

impl WorkQueueTransfer for QueueKeeper {
    fn stage(
        &self,
        owners: &[String],
        own: Option<&str>,
        directory: &Path,
        mut allowance: u64,
    ) -> io::Result<Option<StagedQueue>> {
        let _guard = self.lock()?;
        let Some(mut carried) = self.collect(owners, own)? else {
            return Ok(None);
        };
        let fingerprint = carried.fingerprint()?;
        let records = carried.records();
        let mut files = Vec::new();
        for row in &mut carried.items {
            let item = row["id"]
                .as_str()
                .expect("collected Work Queue item")
                .to_owned();
            for file in row["files"].as_array_mut().expect("Work Queue files") {
                let name = file["name"]
                    .as_str()
                    .filter(|name| file_name(name))
                    .ok_or_else(|| invalid("A Work Queue attachment has an invalid name."))?;
                let size = file_size(&file["size"]).ok_or_else(invalid_inventory)?;
                if size > allowance {
                    return Err(io::ErrorKind::FileTooLarge.into());
                }
                let reader =
                    attachments::open_saved(&self.store, &item, name).ok_or_else(|| {
                        invalid(format!(
                            "A Work Queue attachment is missing or unreadable: {name}."
                        ))
                    })?;
                let relative = format!("{item}/{name}");
                let mut digest = None;
                files::write_generated_new(&directory.join(&relative), |to| {
                    let mut reader = reader.take(size.saturating_add(1));
                    let mut hasher = Sha256::new();
                    let mut buffer = [0; 64 * 1024];
                    let mut copied = 0u64;
                    loop {
                        let read = reader.read(&mut buffer)?;
                        if read == 0 {
                            break;
                        }
                        hasher.update(&buffer[..read]);
                        to.write_all(&buffer[..read])?;
                        copied += read as u64;
                    }
                    let sha256 = kuvo_wire::keys::encode(&hasher.finalize());
                    if copied != size
                        || file["sha256"]
                            .as_str()
                            .is_some_and(|expected| expected != sha256)
                    {
                        return Err(invalid(format!(
                            "A Work Queue attachment no longer matches its recorded bytes: {name}."
                        )));
                    }
                    digest = Some(sha256);
                    Ok(())
                })?;
                let sha256 = digest.expect("verified Work Queue attachment");
                file["sha256"] = sha256.clone().into();
                files.push(QueueFile {
                    relative,
                    proof: FileProof { size, sha256 },
                });
                allowance -= size;
            }
        }
        Ok(Some(StagedQueue {
            carried: serde_json::to_value(carried)?,
            snapshot: QueueSnapshot {
                files,
                fingerprint,
                records,
            },
        }))
    }

    fn source_fingerprint(
        &self,
        owners: &[String],
        own: Option<&str>,
    ) -> io::Result<Option<String>> {
        let _guard = self.lock()?;
        self.collect(owners, own)?
            .map(|carried| carried.fingerprint())
            .transpose()
    }

    fn expected_files(
        &self,
        carried: &Value,
        owners: &[String],
        require_proof: bool,
    ) -> io::Result<Vec<ExpectedQueueFile>> {
        let carried = checked(carried.clone(), owners)?;
        let mut seen = BTreeSet::new();
        let mut files = Vec::new();
        for row in &carried.items {
            for file in store::array(row.get("files")) {
                let name = file["name"]
                    .as_str()
                    .filter(|name| file_name(name))
                    .ok_or_else(invalid_inventory)?;
                let size = file_size(&file["size"]).ok_or_else(invalid_inventory)?;
                let relative = format!(
                    "{}/{name}",
                    row["id"].as_str().expect("checked Work Queue item")
                );
                let sha256 = file
                    .get("sha256")
                    .map(|digest| {
                        digest
                            .as_str()
                            .filter(|digest| {
                                digest.len() == 43
                                    && digest.bytes().all(|ch| {
                                        ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'-')
                                    })
                            })
                            .map(str::to_owned)
                            .ok_or_else(invalid_inventory)
                    })
                    .transpose()?;
                if !seen.insert(relative.clone()) || (require_proof && sha256.is_none()) {
                    return Err(invalid_inventory());
                }
                files.push(ExpectedQueueFile {
                    relative,
                    size,
                    sha256,
                });
            }
        }
        Ok(files)
    }

    fn install(
        &self,
        carried: Value,
        owners: &[String],
        staged: &Path,
        files: &[QueueFile],
        movement: &str,
        journal: &mut dyn FnMut(QueuePlacement) -> io::Result<()>,
    ) -> io::Result<()> {
        let carried = checked(carried, owners)?;
        let _guard = self.lock()?;
        let store = &self.store;
        let held = store
            .items()
            .map_err(io::Error::other)?
            .into_iter()
            .map(|item| item.id)
            .collect::<BTreeSet<_>>();
        let stash = attachments::move_files_root(store, false)?;
        let mut placed = Vec::new();
        let mut ids = BTreeSet::new();
        for row in &carried.items {
            let id = row["id"].as_str().expect("checked Work Queue item");
            let owner = row["owner"].as_str().expect("checked Work Queue owner");
            if held.contains(id)
                || !ids.insert(id.to_owned())
                || self.deleted_file(id).try_exists()?
                || (!store::array(row.get("files")).is_empty()
                    && stash
                        .as_ref()
                        .is_some_and(|root| fs::symlink_metadata(root.join(id)).is_ok()))
            {
                return Err(invalid(
                    "A carried Work Queue item or its attachment directory already exists or was deleted here.",
                ));
            }
            placed.push(PlacedItem {
                id: id.into(),
                owner: owner.into(),
            });
        }
        let mut deletions = Vec::new();
        for deletion in carried.deletions {
            let id = deletion["id"]
                .as_str()
                .expect("checked Work Queue deletion");
            if held.contains(id)
                || !ids.insert(id.to_owned())
                || self.deleted_file(id).try_exists()?
            {
                return Err(invalid("A carried Work Queue deletion overlaps an item"));
            }
            deletions.push(json!({"id":id,"queue":deletion["queue"],"at":deletion["at"]}));
        }
        let setup = self.setup.queue_setup();
        let used: BTreeSet<_> = carried
            .items
            .iter()
            .flat_map(|row| store::array(row.get("labels")))
            .filter_map(Value::as_str)
            .collect();
        let provided = carried
            .labels
            .iter()
            .filter_map(|label| {
                let checked = labels::clean_label(
                    label["id"].as_str()?,
                    label["name"].as_str()?,
                    label["color"].as_str().unwrap_or_default(),
                )?;
                used.contains(checked.id.as_str()).then_some(checked)
            })
            .collect();
        let accepted =
            labels::keep_handed(store, setup.as_ref(), provided).map_err(io::Error::other)?;
        let mut documents = BTreeMap::<String, Vec<Value>>::new();
        let mut items = Vec::new();
        for mut row in carried.items {
            let owner = row["owner"].as_str().unwrap().to_owned();
            let item = Item::read(&owner, &row).expect("checked Work Queue item");
            let id = item.id.clone();
            row = item.normalized();
            let fields = row.as_object_mut().expect("Work Queue item");
            if let Some(names) = fields.get_mut("labels").and_then(Value::as_array_mut) {
                names.retain(|name| accepted.iter().any(|label| *name == label.id));
            }
            if !status::STATUSES
                .iter()
                .any(|status| row["status"] == status.id)
            {
                let was = row["status"].clone();
                row["status"] = "todo".into();
                let at = jiff::Timestamp::now().as_millisecond();
                add_history(
                    &mut row,
                    json!({"kind":"moved","from":was,"to":"todo","at":at,"by":{"source":"kuvo","name":"Kuvo Code"}}),
                );
            }
            for file in row["files"].as_array_mut().expect("Work Queue files") {
                let relative = format!("{id}/{}", file["name"].as_str().unwrap());
                let proof = files
                    .iter()
                    .find(|file| file.relative == relative)
                    .ok_or_else(invalid_inventory)?;
                file["sha256"] = proof.proof.sha256.clone().into();
            }
            if !documents.contains_key(&owner) {
                documents.insert(owner.clone(), self.read_owner(&owner)?);
            }
            let has_files = !store::array(row.get("files")).is_empty();
            documents
                .get_mut(&owner)
                .expect("loaded owner queue")
                .push(row);
            items.push((id, has_files));
        }
        journal(QueuePlacement {
            items: placed,
            deletions: deletions.clone(),
        })?;
        let destination_root = if files.is_empty() {
            stash
        } else {
            attachments::move_files_root(store, true)?
        };
        for (id, has_files) in items {
            if has_files {
                let from = staged.join(&id);
                files::write_atomic_through(
                    &from.join(IMPORT_MARKER),
                    movement.as_bytes(),
                    staged,
                )?;
                let destination = destination_root
                    .as_ref()
                    .expect("Work Queue attachment root")
                    .join(&id);
                fs::rename(from, &destination)?;
                let through = self.run().canonicalize()?;
                files::sync_tree(&destination, &through)?;
            }
        }
        self.verify(files)?;
        for (owner, items) in documents {
            self.save_owner(&owner, items)?;
        }
        for deletion in deletions {
            let id = deletion["id"].as_str().unwrap();
            let bytes =
                serde_json::to_vec(&DELETION.encode(deletion.as_object().unwrap().clone()))?;
            files::write_atomic_through(&self.deleted_file(id), &bytes, self.run())?;
        }
        Ok(())
    }

    fn verify(&self, expected: &[QueueFile]) -> io::Result<()> {
        for file in expected {
            let (item, name) = file
                .relative
                .split_once('/')
                .filter(|(item, name)| store::item_id(item) && file_name(name))
                .ok_or_else(invalid_inventory)?;
            attachments::verify_handoff_file(
                &self.store,
                item,
                name,
                file.proof.size,
                &file.proof.sha256,
            )?;
        }
        Ok(())
    }

    fn finish(
        &self,
        movement: &str,
        placement: &QueuePlacement,
        payloads: &[QueueFile],
        roll_back: bool,
    ) -> io::Result<()> {
        let _guard = self.lock()?;
        if roll_back {
            let mut owners = BTreeMap::<&str, Vec<&str>>::new();
            for row in &placement.items {
                if !store::item_id(&row.id) || !store::owner_id(&row.owner) {
                    return Err(invalid("A Work Queue placement journal is invalid."));
                }
                owners.entry(&row.owner).or_default().push(&row.id);
            }
            for (owner, ids) in owners {
                let mut saved = self.read_owner(owner)?;
                saved.retain(|row| !row["id"].as_str().is_some_and(|id| ids.contains(&id)));
                self.save_owner(owner, saved)?;
            }
            for gone in &placement.deletions {
                let id = gone["id"]
                    .as_str()
                    .filter(|id| store::item_id(id))
                    .ok_or_else(invalid_inventory)?;
                let file = self.deleted_file(id);
                if store::read(&file, DELETION)
                    .map_err(io::Error::other)?
                    .is_some_and(|record| {
                        record.get("queue") == gone.get("queue")
                            && record.get("at") == gone.get("at")
                    })
                {
                    files::remove_file_synced(&file)?;
                }
            }
        }
        let Some(attachments) = attachments::move_files_root(&self.store, false)? else {
            return Ok(());
        };
        let ids = payloads
            .iter()
            .filter_map(|file| file.relative.split_once('/').map(|(id, _)| id))
            .filter(|id| store::item_id(id))
            .collect::<BTreeSet<_>>();
        for id in ids {
            let directory = attachments.join(id);
            let marker = directory.join(IMPORT_MARKER);
            if !fs::symlink_metadata(&marker).is_ok_and(|info| info.is_file())
                || files::read(&marker)?.as_deref() != Some(movement.as_bytes())
            {
                continue;
            }
            if roll_back {
                files::remove_tree_synced(&directory)?;
            } else {
                files::remove_file_synced(&marker)?;
            }
        }
        Ok(())
    }

    fn retire(&self, owners: &[String]) -> io::Result<()> {
        let _guard = self.lock()?;
        let attachments = attachments::move_files_root(&self.store, false)?;
        for owner in owners.iter().filter(|owner| store::owner_id(owner)) {
            for row in self.read_owner(owner)? {
                if let Some(id) = row["id"].as_str().filter(|id| store::item_id(id))
                    && let Some(attachments) = &attachments
                {
                    files::remove_tree_synced(&attachments.join(id))?;
                }
            }
            self.save_owner(owner, Vec::new())?;
        }
        Ok(())
    }
}

fn checked(carried: Value, owners: &[String]) -> io::Result<Carried> {
    let carried: Carried = serde_json::from_value(carried)?;
    for row in &carried.items {
        if !row["owner"].as_str().is_some_and(|owner| {
            owners.iter().any(|each| each == owner) && Item::read(owner, row).is_some()
        }) {
            return Err(invalid("A carried Work Queue item is invalid."));
        }
        if !valid_files(row.get("files")) {
            return Err(invalid("Invalid carried Work Queue file list."));
        }
    }
    for gone in &carried.deletions {
        if !gone["id"].as_str().is_some_and(store::item_id)
            || !gone["queue"]
                .as_str()
                .is_some_and(|owner| owners.iter().any(|each| each == owner))
            || gone["at"].as_i64().is_none()
        {
            return Err(invalid("A carried deletion record is invalid"));
        }
    }
    Ok(carried)
}

fn file_size(value: &Value) -> Option<u64> {
    let size = value.as_f64()?;
    (size >= 0.0 && size <= ((1u64 << 53) - 1) as f64 && size.fract() == 0.0).then_some(size as u64)
}

fn valid_files(value: Option<&Value>) -> bool {
    let Some(value) = value else { return true };
    let Some(files) = value.as_array() else {
        return false;
    };
    let mut seen = BTreeSet::new();
    files.iter().all(|file| {
        file["name"]
            .as_str()
            .is_some_and(|name| file_name(name) && seen.insert(name))
            && file_size(&file["size"]).is_some()
            && file.get("sha256").is_none_or(|digest| {
                digest.as_str().is_some_and(|digest| {
                    digest.len() == 43
                        && digest
                            .bytes()
                            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'-'))
                })
            })
    })
}

fn file_name(name: &str) -> bool {
    name != IMPORT_MARKER && store::file_name(name)
}

fn add_history(row: &mut Value, event: Value) {
    let mut history = store::array(row.get("events")).to_vec();
    history.push(event);
    history.drain(..history.len().saturating_sub(50));
    row["events"] = history.into();
}

fn invalid_inventory() -> io::Error {
    invalid("A carried Work Queue file inventory is invalid.")
}

fn invalid(words: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, words.into())
}
