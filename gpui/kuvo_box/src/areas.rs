//! What the rail's areas list, as a box says it (PROTOCOL.md): its bots with what each is doing now
//! (`kuvo/bots`) and group chats (`kuvo/groups`), the Work Queue's counts
//! (`kuvo/work-queue` with `counts=1`), your computers (`kuvo/computers`), the plan limits of every
//! account across them with each one's rings and levels (`kuvo/agents/api/kuvo/limits`'s `rows`),
//! and how many things wait on you in each area (`kuvo/waiting`). The box counts, levels and picks
//! the rings; this reads them.

use serde::Deserialize;

use crate::client::BoxClient;
use crate::error::Result;
use crate::paths::segment;
use crate::transport::Request;
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
    /// All of its job (`job` is its line).
    #[serde(default, rename = "jobDetails")]
    pub job_details: String,
    /// Its personality, by id; none: the model's own way.
    #[serde(default)]
    pub personality: Option<String>,
    /// The model it runs on, where the box says one.
    #[serde(default, deserialize_with = "lenient")]
    pub model: Option<crate::typed::ModelRef>,
    /// The project it works in, as the box names it; none: a folder of its own.
    #[serde(default)]
    pub project: Option<String>,
    /// Its conversation is with the coding agents.
    #[serde(default)]
    pub agents: bool,
    /// What deleting it takes with it, in its box's words (`bots.deletes`).
    #[serde(default, deserialize_with = "readable")]
    pub deletes: Vec<DeleteLine>,
}

/// One thing deleting a bot does, in the words of the box that says it: by what it is (a word a
/// newer box may add to), and whether it stays rather than goes.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DeleteLine {
    #[serde(default)]
    pub what: String,
    pub words: String,
    #[serde(default)]
    pub kept: bool,
}

/// One group chat, as `kuvo/groups` lists it.
pub use crate::groups::Group;

// MARK: Computers

/// One of your computers, as a box knows it (`kuvo/computers`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
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
    /// It isn't one of your own computers (a shared machine, a box of another setup): filed apart.
    #[serde(default)]
    pub apart: bool,
    /// `connected`, `connecting`, `away`, or `unknown` (known by another computer's word alone).
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
    /// How many of its sessions are at work; none where it never said (an older box, one known
    /// only by another's word, one that isn't yours): nothing is claimed of it then.
    #[serde(default)]
    pub working: Option<usize>,
    /// Which of its sessions are at work, newest first (the first few), as it said them.
    #[serde(default, rename = "atWork", deserialize_with = "readable")]
    pub at_work: Vec<AtWork>,
    /// The services whose plan limits it reads.
    #[serde(default)]
    pub limits: Vec<String>,
    /// What its box has (its `kuvo/info` `features`), whole, for the update gate; none where it
    /// didn't say.
    #[serde(default)]
    pub features: Option<Vec<String>>,
    /// What can be done with it from this box now, in the box's order and words.
    #[serde(default, deserialize_with = "readable")]
    pub actions: Vec<ComputerAction>,
}

/** What can be done with a computer from the box that lists it, in the box's words. */
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ComputerAction {
    /// `rename`, `connect` or `remove`.
    pub id: String,
    pub label: String,
    /// Where it asks first, the box's question.
    #[serde(default)]
    pub confirm: Option<crate::accounts::Confirm>,
    /// It can't be done now: why, to show beside it.
    #[serde(default)]
    pub why: Option<String>,
}

/// A session at work on a computer: its id there, and its title.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct AtWork {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
}

impl Computer {
    pub fn is_mac(&self) -> bool {
        self.kind == "mac"
    }

    /// Its box is older than this client needs (the update gate, PROTOCOL.md): this client would
    /// hold it. Only where the list says what it has; one that doesn't say isn't called older.
    pub fn is_older(&self) -> bool {
        self.features.as_deref().is_some_and(|features| !crate::gate::missing(features).is_empty())
    }

    /// What the box offers to do with it, by id (`rename`, `connect`, `remove`).
    pub fn action(&self, id: &str) -> Option<&ComputerAction> {
        self.actions.iter().find(|action| action.id == id)
    }
}

/// Your computers, the answering box first.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct Computers {
    #[serde(default, deserialize_with = "readable")]
    pub computers: Vec<Computer>,
}

// MARK: Plan limits

