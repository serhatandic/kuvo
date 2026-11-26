//! Backup selectors come from the same on-disk declarations as the other readers.
use std::{
    collections::{BTreeMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use kuvo_store::{
    declarations::{
        Backup, DATA_DECLARATIONS, DataDeclaration, Location, PayloadInventory, Snapshot,
    },
    home::Home,
    layout,
    locks::OwnedLock,
};
use serde_json::{Value, json};

use super::{
    Error, Manifest, Result,
    archive::{Contents, Entry},
    artifacts,
    recovery::Authority,
    sqlite,
};

#[derive(Clone)]
pub(super) struct Roots(BTreeMap<&'static str, PathBuf>);

impl Roots {
    pub fn live(home: &Home, env: &kuvo_os::platform::Environment) -> Self {
        let root = home.root();
        let app = (env
            .get(std::ffi::OsStr::new("KUVO_BOX"))
            .is_some_and(|kind| kind == "mac"))
        .then(|| root.parent())
        .flatten();
        let override_path = |name, path| {
            env.get(std::ffi::OsStr::new(name))
                .filter(|value| !value.is_empty())
                .map_or(path, PathBuf::from)
        };
        let mut roots = BTreeMap::from([
            ("home", root.into()),
            ("bots", override_path("KUVO_BOTS", home.bots())),
            (
                "groups",
                override_path("KUVO_GROUPS", app.unwrap_or(root).join("groups")),
            ),
            ("wakes", override_path("KUVO_WAKES", home.wakes())),
            (
                "engine",
                override_path(
                    "KUVO_ENGINE_DB_FOLDER",
                    override_path("KUVO_ENGINE_DATA", home.engine_data()),
                ),
            ),
            (
                "agents",
                override_path("KUVO_AGENTS_HOME", app.unwrap_or(root).join("agents")),
            ),
            (
                "artifacts",
                override_path("KUVO_ARTIFACTS", root.join("artifacts")),
            ),
            ("attachments", home.attachments()),
        ]);
        if let Some(path) = env
            .get(std::ffi::OsStr::new("KUVO_MAC_ATTACHMENTS"))
            .filter(|value| !value.is_empty())
        {
            roots.insert("macAttachments", PathBuf::from(path));
        }
        Self(roots)
    }

    pub fn locations(&self) -> impl Iterator<Item = &Path> {
        self.0.values().map(PathBuf::as_path)
    }

    pub fn get(&self, root: &str) -> Option<&Path> {
        self.0.get(root).map(PathBuf::as_path)
    }
}

fn selectors() -> impl Iterator<Item = (&'static DataDeclaration, &'static Location)> {
    DATA_DECLARATIONS.iter().flat_map(|data| {
        let locations: &[_] = match data.backup {
            Backup::Storage(locations)
            | Backup::Via {
                storage: locations, ..
            } => locations,
            _ => &[],
        };
        locations.iter().map(move |location| (data, location))
    })
}

fn matches(pattern: &str, name: &str) -> bool {
    let Some((start, end)) = pattern.split_once('*') else {
        return pattern == name;
    };
    name.strip_prefix(start)
        .and_then(|rest| rest.strip_suffix(end))
        .is_some()
}

fn temporary(leaf: &str) -> bool {
    leaf.starts_with('.')
        || [".tmp", ".new", ".lock"]
            .iter()
            .any(|suffix| leaf.ends_with(suffix))
}

pub(super) fn storage(name: &str) -> Option<(&'static DataDeclaration, &'static Location)> {
    let parts: Vec<_> = name.split('/').collect();
    selectors().find(|(_, location)| {
        let pattern = location.pattern.trim_end_matches('/');
        let len = pattern.split('/').count();
        (if location.pattern.ends_with('/') {
            parts.len() >= len
        } else {
            parts.len() == len
        }) && pattern
            .split('/')
            .zip(&parts)
            .all(|(pattern, name)| matches(pattern, name))
            && (location.inventory == Some(PayloadInventory::Session)
                || !parts[1..].iter().any(|part| temporary(part))
                || (location.inventory == Some(PayloadInventory::Queue)
                    && parts.len() == 6
                    && item_id(parts[4])
                    && payload_name(parts[5])))
    })
}

pub(super) fn stat(path: &Path) -> io::Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn children(path: &Path) -> Result<Vec<(String, PathBuf)>> {
    if !stat(path)?.is_some_and(|meta| meta.is_dir()) {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        entries.push((name, entry.path()));
    }
    entries.sort_unstable_by(|a, b| a.0.encode_utf16().cmp(b.0.encode_utf16()));
    Ok(entries)
}

fn walk(
    path: &Path,
    name: &str,
    parts: &[&str],
    tree: bool,
    session_payload: bool,
    found: &mut Vec<(String, PathBuf)>,
) -> Result<()> {
    let Some(meta) = stat(path)? else {
        return Ok(());
    };
    if parts.is_empty() && meta.is_file() {
        found.push((name.into(), path.into()));
        return Ok(());
    }
    if let Some((pattern, rest)) = parts.split_first() {
        if !pattern.contains('*') {
            let child = path.join(pattern);
            if meta.is_dir() {
                walk(
                    &child,
                    &format!("{name}/{pattern}"),
                    rest,
                    tree,
                    session_payload,
                    found,
                )?;
            }
        } else {
            for (leaf, child) in children(path)? {
                if !temporary(&leaf) && matches(pattern, &leaf) {
                    walk(
                        &child,
                        &format!("{name}/{leaf}"),
                        rest,
                        tree,
                        session_payload,
                        found,
                    )?;
                }
            }
        }
    } else if tree {
        for (leaf, child) in children(path)? {
            if session_payload || !temporary(&leaf) {
                walk(
                    &child,
                    &format!("{name}/{leaf}"),
                    &[],
                    true,
                    session_payload,
                    found,
                )?;
            }
        }
    }
    Ok(())
}

pub(super) fn file_entries(
    roots: &Roots,
    want_queue: bool,
) -> Result<Vec<(&'static DataDeclaration, &'static Location, String, PathBuf)>> {
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for (data, location) in selectors() {
        let queue = data.kind.starts_with("queue") && data.kind != "queueNumberAllocation";
        if queue != want_queue {
            continue;
        }
        let mut parts = location.pattern.trim_end_matches('/').split('/');
        let root = parts.next().expect("a declared location has a root");
        let Some(base) = roots.get(root) else {
            continue;
        };
        let pattern: Vec<_> = parts.collect();
        let mut found = Vec::new();
        walk(
            base,
            root,
            &pattern,
            location.pattern.ends_with('/'),
            location.inventory == Some(PayloadInventory::Session),
            &mut found,
        )?;
        entries.extend(found.into_iter().filter_map(|(name, path)| {
            seen.insert(name.clone())
                .then_some((data, location, name, path))
        }));
    }
    Ok(entries)
}

const QUEUE_FILES: &str = "home/run/work-queue/files/";
fn item_id(id: &str) -> bool {
    id.strip_prefix("wqi_")
        .is_some_and(|id| super::url_token(id, 20))
}
fn payload_name(name: &str) -> bool {
    (1..=200).contains(&name.encode_utf16().count())
        && !matches!(name, "." | "..")
        && !name.contains(['/', '\\', '\0'])
}

#[derive(Default)]
pub(super) struct Expected {
    pub size: Option<u64>,
    pub digest: Option<String>,
}

pub(super) type QueueManifests = BTreeMap<String, BTreeMap<String, Expected>>;

pub(super) fn queue_manifests(paths: impl IntoIterator<Item = PathBuf>) -> Result<QueueManifests> {
    let mut manifests = QueueManifests::new();
    for path in paths {
        let value: Value = serde_json::from_slice(&fs::read(&path)?)?;
        let items = value
            .get("items")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                Error::Detail(format!(
                    "Unreadable Work Queue inventory: {}.",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ))
            })?;
        for item in items {
            let Some(id) = item
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| item_id(id))
            else {
                continue;
            };
            let Some(files) = item.get("files").and_then(Value::as_array) else {
                continue;
            };
            let entry = manifests.entry(id.into()).or_default();
            for file in files {
                let Some(name) = file
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|name| payload_name(name))
                else {
                    continue;
                };
                let size = file
                    .get("size")
                    .and_then(Value::as_u64)
                    .filter(|size| *size <= 9_007_199_254_740_991);
                let digest = file
                    .get("sha256")
                    .map(|digest| {
                        digest
                            .as_str()
                            .filter(|digest| super::url_token(digest, 43))
                            .ok_or_else(|| {
                                Error::Detail(format!(
                                    "Invalid Work Queue attachment digest: {id}/{name}."
                                ))
                            })
                    })
                    .transpose()?;
                let prior = entry.entry(name.into()).or_default();
                if prior.size.zip(size).is_some_and(|(a, b)| a != b)
                    || prior
                        .digest
                        .as_deref()
                        .zip(digest)
                        .is_some_and(|(a, b)| a != b)
                {
                    return Err(Error::Detail(format!(
                        "Conflicting Work Queue attachment inventory: {id}/{name}."
                    )));
                }
                prior.size = size.or(prior.size);
                if let Some(digest) = digest {
                    prior.digest = Some(digest.into());
                }
            }
        }
    }
    Ok(manifests)
}

