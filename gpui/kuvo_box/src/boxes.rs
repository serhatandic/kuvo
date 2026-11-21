//! Your boxes as one list (PROTOCOL.md, "Computers", "Paired phones"): every computer of your
//! setup, each with a [`BoxClient`] of its own. No box is this client's own. The one on the same
//! computer is only the nearest: reached over its local server, because that's faster and works
//! offline. Every other is reached directly, over the link, as a paired device ([`crate::link`]).
//!
//! - The list is a box's `kuvo/computers`, followed on the nearest box (or, with none here, on
//!   the first that answers). The client connects only at the key and address a box it already
//!   trusts gave it.
//! - It pairs by asking the nearest box (`POST kuvo/devices/pair`): that box is a member of your
//!   setup and vouches for the app on its computer to the main computer. Nothing is pasted or
//!   scanned. What the box refuses is kept in its words.
//! - The last list is kept (public facts only), so the app opens with your computers while the
//!   nearest box is still starting.
//!
//! No GPUI here: any client reuses all of it.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::areas::{Computer, Computers, FleetUpdateSummary};
use crate::client::BoxClient;
use crate::connections::{ComputerTransport, ConnectionMethod, ConnectionPreference, destinations, valid_destination};
use crate::error::{Error, Result};
use crate::follow::{Follow, Followed, Timing as FollowTiming};
use crate::gate::Gate;
use crate::link::{self, Dial, Host, Identity, Keeper, LinkBox, LinkClient};
use crate::setup::SetupOffer;
use crate::ssh::SshBox;
use crate::transport::{Closer, Request, Response, Stream, Transport};

/// How one of your computers stands with this client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reach {
    /// The box on this computer.
    Nearest,
    /// Not asked yet, or being asked now.
    Connecting,
    Connected,
    /// Its box is older than this client needs (PROTOCOL.md, "The update gate"): it waits for its
    /// update, which the box that set it up brings (its row's `update`, `computers.update`).
    Held { missing: Vec<String> },
    /// It doesn't answer: asleep, off, or offline. Why, in words.
    Away(String),
    /// It turned this device away: not paired there (yet), or removed.
    NotPaired,
    /// This computer is checking a pairing it already knows.
    Waiting(String),
    Readd(crate::Refusal),
    /// The list doesn't say where it is (its Kuvo Code is older, or it's known only by another
    /// computer's word): it can't be reached from here.
    Unreached,
}

/// Where this device stands with your setup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pairing {
    /// Nothing asked yet (or nothing to pair with: no other computer).
    NotAsked,
    /// Your computers take this device.
    Paired,
    /// The nearest box couldn't say yet (its main computer isn't answering): asked again later.
    Waiting(String),
    /// The box is refreshing the known pairing.
    Checking(String),
    /// Only the user's Pair Again action starts a new pairing.
    Readd(crate::Refusal),
    /// Refused, in the box's words (the switch is off or the box is older).
    Refused(String),
    /// This device was removed from your devices: only your word pairs it again
    /// ([`Boxes::pair_again`]).
    Removed(String),
}

/// One of your computers, as the list says it, with how this client stands with it.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub computer: Computer,
    pub reach: Reach,
}

impl Row {
    pub fn key(&self) -> &str {
        &self.computer.key
    }

    /// Whether its sessions can be opened from here now.
    pub fn usable(&self) -> bool {
        matches!(self.reach, Reach::Nearest | Reach::Connected | Reach::Connecting | Reach::Held { .. } | Reach::Waiting(_))
    }
}

/// How long it waits.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub follow: FollowTiming,
    pub link: link::Timing,
    /// A computer that stopped answering is said to be away only after this long (PROTOCOL.md,
    /// "When a client says a computer isn't connected": 10 s, then a check).
    pub away_after: Duration,
    /// How long before the nearest box is asked to pair again after it couldn't say.
    pub pair_again: Duration,
    /// With no box to follow the list on, how often the kept computers are tried.
    pub look_again: Duration,
}

impl Default for Timing {
    fn default() -> Timing {
        Timing { follow: FollowTiming::default(), link: link::Timing::default(), away_after: Duration::from_secs(10), pair_again: Duration::from_secs(60), look_again: Duration::from_secs(30) }
    }
}

/// What a [`Boxes`] is made with.
pub struct Options {
    /// The box on this computer, where there is one.
    pub nearest: Option<Arc<dyn Transport>>,
    /// Where this device's key is kept.
    pub keeper: Arc<dyn Keeper>,
    /// How the relay is reached.
    pub dial: Arc<dyn Dial>,
    /// What this device is called among your devices, and the app's version.
    pub name: String,
    pub app: String,
    /// Where the last list is kept (a file in the app's own folder); none keeps nothing.
    pub kept: Option<PathBuf>,
    pub timing: Timing,
}