/// How close a share is to its limit, as the box says it. A word a newer box names that this
/// doesn't know (or none) is `Unknown`, drawn quiet: the window stays, uncoloured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Calm,
    Near,
    Over,
    #[default]
    #[serde(other)]
    Unknown,
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
    #[serde(default)]
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
    #[serde(default)]
    pub level: Level,
    /// Its name, as the box says it (`title`): "Session", "Weekly", "Sonnet", "Premium requests".
    #[serde(default)]
    pub title: String,
    /// It started over since it was read, as the box says (`expired`): not read since, `used` 0.
    #[serde(default)]
    pub expired: bool,
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
    /// Its name, as the box says it (`title`): "Claude", "Claude · Work", "ChatGPT", told apart from
    /// another row of that name ("Claude · hetzner").
    #[serde(default)]
    pub title: String,
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
pub(crate) fn readable<'de, D: serde::Deserializer<'de>, T: serde::de::DeserializeOwned>(deserializer: D) -> std::result::Result<Vec<T>, D::Error> {
    let values = Vec::<serde_json::Value>::deserialize(deserializer)?;
    Ok(values.into_iter().filter_map(|value| serde_json::from_value(value).ok()).collect())
}

impl Account {
    /// The rings to draw, outside first, each as its share and level: the box's short ring, then
    /// its long one (one where it says one, none where none). One whose window started over is
    /// empty, in its place, as the box says it (PROTOCOL.md, "Rings and levels").
    pub fn rings(&self) -> Vec<(f32, Level)> {
        [&self.rings.short, &self.rings.long].into_iter().flatten().map(|ring| (ring.used, ring.level)).collect()
    }
}

