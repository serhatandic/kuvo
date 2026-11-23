//! Revisioned preferences. Each field is compared to the caller's accepted baseline; an
//! intervening change to that field becomes a choice, while independent fields still commit.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{
    Error, SharedKind,
    actor::Response,
    model::{Identity, Link, fingerprint, safe_token},
    preferences,
    storage::Files,
};

const OUTBOX: &str = "run/owned-data/edit-outbox";

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(crate) struct Target {
    pub kind: SharedKind,
}

impl Target {
    pub fn validate(self) -> Result<(), Error> {
        if !matches!(
            self.kind,
            SharedKind::SetupUsageDisplay
                | SharedKind::SetupNames
                | SharedKind::SetupTimeZone
                | SharedKind::SetupPersonalities
                | SharedKind::QueueLabels
        ) {
            return Err(Error::Invalid("setup preference"));
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Edit {
    #[serde(rename = "ref")]
    pub reference: Target,
    pub operation: String,
    pub base_revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resolve: Option<Resolution>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Resolution {
    id: String,
    #[serde(rename = "use")]
    use_choice: ChoiceSide,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ChoiceSide {
    Accepted,
    Proposed,
}

impl Edit {
    pub fn read(given: Value) -> Result<Self, Error> {
        let edit: Self = serde_json::from_value(given).map_err(|_| Error::Invalid("setup edit"))?;
        edit.validate()?;
        Ok(edit)
    }

    fn validate(&self) -> Result<(), Error> {
        self.reference.validate()?;
        if !safe_token(&self.operation, 1, 80)
            || !safe_token(&self.base_revision, 43, 43)
            || self.fields.as_ref().is_some_and(|fields| fields.len() > 64)
            || self
                .resolve
                .as_ref()
                .is_some_and(|resolve| !safe_token(&resolve.id, 43, 43))
            || self.resolve.is_some() == self.fields.is_some()
        {
            return Err(Error::Invalid("setup edit"));
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub(crate) enum Submitter {
    Computer,
    Paired { key: String, pairing: String },
}

impl Submitter {
    pub fn current(&self, link: &Link) -> bool {
        match self {
            Self::Computer => true,
            Self::Paired { key, pairing } if !link.devices_disabled => {
                kuvo_wire::keys::decode_exact::<32>(key)
                    .and_then(|key| {
                        let own = Identity::read(link).ok()?;
                        link.grants(&link.member_list().ok()?, &own.public)
                            .remove(&key)
                    })
                    .is_some_and(|grant| grant.id == *pairing)
            }
            Self::Paired { .. } => false,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Envelope {
    pub setup: String,
    nonce: String,
    pub edit: Edit,
    submitter: Submitter,
    #[serde(default, skip_serializing_if = "is_false")]
    pub replay_only: bool,
}

fn is_false(value: &bool) -> bool {
    !value
}

impl Envelope {
    pub fn read(value: &Value, sender: &str) -> Result<Self, Error> {
        if value
            .get("formatVersion")
            .is_some_and(|v| !matches!(v.as_f64(), Some(0.0 | 1.0)))
        {
            return Err(Error::Version("queued setup edit"));
        }
        let envelope: Self = serde_json::from_value(value.clone())
            .map_err(|_| Error::Invalid("queued setup edit"))?;
        let edit = &envelope.edit;
        edit.validate()?;
        if !safe_token(&envelope.nonce, 1, 80)
            || (edit.operation != fingerprint(&json!([envelope.setup, envelope.nonce]))
                && edit.operation != fingerprint(&json!([envelope.setup, sender, envelope.nonce])))
        {
            return Err(Error::Invalid("queued edit operation"));
        }
        Ok(envelope)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Outgoing {
    #[serde(default)]
    format_version: u8,
    pub setup: String,
    pub owner: String,
    digest: String,
    #[serde(rename = "ref")]
    pub reference: Target,
    pub receipt: Value,
    pub submitter: Submitter,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queued_edit: Option<Envelope>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub recovery: Map<String, Value>,
}

fn address(operation: &str) -> Result<String, Error> {
    if !safe_token(operation, 43, 43) {
        return Err(Error::Invalid("edit operation"));
    }
    Ok(format!("{OUTBOX}/{operation}.json"))
}

pub(crate) fn read_outgoing(files: &Files, operation: &str) -> Result<Option<Outgoing>, Error> {
    let Some(row) = files.read::<Value>(&address(operation)?)? else {
        return Ok(None);
    };
    if row
        .get("formatVersion")
        .is_some_and(|value| !matches!(value.as_f64(), Some(0.0 | 1.0)))
    {
        return Err(Error::Version("edit outbox"));
    }
    let Some(reference) = row
        .get("ref")
        .and_then(|value| serde_json::from_value::<Target>(value.clone()).ok())
    else {
        return Ok(None);
    };
    if reference.validate().is_err() {
        return Ok(None);
    }
    let outgoing = serde_json::from_value(row).map_err(|_| Error::Invalid("edit outbox"))?;
    Ok(Some(outgoing))
}

pub(crate) fn rows(files: &Files) -> Result<Vec<Outgoing>, Error> {
    let mut rows = Vec::new();
    for name in files.names(OUTBOX)? {
        if let Some(operation) = name
            .strip_suffix(".json")
            .filter(|id| safe_token(id, 43, 43))
            && let Some(row) = read_outgoing(files, operation)?
        {
            rows.push(row);
        }
    }
    Ok(rows)
}

impl Outgoing {
    pub fn stage(
        files: &Files,
        link: &Link,
        own: &str,
        mut edit: Edit,
        submitter: Submitter,
    ) -> Result<Self, Error> {
        edit.validate()?;
        let setup = &link.setup()?.position;
        let nonce = edit.operation.clone();
        let preferred = fingerprint(&json!([setup.id, nonce]));
        let legacy = fingerprint(&json!([setup.id, own, nonce]));
        let previous = read_outgoing(files, &preferred)?.or(read_outgoing(files, &legacy)?);
        let operation = previous
            .as_ref()
            .and_then(|row| row.receipt["operation"].as_str())
            .unwrap_or(&preferred)
            .to_owned();
        edit.operation = operation.clone();
        let reference = edit.reference;
        let digest = fingerprint(&json!({ "setup": setup.id, "nonce": nonce, "edit": edit }));
        if let Some(previous) = previous {
            return if previous.digest == digest && previous.submitter == submitter {
                Ok(previous)
            } else {
                Err(Error::Conflict)
            };
        }
        if rows(files)?
            .into_iter()
            .filter(|row| row.queued_edit.is_some())
            .count()
            >= 200
        {
            return Err(Error::Capacity);
        }
        let outgoing = Self {
            format_version: 1,
            setup: setup.id.clone(),
            owner: setup.primary.clone(),
            digest,
            reference,
            receipt: json!({"operation": operation, "status": "pending", "message": "Saved here. Waiting for the current owner to accept it."}),
            queued_edit: Some(Envelope {
                setup: setup.id.clone(),
                nonce,
                edit,
                submitter: submitter.clone(),
                replay_only: false,
            }),
            submitter,
            recovery: Map::new(),
        };
        outgoing.keep(files)?;
        Ok(outgoing)
    }

    pub fn keep(&self, files: &Files) -> Result<(), Error> {
        let operation = self
            .receipt
            .get("operation")
            .and_then(Value::as_str)
            .ok_or(Error::Invalid("edit outbox address"))?;
        files.write(&address(operation)?, self)
    }

    pub fn finish(&mut self, files: &Files, receipt: Value) -> Result<(), Error> {
        if let Some(envelope) = self.queued_edit.take() {
            if matches!(
                receipt.get("status").and_then(Value::as_str),
                Some("interrupted" | "refresh" | "capacity" | "deleted")
            ) {
                self.recovery = envelope
                    .edit
                    .fields
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|(_, value)| value.is_string())
                    .collect();
            }
            self.receipt = receipt;
            self.keep(files)?;
        }
        Ok(())
    }

    pub fn answer(&self) -> Response {
        let status = match self.receipt["status"].as_str() {
            Some("pending") => 202,
            Some("interrupted" | "refresh" | "deleted") => 409,
            Some("capacity") => 413,
            _ => 200,
        };
        Response {
            status,
            value: self.receipt.clone(),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct History {
    #[serde(default)]
    versions: Vec<Version>,
    #[serde(default)]
    choices: Vec<Choice>,
    #[serde(default)]
    receipts: Vec<Value>,
    #[serde(default)]
    additions: Map<String, Value>,
}

#[derive(Serialize, Deserialize)]
struct Version {
    revision: String,
    fields: Map<String, Value>,
    deleted: bool,
}

#[derive(Serialize, Deserialize)]
struct Choice {
    id: String,
    field: String,
    base: Value,
    accepted: Value,
    proposed: Value,
}

fn field<'a>(values: &'a Map<String, Value>, key: &str) -> &'a Value {
    values.get(key).unwrap_or(&Value::Null)
}

impl History {
    fn observe(&mut self, address: &str, fields: Map<String, Value>) {
        let head = self.versions.last();
        let revision =
            if let Some(head) = head.filter(|head| !head.deleted && head.fields == fields) {
                head.revision.clone()
            } else {
                let revision = fingerprint(&json!([
                    address,
                    head.map(|head| &head.revision),
                    false,
                    fields
                ]));
                self.versions.push(Version {
                    revision: revision.clone(),
                    fields: fields.clone(),
                    deleted: false,
                });
                let mut byte_count = 0;
                let present = self
                    .versions
                    .iter()
                    .rev()
                    .take(8)
                    .take_while(|version| {
                        byte_count += serde_json::to_vec(version)
                            .expect("revision serializes")
                            .len();
                        byte_count <= 64 * 1024
                    })
                    .count()
                    .max(1);
                self.versions.drain(..self.versions.len() - present);
                revision
            };

        self.choices.retain_mut(|choice| {
            let accepted = field(&fields, &choice.field);
            if &choice.accepted == accepted {
                return true;
            }
            if &choice.proposed == accepted {
                return false;
            }
            choice.id = fingerprint(&json!([choice.id, revision]));
            choice.base = std::mem::replace(&mut choice.accepted, accepted.clone());
            true
        });
    }

    fn head(&self) -> &Version {
        self.versions.last().expect("the setup value was observed")
    }
}

fn checked_usage(key: &str, value: &Value) -> Option<Value> {
    const LISTS: [&str; 6] = [
        "placed",
        "providerOrder",
        "pinned",
        "expanded",
        "expandOnEnable",
        "seeded",
    ];
    let is_id = |id: &str| {
        !id.is_empty() && id.encode_utf16().count() <= 200 && !id.chars().any(|c| c < ' ')
    };
    let ids = |value: &Value| -> Option<Value> {
        let list = value.as_array()?;
        if list.len() > 200 {
            return None;
        }
        let mut seen = std::collections::HashSet::new();
        Some(json!(
            list.iter()
                .map(Value::as_str)
                .collect::<Option<Vec<_>>>()?
                .into_iter()
                .filter(|item| is_id(item) && seen.insert(*item))
                .collect::<Vec<_>>()
        ))
    };
    if key == "codexFallback" {
        return (value.is_null() || value.as_str().is_some_and(is_id)).then(|| value.clone());
    }
    let mut fields = value.as_object()?.clone();
    let empty = json!([]);
    match key {
        "totalSpend" => {
            value.get("show")?.as_bool()?;
            for key in ["period", "metric"] {
                if !fields.get(key).and_then(Value::as_str).is_some_and(|v| {
                    !v.is_empty() && v.encode_utf16().count() <= 40 && !v.chars().any(|c| c < ' ')
                }) {
                    fields.shift_remove(key);
                }
            }
        }
        "notifications" => {
            for key in ["underTenPercent", "healthyToClose", "closeToRunningOut"] {
                value.get(key)?.as_bool()?;
            }
        }
        "providers" => {
            fields.insert("enabled".into(), ids(value.get("enabled")?)?);
            fields.insert("known".into(), ids(value.get("known").unwrap_or(&empty))?);
        }
        "layout" => {
            let order = value["metricOrder"].as_object()?;
            if order.len() > 200 {
                return None;
            }
            let order: Map<_, _> = order
                .iter()
                .map(|(provider, list)| is_id(provider).then_some((provider.clone(), ids(list)?)))
                .collect::<Option<_>>()?;
            fields.insert("metricOrder".into(), json!(order));
            for key in LISTS {
                fields.insert(key.into(), ids(value.get(key).unwrap_or(&empty))?);
            }
        }
        _ => return None,
    }
    Some(Value::Object(fields))
}

fn normalize(
    kind: SharedKind,
    fields: &Map<String, Value>,
    key: &str,
    value: &Value,
) -> Option<Value> {
    use SharedKind::*;
    match kind {
        SetupTimeZone if key == "zone" => preferences::Zone::choose(value.as_str()?, None)
            .ok()
            .map(|choice| json!(choice.zone)),
        SetupUsageDisplay => checked_usage(key, value),
        SetupNames if fields.contains_key(key) => {
            let name = preferences::short_name(value.as_str()?, 100);
            (!name.is_empty()).then(|| json!(name))
        }
        SetupPersonalities if fields.contains_key(key) => {
            let (_, name) = key.rsplit_once(':')?;
            let value = value.as_str()?.trim();
            let value = match name {
                "name" => preferences::short_name(value, 80),
                "description" => preferences::truncate(value, 2000),
                "symbol" => preferences::truncate(value, 80),
                _ => return None,
            };
            (!value.is_empty()).then(|| json!(value))
        }
        QueueLabels if fields.contains_key(key) => {
            let (_, name) = key.rsplit_once(':')?;
            let value = value.as_str()?;
            if name == "color" {
                [
                    "coral", "orange", "amber", "lime", "mint", "sky", "indigo", "violet", "pink",
                    "graphite",
                ]
                .contains(&value)
                .then(|| json!(value))
            } else {
                let value = preferences::short_name(value, 50);
                (!value.is_empty()).then(|| json!(value))
            }
        }
        _ => None,
    }
}

fn project(files: &Files, link: &Link, kind: SharedKind) -> Result<Map<String, Value>, Error> {
    use SharedKind::*;
    let value = link.value(kind).unwrap_or(&Value::Null);
    let mut fields = Map::new();
    match kind {
        SetupTimeZone => {
            fields.insert(
                "zone".into(),
                json!(preferences::zone(files, link)?.map(|zone| zone.zone)),
            );
        }
        SetupNames => {
            for peer in link.member_list()?.boxes {
                fields.insert(
                    peer.key.clone(),
                    value.get(&peer.key).cloned().unwrap_or_default(),
                );
            }
        }
        SetupUsageDisplay => {
            if let Some(units) = value.get("units").and_then(Value::as_object) {
                for (key, entry) in units {
                    if let Some(value) = entry
                        .get("value")
                        .and_then(|value| checked_usage(key, value))
                    {
                        fields.insert(key.clone(), value);
                    }
                }
            }
        }
        SetupPersonalities => {
            let library = preferences::library(files, link)?.shared();
            project_entries(
                &mut fields,
                &library["personalities"],
                &["name", "description", "symbol"],
            );
        }
        QueueLabels => {
            let library = labels(files, link)?;
            project_entries(&mut fields, &library["labels"], &["name", "color"]);
        }
        _ => return Err(Error::Invalid("setup preference")),
    }
    Ok(fields)
}

fn labels(files: &Files, link: &Link) -> Result<Value, Error> {
    Ok(match link.value(SharedKind::QueueLabels) {
        Some(value) => value.clone(),
        None => files
            .read("run/work-queue/labels.json")?
            .unwrap_or_else(|| json!({"labels": [], "removed": []})),
    })
}

fn project_entries(output: &mut Map<String, Value>, entries: &Value, keys: &[&str]) {
    for entry in entries.as_array().into_iter().flatten() {
        if let Some(id) = entry.get("id").and_then(Value::as_str) {
            for key in keys {
                output.insert(format!("{id}:{key}"), entry[*key].clone());
            }
        }
    }
}

fn new_value(
    files: &Files,
    link: &Link,
    own: &Identity,
    kind: SharedKind,
    current: &Map<String, Value>,
    changed: &Map<String, Value>,
) -> Result<Value, Error> {
    use SharedKind::*;
    match kind {
        SetupTimeZone => {
            let before = preferences::zone(files, link)?;
            let Some(zone) = changed.get("zone").and_then(Value::as_str) else {
                return Ok(json!(before));
            };
            Ok(json!(preferences::Zone::choose(zone, before.as_ref())?))
        }
        SetupNames => {
            let mut fields = current.clone();
            fields.extend(changed.clone());
            Ok(json!(fields))
        }
        SetupUsageDisplay => {
            let mut value = link
                .value(kind)
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let mut units = value
                .remove("units")
                .and_then(|value| value.as_object().cloned())
                .unwrap_or_default();
            for (field, value) in changed {
                let count = units
                    .get(field)
                    .and_then(|unit| unit["stamp"]["v"].as_u64())
                    .unwrap_or(0)
                    .saturating_add(1);
                units.insert(
                    field.clone(),
                    json!({"value": value, "stamp": {"v": count, "by": own.public}}),
                );
            }
            let imported = value
                .get("imported")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            value.insert("imported".into(), json!(imported));
            value.insert("units".into(), json!(units));
            Ok(json!(value))
        }
        SetupPersonalities | QueueLabels => {
            let (mut value, list) = if kind == SetupPersonalities {
                (preferences::library(files, link)?.shared(), "personalities")
            } else {
                (labels(files, link)?, "labels")
            };
            if let Some(entries) = value[list].as_array_mut() {
                let mut label_names = std::collections::HashMap::new();
                let mut edited = Vec::new();
                for entry in entries {
                    let Some(id) = entry["id"].as_str().map(str::to_owned) else {
                        continue;
                    };
                    let mut modified = false;
                    for key in ["name", "description", "symbol", "color"] {
                        if let Some(value) = changed.get(&format!("{id}:{key}")) {
                            entry[key] = value.clone();
                            modified = true;
                            if kind == QueueLabels && key == "name" {
                                edited.push(value.as_str().unwrap_or_default().to_lowercase());
                            }
                        }
                    }
                    if kind == QueueLabels {
                        *label_names
                            .entry(entry["name"].as_str().unwrap_or_default().to_lowercase())
                            .or_insert(0) += 1;
                    } else if modified {
                        entry["updated"] = json!(entry["updated"].as_u64().unwrap_or_default() + 1);
                    }
                }
                if edited.iter().any(|name| label_names[name] > 1) {
                    return Err(Error::Invalid("duplicate setup label name"));
                }
            }
            Ok(value)
        }
        _ => Err(Error::Invalid("setup preference")),
    }
}

fn history(files: &Files, link: &Link, kind: SharedKind, address: &str) -> Result<History, Error> {
    let mut history: History = link
        .shared
        .get(&kind)
        .and_then(|entry| entry.offline_edits.as_ref())
        .map(|prior| serde_json::from_value(prior.clone()))
        .transpose()
        .map_err(|_| Error::Invalid("setup edit history"))?
        .unwrap_or_default();
    if !link.shared.contains_key(&kind) {
        history.additions.insert("setup-seed:".into(), json!("1"));
    }
    history.observe(address, project(files, link, kind)?);
    Ok(history)
}

fn history_address(link: &Link, kind: SharedKind) -> Result<String, Error> {
    let name = json!(kind);
    Ok(format!(
        "{}:{}",
        link.setup()?.position.id,
        name.as_str().expect("shared kind is a string")
    ))
}

pub(crate) fn view(
    files: &Files,
    link: &Link,
    target: Target,
    stale: bool,
) -> Result<Value, Error> {
    target.validate()?;
    let setup = link.setup()?;
    let history = history(
        files,
        link,
        target.kind,
        &history_address(link, target.kind)?,
    )?;
    let head = history.head();
    Ok(json!({
        "fields": head.fields, "revision": head.revision, "deleted": head.deleted, "choices": history.choices,
        "owner": setup.position.primary, "stale": stale,
    }))
}

pub(crate) fn apply(
    files: &Files,
    link: &Link,
    identity: &Identity,
    envelope: &Envelope,
) -> Result<(Option<Link>, Value), Error> {
    let edit = &envelope.edit;
    let kind = edit.reference.kind;
    let digest = fingerprint(&json!(edit));
    if let Some(previous) = link.shared_outcomes.get(&edit.operation) {
        if let Some(record) = &previous.offline_edit {
            if record["digest"] == digest {
                return Ok((None, record["decision"].clone()));
            }
            return Err(Error::Conflict);
        }
        return Ok((
            None,
            interrupted(
                &edit.operation,
                "The main computer changed before this edit committed.",
            ),
        ));
    }
    if envelope.replay_only || !envelope.submitter.current(link) {
        return Ok((
            None,
            interrupted(
                &edit.operation,
                "This device no longer has access to edit here.",
            ),
        ));
    }

    let address = history_address(link, kind)?;
    let mut history = history(files, link, kind, &address)?;
    let head = history.head();
    let revision = head.revision.clone();
    let current = head.fields.clone();
    let Some(index) = history
        .versions
        .iter()
        .position(|version| version.revision == edit.base_revision)
    else {
        return Ok((None, decision(&edit.operation, "refresh", &revision, &[])));
    };
    let mut changes = Map::new();
    let mut added = vec![];
    if let Some(resolution) = &edit.resolve {
        let choice = history.choices.iter().position(|choice| {
            choice.id == resolution.id && &choice.accepted == field(&current, &choice.field)
        });
        let Some(choice) = choice.filter(|_| revision == edit.base_revision) else {
            return Ok((None, decision(&edit.operation, "refresh", &revision, &[])));
        };
        let choice = history.choices.remove(choice);
        if matches!(resolution.use_choice, ChoiceSide::Proposed) {
            changes.insert(choice.field, choice.proposed);
        }
    } else if let Some(fields) = &edit.fields {
        let base = &history.versions[index].fields;
        for (key, wanted) in fields {
            let wanted = normalize(kind, &current, key, wanted)
                .ok_or(Error::Invalid("editable setup preference or value"))?;
            let original = field(base, key);
            let accepted = field(&current, key);
            if &wanted == original || &wanted == accepted {
                continue;
            }
            if history.versions[index + 1..]
                .iter()
                .any(|version| field(&version.fields, key) != original)
            {
                let id = fingerprint(&json!([edit.operation, key]));
                history.choices.push(Choice {
                    id: id.clone(),
                    field: key.clone(),
                    base: original.clone(),
                    accepted: accepted.clone(),
                    proposed: wanted,
                });
                added.push(id);
            } else {
                changes.insert(key.clone(), wanted);
            }
        }
    }
    let value = new_value(files, link, identity, kind, &current, &changes)?;
    let mut next = link.clone();
    next.put(identity, kind, value, None, Some(edit.operation.clone()))?;
    let mut accepted = current;
    accepted.extend(changes);
    history.observe(&address, accepted);
    let result = decision(
        &edit.operation,
        if added.is_empty() {
            "committed"
        } else {
            "choice"
        },
        &history.head().revision,
        &added,
    );
    history.receipts.clear();
    let entry = next.shared.get_mut(&kind).expect("saved preference");
    entry.offline_edits = Some(json!(history));
    let outcome = next
        .shared_outcomes
        .get_mut(&edit.operation)
        .expect("saved outcome");
    outcome.offline_edit = Some(json!({"digest": digest, "decision": result}));
    outcome.mutation_digest = None;
    next.check_capacity()?;
    Ok((Some(next), result))
}

fn decision(operation: &str, status: &str, revision: &str, choices: &[String]) -> Value {
    json!({"operation": operation, "status": status, "revision": revision, "choices": choices})
}

pub(crate) fn interrupted(operation: &str, message: &str) -> Value {
    json!({"operation": operation, "status": "interrupted", "message": message})
}

pub(crate) fn received_receipt(value: &Value) -> Option<(&str, &str, Value)> {
    let setup = value.get("setup")?.as_str()?;
    let receipt = value.get("receipt")?;
    let operation = receipt["operation"].as_str()?;
    if !safe_token(operation, 43, 43) {
        return None;
    }
    let body = match receipt["status"].as_str()? {
        status @ ("interrupted" | "capacity") => {
            json!({"operation": operation, "status": status, "message": receipt["message"].as_str()?})
        }
        status @ ("committed" | "choice" | "deleted" | "refresh") => {
            let revision = receipt["revision"].as_str()?;
            let choices = receipt["choices"].as_array()?;
            if !safe_token(revision, 43, 43)
                || choices
                    .iter()
                    .any(|id| !id.as_str().is_some_and(|id| safe_token(id, 43, 43)))
            {
                return None;
            }
            json!({"operation": operation, "status": status, "revision": revision, "choices": choices})
        }
        _ => return None,
    };
    Some((setup, operation, body))
}
