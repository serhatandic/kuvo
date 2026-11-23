use std::{cmp::Ordering, collections::HashMap, fs, path::Path};

use base64::{Engine, engine::general_purpose::STANDARD};
use jiff::{Timestamp, tz::TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    Error,
    actor::Response,
    lease,
    model::{Link, SharedKind, fingerprint},
    storage::Files,
};

const LIBRARY: &str = "bots/personalities.json";
const ZONE: &str = "box/time-zone.json";
const MAX_COUNT: u64 = 1 << 52;
const SYMBOL: &str = "person.crop.circle";

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Personality {
    id: String,
    name: String,
    description: String,
    symbol: String,
    updated: u64,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Removal {
    id: String,
    removed: u64,
}

enum Take {
    Live(Personality),
    Removed(Removal),
}

impl Take {
    fn id(&self) -> &str {
        match self {
            Self::Live(entry) => &entry.id,
            Self::Removed(entry) => &entry.id,
        }
    }

    fn wins(&self, previous: &Self) -> bool {
        match (self, previous) {
            (Self::Removed(a), Self::Removed(b)) => a.removed > b.removed,
            (Self::Removed(_), Self::Live(_)) => true,
            (Self::Live(_), Self::Removed(_)) => false,
            (Self::Live(a), Self::Live(b)) => match a.updated.cmp(&b.updated) {
                Ordering::Equal => format!("{}\n{}", a.name, a.description)
                    .encode_utf16()
                    .cmp(format!("{}\n{}", b.name, b.description).encode_utf16())
                    .is_gt(),
                order => order.is_gt(),
            },
        }
    }
}

#[derive(Default, Serialize)]
pub(crate) struct Library {
    pub personalities: Vec<Personality>,
    removed: Vec<Removal>,
}

pub(crate) enum PersonalityChange {
    Create(Value),
    Patch(String, Value),
    Delete(String),
    Merge(Value),
}

fn personality_id(id: &str) -> bool {
    id.strip_prefix("custom.").is_some_and(|rest| {
        !rest.is_empty()
            && rest.len() <= 80
            && rest
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
    })
}

pub(crate) fn truncate(text: &str, length: usize) -> String {
    text.chars()
        .scan(0, |size, ch| {
            *size += ch.len_utf16();
            (*size <= length).then_some(ch)
        })
        .collect()
}

pub(crate) fn short_name(text: &str, length: usize) -> String {
    let normalized: String = text
        .chars()
        .map(|ch| match ch {
            '\u{0000}'..='\u{001f}'
            | '\u{007f}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}' => ' ',
            _ => ch,
        })
        .collect();
    truncate(
        &normalized.split_whitespace().collect::<Vec<_>>().join(" "),
        length,
    )
}

fn count(n: Option<&Value>) -> Option<u64> {
    let n = n.or(Some(&Value::Null))?;
    let n = match n {
        Value::String(s) => s.parse::<f64>().ok()?,
        Value::Number(n) => n.as_f64()?,
        Value::Null => 0.0,
        _ => return None,
    };
    (n.is_finite() && (0.0..=MAX_COUNT as f64).contains(&n) && n.fract() == 0.0).then_some(n as u64)
}

fn live(id: String, updated: u64, value: &Value) -> Option<Personality> {
    let name = short_name(value.get("name")?.as_str()?, 80);
    let description = truncate(value.get("description")?.as_str()?.trim(), 2000);
    if name.is_empty() || description.is_empty() || updated > MAX_COUNT {
        return None;
    }
    let symbol = truncate(
        value
            .get("symbol")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or(SYMBOL),
        80,
    );
    Some(Personality {
        id,
        name,
        description,
        symbol,
        updated,
    })
}

fn take(value: &Value) -> Option<Take> {
    let id = value.get("id")?.as_str()?;
    if !personality_id(id) {
        return None;
    }
    let id = id.to_owned();
    if let Some(removed) = value.get("removed") {
        Some(Take::Removed(Removal {
            id,
            removed: count(Some(removed))?,
        }))
    } else {
        Some(Take::Live(live(id, count(value.get("updated"))?, value)?))
    }
}

/// Project custom personalities from a retired kit into the local library.
pub fn project_kit_personalities(kept: &Value, offered: &[Value]) -> Option<Value> {
    let known: std::collections::HashSet<_> = Library::entries(kept)
        .map(|entry| entry.id().to_owned())
        .collect();
    let imported: Vec<_> = offered
        .iter()
        .filter(|entry| entry.get("custom").and_then(Value::as_bool) == Some(true))
        .filter_map(|entry| {
            let id = entry.get("id")?.as_str()?;
            if !personality_id(id) || known.contains(id) {
                return None;
            }
            Some(Take::Live(live(
                id.to_owned(),
                0,
                &json!({ "name": entry.get("name"), "description": entry.get("description") }),
            )?))
        })
        .collect();
    if imported.is_empty() {
        return None;
    }
    Some(Library::merge(Library::entries(kept).chain(imported), 500).document())
}

impl Library {
    pub fn document(&self) -> Value {
        json!({"kuvo": 1, "personalities": self.personalities, "removed": self.removed})
    }

    pub fn shared(&self) -> Value {
        json!(self)
    }

    fn entries(value: &Value) -> impl Iterator<Item = Take> + '_ {
        ["personalities", "removed"]
            .into_iter()
            .flat_map(|key| {
                value
                    .get(key)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
            })
            .filter_map(take)
    }

    fn merge(entries: impl IntoIterator<Item = Take>, max_removals: usize) -> Self {
        let mut positions = HashMap::new();
        let mut all: Vec<Take> = Vec::new();
        for entry in entries {
            if let Some(&index) = positions.get(entry.id()) {
                if entry.wins(&all[index]) {
                    all[index] = entry;
                }
            } else {
                positions.insert(entry.id().to_owned(), all.len());
                all.push(entry);
            }
        }
        let mut result = Self::default();
        for entry in all {
            match entry {
                Take::Live(entry) => result.personalities.push(entry),
                Take::Removed(entry) => result.removed.push(entry),
            }
        }
        result
            .removed
            .sort_by_key(|entry| std::cmp::Reverse(entry.removed));
        result.removed.truncate(max_removals);
        result
    }

    pub fn change(mut self, change: PersonalityChange) -> Result<(Self, Response), Error> {
        let now = lease::wall();
        let response = match change {
            PersonalityChange::Merge(document) => {
                if document
                    .get("personalities")
                    .and_then(Value::as_array)
                    .is_none()
                    || document
                        .get("removed")
                        .is_some_and(|removed| !removed.is_array())
                {
                    return Ok((
                        self,
                        response(
                            400,
                            "Send a library of personalities: {document: {personalities, removed}}.",
                        ),
                    ));
                }
                let previous = self.shared();
                let old: HashMap<_, _> = ["personalities", "removed"]
                    .into_iter()
                    .flat_map(|key| previous[key].as_array().into_iter().flatten())
                    .filter_map(|entry| Some((entry["id"].as_str()?, entry)))
                    .collect();
                self = Self::merge(
                    Self::entries(&previous).chain(Self::entries(&document)),
                    usize::MAX,
                );
                let next = self.shared();
                let changed: Vec<_> = ["personalities", "removed"]
                    .into_iter()
                    .flat_map(|key| next[key].as_array().into_iter().flatten())
                    .filter_map(|entry| {
                        let id = entry["id"].as_str()?;
                        (old.get(id).copied() != Some(entry)).then_some(id)
                    })
                    .collect();
                Response::ok(json!({"changed": changed}))
            }
            PersonalityChange::Create(value) => {
                let Some(made) = live(format!("custom.{}", uuid::Uuid::new_v4()), now, &value)
                else {
                    return Ok((self, incomplete()));
                };
                let response = Response::ok(json!(made));
                self.personalities.push(made);
                response
            }
            PersonalityChange::Patch(id, value) => {
                let Some(prior) = self.personalities.iter_mut().find(|entry| entry.id == id) else {
                    return Ok((self, response(404, "")));
                };
                let fields = json!({
                    "name": value.get("name").filter(|v| !v.is_null()).cloned().unwrap_or_else(|| json!(prior.name)),
                    "description": value.get("description").filter(|v| !v.is_null()).cloned().unwrap_or_else(|| json!(prior.description)),
                    "symbol": prior.symbol,
                });
                let Some(changed) = live(id, prior.updated + 1, &fields) else {
                    return Ok((self, incomplete()));
                };
                let response = Response::ok(json!(changed));
                *prior = changed;
                response
            }
            PersonalityChange::Delete(id) => {
                let Some(index) = self.personalities.iter().position(|entry| entry.id == id) else {
                    return Ok((self, response(404, "")));
                };
                let prior = self.personalities.remove(index);
                self.removed.push(Removal {
                    id,
                    removed: now.max(prior.updated + 1),
                });
                self.removed
                    .sort_by_key(|entry| std::cmp::Reverse(entry.removed));
                Response::ok(json!({}))
            }
        };
        if serde_json::to_vec(&self).expect("library serializes").len()
            > kuvo_wire::documents::MAX_BYTES - 4096
        {
            return Ok((
                self,
                self::response(
                    400,
                    "There’s no room for more in your personalities. Shorten or remove one, then try again.",
                ),
            ));
        }
        Ok((self, response))
    }
}

