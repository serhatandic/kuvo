//! The holder owns one agent socket per registered conversation or folder.
use kuvo_os::timer_fires::{self, Timer};
use std::{
    collections::{HashMap, HashSet},
    fs, io,
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use kuvo_os::watch::{Options, WatchHub};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::{Semaphore, mpsc, watch},
    task::JoinHandle,
    time,
};
use tokio_util::sync::CancellationToken;

use super::{
    Item, Kind, Resident,
    activity::Action,
    broker::Where,
    crypto,
    helper::millis,
    identity::Identity,
    ssh_keys::{Key, put, read_number, read_string},
};

const MAX_PACKET: usize = 256 * 1024;
const IDLE: u64 = 60 * 60_000;
const REFUSED_FOR: u64 = 30_000;
const KEEP_NOTE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

pub(super) struct Agents(JoinHandle<()>);

impl Agents {
    pub(super) async fn start(resident: Arc<Resident>) -> io::Result<Option<Self>> {
        let notes = resident.broker.home.join("run/ssh-for");
        kuvo_store::files::create_private_dirs(&notes)?;
        remove_old_notes(&notes);
        if !resident.broker.gate.agents {
            return Ok(None);
        }
        let sockets = Folder::new(&resident.broker.home, &resident.broker.account)?;
        let watches = WatchHub::default();
        let changes = watches.subscribe(&notes, Options::default()).await?;
        Ok(Some(Self(tokio::spawn(async move {
            let mut agents = HashMap::new();
            let mut changes = Some(changes);
            let mut tick = timer_fires::interval(Timer::VaultSshAgents, Duration::from_secs(1));
            tick.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = tick.tick() => (),
                    found = async { changes.as_mut().expect("watch is present").recv().await }, if changes.is_some() => {
                        if found.is_none() { changes = None; }
                    }
                }
                reconcile(&notes, &sockets, &mut agents, &resident);
                // The registry owns its subscriptions while this task runs.
                let _ = &watches;
            }
        }))))
    }
}

impl Drop for Agents {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct Job {
    folder: PathBuf,
    used: Arc<AtomicU64>,
    handle: JoinHandle<()>,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

fn reconcile(
    notes: &Path,
    sockets: &Folder,
    agents: &mut HashMap<String, Job>,
    resident: &Arc<Resident>,
) {
    let Ok(entries) = fs::read_dir(notes) else {
        return;
    };
    let mut live = HashSet::new();
    for entry in entries.filter_map(std::result::Result::ok) {
        let Ok(note) = entry.file_name().into_string() else {
            continue;
        };
        if !valid_note(&note) {
            continue;
        }
        if agents
            .get(&note)
            .is_some_and(|job| millis().saturating_sub(job.used.load(Ordering::Relaxed)) >= IDLE)
        {
            let _ = fs::remove_file(entry.path());
            continue;
        }
        let Ok(folder) = fs::read_to_string(entry.path()) else {
            continue;
        };
        let folder = PathBuf::from(folder);
        if !folder.is_absolute()
            || note.starts_with("dir-")
                && note != format!("dir-{}", &digest(folder.as_os_str().as_bytes())[..32])
        {
            continue;
        }
        live.insert(note.clone());
        if agents
            .get(&note)
            .is_some_and(|job| job.folder == folder && !job.handle.is_finished())
        {
            continue;
        }
        agents.remove(&note);
        let Ok(listener) = sockets.listen(&note) else {
            continue;
        };
        let source = Arc::new(Source {
            caller: Registration {
                note: note.clone(),
                folder: folder.clone(),
            },
            resident: resident.clone(),
            keys: Mutex::default(),
            decisions: Arc::new(Mutex::default()),
        });
        let used = Arc::new(AtomicU64::new(millis()));
        let handle = tokio::spawn(accept(listener, source, used.clone()));
        agents.insert(
            note,
            Job {
                folder,
                used,
                handle,
            },
        );
    }
    agents.retain(|note, _| live.contains(note));
}

fn valid_note(note: &str) -> bool {
    note.strip_prefix("ses_").is_some_and(|s| {
        (1..=80).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    }) || note.strip_prefix("dir-").is_some_and(|s| {
        s.len() == 32
            && s.bytes()
                .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
    })
}

fn digest(text: &[u8]) -> String {
    hex::encode(Sha256::digest(text))
}

fn remove_old_notes(notes: &Path) {
    for note in fs::read_dir(notes)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(std::result::Result::ok)
    {
        if note.file_type().is_ok_and(|kind| kind.is_file())
            && note
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|time| time.elapsed().ok())
                .is_some_and(|age| age > KEEP_NOTE)
        {
            let _ = fs::remove_file(note.path());
        }
    }
}

struct Folder {
    path: PathBuf,
}

impl Folder {
    fn new(home: &Path, account: &Path) -> io::Result<Self> {
        let own = home.join("run/ssh-agents");
        let limit = if cfg!(target_os = "macos") { 103 } else { 107 };
        let fits = |path: &Path| path.as_os_str().as_bytes().len() + 34 <= limit;
        let path = if !cfg!(target_os = "macos") || fits(&own) {
            own
        } else {
            let id = digest(home.as_os_str().as_bytes());
            let short = account.join(".kuvo-ssh").join(&id[..12]);
            let path = if fits(&short) {
                short
            } else {
                PathBuf::from(format!("/private/tmp/kuvo-ssh-{}", uid())).join(&id[..12])
            };
            private(path.parent().expect("SSH agent parent"))?;
            path
        };
        private(&path)?;
        for entry in fs::read_dir(&path)?.filter_map(std::result::Result::ok) {
            if entry.file_type().is_ok_and(|kind| kind.is_socket()) {
                let _ = fs::remove_file(entry.path());
            }
        }
        Ok(Self { path })
    }

