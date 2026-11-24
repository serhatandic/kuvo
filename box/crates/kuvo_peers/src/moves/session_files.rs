//! File inventories bind exact bytes to this move, the session and its former path.

use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use kuvo_store::files;
use kuvo_wire::keys::{decode_exact, encode};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{
    attachment_paths::{absolute, note_paths, portable_source, visit},
    manifest::{FileProof, SessionFile},
};

fn roots(home: &Path) -> Vec<PathBuf> {
    let mut roots = vec![home.join("attachments")];
    roots.extend(std::env::var_os("KUVO_MAC_ATTACHMENTS").map(PathBuf::from));
    roots
}

/// An inventory for owner reads. Relative, missing, or external paths can be
/// named in a snapshot without granting access to their bytes.
pub(crate) fn owner_attachment_files(
    rows: &[Value],
    local: Option<(&Path, &str)>,
) -> Vec<(String, bool)> {
    use std::collections::HashMap;

    let roots = local.map_or_else(Vec::new, |(home, _)| roots(home));
    let available = |path: &Path| {
        local.is_some_and(|(_, session)| {
            roots.iter().any(|root| {
                path.strip_prefix(root).ok().is_some_and(|relative| {
                    relative.starts_with(session)
                        && checked_path(root, relative).is_ok_and(|path| {
                            fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file())
                        })
                })
            })
        })
    };
    let mut files: Vec<(String, bool)> = Vec::new();
    let mut index = HashMap::new();
    let mut record = |name: &str, have: bool| {
        if name.is_empty() {
            return;
        }
        let name: String = name.chars().take(500).collect();
        let position = *index.entry(name.clone()).or_insert(files.len());
        if let Some((_, previous)) = files.get_mut(position) {
            *previous &= have;
        } else {
            files.push((name, have));
        }
    };
    let file = |entry: &Value, record: &mut dyn FnMut(&str, bool)| {
        if let Some(name) = entry.as_str() {
            return record(name, false);
        }
        let uri = entry["source"]["uri"]
            .as_str()
            .or(entry["uri"].as_str())
            .or(entry["url"].as_str());
        let path = match uri.filter(|uri| uri.starts_with("file:")) {
            Some(uri) => url::Url::parse(uri)
                .ok()
                .and_then(|url| url.to_file_path().ok()),
            None => entry["path"].as_str().map(PathBuf::from),
        };
        let name = entry["name"]
            .as_str()
            .or(entry["filename"].as_str())
            .or_else(|| {
                path.as_deref()
                    .and_then(Path::file_name)
                    .and_then(|leaf| leaf.to_str())
            })
            .or(entry["id"].as_str())
            .or(uri.map(|_| "attachment"));
        if let Some(name) = name {
            let embedded = (entry["source"]["type"] == "inline" && entry["data"].is_string())
                || uri.is_some_and(|uri| uri.starts_with("data:") && uri.contains(','));
            record(name, embedded || path.as_deref().is_some_and(available));
        }
    };
    let note = |text: &Value, record: &mut dyn FnMut(&str, bool)| {
        if let Some(text) = text.as_str() {
            note_paths(text, |path, stayed| {
                record(leaf(path), !stayed && available(Path::new(path)))
            });
        }
    };
    for row in rows {
        for entry in row["files"].as_array().into_iter().flatten() {
            file(entry, &mut record);
        }
        note(&row["text"], &mut record);
        for part in row["content"].as_array().into_iter().flatten() {
            match part["type"].as_str() {
                Some("file" | "image" | "attachment") => file(part, &mut record),
                Some("text") => note(part.get("text").unwrap_or(&part["content"]), &mut record),
                Some("tool") => {
                    for entry in part["state"]["content"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|entry| entry["type"] == "file")
                    {
                        file(entry, &mut record);
                    }
                }
                _ => {}
            }
        }
    }
    files
}

