use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    sync::Arc,
    time::Duration,
};

use kuvo_peers::{
    directory::{Member, Roster},
    relay,
    session::{BoxHello, ComputerKind as PeerKind},
};
use kuvo_server::ssh::{Grant, IdentityRefusal, Pairings};
use kuvo_wire::{
    documents::{self, Document, Kind, Parts},
    keys::{decode_exact, encode},
};
use serde_json::{Value, json};
use tokio::{
    sync::{broadcast, mpsc, oneshot, watch},
    task::JoinHandle,
    time::{self, Instant},
};

use crate::{
    Error, checkpoint, edits, installer, lease, mac_host,
    model::{
        ComputerKind, Identity, Link, Membership, Position, Role, Setup, SharedKind, safe_token,
        validate_position, validate_shared,
    },
    object, preferences, random,
    storage::Files,
};

mod activity;
mod adding;
mod device_access;
mod edit_delivery;
mod handover;
mod predecessor;
mod removal;
mod replication;
pub(crate) use removal::Unlink;

pub use device_access::{ManagedDeviceChange, ManagedDeviceGrant, ManagedDeviceSave};
use handover::{Handover, HandoverReply};

pub(crate) const LINK: &str = "run/link.json";
const CARD: &str = "run/setup-card.json";
const WELCOME: &str = "run/welcomed.json";
const ATTACHED: &str = "run/app-attached.json";

pub struct SetupRuntime {
    handle: SetupHandle,
    task: JoinHandle<()>,
    activity_task: JoinHandle<()>,
}

#[derive(Clone)]
pub struct SetupHandle {
    pub(crate) inbox: mpsc::Sender<Request>,
    pub(crate) views: watch::Receiver<View>,
    rosters: watch::Receiver<Option<Arc<Roster>>>,
    activity: activity::Sender,
}

impl SetupHandle {
    pub fn rosters(&self) -> watch::Receiver<Option<Arc<Roster>>> {
        self.rosters.clone()
    }

    pub fn ownership_state(&self) -> kuvo_peers::ownership::SetupState {
        self.views.borrow().ownership.clone()
    }

    pub async fn phone_activity(&self) {
        activity::record(&self.activity, activity::Activity::Seen).await;
    }
}

struct SetupLink {
    sender: relay::Sender,
    events: broadcast::Receiver<Arc<relay::RelayEvent>>,
}

#[derive(Clone, PartialEq)]
struct PairedAccess {
    key: Option<[u8; 32]>,
    public: Option<String>,
    disabled: bool,
    permits_pairing: bool,
    devices: Arc<kuvo_peers::devices::DeviceRoster>,
    grants: HashMap<[u8; 32], Grant>,
}

impl Pairings for SetupHandle {
    fn identity(&self, computer: &str) -> Result<[u8; 32], IdentityRefusal> {
        let view = self.views.borrow();
        let access = &view.access;
        if access.public.as_deref() != Some(computer) {
            return Err(IdentityRefusal::WrongComputer);
        }
        if access.disabled {
            return Err(IdentityRefusal::Disabled);
        }
        access.key.ok_or(IdentityRefusal::Unavailable)
    }

    fn paired(&self, key: &[u8; 32]) -> Option<Grant> {
        self.views.borrow().access.grants.get(key).cloned()
    }
}

#[derive(Clone)]
pub(crate) struct View {
    pub setup: Value,
    pub personalities: Result<Value, String>,
    pub clock: Value,
    access: PairedAccess,
    ownership: kuvo_peers::ownership::SetupState,
}

pub(crate) struct Request {
    pub command: Command,
    pub reply: oneshot::Sender<Result<Response, Error>>,
}

pub(crate) enum Command {
    Welcome(bool),
    InstallerCode(Option<crate::seed::source::Prepared>, edits::Submitter),
    InstallerCancellation(installer::Cancellation),
    SshAdmission(edits::Submitter),
    SshAddress {
        key: String,
        invite: String,
        destination: String,
        identity: String,
        distro: Option<String>,
        by: edits::Submitter,
    },
    BackupMatch {
        source: crate::backup::Source,
        previous: String,
        replacement: String,
    },
    BackupApproval {
        sender: String,
        request: crate::backup::boundary::Approval,
    },
    RenameComputer(String, String, edits::Submitter),
    UnlinkComputer(String, Unlink, bool, edits::Submitter),
    DeviceChange(device_access::Mutation),
    ReadEdit(edits::Target),
    SubmitEdit(edits::Edit, edits::Submitter),
    EditReceipt(String),
    EditOutbox,
    Attached,
    Personalities(preferences::PersonalityChange),
    Zone {
        zone: String,
        revision: Option<String>,
    },
    Handover {
        primary: String,
        epoch: Option<u64>,
        submitter: edits::Submitter,
    },
    Usage {
        id: String,
        epoch: u64,
        sequence: u64,
        operation: Option<String>,
        value: Value,
    },
}

pub(crate) struct Response {
    pub status: u16,
    pub value: Value,
}

impl Response {
    pub(crate) fn ok(value: Value) -> Self {
        Self { status: 200, value }
    }
}

