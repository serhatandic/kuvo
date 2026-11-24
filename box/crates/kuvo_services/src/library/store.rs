//! This computer's Library index (`library/log.jsonl`), matching `box/src/link/library/store.ts` 1:1.

use std::{
    collections::{BTreeSet, HashMap},
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use jiff::Timestamp;
use kuvo_store::format::ObjectFormat;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::broadcast;

use crate::artifacts::{
    kinds::{basename, strip_folder_prefix, type_of},
    policy::is_secret,
    store::{Clock, stamp, system_clock},
};

pub const LIBRARY_INDEX_FORMAT: ObjectFormat = ObjectFormat::additive("Library index");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LibraryKind {
    Image,
    Document,
    Page,
    Other,
}

impl LibraryKind {
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "image" => Some(Self::Image),
            "document" => Some(Self::Document),
            "page" => Some(Self::Page),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum How {
    Shown,
    Generated,
    Attached,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub path: String,
    pub name: String,
    pub kind: LibraryKind,
    #[serde(rename = "type")]
    pub mime: String,
    pub how: How,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<String>,
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Handover {
    pub path: String,
    pub how: How,
    pub bot: Option<String>,
    pub session: Option<String>,
    pub group: Option<String>,
    pub message: Option<String>,
    pub part: Option<String>,
    pub at: Option<Timestamp>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    pub source: Option<String>,
    pub kind: Option<LibraryKind>,
    pub query: Option<String>,
    pub hidden: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum LibraryStoreError {
    #[error("{0}")]
    UnsupportedFormat(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl From<kuvo_store::format::Error> for LibraryStoreError {
    fn from(err: kuvo_store::format::Error) -> Self {
        match err {
            kuvo_store::format::Error::Unsupported { .. } => {
                Self::UnsupportedFormat(err.to_string())
            }
            other => Self::Io(io::Error::other(other)),
        }
    }
}

fn extension(path: &str) -> String {
    let name = basename(path);
    let Some(dot) = name.rfind('.') else {
        return String::new();
    };
    if dot == 0 {
        return String::new();
    }
    name[dot + 1..].to_ascii_lowercase()
}

pub fn library_kind(path: &str) -> LibraryKind {
    match extension(path).as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "heif" | "svg" | "bmp" | "tiff"
        | "tif" | "avif" => LibraryKind::Image,
        "html" | "htm" => LibraryKind::Page,
        "pdf" | "doc" | "docx" | "rtf" | "pages" | "odt" | "md" | "markdown" | "txt" => {
            LibraryKind::Document
        }
        _ => LibraryKind::Other,
    }
}

const SKILL_FOLDERS: &[&str] = &[
    ".opencode",
    ".claude",
    "opencode",
    ".agents",
    ".codex",
    ".cursor",
    ".gemini",
    ".grok",
];

pub fn handed_over(path: &str, bots: Option<&Path>) -> bool {
    let name = basename(path);
    if name.is_empty()
        || is_secret(path)
        || matches!(
            extension(name).as_str(),
            "log" | "tmp" | "swp" | "crdownload" | "part"
        )
        || name.ends_with('~')
    {
        return false;
    }
    if let Some(bots) = bots
        && let Some(inside) = strip_folder_prefix(path, &bots.to_string_lossy())
    {
        let parts: Vec<&str> = inside.split(std::path::MAIN_SEPARATOR).collect();
        if parts.len() == 2 && matches!(parts[1], "MEMORY.md" | "job.json") {
            return false;
        }
        if parts.get(1) == Some(&"groups") {
            return false;
        }
    }
    let parts: Vec<&str> = path.split(std::path::MAIN_SEPARATOR).collect();
    !parts.iter().enumerate().any(|(idx, &part)| {
        part == "skills"
            && idx > 0
            && SKILL_FOLDERS.contains(&parts[idx - 1])
    })
}

/// Extract an HTML page's `<title>...</title>` from its first 64 KiB.
pub fn title_of(path: &str) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let mut buf = vec![0u8; 64 * 1024];
    let n = file.read(&mut buf).ok()?;
    buf.truncate(n);
    let text = String::from_utf8_lossy(&buf);
    let lower = text.to_ascii_lowercase();
    let open = lower.find("<title")?;
    let gt = lower[open..].find('>')? + open + 1;
    let close = lower[gt..].find("</title>")? + gt;
    let raw = &text[gt..close];
    let unescaped = raw
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    let collapsed = unescaped
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if collapsed.is_empty() {
        None
    } else {
        Some(collapsed.chars().take(120).collect())
    }
}

pub fn matches_library(
    filter: &Filter,
    name: &str,
    kind: LibraryKind,
    bot: Option<&str>,
    is_hidden: bool,
    bot_name: impl Fn(&str) -> Option<String>,
    computer: &str,
) -> bool {
    if is_hidden != filter.hidden {
        return false;
    }
    if let Some(wanted_kind) = filter.kind
        && kind != wanted_kind
    {
        return false;
    }
    if let Some(source) = filter.source.as_deref()
        && source != "all"
    {
        if source == "you" {
            if bot.is_some() {
                return false;
            }
        } else if bot != Some(source) {
            return false;
        }
    }
    let words: Vec<String> = filter
        .query
        .as_deref()
        .unwrap_or("")
        .to_lowercase()
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    if words.is_empty() {
        return true;
    }
    let who = match bot {
        Some(id) => bot_name(id).unwrap_or_default(),
        None => "you".to_owned(),
    };
    let haystack = format!("{name}\n{who}\n{computer}").to_lowercase();
    words.iter().all(|word| haystack.contains(word))
}

struct Inner {
    root: PathBuf,
    bots: Option<PathBuf>,
    clock: Clock,
    items: HashMap<String, Item>,
    lines: usize,
    changes: broadcast::Sender<Item>,
}

#[derive(Clone)]
pub struct LibraryStore {
    inner: Arc<Mutex<Inner>>,
}

impl LibraryStore {
    pub fn open(
        root: impl AsRef<Path>,
        bots: Option<PathBuf>,
        clock: Option<Clock>,
    ) -> Result<Self, LibraryStoreError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root)?;
        let (changes, _) = broadcast::channel(256);
        let mut inner = Inner {
            root,
            bots,
            clock: clock.unwrap_or_else(|| Arc::new(system_clock)),
            items: HashMap::new(),
            lines: 0,
            changes,
        };
        inner.load()?;
        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
        })
    }

    pub fn id_of(path: &str) -> String {
        let digest = hex::encode(Sha256::digest(path.as_bytes()));
        format!("lib_{}", &digest[..24])
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Item> {
        self.inner.lock().unwrap().changes.subscribe()
    }

    pub fn get(&self, id: &str) -> Option<Item> {
        self.inner.lock().unwrap().items.get(id).cloned()
    }

    pub fn hand(&self, handover: Handover) -> Option<Item> {
        let mut inner = self.inner.lock().unwrap();
        if !Path::new(&handover.path).is_absolute()
            || !handed_over(&handover.path, inner.bots.as_deref())
        {
            return None;
        }
        let id = Self::id_of(&handover.path);
        let kind = library_kind(&handover.path);
        let at = stamp(handover.at.unwrap_or_else(|| (inner.clock)()));
        if let Some(known) = inner.items.get(&id)
            && known.at > at
        {
            return None;
        }
        let name = if kind == LibraryKind::Page {
            title_of(&handover.path)
        } else {
            None
        }
        .unwrap_or_else(|| basename(&handover.path).to_owned());
        let item = Item {
            id: id.clone(),
            path: handover.path.clone(),
            name,
            kind,
            mime: type_of(&handover.path, false).to_owned(),
            how: handover.how,
            bot: handover.bot,
            session: handover.session,
            group: handover.group,
            message: handover.message,
            part: handover.part,
            at,
            hidden: None,
        };
        inner.items.insert(id, item.clone());
        let _ = inner.write(&item);
        let _ = inner.changes.send(item.clone());
        Some(item)
    }

    pub fn hide(&self, id: &str, hidden: bool) -> Option<Item> {
        let mut inner = self.inner.lock().unwrap();
        let current_hidden = inner.items.get(id)?.hidden.is_some();
        if current_hidden == hidden {
            return inner.items.get(id).cloned();
        }
        let now = stamp((inner.clock)());
        let item = {
            let item = inner.items.get_mut(id)?;
            if hidden {
                item.hidden = Some(now);
            } else {
                item.hidden = None;
            }
            item.clone()
        };
        let _ = inner.write(&item);
        let _ = inner.changes.send(item.clone());
        Some(item)
    }

    pub fn list(
        &self,
        filter: &Filter,
        bot_name: impl Fn(&str) -> Option<String>,
        computer: &str,
    ) -> Vec<Item> {
        let inner = self.inner.lock().unwrap();
        let mut out: Vec<Item> = inner
            .items
            .values()
            .filter(|item| {
                matches_library(
                    filter,
                    &item.name,
                    item.kind,
                    item.bot.as_deref(),
                    item.hidden.is_some(),
                    &bot_name,
                    computer,
                )
            })
            .cloned()
            .collect();
        out.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| b.id.cmp(&a.id)));
        out
    }

    pub fn sources(&self) -> (Vec<String>, bool) {
        let inner = self.inner.lock().unwrap();
        let mut bots = BTreeSet::new();
        let mut you = false;
        for item in inner.items.values() {
            if item.hidden.is_some() {
                continue;
            }
            if let Some(bot) = &item.bot {
                bots.insert(bot.clone());
            } else {
                you = true;
            }
        }
        (bots.into_iter().collect(), you)
    }
}

