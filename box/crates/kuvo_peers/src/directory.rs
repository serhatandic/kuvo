//! Accepted setup roster, peer announcements, and record ownership.

use std::collections::{BTreeMap, BTreeSet};

use kuvo_protocol::routes::types::{
    GetKuvoComputersStatus200ComputersItemAgents as ComputerAgents,
    GetKuvoComputersStatus200ComputersItemAgentsState4 as AgentState,
    GetKuvoComputersStatus200ComputersItemAtWorkItem as AtWork,
    GetKuvoSetupOutputsStatus200PendingComputersItem as Computer,
};
use kuvo_wire::keys::{Address, Host, encode};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    reach::{Millis, Reach, ReleaseState},
    session::ComputerKind,
};

pub type ComputerKey = [u8; 32];

#[derive(Clone, Debug)]
pub struct Member {
    pub key: ComputerKey,
    pub address: Option<Address>,
    pub name: String,
    pub kind: ComputerKind,
}

impl Member {
    pub fn computer(&self) -> Computer {
        Computer {
            id: encode(&self.key),
            name: self.name.clone(),
        }
    }

    pub fn host(&self) -> Option<Host> {
        Some(Host {
            key: self.key,
            address: self.address?,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Roster {
    pub here: Option<ComputerKey>,
    pub name: String,
    pub main: Option<ComputerKey>,
    /// Accepted enrolled members, in setup order. Neither the local key nor revoked keys belong here.
    pub members: Vec<Member>,
}

impl Roster {
    pub fn local(&self) -> Computer {
        Computer {
            id: self
                .here
                .as_ref()
                .map_or_else(|| "here".into(), |key| encode(key)),
            name: self.name.clone(),
        }
    }

    pub fn member(&self, key: &ComputerKey) -> Option<&Member> {
        self.members.iter().find(|peer| &peer.key == key)
    }

    pub fn at_address(&self, address: Address) -> Option<&Member> {
        self.members
            .iter()
            .find(|peer| peer.address == Some(address))
    }

    pub fn pending(
        &self,
        reach: &Reach,
        told: &Announcements,
        feature: &str,
        has_copy: impl Fn(&ComputerKey) -> bool,
    ) -> Vec<Computer> {
        let mut pending: Vec<_> = self
            .members
            .iter()
            .filter(|peer| {
                let key = encode(&peer.key);
                let supported = told.get(&key).is_none_or(|said| {
                    said.features_cut
                        || said
                            .features
                            .as_ref()
                            .is_none_or(|features| features.iter().any(|name| name == feature))
                });
                peer.address.is_some()
                    && reach.away_since(&key).is_none()
                    && supported
                    && !has_copy(&peer.key)
            })
            .map(Member::computer)
            .collect();
        pending.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then(a.id.cmp(&b.id))
        });
        pending
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AnnouncedAgent {
    pub session: String,
    pub name: String,
    pub kind: AgentKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Session,
    Bot,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerFacts {
    #[serde(rename = "fleetUpdate", skip_serializing_if = "Option::is_none")]
    pub release: Option<FleetUpdate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kuvo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at_work: Option<Vec<AtWork>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agents: Option<ComputerAgents>,
    // Compatibility fields for consumers of the retained announcement, including older boxes.
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

impl<'de> Deserialize<'de> for ComputerFacts {
    fn deserialize<D: Deserializer<'de>>(value: D) -> Result<Self, D::Error> {
        let mut given = serde_json::Map::<String, Value>::deserialize(value)?;
        let release = given
            .remove("fleetUpdate")
            .and_then(|field| serde_json::from_value(field).ok());
        let working = given.remove("working").and_then(|field| {
            let count = field.as_f64()?;
            ((0.0..=10_000.0).contains(&count) && count.fract() == 0.0).then_some(count as u64)
        });
        let at_work = given.remove("atWork").and_then(|rows| {
            working?;
            Some(
                rows.as_array()?
                    .iter()
                    .filter_map(active_session)
                    .take(20)
                    .collect(),
            )
        });
        let agents = given.remove("agents").and_then(|field| {
            let state: AgentState = serde_json::from_value(field.get("state")?.clone()).ok()?;
            let why = field
                .get("why")
                .and_then(Value::as_str)
                .filter(|why| {
                    state == AgentState::Down
                        && matches!(
                            *why,
                            "engine"
                                | "notInstalled"
                                | "outdated"
                                | "keeper"
                                | "crashing"
                                | "stuck"
                                | "notStarting"
                                | "app"
                        )
                })
                .map(str::to_owned);
            Some(ComputerAgents { state, why })
        });
        let mut short = |key, limit| {
            let given = given.remove(key)?;
            let text = given.as_str()?;
            (!text.is_empty() && text.encode_utf16().count() <= limit).then(|| text.to_owned())
        };
        let version = short("version", 40);
        let engine = short("engine", 40);
        let system = short("system", 80);
        let kuvo = short("kuvo", 40);
        let extra = given
            .into_iter()
            .filter(|(key, _)| {
                matches!(
                    key.as_str(),
                    "wsl"
                        | "layout"
                        | "installedSha256"
                        | "staged"
                        | "cannot"
                        | "candidate"
                        | "releaseManifest"
                        | "runningManifest"
                        | "waiting"
                        | "asking"
                        | "oldApp"
                        | "addresses"
                        | "release"
                )
            })
            .collect();
        Ok(Self {
            release,
            version,
            engine,
            system,
            kuvo,
            working,
            at_work,
            agents,
            extra,
        })
    }
}

fn active_session(value: &Value) -> Option<AtWork> {
    let id = value.get("id")?.as_str()?;
    if !record_id(id, "ses_") {
        return None;
    }
    let title = value
        .get("title")
        .and_then(Value::as_str)
        .and_then(work_title);
    Some(AtWork {
        id: id.into(),
        title,
    })
}

pub(crate) fn work_title(text: &str) -> Option<String> {
    let title = crate::calls::plain(text, 200);
    (!title.is_empty()).then_some(title)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FleetUpdate {
    pub state: ReleaseState,
    #[serde(flatten)]
    pub facts: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Announcement {
    pub at: Millis,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heard: Option<Millis>,
    pub list: Vec<AnnouncedAgent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub features: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub features_cut: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owns: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub groups: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub computer: Option<ComputerFacts>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<ComputerKind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub moved: Vec<MoveHint>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MoveHint {
    pub session: String,
    pub key: String,
    pub at: Millis,
}

fn is_false(value: &bool) -> bool {
    !*value
}

pub type Announcements = BTreeMap<String, Announcement>;

impl Announcement {
    pub fn release(&self) -> Option<ReleaseState> {
        Some(self.computer.as_ref()?.release.as_ref()?.state)
    }

    pub fn supports(&self, feature: &str) -> bool {
        self.features
            .as_ref()
            .is_some_and(|features| features.iter().any(|f| f == feature))
    }

    pub(crate) fn announces(&self, scope: RecordScope, id: &str) -> bool {
        if let Some(owns) = &self.owns {
            let digest = owned_digest(id);
            return owns
                .as_bytes()
                .as_chunks::<6>()
                .0
                .iter()
                .any(|entry| entry == digest.as_bytes());
        }
        match scope {
            RecordScope::Group => self
                .groups
                .as_ref()
                .is_some_and(|groups| groups.iter().any(|group| group == id)),
            RecordScope::Session => self.list.iter().any(|agent| agent.session == id),
            RecordScope::Bot => self.list.iter().any(|agent| {
                agent.kind == AgentKind::Bot
                    && (agent.session == id || agent.id.as_deref() == Some(id))
            }),
        }
    }

    pub(crate) fn received(
        mut given: serde_json::Value,
        peer: &Member,
        now: Millis,
    ) -> Option<Self> {
        let input = given.as_object_mut()?;
        let list = input.get_mut("list")?.as_array_mut()?;
        list.retain(|row| {
            row.get("session")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|id| record_id(id, "ses_"))
                && row.get("name").is_some_and(serde_json::Value::is_string)
                && matches!(
                    row.get("kind").and_then(serde_json::Value::as_str),
                    Some("session" | "bot")
                )
        });
        list.truncate(120);
        for row in list {
            let fields = row.as_object_mut()?;
            if fields
                .get("id")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|id| !record_id(id, "bot_"))
            {
                fields.remove("id");
            }
        }
        let features = input.remove("features").and_then(|value| {
            let mut names: Vec<_> = value
                .as_array()?
                .iter()
                .filter_map(serde_json::Value::as_str)
                .filter(|name| name.encode_utf16().count() <= 60)
                .take(301)
                .map(str::to_owned)
                .collect();
            let cut = names.len() > 300;
            names.truncate(300);
            Some((names, cut))
        });
        input.remove("featuresCut");
        if input
            .get("owns")
            .and_then(serde_json::Value::as_str)
            .is_none_or(|owns| {
                !owns.len().is_multiple_of(6)
                    || owns.len() > 30_000
                    || !owns
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            })
        {
            input.remove("owns");
        }
        if let Some(groups) = input.remove("groups").and_then(|value| {
            Some(
                value
                    .as_array()?
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .filter(|id| {
                        record_id(id, "grp_") && !id.bytes().any(|b| b.is_ascii_uppercase())
                    })
                    .take(100)
                    .map(str::to_owned)
                    .collect::<Vec<_>>(),
            )
        }) {
            input.insert("groups".into(), serde_json::json!(groups));
        }
        let moved: Vec<_> = input
            .remove("moved")
            .and_then(|value| value.as_array().cloned())
            .into_iter()
            .flatten()
            .filter_map(|value| serde_json::from_value::<MoveHint>(value).ok())
            .filter(|entry| {
                record_id(&entry.session, "ses_")
                    && kuvo_wire::keys::decode_exact::<32>(&entry.key).is_some()
            })
            .take(400)
            .collect();
        input.remove("heard");
        input.remove("kind");
        if !input
            .get("computer")
            .is_some_and(serde_json::Value::is_object)
        {
            input.remove("computer");
        }
        let mut announcement: Self = serde_json::from_value(given).ok()?;
        announcement.heard = Some(now);
        announcement.moved = moved;
        announcement.kind = Some(peer.kind);
        if let Some((names, cut)) = features {
            announcement.features = Some(names);
            announcement.features_cut = cut;
        }
        Some(announcement)
    }
}

fn record_id(id: &str, prefix: &str) -> bool {
    id.strip_prefix(prefix).is_some_and(|suffix| {
        (1..=60).contains(&suffix.len()) && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
    })
}

pub fn owned_digest(id: &str) -> String {
    encode(&Sha256::digest(id.as_bytes()))[..6].to_owned()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RecordScope {
    Session,
    Bot,
    Group,
}

/// Evidence only this resident can authorize. Moves supply journal destinations, while engine,
/// agents and work supply their held records. Peer announcements can only locate a record.
#[derive(Clone, Debug, Default)]
pub struct Ownership {
    pub held: BTreeSet<(RecordScope, String)>,
    pub frozen: BTreeSet<String>,
    pub journal: BTreeMap<(RecordScope, String), MoveLocation>,
    pub replacements: BTreeMap<ComputerKey, ComputerKey>,
}

#[derive(Clone, Debug)]
pub struct MoveLocation {
    pub key: ComputerKey,
    /// A committed departure has a time. A switch still in the journal
    /// takes precedence over announcements until it settles.
    pub departed_at: Option<Millis>,
}

#[derive(Clone, Debug)]
pub struct Owner {
    pub key: Option<ComputerKey>,
    pub name: String,
    pub local: bool,
    pub reachable: bool,
}

impl Ownership {
    pub fn owner_of(
        &self,
        scope: RecordScope,
        id: &str,
        roster: &Roster,
        told: &Announcements,
        reach: &Reach,
        now: Millis,
    ) -> Option<Owner> {
        if id.is_empty()
            || id.len() > 100
            || !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        {
            return None;
        }
        let record = (scope, id.to_owned());
        if let Some(MoveLocation { key, .. }) = self.journal.get(&record) {
            return Some(named(
                roster,
                told,
                reach,
                *self.replacements.get(key).unwrap_or(key),
                now,
            ));
        }
        if !self.frozen.contains(id) && self.held.contains(&record) {
            return Some(here(roster));
        }
        let mut peers = roster.members.iter().filter(|peer| {
            told.get(&encode(&peer.key))
                .is_some_and(|said| said.announces(scope, id))
        });
        let peer = peers.next()?;
        peers
            .next()
            .is_none()
            .then(|| named(roster, told, reach, peer.key, now))
    }
}

fn here(roster: &Roster) -> Owner {
    Owner {
        key: roster.here,
        name: roster.name.clone(),
        local: true,
        reachable: true,
    }
}

fn named(
    roster: &Roster,
    told: &Announcements,
    reach: &Reach,
    key: ComputerKey,
    now: Millis,
) -> Owner {
    if roster.here == Some(key) {
        return here(roster);
    }
    let text = encode(&key);
    let peer = roster.member(&key);
    let heard = told.get(&text).map(|said| said.heard.unwrap_or(said.at));
    Owner {
        key: Some(key),
        name: peer.map_or_else(|| text.clone(), |peer| peer.name.clone()),
        local: false,
        reachable: peer.is_some() && !reach.peer_is_away(&text, heard, now),
    }
}