impl SetupRuntime {
    pub async fn prepare(
        home: &Path,
        name: String,
        mac: bool,
        relay: Option<String>,
    ) -> Result<PreparedSetup, Error> {
        let files = Files::new(home.to_owned());
        let kind = if mac {
            ComputerKind::Mac
        } else {
            ComputerKind::Machine
        };
        let test_clock = relay
            .as_deref()
            .and_then(|value| reqwest::Url::parse(value).ok())
            .is_some_and(|url| matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "localhost")));
        let overlay = if mac {
            validate_disk(&files)?;
            mac_host::read(&files, &name, relay.clone()).await?
        } else {
            None
        };
        if let Some(overlay) = &overlay {
            predecessor::convert(&files, overlay, test_clock).await?;
        }
        let (link, identity) = tokio::task::spawn_blocking({
            let (home, files, name) = (home.to_owned(), files.clone(), name.clone());
            move || {
                let (link, identity) = open(&home, &files, &name, kind, relay, overlay)?;
                if kind == ComputerKind::Machine {
                    preferences::follow_carried_zone(
                        &files,
                        &link,
                        std::env::var("KUVO_TZ").ok().as_deref(),
                    )?;
                }
                if let Some(zone) = preferences::zone(&files, &link)? {
                    preferences::apply_zone(&files, &zone)?;
                }
                Ok::<_, Error>((link, identity))
            }
        })
        .await
        .map_err(|_| Error::Invalid("setup startup task"))??;
        let lease = lease::Client::new(&files, &link, test_clock)?;
        let welcomed = read_welcome(&files)?;
        Ok(PreparedSetup {
            files,
            link,
            identity,
            kind,
            name,
            lease,
            welcomed,
        })
    }

    pub fn handle(&self) -> SetupHandle {
        self.handle.clone()
    }
}

pub struct PreparedSetup {
    files: Files,
    link: Link,
    identity: Option<Identity>,
    kind: ComputerKind,
    name: String,
    lease: lease::Client,
    welcomed: bool,
}

impl PreparedSetup {
    pub fn has_mac_identity(&self) -> bool {
        self.kind == ComputerKind::Mac && self.identity.is_some()
    }

    /// The legacy machine link marker uses the resident build identifier and has no newline.
    pub fn record_build(&self, build: &str) -> Result<(), Error> {
        if self.kind == ComputerKind::Machine {
            self.files
                .write_bytes("run/link.version", build.as_bytes())?;
        }
        Ok(())
    }

    pub fn relay_config(&self) -> Result<Option<relay::Config>, Error> {
        self.identity
            .as_ref()
            .zip(self.link.relay.as_ref())
            .filter(|_| self.link.setup().is_ok())
            .map(|(own, url)| relay_config(&self.link, own, url, self.kind))
            .transpose()
    }

    pub fn peer_roster(&self) -> Option<Roster> {
        peer_roster(&self.link, self.identity.as_ref())
    }

    pub fn start(self, relay: Option<relay::RelayHandle>) -> Result<SetupRuntime, Error> {
        let network = relay.and_then(|relay| {
            let events = relay.subscribe()?;
            Some(SetupLink {
                sender: relay.sender,
                events,
            })
        });
        let Self {
            files,
            mut link,
            identity,
            kind,
            name,
            lease,
            welcomed,
        } = self;
        let (tx, rx) = mpsc::channel(32);
        let view = make_view(
            &files,
            &link,
            identity.as_ref(),
            &lease,
            kind,
            &name,
            welcomed,
        )?;
        let (views, view_rx) = watch::channel(view);
        let (rosters, roster_rx) =
            watch::channel(peer_roster(&link, identity.as_ref()).map(Arc::new));
        for field in ["sharedPending", "sharedInterrupted", "sharedReceipts"] {
            link.extra.shift_remove(field);
        }
        let (activity, activity_rx) = mpsc::channel(16);
        let activity_task = tokio::spawn(activity::track(files.clone(), activity_rx));
        let actor = Owner {
            files,
            link,
            identity,
            kind,
            name,
            lease,
            welcomed,
            network,
            rx,
            views,
            rosters,
            exports: vec![],
            parts: Parts::default(),
            manifests: VecDeque::new(),
            pending_tips: VecDeque::new(),
            pending_handover: None,
            unlink_challenge: None,
            pending_devices: HashMap::new(),
            forwarded_handover: None,
            next: Instant::now(),
            next_edits: Instant::now(),
            next_digest: Instant::now(),
            activity: activity.clone(),
        };
        actor.write_card()?;
        let task = tokio::spawn(actor.run());
        Ok(SetupRuntime {
            handle: SetupHandle {
                inbox: tx,
                views: view_rx,
                rosters: roster_rx,
                activity,
            },
            task,
            activity_task,
        })
    }
}

impl Drop for SetupRuntime {
    fn drop(&mut self) {
        self.task.abort();
        self.activity_task.abort();
    }
}

pub(crate) fn validate_disk(files: &Files) -> Result<(), Error> {
    if let Some(layout) = files.read::<Value>("folder-version.json")?
        && (layout
            .get("version")
            .and_then(Value::as_u64)
            .is_none_or(|version| version > 23)
            || layout.get("upgradingTo").is_some())
    {
        return Err(Error::Version("setup folder layout"));
    }
    let link = files.read::<Link>(LINK)?;
    if link.as_ref().is_some_and(|link| link.format_version > 1) {
        return Err(Error::Version("link state"));
    }
    if link
        .as_ref()
        .and_then(|link| link.setup.as_ref())
        .is_some_and(|setup| !setup.aside && (!setup.managed || !setup.checkpointed))
    {
        return Err(Error::Version("unconverted setup"));
    }
    Ok(())
}