fn response(status: u16, message: &str) -> Response {
    let value = if message.is_empty() {
        json!({})
    } else {
        json!({"message": message})
    };
    Response { status, value }
}

fn incomplete() -> Response {
    response(400, "A personality needs a name and a description.")
}

pub(crate) fn keep_local_library(files: &Files, library: &Library) -> Result<(), Error> {
    files.write(LIBRARY, &library.document())
}

pub(crate) fn library(files: &Files, link: &Link) -> Result<Library, Error> {
    if let Some(value) = link
        .value(SharedKind::SetupPersonalities)
        .filter(|value| value.is_object())
    {
        return Ok(Library::merge(Library::entries(value), usize::MAX));
    }
    let local = files.read::<Value>(LIBRARY)?.unwrap_or(Value::Null);
    if local
        .get("kuvo")
        .is_some_and(|version| !matches!(version.as_f64(), Some(0.0 | 1.0)))
    {
        return Err(Error::Version("personality library"));
    }
    Ok(Library::merge(Library::entries(&local), 500))
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Zone {
    kuvo: u8,
    pub zone: String,
    n: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    zoneinfo: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub carried: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kit_seen: Option<String>,
}

fn zone_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && !name.starts_with('/')
        && !name.contains("..")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'+' | b'-' | b'/'))
}