fn queue_snapshot(home: &Home, roots: &Roots, scratch: &Path) -> Result<Vec<Entry>> {
    let _lock = OwnedLock::acquire(&home.run().join("work-queue/.lock"), Duration::from_secs(3))?;
    let files = file_entries(roots, true)?;
    let manifests = queue_manifests(
        files
            .iter()
            .filter(|(data, ..)| data.kind == "queueItems")
            .map(|(_, _, _, path)| path.clone()),
    )?;
    let base = home.run().join("work-queue/files");
    let mut entries = Vec::new();
    let mut stage = |name: String, path: &Path, expected: Expected| -> Result<()> {
        let file = scratch.join(format!("queue-{}", entries.len()));
        let mut input = super::archive::regular(path).map_err(|_| {
            Error::Detail(format!(
                "Work Queue attachment missing or unreadable: {name}."
            ))
        })?;
        let mut copy = fs::File::create(&file)?;
        let size = io::copy(&mut input, &mut copy)?;
        if expected.size.is_some_and(|expected| expected != size) {
            return Err(Error::Detail(format!(
                "Work Queue attachment size differs from its record: {name}."
            )));
        }
        entries.push(Entry {
            name,
            contents: Contents::File {
                path: file,
                size: expected.size,
                digest: expected.digest,
            },
        });
        Ok(())
    };
    for (data, _, name, path) in files {
        // Queue files listed in a manifest are staged with their expected hashes below.
        if data.kind != "queueFiles"
            || name
                .strip_prefix(QUEUE_FILES)
                .and_then(|suffix| suffix.split('/').next())
                .is_some_and(|id| !manifests.contains_key(id))
        {
            stage(name, &path, Expected::default())?;
        }
    }
    for (id, attachments) in manifests {
        for (leaf, expected) in attachments {
            let path =
                super::recovery::output(home.root(), &format!("run/work-queue/files/{id}/{leaf}"))?;
            debug_assert!(path.starts_with(&base));
            stage(format!("{QUEUE_FILES}{id}/{leaf}"), &path, expected)?;
        }
    }
    Ok(entries)
}

