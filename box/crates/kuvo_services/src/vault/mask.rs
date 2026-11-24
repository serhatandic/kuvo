//! Redact inside the vault before command output crosses a tool boundary.
use std::collections::HashSet;

use base64::{
    Engine,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE_NO_PAD},
};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use serde_json::Value;
use zeroize::Zeroizing;

use super::Secret;

const URI_COMPONENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'$')
    .add(b'%')
    .add(b'&')
    .add(b'+')
    .add(b',')
    .add(b'/')
    .add(b':')
    .add(b';')
    .add(b'<')
    .add(b'=')
    .add(b'>')
    .add(b'?')
    .add(b'@')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}')
    .add(b'\x7f');

pub struct Hidden {
    pub item: String,
    pub value: Secret,
}

/// Keep the longest forms first so one value that contains another is hidden
/// under the name of the item it came from.
pub fn mask(text: String, hidden: &[Hidden]) -> String {
    let mut forms = Vec::new();
    for entry in hidden {
        if units(entry.value.expose()) < 4 {
            continue;
        }
        forms.extend(
            forms_of(entry.value.expose())
                .into_iter()
                .map(|form| (form, entry.item.as_str())),
        );
    }
    forms.sort_by_key(|(form, _)| std::cmp::Reverse(units(form)));
    let mut text = Zeroizing::new(text);
    for (form, name) in forms {
        if text.contains(form.as_str()) {
            text = Zeroizing::new(text.replace(form.as_str(), &format!("[vault: {name}]")));
        }
    }
    std::mem::take(&mut *text)
}

fn forms_of(secret: &str) -> Vec<Zeroizing<String>> {
    let mut line = Zeroizing::new(secret.to_owned());
    line.push('\n');
    let hex = Zeroizing::new(hex::encode(secret));
    let mut forms = vec![
        Zeroizing::new(secret.to_owned()),
        Zeroizing::new(STANDARD.encode(secret)),
        Zeroizing::new(STANDARD_NO_PAD.encode(secret)),
        Zeroizing::new(URL_SAFE_NO_PAD.encode(secret)),
        Zeroizing::new(STANDARD.encode(line.as_bytes())),
        Zeroizing::new(STANDARD_NO_PAD.encode(line.as_bytes())),
        Zeroizing::new(hex.to_string()),
        Zeroizing::new(hex.to_uppercase()),
        Zeroizing::new(utf8_percent_encode(secret, URI_COMPONENT).to_string()),
        json_string(secret),
    ];
    forms.retain(|text| units(text) >= 4);
    let mut add = |part: &str| {
        if units(part) >= 8 {
            forms.push(Zeroizing::new(part.to_owned()));
            forms.push(json_string(part));
        }
    };
    if secret.contains(['\r', '\n']) {
        for line in secret.split('\n') {
            add(line.trim());
        }
    }
    if hex.len() > 60 {
        for chunk in hex.as_bytes().chunks(60) {
            add(std::str::from_utf8(chunk).expect("hex is ASCII"));
        }
    }
    if let Ok(value) = serde_json::from_str::<Value>(secret) {
        hide_strings(value, &mut add);
    }
    // Preserve first appearance when two forms of a value are the same.
    let mut seen = HashSet::new();
    let indices: Vec<_> = forms
        .iter()
        .enumerate()
        .filter_map(|(index, form)| seen.insert(form.as_str()).then_some(index))
        .collect();
    let mut forms: Vec<_> = forms.into_iter().map(Some).collect();
    indices
        .into_iter()
        .filter_map(|index| forms[index].take())
        .collect()
}

fn hide_strings(value: Value, found: &mut impl FnMut(&str)) {
    match value {
        Value::String(text) => found(&Zeroizing::new(text)),
        Value::Array(values) => {
            for value in values {
                hide_strings(value, found);
            }
        }
        Value::Object(values) => {
            for value in values.into_values() {
                hide_strings(value, found);
            }
        }
        _ => {}
    }
}

fn json_string(text: &str) -> Zeroizing<String> {
    let mut encoded = Zeroizing::new(serde_json::to_string(text).expect("strings serialize"));
    encoded.pop();
    encoded.remove(0);
    encoded
}

fn units(text: &str) -> usize {
    text.encode_utf16().count()
}
