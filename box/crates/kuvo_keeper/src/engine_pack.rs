//! Install a missing pinned OpenCode executable from the signed engine catalogue.

use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use flate2::read::GzDecoder;
use kuvo_store::{files, home::Home};
use serde::Deserialize;
use sha2::{Digest, Sha512};

use crate::{
    manifest::{EngineBuild, EngineCatalogue, SignedRelease, local_target},
    staging::check_engine,
};

const EXECUTABLE: &str = if cfg!(windows) {
    "opencode.exe"
} else {
    "opencode"
};
const MAX_DOWNLOAD: u64 = 400_000_000;
const MAX_UNPACKED: u64 = 500_000_000;

#[derive(Deserialize)]
struct Pin {
    version: String,
    #[serde(flatten)]
    engines: EngineCatalogue,
}

/// Provision the engine pinned in this executable in an already placed Kuvo home.
/// The caller owns the installation reservation and starts the engine separately.
pub async fn provision_pinned_engine(home: &Home) -> io::Result<()> {
    let pin: Pin = serde_json::from_str(include_str!("../../../src/shared/engine-pin.json"))?;
    let build = pin.engines.build_for(local_target()?);
    let home = home.clone();
    tokio::task::spawn_blocking(move || provision(&home, &pin.version, build))
        .await
        .map_err(io::Error::other)?
}

