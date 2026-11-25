//! Desktop device grants and revocations. The accepted setup owns writes for managed computers;
//! the standalone authority keeps its existing link file and projects current grants to transports.

use std::{
    collections::{BTreeSet, HashMap},
    future::Future,
    io,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use kuvo_protocol::{
    json::{Null, Nullable},
    routes::{self, types::*},
};
use kuvo_server::{
    Caller,
    ssh::{Grant, IdentityRefusal, Pairings},
};
use kuvo_wire::keys::{Identity, decode_exact, encode};
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};
use tokio::sync::{mpsc, oneshot, watch};

mod legacy;

pub use legacy::{Mac as ReleasedMac, Writer as RecordWriter};

/// Who keeps the machine records of a computer with this link outside a
/// setup; `static_key` is the link's own private key.
pub fn record_writer(fields: &serde_json::Map<String, Value>, static_key: Option<&str>) -> RecordWriter {
    let here = static_key
        .and_then(decode_exact)
        .map(|private| Identity::from_bytes(private, [0; 32]).public());
    legacy::writer(fields, here)
}

use crate::{
    directory::{ComputerKey, Member, Roster},
    kept,
    reach::Millis,
    relay,
    session::{BoxHello, ComputerKind},
};

#[derive(Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    Phone,
    Desktop,
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct Device {
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<DeviceKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added: Option<Number>,
    #[serde(default)]
    pub pairing: String,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct Revocation {
    pub key: String,
    pub pairing: String,
}

#[derive(Clone, Deserialize)]
struct Setup {
    id: String,
    primary: String,
    #[serde(default)]
    aside: bool,
    #[serde(default)]
    managed: bool,
    #[serde(default)]
    checkpointed: bool,
}

#[derive(Deserialize)]
struct SavedMember {
    key: String,
    address: Option<String>,
    kind: ComputerKind,
    name: String,
}

#[derive(Deserialize)]
struct Membership {
    boxes: Vec<SavedMember>,
    #[serde(default)]
    left: BTreeSet<String>,
}

#[derive(Deserialize)]
struct Accepted<T> {
    value: T,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Shared {
    membership_and_access: Option<Accepted<Membership>>,
    computer_names: Option<Accepted<HashMap<String, String>>>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
struct Link {
    #[serde(rename = "formatVersion", skip_serializing_if = "Option::is_none")]
    version: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    relay: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    secret: Option<String>,
    #[serde(default)]
    phones: Vec<Device>,
    #[serde(default)]
    removed: Vec<Revocation>,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Default, PartialEq)]
pub struct DeviceRoster {
    pub devices: Vec<Device>,
    pub removed: Vec<Revocation>,
}

impl DeviceRoster {
    fn device(&self, key: &str) -> Option<&Device> {
        self.devices.iter().find(|device| {
            device.key == key
                && !self
                    .removed
                    .iter()
                    .any(|removed| removed.key == key && removed.pairing == device.pairing)
        })
    }
}

/// A released beta.1 Mac's roster as it stands: the Macs that enrolled this machine and the
/// computers they listed. Empty in a setup.
#[derive(Clone, Default)]
pub struct LegacyRoster {
    pub macs: Vec<ComputerKey>,
    pub members: Vec<Member>,
}

#[derive(Clone)]
pub struct Devices {
    current: watch::Receiver<Arc<DeviceRoster>>,
    legacy: watch::Receiver<Arc<LegacyRoster>>,
    managed: Option<Arc<dyn ManagedDevices>>,
    commands: mpsc::Sender<Command>,
    identity: Option<([u8; 32], ComputerKey)>,
    enabled: bool,
    can_pair: bool,
}

#[derive(Deserialize)]
struct MacConfig {
    phones: bool,
}

/// Signed setup adapter. The setup owner publishes the accepted roster and writes grants.
pub trait ManagedDevices: Pairings {
    fn device_roster(&self) -> Arc<DeviceRoster>;
    fn permits_pairing(&self) -> bool;
    fn change<'a>(&'a self, caller: &'a Caller, change: DeviceChange) -> DeviceFuture<'a>;
}

pub enum DeviceChange {
    PairDesktop { key: String, name: String },
    Revoke(Vec<Revocation>),
}

pub type DeviceFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<String>, Error>> + Send + 'a>>;

pub struct Bootstrap {
    pub roster: Roster,
    pub devices: Devices,
    pub relay: Option<relay::Config>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Authority {
    Standalone,
    Managed,
    Aside,
    Unconfigured,
}

#[derive(Deserialize)]
pub struct Request {
    key: String,
    name: String,
    kind: DeviceKind,
}

#[derive(Clone)]
pub struct Actor {
    pub key: ComputerKey,
    pub pairing: String,
}

#[derive(Deserialize)]
pub struct Remove {
    pub phones: Vec<Removal>,
}

#[derive(Deserialize)]
pub struct Removal {
    key: String,
    pairing: Option<String>,
}

impl Request {
    fn valid(&self) -> bool {
        self.kind == DeviceKind::Desktop
            && canonical_key(&self.key).is_some()
            && self.name.encode_utf16().count() <= 100
    }
}

impl Removal {
    fn valid(&self) -> bool {
        canonical_key(&self.key).is_some()
            && self.pairing.as_ref().is_none_or(|id| {
                id.len() <= 32
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "Send the app's key (key), its name (name, up to 100 characters) and what it is (kind: desktop)."
    )]
    InvalidDevice,
    #[error("This app was removed from your devices.")]
    Removed,
    #[error(
        "You have as many paired devices as Kuvo Code keeps. Remove one you no longer use, then try again."
    )]
    Full,
    #[error("This machine has no relay to pair through.")]
    NoRelay,
    #[error("“Let my devices connect” is off on this Mac.")]
    Disabled,
    #[error("The main computer could not save the access change.")]
    Managed,
    #[error("{0}")]
    Authority(String),
    #[error("{0}")]
    WaitingForMain(String),
    #[error("Send devices from your setup to remove.")]
    InvalidRemoval,
    #[error("Saving device access: {0}")]
    Save(#[from] io::Error),
    #[error("The device authority stopped.")]
    Stopped,
}

enum Command {
    Pair(Request, Millis, oneshot::Sender<Result<String, Error>>),
    Remove(Remove, Option<Actor>, oneshot::Sender<Result<(), Error>>),
    LegacyMail(
        ComputerKey,
        serde_json::Map<String, Value>,
        Millis,
        oneshot::Sender<io::Result<Option<Vec<Member>>>>,
    ),
    LeaveLegacy(ComputerKey, oneshot::Sender<io::Result<bool>>),
}

struct Store {
    path: PathBuf,
    link: Link,
    authority: Authority,
    configured: bool,
    enabled: bool,
    box_keys: BTreeSet<String>,
}

impl Devices {
    pub async fn load(
        home: &Path,
        fallback_name: String,
        kind: ComputerKind,
    ) -> io::Result<Bootstrap> {
        let home = home.to_path_buf();
        let (store, roster, identity, config) =
            tokio::task::spawn_blocking(move || load(&home, fallback_name, kind)).await??;
        let (publish, current) = watch::channel(Arc::new(store.state()));
        let (legacy_publish, legacy) = watch::channel(Arc::new(store.legacy_roster()));
        let (commands, rx) = mpsc::channel(16);
        let devices = Self {
            current,
            legacy,
            managed: None,
            commands,
            identity,
            enabled: store.enabled,
            can_pair: store.enabled && store.authority == Authority::Standalone,
        };
        tokio::spawn(store.run(rx, publish, legacy_publish));
        Ok(Bootstrap {
            roster,
            devices,
            relay: config,
        })
    }

    pub fn with_managed(mut self, managed: Arc<dyn ManagedDevices>) -> Self {
        self.managed = Some(managed);
        self
    }

    pub(crate) async fn legacy_mail(
        &self,
        mac: ComputerKey,
        payload: &serde_json::Map<String, Value>,
        now: Millis,
    ) -> io::Result<Option<Vec<Member>>> {
        if self.managed.is_some() || !payload.contains_key("peers") {
            return Ok(None);
        }
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Command::LegacyMail(mac, payload.clone(), now, reply))
            .await
            .map_err(io::Error::other)?;
        result.await.map_err(io::Error::other)?
    }

    pub fn legacy_roster(&self) -> Arc<LegacyRoster> {
        self.legacy.borrow().clone()
    }

    /// Follows the released Macs' roster as it changes.
    pub fn legacy_watch(&self) -> watch::Receiver<Arc<LegacyRoster>> {
        self.legacy.clone()
    }

    /// Lets a released beta.1 Mac go from this machine's list; false when it isn't one.
    pub async fn leave_legacy(&self, mac: ComputerKey) -> io::Result<bool> {
        if self.managed.is_some() {
            return Ok(false);
        }
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Command::LeaveLegacy(mac, reply))
            .await
            .map_err(io::Error::other)?;
        result.await.map_err(io::Error::other)?
    }

    pub fn permits_local_pairing(&self) -> bool {
        self.managed
            .as_ref()
            .map_or(self.can_pair, |managed| managed.permits_pairing())
    }

    fn state(&self) -> Arc<DeviceRoster> {
        self.managed.as_ref().map_or_else(
            || self.current.borrow().clone(),
            |managed| managed.device_roster(),
        )
    }

    pub(crate) fn forwarded_pairing(
        &self,
        key: &ComputerKey,
        pairing: &str,
        grants_current: bool,
    ) -> crate::ownership::PairingState {
        use crate::ownership::PairingState;
        if !self.enabled && self.managed.is_none() {
            return PairingState::Revoked;
        }
        let state = self.state();
        let key = encode(key);
        if state
            .removed
            .iter()
            .any(|grant| grant.key == key && grant.pairing == pairing)
        {
            return PairingState::Revoked;
        }
        if self.managed.is_some() && !grants_current {
            return PairingState::Checking;
        }
        match state.device(&key) {
            Some(current) if current.pairing == pairing => PairingState::Allowed,
            _ => PairingState::Revoked,
        }
    }

    pub fn grants(&self) -> HashMap<ComputerKey, String> {
        let state = self.state();
        state
            .devices
            .iter()
            .filter_map(|device| {
                let current = state.device(&device.key)?;
                (current.kind == Some(DeviceKind::Desktop)).then_some(())?;
                Some((decode_exact(&device.key)?, current.pairing.clone()))
            })
            .collect()
    }

    pub async fn changed(&mut self) -> Result<(), watch::error::RecvError> {
        if self.managed.is_some() {
            // The signed setup owner updates relay trust with the membership it accepted.
            return std::future::pending().await;
        }
        self.current.changed().await
    }

    pub fn list(&self, asking: Option<ComputerKey>) -> routes::get_kuvo_phones::Status200 {
        let state = self.state();
        let asking = asking.map(|key| encode(&key));
        let phones = state
            .devices
            .iter()
            .filter(|device| state.device(&device.key).is_some())
            .map(|device| GetKuvoPhonesStatus200PhonesItem {
                key: device.key.clone(),
                name: device.name.clone().unwrap_or_default(),
                added: device
                    .added
                    .clone()
                    .map_or(Nullable::Null(Null), Nullable::Value),
                pairing: device.pairing.clone(),
                you: asking.as_ref() == Some(&device.key),
                kind: if device.kind == Some(DeviceKind::Desktop) {
                    GetKuvoPhonesStatus200PhonesItemKind3::Desktop
                } else {
                    GetKuvoPhonesStatus200PhonesItemKind3::Phone
                },
            })
            .collect();
        routes::get_kuvo_phones::Status200 { phones }
    }

    pub async fn pair(
        &self,
        device: Request,
        now: Millis,
        caller: &Caller,
    ) -> Result<String, Error> {
        if let Some(managed) = &self.managed {
            if !device.valid() {
                return Err(Error::InvalidDevice);
            }
            let roster = managed.device_roster();
            if roster.device(&device.key).is_none()
                && roster.removed.iter().any(|entry| entry.key == device.key)
            {
                return Err(Error::Removed);
            }
            return managed
                .change(
                    caller,
                    DeviceChange::PairDesktop {
                        key: device.key,
                        name: plain_name(&device.name),
                    },
                )
                .await?
                .ok_or(Error::Managed);
        }
        let (answer, reply) = oneshot::channel();
        self.commands
            .send(Command::Pair(device, now, answer))
            .await
            .map_err(|_| Error::Stopped)?;
        reply.await.map_err(|_| Error::Stopped)?
    }

    pub async fn remove(
        &self,
        devices: Remove,
        actor: Option<Actor>,
        caller: &Caller,
    ) -> Result<(), Error> {
        if let Some(managed) = &self.managed {
            let max = if actor.is_some() { 16 } else { 100 };
            if devices.phones.is_empty()
                || devices.phones.len() > max
                || !devices.phones.iter().all(Removal::valid)
            {
                return Err(Error::InvalidRemoval);
            }
            let state = managed.device_roster();
            let grants = devices
                .phones
                .into_iter()
                .map(|device| {
                    let pairing = device.pairing.unwrap_or_else(|| {
                        state
                            .device(&device.key)
                            .map_or_else(String::new, |active| active.pairing.clone())
                    });
                    Revocation {
                        key: device.key,
                        pairing,
                    }
                })
                .collect();
            managed.change(caller, DeviceChange::Revoke(grants)).await?;
            return Ok(());
        }
        let (answer, reply) = oneshot::channel();
        self.commands
            .send(Command::Remove(devices, actor, answer))
            .await
            .map_err(|_| Error::Stopped)?;
        reply.await.map_err(|_| Error::Stopped)?
    }
}

impl Pairings for Devices {
    fn identity(&self, computer: &str) -> Result<[u8; 32], IdentityRefusal> {
        if let Some(managed) = &self.managed {
            return managed.identity(computer);
        }
        if !self.enabled {
            return Err(IdentityRefusal::Disabled);
        }
        let (key, public) = self.identity.ok_or(IdentityRefusal::WrongComputer)?;
        if decode_exact(computer) != Some(public) {
            return Err(IdentityRefusal::WrongComputer);
        }
        Ok(key)
    }

    fn paired(&self, key: &ComputerKey) -> Option<Grant> {
        if let Some(managed) = &self.managed {
            return managed.paired(key);
        }
        if !self.enabled {
            return None;
        }
        let state = self.current.borrow();
        let device = state.device(&encode(key))?;
        if device.kind != Some(DeviceKind::Desktop) {
            return None;
        }
        Some(Grant {
            id: device.pairing.clone(),
            name: device.name.clone().unwrap_or_default(),
        })
    }
}

type IdentityKeys = ([u8; 32], ComputerKey);
type Loaded = (Store, Roster, Option<IdentityKeys>, Option<relay::Config>);

fn load(home: &Path, fallback_name: String, kind: ComputerKind) -> io::Result<Loaded> {
    let path = home.join("run/link.json");
    let mut link: Link = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Link::default(),
        Err(error) => return Err(error),
    };
    if link.version.is_some_and(|version| version > 1) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Unsupported run/link.json format version",
        ));
    }
    let setup = link
        .extra
        .get("setup")
        .and_then(|value| serde_json::from_value::<Setup>(value.clone()).ok());
    let shared: Option<Shared> = link
        .extra
        .get("shared")
        .and_then(|value| serde_json::from_value(value.clone()).ok());
    let checkpoint = setup.as_ref().is_some_and(|setup| {
        !setup.aside
            && setup.checkpointed
            && link.extra.get("sharedSetup").and_then(Value::as_str) == Some(&setup.id)
    });
    let key: Option<[u8; 32]> = link.key.as_deref().and_then(decode_exact);
    let secret: Option<[u8; 32]> = link.secret.as_deref().and_then(decode_exact);
    let here = key.map(|private| Identity::from_bytes(private, [0; 32]).public());
    let identity = key.zip(here);
    let mut name = link
        .name
        .clone()
        .filter(|name| !name.is_empty())
        .unwrap_or(fallback_name);
    let main = setup
        .as_ref()
        .and_then(|setup| decode_exact(&setup.primary));
    let mut members = vec![];
    if checkpoint
        && let Some(shared) = shared
        && let Some(membership) = shared.membership_and_access
    {
        let names = shared
            .computer_names
            .map(|accepted| accepted.value)
            .unwrap_or_default();
        for member in membership.value.boxes.into_iter().take(64) {
            let Some(key) = decode_exact(&member.key) else {
                continue;
            };
            let title = names
                .get(&member.key)
                .filter(|name| !name.is_empty())
                .cloned()
                .unwrap_or(member.name);
            if Some(key) == here {
                name = title;
                continue;
            }
            if membership.value.left.contains(&member.key) {
                continue;
            }
            members.push(Member {
                key,
                address: member.address.as_deref().and_then(decode_exact),
                kind: member.kind,
                name: title,
            });
        }
    }
    if setup.is_none() {
        members = legacy::members(&link.extra, here);
    }
    let enrollers = legacy::enrollers(&link.extra);
    let mut box_keys: BTreeSet<_> = members.iter().map(|member| encode(&member.key)).collect();
    box_keys.extend(enrollers.iter().map(|key| encode(key)));
    box_keys.extend(here.map(|key| encode(&key)));
    box_keys.extend(main.map(|key| encode(&key)));
    let roster = Roster {
        here,
        name: name.clone(),
        main,
        members,
    };
    let configured = link.relay.as_ref().is_some_and(|url| !url.is_empty())
        && secret.is_some()
        && identity.is_some();
    let authority = match setup {
        Some(setup) if setup.aside => Authority::Aside,
        Some(setup) if setup.managed || main != here => Authority::Managed,
        _ if link.extra.contains_key("setup") || !enrollers.is_empty() => Authority::Managed,
        _ if configured => Authority::Standalone,
        _ => Authority::Unconfigured,
    };
    if matches!(authority, Authority::Aside) {
        link.phones.clear();
    }
    // Mac setup supplies its own device switch and identity. Loading a projected link alone
    // cannot authorize an SSH pairing or bootstrap while that switch is unavailable.
    let enabled = authority != Authority::Aside
        && (kind != ComputerKind::Mac
            || kept::read::<MacConfig>(&home.join("run/mac-link.json"))
                .is_some_and(|config| config.phones));
    let config = link
        .relay
        .clone()
        .filter(|url| !url.is_empty())
        .zip(secret)
        .zip(key)
        .map(|((url, secret), key)| {
            let grants = if enabled {
                let state = DeviceRoster {
                    devices: link.phones.clone(),
                    removed: link.removed.clone(),
                };
                state
                    .devices
                    .iter()
                    .filter_map(|device| {
                        (state.device(&device.key)?.kind == Some(DeviceKind::Desktop))
                            .then_some(())?;
                        Some((decode_exact(&device.key)?, device.pairing.clone()))
                    })
                    .collect()
            } else {
                HashMap::new()
            };
            relay::Config {
                url,
                secret,
                key,
                hello: BoxHello {
                    v: 1,
                    kind,
                    name,
                    app: String::new(),
                    engine: None,
                },
                grants,
                legacy_enrollers: enrollers,
                peers: roster
                    .members
                    .iter()
                    .filter_map(|member| member.host())
                    .collect(),
            }
        });
    Ok((
        Store {
            path,
            link,
            authority,
            configured,
            enabled,
            box_keys,
        },
        roster,
        identity,
        config,
    ))
}

