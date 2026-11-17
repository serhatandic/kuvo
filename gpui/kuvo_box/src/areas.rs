//! What the rail's areas list, as a box says it (PROTOCOL.md): its bots with what each is doing now
//! (`kuvo/bots`) and group chats (`kuvo/groups`), the Work Queue's counts
//! (`kuvo/work-queue` with `counts=1`), your computers (`kuvo/computers`), the plan limits of every
//! account across them with each one's rings and levels (`kuvo/agents/api/kuvo/limits`'s `rows`),
//! and how many things wait on you in each area (`kuvo/waiting`). The box counts, levels and picks
//! the rings; this reads them.

use serde::Deserialize;

use crate::client::BoxClient;
use crate::error::{Error, Result};
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
/// wait on you, and, with neither, when the newest of them last changed (ms); how many of those at
/// work are thinking and how many are blocked (stopped on an error or a limit you haven't seen);
/// and the one state the box says it's in, since when (`bots.state`).
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct BotNow {
    #[serde(default)]
    pub waking: bool,
    #[serde(default)]
    pub working: usize,
    #[serde(default)]
    pub asking: usize,
    #[serde(default)]
    pub finished: Option<f64>,
    #[serde(default)]
    pub thinking: usize,
    #[serde(default)]
    pub blocked: usize,
    /// What blocked the newest blocked one: `error` or `limit` (a word a newer box may add to).
    #[serde(default, rename = "blockedBy")]
    pub blocked_by: Option<String>,
    #[serde(default)]
    pub state: BotState,
    /// Since when (ms), as the box saw it change.
    #[serde(default)]
    pub since: Option<f64>,
}

/// The one state a box says a bot is in (`bots.now.state`): which it is, by its precedence, is the
/// box's, so every client draws the same bot the same way. (Offline isn't one: a box can't say it
/// can't be reached, so a client draws that itself.) One a newer box names is drawn idle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BotState {
    #[default]
    Idle,
    Working,
    Thinking,
    NeedsYou,
    Blocked,
    Done,
    Asleep,
}

impl<'de> Deserialize<'de> for BotState {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        Ok(match Option::<String>::deserialize(deserializer)?.as_deref() {
            Some("working") => BotState::Working,
            Some("thinking") => BotState::Thinking,
            Some("needsYou") => BotState::NeedsYou,
            Some("blocked") => BotState::Blocked,
            Some("done") => BotState::Done,
            Some("asleep") => BotState::Asleep,
            _ => BotState::Idle,
        })
    }
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
    pub wakes: Option<f64>,
    #[serde(default)]
    pub watching: usize,
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

/// A service named by the box.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct NamedService {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct ComputerNotice {
    pub title: String,
    pub detail: String,
}

/// One of your computers, as a box knows it (`kuvo/computers`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct Computer {
    #[serde(default, rename = "reachTitles")]
    pub reach_titles: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub notice: Option<ComputerNotice>,
    /// The same standing in words about its settings, for where they would show.
    #[serde(default, rename = "settingsNotice")]
    pub settings_notice: Option<ComputerNotice>,
    #[serde(default, rename = "workingTitle")]
    pub working_title: Option<String>,
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
    #[serde(default, rename = "limitServices")]
    pub limit_services: Vec<NamedService>,
    /// What its box has (its `kuvo/info` `features`), whole, for the update gate; none where it
    /// didn't say.
    #[serde(default)]
    pub features: Option<Vec<String>>,
    /// What can be done with it from this box now, in the box's order and words.
    #[serde(default, deserialize_with = "readable")]
    pub actions: Vec<ComputerAction>,
    /// Where its box is on the relay, and which relay (`computers.reach`): one of your own
    /// computers a paired device reaches directly. None for one that isn't yours.
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub relay: Option<String>,
    /// How its box stands with adding Kuvo Code's app to your devices (`devices.allow`), where the
    /// main computer says: blocked after its app was removed, until you allow it again.
    #[serde(default)]
    pub vouching: Option<Vouching>,
    /// Bringing its Kuvo up to this box's (`computers.update`), where the box that set it up over
    /// SSH says: how that stands, in its words. None: it's up to date, or no box here updates it.
    #[serde(default, deserialize_with = "readable_one")]
    pub update: Option<ComputerUpdate>,
    /// This Mac's own row: the newer Kuvo version this Mac runs than the desktop app that last
    /// opened with it, which checks for its own update then (once for each version).
    #[serde(default, rename = "appCheck")]
    pub app_check: Option<String>,
}

/// How bringing a computer's Kuvo up to date stands, as the box that does it says (every word the
/// box's, drawn as it is).
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct ComputerUpdate {
    /// `waiting` (it updates by itself once nothing's at work there), `updating` (under way now)
    /// or `failed` (it couldn't, and tries again later).
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub detail: String,
    /// It has waited a long while (over a day) for nothing to be at work there.
    #[serde(default)]
    pub long: bool,
}