    fn listen(&self, note: &str) -> io::Result<Socket> {
        let prefix = digest(note.as_bytes());
        let random = crypto::random::<9>().map_err(io::Error::other)?;
        let filename = format!("{}{}.sock", &prefix[..16], URL_SAFE_NO_PAD.encode(&*random));
        let path = self.path.join(filename);
        let temporary = self
            .path
            .join(format!(".{}", URL_SAFE_NO_PAD.encode(&*random)));
        let listener = UnixListener::bind(&temporary)?;
        let install = (|| {
            fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
            let meta = fs::symlink_metadata(&temporary)?;
            fs::rename(&temporary, &path)?;
            Ok((meta.dev(), meta.ino()))
        })();
        match install {
            Ok(identity) => Ok(Socket {
                listener,
                path,
                identity,
            }),
            Err(error) => {
                let _ = fs::remove_file(temporary);
                Err(error)
            }
        }
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.path);
    }
}

fn uid() -> u32 {
    // SAFETY: getuid reads the current process's identity and takes no arguments.
    unsafe { libc::getuid() }
}

fn private(path: &Path) -> io::Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => (),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
        Err(error) => return Err(error),
    }
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() || meta.uid() != uid() || meta.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "SSH agents need a private socket folder.",
        ));
    }
    Ok(())
}

struct Socket {
    listener: UnixListener,
    path: PathBuf,
    identity: (u64, u64),
}

impl Drop for Socket {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path)
            .is_ok_and(|meta| (meta.dev(), meta.ino()) == self.identity)
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

async fn accept(socket: Socket, source: Arc<Source>, used: Arc<AtomicU64>) {
    let mut connections = tokio::task::JoinSet::new();
    let limit = Arc::new(Semaphore::new(16));
    loop {
        tokio::select! {
            result = socket.listener.accept() => {
                let Ok((stream, _)) = result else { break };
                if let Ok(permit) = limit.clone().try_acquire_owned() {
                    used.store(millis(), Ordering::Relaxed);
                    let source = source.clone();
                    connections.spawn(async move { let _permit = permit; let _ = connection(stream, source).await; });
                }
            }
            _ = connections.join_next(), if !connections.is_empty() => {},
        }
    }
}

async fn connection(stream: UnixStream, source: Arc<Source>) -> io::Result<()> {
    let (mut reader, mut writer) = stream.into_split();
    let active = Semaphore::new(4);
    let (send, mut queue) = mpsc::channel(4);
    let cancel = CancellationToken::new();
    let reading = async {
        loop {
            let length = reader.read_u32().await? as usize;
            if length == 0 || length > MAX_PACKET {
                return Err(io::ErrorKind::InvalidData.into());
            }
            let mut packet = vec![0; length];
            reader.read_exact(&mut packet).await?;
            let permit = active.try_acquire().map_err(io::Error::other)?;
            send.try_send((packet, permit))
                .map_err(|_| io::ErrorKind::InvalidData)?;
        }
    };
    let writing = async {
        while let Some((packet, _permit)) = queue.recv().await {
            let answer = respond(&source, &packet, &cancel).await;
            writer.write_u32(answer.len() as u32).await?;
            writer.write_all(&answer).await?;
        }
        Ok(())
    };
    let result = tokio::select! { answer = reading => answer, answer = writing => answer };
    cancel.cancel();
    result
}

