//! Desktop device grants and revocations. The accepted setup owns writes for managed computers;
//! the standalone authority keeps its existing link file and projects current grants to transports.

use std::{
    collections::{BTreeSet, HashMap},
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use kuvo_protocol::{
    json::{Null, Nullable},
    routes::{self, types::*},
};
use kuvo_server::ssh::{Grant, IdentityRefusal, Pairings};
use kuvo_wire::keys::{Identity, decode_exact, encode};
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};
use tokio::sync::{mpsc, oneshot, watch};

use crate::{
    directory::{ComputerKey, Member, Roster},
    kept,
    reach::Millis,
    relay,
    session::{BoxHello, ComputerKind},
};

#[derive(Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum DeviceKind {
    Phone,
    Desktop,
}

#[derive(Clone, Deserialize, Serialize)]
struct Device {
    key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<DeviceKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    added: Option<Number>,
    #[serde(default)]
    pairing: String,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Deserialize, Serialize)]
struct Revocation {
    key: String,
    pairing: String,
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

#[derive(Default, Deserialize, Serialize)]
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

#[derive(Clone)]
struct State {
    devices: Vec<Device>,
    removed: Vec<Revocation>,
}

impl State {
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

#[derive(Clone)]
pub struct Devices {
    current: watch::Receiver<Arc<State>>,
    commands: mpsc::Sender<Command>,
    identity: Option<([u8; 32], ComputerKey)>,
    enabled: bool,
    can_pair: bool,
}

#[derive(Deserialize)]
struct MacConfig {
    phones: bool,
}

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
        let (commands, rx) = mpsc::channel(16);
        let devices = Self {
            current,
            commands,
            identity,
            enabled: store.enabled,
            can_pair: store.enabled && store.authority == Authority::Standalone,
        };
        tokio::spawn(store.run(rx, publish));
        Ok(Bootstrap {
            roster,
            devices,
            relay: config,
        })
    }

    pub fn permits_local_pairing(&self) -> bool {
        self.can_pair
    }

    pub fn grants(&self) -> HashMap<ComputerKey, String> {
        let state = self.current.borrow();
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
        self.current.changed().await
    }

    pub fn list(&self, asking: Option<ComputerKey>) -> routes::get_kuvo_phones::Status200 {
        let state = self.current.borrow();
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

    pub async fn pair(&self, device: Request, now: Millis) -> Result<String, Error> {
        let (answer, reply) = oneshot::channel();
        self.commands
            .send(Command::Pair(device, now, answer))
            .await
            .map_err(|_| Error::Stopped)?;
        reply.await.map_err(|_| Error::Stopped)?
    }

    pub async fn remove(&self, devices: Remove, actor: Option<Actor>) -> Result<(), Error> {
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
    let mut box_keys: BTreeSet<_> = members.iter().map(|member| encode(&member.key)).collect();
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
        _ if link.extra.contains_key("setup") => Authority::Managed,
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
                let state = State {
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
    fn state(&self) -> State {
        State {
            devices: self.link.phones.clone(),
            removed: self.link.removed.clone(),
        }
    }

    async fn run(
        mut self,
        mut commands: mpsc::Receiver<Command>,
        publish: watch::Sender<Arc<State>>,
    ) {
        while let Some(command) = commands.recv().await {
            let published = publish.clone();
            let work = tokio::task::spawn_blocking(move || {
                match command {
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

    fn pair(&mut self, request: Request, now: Millis) -> Result<String, Error> {
        if request.kind != DeviceKind::Desktop
            || canonical_key(&request.key).is_none()
            || self.box_keys.contains(&request.key)
            || request.name.encode_utf16().count() > 100
        {
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
            || request.phones.iter().any(|entry| {
                canonical_key(&entry.key).is_none()
                    || self.box_keys.contains(&entry.key)
                    || entry.pairing.as_ref().is_some_and(|id| {
                        id.len() > 32
                            || !id
                                .bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                    })
            })
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
