//! Antigravity's bounded generation metadata, decoded without opening reply bodies.
use super::native_logs::{Counts, Observation};
use rusqlite::Connection;
use std::path::Path;
const MAX_BLOB: usize = 1_048_576;
const SAFE: u64 = 9_007_199_254_740_991;
enum Field<'a> {
    Number(u64),
    Bytes(&'a [u8]),
}
fn varint(bytes: &[u8], at: &mut usize) -> Option<u64> {
    let mut value = 0;
    for i in 0..10 {
        let byte = *bytes.get(*at)?;
        *at += 1;
        if i == 9 && byte & 0x7f > 1 {
            return None;
        }
        value |= u64::from(byte & 0x7f) << (i * 7);
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}
fn field(mut bytes: &[u8], wanted: u64) -> Option<Field<'_>> {
    while !bytes.is_empty() {
        let mut at = 0;
        let tag = varint(bytes, &mut at)?;
        let number = tag >> 3;
        if number == 0 || number > u32::MAX as u64 {
            return None;
        }
        let found = match tag & 7 {
            0 => Some(Field::Number(varint(bytes, &mut at)?)),
            2 => {
                let len: usize = varint(bytes, &mut at)?.try_into().ok()?;
                let end = at.checked_add(len)?;
                let body = bytes.get(at..end)?;
                at = end;
                Some(Field::Bytes(body))
            }
            1 => {
                at = at.checked_add(8)?;
                None
            }
            5 => {
                at = at.checked_add(4)?;
                None
            }
            _ => return None,
        };
        if at > bytes.len() {
            return None;
        }
        if number == wanted && found.is_some() {
            return found;
        }
        bytes = &bytes[at..];
    }
    None
}
fn bytes(data: &[u8], n: u64) -> Option<&[u8]> {
    match field(data, n)? {
        Field::Bytes(v) => Some(v),
        _ => None,
    }
}
fn integer(data: &[u8], n: u64) -> Option<u64> {
    match field(data, n)? {
        Field::Number(v) => Some(v),
        _ => None,
    }
}
fn text(data: &[u8], n: u64) -> Option<String> {
    let text = std::str::from_utf8(bytes(data, n)?).ok()?.trim();
    (!text.is_empty()).then(|| text.to_owned())
}
fn timestamp(data: &[u8]) -> Option<i64> {
    let seconds = integer(data, 1)?;
    if seconds == 0 || seconds > SAFE / 1000 {
        return None;
    }
    (seconds * 1000).try_into().ok()
}
pub fn generation(
    data: &[u8],
    step: Option<&[u8]>,
    session: &str,
    index: u64,
) -> Option<Observation> {
    if data.len() > MAX_BLOB {
        return None;
    }
    let wrapped = bytes(data, 1)?;
    let usage = bytes(wrapped, 4)?;
    let system = integer(usage, 1).unwrap_or(0);
    let input = integer(usage, 2).unwrap_or(0);
    let output = integer(usage, 3).unwrap_or(0);
    let cache = integer(usage, 5).unwrap_or(0);
    let prompt = system.checked_add(input)?;
    let processed = prompt.checked_add(output)?.checked_add(cache)?;
    if processed > SAFE || processed == 0 {
        return None;
    }
    let id = text(wrapped, 19).map(|id| id.strip_suffix("-tiered").unwrap_or(&id).to_owned());
    let label = text(wrapped, 21);
    let generated = input != 0 || output != 0 || cache != 0;
    if !generated && (id.is_none() && label.is_none() || prompt == 0) {
        return None;
    }
    let at = bytes(wrapped, 9)
        .and_then(|t| bytes(t, 4))
        .and_then(timestamp)
        .or_else(|| {
            step.filter(|s| s.len() <= MAX_BLOB)
                .and_then(|s| bytes(s, 1))
                .and_then(timestamp)
        })?;
    let (model, alternate) = if id.as_deref().is_some_and(|id| id.ends_with("-default")) {
        (label.or_else(|| id.clone()), id)
    } else {
        (id.or_else(|| label.clone()), label)
    };
    Some(Observation {
        at,
        session: Some(session.into()),
        model,
        pricing_model: alternate,
        counts: Counts {
            input: prompt,
            output,
            cache_read: cache,
            ..Default::default()
        },
        billed: None,
        call: Some(index.to_string()),
        message: None,
        request: None,
        sidechain: false,
        has_speed: false,
    })
}
/// There is no row modification clock in this maker DB; a changed channel is
/// read in small keyed pages. Unchanged channel stamps never open SQLite.
pub fn read(
    db: &Connection,
    session: &str,
    since: i64,
    mut take: impl FnMut(u64, Observation) -> Result<(), Box<dyn std::error::Error>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let read = db.unchecked_transaction()?;
    let supported: bool = read.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='gen_metadata')",
        [],
        |r| r.get(0),
    )?;
    if !supported {
        return Ok(());
    }
    let steps: bool = read.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('steps') WHERE name='metadata')",
        [],
        |r| r.get(0),
    )?;
    let step = if steps {
        "(SELECT CASE WHEN length(metadata)<=1048576 THEN metadata END FROM steps WHERE idx=g.idx)"
    } else {
        "NULL"
    };
    let sql = format!(
        "SELECT idx, CASE WHEN length(data)<=1048576 THEN data END,{step} FROM gen_metadata g WHERE idx>? AND data IS NOT NULL ORDER BY idx LIMIT 8"
    );
    let mut query = read.prepare(&sql)?;
    let mut index = -1i64;
    loop {
        let mut rows = query.query([index])?;
        let mut count = 0;
        while let Some(row) = rows.next()? {
            let next: i64 = row.get(0)?;
            if next <= index {
                return Err("Invalid Antigravity generation index".into());
            }
            index = next;
            count += 1;
            // Get only the bounded blobs of this generation.
            let data: Option<Vec<u8>> = row.get(1)?;
            let step: Option<Vec<u8>> = row.get(2)?;
            if let Some(event) = data
                .as_ref()
                .and_then(|blob| generation(blob, step.as_deref(), session, index as u64))
                .filter(|e| e.at >= since)
            {
                take(index as u64, event)?;
            }
        }
        if count < 8 {
            break;
        }
    }
    drop(query);
    read.commit()?;
    Ok(())
}
pub fn database_session(path: &Path) -> Option<&str> {
    path.file_stem()?.to_str().filter(|s| !s.is_empty())
}
#[cfg(test)]
mod parity_corpus {
    use super::*;
    use serde_json::Value;
    #[test]
    fn bounded_generation_fixture() {
        let raw: Value = serde_json::from_str(include_str!(
            "../../../../test/fixtures/usage/antigravity-blobs.json"
        ))
        .unwrap();
        for (index, case) in raw["cases"].as_array().unwrap().iter().enumerate() {
            let blob = hex::decode(case["hex"].as_str().unwrap()).unwrap();
            let step = case["stepHex"]
                .as_str()
                .map(|raw| hex::decode(raw).unwrap());
            let got = generation(&blob, step.as_deref(), "fixture", index as u64);
            if case["expected"].is_null() {
                assert!(got.is_none(), "{}", case["name"]);
                continue;
            }
            let expected = &case["expected"];
            let got = got.unwrap_or_else(|| panic!("{}", case["name"]));
            assert_eq!(
                got.counts.input,
                expected["inputTokens"].as_u64().unwrap(),
                "{}",
                case["name"]
            );
            assert_eq!(
                got.counts.output,
                expected["outputTokens"].as_u64().unwrap(),
                "{}",
                case["name"]
            );
            assert_eq!(
                got.counts.cache_read,
                expected["cacheReadTokens"].as_u64().unwrap(),
                "{}",
                case["name"]
            );
            assert_eq!(
                got.at,
                expected["timestampSeconds"].as_i64().unwrap() * 1000,
                "{}",
                case["name"]
            );
            let id = expected["modelID"].as_str();
            let label = expected["label"].as_str();
            let name = if id.is_some_and(|id| id.ends_with("-default")) {
                label.or(id)
            } else {
                id.or(label)
            }
            .map(|n| n.strip_suffix("-tiered").unwrap_or(n));
            assert_eq!(got.model.as_deref(), name, "{}", case["name"]);
        }
    }

    #[test]
    fn shared_blob_size_limits_keep_embedded_time_and_inclusive_cap() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../../test/fixtures/usage/antigravity-blobs.json"
        ))
        .unwrap();
        let mut events = vec![];
        for (index, row) in fixture["bounded"]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            let case = &fixture["cases"][row["case"].as_u64().unwrap() as usize];
            let mut data = hex::decode(case["hex"].as_str().unwrap()).unwrap();
            let mut step = case["stepHex"]
                .as_str()
                .map(|hex| hex::decode(hex).unwrap());
            if let Some(size) = row["dataBytes"].as_u64() {
                data.resize(size as usize, 0);
            }
            if let Some(size) = row["stepBytes"].as_u64() {
                step.get_or_insert_with(Vec::new).resize(size as usize, 0);
            }
            if let Some(event) = generation(&data, step.as_deref(), "fixture", index as u64) {
                events.push(event);
            }
        }
        assert_eq!(
            events.len(),
            fixture["bounded"]["events"].as_u64().unwrap() as usize
        );
        assert_eq!(
            events.iter().map(|e| e.counts.processed()).sum::<u64>(),
            fixture["bounded"]["tokens"].as_u64().unwrap()
        );
    }
}
