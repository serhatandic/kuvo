//! What can be done with each listed computer from here: Rename, Connect, Remove and the removals
//! a client offers. The setup says who keeps the machines' records (the writer); this box's own
//! hearing says whether the writer is reachable and takes the change.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::setup_lists::View;
use kuvo_protocol::routes::types::GetKuvoComputersStatus200ComputersItemActionsItem as Action;

pub const MANAGE_FEATURE: &str = "computers.manage";
pub const RELAY_LEASE_FEATURE: &str = "setup.relay-lease";
const PRIMARY_UNLINK: &str = "Add another computer to this setup before unlinking its main computer.";

/// What the computers list takes from the setup.
#[derive(Default)]
pub struct Listing {
    pub wipe_pending: Vec<kuvo_protocol::routes::types::GetKuvoComputersStatus200WipePendingItem>,
    pub manage: ManageFacts,
}

/// What the setup knows of who keeps the machines' records, worked out once per list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ManageFacts {
    pub setup: Option<SetupFacts>,
    pub writer: Writer,
    /// The released Macs that enrolled this computer outside a setup.
    pub macs: usize,
    /// The machines whose records the writer keeps, by key.
    pub machines: BTreeSet<String>,
    /// This computer can be wiped from here.
    pub wipe_here: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SetupFacts {
    pub primary: String,
    /// This computer is the main computer it follows.
    pub main_here: bool,
    pub managed: bool,
    pub checkpointed: bool,
    /// The accepted members, this computer among them.
    pub boxes: Vec<Candidate>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub key: String,
    pub name: String,
    pub machine: bool,
    pub signed: bool,
    pub provisional: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Writer {
    #[default]
    None,
    Here,
    Elsewhere { key: String, name: String },
}

/// A listed computer, as its row says it.
pub struct Row<'a> {
    pub key: &'a str,
    pub name: &'a str,
    pub mac: bool,
    pub here: bool,
    pub relay: bool,
    pub away: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum What {
    Rename,
    Remove,
}

impl What {
    fn verb(self) -> &'static str {
        match self {
            What::Rename => "rename",
            What::Remove => "remove",
        }
    }

    fn done(self) -> &'static str {
        match self {
            What::Rename => "renamed",
            What::Remove => "removed",
        }
    }
}

pub struct Manage<'a> {
    facts: &'a ManageFacts,
    view: &'a View,
    here: &'a str,
    own: String,
    reachable: bool,
    takes: bool,
    now: crate::reach::Millis,
}

impl<'a> Manage<'a> {
    pub fn new(
        facts: &'a ManageFacts,
        view: &'a View,
        here: &'a str,
        now: crate::reach::Millis,
    ) -> Self {
        let own = view.roster.here.map(|key| kuvo_wire::keys::encode(&key)).unwrap_or_default();
        let (reachable, takes) = match &facts.writer {
            Writer::None => (false, false),
            Writer::Here => (true, true),
            Writer::Elsewhere { key, .. } => (
                listed(view, key),
                announced(view, key, MANAGE_FEATURE),
            ),
        };
        Self { facts, view, here, own, reachable, takes, now }
    }

    /// The actions for `row`, in the order a client offers them. `trying`: it's being reached
    /// again now (Connect was asked).
    pub fn actions(&self, row: &Row, trying: bool) -> Vec<Action> {
        let mut actions = Vec::new();
        if self.why_not(row, What::Rename).is_none() {
            actions.push(json!({"id":"rename","label":"Rename"}));
        }
        if row.relay && row.away && !trying {
            actions.push(json!({"id":"connect","label":"Connect"}));
        }
        if self.why_not(row, What::Remove).is_none() {
            let name = if row.name.is_empty() { "this machine" } else { row.name };
            let last = self.lone_primary(row.key);
            if last && self.facts.wipe_here {
                actions.push(json!({
                    "id":"remove","label":"Wipe",
                    "confirm":{"title":format!("Wipe {name}?"),"message":"This is the only computer. Wiping deletes Kuvo Code and its sessions and bots. No setup is left.","label":"Wipe"},
                    "removal":{"choices":["wipe"],"last":true},
                }));
            } else if !last {
                // The main hands its role over first.
                let setup = self.facts.setup.as_ref();
                let next = setup
                    .filter(|setup| setup.primary == row.key)
                    .and_then(|_| self.next_main(row.key));
                let first = next.map_or_else(
                    || "Your".to_owned(),
                    |next| format!("{} becomes the main computer first. Then your", next.name),
                );
                let message = format!(
                    "{first} other computers stop reaching {name}. It keeps running, with its sessions, and you can add it again later."
                );
                let label = if setup.is_some_and(|setup| setup.checkpointed) { "Unlink" } else { "Remove" };
                let mut action = json!({"id":"remove","label":label});
                action["confirm"] = json!({"title":format!("Remove {name}?"),"message":message,"label":"Remove"});
                if setup.is_some() {
                    action["removal"] = json!({"choices":["unlink","wipe"]});
                    if next.is_some() {
                        action["removal"]["handOver"] = Value::String(message);
                    }
                }
                actions.push(action);
            }
        }
        actions
            .into_iter()
            .map(|action| serde_json::from_value(action).expect("a computer action fits the route"))
            .collect()
    }

