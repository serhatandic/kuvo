//! Restore plans are checked before the first live file is placed.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Component, Path, PathBuf},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use kuvo_store::{
    declarations::{Restore, Snapshot},
    files,
    home::Home,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    Error, Manifest, PointId, RestorePoint, Result, Source, archive, artifacts,
    attachments::{Addresses, Format},
    inventory::{self, Roots, stat, storage},
    projects, sqlite,
};

/// Setup supplies identity plus accepted ownership. Capture and restore ask the
/// same authority before publishing, so the disk worker never interprets keys.
pub trait Authority {
    fn capture(&self, source: Source, point: RestorePoint) -> Result<Manifest>;
    fn authorize_restore(&self, manifest: &Manifest) -> Result<()>;

    fn confirm_capture(&self) -> Result<()> {
        Ok(())
    }

    fn accepted_snapshot(&self) -> Result<Option<Value>> {
        Ok(None)
    }
    fn capture_entry(&self, _name: &str, _file: &Path) -> Result<bool> {
        Ok(true)
    }
    fn restore_entry(&self, _manifest: &Manifest, _name: &str, _file: &Path) -> Result<bool> {
        Ok(true)
    }
    fn insight_origins(&self, _manifest: &Manifest) -> Result<Option<InsightOrigins>> {
        Ok(None)
    }

    fn recover_queue(&self, _manifest: &Manifest, _queue: &str, _id: &str) -> Result<bool> {
        Ok(true)
    }
    fn fenced_sessions(&self, _manifest: &Manifest) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
}

impl<A: Authority + ?Sized> Authority for &A {
    fn capture(&self, source: Source, point: RestorePoint) -> Result<Manifest> {
        (**self).capture(source, point)
    }
    fn confirm_capture(&self) -> Result<()> {
        (**self).confirm_capture()
    }
    fn accepted_snapshot(&self) -> Result<Option<Value>> {
        (**self).accepted_snapshot()
    }
    fn capture_entry(&self, name: &str, file: &Path) -> Result<bool> {
        (**self).capture_entry(name, file)
    }
    fn authorize_restore(&self, manifest: &Manifest) -> Result<()> {
        (**self).authorize_restore(manifest)
    }
    fn restore_entry(&self, manifest: &Manifest, name: &str, file: &Path) -> Result<bool> {
        (**self).restore_entry(manifest, name, file)
    }
    fn recover_queue(&self, manifest: &Manifest, queue: &str, id: &str) -> Result<bool> {
        (**self).recover_queue(manifest, queue, id)
    }
    fn fenced_sessions(&self, manifest: &Manifest) -> Result<Vec<String>> {
        (**self).fenced_sessions(manifest)
    }
    fn insight_origins(&self, manifest: &Manifest) -> Result<Option<InsightOrigins>> {
        (**self).insight_origins(manifest)
    }
}

pub struct InsightOrigins {
    pub own: String,
    pub accepted: Vec<String>,
}

/// A scratch installation or standalone computer with no accepted remote owners.
pub struct Standalone {
    pub origin: Option<String>,
    pub signer: Option<String>,
    pub setup: Option<String>,
}

impl Authority for Standalone {
    fn capture(&self, source: Source, point: RestorePoint) -> Result<Manifest> {
        Ok(Manifest {
            format: 1,
            source,
            point,
            origin: self.origin.clone(),
            signer: self.signer.clone(),
            setup: self.setup.clone(),
            paired: false,
            projects: Vec::new(),
        })
    }