pub(super) fn portable_approval(value: Value) -> Value {
    let mut approved = serde_json::Map::new();
    if let Some(last) = value
        .get("last")
        .and_then(Value::as_str)
        .filter(|last| matches!(*last, "askFirst" | "acceptEdits" | "auto" | "fullAccess"))
    {
        approved.insert("last".into(), json!(last));
    }
    if let Some(since) = value.get("since").filter(|since| since.is_number()) {
        approved.insert("since".into(), since.clone());
    }
    Value::Object(approved)
}

pub(super) fn capture(
    home: &Home,
    roots: &Roots,
    manifest: &Manifest,
    authority: &impl Authority,
    scratch: &Path,
) -> Result<Vec<Entry>> {
    let mut entries = vec![
        Entry::json("meta/manifest.json", manifest)?,
        Entry::json(
            "meta/attachment-roots.json",
            &json!({
                "attachments": roots.get("attachments"),
                "macAttachments": roots.get("macAttachments"),
            }),
        )?,
    ];
    if let Some(accepted) = authority.accepted_snapshot()? {
        entries.push(Entry::json("meta/accepted.json", &accepted)?);
    }
    entries.extend(queue_snapshot(home, roots, scratch)?);
    let files = file_entries(roots, false)?;
    let artifacts = artifacts::Inventory::from_logs(
        files
            .iter()
            .filter(|(_, _, name, _)| name.starts_with("artifacts/log/"))
            .map(|(_, _, _, file)| file.clone()),
    )?;
    let mut required = BTreeMap::new();
    if let Some(root) = roots.get("artifacts") {
        for (name, expected) in artifacts.copies() {
            let file = super::recovery::output(
                root,
                name.strip_prefix("artifacts/").expect("artifact root"),
            )?;
            if !stat(&file)?.is_some_and(|file| file.is_file()) {
                return Err(Error::Detail(format!(
                    "Backup payload missing or unreadable: {name}."
                )));
            }
            required.insert(name, ());
            entries.push(Entry {
                name: name.into(),
                contents: Contents::File {
                    path: file,
                    size: expected.size,
                    digest: expected.digest.clone(),
                },
            });
        }
    }
    let mut transcripts = HashSet::new();
    for (_, location, mut name, mut path) in files {
        if required.contains_key(name.as_str()) {
            continue;
        }
        if !authority.capture_entry(&name, &path)? {
            continue;
        }
        use super::receipts::{Journal, ReceiptOwner};
        match Journal::capture(&name, &path)? {
            Journal::Origin => continue,
            Journal::Addressed {
                setup,
                owner: Some(owner @ (ReceiptOwner::Issuer(_) | ReceiptOwner::ReplayFence(_))),
            } if !authority.receipt_here(&setup, &owner, None)? => continue,
            _ => (),
        }
        let temp = scratch.join(format!("snapshot-{}", entries.len()));
        let contents = if name == "home/box/approval.json" {
            Contents::Bytes(serde_json::to_vec(&portable_approval(
                serde_json::from_slice(&fs::read(&path)?)?,
            ))?)
        } else {
            match location.snapshot {
                Snapshot::Sqlite | Snapshot::EngineDatabase => {
                    sqlite::snapshot(&path, &temp)?;
                    if location.snapshot == Snapshot::EngineDatabase {
                        sqlite::scrub_engine(&temp, false, &[])?;
                    }
                    path = temp;
                }
                Snapshot::AgentTranscript | Snapshot::Bytes
                    if name.starts_with("agents/sessions/")
                        && (name.ends_with("/messages.json")
                            || location.snapshot == Snapshot::AgentTranscript) =>
                {
                    let folder = path.parent().expect("transcript is in its session folder");
                    let database = folder.join("messages-v1.sqlite");
                    name = format!(
                        "{}/messages.json",
                        name.rsplit_once('/').expect("archive path").0
                    );
                    if !transcripts.insert(name.clone()) {
                        continue;
                    }
                    if stat(&database)?.is_some_and(|meta| meta.is_file()) {
                        sqlite::transcript(&database, &temp)?;
                        path = temp;
                    }
                }
                _ => (),
            }
            Contents::File {
                path,
                size: None,
                digest: None,
            }
        };
        entries.push(Entry { name, contents });
    }
    Ok(entries)
}

