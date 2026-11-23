//! The encrypted deposit protocol. The relay only transports peer mail; the
//! receiving computer owns the reservation and its ciphertext.
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use kuvo_store::{declarations, files, format::ObjectFormat, locks::OwnedLock};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    DestinationStore, Error, PointId, Result, Source, archive, inventory::stat, recovery::output,
};

pub mod mail;

const CHUNK: usize = 256 * 1024;
const MAX_ARCHIVE: u64 = 2 * 1024 * 1024 * 1024;
const MAX_STORED: u64 = 32 * 1024 * 1024 * 1024;
const MAX_STAGING: u64 = 4 * 1024 * 1024 * 1024;
const MAX_UPLOADS: u32 = 4;
const MAX_POINTS: u32 = 4096;
const STALE: Duration = Duration::from_secs(60 * 60);
const FORMAT: ObjectFormat = ObjectFormat::additive("Relay backup upload reservation");
const REFUSAL: &str = "Wait for accepted backup membership.";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    Get,
    Post,
    Delete,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Action {
    List,
    Read,
    Begin,
    Append,
    Commit,
    Abort,
}

#[derive(Default, Deserialize, Serialize)]
pub struct Request {
    setup: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<Action>,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<PointId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    offset: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct Reply {
    pub status: u16,
    pub value: Value,
}

pub trait Mail: Send + Sync {
    /// Send with the resident's accepted peer identity, and correlate the reply
    /// to the selected computer and request.
    fn request(&self, to: &str, method: Method, source: &Source, request: Request)
    -> Result<Reply>;
}

pub trait Scope: Send + Sync {
    /// Check sender and receiver against current membership. For reads and
    /// erase, include revoked owners matched to this sender and source.
    fn owners(
        &self,
        sender: &str,
        setup: &str,
        source: &Source,
        deposit: bool,
    ) -> Result<Vec<String>>;
}

#[derive(PartialEq, Eq, Serialize, Deserialize)]
struct Upload {
    source: Source,
    id: PointId,
    size: u64,
    sha256: String,
}

impl Upload {
    fn valid(&self) -> bool {
        (1..=MAX_ARCHIVE).contains(&self.size)
            && self.sha256.len() == 64
            && self
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }

    fn read(path: &Path, ignore_corrupt: bool) -> Result<Option<(Self, bool)>> {
        let Some(info) = stat(path)? else {
            return Ok(None);
        };
        let corrupt = || {
            if ignore_corrupt {
                Ok(None)
            } else {
                Err(Error::Message("The backup storage needs attention."))
            }
        };
        if !info.is_file() || info.len() > 4096 {
            return Err(Error::Message("The backup storage needs attention."));
        }
        let Ok(value) = serde_json::from_reader(archive::regular(path)?) else {
            return corrupt();
        };
        let decoded = FORMAT
            .decode(value)
            .map_err(|error| Error::Detail(error.to_string()))?;
        let upload: Self = match serde_json::from_value(Value::Object(decoded.fields)) {
            Ok(upload) if Self::valid(&upload) => upload,
            _ => return corrupt(),
        };
        Ok(Some((upload, decoded.version != FORMAT.current)))
    }

    fn write(&self, path: &Path, root: &Path) -> Result<()> {
        let fields = super::super::object(serde_json::to_value(self)?);
        files::write_atomic_through(path, &serde_json::to_vec(&FORMAT.encode(fields))?, root)?;
        Ok(())
    }
}

fn hash(file: &Path) -> Result<String> {
    let mut digest = Sha256::new();
    io::copy(&mut archive::regular(file)?, &mut digest)?;
    Ok(hex::encode(digest.finalize()))
}

fn location(name: &str) -> declarations::ExternalStorage {
    declarations::data(name)
        .and_then(|entry| entry.external)
        .expect("Relay storage is declared")
}

fn archive_name(id: &PointId) -> String {
    format!(
        "{id}{}",
        location("relayBackupArchives")
            .archive_suffix
            .expect("relay suffix")
    )
}

struct Staging {
    journal: PathBuf,
    partial: PathBuf,
}

impl Staging {
    fn from_journal(journal: PathBuf) -> Self {
        let partial = journal.with_file_name(
            location("relayBackupUploads")
                .partial
                .expect("relay partial"),
        );
        Self { journal, partial }
    }

    fn retire(&self, root: &Path) -> Result<()> {
        files::remove_file_synced(&self.partial)?;
        files::remove_file_synced(&self.journal)?;
        sync_removal(&self.journal, root)
    }