impl Store {
    fn state(&self) -> DeviceRoster {
        DeviceRoster {
            devices: self.link.phones.clone(),
            removed: self.link.removed.clone(),
        }
    }

    async fn run(
        mut self,
        mut commands: mpsc::Receiver<Command>,
        publish: watch::Sender<Arc<DeviceRoster>>,
        legacy: watch::Sender<Arc<LegacyRoster>>,
    ) {
        let legacy = Arc::new(legacy);
        while let Some(command) = commands.recv().await {
            let published = publish.clone();
            let legacy = legacy.clone();
            let work = tokio::task::spawn_blocking(move || {
                match command {
                    Command::LegacyMail(mac, payload, now, answer) => {
                        let result = self.take_legacy(mac, payload, now);
                        if matches!(result, Ok(Some(_))) {
                            legacy.send_replace(Arc::new(self.legacy_roster()));
                        }
                        let _ = answer.send(result);
                    }
                    Command::LeaveLegacy(mac, answer) => {
                        let result = self.leave_legacy(mac);
                        if matches!(result, Ok(true)) {
                            legacy.send_replace(Arc::new(self.legacy_roster()));
                        }
                        let _ = answer.send(result);
                    }
                    Command::Pair(device, now, answer) => {
                        let result = self.pair(device, now);
                        if result.is_ok() {
                            published.send_replace(Arc::new(self.state()));
                        }
                        let _ = answer.send(result);
                    }
                    Command::Remove(devices, actor, answer) => {
                        let result = self.remove(devices, actor);
                        if result.is_ok() {
                            published.send_replace(Arc::new(self.state()));
                        }
                        let _ = answer.send(result);
                    }
                }
                self
            });
            match work.await {
                Ok(store) => self = store,
                Err(error) => {
                    eprintln!("Saving device access: {error}");
                    return;
                }
            }
        }
    }

