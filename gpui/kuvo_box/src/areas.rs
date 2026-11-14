//! What the rail's areas list, as a box says it (PROTOCOL.md): its bots with what each is doing now
//! (`kuvo/bots`) and group chats (`kuvo/groups`), the Work Queue's categories and counts
//! (`kuvo/work-queue` with `counts=1`), your computers (`kuvo/computers`), the plan limits of every
//! account across them with each one's rings and levels (`kuvo/agents/api/kuvo/limits`'s `rows`),
//! and how many things wait on you in each area (`kuvo/waiting`). The box counts, levels and picks
//! the rings; this reads them.

use std::collections::HashMap;

use serde::Deserialize;

use crate::client::BoxClient;
use crate::error::Result;
use crate::follow::{Follow, Followed, Timing, follow_lines};

// MARK: Bots

/// A bot's look, as the box sends it: a colour, a shape and a face, by name. One a newer box names
/// that a client doesn't know is drawn as the client's own fallback (graphite, round), as the Mac's.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Look {
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub shape: String,
    #[serde(default)]
    pub face: String,
}

/// What a bot is doing now, as facts (`bots.now`): how many of its sessions are at work, how many
/// wait on you, and, with neither, when the newest of them last changed (ms).
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct BotNow {
    #[serde(default)]
    pub working: usize,
    #[serde(default)]
    pub asking: usize,
    #[serde(default)]
    pub finished: Option<f64>,
}

/// One bot, as `kuvo/bots` lists it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Bot {
    pub id: String,
    pub session: String,
    pub name: String,
    pub look: Look,
    /// What it does, in a few words.
    #[serde(default)]
    pub job: String,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub machine: Option<String>,
    /// What it's doing now.
    #[serde(default)]
    pub now: BotNow,
}

/// The last thing said in a group chat.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GroupLast {
    pub time: f64,
}

/// One group chat, as `kuvo/groups` lists it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Group {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub discussing: bool,
    #[serde(default)]
    pub unread: bool,
    #[serde(default)]
    pub last: Option<GroupLast>,
}

// MARK: The Work Queue

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Category {
    pub id: String,
    pub name: String,
    #[serde(default, rename = "builtIn")]
    pub built_in: bool,
}

/// The Work Queue's counts across your computers, as the box counts them (`work-queue.counts`): its
/// items in progress, in each category (by its id), from each outside source (by its kind), on each
/// computer (by its key), and how many are deleted.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct QueueCounts {
    #[serde(default, rename = "inProgress")]
    pub in_progress: usize,
    #[serde(default)]
    pub categories: HashMap<String, usize>,
    #[serde(default)]
    pub sources: HashMap<String, usize>,
    #[serde(default)]
    pub computers: HashMap<String, usize>,
    #[serde(default)]
    pub removed: usize,
}

/// The Work Queue's categories, in the box's order, and its counts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkQueue {
    pub categories: Vec<Category>,
    pub counts: QueueCounts,
}

// MARK: Computers

/// One of your computers, as a box knows it (`kuvo/computers`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Computer {
    /// Its box's public key (`""` for the box itself while it has none).
    #[serde(default)]
    pub key: String,
    pub name: String,
    /// `mac` or `machine`.
    #[serde(default)]
    pub kind: String,
    /// The box that answered.
    #[serde(default)]
    pub here: bool,
    /// A machine's name on its Mac.
    #[serde(default)]
    pub id: Option<String>,
    /// A machine Macs of different setups share.
    #[serde(default)]
    pub shared: bool,
    /// `connected`, `connecting` or `away`.
    #[serde(default)]
    pub reach: String,
    /// Kuvo Code's version there, its engine's, and its system as people know it ("macOS 15.1").
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub engine: Option<String>,
    #[serde(default)]
    pub system: Option<String>,
    /// How the box that answers knows it: `here`, `relay`, or `peer` (`through` that one's name).
    #[serde(default)]
    pub via: String,
    #[serde(default)]
    pub through: Option<String>,
    /// How many of its sessions are at work.
    #[serde(default)]
    pub working: usize,
    /// The services whose plan limits it reads.
    #[serde(default)]
    pub limits: Vec<String>,
}

