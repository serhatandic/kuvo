use super::*;
use crate::queue::{github::Seen, mentions::Index};
use kuvo_protocol::routes::patch_kuvo_work_queue_items_item as patch;

impl Writer {
    pub(super) fn register_edits(&self, registry: &mut Registry) {
        let writer = self.clone();
        registry.register::<patch::Spec, _, _>(Access::Any, move |call| {
            let writer = writer.clone();
            let reference = super::super::routes::decode(call.param("item"));
            async move {
                if !work_queue_client(&call.request.caller) {
                    return Err(Refusal::new(StatusCode::FORBIDDEN, CLIENT));
                }
                let fields: Value =
                    serde_json::from_slice(&call.request.body).unwrap_or(Value::Null);
                if store::item_id(&reference) && fields.get("baseRevision").is_some() {
                    return offline::reply(
                        writer
                            .ordinary_edit(
                                &call.request.caller,
                                &call.request.headers,
                                &reference,
                                fields,
                                false,
                                false,
                            )
                            .await?,
                    );
                }
                typed(
                    writer
                        .edit_for_client(
                            reference,
                            fields,
                            local_work_queue_app(&call.request.caller),
                        )
                        .await?,
                )
            }
        });
    }

    pub(super) async fn edit(
        &self,
        reference: String,
        fields: Value,
        local: bool,
    ) -> Result<Value, Refusal> {
        let (fields, staged) =
            if fields.get("itemRelation").is_some() || fields.get("files").is_none() {
                (fields, Staged::default())
            } else {
                let writer = self.clone();
                tokio::task::spawn_blocking(move || {
                    let staged = writer
                        .files
                        .stage(&writer.store, fields.get("files"), local)?;
                    Ok::<_, Refusal>((fields, staged))
                })
                .await
                .map_err(internal)??
            };
        let guard = self.lock().await?;
        let writer = self.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            let now = Timestamp::now().as_millisecond();
            let reference = writer
                .refs
                .parse(&reference)
                .ok_or_else(|| Refusal::new(StatusCode::NOT_FOUND, "There's no such item."))?;
            let (all, compaction) = writer.changes(now)?;
            let item = writer
                .refs
                .find(&all, &reference)
                .ok_or_else(|| Refusal::new(StatusCode::NOT_FOUND, "There's no item like that."))?;
            writer.admit(&item.owner)?;
            refuse_handing(item)?;
            let people = Assignees::read(&writer.store);
            let answer = if let Some(relation) = fields.get("itemRelation") {
                writer.relate(&all, item, relation, now, &people)?
            } else {
                if item.removed.is_some() {
                    return Err(bad("It's deleted: bring it back first."));
                }
                writer.edit_fields(
                    &all,
                    item,
                    &fields,
                    &staged,
                    now,
                    &people,
                    &json!({"user":true}),
                    |row| {
                        writer.observe_known(row, &item.id);
                        Ok(())
                    },
                )?
            };
            compaction.finish(&writer, now)?;
            Ok(answer)
        })
        .await
        .map_err(internal)?
    }

    fn relate(
        &self,
        all: &[Item],
        item: &Item,
        directive: &Value,
        now: i64,
        people: &Assignees,
    ) -> Result<Value, Refusal> {
        let other = directive["item"]
            .as_str()
            .filter(|_| item.removed.is_none())
            .ok_or_else(|| bad("Choose two items that are still in the Work Queue."))?;
        let other = all
            .iter()
            .find(|other_item| other_item.id == other && other_item.removed.is_none())
            .ok_or_else(|| bad("Choose an item kept on the same computer and try again."))?;
        self.admit(&other.owner)?;
        refuse_handing(other)?;
        if other.id == item.id {
            return Err(bad("Choose a different item."));
        }
        let kind = directive
            .get("kind")
            .and_then(|kind| {
                if kind.is_null() {
                    return Some(None);
                }
                kind.as_str()
                    .filter(|kind| {
                        matches!(*kind, "related" | "blocks" | "blocked-by" | "duplicate-of")
                    })
                    .map(Some)
            })
            .ok_or_else(|| bad("Choose a kind of link and try again."))?;
        let (holder, counterpart) = if item.id < other.id {
            (item, other)
        } else {
            (other, item)
        };
        let mut document = self.read_owner(&holder.owner, now)?;
        let saved = document
            .items
            .iter_mut()
            .find(|candidate| candidate["id"] == holder.id)
            .ok_or_else(|| bad("Choose an item kept on the same computer and try again."))?;
        let mut links: Vec<_> = store::array(saved.get("relations"))
            .iter()
            .filter(|link| link["item"] != counterpart.id)
            .cloned()
            .collect();
        if let Some(kind) = kind {
            let kind = if holder.id == item.id {
                kind
            } else {
                super::super::refs::inverse(kind).expect("validated relation kind")
            };
            links.push(json!({"item":counterpart.id,"kind":kind}));
        }
        if links.is_empty() {
            saved
                .as_object_mut()
                .expect("queue item")
                .remove("relations");
        } else {
            saved["relations"] = links.into();
        }
        document.save(&self.store).map_err(storage_refusal)?;
        let mut answer = view::mutation(item, people);
        answer["itemRelationApplied"] = true.into();
        Ok(answer)
    }

    pub(super) fn edit_fields(
        &self,
        all: &[Item],
        item: &Item,
        fields: &Value,
        staged: &Staged,
        now: i64,
        people: &Assignees,
        who: &Value,
        revise: impl FnOnce(&mut Value) -> Result<(), Refusal>,
    ) -> Result<Value, Refusal> {
        self.edit_row(
            all,
            item,
            fields,
            staged,
            now,
            people,
            who,
            Value::Object(item.saved.clone()),
            |row, _| revise(row),
        )
    }

    pub(super) fn edit_row(
        &self,
        all: &[Item],
        item: &Item,
        fields: &Value,
        staged: &Staged,
        now: i64,
        people: &Assignees,
        who: &Value,
        mut row: Value,
        revise: impl FnOnce(
            &mut Value,
            &mut std::collections::BTreeMap<String, Document>,
        ) -> Result<(), Refusal>,
    ) -> Result<Value, Refusal> {
        let mut documents = std::collections::BTreeMap::<String, Document>::new();
        let previous = item.status.as_str();
        let new_status = fields
            .get("status")
            .map(status_of)
            .transpose()?
            .unwrap_or(previous);
        let status_changed = previous != new_status;
        let to_top = fields["top"].as_bool() == Some(true);
        let stamp = json!({"at":now,"by":who});
        if let Some(source) = row.get("source") {
            let display = source["ref"].as_str().unwrap_or_default();
            let title = row["title"].as_str().unwrap_or(&item.title);
            let name = format!("{display} \"{title}\"");
            if fields.get("title").is_some()
                || fields.get("body").is_some()
                || fields.get("removeInlineImages").is_some()
            {
                return Err(bad(format!(
                    "{name} takes its title and description from GitHub: change them there."
                )));
            }
            if !staged.is_empty() && who["user"] == true {
                return Err(bad(format!(
                    "{name} takes its description from GitHub: attach files there."
                )));
            }
            let mut projection = row.clone();
            Seen::read(&self.store)
                .map_err(storage_refusal)?
                .apply(&self.refs, &mut projection);
            if projection.get("keptIn").is_some()
                && (who["user"] == true || fields.get("status").is_some())
            {
                return Err(bad(if projection["seen"]["gone"] == true {
                    format!(
                        "{name} is gone from GitHub (or gh here can no longer see it), so it's Done: it's back where it stood once GitHub shows it again."
                    )
                } else {
                    format!(
                        "{name} is closed on GitHub, so it's Done: it's back where it stood once it's reopened there."
                    )
                }));
            }
            if fields.get("status").is_some() && status::closed(new_status) {
                return Err(bad(format!(
                    "{name} is Done or Canceled once it's closed on GitHub (or, a pull request, merged): close it there, or let the pull request that fixes it close it."
                )));
            }
        }
        if let Some(value) = fields.get("title") {
            let title = limit(&plain_name(&text(value, 800, "The title")?), 200);
            if title.is_empty() {
                return Err(bad("A title can't be empty."));
            }
            row["title"] = title.into();
        }
        if let Some(value) = fields.get("body") {
            row["body"] = text(value, 100_000, "The description (body)")?.into();
        }
        if let Some(removals) = fields.get("removeInlineImages") {
            let urls = pictures_to_remove(item, removals)?;
            let body = row["body"].as_str().unwrap_or_default().to_owned();
            row["body"] = self.refs.mentions.without_pictures(body, &urls).into();
        }
        if status_changed {
            row["status"] = new_status.into();
            if status::closed(new_status) {
                row["doneAt"] = now.into();
                row["sessions"] = json!([]);
            } else {
                row.as_object_mut().expect("queue item").remove("doneAt");
            }
        }
        if let Some(value) = fields.get("priority") {
            change_or_remove(&mut row, "priority", priority_of(value)?.map(Value::from));
        }
        if let Some(value) = fields.get("due") {
            change_or_remove(&mut row, "due", due_of(value)?.map(Value::from));
        }
        if let Some(value) = fields.get("labels") {
            let setup = self.setup.queue_setup();
            let book = labels::shown(&self.store, setup.as_ref()).map_err(storage_refusal)?;
            row["labels"] = json!(labels_of(value, &book)?);
        }
        if fields.get("assignee").is_some() {
            let target = assignee(fields, people)?;
            change_or_remove(&mut row, "assignee", target);
        }

        // Allocate an order between neighbors, renumbering a dense status first.
        let same_status = |other: &&Item| {
            other.id != item.id && other.removed.is_none() && other.status == new_status
        };
        let old_index = all
            .iter()
            .filter(|other| {
                other.id != item.id
                    && other.status == previous
                    && other.removed.is_none()
                    && other.order < item.order
            })
            .count();
        let mut peers: Vec<_> = all
            .iter()
            .filter(same_status)
            .map(|other| (other, other.order))
            .collect();
        peers.sort_by(|a, b| a.1.total_cmp(&b.1));
        let before = fields.get("before");
        let after = fields.get("after");
        let order = if let Some(named) = before.or(after) {
            let named = match named {
                Value::String(text) => text.clone(),
                Value::Number(value)
                    if value
                        .as_u64()
                        .is_some_and(|value| (1..1_000_000).contains(&value)) =>
                {
                    value.to_string()
                }
                _ => return Err(bad("There's no item like that to put it beside.")),
            };
            let reference = self
                .refs
                .parse(&named)
                .ok_or_else(|| bad("There's no item like that to put it beside."))?;
            let beside = self
                .refs
                .named(all, &reference)
                .filter(|beside| beside.id != item.id && beside.removed.is_none())
                .min_by_key(|beside| beside.status != new_status)
                .ok_or_else(|| bad(format!("There's no other item {named} to put it beside.")))?;
            if beside.status != new_status {
                let display = item_name(beside);
                let status_name = |id: &str| {
                    STATUSES
                        .iter()
                        .find(|status| status.id == id)
                        .map(|status| status.name.to_owned())
                        .unwrap_or_else(|| id.to_owned())
                };
                return Err(bad(format!(
                    "{display} is {}, not {}: give it that status first (status).",
                    status_name(&beside.status),
                    status_name(new_status)
                )));
            }
            let is_after = before.is_none();
            let anchor = peers
                .iter()
                .position(|(other, _)| other.id == beside.id)
                .expect("same status peer");
            let neighbor = |entries: &[(&Item, f64)]| {
                let current = entries[anchor].1;
                if is_after {
                    entries
                        .iter()
                        .find(|(_, order)| *order > current)
                        .map(|(_, order)| *order)
                } else {
                    entries
                        .iter()
                        .rev()
                        .find(|(_, order)| *order < current)
                        .map(|(_, order)| *order)
                }
            };
            if neighbor(&peers).is_some_and(|order| (order - peers[anchor].1).abs() < 1e-6) {
                let moving = status_changed
                    .then(|| place(all, Some(&item.id), new_status, to_top))
                    .unwrap_or(item.order);
                let before_moving = peers
                    .iter()
                    .filter(|(other, order)| {
                        *order < moving
                            || (*order == moving
                                && all.iter().position(|candidate| candidate.id == other.id)
                                    < all.iter().position(|candidate| candidate.id == item.id))
                    })
                    .count();
                for (index, (other, order)) in peers.iter_mut().enumerate() {
                    *order = (index + usize::from(index >= before_moving)) as f64;
                    let document = self.document(&mut documents, &other.owner, now)?;
                    let kept = document
                        .items
                        .iter_mut()
                        .find(|row| row["id"] == other.id)
                        .expect("reordered row");
                    kept["order"] = json!(*order);
                }
            }
            let base = peers[anchor].1;
            neighbor(&peers).map_or_else(
                || if is_after { base + 1.0 } else { base - 1.0 },
                |next| (base + next) / 2.0,
            )
        } else if status_changed || (fields.get("status").is_none() && to_top) {
            place(all, Some(&item.id), new_status, to_top)
        } else {
            item.order
        };
        row["order"] = json!(order);
        let new_index = peers.iter().filter(|(_, before)| *before < order).count();
        if status_changed && new_status == "done" {
            append_event(&mut row, json!({"at":now,"by":who,"kind":"done"}));
        } else if status_changed || new_index != old_index {
            let mut moved =
                json!({"at":now,"by":who,"kind":"moved","from":previous,"to":new_status});
            if new_index == 0 && (to_top || before.is_some() || after.is_some()) {
                moved["top"] = true.into();
            }
            append_event(&mut row, moved);
        }
        row["updated"] = stamp;
        let removed = take_files(
            &mut row,
            fields.get("remove").filter(|_| who["user"] != true),
        );
        let published = self.files.publish(
            &self.store,
            &mut row,
            staged,
            fields.get("body").is_some(),
            now,
        )?;
        revise(&mut row, &mut documents)?;
        super::super::attachments::remove(&self.store, &item.id, &removed).map_err(internal)?;
        let parsed = Item::read(&item.owner, &row).expect("changed queue item");
        let before = Index::read(item);
        if parsed.body != item.body || parsed.notes != item.notes || before.is_none() {
            let index = Index::rebuild(&self.refs.mentions, &parsed, all, before.as_ref());
            row["mentionIndex"] = json!(index);
            row.as_object_mut()
                .expect("queue item")
                .remove("mentionLinks");
        }
        let parsed = Item::read(&item.owner, &row).expect("changed queue item");
        let mut answer = view::mutation(&parsed, people);
        if fields.get("removeInlineImages").is_some() {
            answer["inlineImagesApplied"] = true.into();
        }
        let document = self.document(&mut documents, &item.owner, now)?;
        *document
            .items
            .iter_mut()
            .find(|saved| saved["id"] == item.id)
            .expect("changed queue item") = row;
        let mut published = Some(published);
        for document in documents.into_values() {
            let edited = document.owner == item.owner;
            document.save(&self.store).map_err(storage_refusal)?;
            if edited && let Some(attachments) = published.take() {
                attachments.commit();
            }
        }
        Ok(answer)
    }

    pub(super) fn document<'a>(
        &self,
        documents: &'a mut std::collections::BTreeMap<String, Document>,
        owner: &str,
        now: i64,
    ) -> Result<&'a mut Document, Refusal> {
        use std::collections::btree_map::Entry;
        Ok(match documents.entry(owner.into()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(self.read_owner(owner, now)?),
        })
    }
}

