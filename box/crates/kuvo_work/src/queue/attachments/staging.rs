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
                id: None,
                client: true,
            });
        }
        Ok(Staged {
            _folder: Some(folder),
            entries: staged,
        })
    }

    pub fn stage_agent(
        &self,
        store: &Store,
        given: Option<&Value>,
        directory: Option<&Path>,
    ) -> Result<Staged, Refusal> {
        let Some(given) = given else {
            return Ok(Staged::default());
        };
        let names = given
            .as_array()
            .map_or(std::slice::from_ref(given), Vec::as_slice)
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .collect::<Vec<_>>();
        if names.is_empty() {
            return Ok(Staged::default());
        }
        if names.len() > MOST_FILES {
            return Err(refused("An item keeps 50 files at most."));
        }
        let places = AgentPlaces::read(store, directory);
        let root = store.root.join("staging");
        files::create_private_dirs(&root).map_err(internal)?;
        let folder = tempfile::Builder::new()
            .prefix("attach-")
            .tempdir_in(&root)
            .map_err(internal)?;
        let mut entries = Vec::with_capacity(names.len());
        for (index, given) in names.into_iter().enumerate() {
            let (name, bytes) = places.read_file(given)?;
            let path = folder.path().join(index.to_string());
            files::write_new(&path, &bytes).map_err(internal)?;
            entries.push(Entry {
                digest: kuvo_wire::keys::encode(&Sha256::digest(&bytes)),
                size: bytes.len(),
                name,
                path,
                id: None,
                client: false,
            });
        }
        Ok(Staged {
            _folder: Some(folder),
            entries,
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
            let mut file = json!({"name":name,"size":entry.size,"sha256":entry.digest,"type":media_type(&name),"at":now});
            if entry.client {
                let mut random = [0; 12];
                getrandom::fill(&mut random).map_err(internal)?;
                let anchor = random
                    .into_iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>();
                file["anchor"] = anchor.into();
            }
            if let Some(id) = &entry.id {
                file["id"] = id.clone().into();
            }
            added.push(file);
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

struct AgentPlaces {
    own: Option<PathBuf>,
    roots: Vec<PathBuf>,
    private: Vec<PathBuf>,
    library: Vec<PathBuf>,
    mac_app_temp: Option<Regex>,
}

impl AgentPlaces {
    fn read(store: &Store, directory: Option<&Path>) -> Self {
        let home = store
            .root
            .parent()
            .and_then(Path::parent)
            .expect("Work Queue under home");
        let own = directory.and_then(|folder| folder.canonicalize().ok());
        let projects: Value = fs::read(home.join("box/projects.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let others = store::array(projects.get("projects"))
            .iter()
            .find(|project| {
                directory.is_some_and(|directory| {
                    project["directory"].as_str().map(Path::new) == Some(directory)
                })
            })
            .map(|project| store::array(project.get("folders")))
            .unwrap_or_default();
        let roots = own
            .iter()
            .cloned()
            .chain(
                others
                    .iter()
                    .filter_map(Value::as_str)
                    .map(PathBuf::from)
                    .chain([std::env::temp_dir(), PathBuf::from("/tmp")])
                    .filter_map(|root| root.canonicalize().ok()),
            )
            .collect();
        let app = std::env::var_os("KUVO_BOX").is_some_and(|kind| kind == "mac");
        let bots = std::env::var_os("KUVO_BOTS").map_or_else(|| home.join("bots"), PathBuf::from);
        let private = [
            Some(home.to_owned()),
            Some(bots),
            home.parent().filter(|_| app).map(Path::to_owned),
        ]
        .into_iter()
        .flatten()
        .filter_map(|root| root.canonicalize().ok())
        .collect();
        let on_mac = cfg!(target_os = "macos") && !app;
        let library = if on_mac {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .into_iter()
                .chain(
                    kuvo_os::platform::Account::current()
                        .ok()
                        .map(|account| account.home),
                )
                .map(|home| home.canonicalize().unwrap_or(home).join("Library"))
                .collect()
        } else {
            Vec::new()
        };
        let mac_app_temp = on_mac.then(|| {
            Regex::new(r"(?i)^/private/var/folders/[^/]+/[^/]+/[^/]+/run\.kuvo\.code[^/]*(/|$)")
                .expect("Mac app temporary folders")
        });
        Self {
            own,
            roots,
            private,
            library,
            mac_app_temp,
        }
    }

    fn read_file(&self, given: &str) -> Result<(String, Vec<u8>), Refusal> {
        let path = Path::new(given);
        if !path.is_absolute() {
            return Err(refused(&format!("Give {given} as a full path.")));
        }
        let file = path
            .canonicalize()
            .map_err(|_| refused(&format!("There's no file at {given}.")))?;
        let denied = || {
            refused(&format!(
                "{given} isn't in this conversation's folder, its project or a temporary folder: only files from there are kept with an item."
            ))
        };
        let root = self
            .roots
            .iter()
            .find(|root| file.starts_with(root))
            .ok_or_else(denied)?;
        let own_private = Some(root) == self.own.as_ref()
            && self.private.iter().any(|base| root.starts_with(base));
        let mac_private = self
            .mac_app_temp
            .as_ref()
            .is_some_and(|pattern| pattern.is_match(&file.to_string_lossy()))
            || self.library.iter().any(|library| {
                file.strip_prefix(library)
                    .ok()
                    .and_then(|path| path.components().nth(1))
                    .is_some_and(|component| {
                        component
                            .as_os_str()
                            .to_string_lossy()
                            .to_ascii_lowercase()
                            .starts_with("run.kuvo.code")
                    })
            });
        if !own_private && (mac_private || self.private.iter().any(|base| file.starts_with(base))) {
            return Err(denied());
        }
        if file
            .strip_prefix(root)
            .expect("attachment under root")
            .components()
            .any(|part| {
                part.as_os_str()
                    .to_str()
                    .is_some_and(|name| name.starts_with('.'))
            })
        {
            return Err(refused(&format!(
                "{given} is hidden (a name starting with a dot): it isn't kept with an item."
            )));
        }
        let unreadable = |_| refused(&format!("{given} can't be read."));
        let meta = fs::symlink_metadata(&file).map_err(unreadable)?;
        if !meta.is_file() || is_link(&meta) {
            return Err(refused(&format!("{given} isn't a file.")));
        }
        if meta.len() > MOST_FILE {
            return Err(refused(&format!(
                "{given} is over 25 MB: too big to keep with an item."
            )));
        }
        let changed = || refused(&format!("{given} changed as it was read: try again."));
        let stamp = files::file_stamp(&file)
            .map_err(unreadable)?
            .ok_or_else(changed)?;
        let opened = files::open_payload(&file).map_err(|error| {
            if error.kind() == io::ErrorKind::InvalidData {
                refused(&format!(
                    "{given} has other names (a hard link): it isn't kept with an item."
                ))
            } else {
                unreadable(error)
            }
        })?;
        if files::opened_file_stamp(&opened).map_err(unreadable)? != stamp
            || path.canonicalize().ok().as_ref() != Some(&file)
        {
            return Err(changed());
        }
        let mut bytes = Vec::with_capacity(stamp.size.min(MOST_FILE) as usize);
        opened
            .take(stamp.size.min(MOST_FILE))
            .read_to_end(&mut bytes)
            .map_err(unreadable)?;
        if path.canonicalize().ok().as_ref() != Some(&file) {
            return Err(changed());
        }
        let name = file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or("file".into());
        Ok((name, bytes))
    }
}

#[derive(Default)]
pub(in crate::queue) struct Staged {
    _folder: Option<tempfile::TempDir>,
    entries: Vec<Entry>,
}

impl Staged {
    pub fn for_computer(&self, computer: &str) -> Result<Value, Refusal> {
        if self.entries.iter().map(|entry| entry.size).sum::<usize>() > 4_000_000 {
            return Err(Refusal::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                format!(
                    "These files are too big to send to {computer} with one change (4 MB in all at most)."
                ),
            ));
        }
        let base64 = base64::engine::general_purpose::STANDARD;
        self.entries.iter().map(|entry| {
            let content = fs::read(&entry.path).map_err(internal)?;
            let name = &entry.name;
            Ok(json!({"name":name,"uri":format!("data:{};base64,{}",media_type(name),base64.encode(content))}))
        }).collect::<Result<Vec<_>,_>>().map(Value::from)
    }

    pub fn with_ids(mut self, ids: impl Iterator<Item = String>) -> Self {
        for (entry, id) in self.entries.iter_mut().zip(ids) {
            entry.id = Some(id);
        }
        self
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

struct Entry {
    path: PathBuf,
    name: String,
    size: usize,
    digest: String,
    id: Option<String>,
    client: bool,
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