    /// Why `row` can't be renamed or removed from here, in words a client shows; `None` when it can.
    fn why_not(&self, row: &Row, what: What) -> Option<String> {
        let name = if row.name.is_empty() { "This computer" } else { row.name };
        let setup = self.facts.setup.as_ref();
        let managed = setup.is_some_and(|setup| setup.managed);
        if what == What::Remove && !row.key.is_empty() && setup.is_some_and(|setup| setup.primary == row.key) {
            return (!self.lone_primary(row.key) && self.next_main(row.key).is_none())
                .then(|| PRIMARY_UNLINK.to_owned());
        }
        if row.mac && !managed {
            return Some(match what {
                What::Rename => format!("{name} is a Mac: it goes by the name it has there."),
                What::Remove => "A Mac can't be removed from here.".to_owned(),
            });
        }
        if what == What::Remove && row.here && !managed {
            return Some(format!(
                "{} can't remove itself: remove it from another of your computers.",
                self.here
            ));
        }
        let writer = match &self.facts.writer {
            Writer::None if self.facts.macs > 1 => {
                let verb = match what {
                    What::Rename => "Rename",
                    What::Remove => "Remove",
                };
                return Some(format!("{verb} {name} from the Mac that added it."));
            }
            Writer::None => {
                return Some(format!(
                    "No Mac keeps {name}'s record, so it can't be {} from here.",
                    what.done()
                ));
            }
            Writer::Here => self.here,
            Writer::Elsewhere { name, .. } => name.as_str(),
        };
        if !self.reachable {
            return Some(format!(
                "{} can't reach {writer}, which keeps your machines' records: {} {name} from there.",
                self.here,
                what.verb()
            ));
        }
        if row.key.is_empty() || !self.facts.machines.contains(row.key) {
            return Some(format!(
                "{name} isn't one of {writer}’s machines, so it can't be {} from here.",
                what.done()
            ));
        }
        if !self.takes {
            return Some(format!(
                "{writer} needs a newer Kuvo Code to {} your machines. Update it, then try again.",
                what.verb()
            ));
        }
        None
    }

    /// The only computer of a setup it leads.
    fn lone_primary(&self, key: &str) -> bool {
        self.facts
            .setup
            .as_ref()
            .is_some_and(|setup| setup.main_here && setup.primary == key)
            && self.view.roster.members.is_empty()
    }

    /// Who would take over as main from `except`: a signed, settled member this computer hears
    /// (and that takes relay leases), at hand first, then machines, then by name.
    fn next_main(&self, except: &str) -> Option<&Candidate> {
        let setup = self.facts.setup.as_ref()?;
        let now = self.now;
        let rank = |candidate: &Candidate| {
            let present = candidate.key == self.own
                || !self.view.reach.peer_is_away(&candidate.key, heard(self.view, &candidate.key), now);
            u8::from(present) * 2 + u8::from(candidate.machine)
        };
        let mut candidates: Vec<_> = setup
            .boxes
            .iter()
            .filter(|candidate| {
                candidate.key != except
                    && candidate.signed
                    && !candidate.provisional
                    && (candidate.key == self.own
                        || (announced(self.view, &candidate.key, RELAY_LEASE_FEATURE)
                            && listed(self.view, &candidate.key)))
            })
            .collect();
        candidates.sort_by(|a, b| rank(b).cmp(&rank(a)).then_with(|| a.name.cmp(&b.name)));
        candidates.into_iter().next()
    }
}

fn listed(view: &View, key: &str) -> bool {
    view.roster
        .members
        .iter()
        .any(|member| kuvo_wire::keys::encode(&member.key) == key)
}

fn announced(view: &View, key: &str, feature: &str) -> bool {
    view.announced
        .get(key)
        .and_then(|said| said.features.as_ref())
        .is_some_and(|features| features.iter().any(|name| name == feature))
}

fn heard(view: &View, key: &str) -> Option<crate::reach::Millis> {
    view.announced.get(key).and_then(|said| said.heard)
}