fn take_files(row: &mut Value, given: Option<&Value>) -> Vec<String> {
    let Some(given) = given else {
        return Vec::new();
    };
    let names = given
        .as_array()
        .map_or(std::slice::from_ref(given), Vec::as_slice);
    let mut removed = Vec::new();
    let mut kept = store::array(row.get("files")).to_vec();
    kept.retain(|file| {
        if names.contains(&file["name"])
            && let Some(name) = file["name"].as_str()
        {
            removed.push(name.to_owned());
            false
        } else {
            true
        }
    });
    row["files"] = kept.into();
    removed
}

fn item_name(item: &Item) -> String {
    if let Some(source) = item.source() {
        format!(
            "{} \"{}\"",
            source["ref"].as_str().unwrap_or_default(),
            item.title
        )
    } else if let Some(number) = item.number {
        format!("#{number} \"{}\"", item.title)
    } else {
        let waiting = item
            .saved
            .get("wanted")
            .and_then(Value::as_u64)
            .map_or_else(String::new, |number| format!(": asked for #{number}"));
        format!("\"{}\" ({}, no number yet{waiting})", item.title, item.id)
    }
}

pub(super) fn refuse_handing(item: &Item) -> Result<(), Refusal> {
    if let Some(handing) = item.saved.get("handing") {
        let to = handing["name"]
            .as_str()
            .filter(|name| !name.is_empty())
            .or_else(|| handing["to"].as_str())
            .unwrap_or_default();
        return Err(bad(format!(
            "{} is being handed to {to} on another computer: change it there once it's arrived.",
            item_name(item)
        )));
    }
    Ok(())
}