    fn expire(&self, root: &Path, now: SystemTime) -> Result<bool> {
        let Some(meta) = stat(&self.journal)?.filter(|meta| meta.is_file()) else {
            return Ok(false);
        };
        let mut modified = meta.modified()?;
        if let Some(track) = stat(&self.partial)? {
            modified = modified.max(track.modified()?);
        }
        if now.duration_since(modified).is_ok_and(|age| age >= STALE) {
            Upload::read(&self.journal, true)?;
            self.retire(root)?;
            return Ok(true);
        }
        Ok(false)
    }
}

fn sync_removal(path: &Path, root: &Path) -> Result<()> {
    let mut folder = path.parent().expect("backup path has a parent");
    while folder != root {
        match fs::remove_dir(folder) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => break,
            Err(error) => return Err(error.into()),
        }
        folder = folder.parent().expect("backup folder has a parent");
    }
    files::sync_folder_through(folder, root)?;
    Ok(())
}

fn reserve(root: &Path, size: u64, now: SystemTime) -> Result<()> {
    #[derive(Default)]
    struct Counts {
        stored: u64,
        staging: u64,
        unwritten: u64,
        points: u32,
        uploads: u32,
    }
    let mut totals = Counts::default();
    let upload = location("relayBackupUploads")
        .journal
        .expect("relay journal");
    let suffix = location("relayBackupArchives")
        .archive_suffix
        .expect("relay suffix");
    fn walk(
        folder: &Path,
        root: &Path,
        journal: &str,
        suffix: &str,
        now: SystemTime,
        totals: &mut Counts,
    ) -> Result<()> {
        for file in fs::read_dir(folder)? {
            let file = file?;
            let leaf = file.file_name();
            let leaf = leaf.to_string_lossy();
            let kind = file.file_type()?;
            if leaf == journal && kind.is_file() {
                let staging = Staging::from_journal(file.path());
                if !staging.expire(root, now)?
                    && let Some((entry, _)) = Upload::read(&staging.journal, false)?
                {
                    let present = stat(&staging.partial)?.map_or(0, |info| info.len());
                    totals.unwritten = totals
                        .unwritten
                        .saturating_add(entry.size.saturating_sub(present));
                    totals.staging = totals.staging.saturating_add(entry.size);
                    totals.uploads = totals.uploads.saturating_add(1);
                }
            } else if !leaf.starts_with('.') && kind.is_dir() {
                walk(&file.path(), root, journal, suffix, now, totals)?;
            } else if !leaf.starts_with('.') && kind.is_file() && leaf.ends_with(suffix) {
                totals.points = totals.points.saturating_add(1);
                totals.stored = totals.stored.saturating_add(file.metadata()?.len());
            }
        }
        Ok(())
    }
    walk(root, root, upload, suffix, now, &mut totals)?;
    if totals.uploads >= MAX_UPLOADS
        || totals.points.saturating_add(totals.uploads) >= MAX_POINTS
        || totals.staging.saturating_add(size) > MAX_STAGING
        || totals
            .stored
            .saturating_add(totals.staging)
            .saturating_add(size)
            > MAX_STORED
        || totals.unwritten.saturating_add(size) > kuvo_os::platform::available_space(root)?
    {
        return Err(Error::Message(
            "The receiving computer doesn’t have enough backup capacity.",
        ));
    }
    Ok(())
}

pub struct Receiver<S> {
    account: PathBuf,
    scope: S,
}

impl<S: Scope> Receiver<S> {
    pub fn new(account: PathBuf, scope: S) -> Self {
        Self { account, scope }
    }

    pub fn apply(
        &self,
        sender: &str,
        method: Method,
        source: &Source,
        request: Request,
        now: SystemTime,
    ) -> Reply {
        let response = self.apply_inner(sender, method, source, request, now);
        response.unwrap_or_else(|error| Reply {
            status: 409,
            value: json!({ "message": error.to_string() }),
        })
    }

    fn authorize(
        &self,
        sender: &str,
        method: Method,
        source: &Source,
        setup: &str,
    ) -> Result<Vec<String>> {
        if !setup
            .strip_prefix("set_")
            .is_some_and(|id| super::url_token(id, 22))
            || !super::url_token(sender, 43)
        {
            return Err(Error::Message(REFUSAL));
        }
        let owners = self
            .scope
            .owners(sender, setup, source, method == Method::Post)?;
        if owners.first().is_none_or(|owner| owner != sender)
            || (method == Method::Post && owners.len() != 1)
            || owners.iter().any(|owner| !super::url_token(owner, 43))
        {
            return Err(Error::Message(REFUSAL));
        }
        Ok(owners)
    }