/// The engine's services by their plans' names (KuvoClient's `AccountNames.services`), for the
/// services a computer reads the limits of (`kuvo/computers`' `limits`, which says only their ids).
pub fn service_name(service: &str) -> Option<&'static str> {
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

    /// Your computers as one list, the box that answers first.
    pub fn computers(&self) -> Result<Vec<Computer>> {
        Ok(self.get::<Computers>("kuvo/computers")?.computers)
    }

    /// Renames one of your machines (`computers.manage`): done by the box that writes its record,
    /// which the box asked hands it to; refused in its words.
    pub fn rename_computer(&self, key: &str, name: &str) -> Result<()> {
        self.done(&Request { method: "PATCH".into(), path: format!("kuvo/computers/{}", segment(key)), headers: Vec::new(), body: Some(serde_json::json!({ "name": name })) })
    }

    /// Removes one of your machines, as `rename_computer` renames it.
    pub fn remove_computer(&self, key: &str) -> Result<()> {
        self.done(&Request::delete(format!("kuvo/computers/{}", segment(key))))
    }

    /// Reaches one of your computers that's away again, from the box asked.
    pub fn connect_computer(&self, key: &str) -> Result<()> {
        self.done(&Request::post(format!("kuvo/computers/{}/connect", segment(key)), serde_json::json!({})))
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
        // A level a newer box names keeps its window, uncoloured.
        let newer: Account = serde_json::from_str(r#"{"service":"claude","windows":[{"id":"w","used":0.4,"level":"dire"}],"rings":{"short":{"window":"w","used":0.4,"level":"dire"}}}"#).unwrap();
        assert_eq!((newer.windows.len(), newer.windows[0].level, newer.rings.short.map(|ring| ring.level)), (1, Level::Unknown, Some(Level::Unknown)));
    }

    /// Every name is the box's (`limits.titles`): drawn as it says, with no table of its own.
    #[test]
    fn accounts_and_windows_are_named_as_the_box_says() {
        let row: Account = serde_json::from_str(r#"{"service":"claude","agent":"claude","label":"Work","title":"Claude · hetzner","windows":[{"id":"five_hour","title":"Session","used":0.4,"level":"calm"},{"id":"x","title":"Premium requests","used":0,"expired":true}]}"#).unwrap();
        assert_eq!(row.title, "Claude · hetzner");
        assert_eq!(row.windows.iter().map(|window| (window.title.as_str(), window.expired)).collect::<Vec<_>>(), vec![("Session", false), ("Premium requests", true)]);
    }

    /// The rings are the box's: its short one outside and its long one inside, each with its level;
    /// one where it says one, none where none. Nothing is picked or levelled here.
    #[test]
    fn the_rings_are_the_ones_the_box_says() {
        let row = |json: &str| serde_json::from_str::<Account>(json).unwrap();
        // The fullest window is a model's: it's no ring, as the box didn't pick it.
        let both = row(r#"{"service":"claude","windows":[{"id":"week","used":0.2,"level":"calm","span":"long"},{"id":"model","used":0.95,"level":"over"},{"id":"five","used":0.75,"level":"near","span":"short"}],"rings":{"short":{"window":"five","used":0.75,"level":"near"},"long":{"window":"week","used":0.2,"level":"calm"}}}"#);
        assert_eq!(both.rings(), vec![(0.75, Level::Near), (0.2, Level::Calm)], "the short window outside, the long inside");
        let one = row(r#"{"service":"grok","windows":[{"id":"week","used":0.2,"level":"calm","span":"long"}],"rings":{"long":{"window":"week","used":0.2,"level":"calm"}}}"#);
        assert_eq!(one.rings(), vec![(0.2, Level::Calm)]);
        let none = row(r#"{"service":"ollama","windows":[],"rings":{}}"#);
        assert!(none.rings().is_empty());
        let started_over = row(r#"{"service":"codex","rings":{"short":{"window":"five","used":0,"level":"calm"},"long":{"window":"week","used":0.2,"level":"calm"}}}"#);
        assert_eq!(started_over.rings(), vec![(0., Level::Calm), (0.2, Level::Calm)], "one that started over keeps its place, empty, as the box says");
        // A level a newer box names keeps its ring, uncoloured; a ring this can't read at all
        // leaves the account, with its other ring.
        let newer = row(r#"{"service":"claude","rings":{"short":{"window":"x","used":0.3,"level":"dire"},"long":{"window":"week","used":0.2,"level":"calm"}}}"#);
        assert_eq!(newer.rings(), vec![(0.3, Level::Unknown), (0.2, Level::Calm)]);
        let unread = row(r#"{"service":"claude","rings":{"short":{"window":"x","used":"most"},"long":{"window":"week","used":0.2,"level":"calm"}}}"#);
        assert_eq!(unread.rings(), vec![(0.2, Level::Calm)]);
    }

    #[test]
    fn your_computers_read_as_the_box_says_them() {
        let said: Computers = serde_json::from_str(
            r#"{"computers":[{"key":"","name":"Studio","kind":"mac","here":true,"shared":false,"reach":"connected","version":"0.8.0","engine":"2.3.0","system":"macOS 15.1","via":"here","working":2,"limits":["claude"]},
                {"key":"k1","name":"devbox","kind":"machine","id":"devbox","shared":true,"reach":"unknown","via":"peer","through":"Studio","working":0,"limits":[]},
                {"key":"k2"}]}"#,
        )
        .unwrap();
        assert_eq!(said.computers.len(), 2, "one in a shape this can't read is left out");
        assert!(said.computers[0].here && said.computers[0].is_mac());
        assert_eq!((said.computers[1].shared, said.computers[1].reach.as_str(), said.computers[1].through.as_deref()), (true, "unknown", Some("Studio")));
        // What's at work is said where the box said it; one that never said has no count at all.
        assert_eq!(said.computers[0].working, Some(2));
        let facts: Computer = serde_json::from_str(r#"{"key":"k3","name":"mini","kind":"machine","reach":"connected","via":"relay","atWork":[{"id":"ses_1","title":"Ship it"},{"id":"ses_2"},{"title":"no id"}],"actions":[{"id":"connect","label":"Connect"},{"label":"no id"}],"limits":[]}"#).unwrap();
        assert_eq!((facts.working, facts.at_work.len(), facts.at_work[1].title.as_deref()), (None, 2, None));
        assert_eq!((facts.actions.len(), facts.action("connect").map(|action| action.label.as_str())), (1, Some("Connect")));
        let waits: Computer = serde_json::from_str(r#"{"key":"k4","name":"attic","actions":[{"id":"remove","label":"Remove","why":"Quit the old app first."}]}"#).unwrap();
        assert_eq!(waits.action("remove").and_then(|action| action.why.as_deref()), Some("Quit the old app first."));
    }

    /// A computer is older only where the list says what it has, and lacks something the gate needs.
    #[test]
    fn a_computer_is_older_only_where_the_list_says_it_lacks_what_the_gate_needs() {
        let computer = |features: Option<Vec<String>>| super::Computer { name: "devbox".into(), features, ..Default::default() };
        assert!(!computer(None).is_older(), "one that doesn't say isn't called older");
        assert!(computer(Some(vec!["typed".into()])).is_older());
        assert!(!computer(Some(crate::gate::REQUIRED.iter().map(|name| (*name).to_owned()).collect())).is_older());
    }
}
