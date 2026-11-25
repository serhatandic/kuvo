use super::{
    status,
    store::{self, Error, LABELS, QUEUE, array, label_id, limit, plain_name, queue_files},
};
use icu_collator::{Collator, options::CollatorOptions};
use jiff::Timestamp;
use kuvo_store::{
    files,
    format::{self, Fields, ObjectFormat},
    locks::{OnBusy, StaleLock},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fs, io,
    path::Path,
    time::Duration,
};

const MODEL: ObjectFormat = ObjectFormat::additive("Work Queue status model");
const CATEGORIES: ObjectFormat = ObjectFormat::additive("Work Queue legacy categories");
const STALE: Duration = Duration::from_secs(120);

fn current(root: &Path) -> Result<bool, Error> {
    let record = store::read(&root.join("model.json"), MODEL)?;
    let model = record
        .as_ref()
        .and_then(|fields| fields.get("model"))
        .and_then(Value::as_f64);
    if let Some(version) = model.filter(|version| *version > 2.0) {
        return Err(format::Error::Unsupported {
            family: MODEL.family.into(),
            found: if version.fract() == 0.0 && version <= 9_007_199_254_740_991.0 {
                version.to_string()
            } else {
                "invalid".into()
            },
            current: 2,
        }
        .into());
    }
    Ok(model == Some(2.0))
}

pub(super) fn migrate(root: &Path) -> Result<(), Error> {
    migrate_once(root).map_err(|error| match error {
        Error::Io(error) => Error::Migration(format!(
            "The Work Queue's items couldn't be moved over to statuses on this computer ({error}), so nothing was changed. Check {} (is the disk full?), then try again.",
            root.display()
        )),
        error => error,
    })
}

fn migrate_once(root: &Path) -> Result<(), Error> {
    if current(root)? || !root.try_exists()? {
        return Ok(());
    }
    let _moving = StaleLock::acquire(
        &root.join(".moving-to-statuses"),
        STALE,
        Duration::from_millis(25),
        OnBusy::Refuse,
    )
    .map_err(|error| {
        if error.kind() == io::ErrorKind::WouldBlock {
            Error::Migration("The Work Queue's items are being moved over to statuses on this computer right now, so nothing was changed. Try again in a moment (two minutes at most, if what was moving them was cut off).".into())
        } else {
            error.into()
        }
    })?;
    let _queues = store::exclusive(&root.join(".lock"), Duration::from_secs(10))?;
    if current(root)? {
        return Ok(());
    }
    let _labels = StaleLock::acquire(
        &root.join(".labels.lock"),
        STALE,
        Duration::from_secs(10),
        OnBusy::Refuse,
    )?;
    migrate_locked(root)
}