    fn here(&self) -> Option<ComputerKey> {
        self.link
            .key
            .as_deref()
            .and_then(decode_exact)
            .map(|private| Identity::from_bytes(private, [0; 32]).public())
    }

    fn legacy_roster(&self) -> LegacyRoster {
        LegacyRoster {
            macs: legacy::enrollers(&self.link.extra),
            members: legacy::members(&self.link.extra, self.here()),
        }
    }

    fn leave_legacy(&mut self, mac: ComputerKey) -> io::Result<bool> {
        let mut next = self.link.clone();
        if !legacy::leave(&mut next.extra, mac) {
            return Ok(false);
        }
        next.version = Some(1);
        kept::write_durable(&self.path, &next)?;
        self.link = next;
        let roster = self.legacy_roster();
        self.box_keys = roster
            .members
            .iter()
            .map(|member| &member.key)
            .chain(&roster.macs)
            .chain(self.here().as_ref())
            .map(|key| encode(key))
            .collect();
        Ok(true)
    }

    fn take_legacy(
        &mut self,
        mac: ComputerKey,
        payload: serde_json::Map<String, Value>,
        now: Millis,
    ) -> io::Result<Option<Vec<Member>>> {
        let mut next = self.link.clone();
        if !legacy::take(&mut next.extra, mac, &payload, now) {
            return Ok(None);
        }
        next.version = Some(1);
        kept::write_durable(&self.path, &next)?;
        let here = next
            .key
            .as_deref()
            .and_then(decode_exact)
            .map(|private| Identity::from_bytes(private, [0; 32]).public());
        let members = legacy::members(&next.extra, here);
        self.box_keys = members
            .iter()
            .map(|member| encode(&member.key))
            .chain(legacy::enrollers(&next.extra).iter().map(|key| encode(key)))
            .chain(here.map(|key| encode(&key)))
            .collect();
        self.link = next;
        Ok(Some(members))
    }