pub(super) fn bind_attachments(
    roots: &Roots,
    authority: &impl Authority,
    selected: &BTreeMap<String, String>,
    entries: &mut [Entry],
) -> Result<()> {
    let mut unchecked = BTreeMap::new();
    for (name, digest) in selected {
        let Some((root, relative)) = name.split_once('/') else {
            return Err(Error::Message("Invalid backup entry name."));
        };
        let file = roots
            .get(root)
            .filter(|_| super::archive::valid_name(name))
            .ok_or(Error::Message("Invalid backup entry name."))?
            .join(relative);
        if authority.capture_entry(name, &file)? {
            unchecked.insert(name.as_str(), digest);
        }
    }
    for entry in entries {
        let Contents::File { digest, .. } = &mut entry.contents else {
            continue;
        };
        let Some(expected) = unchecked.remove(entry.name.as_str()) else {
            if storage(&entry.name)
                .is_some_and(|(data, _)| matches!(data.kind, "sessionFiles" | "queueFiles"))
            {
                return Err(Error::Detail(format!(
                    "Attachment added during export: {}.",
                    entry.name
                )));
            }
            continue;
        };
        if digest.as_ref().is_some_and(|kept| kept != expected) {
            return Err(Error::Detail(format!(
                "Work Queue attachment digest differs from its record: {}.",
                entry.name
            )));
        }
        *digest = Some(expected.clone());
    }
    if let Some((name, _)) = unchecked.first_key_value() {
        return Err(Error::Detail(format!(
            "Attachment omitted during export: {name}."
        )));
    }
    Ok(())
}