fn open(
    home: &Path,
    files: &Files,
    name: &str,
    kind: ComputerKind,
    relay: Option<String>,
    overlay: Option<mac_host::Overlay>,
) -> Result<(Link, Option<Identity>), Error> {
    validate_disk(files)?;
    let mut link = files.read::<Link>(LINK)?.unwrap_or_default();
    let native = overlay.is_some();
    if let Some(overlay) = overlay {
        overlay.apply(&mut link);
    }
    if link.relay.is_none() {
        link.relay = relay
            .filter(|value| {
                reqwest::Url::parse(value).is_ok_and(|url| matches!(url.scheme(), "ws" | "wss"))
            })
            .map(|mut value| {
                if value.ends_with('/') {
                    value.pop();
                }
                value
            });
    }
    if let Some(setup) = &link.setup {
        validate_position(&setup.position)?;
        if kind == ComputerKind::Mac && !native {
            return Ok((link, None));
        }
        let identity = Identity::read(&link)?;
        if !setup.aside {
            validate_shared(&link.shared)?;
            let members = link.member_list()?;
            if !members.boxes.iter().any(|peer| {
                peer.key == identity.public && peer.sign.as_deref() == Some(&identity.sign)
            }) || (setup.role == Role::Primary) != (setup.position.primary == identity.public)
            {
                return Err(Error::Invalid("local setup membership"));
            }
            if let Some(envelope) = link.leadership.get(&setup.position.epoch) {
                let doc = documents::verify(envelope.clone())?;
                let cert = checkpoint::certificate(&doc)?;
                if cert.setup != setup.position || encode(&doc.origin) != setup.sign {
                    return Err(Error::Invalid("stored leadership"));
                }
                checkpoint::resolve(files, &doc, &cert)?;
            } else if setup.role == Role::Primary
                || !setup
                    .extra
                    .get("leadershipHash")
                    .and_then(Value::as_str)
                    .is_some_and(|h| safe_token(h, 11, 11))
            {
                return Err(Error::Invalid("missing stored leadership"));
            }
        }
        if remember_link_key(&mut link, &identity, kind) {
            write_link(files, &link, kind)?;
        }
        return Ok((link, Some(identity)));
    }

    if (kind == ComputerKind::Mac && !native)
        || [
            "localUnlink",
            "localWipe",
            "removalSource",
            "macRotation",
            "mac",
        ]
        .iter()
        .any(|key| link.extra.get(*key).is_some_and(|v| !v.is_null()))
        || link
            .extra
            .get("macs")
            .and_then(Value::as_array)
            .is_some_and(|macs| !macs.is_empty())
        || installer_pending(&link)
        || home
            .parent()
            .is_some_and(|parent| parent.join(".kuvo-code-adding").symlink_metadata().is_ok())
    {
        return Ok((link, None));
    }
    if link.key.is_none() {
        link.key = Some(encode(&random::<32>()?));
    }
    if link.sign.is_none() {
        link.sign = Some(encode(&random::<32>()?));
    }
    if link.secret.is_none() {
        link.secret = Some(encode(&random::<32>()?));
    }
    link.name.get_or_insert_with(|| name.into());
    let own = Identity::read(&link)?;
    bootstrap(files, &mut link, &own, kind)?;
    write_link(files, &link, kind)?;
    Ok((link, Some(own)))
}

fn bootstrap(
    files: &Files,
    link: &mut Link,
    own: &Identity,
    kind: ComputerKind,
) -> Result<(), Error> {
    let position = Position {
        id: format!("set_{}", encode(&random::<16>()?)),
        epoch: 1,
        primary: own.public.clone(),
    };
    let roster = json!({
        "boxes": [own.self_peer(kind, link.name.as_deref().unwrap_or_default())], "left": [], "moved": [],
        "devices": { "phones": link.extra.get("phones").cloned().unwrap_or_else(|| json!([])), "removed": [] },
        "removalRevisions": {},
    });
    link.shared.clear();
    link.shared_outcomes.clear();
    link.leadership.clear();
    link.shared_setup = Some(position.id.clone());
    link.shared.insert(
        SharedKind::MembershipAndAccess,
        crate::model::Entry {
            sequence: 1,
            value: roster,
            offline_edits: None,
        },
    );
    let cert = checkpoint::build(files, own, position.clone(), None, 1, &link)?;
    link.leadership.insert(1, cert.envelope);
    link.setup = Some(new_setup(position, own.sign.clone(), Role::Primary));
    link.format_version = 1;
    link.extra.entry("phones").or_insert_with(|| json!([]));
    remember_link_key(link, own, kind);
    Ok(())
}

pub(crate) fn remember_link_key(link: &mut Link, own: &Identity, kind: ComputerKind) -> bool {
    if link
        .extra
        .get("macRotation")
        .is_some_and(|value| !value.is_null())
        || kind == ComputerKind::Machine
            && (link
                .extra
                .get("mac")
                .and_then(Value::as_str)
                .is_some_and(|mac| !mac.is_empty())
                || link
                    .extra
                    .get("macs")
                    .and_then(Value::as_array)
                    .is_some_and(|macs| !macs.is_empty()))
    {
        return false;
    }
    let former = link.extra.get("keyNow").and_then(Value::as_str);
    if former == Some(&own.public) {
        return false;
    }
    if let Some(former) = former.filter(|key| !key.is_empty()) {
        let mut before = link
            .extra
            .get("keysBefore")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        before.retain(|key| key != &own.public && key != former);
        before.push(json!(former));
        before.drain(..before.len().saturating_sub(8));
        link.extra.insert("keysBefore".into(), json!(before));
    }
    link.extra.insert("keyNow".into(), json!(own.public));
    true
}

pub(crate) fn write_link(files: &Files, link: &Link, kind: ComputerKind) -> Result<(), Error> {
    if kind == ComputerKind::Mac {
        let mut kept = link.clone();
        kept.key = None;
        kept.secret = None;
        kept.sign = None;
        kept.name = None;
        kept.relay = None;
        files.write(LINK, &kept)
    } else {
        files.write(LINK, link)
    }
}

pub(crate) fn new_setup(position: Position, sign: String, role: Role) -> Setup {
    Setup {
        position,
        sign,
        role,
        aside: false,
        managed: true,
        checkpointed: true,
        extra: Default::default(),
    }
}