    fn apply_inner(
        &self,
        sender: &str,
        method: Method,
        source: &Source,
        given: Request,
        now: SystemTime,
    ) -> Result<Reply> {
        self.authorize(sender, method, source, &given.setup)?;
        let account = self.account.canonicalize()?;
        let root = output(&account, location("relayBackupArchives").root)?;
        check_private(&root)?;
        let _guard = OwnedLock::acquire(&root.join(".operation"), Duration::from_millis(100))?;
        let owners = self.authorize(sender, method, source, &given.setup)?;
        let yes = |value| Ok(Reply { status: 200, value });
        let no = |status, message| {
            Ok(Reply {
                status,
                value: json!({ "message": message }),
            })
        };
        let staging_of = |owner| {
            output(
                &root,
                &format!(
                    "{}/{owner}/{}",
                    given.setup,
                    location("relayBackupUploads")
                        .journal
                        .expect("relay journal")
                ),
            )
            .map(Staging::from_journal)
        };
        let archives = |owner| output(&root, &format!("{}/{owner}/{source}", given.setup));
        if method == Method::Get && given.action == Some(Action::List) {
            let suffix = location("relayBackupArchives")
                .archive_suffix
                .expect("relay suffix");
            let mut points = BTreeSet::new();
            for owner in &owners {
                let folder = archives(owner)?;
                if !stat(&folder)?.is_some_and(|meta| meta.is_dir()) {
                    continue;
                }
                for file in fs::read_dir(folder)? {
                    let file = file?;
                    if file.file_type()?.is_file()
                        && let Some(id) = file
                            .file_name()
                            .to_string_lossy()
                            .strip_suffix(suffix)
                            .and_then(|id| PointId::try_from(id.to_owned()).ok())
                    {
                        points.insert(id);
                    }
                }
            }
            return yes(json!(points.into_iter().rev().collect::<Vec<_>>()));
        }
        let Some(id) = given.id else {
            return no(400, "Choose a backup point.");
        };
        let paths: Vec<_> = owners
            .iter()
            .map(|owner| {
                output(
                    &root,
                    &format!("{}/{owner}/{source}/{}", given.setup, archive_name(&id)),
                )
            })
            .collect::<Result<_>>()?;
        if method != Method::Post {
            let present: Vec<_> = paths
                .iter()
                .enumerate()
                .map(|(i, path)| Ok((i, stat(path)?.is_some_and(|meta| meta.is_file()))))
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .filter_map(|(i, ok)| ok.then_some(i))
                .collect();
            if present.len() > 1 {
                return no(409, "The backup point has ambiguous ownership.");
            }
            let only = present.first().copied();
            if method == Method::Delete {
                for (index, owner) in owners
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| only.is_none_or(|only| only == *i))
                {
                    let staging = staging_of(owner)?;
                    if Upload::read(&staging.journal, true)?
                        .is_some_and(|(upload, _)| upload.source == *source && upload.id == id)
                    {
                        staging.retire(&root)?;
                    }
                    if only == Some(index) {
                        files::remove_file_synced(&paths[index])?;
                    }
                    sync_removal(&paths[index], &root)?;
                }
                return yes(json!({}));
            }
            let Some(index) = only else {
                return no(404, "The backup point is unavailable.");
            };
            let Some(offset) = given.offset.filter(|_| given.action == Some(Action::Read)) else {
                return no(400, "Invalid backup read.");
            };
            let mut file = archive::regular(&paths[index])?;
            let size = file.metadata()?.len();
            if offset > size {
                return no(400, "Invalid backup read.");
            }
            let mut bytes = vec![0; (size - offset).min(CHUNK as u64) as usize];
            file.seek(SeekFrom::Start(offset))?;
            file.read_exact(&mut bytes)?;
            return yes(json!({"size": size, "data": URL_SAFE_NO_PAD.encode(bytes)}));
        }

