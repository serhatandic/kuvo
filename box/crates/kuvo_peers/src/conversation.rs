//! The last page of a conversation carried from a peer, without large inline attachments.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use kuvo_wire::keys::encode;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{directory::ComputerKey, kept, reach::Millis};

const MAX_MESSAGES_BYTES: usize = 2 * 1024 * 1024;
const MAX_PAGES: usize = 64;
const INLINE_BYTES: usize = 4096;

#[derive(Deserialize, Serialize)]
pub struct Page {
    pub at: Millis,
    pub data: Vec<Value>,
}

pub fn valid_session(id: &str) -> bool {
    id.strip_prefix("ses_").is_some_and(|suffix| {
        (1..=95).contains(&suffix.len())
            && suffix
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    })
}

fn folder(run: &Path, key: &ComputerKey) -> PathBuf {
    run.join("peers").join(encode(key)).join("conversations")
}

pub fn load(run: &Path, key: &ComputerKey, session: &str) -> Option<Page> {
    valid_session(session)
        .then(|| kept::read(&folder(run, key).join(format!("{session}.json"))))
        .flatten()
}

/// The conversation follow owner calls this after receiving a complete newest page.
pub fn keep(run: &Path, key: &ComputerKey, session: &str, mut page: Page) -> io::Result<()> {
    if !valid_session(session) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let directory = folder(run, key);
    let path = directory.join(format!("{session}.json"));
    if kept::read::<Page>(&path).is_some_and(|before| before.at > page.at) {
        return Ok(());
    }
    let mut bytes = 0;
    let mut count = 0;
    for value in &mut page.data {
        bytes += without_inline(value);
        if bytes > MAX_MESSAGES_BYTES {
            break;
        }
        count += 1;
    }
    page.data.truncate(count);
    kept::write(&path, &page)?;
    let mut copies: Vec<_> = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .collect();
    if copies.len() > MAX_PAGES {
        copies.sort_by_key(|entry| {
            (
                entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .ok(),
                entry.file_name(),
            )
        });
        let prune = copies.len() - MAX_PAGES;
        for entry in copies.into_iter().take(prune) {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

fn without_inline(value: &mut Value) -> usize {
    fn trim(value: &mut Value) {
        match value {
            Value::String(text) if text.len() > INLINE_BYTES && text.starts_with("data:") => {
                text.truncate(text.find(',').map_or(0, |comma| comma + 1));
            }
            Value::Object(fields) => fields.values_mut().for_each(trim),
            Value::Array(items) => items.iter_mut().for_each(trim),
            _ => {}
        }
    }
    trim(value);
    serde_json::to_vec(value).map_or(MAX_MESSAGES_BYTES, |bytes| bytes.len() + 1)
}