async fn respond(source: &Arc<Source>, packet: &[u8], cancel: &CancellationToken) -> Vec<u8> {
    match packet[0] {
        11 => {
            let keys = source.keys().await;
            let mut answer = vec![12];
            answer.extend_from_slice(&(keys.len() as u32).to_be_bytes());
            for (item, key) in keys {
                put(&mut answer, &key.blob);
                put(&mut answer, item.name.as_bytes());
            }
            answer
        }
        13 => {
            let mut body = &packet[1..];
            if let Some((blob, data, flags)) = sign_request(&mut body)
                && let Some((item, key)) = source
                    .keys()
                    .await
                    .into_iter()
                    .find(|(_, key)| key.blob == blob)
                && source.allowed(item, cancel).await
                && let Ok(signature) = key.sign(data, flags)
            {
                let mut answer = vec![14];
                put(&mut answer, &signature);
                return answer;
            }
            vec![5]
        }
        _ => vec![5],
    }
}

fn sign_request<'a>(body: &mut &'a [u8]) -> Option<(&'a [u8], &'a [u8], u32)> {
    let blob = read_string(body)?;
    let data = read_string(body)?;
    let flags = if body.is_empty() {
        0
    } else {
        read_number(body)?
    };
    Some((blob, data, flags))
}

struct Registration {
    note: String,
    folder: PathBuf,
}

impl Registration {
    async fn caller(&self, resident: &Resident) -> Option<Identity> {
        if self.note.starts_with("ses_") {
            let identity = resident.broker.reader.caller(&self.note).await;
            let folder = identity.folder.as_ref()?;
            let real = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_owned());
            return (real(folder) == real(&self.folder)).then_some(identity);
        }
        let bot = resident.broker.reader.folder_bot(&self.folder).await;
        let root = bot
            .as_ref()
            .map_or_else(String::new, |bot| bot.session.clone());
        Some(Identity {
            folder: Some(self.folder.clone()),
            bot,
            root,
            ..Identity::default()
        })
    }
}

struct Source {
    caller: Registration,
    resident: Arc<Resident>,
    keys: Mutex<HashMap<String, (u64, Option<Arc<Key>>)>>,
    decisions: Arc<Mutex<Decisions>>,
}

#[derive(Default)]
struct Decisions {
    waiting: HashMap<String, Arc<Decision>>,
    refused: HashMap<String, u64>,
}

struct Decision {
    answer: watch::Sender<Option<bool>>,
    cancel: CancellationToken,
}

struct Waiter {
    state: Arc<Mutex<Decisions>>,
    id: String,
    decision: Arc<Decision>,
    answer: watch::Receiver<Option<bool>>,
}

impl Drop for Waiter {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.lock()
            && self.decision.answer.receiver_count() == 1
            && state
                .waiting
                .get(&self.id)
                .is_some_and(|waiting| Arc::ptr_eq(waiting, &self.decision))
        {
            self.decision.cancel.cancel();
            state.waiting.remove(&self.id);
        }
    }
}