pub(super) fn stage(
    home: &Path,
    id: &str,
    sessions: &[String],
    exports: &mut [Value],
    queues: &mut BTreeMap<String, Value>,
    stage: &Path,
    mut limit: u64,
) -> io::Result<Vec<SessionFile>> {
    let roots = roots(home);
    let mut inventory = Vec::new();
    let mut seen = HashSet::new();
    for session in sessions {
        for root in &roots {
            let folder = checked_path(root, Path::new(session))?;
            walk(root, &folder, session, &mut seen, &mut inventory)?;
        }
    }
    visit(exports, queues, &mut |path, stayed| {
        let file = Path::new(path);
        let location = roots.iter().find_map(|root| {
            let relative = file.strip_prefix(root).ok()?;
            (!relative.as_os_str().is_empty()).then_some((root, relative))
        });
        let Some((root, relative)) = location.filter(|_| !stayed) else {
            return Err(io::Error::other(format!(
                "Some attachment references are outside the managed roots: {}. Review the export-only entries.",
                leaf(path),
            )));
        };
        let owner = relative
            .iter()
            .next()
            .and_then(|name| name.to_str())
            .and_then(|name| sessions.iter().find(|session| *session == name))
            .or_else(|| sessions.first())
            .ok_or_else(invalid)?;
        if seen.insert(file.to_owned()) {
            inventory.push((root.clone(), file.to_owned(), owner.clone()));
        }
        Ok(None)
    })?;
    let mut payloads = Vec::with_capacity(inventory.len());
    for (root, source, session) in inventory {
        let from = source.to_str().ok_or_else(invalid)?;
        let relative = format!("{session}/move-{id}/{}/{}", slot(from), leaf(from));
        let input = source.strip_prefix(&root).map_err(|_| invalid())?;
        let staged = checked_path(stage, Path::new(&relative))?;
        let proof = measure(&root, input, Some(&staged), limit, None).map_err(|error| {
            io::Error::other(format!(
                "Session attachments are missing, expired or unreadable: {}. {error}",
                leaf(from),
            ))
        })?;
        limit -= proof.size;
        payloads.push(SessionFile {
            from: from.into(),
            relative,
            proof,
        });
    }
    Ok(payloads)
}

fn walk(
    root: &Path,
    path: &Path,
    owner: &str,
    seen: &mut HashSet<PathBuf>,
    inventory: &mut Vec<(PathBuf, PathBuf, String)>,
) -> io::Result<()> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if meta.is_dir() {
        let mut children = fs::read_dir(path)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<io::Result<Vec<_>>>()?;
        children.sort();
        for child in children {
            walk(root, &child, owner, seen, inventory)?;
        }
    } else if meta.is_file() {
        if seen.insert(path.to_owned()) {
            inventory.push((root.to_owned(), path.to_owned(), owner.into()));
        }
    } else {
        return Err(io::Error::other(format!(
            "Session attachments are missing, expired or unreadable: {}. Review the move and removal plan.",
            path.file_name().unwrap_or_default().to_string_lossy()
        )));
    }
    Ok(())
}

pub(super) fn validate(id: &str, sessions: &[String], payloads: &[SessionFile]) -> io::Result<()> {
    let mut relative = HashSet::new();
    let mut sources = HashSet::new();
    let folder = format!("move-{id}");
    for file in payloads {
        let mut parts = file.relative.split('/');
        let (Some(session), Some(directory), Some(slot_name), Some(name), None) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            return Err(invalid());
        };
        if !sessions.iter().any(|each| each == session)
            || directory != folder
            || slot_name != slot(&file.from)
            || name != leaf(&file.from)
            || !safe_component(name)
            || !absolute(&file.from)
            || file.from.contains(['\0', '\r', '\n'])
            || decode_exact::<32>(&file.proof.sha256).is_none()
            || !relative.insert(file.relative.as_str())
            || !sources.insert(portable_source(&file.from))
        {
            return Err(invalid());
        }
    }
    Ok(())
}

pub(super) fn source_unchanged(home: &Path, payloads: &[SessionFile]) -> io::Result<()> {
    let roots = roots(home);
    for file in payloads {
        let source = Path::new(&file.from);
        let (root, relative) = roots
            .iter()
            .find_map(|root| {
                source
                    .strip_prefix(root)
                    .ok()
                    .map(|relative| (root, relative))
            })
            .ok_or_else(invalid)?;
        measure(root, relative, None, file.proof.size, Some(&file.proof))?;
    }
    Ok(())
}

pub(super) fn verify(home: &Path, payloads: &[SessionFile]) -> io::Result<()> {
    for file in payloads {
        measure(
            &home.join("attachments"),
            Path::new(&file.relative),
            None,
            file.proof.size,
            Some(&file.proof),
        )?;
    }
    Ok(())
}

pub(super) fn install(home: &Path, staged: &Path, payloads: &[SessionFile]) -> io::Result<()> {
    for file in payloads {
        let relative = Path::new(&file.relative);
        let destination = checked_path(&home.join("attachments"), relative)?;
        measure(
            staged,
            relative,
            Some(&destination),
            file.proof.size,
            Some(&file.proof),
        )?;
    }
    verify(home, payloads)
}

