//! Install a missing pinned OpenCode executable from the signed engine catalogue.

use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use flate2::read::GzDecoder;
use kuvo_store::{files, home::Home};
use sha2::{Digest, Sha512};

use crate::manifest::{EngineBuild, SignedRelease};

const EXECUTABLE: &str = if cfg!(windows) {
    "opencode.exe"
} else {
    "opencode"
};
const MAX_DOWNLOAD: u64 = 400_000_000;
const MAX_UNPACKED: u64 = 500_000_000;

pub(crate) struct EnginePack {
    installed: PathBuf,
    draft: Option<PathBuf>,
}

impl EnginePack {
    pub(crate) fn find_or_fetch(
        home: &Home,
        release: &SignedRelease,
        target: &str,
        scratch: &Path,
    ) -> io::Result<Self> {
        let installed = home
            .root()
            .join("engine")
            .join(release.engine())
            .join(EXECUTABLE);
        let draft = if installed.is_file() {
            None
        } else {
            let build = release
                .engine_build(target)
                .ok_or_else(|| io::Error::other("The release has no engine for this machine."))?;
            let archive = download(build, scratch)?;
            let executable = scratch.join(EXECUTABLE);
            unpack(&archive, &executable)?;
            Some(executable)
        };
        Ok(Self { installed, draft })
    }

    pub(crate) fn executable(&self) -> &Path {
        self.draft.as_deref().unwrap_or(&self.installed)
    }

    pub(crate) fn publish(self, home: &Home) -> io::Result<()> {
        if let Some(draft) = self.draft {
            files::create_private_dirs(self.installed.parent().expect("engine folder"))?;
            files::sync_file_through(&draft, home.root())?;
            fs::hard_link(&draft, &self.installed)?;
            files::sync_parent(&self.installed)?;
        }
        Ok(())
    }
}

fn download(build: EngineBuild, scratch: &Path) -> io::Result<PathBuf> {
    let bad_engine = || io::Error::other("The engine isn't what the release says.");
    let integrity = build
        .integrity
        .strip_prefix("sha512-")
        .and_then(|base64| STANDARD.decode(base64).ok())
        .filter(|bytes| bytes.len() == 64)
        .ok_or_else(bad_engine)?;
    let fetch_error =
        |error| io::Error::other(format!("Kuvo couldn't download the engine: {error}"));
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(900))
        .build()
        .map_err(fetch_error)?;
    let response = client
        .get(build.url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(fetch_error)?;
    let archive = scratch.join("engine.tar.gz");
    let mut file = File::create(&archive)?;
    let len = io::copy(&mut response.take(MAX_DOWNLOAD + 1), &mut file)?;
    if len > MAX_DOWNLOAD {
        return Err(bad_engine());
    }
    let mut digest = Sha512::new();
    io::copy(&mut File::open(&archive)?, &mut digest)?;
    if digest.finalize().as_slice() != integrity {
        return Err(bad_engine());
    }
    Ok(archive)
}

fn unpack(archive: &Path, executable: &Path) -> io::Result<()> {
    let invalid = || io::Error::other("The engine couldn't be unpacked.");
    let mut archive = tar::Archive::new(GzDecoder::new(File::open(archive)?).take(MAX_UNPACKED));
    let wanted = format!("package/bin/{EXECUTABLE}");
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.path_bytes() != wanted.as_bytes() {
            continue;
        }
        if !entry.header().entry_type().is_file() {
            return Err(invalid());
        }
        let mut file = File::create(executable)?;
        if io::copy(&mut entry, &mut file)? != entry.size() {
            return Err(invalid());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            file.set_permissions(fs::Permissions::from_mode(0o700))?;
        }
        return Ok(());
    }
    Err(invalid())
}