impl ComputerUpdate {
    pub fn is_updating(&self) -> bool {
        self.state == "updating"
    }
}

/// Whether a computer's box may add the app beside it to your devices, in the box's words.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct Vouching {
    #[serde(default)]
    pub blocked: bool,
    #[serde(default)]
    pub words: String,
}

/** What can be done with a computer from the box that lists it, in the box's words. */
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ComputerAction {
    /// `rename`, `connect`, `remove`, `allow-devices`, `setup`, `uninstall`, `update` (brings
    /// its Kuvo up to this box's now: `POST kuvo/computers/<key>/update`) or, on the box's own
    /// row, `update-here` (it takes the release waiting for it now: `POST kuvo/update {now}`).
    pub id: String,
    pub label: String,
    /// Where it asks first, the box's question.
    #[serde(default)]
    pub confirm: Option<crate::accounts::Confirm>,
    /// It can't be done now: why, to show beside it.
    #[serde(default)]
    pub why: Option<String>,
}

/// Adding computers, as a box says it (`computers.add`, PROTOCOL.md "Computers", Adding one):
/// whether it adds them (`why`, in its words, where it doesn't), the hosts to suggest, and its
/// tries, those under way first.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct Adding {
    #[serde(default)]
    pub can: bool,
    #[serde(default)]
    pub why: Option<String>,
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default, deserialize_with = "readable")]
    pub tries: Vec<AddTry>,
}

/// A try at adding a computer over SSH, setting one up again or uninstalling Kuvo Code from one,
/// as its box runs it: every word is the box's.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct AddTry {
    pub id: String,
    /// `add`, `setup`, `uninstall` or `update`.
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub destination: String,
    #[serde(default)]
    pub name: String,
    /// `working`, `asking`, `stopping`, `failed` or `done`.
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub steps: Vec<AddStep>,
    #[serde(default)]
    pub question: Option<AddQuestion>,
    #[serde(default)]
    pub failure: Option<AddFailure>,
    /// The computer it came to.
    #[serde(default)]
    pub computer: Option<AddedComputer>,
    /// It was one of your computers already, at another address.
    #[serde(default)]
    pub known: bool,
}

impl AddTry {
    /// Still going: working, asking you, or stopping.
    pub fn under_way(&self) -> bool {
        !matches!(self.state.as_str(), "failed" | "done")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct AddStep {
    pub id: String,
    #[serde(default)]
    pub title: String,
    /// `waiting`, `now`, `done` or `failed`.
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub detail: Option<String>,
    /// How far a copy has got, in bytes.
    #[serde(default)]
    pub sent: Option<u64>,
    #[serde(default)]
    pub total: Option<u64>,
}

/// What a try asks before it goes on: facts to look at (a host key's fingerprint), and its answers.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct AddQuestion {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub message: String,
    /// This ask's number, which an answer names so one meant for an earlier ask isn't taken for it.
    #[serde(default)]
    pub at: u64,
    #[serde(default)]
    pub facts: Vec<AddFact>,
    #[serde(default)]
    pub answers: Vec<AddAnswer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct AddFact {
    pub label: String,
    #[serde(default)]
    pub value: String,
    /// Drawn in the code font (a fingerprint).
    #[serde(default)]
    pub code: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct AddAnswer {
    pub id: String,
    #[serde(default)]
    pub label: String,
    /// `primary`, `plain` or `danger`.
    #[serde(default)]
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct AddFailure {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub message: String,
    /// Trying again could go otherwise.
    #[serde(default)]
    pub again: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct AddedComputer {
    pub key: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Deserialize)]
struct Tried {
    #[serde(rename = "try")]
    tried: AddTry,
}

/// A session at work on a computer: its id there, and its title.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct AtWork {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
}

impl Computer {
    /// Display words for this app’s local reach state, supplied by kuvo/computers.
    pub fn reach_title(&self, state: &str) -> String {
        self.reach_titles.get(state).cloned().unwrap_or_default()
    }

    pub fn is_mac(&self) -> bool {
        self.kind == "mac"
    }

    /// Its box is one this client can't speak to at all (the update gate, PROTOCOL.md: it lacks
    /// the baseline), so it would hold it. Only where the list says what it has; one that doesn't
    /// say isn't called older. One that's only behind (`is_behind`) is used as it is.
    pub fn is_older(&self) -> bool {
        self.features.as_deref().is_some_and(|features| !crate::gate::missing_baseline(features).is_empty())
    }

    /// Its box lacks some of what this client draws: used as it is, its update offered.
    pub fn is_behind(&self) -> bool {
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
    /// Its plan isn't active now, as the box says (`ended`): no ring is drawn for it, and where
    /// accounts are listed it says its plan ended.
    #[serde(default)]
    pub ended: bool,
    /// The computers it's signed in on, as the box says (`boxes`).
    #[serde(default, deserialize_with = "readable")]
    pub boxes: Vec<RowBox>,
}

/// A computer an account is signed in on, as a row says it: `here`, the computer whose box answered.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RowBox {
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub here: bool,
}

/// A list's items this can read: one it can't is left out, not all of them (KuvoClient's `Readable`).
pub(crate) fn readable<'de, D: serde::Deserializer<'de>, T: serde::de::DeserializeOwned>(deserializer: D) -> std::result::Result<Vec<T>, D::Error> {
    let values = Vec::<serde_json::Value>::deserialize(deserializer)?;
    Ok(values.into_iter().filter_map(|value| serde_json::from_value(value).ok()).collect())
}

/// One value, or none where it isn't one this client reads (a newer box's shape).
pub(crate) fn readable_one<'de, D: serde::Deserializer<'de>, T: serde::de::DeserializeOwned>(deserializer: D) -> std::result::Result<Option<T>, D::Error> {
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| serde_json::from_value(value).ok()))
}

impl Account {
    /// The rings to draw, outside first, each as its share and level: the box's short ring, then
    /// its long one (one where it says one, none where none). One whose window started over is
    /// empty, in its place, as the box says it (PROTOCOL.md, "Rings and levels").
    pub fn rings(&self) -> Vec<(f32, Level)> {
        [&self.rings.short, &self.rings.long].into_iter().flatten().map(|ring| (ring.used, ring.level)).collect()
    }

