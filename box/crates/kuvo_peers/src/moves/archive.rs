//! Gzip tar framing for move bundles. Installation belongs to the transfer
//! driver, after it verifies the manifest and the move's ownership.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{self, Read, Write},
    path::{Component, Path},
};

use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use kuvo_store::files;
use kuvo_wire::keys::encode;
use sha2::{Digest, Sha256};
use tar::{Archive, Builder, Header};

use super::{
    manifest::{self, Manifest, QueueFile, SessionFile},
    session_files,
    transfer::MAX_BUNDLE_BYTES,
    wake_archive,
    work_queue::ExpectedQueueFile,
};

const MANIFEST: &str = "move.json";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("What came isn’t a move.")]
    Invalid,
    #[error("The move exceeds the bundle size limit.")]
    TooLarge,
    #[error("The move needs files beside its manifest.")]
    RequiresFiles,
    #[error(transparent)]
    Manifest(#[from] manifest::Error),
    #[error("Reading or writing the move archive: {0}")]
    Archive(#[from] io::Error),
}

/// Read the manifest without extracting any files or following archive links.
pub fn read_manifest(path: &Path) -> Result<Manifest, Error> {
    read(path).map(|(manifest, _)| manifest)
}

/// Open the plain-chat form only. No other archive member can be silently
/// discarded by a caller that installs only the session JSON.
pub fn read_manifest_only(path: &Path) -> Result<Manifest, Error> {
    let (manifest, has_files) = read(path)?;
    if has_files || needs_files(&manifest) {
        return Err(Error::RequiresFiles);
    }
    Ok(manifest)
}

fn read(path: &Path) -> Result<(Manifest, bool), Error> {
    let file = File::open(path)?;
    if file.metadata()?.len() > MAX_BUNDLE_BYTES {
        return Err(Error::TooLarge);
    }
    let mut archive = Archive::new(GzDecoder::new(file));
    let mut manifest = None;
    let mut total = 0u64;
    let mut has_files = false;
    for entry in archive.entries()? {
        let entry = entry?;
        let path = entry.path()?;
        // These are tar paths. A backslash must never become a separator when
        // the staged data goes to a Windows filesystem.
        if path
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
            || entry.path_bytes().contains(&b'\\')
        {
            return Err(Error::Invalid);
        }
        total = total.saturating_add(entry.size());
        if total > MAX_BUNDLE_BYTES {
            return Err(Error::TooLarge);
        }
        if path == Path::new(MANIFEST) {
            if manifest.is_some() || !entry.header().entry_type().is_file() {
                return Err(Error::Invalid);
            }
            manifest = Some(Manifest::read_from(entry)?);
        } else {
            has_files = true;
        }
    }
    Ok((manifest.ok_or(Error::Invalid)?, has_files))
}

/// Pack a move that carries its entire payload in the manifest, such as a
/// conversation without side files. Files require an attachment inventory.
pub fn write_manifest_only(path: &Path, manifest: &Manifest) -> Result<(), Error> {
    if needs_files(manifest) {
        return Err(Error::RequiresFiles);
    }
    write_chat(path, manifest, None, None, None)
}

pub(super) fn write_chat(
    path: &Path,
    manifest: &Manifest,
    staged: Option<&Path>,
    wake: Option<&Path>,
    work_queue: Option<(&Path, &[QueueFile])>,
) -> Result<(), Error> {
    if manifest.folder.is_some() != wake.is_some()
        || manifest.lane.is_some()
        || manifest.work_queue.is_some() != work_queue.is_some()
    {
        return Err(Error::RequiresFiles);
    }
    let payloads = manifest.session_files.as_deref().unwrap_or_default();
    let bytes = manifest.encode()?;
    let content = payloads
        .iter()
        .map(|file| file.proof.size)
        .chain(
            work_queue
                .into_iter()
                .flat_map(|(_, files)| files.iter().map(|file| file.proof.size)),
        )
        .fold(bytes.len() as u64, u64::saturating_add);
    if content > MAX_BUNDLE_BYTES {
        return Err(Error::TooLarge);
    }
    if let Some(wake) = wake {
        wake_archive::measure(wake, MAX_BUNDLE_BYTES - content).map_err(|error| {
            if error.kind() == io::ErrorKind::FileTooLarge {
                Error::TooLarge
            } else {
                Error::Archive(error)
            }
        })?;
    }
    files::write_generated(path, |file| {
        let mut archive = Builder::new(GzEncoder::new(file, Compression::default()));
        let mut header = Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o600);
        archive.append_data(&mut header, MANIFEST, bytes.as_slice())?;
        for file in payloads {
            let root = staged.ok_or_else(|| io::Error::other("No staged attachments"))?;
            let source = session_files::checked_path(root, Path::new(&file.relative))?;
            let contents = files::open_payload(&source)?;
            let mut header = Header::new_gnu();
            header.set_size(file.proof.size);
            header.set_mode(0o600);
            archive.append_data(
                &mut header,
                format!("session-files/{}", file.relative),
                contents,
            )?;
        }
        if let Some((root, files)) = work_queue {
            for file in files {
                let source = session_files::checked_path(root, Path::new(&file.relative))?;
                let contents = files::open_payload(&source)?;
                let mut header = Header::new_gnu();
                header.set_size(file.proof.size);
                header.set_mode(0o600);
                archive.append_data(
                    &mut header,
                    format!("work-queue/{}", file.relative),
                    contents,
                )?;
            }
        }
        if let Some((wake, name)) = wake.zip(manifest.folder.as_deref()) {
            archive.follow_symlinks(false);
            archive.append_dir_all(name, wake)?;
        }
        archive.into_inner()?.finish()?;
        Ok(())
    })?;
    Ok(())
}