/// What's kept between runs: public facts only.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Kept {
    /// The device key your computers took (its public half).
    #[serde(default)]
    paired: Option<String>,
    #[serde(default)]
    computers: Vec<KeptComputer>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    connections: HashMap<String, ConnectionPreference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dismissed_update: Option<DismissedUpdate>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct DismissedUpdate {
    target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target_sha256: Option<String>,
    failures: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct KeptComputer {
    key: String,
    name: String,
    kind: String,
    address: Option<String>,
    relay: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    ssh: Vec<String>,
}

#[derive(PartialEq, Eq)]
struct ConnectionFacts {
    address: Option<String>,
    relay: Option<String>,
    method: ConnectionMethod,
    ssh: Vec<String>,
}

/// How one computer is reached: over the link (its relay connection and where it is there), or as
/// the nearest box's own transport says (a stand-in's peers).
struct Linked {
    client: BoxClient,
    link: Option<(LinkClient, Host)>,
    /// The address and relay it was made for: made anew when the list says another.
    at: ConnectionFacts,
    selected: Option<Arc<ComputerTransport>>,
    /// Which client this is, counted: what holds an older one sees it's been replaced.
    made: u64,
}

impl Linked {
    /// This client no longer talks to that box: its session ends, told.
    fn close(&self) {
        if let Some((link, host)) = &self.link {
            link.close(host);
        }
    }
}

#[derive(Default)]
struct State {
    /// The list as last taken, and whether the nearest box said it.
    list: Vec<Computer>,
    rows: Vec<Row>,
    fleet_update: Option<FleetUpdateSummary>,
    nearest_key: Option<String>,
    nearest_has_fleet: Option<bool>,
    linked: HashMap<String, Linked>,
    /// One connection per relay.
    relays: HashMap<String, LinkClient>,
    reach: HashMap<String, Reach>,
    identity: Option<Identity>,
    kept: Kept,
    pairing: Option<Pairing>,
    asked: Option<Instant>,
    /// When each computer first stopped answering, and when each was last asked.
    failing: HashMap<String, Instant>,
    checked: HashMap<String, Instant>,
    stopped: bool,
    started: bool,
    made: u64,
    /// Whether the nearest box was asked again since a computer last turned this device away.
    asked_again: bool,
    /// Which box the list is followed on (`""`: the nearest), and the follow.
    source: Option<(String, u64)>,
    follow: Option<Arc<Follow<Followed<Computers>>>>,
}

struct Inner {
    raw: Option<Arc<dyn Transport>>,
    nearest: Option<BoxClient>,
    keeper: Arc<dyn Keeper>,
    dial: Arc<dyn Dial>,
    name: String,
    app: String,
    kept: Option<PathBuf>,
    timing: Timing,
    state: Mutex<State>,
    woken: Condvar,
    changed: (async_channel::Sender<()>, async_channel::Receiver<()>),
}

/// Your boxes. Cheap to clone: clones share them.
#[derive(Clone)]
pub struct Boxes {
    inner: Arc<Inner>,
}

/// A box's transport that also knows how your other computers are reached ([`Transport::peer`]),
/// so anything holding one box's client finds another's ([`BoxClient::computer`]).
struct Via {
    inner: Arc<dyn Transport>,
    boxes: Weak<Inner>,
}

impl Via {
    fn fleet_request<'a>(&self, request: &'a Request) -> Cow<'a, Request> {
        if !matches!(request.path.split('?').next(), Some("kuvo/computers" | "kuvo/computers/events" | "kuvo/update/dismiss"))
            || request.headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("x-kuvo-client-id"))
        {
            return Cow::Borrowed(request);
        }
        let Some(inner) = self.boxes.upgrade() else { return Cow::Borrowed(request) };
        let boxes = Boxes { inner };
        let Some(identity) = boxes.identity(&mut boxes.state()) else { return Cow::Borrowed(request) };
        use sha2::{Digest, Sha256};
        let id = format!("{:x}", Sha256::digest(identity.public()));
        Cow::Owned(request.clone().header("x-kuvo-client-id", &id[..32]))
    }
}

impl Transport for Via {
    fn send(&self, request: &Request) -> Result<Response> {
        self.inner.send(&self.fleet_request(request))
    }

    fn open(&self, request: &Request, hold: &dyn Fn(Closer) -> bool) -> Result<Stream> {
        self.inner.open(&self.fleet_request(request), hold)
    }

    fn refresh(&self) -> u64 {
        self.inner.refresh()
    }

    fn streams(&self) -> bool {
        self.inner.streams()
    }

    fn here(&self) -> bool {
        self.inner.here()
    }

    fn peer(&self, key: &str) -> Option<Arc<dyn Transport>> {
        let boxes = Boxes { inner: self.boxes.upgrade()? };
        boxes.client(key).map(|client| client.transport())
    }

    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn take_back(&self, keys: &[String]) {
        self.inner.take_back(keys)
    }
}

