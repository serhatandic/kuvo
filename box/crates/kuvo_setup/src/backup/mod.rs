//! Backup points and recovery in the TypeScript box's stored format.
mod archive;
mod artifacts;
mod attachments;
pub(crate) mod boundary;
mod destination;
mod inventory;
mod pending;
mod projects;
mod recovery;
pub mod relay;
pub mod routes;
mod sqlite;
pub mod vault;

use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use kuvo_store::{files, home::Home, locks::OwnedLock};
use serde::{Deserialize, Serialize};

pub use destination::{Destination, DestinationStore, DiskStore, SshStore};
pub use recovery::{Authority, RecoveryResult, Standalone};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Message(&'static str),
    #[error("{0}")]
    Detail(String),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Layout(#[from] kuvo_store::layout::Error),
    #[error(transparent)]
    Sqlite(#[from] kuvo_store::sqlite::Error),
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

fn random<const N: usize>() -> Result<[u8; N]> {
    super::random().map_err(|error| Error::Detail(error.to_string()))
}

/// Kept out of Debug and serialized only where the recovery key is expected.
pub struct RecoveryKey([u8; 32]);

impl RecoveryKey {
    pub fn generate() -> Result<Self> {
        Ok(Self(random()?))
    }

    pub fn parse(given: &str) -> Result<Self> {
        let mut bytes = [0; 32];
        if given.len() == 43 && URL_SAFE_NO_PAD.decode_slice(given, &mut bytes) == Ok(32) {
            return Ok(Self(bytes));
        }
        Err(Error::Message(
            "Enter the recovery key shown when this backup was set up.",
        ))
    }

    pub fn expose(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Daily,
    Move,
    Wipe,
    Update,
    Vault,
}

impl Reason {
    fn name(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Move => "move",
            Self::Wipe => "wipe",
            Self::Update => "update",
            Self::Vault => "vault",
        }
    }
}

fn url_token(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Source(String);

impl Source {
    pub fn generate() -> Result<Self> {
        Ok(Self(format!(
            "b_{}",
            URL_SAFE_NO_PAD.encode(random::<16>()?)
        )))
    }
}

impl TryFrom<String> for Source {
    type Error = Error;

    fn try_from(value: String) -> Result<Self> {
        if value.strip_prefix("b_").is_some_and(|id| url_token(id, 22)) {
            Ok(Self(value))
        } else {
            Err(Error::Message("Choose the backup's computer."))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PointId(String);

impl PointId {
    pub fn generate(reason: Reason, at: i64) -> Result<Self> {
        let date = jiff::Timestamp::from_millisecond(at)
            .map_err(|error| Error::Detail(error.to_string()))?;
        let date = date.strftime("%Y-%m-%dT%H%M%S%.3fZ");
        format!("{date}-{}-{}", reason.name(), hex::encode(random::<8>()?)).try_into()
    }

    pub fn reason(&self) -> Reason {
        match &self.0[23..self.0.len() - 17] {
            "daily" => Reason::Daily,
            "move" => Reason::Move,
            "wipe" => Reason::Wipe,
            "update" => Reason::Update,
            "vault" => Reason::Vault,
            _ => unreachable!("validated backup reason"),
        }
    }

    fn day(&self) -> &str {
        &self.0[..10]
    }
}

impl TryFrom<String> for PointId {
    type Error = Error;

    fn try_from(value: String) -> Result<Self> {
        let valid = value.get(..23).is_some_and(|date| {
            date.bytes()
                .zip(b"0000-00-00T000000.000Z-")
                .all(|(b, mask)| {
                    if *mask == b'0' {
                        b.is_ascii_digit()
                    } else {
                        b == *mask
                    }
                })
        }) && value
            .get(23..)
            .and_then(|rest| rest.split_once('-'))
            .is_some_and(|(reason, nonce)| {
                matches!(reason, "daily" | "move" | "wipe" | "update" | "vault")
                    && nonce.len() == 16
                    && nonce
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            });
        if valid {
            Ok(Self(value))
        } else {
            Err(Error::Message("Choose a dated backup."))
        }
    }
}

macro_rules! text_id {
    ($id:ty) => {
        impl fmt::Display for $id {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<$id> for String {
            fn from(value: $id) -> Self {
                value.0
            }
        }
    };
}

text_id!(Source);
text_id!(PointId);

pub fn expired_points(points: &[PointId], now: i64) -> Result<Vec<PointId>> {
    let date = |at| {
        jiff::Timestamp::from_millisecond(at)
            .map(|at| at.strftime("%Y-%m-%d").to_string())
            .map_err(|error| Error::Detail(error.to_string()))
    };
    let today = date(now)?;
    let mut daily: Vec<_> = points
        .iter()
        .filter(|id| id.reason() == Reason::Daily)
        .collect();
    daily.sort_unstable_by(|a, b| b.cmp(a));
    daily.dedup_by(|a, b| a.day() == b.day());
    let fallback = date(now.saturating_sub(29 * 86_400_000))?;
    let cutoff = daily.get(29).map_or(fallback.as_str(), |p| p.day());
    Ok(points
        .iter()
        .filter(|point| {
            point.day() <= today.as_str()
                && match point.reason() {
                    Reason::Vault => false,
                    Reason::Daily => !daily.iter().take(30).any(|keep| *keep == *point),
                    _ => point.day() < cutoff,
                }
        })
        .cloned()
        .collect())
}

fn context(source: &Source, id: &PointId) -> Vec<u8> {
    format!("backup\0{source}\0{id}").into_bytes()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RestorePoint {
    pub id: PointId,
    pub at: i64,
    pub version: u32,
}

#[derive(Serialize, Deserialize)]
pub struct Manifest {
    format: u32,
    pub source: Source,
    #[serde(flatten)]
    pub point: RestorePoint,
    pub origin: Option<String>,
    pub signer: Option<String>,
    pub setup: Option<String>,
    pub paired: bool,
    pub projects: Vec<String>,
}

impl Manifest {
    fn valid(&self, source: &Source, id: &PointId) -> bool {
        self.format == 1
            && self.source == *source
            && self.point.id == *id
            && self
                .origin
                .iter()
                .chain(self.signer.iter())
                .all(|key| url_token(key, 43))
    }
}

pub struct Backups {
    home: Home,
    roots: inventory::Roots,
}

impl Backups {
    pub fn new(home: Home) -> Self {
        Self::with_environment(home, &Default::default())
    }

    pub fn with_environment(home: Home, env: &kuvo_os::platform::Environment) -> Self {
        Self {
            roots: inventory::Roots::live(&home, env),
            home,
        }
    }

    fn scratch(&self) -> Result<(OwnedLock, tempfile::TempDir)> {
        let backup = self.home.root().join("backup");
        if inventory::stat(&backup)?.is_some_and(|meta| !meta.is_dir()) {
            return Err(Error::Message("The backup folder contains a link."));
        }
        files::private_folder(&backup)?;
        let guard = OwnedLock::acquire(&backup.join(".operation"), Duration::from_millis(100))
            .map_err(|error| {
                if error.kind() == io::ErrorKind::WouldBlock {
                    Error::Message("A backup or restore is already running.")
                } else {
                    error.into()
                }
            })?;
        for entry in fs::read_dir(&backup)? {
            let entry = entry?;
            if entry.file_name().to_string_lossy().starts_with(".scratch-") {
                match entry.file_type()? {
                    kind if kind.is_dir() => fs::remove_dir_all(entry.path())?,
                    _ => fs::remove_file(entry.path())?,
                }
            }
        }
        Ok((
            guard,
            tempfile::Builder::new()
                .prefix(".scratch-")
                .tempdir_in(backup)?,
        ))
    }

    pub fn export(
        &self,
        store: &(impl DestinationStore + ?Sized),
        source: &Source,
        key: &RecoveryKey,
        id: &PointId,
        at: i64,
        identity: &impl Authority,
    ) -> Result<RestorePoint> {
        let (_guard, scratch) = self.scratch()?;
        self.export_locked(store, source, key, id, at, identity, &[], scratch.path())
    }

    fn export_locked(
        &self,
        store: &(impl DestinationStore + ?Sized),
        source: &Source,
        key: &RecoveryKey,
        id: &PointId,
        at: i64,
        identity: &impl Authority,
        projects: &[PathBuf],
        scratch: &Path,
    ) -> Result<RestorePoint> {
        let aad = context(source, id);
        if store.list()?.contains(id) {
            let received = store.get(id, scratch)?;
            let entries = archive::open(&received, key, &aad, scratch)?;
            return Ok(recovery::manifest(&entries, source, id)?.point);
        }
        let version = inventory::layout(&self.home)?;
        let point = RestorePoint {
            id: id.clone(),
            at,
            version,
        };
        let mut manifest = identity.capture(source.clone(), point.clone())?;
        manifest.projects = projects
            .iter()
            .filter_map(|project| project.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .collect();
        let mut entries =
            inventory::capture(&self.home, &self.roots, &manifest, identity, scratch)?;
        projects::capture(projects, &mut entries)?;
        let mut draft = tempfile::NamedTempFile::new_in(scratch)?;
        archive::seal(draft.as_file_mut(), key, &aad, entries.into_iter().map(Ok))?;
        if inventory::layout(&self.home)? != version {
            return Err(Error::Message(
                "The folder layout changed during backup. Try again.",
            ));
        }
        identity.confirm_capture()?;
        store.put(id, draft.path())?;
        let verified = store
            .get(id, scratch)
            .and_then(|file| archive::authenticate(&file, key, &aad, None));
        if let Err(error) = verified {
            let _ = store.erase(id);
            return Err(error);
        }
        Ok(point)
    }

    /// Authenticate a previously published point again. Wipe commits its proof
    /// while this guard still excludes backup erasure and export.
    pub fn verify_for_wipe<T>(
        &self,
        store: &(impl DestinationStore + ?Sized),
        source: &Source,
        key: &RecoveryKey,
        id: &PointId,
        commit: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let (_guard, scratch) = self.scratch()?;
        store.get(id, scratch.path())
            .and_then(|file| archive::authenticate(&file, key, &context(source, id), None))
            .map_err(|_| Error::Message("The backup could not be read back and verified. Check its destination and retry."))?;
        commit()
    }

    pub fn restore(
        &self,
        store: &(impl DestinationStore + ?Sized),
        source: &Source,
        key: &RecoveryKey,
        id: &PointId,
        authority: &impl Authority,
    ) -> Result<RecoveryResult> {
        self.restore_with(store, source, key, id, || Ok(authority))
    }

    fn restore_with<A: Authority>(
        &self,
        store: &(impl DestinationStore + ?Sized),
        source: &Source,
        key: &RecoveryKey,
        id: &PointId,
        authority: impl FnOnce() -> Result<A>,
    ) -> Result<RecoveryResult> {
        if id.reason() == Reason::Vault {
            return Err(Error::Message("Choose a dated computer backup."));
        }
        let (_guard, scratch) = self.scratch()?;
        recovery::no_workers(&self.home)?;
        let file = store.get(id, scratch.path())?;
        let archived = archive::open(&file, key, &context(source, id), scratch.path())?;
        let manifest = recovery::manifest(&archived, source, id)?;
        let authority = authority()?;
        recovery::restore(
            &self.home,
            &self.roots,
            archived,
            &manifest,
            scratch.path(),
            &authority,
        )
    }

    pub fn root(&self) -> &Path {
        self.home.root()
    }
}

/// Identity checked by the computer that was explicitly selected for an SSH deposit.
pub fn ssh_identity(home: &Path) -> Result<String> {
    let kept: crate::model::Link = crate::storage::Files::new(home.to_owned())
        .read(crate::actor::LINK)
        .map_err(|error| Error::Detail(error.to_string()))?
        .ok_or(Error::Message(
            "The computer’s backup identity is unavailable.",
        ))?;
    if let Ok(identity) = crate::model::Identity::read(&kept) {
        return Ok(identity.public);
    }
    kept.extra
        .get("keyNow")
        .and_then(serde_json::Value::as_str)
        .filter(|key| url_token(key, 43))
        .map(str::to_owned)
        .ok_or(Error::Message(
            "The computer’s backup identity is unavailable.",
        ))
}
