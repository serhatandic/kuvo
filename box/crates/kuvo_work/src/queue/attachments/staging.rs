//! Prepare client attachment bytes outside the queue lock and publish them without replacing an existing name.

use super::{is_link, open};
use crate::queue::{
    routes::internal,
    store::{self, Store},
};
use base64::{
    Engine, alphabet,
    engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
};
use http::StatusCode;
use kuvo_server::Refusal;
use kuvo_store::files;
use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};
use unicode_normalization::UnicodeNormalization;

const MOST_FILE: u64 = 25_000_000;
const MOST_FILES: usize = 50;
const TYPES: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("heic", "image/heic"),
    ("bmp", "image/bmp"),
    ("tiff", "image/tiff"),
    ("tif", "image/tiff"),
    ("svg", "image/svg+xml"),
    ("pdf", "application/pdf"),
    ("md", "text/markdown"),
    ("txt", "text/plain"),
    ("json", "application/json"),
    ("csv", "text/csv"),
    ("mp4", "video/mp4"),
    ("mov", "video/quicktime"),
];

pub(in crate::queue) struct ClientFiles {
    inline: Regex,
    draft: Regex,
}

impl ClientFiles {
    pub fn new() -> Self {
        Self {
            inline: Regex::new(r"^data:([^;,]*)(?:;[^;,]*)*;base64,")
                .expect("client attachment data URL"),
            draft: Regex::new(r"!\[Attachment\]\(kuvo-attachment:draft/([0-9]+)/[^\s)]*\)")
                .expect("client attachment picture marker"),
        }
    }

    pub fn stage(
        &self,
        store: &Store,
        given: Option<&Value>,
        local: bool,
    ) -> Result<Staged, Refusal> {
        let Some(given) = given else {
            return Ok(Staged::default());
        };
        let entries = given
            .as_array()
            .ok_or_else(|| refused("Send files as a list."))?;
        if entries.len() > MOST_FILES {
            return Err(refused("An item keeps 50 files at most."));
        }
        if entries.is_empty() {
            return Ok(Staged::default());
        }
        let root = store.root.join("staging");
        files::create_private_dirs(&root).map_err(internal)?;
        let folder = tempfile::Builder::new()
            .prefix("attach-")
            .tempdir_in(&root)
            .map_err(internal)?;
        let mut staged = Vec::with_capacity(entries.len());
        for (index, entry) in entries.iter().enumerate() {
            let (bytes, name) = self.client_file(entry, local)?;
            let name = short(
                name.replace('\\', "/")
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or_default(),
            );
            if !store::file_name(&name) {
                return Err(refused("That file needs a name."));
            }
            let path = folder.path().join(index.to_string());
            files::write_new(&path, &bytes).map_err(internal)?;
            staged.push(Entry {
                name,
                size: bytes.len(),
                digest: kuvo_wire::keys::encode(&Sha256::digest(&bytes)),
                path,
            });
        }
        Ok(Staged {
            _folder: Some(folder),
            entries: staged,
        })
    }