impl Computer {
    pub fn is_mac(&self) -> bool {
        self.kind == "mac"
    }
}

/// Your computers, the answering box first.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct Computers {
    #[serde(default, deserialize_with = "readable")]
    pub computers: Vec<Computer>,
}

// MARK: Plan limits

/// How close a share is to its limit, as the box says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Calm,
    Near,
    Over,
}

/// One of an account's two rings, as the box picks it (`limits.rings`): the window it shows, its
/// share and when it starts over, and its level.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Ring {
    /// The window's `id`.
    pub window: String,
    pub used: f32,
    #[serde(default, rename = "resetsAt")]
    pub resets_at: Option<f64>,
    pub level: Level,
}

/// An account's rings: the short window (a 5-hour session) outside, the long one (the week) inside;
/// one where its plan has one window, none where it has none.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Rings {
    #[serde(default, deserialize_with = "lenient")]
    pub short: Option<Ring>,
    #[serde(default, deserialize_with = "lenient")]
    pub long: Option<Ring>,
}

/// One window of a plan's limits.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LimitWindow {
    pub id: String,
    /// How much of it is gone, 0 to 1.
    pub used: f32,
    #[serde(default, rename = "resetsAt")]
    pub resets_at: Option<f64>,
    #[serde(default)]
    pub minutes: Option<u32>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    /// How close it is to its limit, in the box's word.
    pub level: Level,
}

/// A ring a newer box may say in a way this doesn't know: none, not the whole account.
fn lenient<'de, D: serde::Deserializer<'de>, T: serde::de::DeserializeOwned>(deserializer: D) -> std::result::Result<Option<T>, D::Error> {
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| serde_json::from_value(value).ok()))
}

/// An account across your computers, as the limits' `rows` list it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Account {
    /// Its mark and colour: `claude`, `codex`, `grok`, `cursor`, `antigravity`, `copilot`,
    /// `openrouter`, `zai`, `ollama`, `opencode`.
    pub service: String,
    /// The row's own name, where the box gives one (two accounts it tells apart).
    #[serde(default)]
    pub name: Option<String>,
    /// The coding agent whose sign-in it is (`claude`…); none for the engine's.
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub plan: Option<String>,
    #[serde(default, deserialize_with = "readable")]
    pub windows: Vec<LimitWindow>,
    /// The circles to draw for it, as the box picks them.
    #[serde(default)]
    pub rings: Rings,
}

/// A list's items this can read: one it can't is left out, not all of them (KuvoClient's `Readable`).
fn readable<'de, D: serde::Deserializer<'de>, T: serde::de::DeserializeOwned>(deserializer: D) -> std::result::Result<Vec<T>, D::Error> {
    let values = Vec::<serde_json::Value>::deserialize(deserializer)?;
    Ok(values.into_iter().filter_map(|value| serde_json::from_value(value).ok()).collect())
}

impl Account {
    /// Its name, as KuvoClient's `LimitsAccount.title` says it: the row's own, else its coding
    /// agent's or its service's, with what you called it ("Claude · Work").
    pub fn title(&self) -> String {
        if let Some(name) = &self.name {
            return name.clone();
        }
        let base = match &self.agent {
            Some(agent) => agent_name(agent).map(str::to_owned).unwrap_or_else(|| capitalized(agent)),
            None => service_name(&self.service).map(str::to_owned).unwrap_or_else(|| capitalized(&self.service)),
        };
        match self.label.as_deref().map(str::trim).filter(|label| !label.is_empty()) {
            Some(label) => format!("{base} · {label}"),
            None => base,
        }
    }