    fn pair(&mut self, request: Request, now: Millis) -> Result<String, Error> {
        if !request.valid() || self.box_keys.contains(&request.key) {
            return Err(Error::InvalidDevice);
        }
        if !self.enabled {
            return Err(Error::Disabled);
        }
        let state = self.state();
        if let Some(current) = state.device(&request.key) {
            return Ok(current.pairing.clone());
        }
        if state.removed.iter().any(|entry| entry.key == request.key) {
            return Err(Error::Removed);
        }
        if self.authority == Authority::Managed {
            return Err(Error::Managed);
        }
        if !self.configured {
            return Err(Error::NoRelay);
        }
        if self.link.phones.len() >= 64 {
            return Err(Error::Full);
        }
        let mut bytes = [0; 12];
        getrandom::fill(&mut bytes).map_err(|error| io::Error::other(error.to_string()))?;
        let pairing = encode(&bytes);
        let mut devices = state.devices;
        devices.push(Device {
            key: request.key,
            name: Some(plain_name(&request.name)),
            pairing: pairing.clone(),
            kind: Some(request.kind),
            added: Some(now.div_euclid(1000).into()),
            extra: serde_json::Map::from_iter([("from".into(), Value::from("pair"))]),
        });
        self.save(devices, state.removed)?;
        Ok(pairing)
    }