        let published = &paths[0];
        let staging = staging_of(&owners[0])?;
        let action = given.action;
        if action == Some(Action::Begin) {
            staging.expire(&root, now)?;
        }
        let saved = Upload::read(&staging.journal, false)?;
        let request = Upload {
            source: source.clone(),
            id,
            size: given.size.unwrap_or_default(),
            sha256: given.sha256.unwrap_or_default(),
        };
        let same = saved.as_ref().is_some_and(|(upload, _)| *upload == request);
        let info = stat(&staging.partial)?;
        if action == Some(Action::Abort) {
            if same {
                staging.retire(&root)?;
            }
            return yes(json!({}));
        }
        if !request.valid() {
            return no(
                409,
                "The encrypted backup exceeds the Relay archive limit or has an invalid digest.",
            );
        }
        if let Some(info) = stat(published)? {
            if action != Some(Action::Commit)
                || !info.is_file()
                || info.len() != request.size
                || hash(published)? != request.sha256
            {
                return no(409, "That backup point already exists.");
            }
            self.authorize(sender, method, source, &given.setup)?;
            files::sync_file_through(published, &account)?;
            if same {
                staging.retire(&root)?;
            }
            return yes(json!({}));
        }
        let legacy = same && saved.as_ref().is_some_and(|(_, legacy)| *legacy);
        let size = info
            .as_ref()
            .filter(|info| info.is_file() && info.len() <= request.size)
            .map(|info| info.len());
        if action == Some(Action::Begin) {
            if same && let Some(size) = size {
                if legacy {
                    request.write(&staging.journal, &root)?;
                }
                return yes(json!({ "offset": size }));
            }
            if same && info.is_none() {
                staging.retire(&root)?;
            }
            reserve(&root, request.size, now)?;
            if stat(&staging.journal)?.is_some() || stat(&staging.partial)?.is_some() {
                return no(409, "This computer already has an upload in progress.");
            }
            request.write(&staging.journal, &root)?;
            if let Err(error) = files::write_new(&staging.partial, b"") {
                staging.retire(&root)?;
                return Err(error.into());
            }
            return yes(json!({ "offset": 0 }));
        }
        let Some(size) = size.filter(|_| same) else {
            return no(409, "The backup deposit is no longer open.");
        };
        if action == Some(Action::Append) {
            let Some(data) = given.data.filter(|data| {
                !data.is_empty()
                    && super::url_token(data, data.len())
                    && data.len() <= (CHUNK * 4).div_ceil(3)
            }) else {
                return no(400, "Invalid backup chunk.");
            };
            let Ok(bytes) = URL_SAFE_NO_PAD.decode(data) else {
                return no(400, "Invalid backup offset.");
            };
            let Some(offset) = given
                .offset
                .filter(|offset| *offset <= size && *offset + bytes.len() as u64 <= request.size)
            else {
                return no(400, "Invalid backup offset.");
            };
            if legacy {
                request.write(&staging.journal, &root)?;
            }
            let mut part = staging_file(&staging.partial)?;
            part.seek(SeekFrom::Start(offset))?;
            if offset < size {
                let mut old = vec![0; bytes.len()];
                if part.read_exact(&mut old).is_err() || old != bytes {
                    return no(409, "The repeated backup chunk differs.");
                }
            } else if let Err(error) = part.write_all(&bytes) {
                drop(part);
                staging.retire(&root)?;
                return Err(error.into());
            }
            return yes(json!({ "offset": offset + bytes.len() as u64 }));
        }
        if action != Some(Action::Commit) || size != request.size {
            return no(409, "The encrypted deposit is incomplete.");
        }
        if hash(&staging.partial)? != request.sha256 {
            staging.retire(&root)?;
            return no(409, "The encrypted deposit failed its checksum.");
        }
        self.authorize(sender, method, source, &given.setup)?;
        files::create_private_dirs(published.parent().expect("published archive has a parent"))?;
        files::sync_file_through(&staging.partial, &root)?;
        fs::hard_link(&staging.partial, published)?;
        files::sync_file_through(published, &account)?;
        staging.retire(&root)?;
        yes(json!({}))
    }
}

fn check_private(root: &Path) -> Result<()> {
    let parent = root.parent().expect("relay storage has a parent");
    files::create_private_dirs(parent)?;
    if parent.canonicalize()? != parent {
        return Err(Error::Message("The backup storage contains a link."));
    }
    files::create_private_dirs(root)?;
    let meta = fs::symlink_metadata(root)?;
    if !meta.is_dir() || root.canonicalize()? != root {
        return Err(Error::Message("The backup storage must be private."));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no arguments and reports this process's owner.
        if meta.mode() & 0o077 != 0 || meta.uid() != unsafe { libc::geteuid() } {
            return Err(Error::Message("The backup storage must be private."));
        }
    }
    Ok(())
}

fn staging_file(path: &Path) -> Result<File> {
    let mut options = File::options();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() || !fs::symlink_metadata(path)?.is_file() {
        return Err(Error::Message("The backup storage needs attention."));
    }
    Ok(file)
}

pub struct Store<M> {
    to: String,
    setup: String,
    source: Source,
    mail: M,
}

