use super::{
    migration,
    store::{
        self, COLORS, Error, LABELS, QUEUE, Store, array, label_id, limit, plain_name, queue_files,
    },
};
use http::StatusCode;
use kuvo_protocol::routes::{
    get_kuvo_work_queue_labels as get, post_kuvo_work_queue_labels as post,
};
use kuvo_server::{Access, Caller, Refusal, Registry, RouteReply};
use kuvo_setup::{QueueLabel, QueueLabelLibrary, QueueSetup, SetupHandle};
use kuvo_store::{
    format::Fields,
    locks::{OnBusy, StaleLock},
};
use serde_json::{Value, json};
use std::{collections::HashSet, time::Duration};
use unicode_normalization::UnicodeNormalization;

struct Kept {
    library: QueueLabelLibrary,
    metadata: Fields,
}

pub(super) fn shown(store: &Store, setup: Option<&QueueSetup>) -> Result<Vec<QueueLabel>, Error> {
    if let Some(accepted) = setup.and_then(|setup| setup.labels.as_ref()) {
        return Ok(accepted
            .library
            .labels
            .iter()
            .filter_map(|label| clean_label(&label.id, &label.name, &label.color))
            .collect());
    }
    Ok(local(store, setup)?.library.labels)
}

pub(super) fn clean_label(id: &str, name: &str, color: &str) -> Option<QueueLabel> {
    if !label_id(id) {
        return None;
    }
    let name = limit(&plain_name(name), 40);
    let name = if name.is_empty() {
        id[4..].to_owned()
    } else {
        name
    };
    Some(QueueLabel {
        id: id.into(),
        name,
        color: if COLORS.contains(&color) {
            color.into()
        } else {
            "graphite".into()
        },
    })
}