impl Source {
    async fn keys(&self) -> Vec<(Item, Arc<Key>)> {
        let caller = self.caller.caller(&self.resident).await;
        self.resident
            .block(|local| {
                let mut cached = self
                    .keys
                    .lock()
                    .map_err(|_| super::Error::refused(503, "The SSH agent stopped."))?;
                let Some(caller) =
                    caller.filter(|_| self.resident.broker.gate.agents && !local.vault.locked())
                else {
                    cached.clear();
                    return Ok(Vec::new());
                };
                if local
                    .vault
                    .paused_for(
                        caller.bot.as_ref().map(|bot| bot.id.as_str()),
                        Some(&caller.root),
                    )?
                    .is_some()
                {
                    cached.clear();
                    return Ok(Vec::new());
                }
                let mut grants = local.vault.grants()?;
                if !cfg!(target_os = "macos") {
                    grants.retain(|grant| {
                        matches!(grant.who, super::Who::Everyone { everyone: true })
                    });
                }
                let mut keys = Vec::new();
                let items = local.vault.items()?;
                let current: HashSet<_> = items.iter().map(|item| item.id.as_str()).collect();
                cached.retain(|id, _| current.contains(id.as_str()));
                for item in items {
                    if item.kind != Kind::Key
                        || !item.fields.iter().any(|field| field == "private_key")
                        || caller.grant(&item, &grants, false).is_none()
                    {
                        continue;
                    }
                    let slot = cached.entry(item.id.clone()).or_insert((u64::MAX, None));
                    if slot.0 != item.changed {
                        let private = local
                            .vault
                            .values(&item.id)
                            .ok()
                            .and_then(|mut values| values.remove("private_key"));
                        *slot = (
                            item.changed,
                            private
                                .and_then(|private| Key::read(&private).ok())
                                .map(Arc::new),
                        );
                    }
                    if let Some(key) = &slot.1 {
                        keys.push((item, key.clone()));
                    }
                }
                Ok(keys)
            })
            .unwrap_or_default()
    }

    async fn allowed(self: &Arc<Self>, item: Item, cancel: &CancellationToken) -> bool {
        let Some(mut wait) = self.join_decision(item) else {
            return false;
        };
        tokio::select! {
            _ = cancel.cancelled() => false,
            result = wait.answer.wait_for(Option::is_some) => result.ok().and_then(|value| *value).unwrap_or(false),
        }
    }

    fn join_decision(self: &Arc<Self>, item: Item) -> Option<Waiter> {
        let mut state = self.decisions.lock().ok()?;
        let now = millis();
        state.refused.retain(|_, until| *until > now);
        if state.refused.contains_key(&item.id) {
            return None;
        }
        let id = item.id.clone();
        let decision = state
            .waiting
            .entry(id.clone())
            .or_insert_with(|| {
                let decision = Arc::new(Decision {
                    answer: watch::channel(None).0,
                    cancel: CancellationToken::new(),
                });
                let source = self.clone();
                let work = decision.clone();
                tokio::spawn(async move {
                    source.decide(item, work).await;
                });
                decision
            })
            .clone();
        Some(Waiter {
            state: self.decisions.clone(),
            id,
            answer: decision.answer.subscribe(),
            decision,
        })
    }

    async fn decide(&self, item: Item, decision: Arc<Decision>) {
        let answer = self.check(&item, &decision.cancel).await;
        if let Ok(mut state) = self.decisions.lock()
            && state
                .waiting
                .get(&item.id)
                .is_some_and(|entry| Arc::ptr_eq(entry, &decision))
        {
            state.waiting.remove(&item.id);
            if matches!(answer, Ok(false)) && !decision.cancel.is_cancelled() {
                state
                    .refused
                    .insert(item.id.clone(), millis() + REFUSED_FOR);
            }
        }
        decision.answer.send_replace(Some(answer.unwrap_or(false)));
    }

    async fn check(&self, item: &Item, cancel: &CancellationToken) -> super::Result<bool> {
        let Some(mut who) = self.caller.caller(&self.resident).await else {
            return Ok(false);
        };
        if !cfg!(target_os = "macos") {
            // Linux's fence cannot isolate sockets by conversation. Offer only
            // keys granted to everyone and ask without inheriting its identity.
            who = Identity {
                session: who.session,
                autopilot: who.autopilot,
                ..Identity::default()
            };
        }
        let place = Where {
            program: Some("ssh".into()),
            ..Where::default()
        };
        match self
            .resident
            .authorize(item, &who, &place, false, cancel)
            .await?
        {
            Ok(how) => {
                self.resident.block(|local| {
                    local.note_agent(
                        Action::Used,
                        Some(item),
                        &who,
                        &place,
                        Some(how.as_str()),
                        None,
                    )
                })?;
                Ok(true)
            }
            Err(super::broker::UseDenied::Autopilot) => {
                Err(super::Error::refused(403, super::broker::LEFT_UNDONE))
            }
            Err(super::broker::UseDenied::Refused(_)) => Ok(false),
        }
    }
}
