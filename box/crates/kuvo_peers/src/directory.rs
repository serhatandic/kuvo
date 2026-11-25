//! Accepted setup roster, peer announcements, and record ownership.

use std::collections::{BTreeMap, BTreeSet};

use kuvo_protocol::json::{Null, Nullable};
use kuvo_protocol::routes::types::{
    GetKuvoBoxAtWorkStatus200AskingItem as Asking,
    GetKuvoComputersStatus200ComputersItemAgents as ComputerAgents,
    GetKuvoComputersStatus200ComputersItemAgentsState4 as AgentState,
    GetKuvoComputersStatus200ComputersItemAtWorkItem as AtWork,
    GetKuvoSetupOutputsStatus200PendingComputersItem as Computer,
    PostKuvoUpdateRetryStatus202 as FleetRow, PostKuvoUpdateRetryStatus202Cannot as CannotRow,
    PostKuvoUpdateRetryStatus202CannotState as CannotState,
    PostKuvoUpdateRetryStatus202UpToDateLine as FleetLine,
    PostKuvoUpdateRetryStatus202UpToDateLineTone2 as FleetTone,
};
use kuvo_wire::keys::{Address, Host, encode};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Number, Value};
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
    pub fn members_by_name(&self) -> Vec<&Member> {
        let mut peers: Vec<_> = self.members.iter().collect();
        peers.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then(a.key.cmp(&b.key))
        });
        peers
    }

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
#[serde(rename_all = "camelCase")]
pub struct AnnouncedAgent {
    pub session: String,
    pub name: String,
    pub kind: AgentKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// A bot's look, as its computer has it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub look: Option<Look>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// A bot's job.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub marketplace_agent: Option<String>,
}

/// A bot's look: its colour, shape and face, each one of the words a bot can have.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Look {
    pub color: String,
    pub shape: String,
    pub face: String,
}

const COLORS: &[&str] = &[
    "mint", "sky", "indigo", "violet", "pink", "coral", "orange", "amber", "lime", "graphite",
];
const SHAPES: &[&str] = &[
    "round", "squircle", "pebble", "gem", "bloom", "loaf", "capsule", "dome",
];
const FACES: &[&str] = &["calm", "happy", "sleepy", "wink", "curious", "bright"];

impl Look {
    /// Whether `given` is a look a bot can have: each part one of the words.
    pub fn fits(given: Option<&serde_json::Value>) -> bool {
        given.is_some_and(|given| {
            [("color", COLORS), ("shape", SHAPES), ("face", FACES)]
                .iter()
                .all(|(name, words)| {
                    given[*name]
                        .as_str()
                        .is_some_and(|word| words.contains(&word))
                })
        })
    }

    /// A look as it was told: each part one of the words, else the plain one.
    pub(crate) fn of(given: &serde_json::Value) -> Option<Self> {
        let given = given.as_object()?;
        let part = |name: &str, words: &[&str], plain: &str| {
            given
                .get(name)
                .and_then(serde_json::Value::as_str)
                .filter(|word| words.contains(word))
                .unwrap_or(plain)
                .to_owned()
        };
        Some(Self {
            color: part("color", COLORS, "graphite"),
            shape: part("shape", SHAPES, "round"),
            face: part("face", FACES, "calm"),
        })
    }
}

/// Text told of an agent, kept up to `most` characters; none when it's empty or not text.
fn kept_text(given: Option<serde_json::Value>, most: usize) -> Option<serde_json::Value> {
    let text = given?.as_str()?.chars().take(most).collect::<String>();
    (!text.is_empty()).then(|| text.into())
}

