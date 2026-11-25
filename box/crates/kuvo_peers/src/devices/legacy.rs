//! The roster of a released beta.1 Mac: only its direct enrollment's word,
//! never a setup certificate or a document to relay to another computer.
use std::collections::BTreeSet;

use kuvo_wire::keys::{decode_exact, encode};
use serde_json::{Map, Value, json};

use crate::{
    directory::{ComputerKey, Member},
    session::ComputerKind,
};

pub(super) fn enrollers(fields: &Map<String, Value>) -> Vec<ComputerKey> {
    if fields.get("setup").is_some_and(|setup| !setup.is_null()) {
        return vec![];
    }
    let mut seen = BTreeSet::new();
    fields
        .get("macs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .chain(fields.get("mac"))
        .filter_map(|value| decode_exact(value.as_str()?))
        .filter(|key| seen.insert(*key))
        .collect()
}

fn signs(fields: &Map<String, Value>, mac: &str) -> bool {
    fields
        .get("macSigns")
        .and_then(|signs| signs.get(mac))
        .and_then(Value::as_str)
        .and_then(decode_exact::<32>)
        .is_some()
}

fn name(value: &Value) -> String {
    value.as_str().unwrap_or_default().chars().map(|c| {
        if c.is_control() || matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') { ' ' } else { c }
    }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ").chars().take(100).collect()
}

fn peer(value: &Value) -> Option<Value> {
    let key = value["key"].as_str()?;
    let address = value["address"].as_str()?;
    decode_exact::<32>(key)?;
    decode_exact::<16>(address)?;
    let kind = value["kind"].as_str()?;
    if !matches!(kind, "mac" | "machine") {
        return None;
    }
    let mut row = json!({"key":key,"address":address,"kind":kind,"name":name(&value["name"])});
    if let Some(id) = value["id"].as_str() {
        row["id"] = Value::String(id.chars().take(200).collect());
    }
    if let Some(sign) = value["sign"]
        .as_str()
        .filter(|sign| decode_exact::<32>(sign).is_some())
    {
        row["sign"] = json!(sign);
    }
    Some(row)
}

fn moved(value: &Value) -> Option<Value> {
    let session = value["session"].as_str()?;
    let suffix = session.strip_prefix("ses_")?;
    if suffix.is_empty() || suffix.len() > 60 || !suffix.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        return None;
    }
    let key = value["key"].as_str()?;
    decode_exact::<32>(key)?;
    Some(json!({"session":session,"key":key}))
}

pub(super) fn members(fields: &Map<String, Value>, here: Option<ComputerKey>) -> Vec<Member> {
    let mut seen = BTreeSet::new();
    enrollers(fields)
        .into_iter()
        .flat_map(|mac| {
            let text = encode(&mac);
            (!signs(fields, &text))
                .then(|| fields.get("legacy")?.get(&text)?.get("peers")?.as_array())
                .flatten()
                .into_iter()
                .flatten()
                .filter_map(peer)
                .take(64)
        })
        .filter_map(|row| {
            let key = decode_exact(row["key"].as_str()?)?;
            if Some(key) == here || !seen.insert(key) {
                return None;
            }
            Some(Member {
                key,
                address: decode_exact(row["address"].as_str()?),
                kind: if row["kind"] == "mac" {
                    ComputerKind::Mac
                } else {
                    ComputerKind::Machine
                },
                name: row["name"].as_str()?.to_owned(),
            })
        })
        .collect()
}

/// A released Mac that never updates is no dead end: you can let it go from a machine. Like it
/// leaving: its list goes with it (TypeScript only aged one out, after 30 days of running).
pub(super) fn leave(fields: &mut Map<String, Value>, mac: ComputerKey) -> bool {
    if !enrollers(fields).contains(&mac) {
        return false;
    }
    let text = encode(&mac);
    if signs(fields, &text) {
        return false;
    }
    if let Some(Value::Array(macs)) = fields.get_mut("macs") {
        macs.retain(|each| each.as_str() != Some(&text));
    }
    if fields.get("mac").and_then(Value::as_str) == Some(&text) {
        fields.remove("mac");
    }
    for field in ["legacy", "macSeen", "macNames"] {
        if let Some(Value::Object(by_mac)) = fields.get_mut(field) {
            by_mac.remove(&text);
        }
    }
    true
}

pub(super) fn take(
    fields: &mut Map<String, Value>,
    mac: ComputerKey,
    payload: &Map<String, Value>,
    now: i64,
) -> bool {
    if !enrollers(fields).contains(&mac) {
        return false;
    }
    let mac = encode(&mac);
    if signs(fields, &mac) {
        return false;
    }
    let Some(peers) = payload.get("peers").and_then(Value::as_array) else {
        return false;
    };
    let Some(at) = payload.get("at") else {
        return false;
    };
    let old = fields
        .get("legacy")
        .and_then(|legacy| legacy.get(&mac))
        .and_then(|entry| entry["at"].as_f64())
        .unwrap_or(0.0);
    if at.as_f64().is_none_or(|at| at <= old) {
        return false;
    }
    let rows: Vec<_> = peers.iter().filter_map(peer).take(64).collect();
    let moves: Vec<_> = payload
        .get("moved")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(moved)
        .take(500)
        .collect();
    let record = json!({"at":at,"peers":rows,"moved":moves});
    for (field, value) in [("legacy", record), ("macSeen", json!(now))] {
        let target = fields.entry(field.to_owned()).or_insert_with(|| json!({}));
        if !target.is_object() {
            *target = json!({});
        }
        target[&mac] = value;
    }
    true
}