/// Why this app cannot pair when its key could not be kept.
const NO_KEYCHAIN: &str = "This app couldn’t keep its key, so it can’t reach your other computers.";

impl Boxes {
    pub fn new(options: Options) -> Boxes {
        let Options { nearest, keeper, dial, name, app, kept, timing } = options;
        let inner = Arc::new_cyclic(|weak: &Weak<Inner>| Inner {
            nearest: nearest.clone().map(|transport| BoxClient::new(Via { inner: transport, boxes: weak.clone() })),
            raw: nearest,
            keeper,
            dial,
            name,
            app,
            kept,
            timing,
            state: Mutex::new(State::default()),
            woken: Condvar::new(),
            changed: async_channel::bounded(1),
        });
        let boxes = Boxes { inner };
        boxes.restore();
        boxes
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn notify(&self) {
        let _ = self.inner.changed.0.try_send(());
        self.inner.woken.notify_all();
    }

    /// Something changed (the list, a computer's state, the pairing): read [`Boxes::rows`] again.
    /// Only the latest waits to be taken.
    pub fn changed(&self) -> async_channel::Receiver<()> {
        self.inner.changed.1.clone()
    }

    /// The box on this computer, where there is one.
    pub fn nearest(&self) -> Option<BoxClient> {
        self.inner.nearest.clone()
    }

    /// A client without a local box or any known computer can offer to add its first one.
    pub fn setup_offer(&self) -> Option<SetupOffer> {
        (self.inner.nearest.is_none() && self.state().list.is_empty()).then_some(SetupOffer::AddComputer)
    }

    /// The nearest box's key, once its list said it.
    pub fn nearest_key(&self) -> Option<String> {
        self.state().nearest_key.clone()
    }

    /// Your computers, in the order the box lists them, each with how this client stands with it.
    pub fn rows(&self) -> Vec<Row> {
        self.state().rows.clone()
    }

    /// The choice saved on this device for that computer.
    pub fn connection_preference(&self, key: &str) -> ConnectionPreference {
        self.state().kept.connections.get(key).cloned().unwrap_or_default()
    }

    /// Which remote route was last selected by this device.
    pub fn connection_method(&self, key: &str) -> Option<ConnectionMethod> {
        let state = self.state();
        let linked = state.linked.get(key)?;
        match &linked.selected {
            Some(selected) => selected.active(),
            None => linked.link.as_ref().map(|_| ConnectionMethod::Relay),
        }
    }

    /// Changes only this client's route to one of its computers.
    pub fn set_connection_preference(&self, key: &str, mut preference: ConnectionPreference) -> Result<()> {
        preference.destination = preference.destination.take().map(|address| address.trim().to_owned()).filter(|address| !address.is_empty());
        if preference.destination.as_deref().is_some_and(|address| !valid_destination(address)) {
            return Err(Error::BadAnswer("Give an SSH host name, or user@host.".into()));
        }
        let mut state = self.state();
        let computer = state.list.iter().find(|computer| !key.is_empty() && computer.key == key)
            .filter(|_| state.nearest_key.as_deref() != Some(key))
            .ok_or_else(|| Error::BadAnswer("Choose another computer in your setup.".into()))?;
        if preference.method == ConnectionMethod::Ssh && destinations(&preference, &computer.ssh).is_empty() {
            return Err(Error::BadAnswer("Give an SSH host for this computer first.".into()));
        }
        let mut kept = state.kept.clone();
        if preference == ConnectionPreference::default() {
            kept.connections.remove(key);
        } else {
            kept.connections.insert(key.to_owned(), preference);
        }
        self.write_kept(&kept).map_err(|error| Error::BadAnswer(format!("Couldn't save this computer's connection choice ({error}).")))?;
        state.kept = kept;
        let list = state.list.clone();
        self.update_clients(&mut state, &list);
        Self::rebuild(&mut state);
        drop(state);
        self.notify();
        Ok(())
    }

    /// The setup-wide update facts from the same live snapshot as the computers.
    pub fn fleet_update(&self) -> Option<FleetUpdateSummary> {
        self.state().fleet_update.clone()
    }

    pub fn dismiss_fleet_update(&self, target: &str) -> Result<()> {
        let source = self.lister().ok_or_else(|| Error::NotRunning("None of your computers is answering.".into()))?;
        let digest = self.fleet_update().filter(|summary| summary.target == target).and_then(|summary| summary.target_sha256);
        let summary = source.dismiss_fleet_release(target, digest.as_deref())?;
        let mut state = self.state();
        state.kept.dismissed_update = Some(DismissedUpdate { target: summary.target.clone(), target_sha256: summary.target_sha256.clone(), failures: summary.failures.clone() });
        let _ = self.write_kept(&state.kept);
        state.fleet_update = Some(summary);
        drop(state);
        self.notify();
        Ok(())
    }

    fn take_snapshot(&self, mut list: Computers, from_nearest: bool, client: &BoxClient, following: Option<&Arc<Follow<Followed<Computers>>>>) -> bool {
        let has_fleet = list.fleet_update.is_some() || {
            if client.current_gate() == Gate::Unknown {
                client.connect().is_ok_and(|info| info.has("update.fleet"))
            } else {
                client.has_fleet_updates()
            }
        };
        let mut state = self.state();
        if following.is_some_and(|follow| !state.follow.as_ref().is_some_and(|active| Arc::ptr_eq(active, follow))) {
            return false;
        }
        if from_nearest {
            state.nearest_has_fleet = Some(has_fleet);
        }
        if !has_fleet {
            // A fallback to an older list during a rolling update cannot disprove a known target.
            for row in &mut list.computers {
                row.fleet_update = state.list.iter().find(|known| known.key == row.key)
                    .and_then(|known| known.fleet_update.clone());
            }
        }
        let dismissal = state.kept.dismissed_update.as_ref();
        let summary = list.fleet_update.map(|mut summary| {
            if let Some(dismissal) = dismissal {
                if dismissal.target == summary.target && dismissal.target_sha256 == summary.target_sha256 && summary.failures.iter().all(|id| dismissal.failures.contains(id)) {
                    summary.dismissed = true;
                }
            }
            summary
        });
        if has_fleet {
            state.fleet_update = summary;
        }
        self.take_list(&mut state, &list.computers, from_nearest);
        drop(state);
        self.notify();
        true
    }

    pub fn pairing(&self) -> Pairing {
        self.state().pairing.clone().unwrap_or(Pairing::NotAsked)
    }

    /// The client of the computer with this key: the nearest box's own for itself, a linked one
    /// for another of your computers; none for one this client doesn't connect to.
    pub fn client(&self, key: &str) -> Option<BoxClient> {
        let state = self.state();
        if state.nearest_key.as_deref() == Some(key) {
            return self.inner.nearest.clone();
        }
        state.linked.get(key).map(|linked| linked.client.clone())
    }

    /// Which client the computer with this key has now (none: this app doesn't reach it): another
    /// number, and what was made with the one before holds a client that's been let go.
    pub fn client_made(&self, key: &str) -> Option<u64> {
        let state = self.state();
        if state.nearest_key.as_deref() == Some(key) {
            return Some(0);
        }
        state.linked.get(key).map(|linked| linked.made)
    }

    // MARK: The list

    /// The kept list, as the app opens: your computers before any box has answered.
    fn restore(&self) {
        let Some(path) = &self.inner.kept else { return };
        let Some(kept) = std::fs::read(path).ok().and_then(|bytes| serde_json::from_slice::<Kept>(&bytes).ok()) else { return };
        let computers: Vec<Computer> = kept.computers.iter().map(|kept| Computer { key: kept.key.clone(), name: kept.name.clone(), kind: kept.kind.clone(), address: kept.address.clone(), relay: kept.relay.clone(), ssh: kept.ssh.clone(), ..Computer::default() }).collect();
        self.state().kept = kept;
        // (With a box here, its key isn't known until it lists itself: nothing's linked before.)
        if self.inner.nearest.is_none() {
            self.take(&computers, false);
        }
    }

    fn save(&self, state: &mut State) {
        let computers = state.list.iter().filter(|computer| !computer.key.is_empty()).map(|computer| KeptComputer {
            key: computer.key.clone(), name: computer.name.clone(), kind: computer.kind.clone(),
            address: computer.address.clone(), relay: computer.relay.clone(), ssh: computer.ssh.clone(),
        }).collect();
        let kept = Kept {
            paired: state.kept.paired.clone(),
            connections: state.kept.connections.clone(),
            dismissed_update: state.kept.dismissed_update.clone(),
            computers,
        };
        if kept == state.kept {
            return;
        }
        state.kept = kept;
        let _ = self.write_kept(&state.kept);
    }

    /// This device's key, made and kept the first time it's needed.
    fn identity(&self, state: &mut State) -> Option<Identity> {
        if state.identity.is_none() {
            state.identity = link::keys::identity(&*self.inner.keeper);
        }
        state.identity.clone()
    }

    /// Takes a box's list of your computers (`from_nearest`: the nearest box said it, so the one it
    /// calls `here` is this computer's): a client for each it can reach, and those it no longer
    /// lists let go.
    pub fn take(&self, computers: &[Computer], from_nearest: bool) {
        let mut state = self.state();
        self.take_list(&mut state, computers, from_nearest);
        drop(state);
        self.notify();
    }

    fn take_list(&self, state: &mut State, computers: &[Computer], from_nearest: bool) {
        if from_nearest {
            state.nearest_key = computers.iter().find(|computer| computer.here && !computer.key.is_empty()).map(|computer| computer.key.clone());
        }
        self.update_clients(state, computers);
        state.list = computers.to_vec();
        Self::rebuild(state);
        self.save(state);
    }

    fn update_clients(&self, state: &mut State, computers: &[Computer]) {
        let mut listed = HashSet::new();
        for computer in computers {
            let key = computer.key.clone();
            if key.is_empty() || state.nearest_key.as_deref() == Some(&key) || !listed.insert(key.clone()) {
                continue;
            }
            let preference = state.kept.connections.get(&key).cloned().unwrap_or_default();
            let at = ConnectionFacts { address: computer.address.clone(), relay: computer.relay.clone(), method: preference.method, ssh: destinations(&preference, &computer.ssh) };
            if state.linked.get(&key).is_some_and(|linked| linked.at == at) {
                continue;
            }
            let host = match (&at.address, &at.relay) {
                (Some(address), Some(_)) if at.method != ConnectionMethod::Ssh => Host::from_text(&key, address),
                _ => None,
            };
            let relay = match (host, &at.relay) {
                (Some(host), Some(relay)) => {
                    let identity = self.identity(state);
                    // Its key can't be read or kept: said, rather than nothing shown.
                    if identity.is_none() && !matches!(state.pairing, Some(Pairing::Refused(_))) {
                        state.pairing = Some(Pairing::Refused(NO_KEYCHAIN.into()));
                    }
                    identity
                }
                .map(|identity| {
                    let inner = &self.inner;
                    let link = state.relays.entry(relay.clone()).or_insert_with(|| LinkClient::new(relay.clone(), identity, inner.name.clone(), inner.app.clone(), inner.dial.clone(), inner.timing.link)).clone();
                    let transport: Arc<dyn Transport> = Arc::new(LinkBox::new(link.clone(), host.clone(), computer.name.clone()));
                    (transport, Some((link, host)))
                }),
                // Where the list doesn't say, as the nearest box's own transport reaches it, if it does.
                _ if at.method == ConnectionMethod::Automatic => self.inner.raw.as_ref().and_then(|raw| raw.peer(&key)).map(|transport| (transport, None)),
                _ => None,
            };
            let selected = (!at.ssh.is_empty() || at.method != ConnectionMethod::Automatic).then(|| {
                let identity = if at.ssh.is_empty() { None } else { self.identity(state) };
                let known = self.inner.kept.as_ref().and_then(|file| file.parent()?.parent()).map(|box_home| box_home.join("ssh/known_hosts"));
                let ssh = at.ssh.iter().filter_map(|address| identity.clone().map(|identity| {
                    let transport: Arc<dyn Transport> = Arc::new(SshBox::new(address).for_computer(&key, identity).known_hosts(known.clone()).app_version(&self.inner.app));
                    transport
                })).collect();
                Arc::new(ComputerTransport::new(computer.name.clone(), at.method, ssh, relay.as_ref().map(|(transport, _)| transport.clone())))
            });
            let made = if let Some(selected) = &selected {
                let transport: Arc<dyn Transport> = selected.clone();
                Some((transport, relay.and_then(|(_, link)| link)))
            } else {
                relay
            };
            match made {
                Some((transport, link)) => {
                    let client = BoxClient::new(Via { inner: transport, boxes: Arc::downgrade(&self.inner) });
                    state.made += 1;
                    let made = state.made;
                    if let Some(old) = state.linked.insert(key.clone(), Linked { client, link, at, selected, made }) {
                        old.close();
                    }
                    state.reach.insert(key.clone(), Reach::Connecting);
                    state.failing.remove(&key);
                    state.checked.remove(&key);
                }
                None => {
                    if let Some(old) = state.linked.remove(&key) {
                        old.close();
                    }
                }
            }
        }
        // One the list drops is never talked to again: its session ends.
        let dropped: Vec<String> = state.linked.keys().filter(|key| !listed.contains(*key)).cloned().collect();
        for key in dropped {
            if let Some(old) = state.linked.remove(&key) {
                old.close();
            }
        }
        state.reach.retain(|key, _| listed.contains(key));
        if state.source.as_ref().is_some_and(|(key, made)| !key.is_empty() && state.linked.get(key).is_none_or(|linked| linked.made != *made)) {
            Self::stop_follow(state);
        }
    }

    /// The rows, from the list and each computer's state.
    fn rebuild(state: &mut State) {
        let rows = state
            .list
            .iter()
            .map(|listed| {
                let mut computer = listed.clone();
                let nearest = !computer.key.is_empty() && state.nearest_key.as_deref() == Some(computer.key.as_str());
                computer.here = nearest;
                let reach = if nearest {
                    Reach::Nearest
                } else if state.linked.contains_key(&computer.key) {
                    state.reach.get(&computer.key).cloned().unwrap_or(Reach::Connecting)
                } else {
                    Reach::Unreached
                };
                Row { computer, reach }
            })
            .collect();
        state.rows = rows;
    }

    /// Reads the list once from the box it's followed on, and takes it.
    pub fn read_list(&self) -> Result<()> {
        let (_, client, from_nearest) = self.source().ok_or_else(|| Error::NotRunning("None of your computers is answering.".into()))?;
        self.take_snapshot(client.computers_snapshot()?, from_nearest, &client, None);
        Ok(())
    }

    /// The box supplying the list and its available actions.
    pub fn lister(&self) -> Option<BoxClient> {
        self.source().map(|(_, client, _)| client)
    }

    /// Prefer the nearest box if it understands fleet updates. An older box's list still
    /// bootstraps pairing and reachability until a fleet-capable peer answers.
    fn source(&self) -> Option<((String, u64), BoxClient, bool)> {
        let state = self.state();
        let nearest = || self.inner.nearest.as_ref().map(|client| ((String::new(), 0), client.clone(), true));
        if self.inner.nearest.is_some() && state.nearest_has_fleet != Some(false) {
            return nearest();
        }
        let connected = |key: &str| {
            if state.reach.get(key) != Some(&Reach::Connected) {
                return None;
            }
            let linked = state.linked.get(key)?;
            Some(((key.to_owned(), linked.made), linked.client.clone(), false))
        };
        let peers = || state.rows.iter().filter_map(|row| connected(row.key()));
        let current = || state.source.as_ref().and_then(|(key, _)| connected(key));
        let capable = |(_, client, _): &((String, u64), BoxClient, bool)| client.has_fleet_updates();
        current().filter(capable).or_else(|| peers().find(capable)).or_else(nearest)
            .or_else(current).or_else(|| peers().next())
    }

    // MARK: Pairing

    /// Whether there's anything to pair for: one of your own computers reached over the link.
    fn wants_pairing(state: &State) -> bool {
        state.linked.values().any(|linked| linked.link.is_some() || !linked.at.ssh.is_empty())
    }

    /// Asks the nearest box to pair this device with your setup (`POST kuvo/devices/pair`): it
    /// vouches for the app on its computer to the main computer. Safe to ask again.
    pub fn pair(&self) -> Pairing {
        let Some(nearest) = &self.inner.nearest else { return self.pairing() };
        let identity = {
            let mut state = self.state();
            state.asked = Some(Instant::now());
            self.identity(&mut state)
        };
        let pairing = match identity {
            None => Pairing::Refused(NO_KEYCHAIN.into()),
            Some(identity) => {
                let body = json!({ "key": link::keys::encode(&identity.public()), "name": self.inner.name, "kind": "desktop" });
                match nearest.send(&Request::post("kuvo/devices/pair", body)) {
                    Ok(response) if response.is_success() => Pairing::Paired,
                    Ok(response) => {
                        let refusal = crate::error::Refusal::from_answer(response.status, &response.body);
                        match refusal.reason.as_deref() {
                            Some("removed") => Pairing::Removed(refusal.message),
                            Some(link::PAIRING_WAIT) => Pairing::Checking(refusal.message),
                            Some(link::PAIRING_READD) => Pairing::Readd(refusal),
                            _ if response.status == 503 => Pairing::Waiting(refusal.message),
                            _ => Pairing::Refused(refusal.message),
                        }
                    }
                    Err(error) => Pairing::Waiting(error.to_string()),
                }
            }
        };
        let mut state = self.state();
        if pairing == Pairing::Paired {
            state.kept.paired = state.identity.as_ref().map(|identity| link::keys::encode(&identity.public()));
            let _ = self.write_kept(&state.kept);
        }
        state.pairing = Some(pairing.clone());
        drop(state);
        self.notify();
        pairing
    }

    fn write_kept(&self, kept: &Kept) -> std::io::Result<()> {
        let Some(path) = &self.inner.kept else { return Ok(()) };
        let bytes = serde_json::to_vec(kept)?;
        write_private(path, &bytes)
    }

    /// After this device was removed, on your word: a new key, paired anew. The old one stays
    /// removed everywhere.
    pub fn pair_again(&self) -> Pairing {
        self.inner.keeper.forget();
        let (list, from_nearest) = {
            let mut state = self.state();
            state.identity = None;
            state.kept.paired = None;
            state.pairing = None;
            state.asked_again = false;
            // The old key's sessions and relay connections go with it.
            state.linked.clear();
            for (_, relay) in state.relays.drain() {
                relay.let_go();
            }
            state.reach.clear();
            let _ = self.write_kept(&state.kept);
            (state.list.clone(), self.inner.nearest.is_some())
        };
        let pairing = self.pair();
        self.take(&list, from_nearest);
        pairing
    }

    // MARK: Each computer's state

    /// Asks the computer with this key what it is (`kuvo/info`), and says how it stands: what a
    /// row shows. One that stops answering is `Connecting` until it's been so for a while
    /// (`away_after`), then `Away`.
    pub fn check(&self, key: &str) -> Option<Reach> {
        let (client, made) = {
            let mut state = self.state();
            state.checked.insert(key.to_owned(), Instant::now());
            let linked = state.linked.get(key)?;
            (linked.client.clone(), linked.made)
        };
        let answered = client.connect();
        let mut state = self.state();
        if state.linked.get(key)?.made != made {
            return None;
        }
        let reach = match answered {
            Ok(_) => match client.gate() {
                Gate::Held { missing } => Reach::Held { missing },
                _ => Reach::Connected,
            },
            Err(Error::NeedsUpdating { missing }) => Reach::Held { missing },
            Err(error) if link::is_unpaired(&error) => Reach::NotPaired,
            Err(Error::Refused(refusal)) if refusal.reason.as_deref() == Some(link::PAIRING_WAIT) => Reach::Waiting(refusal.message),
            Err(Error::Refused(refusal)) if refusal.reason.as_deref() == Some(link::PAIRING_READD) => Reach::Readd(refusal),
            Err(error) => {
                let since = *state.failing.entry(key.to_owned()).or_insert_with(Instant::now);
                if since.elapsed() >= self.inner.timing.away_after { Reach::Away(error.to_string()) } else { Reach::Connecting }
            }
        };
        if matches!(reach, Reach::Connected | Reach::Held { .. } | Reach::NotPaired | Reach::Waiting(_) | Reach::Readd(_)) {
            state.failing.remove(key);
        }
        let changed = state.reach.insert(key.to_owned(), reach.clone()).as_ref() != Some(&reach);
        if changed {
            Self::rebuild(&mut state);
            drop(state);
            self.notify();
        }
        Some(reach)
    }

    /// Pairs if there's something to pair for and it hasn't yet, then asks each computer that's
    /// due: one new to the list, one whose session went, one that turned this device away, one
    /// that's away once its box is said to be back (or it's been a while).
    pub fn settle(&self) {
        let timing = self.inner.timing;
        let ask = {
            let mut state = self.state();
            if !Self::wants_pairing(&state) {
                false
            } else if state.pairing.is_none() && self.identity(&mut state).is_some_and(|identity| state.kept.paired.as_deref() == Some(link::keys::encode(&identity.public()).as_str())) {
                // Paired in a run before this one.
                state.pairing = Some(Pairing::Paired);
                false
            } else {
                let waited = state.asked.is_none_or(|asked| asked.elapsed() >= timing.pair_again);
                let turned_away = state.reach.values().any(|reach| *reach == Reach::NotPaired);
                if !turned_away {
                    state.asked_again = false;
                }
                // Asked once for each time it's turned away, never over and over: a box that says
                // "paired" while a computer refuses (its switch is off) is asked nothing more.
                let again = turned_away && waited && !state.asked_again;
                if again && matches!(state.pairing, Some(Pairing::Paired)) {
                    state.asked_again = true;
                }
                match &state.pairing {
                    None => true,
                    Some(Pairing::Waiting(_) | Pairing::Checking(_)) => waited,
                    // A computer that doesn't take a device its setup paired: asked again, which
                    // says whether it was removed.
                    Some(Pairing::Paired) => again,
                    Some(Pairing::Refused(_)) => waited,
                    Some(Pairing::Removed(_) | Pairing::Readd(_) | Pairing::NotAsked) => false,
                }
            }
        };
        if ask {
            self.pair();
        }
        let due: Vec<String> = {
            let state = self.state();
            let paired = matches!(state.pairing, Some(Pairing::Paired)) || self.inner.nearest.is_none();
            state
                .linked
                .iter()
                .filter(|(key, linked)| {
                    // Nothing's asked over the link before this device is paired.
                    if linked.link.is_some() && linked.at.ssh.is_empty() && !paired {
                        return false;
                    }
                    let since = state.checked.get(*key).map(Instant::elapsed);
                    match state.reach.get(*key) {
                        None | Some(Reach::Connecting) => since.is_none_or(|since| since >= timing.away_after.min(Duration::from_secs(2))) || !state.failing.contains_key(*key),
                        Some(Reach::Connected | Reach::Held { .. }) => {
                            let ssh_active = linked.selected.as_ref().and_then(|selected| selected.active()) == Some(ConnectionMethod::Ssh);
                            !ssh_active && linked.link.as_ref().is_some_and(|(link, host)| !link.is_open(host))
                                || !linked.at.ssh.is_empty() && since.is_none_or(|since| since >= timing.look_again)
                                || matches!(state.reach.get(*key), Some(Reach::Held { .. })) && since.is_none_or(|since| since >= timing.follow.held_again)
                        }
                        Some(Reach::NotPaired | Reach::Waiting(_) | Reach::Readd(_)) => since.is_none_or(|since| since >= timing.away_after),
                        Some(Reach::Away(_)) => {
                            let back = state.list.iter().any(|computer| computer.key == **key && computer.reach == "connected");
                            since.is_none_or(|since| since >= if back { timing.away_after } else { timing.look_again })
                        }
                        _ => false,
                    }
                })
                .map(|(key, _)| key.clone())
                .collect()
        };
        for key in due {
            if self.state().stopped {
                return;
            }
            self.check(&key);
        }
    }

    // MARK: Running

    /// Follows the list and keeps each computer's state, on threads of its own, until
    /// [`Boxes::stop`].
    pub fn start(&self) {
        {
            let mut state = self.state();
            if state.started {
                return;
            }
            state.started = true;
        }
        let boxes = self.clone();
        let _ = std::thread::Builder::new().name("kuvo-boxes".into()).spawn(move || boxes.run());
    }

    fn run(&self) {
        let timing = self.inner.timing;
        loop {
            if self.state().stopped {
                return;
            }
            self.settle();
            self.follow_list();
            let state = self.state();
            if state.stopped {
                return;
            }
            // Again when something changes, when a computer that just stopped answering is due,
            // or after a while.
            let soon = state.reach.iter().any(|(key, reach)| matches!(reach, Reach::Connecting | Reach::NotPaired | Reach::Waiting(_)) && state.linked.contains_key(key)) || matches!(state.pairing, Some(Pairing::Waiting(_) | Pairing::Checking(_)) | None) && Self::wants_pairing(&state);
            let wait = if soon { timing.away_after.min(timing.look_again) } else { timing.look_again };
            let _ = self.inner.woken.wait_timeout(state, wait);
        }
    }

    /// Follows the list on its source, if it isn't yet (or its source went).
    fn follow_list(&self) {
        let source = self.source();
        let mut state = self.state();
        let Some((position, client, from_nearest)) = source else {
            Self::stop_follow(&mut state);
            return;
        };
        if state.source.as_ref() == Some(&position) && state.follow.is_some() {
            return;
        }
        Self::stop_follow(&mut state);
        let follow = Arc::new(client.follow_computers(self.inner.timing.follow));
        state.source = Some(position);
        state.follow = Some(follow.clone());
        drop(state);
        let boxes = self.clone();
        let _ = std::thread::Builder::new().name("kuvo-boxes-list".into()).spawn(move || {
            while let Some(update) = follow.next_blocking() {
                if let Followed::Now(list) = update {
                    if !boxes.take_snapshot(list, from_nearest, &client, Some(&follow)) { break; }
                }
            }
        });
    }

    fn stop_follow(state: &mut State) {
        state.source = None;
        if let Some(follow) = state.follow.take() {
            follow.stop();
        }
    }

    /// The computer woke, or its network changed: relay connections that may have died without
    /// closing are let go, and everything's asked again.
    pub fn wake(&self) {
        let relays: Vec<LinkClient> = self.state().relays.values().cloned().collect();
        for relay in relays {
            relay.let_go();
        }
        self.state().checked.clear();
        self.notify();
    }

    pub fn stop(&self) {
        let follow = {
            let mut state = self.state();
            state.stopped = true;
            for relay in state.relays.values() {
                relay.let_go();
            }
            state.follow.take()
        };
        if let Some(follow) = follow {
            follow.stop();
        }
        self.inner.woken.notify_all();
    }
}

/// Writes a file only you can read (`0600`), in a folder only you can open (`0700`): the modes
/// are set each time, so one that was there already, wider, is narrowed before anything's written.
pub fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let folder = path.parent().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no folder"))?;
    std::fs::create_dir_all(folder)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700))?;
        options.mode(0o600);
        let file = options.open(path)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.set_len(0)?;
        (&file).write_all(bytes)
    }
    #[cfg(not(unix))]
    {
        let file = options.open(path)?;
        file.set_len(0)?;
        (&file).write_all(bytes)
    }
}

/// What this computer is called, for this device's name among your devices.
pub fn device_name() -> String {
    #[cfg(unix)]
    {
        let mut buffer = [0u8; 256];
        // SAFETY: gethostname writes at most the buffer's length, and ends it where it fits.
        if unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len() - 1) } == 0 {
            let end = buffer.iter().position(|byte| *byte == 0).unwrap_or(buffer.len());
            let name = String::from_utf8_lossy(&buffer[..end]).trim_end_matches(".local").trim().to_owned();
            if !name.is_empty() {
                return name;
            }
        }
    }
    "This computer".into()
}