impl Inner {
    fn write(&mut self, item: &Item) -> io::Result<()> {
        let Value::Object(fields) = serde_json::to_value(item).map_err(io::Error::other)? else {
            unreachable!();
        };
        let encoded = serde_json::to_string(&LIBRARY_INDEX_FORMAT.encode(fields))
            .map_err(io::Error::other)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join("log.jsonl"))?;
        file.write_all(encoded.as_bytes())?;
        file.write_all(b"\n")?;
        drop(file);
        self.lines += 1;
        if self.lines > 1000 && self.lines > self.items.len() * 2 {
            self.compact();
        }
        Ok(())
    }

    fn compact(&mut self) {
        let file = self.root.join("log.jsonl");
        let tmp = self.root.join("log.jsonl.new");
        let res = (|| -> io::Result<()> {
            let mut out = File::create(&tmp)?;
            for item in self.items.values() {
                let Value::Object(fields) =
                    serde_json::to_value(item).map_err(io::Error::other)?
                else {
                    unreachable!();
                };
                let encoded = serde_json::to_string(&LIBRARY_INDEX_FORMAT.encode(fields))
                    .map_err(io::Error::other)?;
                out.write_all(encoded.as_bytes())?;
                out.write_all(b"\n")?;
            }
            out.flush()?;
            drop(out);
            fs::rename(&tmp, &file)?;
            self.lines = self.items.len();
            Ok(())
        })();
        if res.is_err() {
            let _ = fs::remove_file(&tmp);
        }
    }

    fn load(&mut self) -> Result<(), LibraryStoreError> {
        let file = self.root.join("log.jsonl");
        let text = match fs::read_to_string(&file) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(err.into()),
        };
        for line in text.split('\n') {
            if line.is_empty() {
                continue;
            }
            self.lines += 1;
            let Ok(saved) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if !saved.is_object() {
                continue;
            }
            let decoded = LIBRARY_INDEX_FORMAT.decode(saved)?;
            let Ok(item) = serde_json::from_value::<Item>(Value::Object(decoded.fields)) else {
                continue;
            };
            if item.id.is_empty() || item.path.is_empty() || item.at.is_empty() {
                continue;
            }
            self.items.insert(item.id.clone(), item);
        }
        Ok(())
    }
}