fn change_or_remove(row: &mut Value, field: &str, value: Option<Value>) {
    if let Some(value) = value {
        row[field] = value;
    } else {
        row.as_object_mut().expect("queue item").remove(field);
    }
}

pub(super) fn pictures_to_remove<'a>(
    item: &Item,
    removals: &'a Value,
) -> Result<std::collections::HashSet<&'a str>, Refusal> {
    let invalid = || bad("Choose pictures to remove and try again.");
    let removals = removals
        .as_array()
        .filter(|removals| removals.len() <= 50)
        .ok_or_else(invalid)?;
    let images: Vec<_> = item
        .files
        .iter()
        .filter(|file| {
            file["type"]
                .as_str()
                .is_some_and(|kind| kind.starts_with("image/"))
        })
        .collect();
    let mut urls = std::collections::HashSet::new();
    for removal in removals {
        let url = removal["url"].as_str().ok_or_else(invalid)?;
        let name = removal["name"].as_str().ok_or_else(invalid)?;
        let known = if let Some(anchor) = url.strip_prefix("kuvo-attachment:file/") {
            anchor.len() == 24
                && anchor
                    .bytes()
                    .all(|ch| ch.is_ascii_digit() || (b'a'..=b'f').contains(&ch))
                && images
                    .iter()
                    .any(|file| file["anchor"] == anchor && file["name"] == name)
        } else if let Some(position) = url.strip_prefix("kuvo-attachment:legacy/") {
            (1..=3).contains(&position.len())
                && position.bytes().all(|ch| ch.is_ascii_digit())
                && position
                    .parse::<usize>()
                    .ok()
                    .and_then(|position| position.checked_sub(1))
                    .and_then(|index| images.get(index))
                    .is_some_and(|file| file["name"] == name)
        } else if let Some((index, encoded)) = url
            .strip_prefix("kuvo-attachment:draft/")
            .and_then(|draft| draft.split_once('/'))
        {
            !index.is_empty()
                && index.bytes().all(|ch| ch.is_ascii_digit())
                && !encoded.is_empty()
                && !encoded.contains('/')
                && percent_encoding::percent_decode_str(encoded)
                    .decode_utf8()
                    .is_ok_and(|decoded| decoded == name)
                && images.iter().any(|file| file["name"] == name)
        } else {
            false
        };
        if known {
            urls.insert(url);
        }
    }
    Ok(urls)
}