    pub fn publish(
        &self,
        store: &Store,
        item: &mut Value,
        staged: &Staged,
        body_changed: bool,
        now: i64,
    ) -> Result<Published, Refusal> {
        let mut published = Published::default();
        if staged.entries.is_empty() {
            return Ok(published);
        }
        let existing = store::array(item.get("files")).to_vec();
        if existing.len() + staged.entries.len() > MOST_FILES {
            return Err(refused(
                "An item keeps 50 files at most: take some away first (remove).",
            ));
        }
        let run = store
            .root
            .parent()
            .expect("Work Queue under run")
            .canonicalize()
            .map_err(|_| saving_failed())?;
        let queue = run.join("work-queue");
        let parent = queue.join("files");
        let folder = parent.join(item["id"].as_str().expect("item id"));
        for (path, create) in [(&queue, false), (&parent, true), (&folder, true)] {
            if create {
                files::create_private_dirs(path).map_err(|_| saving_failed())?;
            }
            let directory = open(path, true)
                .and_then(|file| file.metadata())
                .map_err(|_| saving_failed())?;
            if !directory.is_dir() || is_link(&directory) {
                return Err(saving_failed());
            }
        }
        let mut names: HashSet<_> = existing
            .iter()
            .filter_map(|file| file["name"].as_str())
            .map(fold)
            .collect();
        let mut added = Vec::new();
        for entry in &staged.entries {
            let base = short(
                entry
                    .name
                    .replace(['\\', '\0'], "_")
                    .trim_start_matches('.'),
            );
            let base = if base.is_empty() { "file" } else { &base };
            let (stem, extension) = split_name(base);
            let mut kept = None;
            for number in 1..=1000 {
                let name = if number == 1 {
                    base.to_owned()
                } else {
                    format!("{stem} {number}{extension}")
                };
                let key = fold(&name);
                if names.contains(&key) {
                    continue;
                }
                let destination = folder.join(&name);
                if destination.parent() != Some(folder.as_path()) {
                    return Err(saving_failed());
                }
                match fs::hard_link(&entry.path, &destination) {
                    Ok(()) => {
                        published.paths.push(destination);
                        names.insert(key);
                        kept = Some(name);
                        break;
                    }
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(_) => return Err(saving_failed()),
                }
            }
            let name = kept.ok_or_else(|| {
                refused(
                    "There are too many files with that name. Rename the attachment and try again.",
                )
            })?;
            let mut random = [0; 12];
            getrandom::fill(&mut random).map_err(internal)?;
            let anchor = random
                .into_iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            added.push(json!({"name":name,"size":entry.size,"sha256":entry.digest,"type":media_type(&name),"at":now,"anchor":anchor}));
        }
        if body_changed {
            let marked = self.draft.replace_all(
                item["body"].as_str().unwrap_or_default(),
                |capture: &regex::Captures<'_>| {
                    let file = capture[1]
                        .parse::<usize>()
                        .ok()
                        .and_then(|index| added.get(index));
                    let Some(file) = file else {
                        return capture[0].to_owned();
                    };
                    let name = file["name"].as_str().expect("kept file name");
                    let mut label = String::new();
                    for ch in name.chars() {
                        match ch {
                            '\\' | '[' | ']' => {
                                label.push('\\');
                                label.push(ch);
                            }
                            '\r' | '\n' => label.push(' '),
                            _ => label.push(ch),
                        }
                    }
                    format!(
                        "![{label}](kuvo-attachment:file/{})",
                        file["anchor"].as_str().expect("kept file anchor")
                    )
                },
            );
            if marked.encode_utf16().count() > 100_000 {
                return Err(refused(
                    "The description (body) is over 100,000 characters: make it shorter.",
                ));
            }
            item["body"] = marked.into_owned().into();
        }
        let all = existing.iter().cloned().chain(added).collect::<Vec<_>>();
        item["files"] = all.into();
        Ok(published)
    }

    fn client_file(&self, entry: &Value, local: bool) -> Result<(Vec<u8>, String), Refusal> {
        let uri = entry["uri"]
            .as_str()
            .ok_or_else(|| refused("A file needs its carried URI."))?;
        let name = entry["name"].as_str().filter(|name| !name.is_empty());
        if let Some(captures) = self.inline.captures(uri) {
            let tail = captures.get(0).expect("matched data URL").end();
            let input: Vec<_> = uri[tail..]
                .bytes()
                .take_while(|byte| *byte != b'=')
                .filter_map(|byte| match byte {
                    b'-' => Some(b'+'),
                    b'_' => Some(b'/'),
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'+' | b'/' => Some(byte),
                    _ => None,
                })
                .take((MOST_FILE as usize + 1).div_ceil(3) * 4)
                .collect();
            let engine = GeneralPurpose::new(
                &alphabet::STANDARD,
                GeneralPurposeConfig::new()
                    .with_decode_padding_mode(DecodePaddingMode::Indifferent)
                    .with_decode_allow_trailing_bits(true),
            );
            let used = input.len() - usize::from(input.len() % 4 == 1);
            let bytes = engine.decode(&input[..used]).map_err(internal)?;
            if bytes.len() as u64 > MOST_FILE {
                return Err(too_big());
            }
            let mime = captures[1].to_ascii_lowercase();
            let name = name.map(str::to_owned).unwrap_or_else(|| {
                let extension = TYPES
                    .iter()
                    .find(|(_, kind)| *kind == mime)
                    .map_or("bin", |(ext, _)| ext);
                format!("attachment.{extension}")
            });
            return Ok((bytes, name));
        }
        if !local || !uri.starts_with("file://") {
            return Err(refused("Send the file's contents with it."));
        }
        let unreadable = || refused("That attached file can't be read: attach it again.");
        let file = url::Url::parse(uri)
            .ok()
            .and_then(|value| value.to_file_path().ok())
            .ok_or_else(unreadable)?;
        let handle = open(&file, false).map_err(|_| unreadable())?;
        let metadata = handle.metadata().map_err(|_| unreadable())?;
        if !metadata.is_file() || is_link(&metadata) {
            return Err(refused("That attached file isn't a file."));
        }
        if metadata.len() > MOST_FILE {
            return Err(too_big());
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        handle
            .take(metadata.len())
            .read_to_end(&mut bytes)
            .map_err(|_| unreadable())?;
        let name = name.map(str::to_owned).unwrap_or_else(|| {
            file.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
        Ok((bytes, name))
    }
}

#[derive(Default)]
pub(in crate::queue) struct Staged {
    _folder: Option<tempfile::TempDir>,
    entries: Vec<Entry>,
}

impl Staged {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

struct Entry {
    path: PathBuf,
    name: String,
    size: usize,
    digest: String,
}

#[derive(Default)]
pub(in crate::queue) struct Published {
    paths: Vec<PathBuf>,
}

impl Published {
    pub fn commit(mut self) {
        self.paths.clear();
    }
}

impl Drop for Published {
    fn drop(&mut self) {
        for file in &self.paths {
            let _ = fs::remove_file(file);
        }
    }
}

fn fold(name: &str) -> String {
    name.nfc().collect::<String>().to_lowercase()
}

fn split_name(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(at) if at > 0 && name != ".." => name.split_at(at),
        _ => (name, ""),
    }
}

fn short(name: &str) -> String {
    let (mut stem, extension) = split_name(name);
    let extension = store::limit(extension, 20);
    while stem.len() + extension.len() > 180 {
        stem = &stem[..stem.char_indices().next_back().map_or(0, |(at, _)| at)];
    }
    format!("{stem}{extension}")
}

fn media_type(name: &str) -> &'static str {
    let extension = Path::new(name).extension().and_then(|ext| ext.to_str());
    TYPES
        .iter()
        .find(|(ext, _)| extension.is_some_and(|value| ext.eq_ignore_ascii_case(value)))
        .map_or("application/octet-stream", |(_, kind)| kind)
}

fn refused(message: &str) -> Refusal {
    Refusal::new(StatusCode::BAD_REQUEST, message)
}
fn saving_failed() -> Refusal {
    refused("The attachment couldn’t be saved. Check the available space and try again.")
}
fn too_big() -> Refusal {
    refused("A file is over 25 MB: too big to keep with an item.")
}
