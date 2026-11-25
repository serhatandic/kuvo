//! Keep the app's signed archives independently of the fleet's chosen release.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use kuvo_store::{files, home::Home};
use serde::{Deserialize, Serialize};

use super::{
    Candidate, ReleaseRef, installed, installed_envelope, read_record, retirement, selected,
    staging,
};
use crate::manifest::local_target;

#[derive(Default, Deserialize, Serialize)]
struct Offers {
    latest: Option<ReleaseRef>,
    previous: Option<ReleaseRef>,
    running: Option<ReleaseRef>,
}

impl Offers {
    fn load(root: &Path) -> Self {
        read_record(&root.join("offers.json"))
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    fn retained(&self) -> impl Iterator<Item = &ReleaseRef> {
        [&self.latest, &self.previous, &self.running]
            .into_iter()
            .flatten()
    }
}

fn root(home: &Home) -> io::Result<PathBuf> {
    super::update_folder(
        home,
        "app-offers",
        "The app's release folder is not a directory.",
    )
}

fn folder(root: &Path, release: &ReleaseRef) -> io::Result<PathBuf> {
    release.target()?;
    Ok(root.join(format!("{}-{}", release.version, release.sha256)))
}

pub(super) fn folders(home: &Home, version: &str) -> Vec<(PathBuf, Option<super::Release>)> {
    let Ok(root) = root(home) else {
        return Vec::new();
    };
    Offers::load(&root)
        .retained()
        .filter(|release| release.version == version)
        .filter_map(|release| Some((folder(&root, release).ok()?, Some(release.target().ok()?))))
        .collect()
}

pub(super) fn latest(home: &Home, key: &str) -> Option<Candidate> {
    let root = root(home).ok()?;
    let release = Offers::load(&root).latest?;
    let folder = folder(&root, &release).ok()?;
    if !fs::symlink_metadata(&folder).ok()?.is_dir()
        || folder.canonicalize().ok()? != root.canonicalize().ok()?.join(folder.file_name()?)
    {
        return None;
    }
    let manifest = folder.join("boxes.json");
    if !fs::symlink_metadata(&manifest).ok()?.is_file() {
        return None;
    }
    let bytes = staging::read_release(&manifest, staging::MAX_ENVELOPE).ok()?;
    let candidate = Candidate::verify(String::from_utf8(bytes).ok()?, key).ok()?;
    (candidate.local().ok()? == release
        && fs::symlink_metadata(folder.join("pack"))
            .is_ok_and(|entry| entry.is_file() && entry.len() <= staging::MAX_PACK))
    .then_some(candidate)
}

pub(super) fn receive(home: &Home, source: &Path, key: &str) -> io::Result<()> {
    let bytes = staging::read_release(&source.join("boxes.json"), staging::MAX_ENVELOPE)?;
    let candidate = Candidate::verify(String::from_utf8(bytes).map_err(io::Error::other)?, key)?;
    let release = candidate.local()?;
    let (version, running) = installed(home)?;
    let running = selected(home)?.or(running);
    let version = running
        .as_ref()
        .map(|target| target.version().to_owned())
        .or(version);
    let installed = running
        .as_ref()
        .and_then(|running| {
            installed_envelope(home, running)
                .and_then(|envelope| Candidate::verify(envelope, key).ok())
                .and_then(|candidate| candidate.local().ok())
                .or_else(|| {
                    read_record::<ReleaseRef>(
                        &running.folder(home.root()).join("bin/.installed.json"),
                    )
                    .ok()
                    .flatten()
                })
                .filter(|reference| reference.target().ok().as_ref() == Some(running))
        })
        .or_else(|| {
            read_record::<ReleaseRef>(&home.updates().join("installed.json"))
                .ok()
                .flatten()
                .filter(|reference| {
                    Some(reference.version.as_str()) == version.as_deref()
                        && reference.target().is_ok()
                })
        });
    if version.as_deref().and_then(super::version_order) > super::version_order(candidate.version())
        || version.as_deref() == Some(candidate.version())
            && installed
                .as_ref()
                .is_some_and(|installed| installed.sha256 != release.sha256)
    {
        return Ok(());
    }
    let pack = candidate.signed.pack_for(local_target()?)?;
    let archive = staging::read_release(&source.join(pack.filename()?), staging::MAX_PACK)?;
    if !release.matches_bytes(&archive) {
        return Err(io::Error::other("The release's files aren't what it says."));
    }
    keep(
        home,
        &candidate,
        release,
        version.as_deref(),
        installed.as_ref(),
        &archive,
    )
    .map_err(|error| {
        eprintln!("Keeping the signed app release: {error}");
        io::Error::other("Couldn’t keep the signed release.")
    })
}

fn keep(
    home: &Home,
    candidate: &Candidate,
    release: ReleaseRef,
    version: Option<&str>,
    installed: Option<&ReleaseRef>,
    archive: &[u8],
) -> io::Result<()> {
    let root = root(home)?;
    files::create_private_dirs(&root)?;
    let published = folder(&root, &release)?;
    // Publish complete media once. A peer can hold an open archive while another app offer arrives.
    let lease = retirement::share(home.root(), &release.target()?)?;
    if published.try_exists()? {
        if published.canonicalize()?
            != root
                .canonicalize()?
                .join(published.file_name().expect("offer name"))
            || !fs::symlink_metadata(&published)?.is_dir()
        {
            return Err(io::Error::other("Couldn’t keep the signed release."));
        }
        let pack = published.join("pack");
        if !fs::symlink_metadata(&pack).is_ok_and(|meta| meta.is_file())
            || !staging::read_release(&pack, staging::MAX_PACK)
                .is_ok_and(|existing| release.matches_bytes(&existing))
        {
            files::write_secret(&pack, archive)?;
        }
        // An identical signed archive may have a refreshed envelope (e.g. a new URL).
        files::write_secret(
            &published.join("boxes.json"),
            candidate.envelope().as_bytes(),
        )?;
    } else {
        let draft = tempfile::Builder::new()
            .prefix(".offer-")
            .tempdir_in(&root)?;
        files::write_secret(&draft.path().join("pack"), archive)?;
        files::write_secret(
            &draft.path().join("boxes.json"),
            candidate.envelope().as_bytes(),
        )?;
        files::replace_synced(draft.path(), &published)?;
    }
    let mut index = Offers::load(&root);
    let older = index.latest.as_ref().is_some_and(|previous| {
        super::version_order(&previous.version) > super::version_order(candidate.version())
    });
    if older {
        if version == Some(candidate.version()) {
            index.running = Some(release);
        } else {
            index.previous = Some(release);
        }
    } else {
        if let Some(previous) = index.latest.take() {
            if installed == Some(&previous) {
                index.running = Some(previous);
            } else if read_record::<ReleaseRef>(&home.run().join("fleet-ready.json"))?.as_ref()
                == Some(&previous)
            {
                index.previous = Some(previous);
            }
        }
        index.latest = Some(release);
    }
    files::write_secret(&root.join("offers.json"), &serde_json::to_vec(&index)?)?;
    drop(lease);
    // The files are immutable, and active users hold the release's shared OS lease.
    // Retire abandoned media first, then remove it outside the lease.
    if let Err(error) = retire_unused(home, &root, &index) {
        eprintln!("Removing superseded app release media: {error}");
    }
    Ok(())
}

fn retire_unused(home: &Home, root: &Path, index: &Offers) -> io::Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with(".retired-") || name.starts_with(".offer-") {
            files::remove_tree_synced(&entry.path())?;
            continue;
        }
        let Some((version, sha256)) = name.rsplit_once('-') else {
            continue;
        };
        let release = ReleaseRef {
            version: version.into(),
            sha256: sha256.into(),
        };
        let Ok(target) = release.target() else {
            continue;
        };
        if index.retained().any(|kept| kept == &release) {
            continue;
        }
        match retirement::exclusive(home.root(), &target) {
            Ok(lease) => {
                let retired = tempfile::Builder::new()
                    .prefix(".retired-")
                    .tempdir_in(root)?
                    .keep();
                files::replace_synced(&entry.path(), &retired.join("offer"))?;
                drop(lease);
                files::remove_tree_synced(&retired)?;
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
