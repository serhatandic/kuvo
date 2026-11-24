//! Work Queue revisions, conflict choices and replay records.

mod text;

use crate::queue::store::array;
use kuvo_store::format::Fields;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use text::{Hunk, Passages};

const MAX_VERSIONS: usize = 32;
const MAX_BYTES: usize = 256 * 1024;
const SCALARS: [&str; 7] = [
    "title", "body", "status", "priority", "labels", "assignee", "due",
];

pub(in crate::queue::items) struct Revisions {
    passages: Passages,
}

impl Revisions {
    pub(in crate::queue::items) fn new() -> Self {
        Self {
            passages: Passages::new(),
        }
    }

    // Old notes gain the same content-based ID on every computer before carrying a revision.
    pub(super) fn observe_item(
        &self,
        row: &mut Value,
        item: &str,
        passages: Option<Layout>,
    ) -> History {
        let mut fields: Fields = SCALARS
            .into_iter()
            .map(|name| (name.into(), row[name].clone()))
            .collect();
        fields.insert(
            "order".into(),
            json!({"position":row["order"],"status":row["status"]}),
        );
        let mut notes = array(row.get("notes")).to_vec();
        for (index, note) in notes
            .iter_mut()
            .enumerate()
            .filter(|(_, note)| note.is_object())
        {
            if !note["id"].is_string() {
                note["id"] = key(json!([item, index, note["at"], note["by"], note["text"]])).into();
            }
            let id = note["id"].as_str().expect("note id");
            fields.insert(format!("note:{id}"), note["text"].clone());
        }
        row["notes"] = notes.into();
        let old = row
            .get("offlineEdits")
            .and_then(|history| serde_json::from_value(history.clone()).ok())
            .unwrap_or_default();
        self.observe(old, item, fields, row["removed"].is_object(), passages)
    }

    pub(super) fn absent(&self, id: &str, deleted: bool) -> History {
        self.observe(History::default(), id, Fields::new(), deleted, None)
    }

    fn observe(
        &self,
        mut history: History,
        id: &str,
        fields: Fields,
        deleted: bool,
        passages: Option<Layout>,
    ) -> History {
        let before = history.versions.last();
        if !before.is_some_and(|before| before.deleted == deleted && before.fields == fields) {
            let revision = key(json!([
                id,
                before.map(|before| &before.revision),
                deleted,
                fields
            ]));
            history.versions.push(Snapshot {
                revision,
                fields,
                deleted,
                passages,
            });
            history
                .versions
                .drain(..history.versions.len().saturating_sub(MAX_VERSIONS));
            let mut bytes = 0;
            for (index, version) in history.versions.iter().enumerate().rev() {
                bytes += serde_json::to_vec(version)
                    .expect("snapshot serializes")
                    .len();
                if bytes > MAX_BYTES {
                    let through = (index + 1).min(history.versions.len() - 1);
                    history.versions.drain(..through);
                    break;
                }
            }
        }
        if deleted {
            history.choices.clear();
            return history;
        }
        let current = history.versions.last().expect("observed revision");
        let head = &current.fields;
        history.choices.retain_mut(|choice| {
            let accepted = at(head, &choice.field);
            if &choice.accepted == accepted {
                return true;
            }
            let mut proposed = choice.proposed.clone();
            if let Some((base, accepted, proposed_text)) =
                passage(&choice.field, &choice.accepted, accepted, &proposed)
            {
                let merged = self
                    .passages
                    .merge(base, accepted, proposed_text, &[], None);
                proposed = merged.proposed.unwrap_or(merged.accepted).into();
            }
            if accepted == &proposed {
                return false;
            }
            choice.id = key(json!([choice.id, current.revision]));
            choice.base = std::mem::replace(&mut choice.accepted, accepted.clone());
            choice.proposed = proposed;
            true
        });
        history
    }

