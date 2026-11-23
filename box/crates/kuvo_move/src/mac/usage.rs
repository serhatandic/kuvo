//! Carry the earlier app's Usage preferences into the versioned units.
use crate::{Disk, Error, json_at, save_json};
use kuvo_store::format::{Fields, check_version};
use serde_json::{Value, json};
use std::{path::Path, time::UNIX_EPOCH};

pub(super) fn take(stage: &Disk, prefs: &Fields) -> Result<(), Error> {
    let mut input = Fields::new();
    if let Some(layout) = layout(prefs) {
        input.insert("layout".into(), layout);
    }
    let spend = [
        prefs.get("showTotalSpend"),
        prefs.get("openusage.totalSpend.period"),
        prefs.get("openusage.totalSpend.metric"),
    ];
    if spend.iter().any(Option::is_some) {
        let mut unit = Fields::new();
        unit.insert(
            "show".into(),
            json!(spend[0].and_then(Value::as_bool).unwrap_or(true)),
        );
        for (key, value) in ["period", "metric"].into_iter().zip(&spend[1..]) {
            if let Some(value) = value.and_then(valid_word::<40>) {
                unit.insert(key.into(), json!(value));
            }
        }
        input.insert("totalSpend".into(), Value::Object(unit));
    }
    let flags = ["underTenPercent", "healthyToClose", "closeToRunningOut"];
    let notification = |flag| {
        prefs
            .get(&format!("openusage.notifications.{flag}"))
            .and_then(Value::as_bool)
    };
    if flags.iter().any(|flag| notification(*flag).is_some()) {
        let flags: Fields = flags
            .into_iter()
            .map(|flag| (flag.into(), json!(notification(flag).unwrap_or(false))))
            .collect();
        input.insert("notifications".into(), Value::Object(flags));
    }
    if let Some(fallback) = prefs
        .get("openusage.codex.fallbackModel")
        .and_then(valid_word::<200>)
    {
        input.insert("codexFallback".into(), json!(fallback));
    }

    let had_input = prefs
        .get("openusage.layout.v1")
        .is_some_and(Value::is_array)
        || prefs
            .get("openusage.codex.fallbackModel")
            .and_then(Value::as_str)
            .is_some_and(|model| !model.is_empty());
    if input.is_empty() && !had_input {
        return Ok(());
    }
    let file = stage.path("agents/usage/settings.json")?;
    let saved = json_at::<Value>(&file).ok().flatten().unwrap_or_default();
    check_version("Usage settings", saved.get("kuvo"), 0, 1)?;
    if saved["imported"] == true {
        return Ok(());
    }
    let mut units = saved["units"].as_object().cloned().unwrap_or_default();
    adopt_old_codex(&file, &saved, &mut units)?;
    let mut nonce = [0u8; 8];
    getrandom::fill(&mut nonce).map_err(std::io::Error::other)?;
    let stamp = json!({
        "v": jiff::Timestamp::now().as_millisecond(), "by": format!("?{}", hex::encode(nonce)), "imported": true
    });
    for (name, value) in input {
        units
            .entry(name)
            .or_insert_with(|| json!({ "value": value, "stamp": stamp }));
    }
    let mut output =
        Fields::from_iter([("kuvo".into(), json!(1)), ("imported".into(), json!(true))]);
    if let Some(fallback) = units
        .get("codexFallback")
        .and_then(|value| value.get("value"))
    {
        output.insert("codex".into(), json!({ "fallback": fallback }));
    }
    if let Some(setup) = saved
        .get("setup")
        .filter(|value| value.is_null() || value.is_string())
    {
        output.insert("setup".into(), setup.clone());
    }
    output.insert("units".into(), Value::Object(units));
    save_json(&file, &Value::Object(output))
}

fn adopt_old_codex(file: &Path, saved: &Value, units: &mut Fields) -> Result<(), Error> {
    if units.contains_key("codexFallback") {
        return Ok(());
    }
    let Some(fallback) = saved["codex"].get("fallback") else {
        return Ok(());
    };
    let value = if fallback.is_null() || fallback == "" {
        Value::Null
    } else if let Some(model) = valid_word::<200>(fallback) {
        json!(model)
    } else {
        return Ok(());
    };
    let v = std::fs::metadata(file)?
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_or(0, |at| at.as_millis() as u64);
    units.insert(
        "codexFallback".into(),
        json!({ "value": value, "stamp": { "v": v, "by": "", "imported": true } }),
    );
    Ok(())
}

fn layout(prefs: &Fields) -> Option<Value> {
    const LAYOUT: &str = "openusage.layout.v1";
    let placed = prefs
        .get(LAYOUT)?
        .as_array()?
        .iter()
        .filter_map(|entry| {
            if entry.is_object() {
                entry["descriptorID"].as_str()
            } else {
                entry.as_str()
            }
        })
        .collect::<Vec<_>>();
    let mut layout = Fields::new();
    layout.insert("placed".into(), json!(ids(placed)?));
    for (key, pref) in [
        ("providerOrder", "providerOrder"),
        ("pinned", "menuBarPins"),
        ("expanded", "expandedMetrics"),
        ("expandOnEnable", "expandOnEnable"),
        ("seeded", "seededDefaults"),
    ] {
        let list = prefs
            .get(&format!("{LAYOUT}.{pref}"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str);
        layout.insert(key.into(), json!(ids(list.collect())?));
    }
    let mut order = Fields::new();
    if let Some(given) = prefs
        .get(&format!("{LAYOUT}.metricOrderByProvider"))
        .and_then(Value::as_object)
    {
        if given.len() > 200 {
            return None;
        }
        for (provider, list) in given {
            word::<200>(provider).then_some(())?;
            let words = list
                .as_array()?
                .iter()
                .map(Value::as_str)
                .collect::<Option<Vec<_>>>()?;
            order.insert(provider.clone(), json!(ids(words)?));
        }
    }
    layout.insert("metricOrder".into(), Value::Object(order));
    Some(Value::Object(layout))
}

fn ids(words: Vec<&str>) -> Option<Vec<&str>> {
    if words.len() > 1000 || words.iter().any(|word| !self::word::<200>(word)) {
        return None;
    }
    let mut seen = std::collections::HashSet::new();
    Some(
        words
            .into_iter()
            .filter(|word| seen.insert(*word))
            .collect(),
    )
}

fn valid_word<const LIMIT: usize>(value: &Value) -> Option<&str> {
    value.as_str().filter(|value| word::<LIMIT>(value))
}

fn word<const LIMIT: usize>(value: &str) -> bool {
    !value.is_empty() && value.encode_utf16().count() <= LIMIT && !value.chars().any(|ch| ch < ' ')
}