    /// The rings to draw, outside first, each as its share and level: the box's short ring, then
    /// its long one (one where it says one, none where none). One whose time has passed since it
    /// was read keeps its place, empty: what's used now isn't known (PROTOCOL.md, "Rings and levels").
    pub fn rings(&self, now: f64) -> Vec<(f32, Level)> {
        [&self.rings.short, &self.rings.long]
            .into_iter()
            .flatten()
            .map(|ring| (if ring.resets_at.is_some_and(|at| at <= now) { 0. } else { ring.used }, ring.level))
            .collect()
    }
}

impl LimitWindow {
    /// Its name, as KuvoClient's `LimitsWindow.title`: "Session", "Weekly", "Sonnet", "Premium requests".
    pub fn title(&self) -> String {
        if let Some(name) = self.name.as_ref().filter(|name| !name.is_empty()) {
            return name.clone();
        }
        if let Some(model) = self.model.as_ref().filter(|model| !model.is_empty()) {
            return model.clone();
        }
        match self.minutes {
            Some(300) => "Session".into(),
            Some(10_080) => "Weekly".into(),
            Some(minutes) if (40_000..=45_000).contains(&minutes) => "Monthly".into(),
            Some(minutes) if minutes >= 1_440 => format!("{}-day", minutes / 1_440),
            Some(minutes) if minutes >= 60 => format!("{}-hour", minutes / 60),
            _ => "Limit".into(),
        }
    }
}

/// The coding agents by their own names (KuvoClient's `AccountNames.agents`).
fn agent_name(agent: &str) -> Option<&'static str> {
    Some(match agent {
        "claude" => "Claude",
        "codex" => "Codex",
        "grok" => "Grok",
        "cursor" => "Cursor",
        "antigravity" => "Antigravity",
        _ => return None,
    })
}

/// The engine's services by their plans' names (KuvoClient's `AccountNames.services`).
fn service_name(service: &str) -> Option<&'static str> {
    Some(match service {
        "codex" => "ChatGPT",
        "grok" => "SuperGrok",
        "copilot" => "Copilot",
        "openrouter" => "OpenRouter",
        "zai" => "Z.ai",
        "ollama" => "Ollama",
        "opencode" => "OpenCode Go",
        _ => return None,
    })
}

/// Swift's `capitalized` for one word.
fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars.flat_map(char::to_lowercase)).collect()).unwrap_or_default()
}

// MARK: What waits on you

/// How many things wait on you in each area, across the box, as it counts them (`kuvo/waiting`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
pub struct Waiting {
    #[serde(default)]
    pub sessions: usize,
    #[serde(default)]
    pub bots: usize,
    #[serde(default)]
    pub queue: usize,
}

impl BoxClient {
    /// The bots that live on this box.
    pub fn bots(&self) -> Result<Vec<Bot>> {
        #[derive(Deserialize)]
        struct Answer {
            bots: Vec<serde_json::Value>,
        }
        let answer: Answer = self.get("kuvo/bots")?;
        // One a newer box lists in a shape this can't read is left out, not the whole list.
        Ok(answer.bots.into_iter().filter_map(|bot| serde_json::from_value(bot).ok()).collect())
    }

    /// The group chats that live on this box.
    pub fn groups(&self) -> Result<Vec<Group>> {
        #[derive(Deserialize)]
        struct Answer {
            groups: Vec<serde_json::Value>,
        }
        let answer: Answer = self.get("kuvo/groups")?;
        Ok(answer.groups.into_iter().filter_map(|group| serde_json::from_value(group).ok()).collect())
    }

    /// The Work Queue's categories, in the box's order, and its counts across your computers. Read
    /// at a glance: nothing is fetched from GitHub for a count.
    pub fn work_queue(&self) -> Result<WorkQueue> {
        #[derive(Deserialize)]
        struct Answer {
            #[serde(default)]
            categories: Vec<Category>,
            #[serde(default)]
            counts: QueueCounts,
        }
        let answer: Answer = self.get("kuvo/work-queue?owner=all&counts=1&glance=1")?;
        Ok(WorkQueue { categories: answer.categories, counts: answer.counts })
    }