fn migrate_locked(root: &Path) -> Result<(), Error> {
    let queues = root.join("queues");
    let categories = root.join("categories.json");
    let labels = root.join("labels.json");
    let original = root.join("before-statuses");
    let live = queue_files(&queues)
        .into_iter()
        .map(|file| store::read(&file, QUEUE).map(|fields| (file, fields)))
        .collect::<Result<Vec<_>, _>>()?;
    let kept_labels = store::read(&labels, LABELS)?;
    store::read(&categories, CATEGORIES)?;

    if !original.try_exists()? {
        if live.is_empty() && !categories.try_exists()? {
            return finish(root, Vec::new());
        }
        // Take a complete, durable copy under the migration lock before editing any file.
        let draft = root.join("before-statuses.rust");
        for name in files::list_of(root)
            .into_iter()
            .filter(|name| name.to_string_lossy().starts_with("before-statuses."))
        {
            files::remove_tree_synced(&root.join(name))?;
        }
        let draft_queues = draft.join("queues");
        files::create_private_dirs(&draft_queues)?;
        for (file, _) in &live {
            files::write_atomic(
                &draft_queues.join(file.file_name().expect("queue file name")),
                &fs::read(file)?,
            )?;
        }
        if categories.try_exists()? {
            files::write_atomic(&draft.join("categories.json"), &fs::read(&categories)?)?;
        }
        files::sync_parent(&draft_queues)?;
        fs::rename(draft, &original)?;
        files::sync_parent(&original)?;
    }

    let saved_queues = original.join("queues");
    if !saved_queues.is_dir() {
        return Err(io::Error::other(format!(
            "{} isn't the copy of what was kept",
            original.display()
        ))
        .into());
    }
    let saved_categories = store::read(&original.join("categories.json"), CATEGORIES)?;
    let custom: Vec<_> = saved_categories
        .iter()
        .flat_map(|fields| array(fields.get("custom")))
        .filter_map(|entry| Some((entry["id"].as_str()?, entry["name"].as_str()?)))
        .filter(|(id, _)| id.starts_with("cat_") && label_id(id))
        .collect();
    let names: HashMap<_, _> = custom.iter().copied().collect();
    let convert = |id: Option<&str>| {
        let (status, label) = status::category(id, id.and_then(|id| names.get(id).copied()));
        (status, label.map(str::to_owned))
    };
    let position = |id: Option<&str>| match id {
        Some("next") => 0,
        Some("later") => 1,
        Some("done") => custom.len() + 3,
        _ => {
            custom
                .iter()
                .position(|(key, _)| Some(*key) == id)
                .unwrap_or(custom.len())
                + 2
        }
    };

    let mut order = HashMap::<&str, Vec<_>>::new();
    let mut started = Vec::new();
    let mut used = BTreeSet::new();
    for file in queue_files(&saved_queues) {
        let Some(queue) = store::read(&file, QUEUE)? else {
            continue;
        };
        for item in array(queue.get("items"))
            .iter()
            .filter(|item| !item["status"].is_string())
        {
            let Some(id) = item["id"].as_str() else {
                continue;
            };
            let category = item["category"].as_str();
            let (status, label) = convert(category);
            used.extend(label);
            if !truthy(&item["removed"]) && !status::closed(status) && was_started(item) {
                started.push(id.to_owned());
            }
            order.entry(status).or_default().push((
                position(category),
                item["order"].as_f64().unwrap_or_default(),
                id.to_owned(),
            ));
        }
    }
    let collation = Collator::try_new(Default::default(), CollatorOptions::default())
        .expect("the root collation is compiled in");
    let order: HashMap<_, _> = order
        .into_values()
        .flat_map(|mut items| {
            items.sort_by(|(ap, ao, ai), (bp, bo, bi)| {
                ap.cmp(bp)
                    .then_with(|| ao.partial_cmp(bo).unwrap_or(std::cmp::Ordering::Equal))
                    .then_with(|| collation.compare(ai, bi))
            });
            items
                .into_iter()
                .enumerate()
                .map(|(index, (_, _, id))| (id, index))
        })
        .collect();

    let needs: HashSet<_> = live
        .iter()
        .filter_map(|(_, queue)| queue.as_ref())
        .flat_map(|queue| array(queue.get("items")))
        .filter(|item| item.is_object() && !item["status"].is_string())
        .filter_map(|item| convert(item["category"].as_str()).1)
        .collect();
    let mut created = Vec::new();
    for &(id, name) in &custom {
        if convert(Some(id)).1.is_some() {
            let name = limit(&plain_name(name), 40);
            created.push(label(
                id,
                if name.is_empty() {
                    id[4..].into()
                } else {
                    name
                },
            ));
        }
    }
    for id in used
        .iter()
        .filter(|id| id.as_str() != "lbl_parked" && !names.contains_key(id.as_str()))
    {
        created.push(label(id, id[4..].replace('-', " ")));
    }
    if used.contains("lbl_parked") {
        created.push(json!({"id": "lbl_parked", "name": "Parked", "color": "graphite"}));
    }
    let already_kept = kept_labels.is_some();
    let mut fields = kept_labels.unwrap_or_default();
    let mut existing = array(fields.get("labels")).to_vec();
    let have: HashSet<_> = existing
        .iter()
        .filter_map(|label| label["id"].as_str())
        .map(str::to_owned)
        .collect();
    let old_len = existing.len();
    existing.extend(created.into_iter().filter(|label| {
        let id = label["id"].as_str().expect("migrated label id");
        !have.contains(id) && (!already_kept || needs.contains(id))
    }));
    if existing.len() != old_len {
        fields.insert("labels".into(), existing.into());
        store::write(&labels, LABELS, fields)?;
    }

    for (file, fields) in live {
        let Some(mut fields) = fields else { continue };
        let Some(items) = fields.get_mut("items").and_then(Value::as_array_mut) else {
            continue;
        };
        let mut changed = false;
        for item in items.iter_mut().filter_map(Value::as_object_mut) {
            if item.get("status").is_some_and(Value::is_string) {
                continue;
            }
            changed = true;
            let (status, label) = convert(item.get("category").and_then(Value::as_str));
            item.insert("status".into(), status.into());
            item.insert(
                "labels".into(),
                json!(label.into_iter().collect::<Vec<_>>()),
            );
            item.remove("category");
            if let Some(place) = item
                .get("id")
                .and_then(Value::as_str)
                .and_then(|id| order.get(id))
            {
                item.insert("order".into(), (*place).into());
            }
            if let Some(events) = item.get_mut("events").and_then(Value::as_array_mut) {
                for event in events.iter_mut().filter_map(Value::as_object_mut) {
                    if event.get("kind").and_then(Value::as_str) != Some("moved") {
                        continue;
                    }
                    for side in ["from", "to"] {
                        let Some(category) = event.get(side).and_then(Value::as_str) else {
                            continue;
                        };
                        let (status, label) = convert(Some(category));
                        event.insert(side.into(), status.into());
                        if let Some(label) = label {
                            event.insert(format!("{side}Label"), label.into());
                        }
                    }
                }
            }
        }
        if changed {
            store::write(&file, QUEUE, fields)?;
        }
    }
    if categories.try_exists()? {
        files::remove_file_synced(&categories)?;
    }
    finish(root, started)
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::String(text) => !text.is_empty(),
        Value::Number(number) => number.as_f64() != Some(0.0),
        _ => true,
    }
}