fn rules(name: &str) -> Option<String> {
    if !zone_name(name) {
        return None;
    }
    let bytes = fs::read(Path::new("/usr/share/zoneinfo").join(name)).ok()?;
    (bytes.starts_with(b"TZif") && bytes.len() < 16_384).then(|| STANDARD.encode(bytes))
}

impl Zone {
    pub fn choose(name: &str, before: Option<&Self>) -> Result<Self, Error> {
        if !zone_name(name) || TimeZone::get(name).is_err() {
            return Err(Error::Invalid("time zone"));
        }
        Ok(Self {
            kuvo: 1,
            zone: name.into(),
            n: lease::wall()
                .max(before.map_or(1, |zone| zone.n + 1))
                .min(MAX_COUNT),
            zoneinfo: rules(name).or_else(|| {
                before
                    .filter(|prior| prior.zone == name)
                    .and_then(|prior| prior.zoneinfo.clone())
            }),
            carried: false,
            kit_seen: None,
        })
    }

    fn valid(&self) -> bool {
        self.kuvo == 1 && zone_name(&self.zone) && self.n <= MAX_COUNT
    }
}

pub(crate) fn keep_local_zone(files: &Files, zone: &Zone) -> Result<(), Error> {
    if files
        .read::<Value>(ZONE)?
        .and_then(|value| value.get("kuvo").cloned())
        .is_some_and(|v| v != 1)
    {
        return Err(Error::Version("time zone"));
    }
    files.write(ZONE, zone)
}

