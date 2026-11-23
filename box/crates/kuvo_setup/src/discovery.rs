//! A probe supplies discovery hints. It never grants membership or access.

mod here;
pub use here::locate;

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use serde_json::Value;

use crate::{
    model::{ComputerKind, Role, safe_token},
    preferences::short_name,
};

#[derive(Clone, Serialize)]
struct Mac {
    key: String,
    name: String,
}

#[derive(Clone, Serialize)]
struct Primary {
    key: String,
    name: String,
    heard: Option<f64>,
}

struct Card {
    key: Option<String>,
    name: String,
    kind: Option<ComputerKind>,
    setup: Option<String>,
    role: Option<Role>,
    primary: Option<Primary>,
    macs: Option<Vec<Mac>>,
    computers: Option<f64>,
    items: Option<f64>,
    app: bool,
    used: bool,
}

#[derive(Default)]
struct Legacy {
    macs: Vec<String>,
    seen: HashMap<String, f64>,
    names: HashMap<String, String>,
    phones: bool,
}

struct Found<'a> {
    folder: &'a str,
    app: bool,
    card: Option<Card>,
    legacy: Option<Legacy>,
}

#[derive(Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Verdict {
    #[serde(rename = "self")]
    ThisComputer,
    Free,
    Ours,
    Older,
    Elsewhere,
    Main,
    Two,
}

#[derive(Serialize)]
struct JudgedBox<'a> {
    folder: &'a str,
    app: bool,
    verdict: Verdict,
    key: Option<String>,
    name: String,
    kind: Option<ComputerKind>,
    setup: Option<String>,
    role: Option<Role>,
    primary: Option<Primary>,
    macs: Vec<Mac>,
    computers: Option<f64>,
    items: Option<f64>,
}

#[derive(Serialize)]
pub(crate) struct Judgment<'a> {
    verdict: Verdict,
    joins: bool,
    boxes: Vec<JudgedBox<'a>>,
}

struct Ours<'a> {
    key: Option<&'a str>,
    setup: Option<&'a str>,
    keys: HashSet<&'a str>,
}

fn key(value: &str) -> bool {
    safe_token(value, 43, 43)
}

fn name(value: &Value) -> String {
    // The probe treats names as plain text even when an older card used a scalar.
    if value.is_null() {
        return String::new();
    }
    match value.as_str() {
        Some(text) => short_name(text, 100),
        None => short_name(&value.to_string(), 100),
    }
}

fn count(value: &Value) -> Option<f64> {
    value.as_f64().filter(|n| *n >= 0.0 && n.fract() == 0.0)
}

fn time(value: &Value) -> Option<f64> {
    value.as_f64().filter(|n| *n > 0.0)
}

fn card(value: &Value) -> Result<Option<Card>, String> {
    let Some(fields) = value.as_object() else {
        return Ok(None);
    };
    if let Some(version) = fields.get("formatVersion")
        && !matches!(version.as_f64(), Some(0.0 | 1.0))
    {
        let version = version
            .as_f64()
            .filter(|n| n.fract() == 0.0 && n.abs() <= 9_007_199_254_740_991.0)
            .map_or_else(|| "invalid".into(), |n| n.to_string());
        return Err(format!(
            "Setup discovery card format version {version} is unsupported (this build writes 1)."
        ));
    }
    let Some(own) = value.get("self").filter(|own| own.is_object()) else {
        return Ok(None);
    };
    let primary = &value["primary"];
    Ok(Some(Card {
        key: own["key"]
            .as_str()
            .filter(|text| key(text))
            .map(str::to_owned),
        name: name(&own["name"]),
        kind: serde_json::from_value(own["kind"].clone()).ok(),
        setup: value["setup"]
            .as_str()
            .filter(|id| {
                id.strip_prefix("set_")
                    .is_some_and(|token| safe_token(token, 22, 22))
            })
            .map(str::to_owned),
        role: serde_json::from_value(value["role"].clone()).ok(),
        primary: primary["key"]
            .as_str()
            .filter(|text| key(text))
            .map(|key| Primary {
                key: key.into(),
                name: name(&primary["name"]),
                heard: time(&primary["heard"]),
            }),
        macs: value["macs"].as_array().map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    Some(Mac {
                        key: row["key"].as_str().filter(|text| key(text))?.into(),
                        name: name(&row["name"]),
                    })
                })
                .take(16)
                .collect()
        }),
        computers: count(&value["computers"]),
        items: count(&value["items"]),
        app: value["app"] == true,
        used: value["used"] == true,
    }))
}

