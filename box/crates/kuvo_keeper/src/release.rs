use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
    str::FromStr,
};

use kuvo_store::files;
use serde::Deserialize;

/// The relative target used by the shell keeper's bin symlink and run/previous.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Release {
    version: String,
    digest: String,
}

impl FromStr for Release {
    type Err = io::Error;

    fn from_str(target: &str) -> io::Result<Self> {
        let invalid = || io::Error::new(io::ErrorKind::InvalidData, "Invalid Kuvo release target");
        let (version, digest) = target
            .strip_prefix("versions/")
            .and_then(|tail| tail.strip_suffix("/bin"))
            .and_then(|name| name.rsplit_once('-'))
            .ok_or_else(invalid)?;
        let digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
        let (base, beta) = version
            .split_once("-beta.")
            .map_or((version, None), |(base, beta)| (base, Some(beta)));
        if base.split('.').count() != 3
            || !base.split('.').all(digits)
            || beta.is_some_and(|beta| !digits(beta))
            || digest.len() != 12
            || !digest
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(invalid());
        }
        Ok(Self {
            version: version.into(),
            digest: digest.into(),
        })
    }
}

impl fmt::Display for Release {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "versions/{}-{}/bin", self.version, self.digest)
    }
}

impl Release {
    pub(crate) fn version(&self) -> &str {
        &self.version
    }

    pub(crate) fn read(path: &Path) -> io::Result<Option<Self>> {
        files::read(path)?
            .map(|bytes| {
                std::str::from_utf8(&bytes)
                    .map_err(io::Error::other)?
                    .trim_end_matches(['\r', '\n'])
                    .parse()
            })
            .transpose()
    }

    pub(crate) fn write(&self, path: &Path) -> io::Result<()> {
        files::write_secret(path, self.to_string().as_bytes())
    }

    pub(crate) fn folder(&self, home: &Path) -> PathBuf {
        home.join("versions")
            .join(format!("{}-{}", self.version, self.digest))
    }

    pub(crate) fn executable(&self, home: &Path) -> PathBuf {
        self.folder(home)
            .join("bin")
            .join(format!("kuvo-box{}", std::env::consts::EXE_SUFFIX))
    }

    pub(crate) fn validate(&self, home: &Path) -> io::Result<()> {
        #[derive(Deserialize)]
        struct Manifest {
            kuvo: String,
            version: String,
            launch: String,
        }
        let program = self.executable(home);
        let manifest = self.folder(home).join("box.json");
        let bytes = files::read(&manifest)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("No release manifest for {self}"),
            )
        })?;
        let release: Manifest = serde_json::from_slice(&bytes).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Manifest for {self}: {error}"),
            )
        })?;
        if release.kuvo != "box/1" || release.version != self.version || release.launch != "rust" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("The manifest for {self} does not describe this Rust release"),
            ));
        }
        if !fs::metadata(&program)?.is_file() {
            return Err(io::Error::other(format!("No executable in {self}")));
        }
        Ok(())
    }

    pub(crate) fn refusal(&self, seconds: u64) -> String {
        format!("{seconds} {} {}\n", self.version, self.digest)
    }
}