pub(super) fn unpack_chat_files(
    path: &Path,
    stage: &Path,
    payloads: &[SessionFile],
    wake: Option<(&str, &Path)>,
    queue: Option<(&Path, Vec<ExpectedQueueFile>)>,
) -> Result<Vec<QueueFile>, Error> {
    let mut wake = wake.map(|(label, path)| wake_archive::Staging::new(label, path));
    let mut expected: BTreeMap<_, _> = payloads
        .iter()
        .map(|file| {
            (
                format!("session-files/{}", file.relative),
                Expected::Session(stage, file),
            )
        })
        .collect();
    if let Some((stage, queue)) = queue {
        expected.extend(queue.into_iter().map(|file| {
            (
                format!("work-queue/{}", file.relative),
                Expected::Queue(stage, file),
            )
        }));
    }
    let dirs = expected
        .keys()
        .flat_map(|path| path.match_indices('/').map(move |(end, _)| &path[..end]))
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let mut queue_files = Vec::new();
    let mut archive = Archive::new(GzDecoder::new(File::open(path)?));
    for entry in archive.entries()? {
        let mut entry = entry?;
        let name = entry
            .path()?
            .to_str()
            .ok_or(Error::Invalid)?
            .trim_end_matches('/')
            .to_owned();
        let kind = entry.header().entry_type();
        if name == MANIFEST && kind.is_file() {
            continue;
        }
        if let Some(wake) = wake.as_mut()
            && wake.accept(&name, &mut entry)?
        {
            continue;
        }
        if kind.is_dir() && entry.size() == 0 && dirs.contains(&name) {
            continue;
        }
        let expected = expected.remove(&name).ok_or(Error::Invalid)?;
        let (stage, relative, size, sha256) = match &expected {
            Expected::Session(stage, file) => (
                *stage,
                file.relative.as_str(),
                file.proof.size,
                Some(file.proof.sha256.as_str()),
            ),
            Expected::Queue(stage, file) => (
                *stage,
                file.relative.as_str(),
                file.size,
                file.sha256.as_deref(),
            ),
        };
        if !kind.is_file() || entry.size() != size {
            return Err(Error::Invalid);
        }
        let destination = session_files::checked_path(stage, Path::new(relative))?;
        let mut verified = None;
        let exact = entry.size();
        let failure = match expected {
            Expected::Session(..) => "A session attachment did not match its receipt.",
            Expected::Queue(..) => "A Work Queue attachment did not match its receipt.",
        };
        files::write_generated(&destination, |to| {
            let mut hash = Sha256::new();
            let mut buffer = vec![0; 64 * 1024];
            let mut copied = 0;
            loop {
                let length = entry.read(&mut buffer)?;
                if length == 0 {
                    break;
                }
                hash.update(&buffer[..length]);
                to.write_all(&buffer[..length])?;
                copied += length as u64;
            }
            let digest = encode(&hash.finalize());
            if copied != exact || sha256.is_some_and(|given| given != digest) {
                return Err(io::Error::new(io::ErrorKind::InvalidData, failure));
            }
            verified = Some(digest);
            Ok(())
        })?;
        if let Expected::Queue(_, file) = expected {
            queue_files.push(file.installed(verified.expect("checked move attachment")));
        }
    }
    if !expected.is_empty() {
        return Err(Error::RequiresFiles);
    }
    if let Some(wake) = wake {
        wake.seal()?;
    }
    Ok(queue_files)
}

enum Expected<'a> {
    Session(&'a Path, &'a SessionFile),
    Queue(&'a Path, ExpectedQueueFile),
}

fn needs_files(manifest: &Manifest) -> bool {
    manifest.folder.is_some()
        || manifest.lane.is_some()
        || manifest.work_queue.is_some()
        || manifest
            .session_files
            .as_ref()
            .is_some_and(|files| !files.is_empty())
}