impl Legacy {
    fn read(&mut self, field: &str, first: &str, rest: Option<&str>) {
        match field {
            "phones" => self.phones = first == "some",
            "mac" if key(first) => {
                self.macs.retain(|known| known != first);
                self.macs.insert(0, first.into());
                self.macs.truncate(16);
            }
            "macs"
                if key(first)
                    && self.macs.len() < 16
                    && !self.macs.iter().any(|known| known == first) =>
            {
                self.macs.push(first.into());
            }
            "seen" if key(first) => {
                if let Some(n) = rest
                    .and_then(|text| text.trim().parse::<f64>().ok())
                    .filter(|n| n.is_finite() && *n > 0.0)
                {
                    self.seen.insert(first.into(), n);
                }
            }
            "name" if key(first) => {
                if let Some(text) = rest.and_then(|text| serde_json::from_str::<String>(text).ok())
                {
                    let text = short_name(&text, 100);
                    if !text.is_empty() {
                        self.names.insert(first.into(), text);
                    }
                }
            }
            _ => {}
        }
    }

    fn primary(&self) -> Option<Primary> {
        let key = self.macs.first()?.clone();
        Some(Primary {
            name: self.names.get(&key).cloned().unwrap_or_default(),
            heard: self.seen.get(&key).copied(),
            key,
        })
    }
}

fn read_probe(text: &str) -> Result<(Vec<Found<'_>>, bool), String> {
    let lines = text.split('\n').take(2000);
    let app = lines
        .clone()
        .any(|line| line.strip_prefix("app ").is_some_and(folder));
    let mut boxes: Vec<Found<'_>> = Vec::new();
    for line in lines {
        if let Some((kind, path)) = line
            .strip_prefix("box ")
            .and_then(|text| text.split_once(' '))
            && matches!(kind, "machine" | "app")
            && folder(path)
        {
            if boxes.len() == 8 {
                break;
            }
            boxes.push(Found {
                folder: path,
                app: kind == "app",
                card: None,
                legacy: None,
            });
            continue;
        }
        let Some(current) = boxes.last_mut() else {
            continue;
        };
        if let Some(given) = line.strip_prefix("card ") {
            if current.card.is_none()
                && let Ok(value) = serde_json::from_str(given)
            {
                current.card = card(&value)?;
            }
        } else if let Some(fields) = line.strip_prefix("link ") {
            let mut fields = fields.splitn(3, ' ');
            if let (Some(field), Some(first)) = (fields.next(), fields.next())
                && matches!(field, "mac" | "macs" | "phones" | "seen" | "name")
                && !first.is_empty()
                && !first.chars().any(char::is_whitespace)
            {
                current.legacy.get_or_insert_with(Legacy::default).read(
                    field,
                    first,
                    fields.next(),
                );
            }
        }
    }
    Ok((boxes, app))
}

fn folder(text: &str) -> bool {
    !text.is_empty()
        && !text.contains(['\r', '\u{2028}', '\u{2029}'])
        && text.encode_utf16().count() <= 500
}