fn peer_roster(link: &Link, identity: Option<&Identity>) -> Option<Roster> {
    let own = identity?;
    let setup = link.setup().ok()?;
    let roster = link.member_list().ok()?;
    let named = link.value(SharedKind::SetupNames);
    let mut name = link.name.clone().unwrap_or_default();
    let mut members = Vec::new();
    for peer in roster.boxes {
        let title = named
            .and_then(|names| names.get(&peer.key))
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .unwrap_or(&peer.name)
            .to_owned();
        if peer.key == own.public {
            name = title;
        } else if let Some(key) = decode_exact(&peer.key) {
            members.push(Member {
                key,
                address: decode_exact(&peer.address),
                name: title,
                kind: if peer.kind == ComputerKind::Mac {
                    PeerKind::Mac
                } else {
                    PeerKind::Machine
                },
            });
        }
    }
    Some(Roster {
        here: decode_exact(&own.public),
        name,
        main: decode_exact(&setup.position.primary),
        members,
    })
}

fn relay_config(
    link: &Link,
    identity: &Identity,
    url: &str,
    kind: ComputerKind,
) -> Result<relay::Config, Error> {
    let roster = link.member_list()?;
    let grants = if link.devices_disabled {
        HashMap::new()
    } else {
        link.grants(&roster, &identity.public)
    };
    Ok(relay::Config {
        url: url.into(),
        secret: identity.secret,
        key: identity.key,
        peers: roster.hosts(&identity.public).collect(),
        grants: grants
            .into_iter()
            .map(|(key, grant)| (key, grant.id))
            .collect(),
        hello: BoxHello {
            v: 1,
            kind: match kind {
                ComputerKind::Mac => kuvo_peers::session::ComputerKind::Mac,
                ComputerKind::Machine => kuvo_peers::session::ComputerKind::Machine,
            },
            name: link.name.clone().unwrap_or_default(),
            app: env!("CARGO_PKG_VERSION").into(),
            engine: None,
        },
    })
}

fn read_welcome(files: &Files) -> Result<bool, Error> {
    versioned_marker(files, WELCOME)
}

fn versioned_marker(files: &Files, path: &str) -> Result<bool, Error> {
    let entry = files.read::<Value>(path)?;
    if entry
        .as_ref()
        .and_then(|entry| entry.get("formatVersion"))
        .is_some_and(|v| !matches!(v.as_f64(), Some(0.0 | 1.0)))
    {
        return Err(Error::Version("setup marker"));
    }
    Ok(entry.is_some())
}

fn installer_pending(link: &Link) -> bool {
    link.extra.get("installerClaim").is_some_and(|claim| {
        let adopted = claim.get("adopted").and_then(Value::as_bool) == Some(true);
        let active = claim
            .get("until")
            .and_then(Value::as_u64)
            .is_some_and(|until| until > lease::wall());
        let already_member = link.setup().is_ok_and(|setup| {
            setup.role == Role::Member
                && claim.get("setup").and_then(Value::as_str) == Some(&setup.position.id)
        });
        active && !adopted && !already_member
    })
}

fn make_view(
    files: &Files,
    link: &Link,
    own: Option<&Identity>,
    lease: &lease::Client,
    kind: ComputerKind,
    name: &str,
    welcomed: bool,
) -> Result<View, Error> {
    let setup = link.setup().ok().filter(|_| own.is_some());
    let mut view = json!({
        "setup": null, "epoch": null, "primary": null,
        "you": { "key": own.map(|own| &own.public), "role": null },
        "computers": [], "heldBy": [], "welcomed": welcomed,
        "startsFromOthers": {
            "value": link.extra.get("startsFromOthers").and_then(Value::as_str).filter(|value| matches!(*value, "on" | "off")),
            "default": if setup.map_or(kind == ComputerKind::Mac, |s| s.role == Role::Primary) { "off" } else { "on" },
        },
    });
    if let Some(setup) = setup {
        let mut roster = link.member_list()?.boxes;
        roster.sort_by_key(|peer| peer.key != setup.position.primary);
        let mut computers = Vec::new();
        for peer in roster.into_iter().take(65) {
            let name = link
                .value(SharedKind::SetupNames)
                .and_then(|names| names.get(&peer.key))
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .unwrap_or(&peer.name);
            let primary = peer.key == setup.position.primary;
            if primary {
                view["primary"] = json!({"key": peer.key, "name": name, "kind": peer.kind});
            }
            computers.push(json!({
                "key": peer.key, "name": name, "kind": peer.kind,
                "role": if primary { Role::Primary } else { Role::Member }, "app": peer.kind == ComputerKind::Mac,
            }));
        }
        view["setup"] = json!(setup.position.id);
        view["epoch"] = json!(setup.position.epoch);
        view["you"]["role"] = json!(setup.role);
        view["computers"] = json!(computers);
        if let Some(conflict) = setup.extra.get("conflict") {
            view["conflict"] = conflict.clone();
        }
        if lease
            .seen
            .as_ref()
            .is_none_or(|l| l.holder == setup.position.primary)
        {
            let warning = if setup.role == Role::Primary && !lease.holds(link) {
                Some(Error::Fenced.to_string())
            } else if lease.seen.as_ref().is_some_and(|l| l.relay_host) {
                Some("The main computer also runs Kuvo's relay. If this computer goes offline, another computer can't take over.".into())
            } else {
                None
            };
            if let Some(warning) = warning {
                view["leadershipWarning"] = json!(warning);
            }
        }
    }
    let access = match (setup, own) {
        (Some(_), Some(own)) => {
            let roster = link.member_list()?;
            let disabled = link.devices_disabled || link.disabled(&roster, &own.public);
            PairedAccess {
                key: Some(own.key),
                public: Some(own.public.clone()),
                disabled,
                permits_pairing: !disabled && link.relay.is_some(),
                devices: Arc::new(device_access::device_roster(link, &roster, &own.public)),
                grants: if link.devices_disabled {
                    HashMap::new()
                } else {
                    link.grants(&roster, &own.public)
                },
            }
        }
        _ => PairedAccess {
            key: None,
            public: None,
            disabled: true,
            permits_pairing: false,
            devices: Arc::default(),
            grants: HashMap::new(),
        },
    };
    let ownership = if let Some(setup) = setup {
        let roster = link.member_list()?;
        let mut replacements = std::collections::BTreeMap::new();
        for removed in &roster.left {
            let name = format!("replacement:{removed}");
            let replacement = link
                .value(SharedKind::OwnershipRecovery)
                .and_then(|entries| entries.get(&name))
                .or_else(|| {
                    link.value(SharedKind::OwnershipAssignments)
                        .and_then(|entries| entries.get(&name))
                });
            if let (Some(old), Some(next)) = (
                decode_exact(removed),
                replacement
                    .and_then(|row| row["computer"].as_str())
                    .and_then(decode_exact),
            ) {
                replacements.insert(old, next);
            }
        }
        kuvo_peers::ownership::SetupState {
            id: Some(setup.position.id.clone()),
            replacements,
            grants_disabled: access.disabled,
            // After restart, a member waits for this process to observe the
            // current setup holder instead of authorizing from a stale lease.
            grants_current: setup.role == Role::Primary || (welcomed && lease.holds(link)),
        }
    } else {
        kuvo_peers::ownership::SetupState::default()
    };
    Ok(View {
        setup: view,
        access,
        ownership,
        personalities: match preferences::library(files, link) {
            Ok(library) => Ok(library.document()),
            Err(Error::PersonalityVersion(version)) => Err(version),
            Err(error) => return Err(error),
        },
        clock: preferences::clock(files, link, name)?,
    })
}