fn provision(home: &Home, version: &str, build: Option<EngineBuild>) -> io::Result<()> {
    let home = Home::new(home.root().canonicalize()?)?;
    let engine = home.root().join("engine");
    let installed = engine.join(version).join(EXECUTABLE);
    let version_folder = installed.parent().expect("engine version folder");
    for folder in [&engine, version_folder] {
        match fs::symlink_metadata(folder) {
            Ok(meta) if meta.is_dir() && folder.canonicalize()? == folder => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Ok(_) => {
                return Err(io::Error::other(
                    "The engine folder is outside its installation.",
                ));
            }
            Err(error) => return Err(error),
        }
    }
    match fs::symlink_metadata(&installed) {
        Ok(meta) if !meta.is_file() || installed.canonicalize()? != installed => {
            return Err(io::Error::other(
                "The installed engine isn't a regular file here.",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    files::create_private_dirs(&engine)?;
    let scratch = tempfile::Builder::new()
        .prefix(".staging.")
        .tempdir_in(&engine)?;
    let pack = EnginePack::in_home(installed.clone(), || build, scratch.path())?;
    check_engine(
        pack.executable(),
        None,
        "The engine doesn't run on this computer.",
    )?;
    match pack.publish(&home) {
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            check_engine(&installed, None, "The engine doesn't run on this computer.")
        }
        outcome => outcome,
    }
}

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
        downloading: impl FnOnce(),
    ) -> io::Result<Self> {
        let installed = home
            .root()
            .join("engine")
            .join(release.engine())
            .join(EXECUTABLE);
        Self::in_home(
            installed,
            || {
                downloading();
                release.engine_build(target)
            },
            scratch,
        )
    }

    fn in_home(
        installed: PathBuf,
        build: impl FnOnce() -> Option<EngineBuild>,
        scratch: &Path,
    ) -> io::Result<Self> {
        let draft = if installed.is_file() {
            None
        } else {
            let build = build()
                .ok_or_else(|| io::Error::other("The release has no engine for this machine."))?;
            Some(fetch_engine(build, scratch)?)
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

/// Status can use any installed engine. On a fresh computer, check the pinned
/// download without retaining any installer files.
pub(crate) fn check_installer_engine(
    home: &Home,
    release: &SignedRelease,
    target: &str,
) -> io::Result<()> {
    let installed = fs::read_dir(home.root().join("engine"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| !entry.file_name().as_encoded_bytes().starts_with(b"."))
        .any(|entry| {
            fs::metadata(entry.path().join(EXECUTABLE)).is_ok_and(|metadata| {
                #[cfg(unix)]
                let executable = {
                    use std::os::unix::fs::PermissionsExt as _;
                    metadata.permissions().mode() & 0o111 != 0
                };
                #[cfg(not(unix))]
                let executable = true;
                metadata.is_file() && executable
            })
        });
    if installed {
        return Ok(());
    }
    let here = if cfg!(target_os = "macos") {
        "this Mac"
    } else {
        "this computer"
    };
    let build = release.engine_build(target).ok_or_else(|| {
        io::Error::other("This copy of Kuvo has no engine for this kind of computer.")
    })?;
    receive(build, io::sink()).map_err(|error| match error.kind() {
        io::ErrorKind::InvalidData => io::Error::other(
            "The engine that was downloaded isn't what this copy of Kuvo's release says. Try again.",
        ),
        io::ErrorKind::NotConnected => io::Error::other(format!(
            "The engine couldn't be downloaded. Check that {here} is online, then try again."
        )),
        _ => error,
    })
}

fn fetch_engine(build: EngineBuild, scratch: &Path) -> io::Result<PathBuf> {
    let archive = scratch.join("engine.tar.gz");
    receive(build, File::create(&archive)?)?;
    let executable = scratch.join(EXECUTABLE);
    unpack(&archive, &executable)?;
    Ok(executable)
}

fn receive(build: EngineBuild, mut output: impl Write) -> io::Result<()> {
    let bad_engine = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "The engine isn't what the release says.",
        )
    };
    let integrity = build
        .integrity
        .strip_prefix("sha512-")
        .and_then(|base64| STANDARD.decode(base64).ok())
        .filter(|bytes| bytes.len() == 64)
        .ok_or_else(bad_engine)?;
    let fetch_error = |error| {
        io::Error::new(
            io::ErrorKind::NotConnected,
            format!("Kuvo couldn't download the engine: {error}"),
        )
    };
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(900))
        .build()
        .map_err(fetch_error)?;
    let mut response = client
        .get(build.url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(fetch_error)?
        .take(MAX_DOWNLOAD + 1);
    let mut size = 0;
    let mut digest = Sha512::new();
    let mut buffer = [0; 32 * 1024];
    loop {
        let read = response
            .read(&mut buffer)
            .map_err(|error| io::Error::new(io::ErrorKind::NotConnected, error))?;
        if read == 0 {
            break;
        }
        size += read as u64;
        if size > MAX_DOWNLOAD {
            return Err(bad_engine());
        }
        let bytes = &buffer[..read];
        digest.update(bytes);
        output.write_all(bytes)?;
    }
    if digest.finalize().as_slice() != integrity {
        return Err(bad_engine());
    }
    Ok(())
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use std::{io::Write, net::TcpListener, os::unix::fs::PermissionsExt};

    #[test]
    fn provisioning_verifies_download_and_reuses_the_pinned_engine() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let home = Home::new(root.path().canonicalize()?)?;
        let mut archive = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        let program = br#"#!/bin/sh
set -eu
if [ -n "${XDG_DATA_HOME:-}" ]; then
    [ -d "$XDG_DATA_HOME" ] && [ -w "$XDG_DATA_HOME" ] || exit 1
    [ ! -e "$XDG_DATA_HOME/opencode" ] || [ -d "$XDG_DATA_HOME/opencode" ] || exit 1
fi
printf 'opencode v2.2.6\n'
"#;
        let mut entry = tar::Header::new_gnu();
        entry.set_mode(0o700);
        entry.set_size(program.len() as u64);
        archive.append_data(&mut entry, "package/bin/opencode", &program[..])?;
        let bytes = archive.into_inner()?.finish()?;
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        let url = format!("http://{}/engine.tgz", listener.local_addr()?);
        let integrity = format!("sha512-{}", STANDARD.encode(Sha512::digest(&bytes)));
        let server = std::thread::spawn(move || -> io::Result<()> {
            let (mut stream, _) = listener.accept()?;
            let mut request = [0; 1024];
            let _ = stream.read(&mut request)?;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            )?;
            stream.write_all(&bytes)
        });

        provision(&home, "2.2.6", Some(EngineBuild { url, integrity }))?;
        server.join().expect("fixture feed panicked")?;
        let installed = home.root().join("engine/2.2.6/opencode");
        assert_eq!(fs::read(&installed)?, program);
        provision(&home, "2.2.6", None)?;

        let wrong = b"#!/bin/sh\nprintf 'opencode v2.2.6\\n'\nexit 1\n";
        fs::write(&installed, wrong)?;
        fs::set_permissions(&installed, fs::Permissions::from_mode(0o700))?;
        assert_eq!(
            provision(&home, "2.2.6", None).unwrap_err().to_string(),
            "The engine doesn't run on this computer."
        );
        assert_eq!(fs::read(&installed)?, wrong);
        assert_eq!(files::list_of(&home.root().join("engine")).len(), 1);
        Ok(())
    }
}