fn verdict(found: &Found<'_>, ours: &Ours<'_>) -> Verdict {
    let legacy = found.legacy.as_ref();
    let Some(card) = &found.card else {
        return match legacy {
            Some(link) if link.macs.iter().any(|key| ours.keys.contains(key.as_str())) => {
                Verdict::Ours
            }
            Some(link) if !link.macs.is_empty() || link.phones => Verdict::Older,
            _ if found.app => Verdict::Older,
            _ => Verdict::Free,
        };
    };
    let macs = macs(found);
    if ours.key.is_some() && card.key.as_deref() == ours.key {
        Verdict::ThisComputer
    } else if card.setup.is_some()
        && (card.setup.as_deref() == ours.setup
            || ours
                .key
                .is_some_and(|key| card.primary.as_ref().is_some_and(|own| own.key == key)))
    {
        Verdict::Ours
    } else if let (Some(_), Some(role)) = (&card.setup, card.role) {
        match role {
            Role::Primary => Verdict::Main,
            Role::Member => Verdict::Elsewhere,
        }
    } else if card
        .key
        .as_deref()
        .is_some_and(|key| ours.keys.contains(key))
        || macs.iter().any(|mac| ours.keys.contains(mac.key.as_str()))
    {
        Verdict::Ours
    } else if !macs.is_empty() {
        Verdict::Elsewhere
    } else if card.app || card.used {
        Verdict::Main
    } else if card.macs.is_none() && legacy.is_none() {
        Verdict::Older
    } else {
        Verdict::Free
    }
}

fn macs(found: &Found<'_>) -> Vec<Mac> {
    if let Some(macs) = found.card.as_ref().and_then(|card| card.macs.clone()) {
        return macs;
    }
    found
        .legacy
        .iter()
        .flat_map(|legacy| {
            legacy.macs.iter().map(|key| Mac {
                key: key.clone(),
                name: legacy.names.get(key).cloned().unwrap_or_default(),
            })
        })
        .collect()
}

pub(crate) fn judge<'a>(text: &'a str, setup: &Value) -> Result<Judgment<'a>, String> {
    let ours = Ours {
        key: setup["you"]["key"].as_str(),
        setup: setup["setup"].as_str(),
        keys: setup["computers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|computer| computer["key"].as_str())
            .collect(),
    };
    let (found, app) = read_probe(text)?;
    let boxes: Vec<_> = found
        .iter()
        .map(|found| describe(found, verdict(found, &ours)))
        .collect();
    let mut occupied = boxes.iter().filter(|found| found.verdict != Verdict::Free);
    let first = occupied.next();
    let verdict = if boxes
        .iter()
        .any(|found| found.verdict == Verdict::ThisComputer)
    {
        Verdict::ThisComputer
    } else {
        match (first, occupied.next()) {
            (None, _) => Verdict::Free,
            (Some(one), None) => one.verdict,
            (Some(one), Some(two))
                if one.verdict == Verdict::Ours
                    && two.verdict == Verdict::Ours
                    && occupied.all(|each| each.verdict == Verdict::Ours) =>
            {
                Verdict::Ours
            }
            _ => Verdict::Two,
        }
    };
    let joins = verdict == Verdict::Main
        && !app
        && first
            .is_some_and(|one| one.computers.is_some_and(|n| n <= 1.0) && one.items == Some(0.0))
        && !found.iter().any(|box_| {
            box_.app
                || box_.card.as_ref().is_some_and(|card| card.app || card.used)
                || box_.legacy.as_ref().is_some_and(|legacy| legacy.phones)
        });
    Ok(Judgment {
        verdict,
        joins,
        boxes,
    })
}

fn describe<'a>(found: &Found<'a>, verdict: Verdict) -> JudgedBox<'a> {
    let card = found.card.as_ref();
    JudgedBox {
        folder: found.folder,
        app: found.app,
        verdict,
        key: card.and_then(|card| card.key.clone()),
        name: card.map(|card| card.name.clone()).unwrap_or_default(),
        kind: card.and_then(|card| card.kind),
        setup: card.and_then(|card| card.setup.clone()),
        role: card.and_then(|card| card.role),
        primary: card.and_then(|card| card.primary.clone()).or_else(|| {
            if verdict == Verdict::Older {
                found.legacy.as_ref()?.primary()
            } else {
                None
            }
        }),
        macs: macs(found),
        computers: card.and_then(|card| card.computers),
        items: card.and_then(|card| card.items),
    }
}
