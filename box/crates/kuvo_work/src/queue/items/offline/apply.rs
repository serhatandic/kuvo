//! Apply a received draft once under the same lock used by ordinary queue operations.

use super::*;
use history::{Decision, History, Outcome};

impl Writer {
    pub(in crate::queue::items) fn observe_known(&self, row: &mut Value, id: &str) {
        if row.get("offlineEdits").is_some() {
            row["offlineEdits"] = json!(self.revisions.observe_item(row, id, None));
        }
    }

    pub(super) async fn apply_draft(
        &self,
        draft: Draft,
        sender: String,
    ) -> Result<Option<Value>, Refusal> {
        let guard = self.lock().await?;
        let writer = self.clone();
        let operation = draft.edit.operation.clone();
        let decision = tokio::task::spawn_blocking(move || {
            let _guard = guard;
            writer.decide_draft(&draft, &sender)
        })
        .await
        .map_err(internal)?;
        match decision {
            Ok(answer) => {
                if answer.as_ref().is_some_and(|answer| {
                    matches!(answer["status"].as_str(), Some("committed" | "choice"))
                }) {
                    self.retell.again();
                }
                Ok(answer)
            }
            Err(error) => {
                use http_body_util::BodyExt;
                let reply = error.reply();
                if reply.status.is_server_error() {
                    return Ok(None);
                }
                let message = reply
                    .body
                    .collect()
                    .await
                    .ok()
                    .and_then(|body| serde_json::from_slice::<Value>(&body.to_bytes()).ok())
                    .and_then(|body| body["message"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| {
                        "This computer could not confirm the current record.".into()
                    });
                Ok(Some(types::interrupted(&operation, &message)))
            }
        }
    }

    fn decide_draft(&self, draft: &Draft, sender: &str) -> Result<Option<Value>, Refusal> {
        let Some(id) = self
            .identity()
            .filter(|_| self.accepts_editor(&draft.setup, sender))
        else {
            return Ok(None);
        };
        let now = Timestamp::now().as_millisecond();
        let (all, compaction) = self.changes(now)?;
        let decision = self.decide_change(draft, sender, &id, now, &all)?;
        compaction.finish(self, now)?;
        Ok(decision)
    }

    fn decide_change(
        &self,
        draft: &Draft,
        sender: &str,
        id: &Identity,
        now: i64,
        all: &[Item],
    ) -> Result<Option<Value>, Refusal> {
        let incoming = &draft.edit;
        let mut edit = incoming.clone();
        edit.operation = types::op(&draft.setup, &draft.nonce);
        let digest = types::key(json!(edit));
        let edit_target = &edit.target;
        let item = all.iter().find(|item| item.id == edit_target.id);
        let deletion = if item.is_none() {
            self.fence(Fence::Deleted, &edit_target.id)?
        } else {
            None
        };
        let target = fence_target(edit_target, deletion.as_ref());
        if !self.owns_editor(&target, &id.here, item) {
            return Ok(None);
        }
        let mut row = item
            .map(|item| Value::Object(item.saved.clone()))
            .unwrap_or_default();
        let mut history = if let Some(item) = item {
            self.revisions.observe_item(&mut row, &item.id, None)
        } else {
            self.revisions.absent(&edit_target.id, deletion.is_some())
        };
        let answer_for_sender = |mut decision: Decision| {
            decision.operation = incoming.operation.clone();
            Some(json!(decision))
        };
        if incoming.operation != edit.operation {
            let received = types::key(json!(incoming));
            if let Some(Ok(decision)) = history.replay(&incoming.operation, &received) {
                return Ok(answer_for_sender(decision));
            }
        }
        if let Some(previous) = history.replay(&edit.operation, &digest) {
            return previous
                .map(answer_for_sender)
                .map_err(|()| bad("That operation id is already in use."));
        }
        let view = self.peers.view();
        let mut used = false;
        for key in std::iter::once(sender.to_owned())
            .chain(std::iter::once(id.here.clone()))
            .chain(
                view.roster
                    .members
                    .iter()
                    .map(|member| kuvo_wire::keys::encode(&member.key)),
            )
        {
            let mut previously = incoming.clone();
            previously.operation = types::legacy_op(&draft.setup, &key, &draft.nonce);
            if let Some(receipt) =
                history.replay(&previously.operation, &types::key(json!(previously)))
            {
                match receipt {
                    Ok(decision) => return Ok(answer_for_sender(decision)),
                    Err(()) => used = true,
                }
            }
        }
        if used {
            return Err(bad("That operation id is already in use."));
        }
        if !self.accepts_editor(&draft.setup, sender) {
            return Ok(None);
        }
        match self.authority(draft) {
            PairingState::Allowed => {}
            PairingState::Checking => return Ok(None),
            PairingState::Revoked if draft.submitter.is_none() && draft.replay_only.is_none() => {
                return Err(bad(
                    "This work predates device tracking. Check the record before submitting it again.",
                ));
            }
            PairingState::Revoked => {
                return Err(bad(
                    "The submitting device no longer has access. Check the record before sending again.",
                ));
            }
        }
        let Some(item) = item else {
            if edit.create()
                && deletion.is_none()
                && self.fence(Fence::Departed, &edit_target.id)?.is_none()
            {
                return self
                    .add_offline(&edit, digest, all, now)
                    .map(answer_for_sender);
            }
            return Ok(Some(
                json!({"operation":incoming.operation,"status":"refresh","revision":history.revision(),"choices":[]}),
            ));
        };
        if edit.create() {
            return Err(bad("That operation id is already in use."));
        }
        let baseline = match history.baseline(edit.base_revision.as_deref(), edit.resolve.as_ref())
        {
            Ok(baseline) => baseline,
            Err(outcome) => {
                let decision = remember(&mut history, draft, &edit, digest, outcome, Vec::new());
                row["offlineEdits"] = json!(history);
                self.save_edit_row(&item.owner, &item.id, row, now)?;
                return Ok(answer_for_sender(decision));
            }
        };
        let people = Assignees::read(&self.store);
        let fields = self.normalize_edit(&edit, &people)?;
        let merged = self.revisions.merge(
            &history,
            &edit.operation,
            baseline,
            &fields,
            edit.resolve.as_ref(),
            edit.delete(),
        );
        let additions = self.additions(&edit, &mut history, Some(item))?;
        let staged = self.stage_offline(&additions.files)?;
        let fields = queue_fields(&merged.changes);
        let changes = !merged.changes.is_empty() || !staged.is_empty();
        let previous = history
            .choices
            .iter()
            .map(|choice| choice.id.clone())
            .collect::<HashSet<_>>();
        let write = |row: &mut Value| {
            let mut notes = store::array(row.get("notes")).to_vec();
            for (field, value) in &merged.changes {
                let Some(id) = field.strip_prefix("note:") else {
                    continue;
                };
                let note = notes
                    .iter_mut()
                    .find(|note| note["id"] == id)
                    .ok_or_else(|| bad("That note could not be updated."))?;
                note["text"] = value.clone();
            }
            notes.extend(additions.notes);
            row["notes"] = notes.into();
            history.choices = merged.choices;
            row["offlineEdits"] = json!(history);
            let mut history = self.revisions.observe_item(row, &item.id, merged.layout);
            if history.choices.iter().any(|choice| {
                !previous.contains(&choice.id)
                    && choice.proposed.as_str().is_some_and(|text| {
                        let max = if choice.field.starts_with("note:") {
                            20_000
                        } else if choice.field == "body" {
                            100_000
                        } else {
                            usize::MAX
                        };
                        text.encode_utf16().count() > max
                    })
            }) {
                return Err(bad("The combined alternative would exceed its text limit."));
            }
            let decision = remember(
                &mut history,
                draft,
                &edit,
                digest,
                merged.status,
                merged.added,
            );
            row["offlineEdits"] = json!(history);
            Ok(decision)
        };
        if edit.delete() {
            row["removed"] = json!({"at":now,"by":{"user":true}});
            row["updated"] = row["removed"].clone();
            row["sessions"] = json!([]);
            event(&mut row, now, "deleted");
        }
        let decision = if changes {
            if item.source().is_some()
                && (fields.get("body").is_some()
                    || fields.get("title").is_some()
                    || !staged.is_empty())
            {
                return Err(bad(
                    "Change that item's title, description or attachments at its source.",
                ));
            }
            // Legacy notes were assigned stable IDs when the edit baseline was observed.
            let baseline = Item::read(&item.owner, &row).expect("queue item");
            let mut decision = None;
            self.edit_fields(
                &baseline,
                &fields,
                &staged,
                ChangeContext {
                    all,
                    people: &people,
                    now,
                    who: &json!({"user":true}),
                },
                |row| {
                    decision = Some(write(row)?);
                    Ok(())
                },
            )?;
            decision.expect("saved offline decision")
        } else {
            let decision = write(&mut row)?;
            self.save_edit_row(&item.owner, &item.id, row, now)?;
            decision
        };
        Ok(answer_for_sender(decision))
    }

    fn normalize_edit(&self, edit: &Edit, people: &Assignees) -> Result<Fields, Refusal> {
        let mut kept = Fields::new();
        let invalid = || bad("That is not an editable Work Queue field or value.");
        for (field, value) in edit.fields.iter().flatten() {
            let value = match field.as_str() {
                "title" => {
                    let title = value
                        .as_str()
                        .filter(|title| title.encode_utf16().count() <= 800)
                        .map(|title| limit(&plain_name(title), 200))
                        .filter(|title| !title.is_empty())
                        .ok_or_else(|| bad("Say a title in one line."))?;
                    title.into()
                }
                "body" => value
                    .as_str()
                    .filter(|text| text.encode_utf16().count() <= 100_000)
                    .ok_or_else(invalid)?
                    .into(),
                "status" => status_of(value)?.into(),
                "priority" => priority_of(value)?.map_or(Value::Null, Value::from),
                "labels" => {
                    let setup = self.setup.queue_setup();
                    let labels =
                        labels::shown(&self.store, setup.as_ref()).map_err(storage_refusal)?;
                    json!(labels_of(value, &labels)?)
                }
                "assignee" => assignee(&json!({"assignee":value}), people)?.unwrap_or_default(),
                "due" => due_of(value)?.map_or(Value::Null, Value::from),
                "order" => {
                    let valid = value.as_object().is_some_and(|order| order.len() == 1)
                        && (value["top"] == true
                            || value["before"].is_string()
                            || value["after"].is_string());
                    if !valid {
                        return Err(invalid());
                    }
                    value.clone()
                }
                note if !edit.create() && note.strip_prefix("note:").is_some_and(types::token) => {
                    value
                        .as_str()
                        .filter(|text| {
                            !text.trim().is_empty() && text.encode_utf16().count() <= 20_000
                        })
                        .ok_or_else(invalid)?
                        .into()
                }
                _ => return Err(invalid()),
            };
            kept.insert(field.clone(), value);
        }
        Ok(kept)
    }

    fn additions(
        &self,
        edit: &Edit,
        history: &mut History,
        item: Option<&Item>,
    ) -> Result<Additions, Refusal> {
        let mut notes = Vec::new();
        let mut files = Vec::new();
        let now = Timestamp::now().as_millisecond();
        for note in edit.notes.iter().flatten() {
            if addition(history, format!("note:{}", note.id), json!(note))? {
                if item.is_some_and(|item| item.notes.iter().any(|other| other["id"] == note.id)) {
                    return Err(bad("That note id is already in use."));
                }
                notes.push(json!({"id":note.id,"text":note.text,"at":now,"by":{"user":true}}));
            }
        }
        if item.map_or(0, |item| item.notes.len()) + notes.len() > 200 {
            return Err(bad("This item has reached its note limit."));
        }
        for file in edit.attachments.iter().flatten() {
            if addition(history, format!("attachment:{}", file.id), json!(file))? {
                files.push(file.clone());
            }
        }
        Ok(Additions { notes, files })
    }

    fn stage_offline(&self, attachments: &[types::Attachment]) -> Result<Staged, Refusal> {
        if attachments.is_empty() {
            return Ok(Staged::default());
        }
        let files = json!(
            attachments
                .iter()
                .map(|attachment| &attachment.file)
                .collect::<Vec<_>>()
        );
        Ok(self
            .files
            .stage(&self.store, Some(&files), false)?
            .with_ids(attachments.iter().map(|file| file.id.clone())))
    }

    fn add_offline(
        &self,
        edit: &Edit,
        digest: String,
        all: &[Item],
        now: i64,
    ) -> Result<Decision, Refusal> {
        let owner = &edit.target.queue;
        let people = Assignees::read(&self.store);
        let fields = self.normalize_edit(edit, &people)?;
        let mut history = self.revisions.absent(&edit.target.id, false);
        let add = self.additions(edit, &mut history, None)?;
        let status = fields
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("todo");
        let order = place(
            all,
            None,
            status,
            fields
                .get("order")
                .is_some_and(|order| order["top"] == true),
        );
        let stamp = json!({"at":now,"by":{"user":true}});
        let title = fields
            .get("title")
            .ok_or_else(|| bad("Say what it is (title), in a line."))?;
        let mut row = json!({
            "id":edit.target.id,"owner":owner,"title":title,"body":fields.get("body").cloned().unwrap_or(Value::String(String::new())),
            "status":status,"order":order,"labels":fields.get("labels").cloned().unwrap_or_else(|| json!([])),
            "notes":add.notes,"files":[],"links":[],"sessions":[],"created":stamp,"updated":stamp,
        });
        if status::closed(status) {
            row["doneAt"] = now.into();
        }
        for field in ["assignee", "priority", "due"] {
            if let Some(value) = fields.get(field).filter(|value| !value.is_null()) {
                row[field] = value.clone();
            }
        }
        let staged = self.stage_offline(&add.files)?;
        let published = self.files.publish(
            &self.store,
            &mut row,
            &staged,
            fields.contains_key("body"),
            now,
        )?;
        let item = Item::read(owner, &row).expect("new offline queue item");
        row["mentionIndex"] = json!(Index::rebuild(&self.refs.mentions, &item, all, None));
        row["offlineEdits"] = json!(history);
        let mut history = self.revisions.observe_item(&mut row, &item.id, None);
        let decision = history.remember(
            edit.operation.clone(),
            digest,
            Outcome::Committed,
            Vec::new(),
        );
        row["offlineEdits"] = json!(history);
        self.save_edit_row(owner, &item.id, row, now)?;
        published.commit();
        Ok(decision)
    }
}

fn remember(
    history: &mut History,
    draft: &Draft,
    edit: &Edit,
    digest: String,
    status: Outcome,
    choices: Vec<String>,
) -> Decision {
    let decision = history.remember(edit.operation.clone(), digest, status, choices.clone());
    if draft.edit.operation != edit.operation {
        history.remember(
            draft.edit.operation.clone(),
            types::key(json!(draft.edit)),
            status,
            choices,
        );
    }
    decision
}

fn queue_fields(fields: &Fields) -> Value {
    let mut ordinary = fields
        .iter()
        .filter(|(field, _)| field.as_str() != "order" && !field.starts_with("note:"))
        .map(|(field, value)| (field.clone(), value.clone()))
        .collect::<Fields>();
    if let Some(order) = fields.get("order") {
        for field in ["before", "after", "top"] {
            if let Some(value) = order.get(field) {
                ordinary.insert(field.into(), value.clone());
            }
        }
    }
    Value::Object(ordinary)
}

fn addition(history: &mut History, key: String, value: Value) -> Result<bool, Refusal> {
    use std::collections::btree_map::Entry;
    let digest = types::key(value);
    match history.additions.entry(key) {
        Entry::Vacant(entry) => {
            entry.insert(digest);
            Ok(true)
        }
        Entry::Occupied(entry) if entry.get() == &digest => Ok(false),
        Entry::Occupied(_) => Err(bad("That addition id was used with different content.")),
    }
}

struct Additions {
    notes: Vec<Value>,
    files: Vec<types::Attachment>,
}
