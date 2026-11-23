//! Gzip tar framing for move bundles. Installation belongs to the transfer
//! driver, after it verifies the manifest and the move's ownership.

use std::{
    fs::File,
    io,
    path::{Component, Path},
};

use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use kuvo_store::files;
use tar::{Archive, Builder, Header};

use super::{
    manifest::{self, Manifest},
    transfer::MAX_BUNDLE_BYTES,
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
    let bytes = manifest.encode()?;
    if bytes.len() as u64 > MAX_BUNDLE_BYTES {
        return Err(Error::TooLarge);
    }
    files::write_generated(path, |file| {
        let mut archive = Builder::new(GzEncoder::new(file, Compression::default()));
        let mut header = Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o600);
        archive.append_data(&mut header, MANIFEST, bytes.as_slice())?;
        archive.into_inner()?.finish()?;
        Ok(())
    })?;
    Ok(())
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