    fn remove(&mut self, request: Remove, actor: Option<Actor>) -> Result<(), Error> {
        let state = self.state();
        let paired = actor.as_ref().is_none_or(|actor| {
            state
                .device(&encode(&actor.key))
                .is_some_and(|device| device.pairing == actor.pairing)
        });
        let maximum = if actor.is_some() { 16 } else { 256 };
        if !paired
            || request.phones.is_empty()
            || request.phones.len() > maximum
            || request
                .phones
                .iter()
                .any(|entry| !entry.valid() || self.box_keys.contains(&entry.key))
        {
            return Err(Error::InvalidRemoval);
        }
        if matches!(self.authority, Authority::Managed | Authority::Aside) {
            return Err(Error::Managed);
        }
        let mut devices = state.devices;
        let mut removed = state.removed;
        for entry in request.phones {
            let current = devices.iter().position(|device| device.key == entry.key);
            let pairing = entry.pairing.unwrap_or_else(|| {
                current
                    .map(|index| devices[index].pairing.clone())
                    .unwrap_or_default()
            });
            if current.is_some_and(|index| devices[index].pairing == pairing) {
                devices.remove(current.expect("matched current device"));
            }
            if !removed
                .iter()
                .any(|old| old.key == entry.key && old.pairing == pairing)
            {
                removed.push(Revocation {
                    key: entry.key,
                    pairing,
                });
            }
        }
        removed.drain(..removed.len().saturating_sub(256));
        self.save(devices, removed).map_err(Error::from)
    }

    fn save(&mut self, devices: Vec<Device>, removed: Vec<Revocation>) -> io::Result<()> {
        let mut next = serde_json::to_value(&self.link)?;
        next["formatVersion"] = 1.into();
        next["phones"] = serde_json::to_value(&devices)?;
        next["removed"] = serde_json::to_value(&removed)?;
        kept::write_durable(&self.path, &next)?;
        self.link.version = Some(1);
        self.link.phones = devices;
        self.link.removed = removed;
        Ok(())
    }
}

fn canonical_key(key: &str) -> Option<ComputerKey> {
    let decoded = decode_exact(key)?;
    (encode(&decoded) == key).then_some(decoded)
}

fn plain_name(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