    /// Signed in on the computer whose box answered (one of its `boxes` is `here`).
    pub fn here(&self) -> bool {
        self.boxes.iter().any(|each| each.here)
    }
}

// MARK: What waits on you

/// What's at work in Kuvo on a computer, as its box says it (`kuvo/box/at-work`): of it, what this
/// app reads: whether that box is shown in its computer's menu bar (a Mac's own box, run by Kuvo
/// Helper), how many of its sessions are at work, and whether anything would stop with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
pub struct HereAtWork {
    #[serde(default)]
    pub applies: bool,
    #[serde(default)]
    pub working: usize,
    #[serde(default)]
    pub busy: bool,
}

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

    /// Lets a computer's box add Kuvo Code's app there to your devices again, after its app was
    /// removed (asked of your setup's main computer).
    pub fn allow_devices(&self, key: &str) -> Result<()> {
        self.done(&Request::post(format!("kuvo/computers/{}/allow-devices", segment(key)), serde_json::json!({})))
    }

    /// Adding computers, as this box says it now (`computers.add`).
    pub fn adding(&self) -> Result<Adding> {
        self.get("kuvo/computers/add")
    }

    /// Follows it: now, and again as a try changes.
    pub fn follow_adding(&self, timing: Timing) -> Follow<Followed<Adding>> {
        follow_lines(self, "kuvo-box adding", "kuvo/computers/add/events".into(), "kuvo/computers/add".into(), timing)
    }

    /// Asks the box to add the computer at `destination` (a Host from its `~/.ssh/config`, a
    /// host's name or `user@host`): the try it began. Refused in its words.
    pub fn add_computer(&self, destination: &str) -> Result<AddTry> {
        Ok(self.value::<Tried>(&Request::post("kuvo/computers/add", serde_json::json!({ "destination": destination })))?.tried)
    }

    /// Answers what the try `id` asks (its ask numbered `at`), with one of its answers' ids.
    pub fn answer_add(&self, id: &str, at: u64, answer: &str) -> Result<AddTry> {
        Ok(self.value::<Tried>(&Request::post(format!("kuvo/computers/add/{}/answer", segment(id)), serde_json::json!({ "answer": answer, "at": at })))?.tried)
    }

    /// Stops the try `id`, or forgets one that's over.
    pub fn end_add(&self, id: &str) -> Result<()> {
        self.done(&Request::delete(format!("kuvo/computers/add/{}", segment(id))))
    }

    /// Sets a machine this box added over SSH up again, at the address it was added under.
    pub fn setup_computer(&self, key: &str) -> Result<AddTry> {
        Ok(self.value::<Tried>(&Request::post(format!("kuvo/computers/{}/setup", segment(key)), serde_json::json!({})))?.tried)
    }

    /// Brings the Kuvo of a machine this box set up over SSH up to its own now (`computers.update`),
    /// whatever is at work there: asked of the box that lists it, never of that machine's own.
    pub fn update_computer(&self, key: &str) -> Result<AddTry> {
        Ok(self.value::<Tried>(&Request::post(format!("kuvo/computers/{}/update", segment(key)), serde_json::json!({})))?.tried)
    }