    fn authorize_restore(&self, manifest: &Manifest) -> Result<()> {
        if self
            .origin
            .as_ref()
            .is_some_and(|key| manifest.origin.as_ref() == Some(key))
            || self
                .signer
                .as_ref()
                .is_some_and(|key| manifest.signer.as_ref() == Some(key))
        {
            return Err(Error::Message(
                "Restore onto a replacement with fresh membership keys.",
            ));
        }
        if manifest.paired {
            return Err(Error::Message(
                "Enroll the replacement in the current setup and wait for its accepted membership.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryResult {
    pub id: PointId,
    pub at: i64,
    pub installed: usize,
    pub held: Vec<String>,
    pub unavailable: Vec<Unavailable>,
    pub saved_after_point: String,
    pub credentials: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Unavailable {
    pub entry: String,
    pub reason: String,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
enum State {
    Restoring,
    Restored { result: RecoveryResult },
}

#[derive(Serialize, Deserialize)]
struct Journal {
    id: PointId,
    source: Source,
    #[serde(flatten)]
    state: State,
    #[serde(
        default,
        rename = "insightsAlias",
        skip_serializing_if = "Option::is_none"
    )]
    insights_alias: Option<String>,
    #[serde(default, rename = "formatVersion")]
    version: u32,
}

fn json_at(path: &Path) -> Result<Option<Value>> {
    files::read(path)?
        .map(|bytes| Ok(serde_json::from_slice(&bytes)?))
        .transpose()
}

pub(super) fn manifest(
    entries: &[archive::Opened],
    source: &Source,
    id: &PointId,
) -> Result<Manifest> {
    let invalid = || Error::Message("The archive doesn’t match that computer or restore point.");
    let entry = entries
        .iter()
        .find(|entry| entry.name == "meta/manifest.json")
        .ok_or_else(invalid)?;
    let found: Manifest =
        serde_json::from_reader(archive::regular(&entry.file)?).map_err(|_| invalid())?;
    if !found.valid(source, id) {
        return Err(invalid());
    }
    Ok(found)
}

pub(super) fn record_key(parts: &[&str]) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(
        serde_json::to_vec(parts).expect("strings serialize"),
    ))
}

pub(super) fn output(base: &Path, name: &str) -> Result<PathBuf> {
    let mut path = base.to_owned();
    let mut parts = name.split('/').peekable();
    check_directory(&path)?;
    while let Some(part) = parts.next() {
        let mut components = Path::new(part).components();
        if components.next() != Some(Component::Normal(part.as_ref()))
            || components.next().is_some()
            || part.contains(['\\', '\0'])
            || (cfg!(windows) && (part.contains(':') || part.ends_with(['.', ' '])))
        {
            return Err(Error::Message("Invalid recovery path."));
        }
        path.push(part);
        if parts.peek().is_some() {
            check_directory(&path)?;
        }
    }
    Ok(path)
}

fn check_directory(path: &Path) -> Result<()> {
    if stat(path)?.is_some_and(|meta| !meta.is_dir()) {
        return Err(Error::Message(
            "The recovery path contains a link or non-directory.",
        ));
    }
    Ok(())
}

pub(super) fn no_workers(home: &Home) -> Result<()> {
    let run = home.run();
    if [
        "update-trial",
        ".updating",
        "engine-swap.json",
        "update-engine-hold.json",
    ]
    .iter()
    .any(|marker| run.join(marker).exists())
    {
        return Err(Error::Message("Finish the update before restoring."));
    }
    let runs = |pid: u64, name| {
        u32::try_from(pid)
            .ok()
            .is_some_and(|pid| kuvo_os::process::legacy_process_runs(pid, name))
    };
    for (file, name) in [
        ("engine.json", "opencode"),
        ("bridge.json", "kuvo-agents"),
        ("keeper.json", "kuvo-start"),
    ] {
        if let Some(record) = json_at(&run.join(file))?
            && let Some(pid) = record.get("pid").and_then(Value::as_u64)
            && (record
                .get("startIdentity")
                .and_then(Value::as_str)
                .is_some_and(|start| {
                    u32::try_from(pid).ok().is_some_and(|pid| {
                        kuvo_os::process::ProcessIdentity {
                            pid,
                            start_identity: start.into(),
                        }
                        .state()
                            != kuvo_os::process::ProcessState::Stale
                    })
                })
                || runs(pid, name))
        {
            return Err(Error::Message(
                "Stop the engine, agents, and keeper before restoring.",
            ));
        }
    }
    if files::text_of(&run.join("box.pid"))
        .and_then(|pid| pid.trim().parse().ok())
        .is_some_and(|pid| runs(pid, "kuvo-start"))
    {
        return Err(Error::Message("Stop the keeper before restoring."));
    }
    Ok(())
}

struct Placement {
    entry: String,
    from: PathBuf,
    to: PathBuf,
    engine: bool,
}

fn same(a: &Path, b: &Path) -> Result<bool> {
    if !stat(a)?.is_some_and(|info| info.is_file()) {
        return Ok(false);
    }
    let (mut a, mut b) = (archive::regular(a)?, archive::regular(b)?);
    if a.metadata()?.len() != b.metadata()?.len() {
        return Ok(false);
    }
    let (mut ah, mut bh) = (Sha256::new(), Sha256::new());
    io::copy(&mut a, &mut ah)?;
    io::copy(&mut b, &mut bh)?;
    Ok(ah.finalize() == bh.finalize())
}

fn place(from: &Path, to: &Path, replace: bool) -> Result<()> {
    let parent = to
        .parent()
        .ok_or(Error::Message("Invalid recovery path."))?;
    files::create_private_dirs(parent)?;
    let mut partial = tempfile::NamedTempFile::new_in(parent)?;
    io::copy(&mut archive::regular(from)?, partial.as_file_mut())?;
    partial.as_file().sync_all()?;
    if replace {
        files::replace_synced(partial.path(), to)?;
    } else {
        match fs::hard_link(partial.path(), to) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && same(to, from)? => (),
            Err(error) => return Err(error.into()),
        }
        files::sync_parent(to)?;
    }
    Ok(())
}

pub(super) fn restore(
    home: &Home,
    roots: &Roots,
    archived: Vec<archive::Opened>,
    manifest: &Manifest,
    scratch: &Path,
    authority: &impl Authority,
) -> Result<RecoveryResult> {
    if manifest.point.version > kuvo_store::layout::VERSION {
        return Err(Error::Message(
            "The archive uses an unsupported folder layout.",
        ));
    }
    let mut entries = BTreeMap::new();
    let mut ordered = Vec::new();
    for archive::Opened { name, file } in archived {
        let (root, _) = name
            .split_once('/')
            .ok_or(Error::Message("The archive contains an undeclared record."))?;
        if !matches!(root, "meta" | "projects") && storage(&name).is_none() {
            return Err(Error::Message("The archive contains an undeclared record."));
        }
        ordered.push(name.clone());
        entries.insert(name, file);
    }
    // TypeScript inventories the staged layout in declaration order, with queue
    // metadata and attachment files selected by the saved item lists.
    inventory::order_recovery(&mut ordered);
    let project_plan = projects::RestorePlan::read(&entries)?;
    authority.authorize_restore(manifest)?;
    no_workers(home)?;
    let backup = home.root().join("backup");
    let journal_file = backup.join("restore.json");
    if let Some(journal) = json_at(&journal_file)? {
        let journal: Journal = serde_json::from_value(journal)?;
        if journal.version > 1 {
            return Err(Error::Message("Unsupported restore journal."));
        }
        if journal.id != manifest.point.id || journal.source != manifest.source {
            return Err(Error::Message(
                "This replacement already has a restore point.",
            ));
        }
        if let State::Restored { result } = journal.state {
            files::remove_file_synced(&backup.join("restoring"))?;
            return Ok(result);
        }
    }
    let recovery = backup.join("recovered").join(manifest.point.id.to_string());
    let mut result = RecoveryResult {
        id: manifest.point.id.clone(),
        at: manifest.point.at,
        installed: 0,
        held: Vec::new(),
        unavailable: Vec::new(),
        saved_after_point: "unavailable unless verified elsewhere".into(),
        credentials: "sign-in required".into(),
    };
    let artifacts = artifacts::Inventory::from_logs(
        ordered
            .iter()
            .filter(|name| name.starts_with("artifacts/log/"))
            .map(|name| entries[name].clone()),
    )?;
    for (name, expected) in artifacts.copies() {
        if !entries
            .get(name)
            .map(|file| artifacts::matches(file, expected))
            .transpose()?
            .unwrap_or(false)
        {
            entries.remove(name);
            result.unavailable.push(Unavailable {
                entry: name.into(),
                reason: "retained artifact copy missing or differs from its record".into(),
            });
        }
    }
    result.unavailable.extend(artifacts.prune_report());
    ordered.retain(|name| entries.contains_key(name));
    let manifests = inventory::queue_manifests(
        ordered
            .iter()
            .filter(|name| storage(name).is_some_and(|(data, _)| data.kind == "queueItems"))
            .map(|name| entries[name].clone()),
    )?;
    let mut unavailable_files = BTreeMap::new();
    for (id, names) in &manifests {
        for (leaf, expected) in names {
            let entry = format!("home/run/work-queue/files/{id}/{leaf}");
            let reason = match entries.get(&entry) {
                None => Some("attachment missing from restore point"),
                Some(file)
                    if expected.size.is_some_and(|size| {
                        stat(file)
                            .ok()
                            .flatten()
                            .is_some_and(|meta| meta.len() != size)
                    }) =>
                {
                    Some("attachment size differs from its record")
                }
                Some(file) if !artifacts::matches(file, expected)? => {
                    Some("attachment digest differs from its record")
                }
                _ => None,
            };
            if let Some(reason) = reason {
                unavailable_files
                    .entry(id.clone())
                    .or_insert_with(Vec::new)
                    .push(Unavailable {
                        entry,
                        reason: reason.into(),
                    });
            }
        }
    }
    let mut placements = Vec::new();
    let mut reviewed_payloads = BTreeMap::new();
    let mut queue_checks = Vec::new();
    let mut pauses = BTreeSet::new();
    let mut attachments = BTreeMap::new();
    let mut ownership = Vec::new();
    let engine_fences = authority.fenced_sessions(manifest)?;
    let attachment_roots = entries
        .get("meta/attachment-roots.json")
        .map(|file| json_at(file))
        .transpose()?
        .flatten();
    let mut addresses = Addresses::new(attachment_roots.as_ref(), &recovery);
    if attachment_roots.is_some() {
        for name in &ordered {
            if !storage(name).is_some_and(|(data, _)| data.kind == "sessionFiles") {
                continue;
            }
            let file = &entries[name];
            if !authority.restore_entry(manifest, name, file)? {
                continue;
            }
            let (root, rest) = name.split_once('/').expect("validated entry");
            let mut target = roots.get(root).map(|base| output(base, rest)).transpose()?;
            if target
                .as_ref()
                .map(|target| -> Result<_> { Ok(stat(target)?.is_some() && !same(target, file)?) })
                .transpose()?
                .unwrap_or(true)
            {
                let slot = format!(
                    "restore-{}",
                    record_key(&[&manifest.point.id.to_string(), name])
                );
                let parts: Vec<_> = rest.split('/').collect();
                let container = if parts.len() > 1 { parts[0] } else { &slot };
                let fallback = output(
                    roots.get("attachments").expect("attachments root"),
                    &format!(
                        "{container}/{slot}/{}",
                        parts.last().expect("entry has leaf")
                    ),
                )?;
                if stat(&fallback)?.is_some() && !same(&fallback, file)? {
                    return Err(Error::Message(
                        "A recovered attachment location is occupied.",
                    ));
                }
                if target.is_some() {
                    result.unavailable.push(Unavailable {
                        entry: name.clone(),
                        reason:
                            "existing local attachment; recovered copy stored at a separate address"
                                .into(),
                    });
                }
                target = Some(fallback);
            }
            let target = target.expect("attachment has a target or fallback");
            addresses.add(root, rest, &target);
            attachments.insert(
                name.clone(),
                Placement {
                    entry: name.clone(),
                    from: file.clone(),
                    to: target,
                    engine: false,
                },
            );
        }
        rewrite_legacy_parts(&entries, &mut addresses, &mut result.unavailable)?;
    }
    for name in ordered {
        if matches!(
            name.as_str(),
            "meta/manifest.json" | "meta/projects.json" | "meta/attachment-roots.json"
        ) {
            continue;
        }
        let file = entries[&name].clone();
        let (root, rest) = name.split_once('/').expect("validated entry");
        let (kind, policy, snapshot) = storage(&name).map_or(
            ("", Restore::NeedsFreshApproval, Snapshot::Bytes),
            |(data, location)| {
                (
                    data.kind,
                    location.restore.unwrap_or(data.restore),
                    location.snapshot,
                )
            },
        );
        let legacy = [
            "engine/storage/session/",
            "engine/storage/message/",
            "engine/storage/part/",
        ]
        .iter()
        .any(|prefix| name.starts_with(prefix));
        let legacy_order = if legacy {
            let value = json_at(&file).ok().flatten().unwrap_or(Value::Null);
            super::attachments::legacy_session(&name, &value)
                .is_some_and(|id| !engine_fences.contains(&id))
        } else {
            true
        };
        let allowed = legacy_order && authority.restore_entry(manifest, &name, &file)?;
        if legacy_order {
            ownership.push((name.clone(), file.clone(), allowed));
        }
        if !allowed {
            result.held.push(name.clone());
            result.unavailable.push(Unavailable {
                entry: name.clone(),
                reason: "moved".into(),
            });
            placements.push(Placement {
                to: output(&recovery, &name)?,
                entry: name,
                from: file,
                engine: false,
            });
            continue;
        }
        let base = roots.get(root);
        let parts: Vec<_> = rest.split('/').collect();
        if kind == "queueFiles" && parts.get(3).is_some_and(|id| manifests.contains_key(*id)) {
            if parts.len() != 5 || !manifests[parts[3]].contains_key(parts[4]) {
                continue;
            }
        }
        if kind == "queueItems" {
            let mut queue = json_at(&file)?.ok_or(Error::Message(
                "The backup contains an unreadable Work Queue.",
            ))?;
            let owner = parts
                .last()
                .and_then(|leaf| leaf.strip_suffix(".json"))
                .expect("declared queue filename");
            let items =
                queue
                    .get_mut("items")
                    .and_then(Value::as_array_mut)
                    .ok_or(Error::Message(
                        "The backup contains an unreadable Work Queue.",
                    ))?;
            let mut rewrite = false;
            let mut kept = Vec::new();
            for mut item in std::mem::take(items) {
                let id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or(Error::Message(
                        "The backup contains an unreadable Work Queue item.",
                    ))?
                    .to_owned();
                let fence = |family| -> Result<bool> {
                    let address =
                        format!("run/owned-data/edit-{family}/{}.json", record_key(&[&id]));
                    let local = output(home.root(), &address)?;
                    Ok(stat(&local)?.is_some() || entries.contains_key(&format!("home/{address}")))
                };
                let deleted = fence("deletions")?;
                let allowed = authority.recover_queue(manifest, owner, &id)?;
                queue_checks.push((owner.to_owned(), id.clone(), allowed));
                let needs_review = item.get("handing").is_some_and(Value::is_object)
                    || fence("departures")?
                    || !allowed;
                let removed = truthy(item.get("removed"));
                if deleted || needs_review {
                    let held = !deleted && !removed;
                    reviewed_payloads.insert(id.clone(), held);
                    result.unavailable.push(Unavailable {
                        entry: id.clone(),
                        reason: if deleted {
                            "deleted"
                        } else {
                            "moved or transfer needs review"
                        }
                        .into(),
                    });
                    if held {
                        let entry = format!("work-queue-items/{}.json", record_key(&[&id]));
                        let copy = scratch.join(format!("queue-review-{}", placements.len()));
                        item["owner"] = json!(owner);
                        files::write_new(&copy, &serde_json::to_vec(&item)?)?;
                        placements.push(Placement {
                            to: output(&recovery, &entry)?,
                            entry: entry.clone(),
                            from: copy,
                            engine: false,
                        });
                        result.held.push(entry);
                        result
                            .unavailable
                            .extend(unavailable_files.remove(&id).unwrap_or_default());
                    }
                    if removed {
                        if item
                            .get("files")
                            .and_then(Value::as_array)
                            .is_some_and(|files| !files.is_empty())
                        {
                            item["files"] = json!([]);
                            rewrite = true;
                        }
                        kept.push(item);
                    } else {
                        rewrite = true;
                    }
                } else {
                    result
                        .unavailable
                        .extend(unavailable_files.remove(&id).unwrap_or_default());
                    kept.push(item);
                }
            }
            if rewrite {
                *items = kept;
                files::write_atomic(&file, &serde_json::to_vec(&queue)?)?;
            }
        }
        if root == "agents"
            && parts.len() == 3
            && parts[0] == "sessions"
            && parts[2] == "agent.json"
        {
            let mut agent = json_at(&file)?
                .and_then(|value| value.as_object().cloned())
                .ok_or(Error::Message(
                    "The backup contains an unreadable agent record.",
                ))?;
            if let Some(inbox) = agent
                .shift_remove("inbox")
                .filter(|inbox| inbox.as_array().is_some_and(|rows| !rows.is_empty()))
            {
                let held = format!("agents/sessions/{}/inbox.json", parts[1]);
                let copy = scratch.join(format!("inbox-{}", placements.len()));
                files::write_new(&copy, &serde_json::to_vec(&inbox)?)?;
                placements.push(Placement {
                    to: output(&recovery, &held)?,
                    entry: held.clone(),
                    from: copy,
                    engine: false,
                });
                result.held.push(held);
            }
            agent.shift_remove("lateShells");
            agent.shift_remove("revertNow");
            files::write_atomic(&file, &serde_json::to_vec(&agent)?)?;
        }
        if name == "home/box/approval.json" {
            let approved = inventory::portable_approval(json_at(&file)?.unwrap_or(Value::Null));
            files::write_atomic(&file, &serde_json::to_vec(&approved)?)?;
        }
        if kind == "groupSettings" && parts.len() == 2 && parts[1] == "group.json" {
            let mut group = json_at(&file)?.unwrap_or(Value::Null);
            let tasks = entries
                .get(&format!("groups/{}/messages.json", parts[0]))
                .map(|file| json_at(file))
                .transpose()?
                .flatten();
            if (["talking", "paused", "turning"]
                .iter()
                .any(|key| truthy(group.get(key)))
                || truthy(group.get("group").and_then(|g| g.get("meeting")))
                || tasks
                    .and_then(|tasks| tasks.as_array().cloned())
                    .into_iter()
                    .flatten()
                    .filter_map(|msg| msg.get("tasks").and_then(Value::as_array).cloned())
                    .flatten()
                    .any(|task| {
                        matches!(
                            task.get("state").and_then(Value::as_str),
                            Some("waiting" | "working")
                        )
                    }))
                && let Some(group) = group.as_object_mut()
            {
                group.shift_remove("reviewReceipt");
                group.insert("status".into(), json!("held"));
                group.insert("restoreReview".into(), json!(true));
                files::write_atomic(&file, &serde_json::to_vec(group)?)?;
                result.held.push(name.clone());
            }
        }
        if name == "groups/peer-memory.json" {
            let memory = json_at(&file)?
                .and_then(|value| value.as_object().cloned())
                .ok_or(Error::Message(
                    "The backup contains unreadable group memory.",
                ))?;
            let bots = roots.get("bots").ok_or(Error::Message(
                "The backup contains unreadable group memory.",
            ))?;
            let mut rewritten = serde_json::Map::new();
            for (bot, recaps) in memory {
                if !bot.strip_prefix("bot_").is_some_and(|id| {
                    !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
                }) {
                    continue;
                }
                let Value::Object(recaps) = recaps else {
                    continue;
                };
                let mut kept = serde_json::Map::new();
                for (address, mut recap) in recaps {
                    if !recap.is_object() {
                        continue;
                    }
                    if recap.get("forgotten") == Some(&Value::Bool(true)) {
                        kept.insert(address, recap);
                    } else if let Some(leaf) = recap
                        .get("file")
                        .and_then(Value::as_str)
                        .and_then(|file| Path::new(file).file_name())
                        .and_then(|leaf| leaf.to_str())
                    {
                        let relative = format!("{bot}/groups/{leaf}");
                        let entry = format!("bots/{relative}");
                        if let Some(saved) = entries.get(&entry)
                            && authority.restore_entry(manifest, &entry, saved)?
                        {
                            recap["file"] = json!(output(bots, &relative)?);
                            kept.insert(address, recap);
                        }
                    }
                }
                rewritten.insert(bot, Value::Object(kept));
            }
            files::write_atomic(&file, &serde_json::to_vec(&rewritten)?)?;
        }
        if snapshot == Snapshot::EngineDatabase {
            sqlite::scrub_engine(&file, true, &engine_fences)?;
            if attachment_roots.is_some() {
                sqlite::rewrite_attachments(&file, &mut addresses, &mut result.unavailable)?;
            }
        } else if attachment_roots.is_some()
            && name.ends_with(".json")
            && (kind == "groupTranscript"
                || (root == "agents" && parts.get(2) == Some(&"messages.json"))
                || (root == "engine" && parts.get(1) == Some(&"message"))
                || name.starts_with("home/run/waiting/"))
            && let Ok(Some(mut value)) = json_at(&file)
        {
            let format = if root == "home" {
                Format::Prompt
            } else {
                Format::Transcript
            };
            if addresses.rewrite(&mut value, format, &mut result.unavailable)? {
                files::write_atomic(&file, &serde_json::to_vec(&value)?)?;
            }
        }
        if matches!(kind, "botSchedules" | "sessionSchedules") {
            if parts.len() == 2 && parts[1] == "paused" {
                continue;
            }
            if parts.len() > 2
                && let Some(base) = base
            {
                pauses.insert(output(base, &format!("{}/paused", parts[0]))?);
            }
        }
        if attachments.contains_key(&name) {
            continue;
        }
        let target = match base.filter(|_| policy == Restore::Portable) {
            Some(base) => output(base, rest)?,
            None => {
                result.held.push(name.clone());
                output(&recovery, &name)?
            }
        };
        let placement = Placement {
            entry: name,
            from: file,
            to: target,
            engine: snapshot == Snapshot::EngineDatabase,
        };
        placements.push(placement);
    }
    let insights = authority.insight_origins(manifest)?;
    let insights_alias = insights.as_ref().map(|scope| {
        hex::encode(Sha256::digest(format!(
            "restored Insights computer\0{}",
            scope.own
        )))[..32]
            .to_owned()
    });
    let mut journal = Journal {
        version: 1,
        id: manifest.point.id.clone(),
        source: manifest.source.clone(),
        state: State::Restoring,
        insights_alias,
    };
    files::write_atomic(&journal_file, &serde_json::to_vec(&journal)?)?;
    files::write_atomic(
        &backup.join("restoring"),
        manifest.point.id.to_string().as_bytes(),
    )?;
    no_workers(home)?;
    authority.authorize_restore(manifest)?;
    for (name, file, allowed) in ownership {
        if authority.restore_entry(manifest, &name, &file)? != allowed {
            return Err(Error::Message(
                "Ownership changed during restore. Verify the replacement again.",
            ));
        }
    }
    for (queue, id, allowed) in queue_checks {
        if authority.recover_queue(manifest, &queue, &id)? != allowed {
            return Err(Error::Message(
                "Ownership changed during restore. Verify the replacement again.",
            ));
        }
    }
    if authority.fenced_sessions(manifest)? != engine_fences {
        return Err(Error::Message(
            "Ownership changed during restore. Verify the replacement again.",
        ));
    }
    let paused = scratch.join("paused");
    files::write_new(&paused, b"restore-review")?;
    for target in pauses {
        if stat(&target)?.is_none() {
            place(&paused, &target, false)?;
        }
    }
    for Placement {
        entry,
        from,
        to,
        engine,
    } in attachments.into_values().chain(placements)
    {
        if let Some(id) = entry
            .strip_prefix("home/run/work-queue/files/")
            .and_then(|rest| rest.split('/').next())
            && let Some(held) = reviewed_payloads.get(id)
        {
            if *held {
                place(&from, &output(&recovery, &entry)?, false)?;
                result.held.push(entry);
            }
            continue;
        }
        if entry == "agents/usage/insights.sqlite"
            && let Some(scope) = &insights
        {
            let exists = stat(&to)?.is_some();
            if sqlite::merge_insights(exists.then_some(to.as_path()), &from, scope).is_err() {
                place(&from, &output(&recovery, &entry)?, false)?;
                result.unavailable.push(Unavailable {
                    entry,
                    reason:
                        "Insights index could not be reconciled; recovered copy held for review"
                            .into(),
                });
                continue;
            }
            if !exists {
                place(&from, &to, false)?;
            }
            result.installed += 1;
            continue;
        }
        let mut replace = false;
        if stat(&to)?.is_some() {
            if same(&to, &from)? {
                if to.starts_with(home.root()) {
                    files::sync_file_through(&to, home.root())?;
                } else {
                    files::sync_parent(&to)?;
                }
                result.installed += 1;
                continue;
            }
            replace = engine && sqlite::prepare_bootstrap(&to);
            if !replace {
                place(&from, &output(&recovery, &entry)?, false)?;
                result.unavailable.push(Unavailable {
                    entry,
                    reason: "existing local record; recovered copy held for review".into(),
                });
                continue;
            }
        }
        place(&from, &to, replace)?;
        result.installed += 1;
    }
    if let Some(plan) = project_plan {
        result.unavailable.extend(plan.finish(&recovery)?);
    }
    journal.state = State::Restored { result };
    files::write_atomic(&journal_file, &serde_json::to_vec(&journal)?)?;
    files::remove_file_synced(&backup.join("restoring"))?;
    let State::Restored { result } = journal.state else {
        unreachable!()
    };
    Ok(result)
}

fn rewrite_legacy_parts(
    entries: &BTreeMap<String, PathBuf>,
    addresses: &mut Addresses,
    unavailable: &mut Vec<Unavailable>,
) -> Result<()> {
    for (name, file) in entries {
        if !name.starts_with("engine/storage/part/") || !name.ends_with(".json") {
            continue;
        }
        let Ok(Some(mut part)) = json_at(file) else {
            continue;
        };
        if part.get("type").and_then(Value::as_str) == Some("text")
            && let Some(session) = super::attachments::legacy_session(name, &part)
            && let Some(message) = part.get("messageID").and_then(Value::as_str).filter(|id| {
                id.strip_prefix("msg_")
                    .is_some_and(|id| !id.is_empty() && super::url_token(id, id.len()))
            })
            && let Some(parent) =
                entries.get(&format!("engine/storage/message/{session}/{message}.json"))
            && let Ok(Some(info)) = json_at(parent)
            && info.get("role").and_then(Value::as_str) == Some("user")
        {
            let mut envelope = json!({ "info": info, "parts": [part] });
            if addresses.rewrite(&mut envelope, Format::Transcript, unavailable)? {
                let new_info = &envelope["info"];
                if new_info != &info {
                    files::write_atomic(parent, &serde_json::to_vec(new_info)?)?;
                }
                files::write_atomic(file, &serde_json::to_vec(&envelope["parts"][0])?)?;
            }
        } else if addresses.rewrite(&mut part, Format::Part, unavailable)? {
            files::write_atomic(file, &serde_json::to_vec(&part)?)?;
        }
    }
    Ok(())
}

fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null | Value::Bool(false)) => false,
        Some(Value::Number(number)) => number.as_f64() != Some(0.0),
        Some(Value::String(text)) => !text.is_empty(),
        _ => true,
    }
}