    /// Your computers as one list, the box that answers first.
    pub fn computers(&self) -> Result<Vec<Computer>> {
        Ok(self.get::<Computers>("kuvo/computers")?.computers)
    }

    /// Follows your computers: the list now, and again as it changes.
    pub fn follow_computers(&self, timing: Timing) -> Follow<Followed<Computers>> {
        follow_lines(self, "kuvo-box computers", "kuvo/computers/events".into(), "kuvo/computers".into(), timing)
    }

    /// Every account across your computers, in the box's order (its limits' `rows`).
    pub fn accounts(&self) -> Result<Vec<Account>> {
        #[derive(Deserialize)]
        struct Data {
            #[serde(default)]
            rows: Vec<serde_json::Value>,
        }
        #[derive(Deserialize)]
        struct Answer {
            data: Data,
        }
        let answer: Answer = self.get("kuvo/agents/api/kuvo/limits")?;
        Ok(answer.data.rows.into_iter().filter_map(|row| serde_json::from_value(row).ok()).collect())
    }

    /// How many things wait on you in each area.
    pub fn waiting(&self) -> Result<Waiting> {
        self.get("kuvo/waiting")
    }

    /// Follows what waits on you: the counts now, and again as they change.
    pub fn follow_waiting(&self, timing: Timing) -> Follow<Followed<Waiting>> {
        follow_lines(self, "kuvo-box waiting", "kuvo/waiting/events".into(), "kuvo/waiting".into(), timing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bot_the_box_lists_in_an_unknown_shape_leaves_the_rest() {
        let bots: Vec<serde_json::Value> = serde_json::from_str(
            r#"[{"id":"bot_1","session":"ses_1","name":"Mira","look":{"color":"sky","shape":"round","face":"calm"},"job":"Looks after the website","paused":false,"watching":0,"agents":false,"folder":"/x","jobDetails":"","now":{"working":1,"asking":0}},
                {"id":"bot_2","name":"No session"}]"#,
        )
        .unwrap();
        let read: Vec<Bot> = bots.into_iter().filter_map(|bot| serde_json::from_value(bot).ok()).collect();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].now, BotNow { working: 1, asking: 0, finished: None });
    }

    #[test]
    fn a_window_in_a_shape_this_cant_read_is_left_out() {
        let row: Account = serde_json::from_str(r#"{"service":"claude","label":"Work","windows":[{"id":"five_hour","used":0.4,"read":1,"source":"usage","level":"calm","span":"short"},{"id":"newer"}],"rings":{"short":{"window":"five_hour","used":0.4,"level":"calm"}}}"#).unwrap();
        assert_eq!(row.windows.len(), 1);
        assert_eq!(row.windows[0].level, Level::Calm);
    }

    /// Held to KuvoClient's `LimitsAccount.title` and `LimitsWindow.title`.
    #[test]
    fn accounts_and_windows_are_named_as_kuvoclient_does() {
        let row = |json: &str| serde_json::from_str::<Account>(json).unwrap();
        assert_eq!(row(r#"{"service":"claude","agent":"claude","label":"Work"}"#).title(), "Claude · Work");
        assert_eq!(row(r#"{"service":"codex"}"#).title(), "ChatGPT", "the engine's ChatGPT sign-in");
        assert_eq!(row(r#"{"service":"codex","agent":"codex"}"#).title(), "Codex");
        assert_eq!(row(r#"{"service":"opencode"}"#).title(), "OpenCode Go");
        assert_eq!(row(r#"{"service":"claude","agent":"claude","name":"Claude (studio)","label":"Work"}"#).title(), "Claude (studio)");
        let window = |minutes: Option<u32>, model: Option<&str>, name: Option<&str>| LimitWindow { id: "w".into(), used: 0., resets_at: None, minutes, model: model.map(str::to_owned), name: name.map(str::to_owned), level: Level::Calm };
        assert_eq!(window(Some(300), None, None).title(), "Session");
        assert_eq!(window(Some(10_080), Some("Sonnet"), None).title(), "Sonnet");
        assert_eq!(window(Some(43_200), None, None).title(), "Monthly");
        assert_eq!(window(None, None, Some("Premium requests")).title(), "Premium requests");
    }

    /// The rings are the box's: its short one outside and its long one inside, each with its level;
    /// one where it says one, none where none. Nothing is picked or levelled here.
    #[test]
    fn the_rings_are_the_ones_the_box_says() {
        let row = |json: &str| serde_json::from_str::<Account>(json).unwrap();
        // The fullest window is a model's: it's no ring, as the box didn't pick it.
        let both = row(r#"{"service":"claude","windows":[{"id":"week","used":0.2,"level":"calm","span":"long"},{"id":"model","used":0.95,"level":"over"},{"id":"five","used":0.75,"level":"near","span":"short"}],"rings":{"short":{"window":"five","used":0.75,"level":"near"},"long":{"window":"week","used":0.2,"level":"calm"}}}"#);
        assert_eq!(both.rings(0.), vec![(0.75, Level::Near), (0.2, Level::Calm)], "the short window outside, the long inside");
        let one = row(r#"{"service":"grok","windows":[{"id":"week","used":0.2,"level":"calm","span":"long"}],"rings":{"long":{"window":"week","used":0.2,"level":"calm"}}}"#);
        assert_eq!(one.rings(0.), vec![(0.2, Level::Calm)]);
        let none = row(r#"{"service":"ollama","windows":[],"rings":{}}"#);
        assert!(none.rings(0.).is_empty());
        let started_over = row(r#"{"service":"codex","rings":{"short":{"window":"five","used":0.5,"resetsAt":10,"level":"calm"},"long":{"window":"week","used":0.2,"level":"calm"}}}"#);
        assert_eq!(started_over.rings(20.), vec![(0., Level::Calm), (0.2, Level::Calm)], "one that started over keeps its place, empty");
        // A ring a newer box says in a way this can't read leaves the account, with its other ring.
        let newer = row(r#"{"service":"claude","rings":{"short":{"window":"x","used":0.3,"level":"dire"},"long":{"window":"week","used":0.2,"level":"calm"}}}"#);
        assert_eq!(newer.rings(0.), vec![(0.2, Level::Calm)]);
    }

    #[test]
    fn the_work_queues_counts_and_your_computers_read_as_the_box_says_them() {
        let counts: QueueCounts = serde_json::from_str(r#"{"inProgress":2,"categories":{"next":6,"done":0},"sources":{"github":3},"computers":{"":5,"k1":4},"removed":1}"#).unwrap();
        assert_eq!((counts.in_progress, counts.categories["next"], counts.sources["github"], counts.computers["k1"], counts.removed), (2, 6, 3, 4, 1));
        let said: Computers = serde_json::from_str(
            r#"{"computers":[{"key":"","name":"Studio","kind":"mac","here":true,"shared":false,"reach":"connected","version":"1.0.0","engine":"2.3.0","system":"macOS 15.1","via":"here","working":2,"limits":["claude"]},
                {"key":"k1","name":"devbox","kind":"machine","id":"devbox","shared":true,"reach":"away","via":"peer","through":"Studio","working":0,"limits":[]},
                {"key":"k2"}]}"#,
        )
        .unwrap();
        assert_eq!(said.computers.len(), 2, "one in a shape this can't read is left out");
        assert!(said.computers[0].here && said.computers[0].is_mac());
        assert_eq!((said.computers[1].shared, said.computers[1].reach.as_str(), said.computers[1].through.as_deref()), (true, "away", Some("Studio")));
    }
}