pub(super) fn require_unused_paths(home: &Path, payloads: &[SessionFile]) -> io::Result<()> {
    let root = home.join("attachments");
    for file in payloads {
        let destination = checked_path(&root, Path::new(&file.relative))?;
        match fs::symlink_metadata(destination) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "A move attachment already exists at its destination.",
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn rewrite(
    home: &Path,
    payloads: &[SessionFile],
    exports: &mut [Value],
    queues: &mut BTreeMap<String, Value>,
) -> io::Result<()> {
    // Some queue writers saved the composed prompt as the display text.
    // Where both fields match, use the typed words and leave the prompt intact.
    for entry in queues
        .values_mut()
        .filter_map(|queue| queue.get_mut("waiting").and_then(Value::as_array_mut))
        .flatten()
    {
        if entry["shown"]["text"].is_string()
            && entry["shown"]["text"] == entry["message"]["text"]
            && let Some(words) = entry
                .pointer("/message/metadata/kuvo/parts/words")
                .filter(|value| value.is_string())
                .cloned()
        {
            entry["shown"]["text"] = words;
        }
    }
    let targets: BTreeMap<_, _> = payloads
        .iter()
        .map(|file| (portable_source(&file.from), &file.relative))
        .collect();
    let root = home.join("attachments");
    visit(exports, queues, &mut |from, stayed| {
        let relative = targets.get(&portable_source(from)).filter(|_| !stayed).ok_or_else(|| io::Error::other(
            "A promised attachment has no file in the move. Update both computers and review the source files."
        ))?;
        Ok(Some(checked_path(&root, Path::new(relative))?))
    })
}

pub(super) fn discard(home: &Path, payloads: &[SessionFile]) -> io::Result<()> {
    let root = home.join("attachments");
    for file in payloads {
        let destination = checked_path(&root, Path::new(&file.relative))?;
        files::remove_file_synced(&destination)?;
    }
    Ok(())
}

pub(super) fn checked_path(root: &Path, relative: &Path) -> io::Result<PathBuf> {
    let mut target = root.to_owned();
    let names = relative.to_str().ok_or_else(invalid)?;
    if let Ok(meta) = fs::symlink_metadata(root)
        && !meta.is_dir()
    {
        return Err(invalid());
    }
    for name in names.split('/') {
        if !safe_component(name) {
            return Err(invalid());
        }
        target.push(name);
        match fs::symlink_metadata(&target) {
            Ok(meta) if meta.file_type().is_symlink() => return Err(invalid()),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(target)
}

fn measure(
    root: &Path,
    relative: &Path,
    copy: Option<&Path>,
    limit: u64,
    expected: Option<&FileProof>,
) -> io::Result<FileProof> {
    // The source path on Windows uses native separators. Archive paths have
    // already been validated as portable slash-separated components.
    let portable = relative
        .iter()
        .map(|part| part.to_str().ok_or_else(invalid))
        .collect::<io::Result<Vec<_>>>()?
        .join("/");
    let path = checked_path(root, Path::new(&portable))?;
    let mut file = files::open_payload(&path)?;
    let before = files::opened_file_stamp(&file)?;
    if before.size > limit
        || !path.canonicalize()?.starts_with(root.canonicalize()?)
        || files::file_stamp(&path)?.as_ref() != Some(&before)
    {
        return Err(invalid());
    }
    let mut measure = |mut output: Option<&mut fs::File>| {
        let mut hash = Sha256::new();
        let mut buffer = vec![0; 64 * 1024];
        let mut size = 0;
        loop {
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            size += n as u64;
            if size > before.size {
                return Err(invalid());
            }
            hash.update(&buffer[..n]);
            if let Some(target) = output.as_mut() {
                target.write_all(&buffer[..n])?;
            }
        }
        let proof = FileProof {
            size,
            sha256: encode(&hash.finalize()),
        };
        if size != before.size
            || files::opened_file_stamp(&file)? != before
            || files::file_stamp(&path)?.as_ref() != Some(&before)
            || expected.is_some_and(|expected| expected != &proof)
        {
            return Err(io::Error::other(
                "A managed file no longer matches its recorded bytes.",
            ));
        }
        Ok(proof)
    };
    if let Some(copy) = copy {
        let mut proof = None;
        files::write_generated_new(copy, |target| {
            proof = Some(measure(Some(target))?);
            Ok(())
        })?;
        Ok(proof.expect("the staged payload was measured"))
    } else {
        measure(None)
    }
}

fn safe_component(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['\\', '/', '\0', '\r', '\n'])
        && matches!(
            Path::new(name).components().next(),
            Some(std::path::Component::Normal(_))
        )
        && (!cfg!(windows) || !name.contains(':'))
}

fn leaf(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn slot(path: &str) -> String {
    format!("{:x}", Sha256::digest(path.as_bytes()))
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Invalid session attachment inventory.",
    )
}