    pub(super) fn merge(
        &self,
        history: &History,
        operation: &str,
        index: usize,
        wanted: &Fields,
        resolve: Option<&Resolve>,
        delete: bool,
    ) -> Merge {
        let head = history.current();
        let base = &history.versions[index];
        let mut merged = Merge {
            choices: history.choices.clone(),
            ..Merge::default()
        };
        if let Some(resolve) = resolve {
            let choice = history
                .choices
                .iter()
                .find(|choice| choice.id == resolve.id)
                .expect("resolution was checked");
            if matches!(resolve.side, Side::Proposed) {
                merged
                    .changes
                    .insert(choice.field.clone(), choice.proposed.clone());
            }
            merged.choices.retain(|choice| choice.id != resolve.id);
            return merged;
        }
        if delete {
            return merged;
        }
        let versions = &history.versions[index + 1..];
        let mut layouts = BTreeMap::new();
        for (field, value) in wanted {
            let original = at(&base.fields, field);
            let accepted = at(&head.fields, field);
            if value == original {
                continue;
            }
            let passage = passage(field, original, accepted, value);
            if value == accepted && passage.is_none() {
                continue;
            }
            let mut kept = accepted.clone();
            let mut proposed = value.clone();
            if let Some((original, accepted, value)) = passage {
                let known: Vec<_> = versions
                    .iter()
                    .rev()
                    .filter_map(|version| {
                        let layout = version
                            .passages
                            .as_ref()
                            .filter(|layout| layout.base == base.revision)?;
                        layout.fields.get(field).map(Vec::as_slice)
                    })
                    .collect();
                let combined =
                    self.passages
                        .merge(original, accepted, value, &known, Some(operation));
                if combined.accepted != accepted {
                    layouts.insert(field.clone(), combined.edits);
                    kept = combined.accepted.into();
                    merged.changes.insert(field.clone(), kept.clone());
                }
                let Some(alternative) = combined.proposed else {
                    continue;
                };
                proposed = alternative.into();
            } else if !versions
                .iter()
                .any(|version| at(&version.fields, field) != original)
            {
                merged.changes.insert(field.clone(), proposed);
                continue;
            }
            let id = key(json!([operation, field]));
            merged.choices.push(Choice {
                id: id.clone(),
                field: field.clone(),
                base: original.clone(),
                accepted: kept,
                proposed,
            });
            merged.added.push(id);
            merged.status = Outcome::Choice;
        }
        if !layouts.is_empty() {
            merged.layout = Some(Layout {
                base: base.revision.clone(),
                fields: layouts,
            });
        }
        merged
    }
}

fn key(value: Value) -> String {
    kuvo_wire::keys::encode(&kuvo_wire::json::digest(&value))
}

fn at<'a>(fields: &'a Fields, field: &str) -> &'a Value {
    fields.get(field).unwrap_or(&Value::Null)
}

fn passage<'a>(
    field: &str,
    a: &'a Value,
    b: &'a Value,
    c: &'a Value,
) -> Option<(&'a str, &'a str, &'a str)> {
    (field == "body" || field.starts_with("note:"))
        .then(|| Some((a.as_str()?, b.as_str()?, c.as_str()?)))
        .flatten()
}

#[derive(Serialize, Deserialize, Default)]
pub(super) struct History {
    versions: Vec<Snapshot>,
    pub choices: Vec<Choice>,
    receipts: Vec<Remembered>,
    pub additions: BTreeMap<String, String>,
}

impl History {
    fn current(&self) -> &Snapshot {
        self.versions.last().expect("observed revision")
    }

    pub fn revision(&self) -> &str {
        &self.current().revision
    }

    pub fn view(&self) -> Value {
        let head = self.current();
        json!({"revision":head.revision,"fields":head.fields,"deleted":head.deleted,"choices":self.choices})
    }

    pub fn baseline(
        &self,
        revision: Option<&str>,
        resolve: Option<&Resolve>,
    ) -> Result<usize, Outcome> {
        let head = self.current();
        if head.deleted {
            return Err(Outcome::Deleted);
        }
        let index = self
            .versions
            .iter()
            .position(|version| Some(version.revision.as_str()) == revision)
            .ok_or(Outcome::Refresh)?;
        if self.versions[index..].iter().any(|version| version.deleted)
            || resolve.is_some_and(|resolve| {
                index + 1 != self.versions.len()
                    || !self.choices.iter().any(|choice| {
                        choice.id == resolve.id
                            && &choice.accepted == at(&head.fields, &choice.field)
                    })
            })
        {
            return Err(Outcome::Refresh);
        }
        Ok(index)
    }

    pub fn replay(&self, operation: &str, digest: &str) -> Option<Result<Decision, ()>> {
        let saved = self
            .receipts
            .iter()
            .find(|entry| entry.decision.operation == operation)?;
        Some(
            (saved.digest == digest)
                .then(|| saved.decision.clone())
                .ok_or(()),
        )
    }

    pub fn remember(
        &mut self,
        operation: String,
        digest: String,
        status: Outcome,
        choices: Vec<String>,
    ) -> Decision {
        let decision = Decision {
            operation,
            status,
            revision: self.current().revision.clone(),
            choices,
        };
        self.receipts.push(Remembered {
            digest,
            decision: decision.clone(),
        });
        decision
    }
}

#[derive(Serialize, Deserialize)]
pub(super) struct Layout {
    base: String,
    fields: BTreeMap<String, Vec<Hunk>>,
}

#[derive(Serialize, Deserialize)]
struct Snapshot {
    revision: String,
    fields: Fields,
    deleted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    passages: Option<Layout>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Choice {
    pub id: String,
    pub field: String,
    pub base: Value,
    pub accepted: Value,
    pub proposed: Value,
}

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Outcome {
    #[default]
    Committed,
    Choice,
    Deleted,
    Refresh,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Decision {
    pub operation: String,
    pub status: Outcome,
    revision: String,
    choices: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct Remembered {
    digest: String,
    decision: Decision,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Resolve {
    pub id: String,
    #[serde(rename = "use")]
    side: Side,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Side {
    Accepted,
    Proposed,
}

#[derive(Default)]
pub(super) struct Merge {
    pub status: Outcome,
    pub choices: Vec<Choice>,
    pub changes: Fields,
    pub added: Vec<String>,
    pub layout: Option<Layout>,
}