    /// Brings this box itself up to the release waiting for it, now (`kuvo/update {now}`,
    /// `computers.update.here`: its own row's `update-here`), whatever is at work on it.
    pub fn update_here(&self) -> Result<()> {
        self.value::<serde_json::Value>(&Request::post("kuvo/update", serde_json::json!({ "now": true }))).map(|_| ())
    }

    /// Uninstalls Kuvo Code from a machine this box added over SSH, and removes it.
    pub fn uninstall_computer(&self, key: &str) -> Result<AddTry> {
        Ok(self.value::<Tried>(&Request::post(format!("kuvo/computers/{}/uninstall", segment(key)), serde_json::json!({})))?.tried)
    }

    /// Follows your computers: the list now, and again as it changes.
    pub fn follow_computers(&self, timing: Timing) -> Follow<Followed<Computers>> {
        follow_lines(self, "kuvo-box computers", "kuvo/computers/events".into(), "kuvo/computers".into(), timing)
    }

    /// Every account across your computers, in the box's order (its limits' `rows`).
    pub fn accounts(&self) -> Result<Vec<Account>> {
        #[derive(Deserialize)]
        struct Data {
            rows: Option<Vec<serde_json::Value>>,
        }
        #[derive(Deserialize)]
        struct Answer {
            data: Data,
        }
        let answer: Answer = self.get("kuvo/agents/api/kuvo/limits")?;
        // A coding agents' bridge from before `rows` (one still running after its box updated, until
        // it restarts) lists none across your computers: not "no accounts", but none it says.
        let rows = answer.data.rows.ok_or_else(|| Error::BadAnswer("This computer's Kuvo is from before it listed your accounts' limits: they show once it restarts.".into()))?;
        Ok(rows.into_iter().filter_map(|row| serde_json::from_value(row).ok()).collect())
    }

    /// What's at work in Kuvo on this box's computer, as it says it for its menu bar.
    pub fn at_work(&self) -> Result<HereAtWork> {
        self.get("kuvo/box/at-work")
    }

    /// Tells Kuvo on this Mac where this app is installed and its version (`kuvo/box/app`'s `desktop`
    /// and `version`), so the menu bar's Open Kuvo opens this very app, never an older copy with the
    /// same bundle ID, and Kuvo says when it runs a newer version than this app (`appCheck`). A box
    /// that doesn't take it (an older one, one in the app's folder) refuses; that's all.
    pub fn register_desktop(&self, bundle: &str, version: &str) -> Result<()> {
        let response = self.send(&Request::post("kuvo/box/app", serde_json::json!({ "desktop": bundle, "version": version })))?;
        if response.is_success() {
            Ok(())
        } else {
            Err(Error::Refused(crate::error::Refusal::from_answer(response.status, &response.body)))
        }
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
        assert_eq!(read[0].now, BotNow { working: 1, asking: 0, ..BotNow::default() });
    }

    /// The one state the box says a bot is in, with its facts (`bots.state`); a word a newer box
    /// adds is read as idle, and the rest of the bot stays.
    #[test]
    fn a_bots_state_is_read_as_the_box_says_it() {
        let now = |json: &str| serde_json::from_str::<BotNow>(json).unwrap();
        let blocked = now(r#"{"working":0,"asking":0,"blocked":1,"blockedBy":"limit","state":"blocked","since":5}"#);
        assert_eq!((blocked.state, blocked.blocked, blocked.blocked_by.as_deref(), blocked.since), (BotState::Blocked, 1, Some("limit"), Some(5.)));
        assert_eq!(now(r#"{"working":2,"asking":0,"thinking":2,"state":"thinking","since":1}"#).thinking, 2);
        assert_eq!(now(r#"{"working":0,"asking":0,"state":"napping","since":1}"#).state, BotState::Idle);
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
    fn computer_service_titles_are_the_box_words() {
        let computer: Computer = serde_json::from_str(r#"{"key":"","name":"Studio","limitServices":[{"id":"codex","title":"A plan named by the box"}]}"#).unwrap();
        assert_eq!(computer.limit_services[0].title, "A plan named by the box");
    }

    #[test]
    fn your_computers_read_as_the_box_says_them() {
        let said: Computers = serde_json::from_str(
            r#"{"computers":[{"key":"","name":"Studio","kind":"mac","here":true,"reach":"connected","version":"0.8.0","engine":"2.3.0","system":"macOS 15.1","via":"here","working":2,"limits":["claude"]},
                {"key":"k1","name":"devbox","kind":"machine","id":"devbox","reach":"unknown","via":"peer","through":"Studio","working":0,"limits":[]},
                {"key":"k2"}]}"#,
        )
        .unwrap();
        assert_eq!(said.computers.len(), 2, "one in a shape this can't read is left out");
        assert!(said.computers[0].here && said.computers[0].is_mac());
        assert_eq!((said.computers[1].reach.as_str(), said.computers[1].through.as_deref()), ("unknown", Some("Studio")));
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