impl<M: Mail> Store<M> {
    pub fn new(to: String, setup: String, source: Source, mail: M) -> Self {
        Self {
            to,
            setup,
            source,
            mail,
        }
    }

    fn ask(&self, method: Method, mut request: Request) -> Result<Value> {
        request.setup.clone_from(&self.setup);
        let reply = self.mail.request(&self.to, method, &self.source, request)?;
        if reply.status != 200 {
            return Err(Error::Detail(
                reply
                    .value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("The receiving computer couldn’t use the backup.")
                    .into(),
            ));
        }
        Ok(reply.value)
    }
}

impl<M: Mail> DestinationStore for Store<M> {
    fn list(&self) -> Result<Vec<PointId>> {
        let value = self.ask(
            Method::Get,
            Request {
                action: Some(Action::List),
                ..Default::default()
            },
        )?;
        serde_json::from_value(value)
            .map_err(|_| Error::Message("The receiving computer returned an invalid list."))
    }

    fn put(&self, id: &PointId, path: &Path) -> Result<()> {
        let mut file = archive::regular(path)?;
        let size = file.metadata()?.len();
        if size > MAX_ARCHIVE {
            return Err(Error::Message(
                "This encrypted backup exceeds the Relay archive limit.",
            ));
        }
        let sha256 = hash(path)?;
        let request = |action, offset, data| Request {
            action: Some(action),
            id: Some(id.clone()),
            size: Some(size),
            sha256: Some(sha256.clone()),
            offset,
            data,
            ..Default::default()
        };
        let result = (|| {
            let begun = self.ask(Method::Post, request(Action::Begin, None, None))?;
            let mut offset = begun
                .get("offset")
                .and_then(Value::as_u64)
                .filter(|offset| *offset <= size)
                .ok_or(Error::Message(
                    "The receiving computer returned an invalid upload offset.",
                ))?;
            file.seek(SeekFrom::Start(offset))?;
            let mut buffer = vec![0; CHUNK];
            while offset < size {
                let bytes = &mut buffer[..(size - offset).min(CHUNK as u64) as usize];
                file.read_exact(bytes)
                    .map_err(|_| Error::Message("The encrypted backup changed while sending."))?;
                let next = self.ask(
                    Method::Post,
                    request(
                        Action::Append,
                        Some(offset),
                        Some(URL_SAFE_NO_PAD.encode(&*bytes)),
                    ),
                )?;
                offset += bytes.len() as u64;
                if next.get("offset").and_then(Value::as_u64) != Some(offset) {
                    return Err(Error::Message(
                        "The receiving computer didn’t confirm the chunk.",
                    ));
                }
            }
            self.ask(Method::Post, request(Action::Commit, None, None))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = self.ask(Method::Post, request(Action::Abort, None, None));
        }
        result
    }

    fn get(&self, id: &PointId, scratch: &Path) -> Result<PathBuf> {
        let mut file = tempfile::NamedTempFile::new_in(scratch)?;
        let mut offset = 0;
        let mut full_size = None;
        loop {
            let reply = self.ask(
                Method::Get,
                Request {
                    action: Some(Action::Read),
                    id: Some(id.clone()),
                    offset: Some(offset),
                    ..Default::default()
                },
            )?;
            let data = reply
                .get("data")
                .and_then(Value::as_str)
                .filter(|data| data.len() <= (CHUNK * 4).div_ceil(3))
                .ok_or(Error::Message("Invalid encrypted backup chunk."))?;
            let invalid = || Error::Message("Incomplete encrypted backup.");
            let size = reply
                .get("size")
                .and_then(Value::as_u64)
                .filter(|size| {
                    (1..=MAX_ARCHIVE).contains(size) && full_size.is_none_or(|known| known == *size)
                })
                .ok_or_else(invalid)?;
            let bytes = URL_SAFE_NO_PAD.decode(data).map_err(|_| invalid())?;
            if bytes.is_empty() || offset + bytes.len() as u64 > size {
                return Err(invalid());
            }
            file.write_all(&bytes)?;
            offset += bytes.len() as u64;
            if offset == size {
                let target = scratch.join("received");
                file.persist_noclobber(&target)
                    .map_err(|error| error.error)?;
                return Ok(target);
            }
            full_size = Some(size);
        }
    }

    fn erase(&self, id: &PointId) -> Result<()> {
        self.ask(
            Method::Delete,
            Request {
                id: Some(id.clone()),
                ..Default::default()
            },
        )?;
        Ok(())
    }
}