pub(super) fn layout(home: &Home) -> Result<u32> {
    let run = home.run();
    if [
        "update-trial",
        "update-restore",
        ".updating",
        "engine-swap.json",
        "update-engine-hold.json",
    ]
    .iter()
    .any(|marker| run.join(marker).exists())
    {
        return Err(Error::Message(
            "Wait for the update to finish before backing up.",
        ));
    }
    Ok(layout::backup_version(home)?)
}

pub(super) fn order_recovery(names: &mut [String]) {
    let order: Vec<_> = selectors().map(|(_, location)| location).collect();
    names.sort_by(|a, b| {
        let rank = |name: &str| {
            storage(name)
                .and_then(|(_, place)| order.iter().position(|place2| *place2 == place))
                .unwrap_or(if name.starts_with("meta/") {
                    usize::MAX - 1
                } else {
                    usize::MAX
                })
        };
        rank(a)
            .cmp(&rank(b))
            .then_with(|| a.encode_utf16().cmp(b.encode_utf16()))
    });
}

pub(super) fn references(
    name: &str,
    roots: &Roots,
) -> Vec<(kuvo_peers::directory::RecordScope, String)> {
    use kuvo_peers::directory::RecordScope::{Bot, Group, Session};
    let parts: Vec<_> = name.split('/').collect();
    let mapped = |root, id: &str| -> String {
        roots
            .get(root)
            .and_then(|base| {
                let path = base.join(id).join("session");
                super::archive::regular(&path).ok()?;
                std::fs::read_to_string(path)
                    .ok()
                    .map(|text| text.trim().to_owned())
            })
            .unwrap_or_else(|| id.to_owned())
    };
    match parts.as_slice() {
        ["home", "run", "work", "notes", name] if name.starts_with("ses_") => name
            .strip_suffix(".ordinary.json")
            .map(|id| (Session, id.into()))
            .into_iter()
            .collect(),
        ["bots", id, _, ..] => vec![(Bot, (*id).into()), (Session, mapped("bots", id))],
        ["groups", id, _, ..] => vec![(Group, (*id).into())],
        ["agents", "sessions", id, _, ..] => vec![(Session, (*id).into())],
        ["wakes", id, ..] => vec![(Session, mapped("wakes", id))],
        ["attachments" | "macAttachments", id, ..] if id.starts_with("ses_") => {
            vec![(Session, (*id).into())]
        }
        _ => Vec::new(),
    }
}