fn local(store: &Store, setup: Option<&QueueSetup>) -> Result<Kept, Error> {
    let metadata = store::read(&store.root.join("labels.json"), LABELS)?.unwrap_or_default();
    let current = match (metadata.get("removedSetup"), setup) {
        (Some(Value::Null), None) => true,
        (Some(Value::String(id)), Some(setup)) => setup.id == *id,
        _ => false,
    };
    let removed = if current {
        array(metadata.get("removed"))
            .iter()
            .filter_map(Value::as_str)
            .filter(|id| label_id(id))
            .map(str::to_owned)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let mut seen: HashSet<_> = removed.iter().cloned().collect();
    let labels = array(metadata.get("labels"))
        .iter()
        .filter_map(|label| {
            let label = clean_label(
                label["id"].as_str()?,
                label["name"].as_str()?,
                label["color"].as_str().unwrap_or_default(),
            )?;
            seen.insert(label.id.clone()).then_some(label)
        })
        .collect();
    Ok(Kept {
        library: QueueLabelLibrary { labels, removed },
        metadata,
    })
}

enum Edit {
    Add {
        name: String,
        slug: String,
        color: Option<String>,
    },
    Change {
        which: Value,
        name: Option<String>,
        color: Option<String>,
    },
    Remove {
        which: Value,
    },
}

impl Edit {
    fn parse(body: &Value) -> Result<Self, String> {
        if let Some(add) = body["add"].as_str() {
            let name = limit(&plain_name(add), 40);
            if name.is_empty() {
                return Err("Say what the label is called.".into());
            }
            let slug = slug_of(&name);
            if slug.is_empty() {
                return Err(format!(
                    "{name} can't be a label's name: use letters or numbers."
                ));
            }
            let color = body
                .get("color")
                .filter(|color| !color.is_null() && color.as_str() != Some(""))
                .map(color_of)
                .transpose()?;
            Ok(Self::Add { name, slug, color })
        } else if let Some(which) = body.get("change") {
            let name = body.get("name").map(|value| {
                limit(
                    &plain_name(&if value.is_null() {
                        String::new()
                    } else {
                        words(value)
                    }),
                    40,
                )
            });
            if let Some(name) = &name {
                if name.is_empty() {
                    return Err("A label's name can't be empty.".into());
                }
                if slug_of(name).is_empty() {
                    return Err(format!(
                        "{name} can't be a label's name: use letters or numbers."
                    ));
                }
            }
            let color = body.get("color").map(color_of).transpose()?;
            if name.is_none() && color.is_none() {
                return Err("Say its new name (name), its new colour (color), or both.".into());
            }
            Ok(Self::Change {
                which: which.clone(),
                name,
                color,
            })
        } else if let Some(which) = body.get("remove") {
            Ok(Self::Remove {
                which: which.clone(),
            })
        } else {
            Err("Send {add, color?}, {change, name?, color?} or {remove}.".into())
        }
    }

    fn apply(&self, library: &mut QueueLabelLibrary) -> Result<QueueLabel, String> {
        match self {
            Self::Add { name, slug, color } => {
                if named(&library.labels, &Value::String(name.clone())).is_some() {
                    return Err(format!("There's a label called {name} already."));
                }
                if library.labels.len() >= 60 {
                    return Err("There are 60 labels already: remove one first.".into());
                }
                let mut id = format!("lbl_{slug}");
                for index in 2.. {
                    if !library.removed.contains(&id)
                        && !library.labels.iter().any(|label| label.id == id)
                    {
                        break;
                    }
                    id = format!("lbl_{}-{index}", limit(slug, 36));
                }
                let color = color.clone().unwrap_or_else(|| {
                    COLORS[..COLORS.len() - 1]
                        .iter()
                        .min_by_key(|color| {
                            library
                                .labels
                                .iter()
                                .filter(|label| label.color == **color)
                                .count()
                        })
                        .expect("Work Queue label colors")
                        .to_string()
                });
                let label = QueueLabel {
                    id,
                    name: name.clone(),
                    color,
                };
                library.labels.push(label.clone());
                Ok(label)
            }
            Self::Change { which, name, color } => {
                let index = named_or_missing(&library.labels, which)?;
                if let Some(name) = name
                    && named(&library.labels, &Value::String(name.clone()))
                        .is_some_and(|other| other != index)
                {
                    return Err(format!("There's a label called {name} already."));
                }
                let label = &mut library.labels[index];
                if let Some(name) = name {
                    label.name.clone_from(name);
                }
                if let Some(color) = color {
                    label.color.clone_from(color);
                }
                Ok(label.clone())
            }
            Self::Remove { which } => {
                let index = named_or_missing(&library.labels, which)?;
                let label = library.labels.remove(index);
                if !library.removed.contains(&label.id) {
                    library.removed.push(label.id.clone());
                }
                Ok(label)
            }
        }
    }

    fn removal(&self) -> bool {
        matches!(self, Self::Remove { .. })
    }
}

fn named(labels: &[QueueLabel], which: &Value) -> Option<usize> {
    let text = which.as_str()?.trim().to_lowercase();
    if text.is_empty() {
        return None;
    }
    labels
        .iter()
        .position(|label| label.id == text)
        .or_else(|| {
            labels
                .iter()
                .position(|label| label.name.to_lowercase() == text)
        })
}

fn named_or_missing(labels: &[QueueLabel], which: &Value) -> Result<usize, String> {
    named(labels, which).ok_or_else(|| {
        let text = if which.is_null() {
            String::new()
        } else {
            words(which)
        };
        format!("There's no label {text}.")
    })
}

fn slug_of(name: &str) -> String {
    let normalized = name.to_lowercase().nfkd().collect::<String>();
    let words = normalized
        .split(|ch: char| !ch.is_ascii_lowercase() && !ch.is_ascii_digit())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    limit(&words, 40)
}

fn color_of(value: &Value) -> Result<String, String> {
    let name = value.as_str().unwrap_or_default().trim().to_lowercase();
    if matches!(name.as_str(), "gray" | "grey") {
        return Ok("graphite".into());
    }
    if COLORS.contains(&name.as_str()) {
        return Ok(name);
    }
    Err(format!(
        "{} isn't a label's colour: use {}.",
        words(value),
        COLORS.join(", ")
    ))
}

fn words(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| {
                if item.is_null() {
                    String::new()
                } else {
                    words(item)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
        _ => value.to_string(),
    }
}

pub(super) fn strip(store: &Store, removed: &[String]) -> Result<(), Error> {
    if removed.is_empty() {
        return Ok(());
    }
    migration::migrate(&store.root)?;
    let _queues = StaleLock::acquire(
        &store.root.join(".lock"),
        Duration::from_secs(10),
        Duration::from_secs(3),
        OnBusy::Refuse,
    )?;
    for file in queue_files(&store.root.join("queues")) {
        if !file
            .file_stem()
            .and_then(|name| name.to_str())
            .is_some_and(store::owner_id)
        {
            continue;
        }
        let Some(mut fields) = store::read(&file, QUEUE)? else {
            continue;
        };
        let mut changed = false;
        for item in fields
            .get_mut("items")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            if let Some(labels) = item.get_mut("labels").and_then(Value::as_array_mut) {
                let before = labels.len();
                labels.retain(|label| {
                    !label
                        .as_str()
                        .is_some_and(|label| removed.iter().any(|id| id == label))
                });
                changed |= before != labels.len();
            }
        }
        if changed {
            store::write(&file, QUEUE, fields)?;
        }
    }
    Ok(())
}

fn local_save(store: &Store, edit: Edit) -> Result<Value, Refusal> {
    migration::migrate(&store.root).map_err(super::routes::storage_refusal)?;
    let removal = edit.removal();
    if let Edit::Remove { which } = &edit {
        let kept = local(store, None).map_err(super::routes::storage_refusal)?;
        let index = named_or_missing(&kept.library.labels, which).map_err(bad)?;
        strip(store, &[kept.library.labels[index].id.clone()])
            .map_err(super::routes::storage_refusal)?;
    }
    let _lock = StaleLock::acquire(
        &store.root.join(".labels.lock"),
        Duration::from_secs(10),
        Duration::from_secs(3),
        OnBusy::Refuse,
    )
    .map_err(super::routes::internal)?;
    let mut kept = local(store, None).map_err(super::routes::storage_refusal)?;
    let done = edit.apply(&mut kept.library).map_err(bad)?;
    let mut fields = Fields::from_iter([("labels".into(), json!(kept.library.labels))]);
    if !kept.library.removed.is_empty() {
        fields.insert("removed".into(), json!(kept.library.removed));
        fields.insert("removedSetup".into(), Value::Null);
    }
    let from_primary: Vec<_> = array(kept.metadata.get("fromPrimary"))
        .iter()
        .filter(|id| {
            id.as_str()
                .is_some_and(|id| kept.library.labels.iter().any(|label| label.id == id))
        })
        .collect();
    if !from_primary.is_empty() {
        fields.insert("fromPrimary".into(), json!(from_primary));
    }
    for name in ["primary", "primaryAt"] {
        if let Some(value) = kept.metadata.remove(name) {
            fields.insert(name.into(), value);
        }
    }
    store::write(&store.root.join("labels.json"), LABELS, fields)
        .map_err(super::routes::internal)?;
    Ok(answer(kept.library.labels, done, removal))
}

fn answer(labels: Vec<QueueLabel>, done: QueueLabel, removal: bool) -> Value {
    let mut value = json!({"labels": labels});
    if !removal {
        value["label"] = json!(done);
    }
    value
}

fn bad(message: String) -> Refusal {
    Refusal::new(StatusCode::BAD_REQUEST, message)
}

async fn managed_save(
    store: Store,
    setup: SetupHandle,
    selected: QueueSetup,
    caller: Caller,
    edit: Edit,
) -> Result<RouteReply<post::Spec>, Refusal> {
    let removal = edit.removal();
    let fixture = store.clone();
    let selection = selected.clone();
    let (library, done) = tokio::task::spawn_blocking(move || {
        migration::migrate(&fixture.root).map_err(super::routes::storage_refusal)?;
        let mut library = match selection.labels {
            Some(accepted) => accepted.library,
            None => {
                local(&fixture, Some(&selection))
                    .map_err(super::routes::storage_refusal)?
                    .library
            }
        };
        let done = edit.apply(&mut library).map_err(bad)?;
        Ok::<_, Refusal>((library, done))
    })
    .await
    .map_err(super::routes::internal)??;
    let receipt = setup
        .save_queue_labels(&caller, &selected, library.clone())
        .await?;
    if removal {
        let gone = done.id.clone();
        super::routes::blocking(move || strip(&store, &[gone])).await?;
    }
    let response: post::Status200 = serde_json::from_value(answer(library.labels, done, removal))
        .map_err(super::routes::internal)?;
    RouteReply::json_with::<200>(
        response,
        [
            ("setup", receipt.setup.into()),
            ("epoch", receipt.epoch.into()),
            ("operation", receipt.operation.into()),
            ("status", receipt.status.into()),
            ("proof", json!(receipt.proof)),
        ],
    )
    .map_err(super::routes::internal)
}

pub(super) fn register(registry: &mut Registry, store: Store, setup: SetupHandle) {
    let here = store.clone();
    let accepted = setup.clone();
    registry.register::<get::Spec, _, _>(Access::Any, move |_| {
        let store = here.clone();
        let selected = accepted.queue_setup();
        async move {
            super::routes::blocking(move || {
                Ok(json!({"labels": shown(&store, selected.as_ref())?}))
            })
            .await
            .and_then(super::routes::typed)
        }
    });

    registry.register::<post::Spec, _, _>(Access::Any, move |call| {
        let store = store.clone();
        let setup = setup.clone();
        async move {
            let caller = call.request.caller;
            if !super::routes::work_queue_client(&caller) {
                return Err(Refusal::new(StatusCode::FORBIDDEN,
                    "Only Kuvo Code on your Mac or iPhone can do this: agents change the Work Queue with their tools."));
            }
            let body = serde_json::from_slice(&call.request.body).unwrap_or(Value::Null);
            let edit = Edit::parse(&body).map_err(bad)?;
            if let Some(selected) = setup.queue_setup() {
                return managed_save(store, setup, selected, caller, edit).await;
            }
            tokio::task::spawn_blocking(move || local_save(&store, edit)).await
                .map_err(super::routes::internal)?.and_then(super::routes::typed)
        }
    });
}
