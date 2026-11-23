//! JSON as the TypeScript box writes it. Canonical keys sort by UTF-16 code units.

use serde_json::{Number, Value};
use sha2::{Digest, Sha256};

pub fn canonical(value: &Value) -> String {
    let mut out = String::new();
    write(value, &mut out, true);
    out
}

pub fn digest(value: &Value) -> [u8; 32] {
    Sha256::digest(canonical(value)).into()
}

/// Ordinary JSON.stringify order, used for the original bytes of mail and documents.
pub fn ordinary(value: &Value) -> String {
    let mut out = String::new();
    write(value, &mut out, false);
    out
}

fn index(key: &str) -> Option<u32> {
    let n: u32 = key.parse().ok()?;
    (n != u32::MAX && n.to_string() == key).then_some(n)
}

fn write(value: &Value, out: &mut String, sorted: bool) {
    match value {
        Value::Object(map) => {
            let mut fields: Vec<_> = map.iter().collect();
            if sorted {
                fields.sort_unstable_by(|(a, _), (b, _)| a.encode_utf16().cmp(b.encode_utf16()));
            } else {
                fields.sort_by_key(|(key, _)| index(key).unwrap_or(u32::MAX));
            }
            out.push('{');
            for (i, (key, value)) in fields.into_iter().enumerate() {
                if i != 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key).expect("a JSON string"));
                out.push(':');
                write(value, out, sorted);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, value) in items.iter().enumerate() {
                if i != 0 {
                    out.push(',');
                }
                write(value, out, sorted);
            }
            out.push(']');
        }
        Value::Number(number) => out.push_str(&number_text(number)),
        other => out.push_str(&other.to_string()),
    }
}

fn number_text(number: &Number) -> String {
    // JSON.parse in JavaScript reads even integer literals as IEEE-754 binary64.
    let mut buffer = ryu_js::Buffer::new();
    buffer
        .format(number.as_f64().expect("a JSON number"))
        .to_owned()
}