pub(crate) fn apply_zone(files: &Files, setting: &Zone) -> Result<(), Error> {
    let mut paths = vec!["wakes".to_owned()];
    for entry in files.names("bots")? {
        let path = format!("bots/{entry}");
        if files.path(&format!("{path}/bot.json")).is_file()
            && !files.path(&format!("{path}/.moving")).exists()
        {
            paths.push(path);
        }
    }
    let rules = setting
        .zoneinfo
        .as_ref()
        .filter(|value| value.len() <= 22_000)
        .and_then(|value| STANDARD.decode(value).ok())
        .filter(|value| value.len() > 4 && value.len() < 16_384 && value.starts_with(b"TZif"));
    for folder in paths {
        let rule_path = format!("{folder}/zoneinfo");
        if let Some(rules) = &rules {
            files.write_bytes(&rule_path, rules)?;
        }
        files.write_bytes(&format!("{folder}/timezone"), setting.zone.as_bytes())?;
        if rules.is_none() {
            files.remove(&rule_path)?;
        }
    }
    Ok(())
}

pub(crate) fn zone(files: &Files, link: &Link) -> Result<Option<Zone>, Error> {
    if let Some(setting) = link
        .value(SharedKind::SetupTimeZone)
        .and_then(|value| serde_json::from_value::<Zone>(value.clone()).ok())
        .filter(Zone::valid)
    {
        return Ok(Some(setting));
    }
    let kept = files.read::<Zone>(ZONE)?.filter(Zone::valid);
    Ok(kept)
}

fn title(name: &str) -> String {
    name.replace('_', " ").replace('/', " › ")
}

pub(crate) fn clock(files: &Files, link: &Link, computer: &str) -> Result<Value, Error> {
    let kept = zone(files, link)?;
    let system = TimeZone::system();
    let own = system.iana_name().unwrap_or("UTC");
    let selected = kept.as_ref().map_or(own, |zone| zone.zone.as_str());
    let timezone = TimeZone::get(selected).unwrap_or(TimeZone::UTC);
    let seconds = timezone.to_offset(Timestamp::now()).seconds();
    let minutes = seconds.unsigned_abs() / 60;
    let offset = if minutes == 0 {
        "GMT".to_owned()
    } else {
        let sign = if seconds < 0 { "−" } else { "+" };
        let (hours, minutes) = (minutes / 60, minutes % 60);
        if minutes == 0 {
            format!("GMT{sign}{hours}")
        } else {
            format!("GMT{sign}{hours}:{minutes:02}")
        }
    };
    let name = if computer.is_empty() {
        "this computer"
    } else {
        computer
    };
    let mut choices: Vec<_> = jiff::tz::db()
        .available()
        .filter(|id| zone_name(id.as_str()))
        .map(|id| json!({"id": id.as_str(), "title": title(id.as_str())}))
        .collect();
    if kept.is_none() {
        choices.insert(
            0,
            json!({"id": "own-clock", "title": format!("{name}'s clock ({})", title(selected))}),
        );
    } else if !choices.iter().any(|choice| choice["id"] == selected) {
        choices.insert(0, json!({"id": selected, "title": title(selected)}));
    }
    let missing_rules = kept.as_ref().is_some_and(|zone| {
        zone.zone != "UTC" && zone.zoneinfo.is_none() && rules(&zone.zone).is_none()
    });
    let line = match kept.as_ref() {
        Some(_) if missing_rules => format!(
            "{name} has no rules for this time zone, so its wake-ups go by UTC. Install its time zone data (tzdata) to fix it."
        ),
        None => format!(
            "None set yet: {name} goes by its own clock. Pick yours, and your other computers take it too."
        ),
        Some(zone) if zone.carried => format!(
            "Kept from what your Mac last gave {name}: its bots' wake-ups go by it. Pick it, or another, to set it on all your computers."
        ),
        Some(_) => format!(
            "Bots' wake-ups and times of day go by it on all your computers. It's {offset} now."
        ),
    };
    let value = if kept.is_some() {
        selected
    } else {
        "own-clock"
    };
    let choice_ids: Vec<_> = choices.iter().map(|choice| &choice["id"]).collect();
    let revision =
        fingerprint(&json!({"kind": "choice", "value": value, "can": true, "choices": choice_ids}));
    let mut page = json!({
        "id": "clock", "title": "Clock",
        "groups": [{"id": "clock", "title": "Clock", "rows": [{
            "id": "time-zone", "title": "Time zone", "line": line, "kind": "choice", "value": value, "choices": choices,
            "can": true, "revision": &revision[..16],
        }]}],
    });
    if missing_rules {
        page["groups"][0]["rows"][0]["tone"] = json!("warning");
    }
    Ok(page)
}