/// A told agent's facts beyond its conversation, name and kind, kept as a box keeps them.
fn keep_agent_facts(fields: &mut serde_json::Map<String, serde_json::Value>) {
    let bot = fields.get("kind").and_then(serde_json::Value::as_str) == Some("bot");
    let look = fields
        .remove("look")
        .filter(|_| bot)
        .and_then(|look| Look::of(&look));
    let agent = fields
        .remove("marketplaceAgent")
        .filter(|_| bot)
        .filter(|agent| {
            agent.as_str().is_some_and(|agent| {
                let mut bytes = agent.bytes();
                agent.len() <= 63
                    && bytes
                        .next()
                        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
                    && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            })
        });
    let texts = [("project", 200), ("about", 500), ("machine", 200)]
        .map(|(name, most)| (name, kept_text(fields.remove(name), most)));
    let kept = [
        ("look", look.map(|look| serde_json::json!(look))),
        ("marketplaceAgent", agent),
    ];
    for (name, value) in kept.into_iter().chain(texts) {
        if let Some(value) = value {
            fields.insert(name.into(), value);
        }
    }
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
    pub waiting: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asking: Option<Vec<Asking>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agents: Option<ComputerAgents>,
    // Compatibility fields for consumers of the retained announcement, including older boxes.
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct UpdateFacts {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kuvo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_manifest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running_manifest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub staged: Option<PreparedRelease>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub switching: Option<PreparedRelease>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receiving: Option<ReleaseProgress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cannot: Option<CannotUpdate>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PreparedRelease {
    pub version: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReleaseProgress {
    #[serde(flatten)]
    pub release: PreparedRelease,
    pub fill: Number,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CannotUpdate {
    #[serde(flatten)]
    pub release: PreparedRelease,
    pub reason: String,
    pub fixable: bool,
}

impl CannotUpdate {
    pub fn row(
        &self,
        current: Option<String>,
        current_sha256: Option<String>,
        can_retry: bool,
    ) -> FleetRow {
        let mut words = match &current {
            Some(version) => {
                let short = version
                    .split_once('-')
                    .map_or(version.as_str(), |(_, short)| short);
                format!("{short} · {}", self.reason)
            }
            None => self.reason.clone(),
        };
        let (tone, short) = if self.fixable {
            (FleetTone::Warning, Some("Can’t update".into()))
        } else {
            words.push_str(" · Updates with the next release");
            (FleetTone::Blocked, None)
        };
        FleetRow::Cannot(CannotRow {
            can_retry: self.fixable && can_retry,
            current: current.map_or(Nullable::Null(Null), Nullable::Value),
            current_sha256,
            fill: None,
            fixable: self.fixable,
            last_failure_at: None,
            line: Some(FleetLine {
                words,
                tone,
                short,
                phase: None,
                fill: None,
            }),
            reason: self.reason.clone(),
            requires_update: None,
            seen: None,
            state: CannotState::Cannot,
            target: self.release.version.clone(),
            target_sha256: Some(self.release.sha256.clone()),
        })
    }
}

impl ComputerFacts {
    pub(crate) fn in_bridge_handoff(&self, bridge: &PreparedRelease) -> bool {
        let Some(version) = self.kuvo.as_deref() else {
            return false;
        };
        let text = |field| self.extra.get(field).and_then(Value::as_str);
        if text("releaseTarget").is_none() {
            return version == "0.8.0-beta.48" || version == bridge.version.as_str();
        }
        // While Rust is on trial, the last confirmed release can still be the bridge.
        version == bridge.version.as_str()
            && text("installedSha256") == Some(bridge.sha256.as_str())
            && self.extra.get("switching").is_some_and(Value::is_object)
    }

    pub(crate) fn updates(&self) -> UpdateFacts {
        let text = |field| {
            self.extra
                .get(field)
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        UpdateFacts {
            kuvo: self.kuvo.clone(),
            installed_sha256: text("installedSha256"),
            release_target: text("releaseTarget"),
            candidate: text("candidate"),
            release_manifest: text("releaseManifest"),
            running_manifest: text("runningManifest"),
            staged: self
                .extra
                .get("staged")
                .and_then(|value| serde_json::from_value(value.clone()).ok()),
            switching: self
                .extra
                .get("switching")
                .and_then(|value| serde_json::from_value(value.clone()).ok()),
            receiving: self
                .extra
                .get("receiving")
                .and_then(|value| serde_json::from_value(value.clone()).ok()),
            cannot: self
                .extra
                .get("cannot")
                .and_then(|value| serde_json::from_value(value.clone()).ok()),
        }
    }
}

impl<'de> Deserialize<'de> for ComputerFacts {
    fn deserialize<D: Deserializer<'de>>(value: D) -> Result<Self, D::Error> {
        let mut given = serde_json::Map::<String, Value>::deserialize(value)?;
        let release = given
            .remove("fleetUpdate")
            .and_then(|field| serde_json::from_value(field).ok());
        let count = |field: Value| {
            let count = field.as_f64()?;
            ((0.0..=10_000.0).contains(&count) && count.fract() == 0.0).then_some(count as u64)
        };
        let working = given.remove("working").and_then(count);
        let waiting = given.remove("waiting").and_then(count);
        let asked = given.remove("asking");
        let asking = waiting.map(|_| {
            asked
                .as_ref()
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(asking_session)
                .take(10)
                .collect()
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
                        | "switching"
                        | "receiving"
                        | "cannot"
                        | "candidate"
                        | "releaseManifest"
                        | "releaseTarget"
                        | "runningManifest"
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
            waiting,
            asking,
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

fn asking_session(value: &Value) -> Option<Asking> {
    let AtWork { id, title } = active_session(value)?;
    let asks = value
        .get("asks")
        .and_then(Value::as_str)
        .and_then(work_title);
    Some(Asking { id, title, asks })
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
    /// Where a bot deleted there ends up: the Finder's Trash (`trash`) or Kuvo Code's own trash
    /// folder (`kuvo`); none from a computer before it said.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trash: Option<String>,
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
            keep_agent_facts(fields);
        }
        // Where a bot deleted there ends up, as it said.
        if !matches!(
            input.get("trash").and_then(serde_json::Value::as_str),
            Some("trash" | "kuvo")
        ) {
            input.remove("trash");
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
    /// The destination confirmed activation. Only source cleanup remains.
    pub(crate) retiring: bool,
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_told_agent_keeps_a_bots_look_job_and_where_its_deleted_bots_go() {
        let peer = Member {
            key: [7; 32],
            address: None,
            name: "Studio".into(),
            kind: ComputerKind::Machine,
        };
        let told = Announcement::received(
            json!({ "at": 5, "trash": "trash", "list": [
                { "session": "ses_far1", "name": "Far", "kind": "bot", "id": "bot_far1",
                  "look": { "color": "sky", "shape": "star", "face": "wink" }, "about": "x".repeat(600),
                  "marketplaceAgent": "news-reader", "project": "" },
                { "session": "ses_plans", "name": "Plans", "kind": "session", "look": { "color": "sky" },
                  "marketplaceAgent": "news-reader", "project": "Trips" },
            ] }),
            &peer,
            9,
        )
        .expect("taken");
        assert_eq!(told.trash.as_deref(), Some("trash"));
        let [far, plans] = &told.list[..] else {
            panic!("two agents: {:?}", told.list);
        };
        // (A part that isn't one of the words is the plain one's.)
        assert_eq!(
            far.look,
            Some(Look {
                color: "sky".into(),
                shape: "round".into(),
                face: "wink".into()
            })
        );
        assert_eq!(far.about.as_ref().map(|about| about.len()), Some(500));
        assert_eq!(far.marketplace_agent.as_deref(), Some("news-reader"));
        assert_eq!(far.project, None);
        // (A look and a marketplace agent are a bot's alone.)
        assert_eq!(plans.look, None);
        assert_eq!(plans.marketplace_agent, None);
        assert_eq!(plans.project.as_deref(), Some("Trips"));
        let unsaid =
            Announcement::received(json!({ "at": 5, "trash": "bin", "list": [] }), &peer, 9);
        assert_eq!(unsaid.expect("taken").trash, None);
    }
}