struct Owner {
    files: Files,
    link: Link,
    identity: Option<Identity>,
    kind: ComputerKind,
    name: String,
    lease: lease::Client,
    welcomed: bool,
    network: Option<SetupLink>,
    rx: mpsc::Receiver<Request>,
    views: watch::Sender<View>,
    rosters: watch::Sender<Option<Arc<Roster>>>,
    exports: Vec<(Kind, documents::Envelope)>,
    parts: Parts,
    manifests: VecDeque<(String, Document)>,
    pending_tips: VecDeque<Document>,
    pending_handover: Option<Handover>,
    unlink_challenge: Option<removal::Challenge>,
    pending_devices: HashMap<String, device_access::Pending>,
    forwarded_handover: Option<Handover>,
    next: Instant,
    next_edits: Instant,
    next_digest: Instant,
    activity: activity::Sender,
}

impl Owner {
    async fn run(mut self) {
        loop {
            let until = self.next.min(self.next_edits);
            let event = self.network.as_mut().map(|relay| &mut relay.events);
            tokio::select! {
                command = self.rx.recv() => {
                    let Some(request) = command else { return; };
                    self.request(request).await;
                }
                incoming = async {
                    match event {
                        Some(events) => events.recv().await,
                        None => std::future::pending().await,
                    }
                } => match incoming {
                    Ok(event) => {
                        if let Err(error) = self.receive(&event).await {
                            eprintln!("setup relay message: {error}");
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // The lease and the next availability round recover missed state.
                        self.next = Instant::now();
                    }
                    Err(broadcast::error::RecvError::Closed) => self.network = None,
                },
                () = time::sleep_until(until) => {
                    if self.next <= Instant::now() {
                        self.next = Instant::now() + Duration::from_secs(2);
                        if let Err(error) = self.resume_local_unlink().await {
                            eprintln!("setup unlink cleanup: {error}");
                        }
                        if let Err(error) = self.offer_unlinks().await {
                            eprintln!("setup removal delivery: {error}");
                        }
                        if let Err(error) = self.tick().await
                            && !matches!(error, Error::Relay(_) | Error::Conflict) {
                            eprintln!("setup heartbeat: {error}");
                        }
                    }
                    if self.next_edits <= Instant::now() {
                        self.next_edits = Instant::now() + Duration::from_secs(5);
                        if let Err(error) = self.flush_edits().await {
                            eprintln!("setup edit delivery: {error}");
                        }
                    }
                    self.expire_handover().await;
                    self.expire_device_changes();
                    self.refresh();
                }
            }
        }
    }

    fn refresh(&self) {
        if self.kind == ComputerKind::Machine
            && let Ok(Some(zone)) = preferences::follow_carried_zone(&self.files, &self.link, None)
            && let Err(error) = preferences::apply_zone(&self.files, &zone)
        {
            eprintln!("following the time zone from the Mac kit: {error}");
        }
        if let Ok(view) = make_view(
            &self.files,
            &self.link,
            self.identity.as_ref(),
            &self.lease,
            self.kind,
            &self.name,
            self.welcomed,
        ) {
            self.views.send_if_modified(|old| {
                if old.setup == view.setup
                    && old.personalities == view.personalities
                    && old.clock == view.clock
                    && old.access == view.access
                {
                    return false;
                }
                *old = view;
                true
            });
        }
    }

    fn write_card(&self) -> Result<(), Error> {
        versioned_marker(&self.files, CARD)?;
        let view = &self.views.borrow().setup;
        let own = view["you"]["key"].as_str();
        let self_peer = view["computers"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|peer| peer["key"].as_str() == own);
        let primary = view["primary"].as_object().map(|primary| {
            let heard = self
                .link
                .extra
                .get("macSeen")
                .and_then(|seen| seen.get(view["primary"]["key"].as_str().unwrap_or_default()));
            json!({"key": primary["key"], "name": primary["name"], "heard": heard})
        });
        let card = json!({
            "formatVersion": 1, "setup": view["setup"], "epoch": view["epoch"], "role": view["you"]["role"],
            "primary": primary, "self": { "key": own, "name": self_peer.and_then(|p| p["name"].as_str()).or(self.link.name.as_deref()).unwrap_or_default(), "kind": self.kind },
            "app": self.kind == ComputerKind::Mac || self.files.path(ATTACHED).is_file(),
            "computers": view["computers"].as_array().map_or(0, Vec::len), "items": 0,
            "used": self.link.shared_outcomes.values().next().is_some(), "heldBy": [],
            "macs": if view["you"]["role"] == "member" { primary.into_iter().map(|p| json!({"key": p["key"], "name": p["name"]})).collect::<Vec<_>>() } else { vec![] },
        });
        if self.files.read::<Value>(CARD)?.as_ref() != Some(&card) {
            self.files.write(CARD, &card)?;
        }
        Ok(())
    }

    fn roster(&self) -> Result<Membership, Error> {
        self.link.member_list()
    }

    fn own(&self) -> Result<&Identity, Error> {
        self.identity.as_ref().ok_or(Error::Conflict)
    }

    fn can_write(&self) -> Result<(), Error> {
        let setup = self.link.setup()?;
        let own = self.own()?;
        let roster = self.roster()?;
        if self.link.disabled(&roster, &own.public)
            || setup.role != Role::Primary
            || setup.position.primary != own.public
            || setup.sign != own.sign
        {
            return Err(Error::Conflict);
        }
        if installer_pending(&self.link)
            || ["macRotation", "setupMerge", "localWipe"]
                .iter()
                .any(|field| self.link.extra.get(*field).is_some_and(|v| !v.is_null()))
            || (roster.boxes.len() > 1 && !self.lease.holds(&self.link))
        {
            return Err(Error::Fenced);
        }
        Ok(())
    }

    async fn commit(&mut self, mut next: Link) -> Result<(), Error> {
        next.format_version = 1;
        let membership_changed = self
            .link
            .shared
            .get(&SharedKind::MembershipAndAccess)
            .map(|entry| entry.sequence)
            != next
                .shared
                .get(&SharedKind::MembershipAndAccess)
                .map(|entry| entry.sequence);
        let position_changed = self.link.setup().ok().map(|setup| &setup.position)
            != next.setup().ok().map(|setup| &setup.position);
        if (membership_changed || position_changed)
            && let (Ok(roster), Some(own)) = (next.member_list(), &self.identity)
        {
            project_membership(&mut next, &roster, own)?;
        }
        if let Some(own) = &self.identity {
            remember_link_key(&mut next, own, self.kind);
        }
        let names_changed = self
            .link
            .shared
            .get(&SharedKind::SetupNames)
            .map(|entry| entry.sequence)
            != next
                .shared
                .get(&SharedKind::SetupNames)
                .map(|entry| entry.sequence);
        let files = self.files.clone();
        let zone_changed = self
            .link
            .shared
            .get(&SharedKind::SetupTimeZone)
            .map(|entry| &entry.value)
            != next
                .shared
                .get(&SharedKind::SetupTimeZone)
                .map(|entry| &entry.value);
        let kind = self.kind;
        let last_device_gone = membership_changed
            && self
                .link
                .member_list()
                .is_ok_and(|roster| roster.devices().next().is_some())
            && next
                .member_list()
                .is_ok_and(|roster| roster.devices().next().is_none());
        let written = tokio::task::spawn_blocking(move || {
            write_link(&files, &next, kind)?;
            if zone_changed
                && let Err(error) = preferences::zone(&files, &next).and_then(|zone| {
                    zone.as_ref()
                        .map_or(Ok(()), |zone| preferences::apply_zone(&files, zone))
                })
            {
                eprintln!("applying accepted time zone: {error}");
            }
            Ok::<_, Error>(next)
        })
        .await
        .map_err(|_| Error::Invalid("setup write task"))??;
        self.link = written;
        if last_device_gone {
            activity::record(&self.activity, activity::Activity::Forget).await;
        }
        if position_changed || membership_changed || names_changed {
            self.rosters
                .send_replace(peer_roster(&self.link, self.identity.as_ref()).map(Arc::new));
        }
        if let (Some(network), Some(own), Ok(roster)) =
            (&self.network, &self.identity, self.roster())
        {
            let grants = if self.link.devices_disabled {
                HashMap::new()
            } else {
                self.link
                    .grants(&roster, &own.public)
                    .into_iter()
                    .map(|(key, grant)| (key, grant.id))
                    .collect()
            };
            let _ = network
                .sender
                .trust_applied(grants, roster.hosts(&own.public).collect())
                .await;
        }
        self.refresh();
        if let Err(error) = self.write_card() {
            eprintln!("setup card: {error}");
        }
        Ok(())
    }

    async fn request(&mut self, request: Request) {
        let Request { command, reply } = request;
        if let Command::Handover {
            primary,
            epoch,
            submitter,
        } = command
        {
            self.begin_handover(primary, epoch, submitter, HandoverReply::Local(reply))
                .await;
            return;
        }
        if let Command::DeviceChange(mutation) = command {
            self.ask_device_change(mutation, reply).await;
            return;
        }
        let result = self.execute(command).await;
        let _ = reply.send(result);
    }

    async fn execute(&mut self, command: Command) -> Result<Response, Error> {
        match command {
            Command::BackupMatch {
                source,
                previous,
                replacement,
            } => {
                self.can_write()?;
                let own = &self.own()?.public;
                match crate::backup::boundary::match_replacement(
                    &self.link,
                    own,
                    &source,
                    &previous,
                    &replacement,
                ) {
                    Ok(Some(value)) => {
                        let receipt = self
                            .save(SharedKind::OwnershipRecovery, value, None, None)
                            .await?;
                        Ok(Response::ok(json!({"matched": true, "save": receipt})))
                    }
                    Ok(None) => Ok(Response::ok(json!({"matched": true}))),
                    Err(error) => Ok(Response {
                        status: 409,
                        value: json!({"message": error.to_string()}),
                    }),
                }
            }
            Command::BackupApproval { sender, request } => {
                let refused = || Response {
                    status: 409,
                    value: json!({"message": "The main computer has no current match for this replacement."}),
                };
                if self.can_write().is_err() {
                    return Ok(refused());
                }
                Ok(
                    match crate::backup::boundary::proof(&self.link, &sender, &request) {
                        Ok(boundary) => Response::ok(json!({"boundary": boundary})),
                        Err(_) => refused(),
                    },
                )
            }
            Command::InstallerCode(credentials, by) => {
                self.issue_installer_code(credentials, by).await
            }
            Command::InstallerCancellation(receipt) => self.cancel_installer(receipt).await,
            Command::SshAdmission(by) => self.ssh_admission(&by),
            Command::SshAddress {
                key,
                invite,
                destination,
                identity,
                distro,
                by,
            } => {
                self.keep_ssh_address(key, invite, destination, identity, distro, by)
                    .await
            }
            Command::UnlinkComputer(key, request, explicit, by) => {
                self.unlink_computer(key, request, explicit, by).await
            }
            Command::RenameComputer(key, name, by) => self.rename_computer(key, name, by).await,
            Command::ReadEdit(target) => {
                let setup = self.link.setup()?;
                let stale = setup.role == Role::Member && !self.lease.holds(&self.link);
                Ok(Response::ok(edits::view(
                    &self.files,
                    &self.link,
                    target,
                    stale,
                )?))
            }
            Command::SubmitEdit(edit, submitter) => {
                if !submitter.current(&self.link) {
                    return Ok(Response {
                        status: 403,
                        value: json!({"message": "Open this from a paired Kuvo Code client."}),
                    });
                }
                let mut outgoing = edits::Outgoing::stage(
                    &self.files,
                    &self.link,
                    &self.own()?.public,
                    edit,
                    submitter,
                )?;
                self.deliver_edit(&mut outgoing).await?;
                Ok(outgoing.answer())
            }
            Command::EditReceipt(operation) => {
                let setup = &self.link.setup()?.position;
                let record = edits::read_outgoing(&self.files, &operation)?
                    .filter(|record| record.setup == setup.id);
                match record {
                    Some(record) => Ok(Response::ok(
                        json!({"receipt": record.receipt, "recovery": record.recovery}),
                    )),
                    None => Ok(Response {
                        status: 404,
                        value: json!({"message": "There is no such edit on this computer."}),
                    }),
                }
            }
            Command::EditOutbox => {
                let setup = &self.link.setup()?.position;
                let entries: Vec<_> = edits::rows(&self.files)?.into_iter()
                    .filter(|row| row.setup == setup.id && (row.queued_edit.is_some() || !row.recovery.is_empty()))
                    .map(|row| json!({"ref": row.reference, "owner": row.owner, "receipt": row.receipt})).collect();
                Ok(Response::ok(json!({"entries": entries})))
            }
            Command::Welcome(value) => {
                if value != self.welcomed {
                    if value {
                        self.files
                            .write(WELCOME, &json!({"formatVersion": 1, "at": lease::wall()}))?;
                    } else {
                        self.files.remove(WELCOME)?;
                    }
                    self.welcomed = value;
                    self.refresh();
                }
                Ok(Response::ok(json!({"welcomed": value})))
            }
            Command::Attached => {
                if !self.files.path(ATTACHED).exists() {
                    self.files.write(ATTACHED, &json!({"at": lease::wall()}))?;
                    self.write_card()?;
                }
                Ok(Response::ok(json!({})))
            }
            Command::Personalities(change) => {
                let before = preferences::library(&self.files, &self.link)?;
                let managed = self.link.setup().is_ok();
                let (after, response) = before.change(change, managed);
                if response.status != 200 {
                    return Ok(response);
                }
                if !managed {
                    preferences::keep_local_library(&self.files, &after)?;
                    self.refresh();
                    return Ok(response);
                }
                let receipt = self
                    .save(SharedKind::SetupPersonalities, after.shared(), None, None)
                    .await?;
                Ok(Response::ok(with_receipt(response.value, receipt)))
            }
            Command::Zone { zone, revision } => {
                let page = preferences::clock(&self.files, &self.link, &self.name)?;
                let row = &page["groups"][0]["rows"][0];
                if revision
                    .as_ref()
                    .is_some_and(|revision| row["revision"].as_str() != Some(revision))
                {
                    return Ok(Response {
                        status: 409,
                        value: json!({"message": "It changed since you read it.", "page": page}),
                    });
                }
                if !row["choices"]
                    .as_array()
                    .is_some_and(|choices| choices.iter().any(|choice| choice["id"] == zone))
                {
                    return Ok(Response {
                        status: 400,
                        value: json!({"message": "Pick one of its choices."}),
                    });
                }
                let before = preferences::zone(&self.files, &self.link)?;
                if zone == "own-clock"
                    || before
                        .as_ref()
                        .is_some_and(|before| before.zone == zone && !before.carried)
                {
                    return Ok(Response::ok(page));
                }
                if self
                    .link
                    .setup()
                    .is_ok_and(|setup| setup.role == Role::Member)
                {
                    return Ok(Response {
                        status: 409,
                        value: json!({"message": "Change your setup's time zone on its main computer."}),
                    });
                }
                let choice = preferences::Zone::choose(&zone, before.as_ref())?;
                if self.link.setup.is_none() {
                    preferences::keep_local_zone(&self.files, &choice)?;
                    preferences::apply_zone(&self.files, &choice)?;
                    self.refresh();
                    return Ok(Response::ok(preferences::clock(
                        &self.files,
                        &self.link,
                        &self.name,
                    )?));
                }
                let receipt = self
                    .save(SharedKind::SetupTimeZone, json!(choice), None, None)
                    .await?;
                Ok(Response::ok(with_receipt(
                    preferences::clock(&self.files, &self.link, &self.name)?,
                    receipt,
                )))
            }
            Command::Usage {
                id,
                epoch,
                sequence,
                operation,
                value,
            } => {
                let setup = self.link.setup()?;
                if setup.position.id != id || setup.position.epoch != epoch {
                    return Err(Error::Conflict);
                }
                if value.get("units").and_then(Value::as_object).is_none()
                    || value.get("imported").and_then(Value::as_bool).is_none()
                {
                    return Err(Error::Invalid("usage display settings"));
                }
                let receipt = self
                    .save(
                        SharedKind::SetupUsageDisplay,
                        value,
                        Some(sequence),
                        operation,
                    )
                    .await?;
                Ok(Response::ok(json!(receipt)))
            }
            Command::Handover { .. } | Command::DeviceChange(_) => {
                unreachable!("this request can wait on relay mail")
            }
        }
    }

    async fn save(
        &mut self,
        kind: SharedKind,
        value: Value,
        expected: Option<u64>,
        operation: Option<String>,
    ) -> Result<crate::Receipt, Error> {
        if let Some(operation) = operation.as_deref()
            && let Some(receipt) =
                self.link
                    .retry(self.own()?, kind, operation, expected, &value)?
        {
            return Ok(receipt);
        }
        self.can_write()?;
        let mut next = self.link.clone();
        let receipt = next.put(self.own()?, kind, value, expected, operation)?;
        self.commit(next).await?;
        if let Err(error) = self.publish(false).await {
            eprintln!("publishing saved setup values: {error}");
        }
        Ok(receipt)
    }
}

pub(crate) fn project_membership(
    link: &mut Link,
    roster: &Membership,
    own: &Identity,
) -> Result<(), Error> {
    let setup = link.setup()?.clone();
    let primary = &setup.position.primary;
    let here = setup.role == Role::Primary;
    if here {
        for key in ["mac", "macNames", "macSeen", "macSigns"] {
            link.extra.shift_remove(key);
        }
        link.extra.insert("macs".into(), json!([]));
        link.extra.insert(
            "macPeers".into(),
            json!(
                roster
                    .boxes
                    .iter()
                    .filter(|peer| peer.key != own.public)
                    .collect::<Vec<_>>()
            ),
        );
        link.extra.insert(
            "left".into(),
            json!(roster.left.iter().rev().take(64).rev().collect::<Vec<_>>()),
        );
        link.extra.insert("removedHere".into(), json!(roster.left));
        for (field, portable) in [
            ("macMoved", "moved"),
            ("removalRevisions", "removalRevisions"),
            ("memberRemovals", "removals"),
            ("wipePreparations", "wipePreparations"),
            ("mergedComputers", "mergedComputers"),
        ] {
            if let Some(value) = roster.extra.get(portable) {
                link.extra.insert(field.into(), value.clone());
            } else {
                link.extra.shift_remove(field);
            }
        }
        let recoveries = crate::installer::recoveries(link, roster);
        if recoveries.is_empty() {
            link.extra.shift_remove("installerRecoveries");
        } else {
            link.extra
                .insert("installerRecoveries".into(), Value::Object(recoveries));
        }
    } else {
        let name = roster
            .boxes
            .iter()
            .find(|entry| &entry.key == primary)
            .map(|peer| &peer.name);
        link.extra.insert("mac".into(), json!(primary));
        link.extra.insert("macs".into(), json!([primary]));
        link.extra
            .insert("macNames".into(), json!({(primary): name}));
        link.extra
            .insert("macSigns".into(), json!({(primary): setup.sign}));
        for field in ["macPeers", "macMoved", "left", "removedHere"] {
            link.extra.shift_remove(field);
        }
    }
    let grants = link.grants(roster, &own.public);
    let mut projected = Vec::new();
    for device in roster.devices() {
        let Some(key) = device
            .get("key")
            .and_then(Value::as_str)
            .and_then(decode_exact::<32>)
        else {
            continue;
        };
        if let Some(grant) = grants.get(&key)
            && device
                .get("pairing")
                .and_then(Value::as_str)
                .unwrap_or_default()
                == grant.id
        {
            let old = link
                .extra
                .get("phones")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find(|old| old.get("key") == device.get("key"));
            let mut row = json!({
                "key": encode(&key), "name": grant.name, "pairing": grant.id,
                "from": if here { "pair" } else { "mac" },
                "added": old.and_then(|row| row.get("added")).and_then(Value::as_u64).unwrap_or_else(|| lease::wall() / 1000),
            });
            if let Some(address) = old
                .and_then(|row| row.get("address"))
                .or_else(|| device.get("address"))
            {
                row["address"] = address.clone();
            }
            if device.get("kind").and_then(Value::as_str) == Some("desktop") {
                row["kind"] = json!("desktop");
            }
            if !here {
                row["mac"] = json!(primary);
            }
            projected.push(row);
        }
    }
    link.extra.insert("phones".into(), json!(projected));
    if let Some(removed) = roster
        .extra
        .get("devices")
        .and_then(|devices| devices.get("removed"))
    {
        link.extra.insert("removed".into(), removed.clone());
    }
    if let Some(history) = roster
        .extra
        .get("computerHistory")
        .filter(|history| history.get("setup").and_then(Value::as_str) == Some(&setup.position.id))
    {
        link.extra.insert("computerHistory".into(), history.clone());
    } else {
        link.extra.shift_remove("computerHistory");
    }
    Ok(())
}

fn with_receipt(mut value: Value, receipt: crate::Receipt) -> Value {
    if let Some(fields) = value.as_object_mut() {
        fields.extend(object(json!(receipt)));
    }
    value
}