fn was_started(item: &Value) -> bool {
    item["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry["session"].as_str())
        .any(|session| {
            item["events"]
                .as_array()
                .into_iter()
                .flatten()
                .rev()
                .filter_map(|event| {
                    let kind = event["kind"].as_str()?;
                    (kind == "done"
                        || (matches!(kind, "started" | "stopped")
                            && event["by"]["session"].as_str() == Some(session)))
                    .then_some(kind)
                })
                .next()
                == Some("started")
        })
}

fn label(id: &str, name: String) -> Value {
    let hash = id.bytes().fold(0u16, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(byte.into())
    });
    let color = ["orange", "pink", "indigo", "mint", "coral", "amber"][usize::from(hash % 6)];
    json!({"id": id, "name": name, "color": color})
}

pub(super) fn read_started(root: &Path) -> Result<BTreeSet<String>, Error> {
    let model = store::read(&root.join("model.json"), MODEL)?.unwrap_or_default();
    Ok(array(model.get("started"))
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect())
}

pub(super) fn forget_started(root: &Path) -> Result<(), Error> {
    let path = root.join("model.json");
    if let Some(mut model) = store::read(&path, MODEL)?
        && model.remove("started").is_some()
    {
        store::write(&path, MODEL, model)?;
    }
    Ok(())
}

fn finish(root: &Path, started: Vec<String>) -> Result<(), Error> {
    let mut fields = Fields::from_iter([
        ("model".into(), 2.into()),
        ("at".into(), Timestamp::now().as_millisecond().into()),
    ]);
    if !started.is_empty() {
        fields.insert("started".into(), json!(started));
    }
    store::write(&root.join("model.json"), MODEL, fields)?;
    Ok(())
}
