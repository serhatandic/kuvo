//! A stand-in box, for running the app, its shots and its tests without a real one: a made-up Mac's
//! box with realistic sessions in a few projects and chats, conversations with tool steps and code,
//! one session at work, and enough history in one to page back through. It lives in this process, as
//! a [`Transport`] answering the routes the app uses the way a box answers them (its session list
//! and its events, the engine's messages and events, `typed`, `send`, the models), so the app's
//! client code runs as it would against a box. Nothing real is reached: no file, process or port.
//!
//! With [`Replies::Live`] (the app's `--stand-in`), a message sent gets an answer streamed back a
//! word at a time, as the engine streams one. Tests use [`Replies::Never`], so nothing happens on a
//! clock, and read what was sent with [`StandIn::seen`].

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufRead, Read};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub use crate::time::now_ms;

pub mod areas;
pub mod bots;
pub mod browser;
pub mod first_run;
pub mod permissions;
pub mod git;
pub mod groups;
pub mod moves;
pub mod insights;
mod library;
pub mod marketplace;
pub mod panel;
pub mod queue;
pub mod settings;
pub mod terminals;
pub mod work_bars;

use kuvo_box::Result;
use kuvo_box::transport::{Closer, Request, Response, Socket, SocketMessage, SocketOut, Stream, Transport};
use serde_json::{Value, json};

/// Whether a message sent gets an answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Replies {
    /// Streamed back on a timer, as an engine would (the app run against the stand-in).
    Live,
    /// None: nothing happens by itself (tests and shots).
    Never,
}

/// A request the stand-in was sent.
#[derive(Clone, Debug, PartialEq)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub body: Option<Value>,
}

/// What a conversation's follower follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Followed {
    Wakes,
    Outputs,
    Asks,
    Queue,
}

/// The stand-in box. Cheap to clone: clones share it.
#[derive(Clone)]
pub struct StandIn {
    inner: Arc<Inner>,
}

struct Inner {
    state: Mutex<State>,
    replies: Replies,
}

#[derive(Default)]
struct State {
    at: u64,
    /// It stands in for a box on another computer.
    elsewhere: bool,
    /// Its name, where it isn't the sample's own.
    name: Option<String>,
    /// The boxes of your other computers it reaches, by key.
    peers: HashMap<String, StandIn>,
    /// It serves the setup's lists (`line.setup-lists`): its own rows and its peers'.
    setup_lists: bool,
    delivery_enabled: bool,
    /// The newest page it carried of each conversation on another computer, by computer and
    /// session: answered, as its last copy, while that computer is away (`line.kept-conversation`).
    carried: HashMap<(String, String), Value>,
    delivery_waiting: Vec<(String, String, Value)>,
    delivery_done: HashSet<String>,
    /// How each peer stands in the setup's lists, by key (none: answering): away since when, and
    /// whether only to switch to a new release.
    setup_standing: HashMap<String, (f64, bool)>,
    /// Rows of computers it keeps a last copy of but reaches no box of, as the setup's list says
    /// them (`kuvo/setup/sessions/events` sessions).
    setup_kept: Vec<Value>,
    older: bool,
    move_scenario: Option<moves::Scenario>,
    /// What its `kuvo/info` leaves out of what this app draws (a box behind on those features).
    lacks: Vec<String>,
    /// It doesn't answer (asleep, off, offline): every request fails.
    down: bool,
    /// It's from before it took the release its app carries the moment its work ended, saying how
    /// that stands itself (no `computers.update.here`): the app brings it up with its installer.
    before_update_here: bool,
    sessions_down: bool,
    sessions: BTreeMap<String, Value>,
    projects: BTreeMap<String, Value>,
    /// Each conversation's engine messages, oldest first.
    messages: HashMap<String, Vec<Value>>,
    running: HashSet<String>,
    models: HashMap<String, Value>,
    /// Open followed routes: the session list's and the engine's events, by an id of their own.
    lists: HashMap<u64, Sender<String>>,
    /// Those following what waits on you (`kuvo/waiting/events`), told as the list changes; and
    /// those following your computers (`kuvo/computers/events`), which stay as they are.
    waits: HashMap<u64, Sender<String>>,
    computers: HashMap<u64, Sender<String>>,
    usage_followers: HashMap<u64, Sender<String>>,
    /// Those following a conversation's asks or queue (`…/asks/events`, `…/queue/events`): its
    /// session, which, and where its lines go.
    followers: HashMap<u64, (String, Followed, Sender<String>)>,
    events: HashMap<u64, Sender<String>>,
    next: u64,
    seen: Vec<Seen>,
    /// Sends answered as an engine that's gone away answers them (`503`).
    sends_fail: bool,
    /// Files on its computer that `kuvo/file` serves, by path.
    files: HashMap<String, Vec<u8>>,
    /// Its copy of a message sent leaves out the files that came with it (a box that lost them).
    drops_files: bool,
    /// No model runs here: a new session starts on none.
    nothing_runs: bool,
    /// What it says of a session's model, where a test has said otherwise.
    model_states: HashMap<String, Value>,
    /// What each session's `data.context` says (`{used, window?, summarizeAt?}`), where a test said.
    contexts: HashMap<String, Value>,
    summarize: HashMap<String, Value>,
    /// The coding agents' bridge can't say its models now (it's starting again).
    agents_unknown: bool,
    /// Your starred models, as `kuvo/models/starred` keeps them.
    starred: Vec<Value>,
    /// More models besides, from other providers (a box with hundreds: shots and timings).
    more_models: usize,
    /// Your Claude Code accounts (name, plan), the agent's own sign-in first; none: as before.
    claude_accounts: Vec<(String, Option<String>)>,
    /// It keeps no stars (a box before `models.starred`).
    no_stars: bool,
    /// Codex is signed in: its models, with their speeds, are offered too.
    codex: bool,
    /// A model with a long name is offered too (a tight composer's chip).
    long_name: bool,
    /// The speed picked for each provider (`kuvo/models/speed`; null: Standard).
    speeds: BTreeMap<String, Value>,
    /// Its sessions' models can't be read now (`503`).
    models_fail: bool,
    /// What each conversation waits on you for, as the box words it (`kuvo/sessions/<id>/asks`).
    asks: HashMap<String, Vec<Value>>,
    /// Each conversation's queue, in the order the box shows it (`kuvo/sessions/<id>/queue`).
    queues: HashMap<String, Vec<Value>>,
    /// The files each message waiting went with, as they were sent (`{uri, name}`), by its id.
    queued_files: HashMap<String, Vec<Value>>,
    /// The conversations whose queue a Stop holds until Resume.
    paused: HashSet<String>,
    /// Why each session's queue waits, as its box says it (an engine being updated).
    queue_lines: HashMap<String, String>,
    /// Steer and Send Now as each was asked (`steer <id>`, `now <id>`), in order.
    queue_asked: Vec<String>,
    /// Its repositories and folders.
    git: Option<git::Git>,
    /// Those following a folder's changed files (`kuvo/git/files/events`): the folder, and where its
    /// lines go.
    git_followers: HashMap<u64, (String, Sender<String>)>,
    /// How it answers for the rail's areas, and the moment its samples are made at.
    areas: areas::Areas,
    /// What was handed over (`kuvo/library`).
    library: library::Library,
    /// The Marketplace: the catalog, and what the setup added (`kuvo/marketplace`).
    marketplace: marketplace::Market,
    made: Option<f64>,
    /// Its accounts, sign-ins and settings.
    settings: settings::Settings,
    /// Its bots.
    bots: bots::Bots,
    wakes: HashMap<String, Value>,
    outputs: HashMap<String, Vec<Value>>,
    output_bytes: HashMap<String, Vec<u8>>,
    /// Its Work Queue.
    queue: queue::Queue,
    /// Its group chats, and those following one (`kuvo/groups/<id>/events`): the chat, and where
    /// its lines go.
    groups: groups::Groups,
    group_followers: HashMap<u64, (String, Sender<String>)>,
    /// Those following the chats' list (`kuvo/groups/events`).
    groups_followers: HashMap<u64, Sender<String>>,
    /// Those following its bots (`kuvo/bots/events`).
    bots_followers: HashMap<u64, Sender<String>>,
    /// What it says for a first run.
    first_run: first_run::FirstRun,
    permissions: permissions::Permissions,
    /// Sessions currently held because they are moving to another computer, with the box's words.
    held_sessions: HashMap<String, String>,
    /// Sessions archived by the engine itself (`time.archived`), which only it can unarchive.
    engine_archived: HashSet<String>,
    /// Removed worktrees whose uncommitted changes were kept for Restore Worktree.
    kept_worktree_changes: HashSet<String>,
    /// Sessions with a manual compaction queued to run when the current turn ends.
    pending_compacts: HashSet<String>,
    /// Its modes, and what runs for each conversation besides its turn.
    work_bars: work_bars::WorkBars,
    /// The side panel's own answers: files found, a command's output, subagents, side chats.
    panel: panel::Panel,
    /// How each new session was asked to start: in a worktree, and its mode.
    starts: Vec<Value>,
    /// Its engine's terminals.
    terminals: terminals::Terminals,
    /// Its browser, live (a machine's box's).
    browser: browser::Browser,
}

pub const NAME: &str = "Stand-in Mac";
pub const HOME: &str = "/Users/sam";


const MINUTE: f64 = 60_000.0;
const HOUR: f64 = 60.0 * MINUTE;
const DAY: f64 = 24.0 * HOUR;

impl StandIn {
    /// The stand-in with its sample sessions, as they'd be `now` (ms since 1970).
    pub fn new(replies: Replies) -> StandIn {
        StandIn::at(replies, now_ms())
    }

    pub fn at(replies: Replies, now: f64) -> StandIn {
        let stand_in = StandIn { inner: Arc::new(Inner { state: Mutex::new(State { at: 1, next: 1, ..State::default() }), replies }) };
        stand_in.fill(now);
        let devbox = StandIn::machine(replies, "devbox");
        devbox.fill_machine(now);
        stand_in.state().peers.insert("key_devbox".into(), devbox);
        stand_in
    }

    /// A running summary for the screenshot fixture.
    pub fn running_compaction(&self) {
        let mut state = self.state();
        state.running.insert("ses_recipes".into());
        state.sessions.get_mut("ses_recipes").unwrap()["running"] = json!(true);
        state.messages.get_mut("ses_recipes").unwrap().push(json!({
            "type": "compaction", "id": "msg_cmp_running", "status": "running",
            "time": {"created": 1_794_650_000_000.0}, "title": "Summarizing the conversation…",
            "workingTitle": "Summarizing the conversation…"
        }));
    }

    /// What runs on the stand-in's machine: a project with a lead's session at work and one
    /// finished, each with a short conversation.
    fn fill_machine(&self, now: f64) {
        let mut state = self.state();
        state.made = Some(now);
        let billing = "/home/sam/billing".to_owned();
        state.projects.insert(billing.clone(), json!({ "name": "Billing", "updated": now - 4.0 * MINUTE }));
        for (id, title, ago, running) in [("ses_releases", "Plan this week’s releases", 4.0 * MINUTE, true), ("ses_invoices", "Reconcile the invoices export", 6.0 * HOUR, false)] {
            let mut facts = json!({ "kind": "project", "directory": billing, "project": billing, "title": title, "created": now - ago - HOUR, "updated": now - ago });
            if running {
                facts["running"] = json!(true);
                state.running.insert(id.to_owned());
            }
            state.sessions.insert(id.to_owned(), facts);
            state.models.insert(id.to_owned(), json!({ "providerID": "anthropic", "id": "claude-opus-5-5", "variant": "high" }));
        }
        let t = now - 10.0 * MINUTE;
        state.messages.insert(
            "ses_releases".into(),
            vec![
                user("msg_d1", t, "What’s left before Thursday’s release?", None),
                json!({ "type": "assistant", "id": "msg_d2", "time": { "created": t + 1000.0, "completed": t + 9000.0 }, "agent": "build", "finish": "stop", "content": [
                    { "type": "text", "text": "Three things are still open:\n\n- the invoice export’s rounding fix, in review\n- the migration for the new tax table\n- release notes" }
                ] }),
            ],
        );
        let t = now - 7.0 * HOUR;
        state.messages.insert(
            "ses_invoices".into(),
            vec![
                user("msg_i1", t, "Does the export match the ledger for October?", None),
                json!({ "type": "assistant", "id": "msg_i2", "time": { "created": t + 1000.0, "completed": t + 9000.0 }, "agent": "build", "finish": "stop", "content": [{ "type": "text", "text": "It does now: 1,204 rows on both sides, and the totals agree to the cent." }] }),
                json!({ "type": "idle", "id": "msg_i3", "time": { "created": t + 9000.0 }, "outcome": "succeeded" }),
            ],
        );
    }

    /// One of your machines' boxes, as this app reaches it from another computer: a Linux
    /// machine with settings of its own (its sandbox, its folders, one account).
    fn machine(replies: Replies, name: &str) -> StandIn {
        let stand_in = Self::empty(replies);
        {
            let mut state = stand_in.state();
            state.elsewhere = true;
            state.name = Some(name.to_owned());
            state.settings.set_allowed(vec![json!({ "id": "alw_srv", "kind": "write", "path": "/srv/cache/billing", "at": 1_794_000_000_000.0_f64 }), json!({ "id": "alw_notes", "kind": "write", "path": "/home/sam/notes", "gone": true, "at": 1_793_000_000_000.0_f64 })]);
            state.settings.set_browser("searchEngine", json!("google"));
            state.settings.set_browser("never", json!([]));
        }
        stand_in
    }

    /// It serves the setup's lists across your computers (`line.setup-lists`), as a box of this
    /// release does: its own rows, and its other computers'.
    pub fn share_setup_lists(&self) {
        self.state().setup_lists = true;
    }

    /// The local box accepts sends even when their owner isn't reachable.
    pub fn enable_delivery(&self) {
        self.state().delivery_enabled = true;
    }

    /// One round of the stand-in box's background delivery, advanced by the test.
    pub fn delivery_round(&self) {
        loop {
            let (computer, session, draft, peer) = {
                let state = self.state();
                let Some((computer, session, draft)) = state.delivery_waiting.first() else { return; };
                let Some(peer) = state.peers.get(computer) else { return; };
                (computer.clone(), session.clone(), draft.clone(), peer.clone())
            };
            let sent = Transport::send(&peer, &Request::post(format!("kuvo/sessions/{session}/send"), json!({
                "id": draft["id"], "words": draft["text"], "delivery": draft["message"]["delivery"],
            })));
            if !sent.is_ok_and(|response| response.is_success()) { return; }
            let mut state = self.state();
            state.delivery_done.insert(draft["id"].as_str().unwrap().to_owned());
            state.delivery_waiting.retain(|(key, _, saved)| key != &computer || saved["id"] != draft["id"]);
        }
    }

    /// Another computer in the setup's lists goes away (`since`: when its last copy came), only
    /// to switch to a new release where `updating`.
    pub fn setup_away(&self, key: &str, since: f64, updating: bool) {
        self.state().setup_standing.insert(key.to_owned(), (since, updating));
    }

    /// A computer it reaches no box of, as its last copy in the setup's lists has it: these sessions.
    pub fn setup_kept(&self, rows: Vec<Value>) {
        self.state().setup_kept = rows;
    }

    /// The setup's sessions and projects, as `kuvo/setup/sessions/events` says them.
    fn setup_sessions(&self) -> Value {
        let state = self.state();
        let here = json!({ "id": "key_stand_in", "name": state.name() });
        let rows = |line: &Value, computer: &Value, standing: Option<(f64, bool)>, local: bool| -> (Vec<Value>, Vec<Value>) {
            // (As a box says it: one only switching to a new release isn't away, but updating.)
            let away = standing.is_some_and(|(_, updating)| !updating);
            let mut origin = json!({ "computer": computer, "away": away });
            match standing {
                Some((_, true)) => origin["updating"] = json!(true),
                Some((since, false)) => origin["lastSeen"] = json!(since),
                None => {}
            }
            if local {
                origin["here"] = json!(true);
            }
            let with = |mut row: Value| {
                for (key, value) in origin.as_object().unwrap() {
                    row[key] = value.clone();
                }
                row
            };
            let sessions = line["sessions"].as_object().into_iter().flatten().map(|(id, facts)| with(json!({ "id": id, "facts": facts, "actions": if away { json!([]) } else { json!(["open", "manage"]) } }))).collect();
            let projects = line["projects"].as_object().into_iter().flatten().map(|(directory, facts)| with(json!({ "directory": directory, "facts": facts }))).collect();
            (sessions, projects)
        };
        let (mut sessions, mut projects) = rows(&state.line(true), &here, None, true);
        let peers: Vec<(String, StandIn)> = state.peers.iter().map(|(key, peer)| (key.clone(), peer.clone())).collect();
        let standing = state.setup_standing.clone();
        sessions.extend(state.setup_kept.iter().cloned());
        drop(state);
        for (key, peer) in peers {
            let (line, name) = { let peer = peer.state(); (peer.line(true), peer.name()) };
            let (more, more_projects) = rows(&line, &json!({ "id": key, "name": name }), standing.get(&key).copied(), false);
            sessions.extend(more);
            projects.extend(more_projects);
        }
        json!({ "sessions": sessions, "projects": projects })
    }

    /// Where a new session can run, as `kuvo/setup/runs-on/events` says it: this computer, then
    /// its peers, an away one with when it was last seen.
    fn runs_on(&self) -> Value {
        let state = self.state();
        let mut computers = vec![json!({ "computer": { "id": "key_stand_in", "name": state.name() }, "here": true, "offered": { "models": [] }, "agents": [] })];
        let mut peers: Vec<(String, StandIn)> = state.peers.iter().map(|(key, peer)| (key.clone(), peer.clone())).collect();
        peers.sort_by(|a, b| a.0.cmp(&b.0));
        let standing = state.setup_standing.clone();
        drop(state);
        for (key, peer) in peers {
            let name = peer.state().name();
            let mut row = json!({ "computer": { "id": key, "name": name }, "offered": { "models": [] }, "agents": [] });
            if let Some((since, updating)) = standing.get(&key) {
                row["away"] = json!(true);
                row["lastSeen"] = json!(since);
                if *updating {
                    row["updating"] = json!(true);
                }
            }
            computers.push(row);
        }
        json!({ "computers": computers })
    }

    fn merged_sessions(&self) -> Value {
        let mut listing = self.state().line(true);
        let rows = self.setup_sessions();
        let on = |row: &Value, actions: Value| json!({
            "id": row["computer"]["id"], "name": row["computer"]["name"], "away": row["away"].as_bool().unwrap_or_default(),
            "lastSeen": row["lastSeen"], "updating": row["updating"].as_bool().unwrap_or_default(), "actions": actions,
        });
        for row in rows["projects"].as_array().into_iter().flatten().filter(|r| r["here"] != true) {
            let (computer, directory) = (row["computer"]["id"].as_str().unwrap(), row["directory"].as_str().unwrap());
            let mut facts = row["facts"].clone();
            if let Some(fields) = facts.as_object_mut() {
                for key in ["shown", "total", "more"] { fields.remove(key); }
            }
            facts["on"] = on(row, json!([]));
            listing["projects"][format!("{computer}:{directory}")] = facts;
        }
        for row in rows["sessions"].as_array().into_iter().flatten().filter(|r| r["here"] != true) {
            let id = row["id"].as_str().unwrap();
            if !listing["sessions"][id].is_null() { continue; }
            let mut facts = row["facts"].clone();
            if facts["kind"] == "project" {
                let directory = facts["project"].as_str().or_else(|| facts["directory"].as_str()).unwrap();
                let project = format!("{}:{directory}", row["computer"]["id"].as_str().unwrap());
                if listing["projects"][&project].is_null() {
                    listing["projects"][&project] = json!({ "name": directory.rsplit('/').next(), "on": on(row, json!([])) });
                }
                facts["project"] = json!(project);
            }
            facts["on"] = on(row, row["actions"].as_array().map_or_else(|| json!(["open", "manage"]), |actions| json!(actions)));
            listing["sessions"][id] = facts;
        }
        listing
    }

    /// The box of another of your computers, as this one reaches it (`key`: its key in
    /// `kuvo/computers`): what a test changes or reads there.
    pub fn peer_box(&self, key: &str) -> Option<StandIn> {
        self.state().peers.get(key).cloned()
    }

    /// It reaches none of your other computers' boxes, as the app does for now against a real one.
    pub fn forget_peers(&self) {
        self.state().peers.clear();
    }

    /// Whether it can say its settings' sections (else 503, as a box that isn't answering).
    pub fn settings_readable(&self, readable: bool) {
        self.state().settings.pages_down = !readable;
    }

    /// A section added to its settings (`kuvo/settings`), with its page.
    pub fn add_settings_section(&self, section: Value, page: Value) {
        self.state().settings.add_section(section, page);
    }

    /// Its MCP Servers section is listed (`kuvo/settings/mcp`).
    pub fn show_mcp(&self) {
        self.state().settings.mcp.on = true;
    }

    /// Its Agents section is listed (`kuvo/settings/agents`), as the real box answered it.
    pub fn show_agents(&self) {
        self.state().settings.agent_settings.on = true;
    }

    /// Its Vault is listed (`kuvo/settings/vault`), as the real box answered it for a machine.
    pub fn show_vault(&self) {
        self.state().settings.vault.on = true;
    }

    /// Its Vault's state, to change or read (locked, what its forms sent, what Show read).
    pub fn with_vault<T>(&self, work: impl FnOnce(&mut crate::stand_in::settings::vault::Vault) -> T) -> T {
        work(&mut self.state().settings.vault)
    }

    /// Its MCP servers' state, to change or read (a sign-in going through, what its forms sent).
    pub fn with_mcp<T>(&self, work: impl FnOnce(&mut crate::stand_in::settings::Settings) -> T) -> T {
        work(&mut self.state().settings)
    }

    /// A browser setting changes, as from another client meanwhile.
    pub fn set_browser_setting(&self, key: &str, value: Value) {
        self.state().settings.set_browser(key, value);
    }

    /// Removes projects and sessions from a fixture while keeping its identity and peers.
    pub fn clear_projects_and_sessions(&self) {
        let mut state = self.state();
        state.areas.empty = true;
        state.projects.clear();
        state.sessions.clear();
        state.messages.clear();
        state.running.clear();
    }

    /// An empty box: no projects, no sessions.
    pub fn empty(replies: Replies) -> StandIn {
        let areas = areas::Areas { empty: true, ..areas::Areas::default() };
        StandIn { inner: Arc::new(Inner { state: Mutex::new(State { at: 1, next: 1, areas, queue: queue::Queue::none(), ..State::default() }), replies }) }
    }

    /// A box whose first run hasn't been had (`welcomed: false`), standing as `scenario` says: a
    /// new computer with nothing on it, or the sample box with its sessions and your computers.
    pub fn first_run(replies: Replies, now: f64, scenario: first_run::Scenario) -> StandIn {
        use first_run::Scenario;
        let stand_in = if scenario == Scenario::Nothing { Self::empty(replies) } else { Self::at(replies, now) };
        {
            let mut state = stand_in.state();
            state.made = Some(now);
            state.settings.finishes = replies == Replies::Live;
            state.first_run.welcomed = false;
            state.first_run.free = true;
            match scenario {
                Scenario::Nothing => {
                    state.first_run.only_free = true;
                    state.first_run.no_folders = true;
                    state.settings.nothing_installed();
                }
                Scenario::Crowded => {}
                Scenario::Running => state.settings.all_well(),
            }
        }
        stand_in
    }

    /// Whether its first run's been had, as it says now.
    pub fn welcomed(&self) -> bool {
        self.state().first_run.welcomed
    }

    /// Changes what it says for a first run.
    pub fn set_first_run(&self, change: impl FnOnce(&mut first_run::FirstRun)) {
        change(&mut self.state().first_run);
    }

    /// What it says macOS allows the program it runs under, from now on.
    /// How its computer stands, changed (`kuvo/settings/computer`).
    pub fn set_computer(&self, change: impl FnOnce(&mut settings::Computer)) {
        change(&mut self.state().settings.computer);
    }

    /// How its computer stands now.
    pub fn computer(&self) -> settings::Computer {
        self.state().settings.computer.clone()
    }

    pub fn set_permissions(&self, change: impl FnOnce(&mut permissions::Permissions)) {
        change(&mut self.state().permissions);
    }

    /// A box from before the features required by the desktop app.
    pub fn held_older(replies: Replies) -> StandIn {
        let stand_in = Self::empty(replies);
        stand_in.state().older = true;
        stand_in
    }

    /// An older Linux machine reached remotely.
    pub fn held_machine(replies: Replies) -> StandIn {
        let stand_in = Self::held_older(replies);
        stand_in.set_elsewhere(true);
        stand_in
    }

    /// Change the compatibility answer without replacing the transport.
    pub fn set_older(&self, older: bool) {
        self.state().older = older;
    }

    /// Leaves these out of what its `kuvo/info` says it has: a box behind on them, used as it is.
    pub fn set_lacks(&self, lacks: &[&str]) {
        self.state().lacks = lacks.iter().map(|name| (*name).to_owned()).collect();
    }

    /// It stops answering (asleep, off, offline), or answers again.
    pub fn set_down(&self, down: bool) {
        self.state().down = down;
    }

    /// Whether Sessions can be read (else 500), independently of the other areas.
    pub fn sessions_readable(&self, readable: bool) {
        let mut state = self.state();
        state.sessions_down = !readable;
        if !readable {
            state.lists.clear();
        }
    }

    /// Whether its Work Queue can be read (else 503).
    pub fn queue_readable(&self, readable: bool) {
        self.state().queue.down = !readable;
    }

    pub fn work_item_fixture(&self, id: &str, fields: Value, file: Option<(&str, Vec<u8>)>) {
        let mut state = self.state();
        state.queue.item_fixture(id, fields);
        if let Some((name, bytes)) = file {
            state.files.insert(format!("kuvo/work-queue/items/{id}/files/{}", kuvo_box::paths::segment(name)), bytes);
        }
    }

    pub fn work_queue_mention_fixture(&self, text: &str, reply: Value) {
        self.state().queue.mention_fixture(text, reply);
    }

    /// What it says `status` counts, whatever its items are.
    pub fn count_status(&self, status: &str, count: usize) {
        self.state().queue.counted.push((status.to_owned(), count));
    }

    /// What it says an item added with nothing said starts as.
    pub fn new_items_start(&self, status: &str, priority: &str) {
        self.state().queue.starts = Some((status.to_owned(), priority.to_owned()));
    }

    /// The day its Work Queue says it is, whatever the moment it's asked at.
    pub fn box_today(&self, day: &str) {
        self.state().queue.today_said = Some(day.to_owned());
    }

    /// One of its Work Queue's items stands in a status its view doesn't list.
    pub fn unlisted_status(&self) {
        let mut state = self.state();
        let now = state.made.unwrap_or_else(now_ms);
        state.queue.add_unlisted_status(now);
    }

    /// Its Work Queue lists a status this app has never heard of, with an item in it.
    pub fn newer_status(&self) {
        let mut state = self.state();
        let now = state.made.unwrap_or_else(now_ms);
        state.queue.add_newer_status(now);
    }

    /// Its Work Queue lists a closed status and a label colour this app has never heard of (a
    /// newer box's), with an item in that status.
    pub fn newer_closed_status(&self) {
        let mut state = self.state();
        let now = state.made.unwrap_or_else(now_ms);
        state.queue.add_newer_closed(now);
    }

    /// Its Work Queue has many more labels than a menu shows at once.
    pub fn many_labels(&self) {
        self.state().queue.add_many_labels();
    }

    /// It adds no computers, saying why (a member of a setup, say).
    pub fn cannot_add(&self, why: &str) {
        self.state().areas.add_cannot = Some(why.to_owned());
    }

    /// A try at adding `destination`, as far as `stage` (`asking`: its host key's question;
    /// `recovery`: clearing an earlier attempt; `copying`: the engine going over; `done`).
    pub fn adding_at(&self, destination: &str, stage: &str) {
        let tried = {
            let mut state = self.state();
            let begun = areas::adding(&mut state.areas, "POST", "kuvo/computers/add", &serde_json::json!({ "destination": destination }));
            let id = begun.and_then(|((_, value), _)| value["try"]["id"].as_str().map(str::to_owned));
            if stage == "recovery"
                && let Some(id) = id.as_deref()
                && let Some(tried) = state.areas.tries.iter_mut().find(|tried| tried["id"].as_str() == Some(id))
            {
                tried["question"] = json!({
                    "id": "recoverAccount", "at": tried["question"]["at"], "title": format!("Continue adding {destination}?"),
                    "message": format!("An earlier attempt to add {destination} didn't finish, and it's no longer running. Kuvo can clear it and start again."),
                    "facts": [],
                    "answers": [{ "id": "cancel", "label": "Cancel", "kind": "plain" }, { "id": "recover", "label": "Clear and Continue", "kind": "primary" }]
                });
            }
            id
        };
        if let (Some(id), "copying" | "done") = (tried, stage) {
            areas::adding(&mut self.state().areas, "POST", &format!("kuvo/computers/add/{id}/answer"), &serde_json::json!({ "answer": "trust" }));
            if stage == "done" {
                self.adding_goes_on();
            }
        }
    }

    /// Its tries at adding a computer that are under way come to their end.
    pub fn adding_goes_on(&self) {
        let mut state = self.state();
        if areas::adding_goes_on(&mut state.areas)
            && let Some((_, now)) = state.area("GET", "kuvo/computers")
        {
            let line = now.to_string();
            state.computers.retain(|_, sender| sender.send(line.clone()).is_ok());
        }
    }

    /// It brings the Kuvo of the computer `key` up to date (`computers.update`): how that stands
    /// (`waiting`, `updating`, `failed`), whether it has waited long, and how many sessions are
    /// at work there.
    pub fn bring_up_to_date(&self, key: &str, state: &'static str, long: bool, working: usize) {
        let mut state_now = self.state();
        state_now.areas.updates.retain(|updating| updating.key != key);
        state_now.areas.updates.push(areas::Updating { key: key.to_owned(), state, long, working });
    }

    /// The setup's update to its accepted release, as its list says it (`fleetUpdate`): the
    /// summary, and each computer's progress by key (one left out reports none, as a Kuvo older
    /// than that). It answers Update Now and Dismiss (`update.fleet`).
    pub fn fleet_update(&self, summary: Value, rows: Vec<(&str, Value)>) {
        let mut state = self.state();
        state.areas.fleet = Some(summary);
        state.areas.fleet_rows = rows.into_iter().map(|(key, row)| (key.to_owned(), row)).collect();
    }

    /// An update snapshot in the box’s wire format.
    pub fn fleet_sample(&self, failed: bool, finished: bool, _now: f64) {
        let target = "0.8.0-beta.38";
        let current = "0.8.0-beta.35";
        let up = json!({ "current": target, "target": target, "state": "up-to-date", "canRetry": false, "line": { "words": "beta.38 · Up to date", "tone": "done" } });
        let ready = json!({ "current": current, "target": target, "state": "ready", "canRetry": false, "line": { "words": "Ready for beta.38", "tone": "done" } });
        let off = json!({ "current": current, "target": target, "state": "off", "canRetry": false, "line": { "words": "beta.35 · Can’t be reached: updates when it’s back", "tone": "off" } });
        let reason = "There isn’t room to prepare the release.";
        let devbox = if failed {
            json!({ "current": current, "target": target, "state": "cannot", "reason": reason, "fixable": true, "canRetry": true, "line": { "words": format!("beta.35 · {reason}"), "tone": "warning", "short": "Can’t update" } })
        } else {
            json!({ "current": current, "target": target, "state": "getting-ready", "canRetry": false, "line": { "words": "Getting beta.38 ready…", "tone": "working" } })
        };
        let rows = if finished {
            vec![("key_stand_in", up.clone()), ("key_devbox", up.clone()), ("key_studio", up.clone()), ("key_more_0", up)]
        } else {
            vec![("key_stand_in", up), ("key_devbox", devbox), ("key_studio", off), ("key_more_0", ready)]
        };
        let summary = if finished {
            json!({ "target": target, "stage": "finished", "done": 4, "updating": 0, "behind": 0, "failed": 0,
                "dismissed": false, "failures": [], "shown": false })
        } else if failed {
            json!({ "target": target, "stage": "failed", "done": 2, "updating": 0, "behind": 1, "failed": 1,
                "dismissed": false, "failures": [], "shown": true, "warning": true, "title": "devbox can’t update to beta.38", "detail": format!("devbox: {reason}") })
        } else {
            json!({ "target": target, "stage": "downloading", "done": 2, "updating": 1, "behind": 1, "failed": 0,
                "dismissed": false, "failures": [], "shown": true, "warning": false, "title": "Updating to beta.38 · 2 of 3 ready",
                "detail": "Everything works on beta.35 while your computers get ready." })
        };
        self.more_computers(1);
        self.fleet_update(summary, rows);
    }

    /// The setup's update at one of its six phases (0 to 5), in the box's wire format: all getting
    /// ready, some ready, all ready, switching, up to date but one off, all up to date.
    pub fn fleet_phase(&self, phase: usize) {
        let (target, current) = ("0.8.0-beta.44", "0.8.0-beta.43");
        let row = |step: &str| -> Value {
            let (state, words, tone, phase, fill): (&str, String, &str, &str, Option<f64>) = match step {
                "ready" => ("ready", "Ready for beta.44".into(), "done", "ready", None),
                "switching" => ("updating", "Switching to beta.44…".into(), "working", "switching", None),
                "done" => ("up-to-date", "beta.44 · Up to date".into(), "done", "done", None),
                "off" => ("off", "beta.43 · seen 2h ago".into(), "off", "off", None),
                filling => {
                    let fill: f64 = filling.parse().unwrap_or(0.);
                    ("getting-ready", format!("Getting ready · {}%", (fill * 100.).round()), "working", "getting-ready", Some(fill))
                }
            };
            let mut line = json!({ "words": words, "tone": tone, "phase": phase });
            if let Some(fill) = fill { line["fill"] = json!(fill); }
            let mut row = json!({ "current": if state == "up-to-date" { target } else { current }, "target": target, "state": state, "canRetry": false, "line": line });
            if state == "updating" { row["stage"] = json!("switching"); }
            row
        };
        let (steps, summary_phase, title, detail): ([&str; 4], &str, &str, Option<&str>) = match phase {
            0 => (["0.72", "0.40", "0.15", "0.58"], "getting-ready", "Getting beta.44 ready", Some("Your computers stay on beta.43 meanwhile.")),
            1 => (["ready", "0.81", "ready", "0.64"], "getting-ready", "Getting beta.44 ready · 2 of 4", Some("Your computers stay on beta.43 meanwhile.")),
            2 => (["ready"; 4], "ready", "beta.44 is ready · 4 of 4", Some("They switch next.")),
            3 => (["done", "switching", "ready", "ready"], "switching", "Switching to beta.44 · 1 of 4 done", Some("3 still on beta.43.")),
            4 => (["done", "done", "done", "off"], "waiting", "beta.44 · 3 of 4 up to date", Some("old-mini updates when it’s back.")),
            _ => (["done"; 4], "done", "All 4 on beta.44", None),
        };
        let keys = ["key_stand_in", "key_devbox", "key_studio", "key_old"];
        let rows = keys.iter().zip(steps).map(|(key, step)| (*key, row(step))).collect();
        let done = steps.iter().filter(|step| **step == "done").count();
        let mut summary = json!({ "target": target, "stage": "downloading", "done": done, "updating": 0, "behind": 0, "failed": 0,
            "dismissed": false, "failures": [], "shown": true, "warning": false, "title": title, "phase": summary_phase });
        if let Some(detail) = detail { summary["detail"] = json!(detail); }
        self.more_computers(0);
        self.fleet_update(summary, rows);
    }

    /// Try Again for a computer is refused with `words`, as a box whose release moved on says.
    pub fn retry_refused(&self, words: &str) {
        self.state().areas.retry_refused = Some(words.to_owned());
    }

    /// Check for Updates (`POST kuvo/update {now}`) answers `status` with `value`, as a box would.
    pub fn check_says(&self, status: u16, value: Value) {
        self.state().areas.check_says = Some((status, value));
    }

    /// It's from before it took the release its app carries itself (no `computers.update.here`).
    pub fn from_before_update_here(&self) {
        self.state().before_update_here = true;
    }

    /// The release its app carries waits for it (`computers.update.here`): its own row says how
    /// (`waiting`, `updating`, `failed`) with Update Now, asked first while `working` sessions are
    /// at work on it.
    pub fn own_update_waits(&self, state: &'static str, working: usize) {
        self.state().areas.here_update = Some(areas::Updating { key: String::new(), state, long: false, working });
    }

    pub fn own_update_words(&self, title: String, detail: String) {
        self.state().areas.here_update_words = Some((title, detail));
    }

    pub fn finish_own_update(&self) {
        self.state().areas.here_update = None;
    }

    /// Its list offers to set the computer `key` up again over SSH (Set Up Again), as the box
    /// that set it up does, without bringing it up to date itself.
    pub fn sets_up_again(&self, key: &str) {
        self.state().areas.set_up_again.push(key.to_owned());
    }

    /// It says the older machine `key`, which it didn't set up over SSH, is to be set up over SSH
    /// from here (its row's `update`, and `add`).
    pub fn needs_set_up_over_ssh(&self, key: &str) {
        self.state().areas.to_set_up.push(key.to_owned());
    }

    /// It says the older computer `key` needs an update it can't bring itself, in `why`'s words,
    /// with nothing to press (a Linux box, or one a Mac's setup has as a member).
    pub fn cant_bring_up(&self, key: &str, why: &str) {
        let mut state = self.state();
        state.areas.cant_bring_up.push((key.to_owned(), why.to_owned()));
        // (It lists your computers then, as every box does.)
        state.areas.lists_yours = true;
    }

    /// It lists the computer `key` as a Mac.
    pub fn lists_as_mac(&self, key: &str) {
        self.state().areas.as_macs.push(key.to_owned());
    }

    /// Sets the `system` string reported for computer `key` (or clears it when `None`).
    pub fn set_computer_system(&self, key: &str, system: Option<&str>) {
        let mut state = self.state();
        state.areas.systems.retain(|(existing, _)| existing != key);
        state.areas.systems.push((key.to_owned(), system.map(str::to_owned)));
    }

    /// It's the main computer of a setup with every computer it lists.
    pub fn setup_roles(&self) {
        self.state().areas.roles = Some(areas::Roles { primary: "key_stand_in".into(), epoch: 3, older_standby: None });
    }

    /// Its `kuvo/setup` names `key` its standby, as a box from before the relay lease does.
    pub fn names_a_standby(&self, key: &str) {
        if let Some(roles) = self.state().areas.roles.as_mut() {
            roles.older_standby = Some(key.to_owned());
        }
    }

    /// Its list of computers has this computer alone.
    pub fn computers_alone(&self) {
        self.state().areas.computers_alone = true;
    }

    /// `more` machines listed after the sample's (a long list).
    pub fn more_computers(&self, more: usize) {
        self.state().areas.more_computers = more;
    }

    /// What renaming and removing say (`503`) instead of doing it, as a box that can't ask the
    /// computer that writes the record says it.
    /// A computer's app was removed from your devices: its box adds none until it's allowed again.
    pub fn block_vouching(&self, key: &str) {
        self.state().areas.vouch_blocked.push(key.to_owned());
    }

    pub fn manage_refused(&self, words: &str) {
        self.state().areas.manage_refused = Some(words.to_owned());
    }

    /// It isn't a Mac's own box shown in the menu bar (a machine's, or one on Linux): the first
    /// Quit has nothing to note.
    pub fn not_in_menu_bar(&self) {
        self.state().areas.no_menu_bar = true;
    }

    /// The old Kuvo Code app is open on this Mac: removing a machine waits, saying so.
    pub fn old_app_open(&self) {
        self.state().areas.old_app = true;
    }

    /// Whether its list of computers can be read (else 503, as a box that isn't answering).
    pub fn computers_readable(&self, readable: bool) {
        self.state().areas.computers_down = !readable;
    }

    /// Every sign-in under way ends so (the box's words for a failure: `message`, the engine's).
    pub fn end_sign_ins(&self, ending: settings::Ending, message: Option<&str>) {
        self.state().settings.end_sign_ins(ending, message);
    }

    pub fn defer_pasted_codes(&self) {
        self.state().settings.defer_pasted_codes = true;
    }

    /// From now on, sign-ins under way say this device code.
    pub fn set_user_code(&self, code: &str) {
        self.state().settings.set_user_code(code);
    }

    /// A value in this computer's settings changes, as on its computer meanwhile.
    pub fn set_config_value(&self, path: &[&str], value: Value) {
        self.state().settings.set_config(path, value);
    }

    /// From now on it stands in for a box on another computer (a machine's), or this one's again.
    pub fn set_elsewhere(&self, elsewhere: bool) {
        self.state().elsewhere = elsewhere;
    }

    /// The computer's name as reported by the stand-in.
    pub fn set_computer_name(&self, name: &str) {
        self.state().name = Some(name.to_owned());
    }

    /// From now on, it says this of its sandbox (`kuvo/sandbox`).
    pub fn set_sandbox(&self, sandbox: Value) {
        self.state().settings.set_sandbox(sandbox);
    }

    /// The Library's sample: draft A's five bots, here, and what they handed over across two
    /// computers; `empty`: nothing handed over yet, on one computer.
    pub fn library_sample(&self, empty: bool) {
        let mut state = self.state();
        let now = state.made.unwrap_or_else(now_ms);
        state.library.empty = empty;
        state.bots.bots.clear();
        state.bots.elsewhere.clear();
        state.bots.held.clear();
        for (id, name, color, shape) in library::BOTS {
            state.bots.add(id, name, json!({ "color": color, "shape": shape, "face": "calm" }), now);
        }
        state.sync_bots();
    }

    /// The Marketplace's sample: nothing added yet, or a setup in use (a second computer too).
    pub fn marketplace_sample(&self, sample: marketplace::Sample) {
        self.state().marketplace.sample(sample);
    }

    /// Every Marketplace sign-in started finishes, as the browser's would.
    pub fn finish_marketplace_sign_ins(&self) {
        self.state().marketplace.finish_sign_ins();
    }

    /// From now on, Insights answers with `sample` (a month of work, a first week, nothing, or a failure).
    pub fn set_insights(&self, sample: insights::Sample) {
        self.state().areas.insights = Some(sample);
    }

    pub fn set_usage_sample(&self, sample: areas::UsageSample) {
        self.state().areas.usage = sample;
    }

    pub fn fail_usage_refresh(&self, failed: bool) {
        self.state().areas.usage_refresh_failed = failed;
    }

    pub fn delay_usage_refresh(&self) {
        self.state().areas.usage_refresh_delayed = true;
    }

    pub fn complete_usage_refresh(&self, sample: areas::UsageSample) {
        let mut state = self.state();
        state.areas.usage_refresh_pending = false;
        state.areas.usage_refresh_delayed = false;
        state.areas.usage_refresh_completed = true;
        state.areas.usage = sample;
        state.usage_followers.retain(|_, sender| sender.send("{}".into()).is_ok());
    }

    /// What its box says of an account from now on (`state`, `line`, `actions`), whatever the
    /// account is: for a client drawing what a box says, a newer box's words among them.
    pub fn set_account_said(&self, account: &str, said: Value) {
        self.state().settings.set_account_said(account, said);
    }

    pub fn set_agent_metadata(&self, id: &str, metadata: Value) {
        self.state().settings.set_agent_metadata(id, metadata);
    }

    /// Whether its bots can be read (`kuvo/bots` answers `503` while not).
    pub fn bots_readable(&self, readable: bool) {
        let mut state = self.state();
        state.bots.down = !readable;
        if !readable {
            state.bots_followers.clear();
        }
    }

    /// Whether it can say what it holds for other computers (`held` and `refused` are left out of
    /// `kuvo/bots` while it can't check its seals).
    pub fn bots_held_sayable(&self, sayable: bool) {
        self.state().bots.unsealed = !sayable;
    }

    /// Whether a bot can be made or changed (`503` while not, as with its engine stopped).
    pub fn bots_changeable(&self, changeable: bool) {
        self.state().bots.stopped = !changeable;
    }

    /// `bot` moves to `computer`: it no longer lists here, and a change to it is held for there.
    pub fn move_bot(&self, bot: &str, computer: &str) {
        let mut state = self.state();
        state.bots.move_away(bot, computer);
        state.sync_bots();
    }

    /// The computer `bot` lives on (one of your others) as the box says it on the bot's row: away,
    /// last heard from at `seen` (ms), or answering (`None`). Those following the bots hear it.
    pub fn bot_computer_away(&self, bot: &str, seen: Option<f64>) {
        let mut state = self.state();
        if let Some(row) = state.bots.elsewhere.iter_mut().find(|each| each["id"] == json!(bot))
            && let Some(row) = row.as_object_mut()
        {
            match seen {
                Some(seen) => {
                    row.insert("away".into(), json!(true));
                    row.insert("lastSeen".into(), json!(seen));
                }
                None => {
                    row.remove("away");
                    row.remove("lastSeen");
                }
            }
        }
        state.sync_bots();
    }

    /// `bot`, on another computer, has `session` as its conversation there.
    pub fn set_elsewhere_session(&self, bot: &str, session: &str) {
        let mut state = self.state();
        if let Some(row) = state.bots.elsewhere.iter_mut().find(|each| each["id"] == json!(bot)) {
            row["session"] = json!(session);
        }
        state.sync_bots();
    }

    /// This box carried `session` from the computer `key` once (its newest page, kept as it keeps
    /// it), as though it was read here before that computer went away.
    pub fn carry(&self, key: &str, session: &str) {
        let _ = Transport::send(self, &Request::get(format!("kuvo/peer-session/{key}/{session}/messages?limit=100&order=desc")));
    }

    /// The computer `bot` lives on switches to a new release (`true`): not away, said to be updating.
    pub fn bot_computer_updating(&self, bot: &str, updating: bool) {
        let mut state = self.state();
        if let Some(row) = state.bots.elsewhere.iter_mut().find(|each| each["id"] == json!(bot))
            && let Some(row) = row.as_object_mut()
        {
            if updating {
                row.remove("away");
                row.remove("lastSeen");
                row.insert("updating".into(), json!(true));
            } else {
                row.remove("updating");
            }
        }
        state.sync_bots();
    }

    /// What `computer` refused of an edit held for `bot`, in its words.
    pub fn refuse_bot_edit(&self, bot: &str, computer: &str, message: &str) {
        let mut state = self.state();
        let at = state.made.unwrap_or_else(now_ms);
        state.bots.refuse(bot, computer, message, at);
    }

    /// One more bot here, idle, in `look`.
    pub fn add_bot(&self, id: &str, name: &str, look: Value) {
        let mut state = self.state();
        let now = state.made.unwrap_or_else(now_ms);
        state.bots.add(id, name, look, now);
        state.sync_bots();
    }

    /// What `bot` is doing now, as its box says it (`bots.now`): those following the bots hear it.
    pub fn set_bot_now(&self, bot: &str, now: Value) {
        let mut state = self.state();
        if let Some(listed) = state.bots.bots.iter_mut().find(|each| each["id"] == json!(bot)) {
            listed["now"] = now;
        }
        state.sync_bots();
    }

    /// A bot's schedule, as its computer says it on both routes.
    pub fn set_bot_wakes(&self, bot: &str, wakes: Vec<Value>, watches: Vec<Value>) {
        let mut state = self.state();
        let Some(listed) = state.bots.bots.iter_mut().find(|each| each["id"] == bot) else { return };
        let session = listed["session"].as_str().expect("bot session").to_owned();
        listed["wakes"] = wakes.first().map(|wake| wake["due"].clone()).unwrap_or_default();
        listed["watching"] = json!(watches.len());
        state.wakes.insert(session.clone(), json!({ "wakes": wakes, "watches": watches, "paused": false }));
        state.sync_bots();
        state.told(&session, Followed::Wakes);
    }

    /// A bot's sessions on `kuvo/work?bot=`, overriding the sample list for `bot`.
    pub fn set_bot_work(&self, bot: &str, sessions: Vec<Value>) {
        let mut state = self.state();
        state.bots.set_work(bot, sessions);
    }

    /// `bot`'s computer no longer lists it (deleted there, or that computer was removed).
    pub fn forget_bot_elsewhere(&self, bot: &str) {
        self.state().bots.elsewhere.retain(|each| each["id"] != json!(bot));
    }

    pub fn add_output(&self, session: &str, artifact: Value, bytes: &[u8]) {
        let mut state = self.state();
        let id = artifact["id"].as_str().expect("artifact id");
        let version = artifact["latest"]["n"].as_u64().expect("artifact version");
        state.output_bytes.insert(format!("kuvo/artifacts/{id}/versions/{version}"), bytes.to_vec());
        state.outputs.entry(session.to_owned()).or_default().push(artifact);
        state.told(session, Followed::Outputs);
    }

    /// Sign-ins under way are forgotten, as by an engine that started over.
    pub fn forget_sign_ins(&self) {
        self.state().settings.forget_sign_ins();
    }

    /// Whether the coding agents' bridge runs: while not, they're down, as a box says when nothing
    /// started them for two minutes, with Try Again (PROTOCOL.md, "The coding agents' state").
    pub fn agents_running(&self, running: bool) {
        self.state().settings.agents_away = (!running).then(|| json!({ "message": "The coding agents haven't started.", "reason": "agentsDown", "action": { "label": "Try Again", "route": "kuvo/agents/start" } }));
    }

    /// The coding agents' bridge restarting (a newer build taking over), as a box says it after
    /// waiting half a minute for it.
    pub fn agents_restarting(&self) {
        self.state().settings.agents_away = Some(json!({ "message": "The coding agents are restarting. This takes a few seconds.", "reason": "agentsRestarting" }));
    }

    /// The coding agents' bridge down, in the box's `words`, with Try Again where `try_again`.
    pub fn agents_down(&self, words: &str, try_again: bool) {
        let mut away = json!({ "message": words, "reason": "agentsDown" });
        if try_again {
            away["action"] = json!({ "label": "Try Again", "route": "kuvo/agents/start" });
        }
        self.state().settings.agents_away = Some(away);
    }

    /// Whether Try Again (`kuvo/agents/start`) gets the coding agents running.
    pub fn agents_start_works(&self, works: bool) {
        self.state().settings.agents_start = works;
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Its group chats can't be reached now (503), or can again.
    pub fn groups_readable(&self, readable: bool) {
        self.state().groups.down = !readable;
    }

    /// `bot` says `text` in `group`, ending its turn (a reply come in), as followers hear it.
    pub fn group_says(&self, group: &str, bot: &str, text: &str) {
        let mut state = self.state();
        let now = state.made.unwrap_or_else(now_ms) + 60_000.0;
        state.groups.say(group, bot, text, now);
        state.group_changed(group);
    }

    /// Its terminals' sockets can't be opened (`Some`: why, as the box's way there says it), or
    /// can again.
    pub fn terminals_not_here(&self, why: Option<&str>) {
        self.state().terminals.not_here = why.map(str::to_owned);
    }

    /// Its engine won't make a terminal now (`Some`: why), or will again.
    pub fn terminals_refused(&self, why: Option<&str>) {
        self.state().terminals.refuses = why.map(str::to_owned);
    }

    /// The way it's reached can't carry a socket (`Some`: why, as that way says it), for a
    /// terminal or the browser: or can again.
    pub fn sockets_not_here(&self, why: Option<&str>) {
        self.terminals_not_here(why);
    }

    /// Its browser's keeper runs (the live view's socket opens), or doesn't.
    pub fn browser_running(&self, running: bool) {
        self.state().browser.running = running;
    }

    /// Its browser's pages are now `pages` (`{active, pages}`, as `Kuvo.pages` says them): its open
    /// socket says so.
    pub fn browser_pages(&self, pages: Value) {
        let mut state = self.state();
        state.browser.pages = pages.clone();
        state.browser.say(browser::fact("Kuvo.pages", &pages));
    }

    /// A page's size is now `viewport` (`{targetId, width, height, scale, by}`): its open socket says so.
    pub fn browser_viewport(&self, viewport: Value) {
        let mut state = self.state();
        let target = viewport["targetId"].clone();
        state.browser.viewports.retain(|each| each["targetId"] != target);
        state.browser.viewports.push(viewport.clone());
        state.browser.say(browser::fact("Kuvo.viewport", &viewport));
    }

    /// Its browser sends a picture (`params`: the frame header's `{targetId, frameId, width,
    /// height, scrollX, scrollY}`) of `jpeg`.
    pub fn browser_frame(&self, params: Value, jpeg: &[u8]) {
        self.state().browser.say(browser::picture(&params, jpeg));
    }

    /// Each command the app sent its browser, in order, as JSON.
    pub fn browser_sent(&self) -> Vec<Value> {
        self.state().browser.sent.clone()
    }

    /// Those of them for `method`: their params.
    pub fn browser_sent_of(&self, method: &str) -> Vec<Value> {
        self.browser_sent().into_iter().filter(|command| command["method"] == method).map(|command| command["params"].clone()).collect()
    }

    /// How many times its browser's socket was opened, and whether the app closed it.
    pub fn browser_opened(&self) -> (usize, bool) {
        let state = self.state();
        (state.browser.opened, state.browser.closed)
    }

    /// Its browser's socket ends (the keeper stopped, or the connection went): why, where said.
    pub fn browser_end(&self, why: Option<&str>) {
        if let Some(socket) = self.state().browser.socket.take() {
            let _ = socket.send_blocking(SocketMessage::Ended(why.map(str::to_owned)));
        }
    }

    /// The terminals it has now (made and not removed), by id.
    pub fn ptys(&self) -> Vec<String> {
        self.state().terminals.ids()
    }

    /// The shell in terminal `id` prints `bytes`: its open socket brings them, and one opened later
    /// replays them first.
    pub fn pty_print(&self, id: &str, bytes: &[u8]) {
        let mut state = self.state();
        let Some(pty) = state.terminals.get(id) else { return };
        pty.printed.extend_from_slice(bytes);
        if let Some(socket) = &pty.socket {
            let _ = socket.send_blocking(SocketMessage::Text(bytes.to_vec()));
        }
    }

    /// The shell in terminal `id` exits with `code`: its socket ends, as the engine's does.
    pub fn pty_exit(&self, id: &str, code: i64) {
        let mut state = self.state();
        let Some(pty) = state.terminals.get(id) else { return };
        pty.info["status"] = json!("exited");
        pty.info["exitCode"] = json!(code);
        if let Some(socket) = pty.socket.take() {
            let _ = socket.send_blocking(SocketMessage::Ended(None));
        }
    }

    /// Terminal `id`'s socket drops, its shell still running (the connection went).
    pub fn pty_drop(&self, id: &str) {
        let mut state = self.state();
        if let Some(socket) = state.terminals.get(id).and_then(|pty| pty.socket.take()) {
            let _ = socket.send_blocking(SocketMessage::Ended(Some("The connection closed.".into())));
        }
    }

    /// What was typed in terminal `id`, as it went, in order.
    pub fn pty_typed(&self, id: &str) -> Vec<String> {
        self.state().terminals.get(id).map(|pty| pty.typed.clone()).unwrap_or_default()
    }

    /// Each size terminal `id` was told it's drawn at, in order (rows, columns).
    pub fn pty_sizes(&self, id: &str) -> Vec<(u16, u16)> {
        self.state().terminals.get(id).map(|pty| pty.sizes.clone()).unwrap_or_default()
    }

    /// Whether terminal `id`'s socket was closed from the app.
    pub fn pty_closed(&self, id: &str) -> bool {
        self.state().terminals.get(id).is_some_and(|pty| pty.closed)
    }

    /// Every request it was sent, in order.
    pub fn seen(&self) -> Vec<Seen> {
        self.state().seen.clone()
    }

    /// Those it was sent to `route` (a path without its query).
    pub fn seen_at(&self, method: &str, route: &str) -> Vec<Seen> {
        self.seen().into_iter().filter(|seen| seen.method == method && seen.path.split('?').next() == Some(route)).collect()
    }

    /// A session's facts changed (as its box would say), and the list's followers hear it.
    pub fn set_facts(&self, session: &str, change: impl FnOnce(&mut Value)) {
        let mut state = self.state();
        if let Some(facts) = state.sessions.get_mut(session) {
            change(facts);
        }
        state.list_changed();
    }

    /// What runs for `session` besides its turn from now on (`{commands, subagents}`, as
    /// `kuvo/work/background/<session>` answers), and what each command printed; its followers hear it.
    pub fn set_background(&self, session: &str, background: Value, printed: &[(&str, &str)]) {
        let mut state = self.state();
        for (id, text) in printed {
            state.work_bars.printed.insert((*id).to_owned(), (*text).to_owned());
        }
        state.work_bars.background.insert(session.to_owned(), background);
        state.work_bars.changed();
    }

    /// What `session`'s `GET api/session/<id>` says of its context from now on (PROTOCOL.md,
    /// "Session context"): `{used, window?, summarizeAt?}`.
    pub fn set_context(&self, session: &str, context: Value) {
        self.state().contexts.insert(session.to_owned(), context);
    }

    /// What `session`'s `GET api/session/<id>` offers of where it's summarized (`data.summarize`),
    /// its choice in use following this stand-in's Settings › Conversations, where a test added it
    /// (`add_settings_section`).
    pub fn set_summarize(&self, session: &str, summarize: Value) {
        self.state().summarize.insert(session.to_owned(), summarize);
    }

    /// `session`'s subagents that are done, newest first (as `kuvo/work/subagents/<session>` lists
    /// them under `done`).
    pub fn set_subagents_done(&self, session: &str, done: Vec<Value>) {
        self.state().panel.done.insert(session.to_owned(), done);
    }

    /// What the next side questions are answered, in order (none left: they stay being answered).
    pub fn answer_side_questions(&self, answers: &[&str]) {
        self.state().panel.answers = answers.iter().map(|answer| (*answer).to_owned()).collect();
    }

    /// `session`'s side questions, as the box keeps them, oldest first.
    pub fn side_questions(&self, session: &str) -> Vec<Value> {
        self.state().panel.questions.get(session).cloned().unwrap_or_default()
    }

    /// A bot's work changed (`kuvo/work/events`): its clients read it again.
    pub fn bot_work_changed(&self, bot: &str) {
        let mut state = self.state();
        state.at += 1;
        let at = state.at as f64;
        state.work_bars.work_changed(bot, at);
    }

    /// Whether Full Access was confirmed here, and why Auto asks first for now (none: it runs).
    pub fn set_approvals(&self, full_access: bool, auto_held: Option<&str>) {
        let mut state = self.state();
        state.settings.approval["fullAccess"] = json!(full_access);
        state.settings.approval["auto"]["engine"]["held"] = auto_held.map_or(Value::Null, |why| json!({ "reason": "sandbox", "why": why }));
    }

    /// `session`'s mode on the box, as last set.
    pub fn approval_of(&self, session: &str) -> Option<String> {
        self.state().work_bars.approvals.get(session).cloned()
    }

    /// Whether Full Access is confirmed here now.
    pub fn full_access(&self) -> bool {
        self.state().settings.approval["fullAccess"] == json!(true)
    }

    /// The commands stopped, in order.
    pub fn stopped_commands(&self) -> Vec<String> {
        self.state().work_bars.stopped.clone()
    }

    /// How each new session was asked to start (`{session, worktree, approval}`), in order.
    pub fn starts(&self) -> Vec<Value> {
        self.state().starts.clone()
    }

    /// From now on, a message sent is answered as by an engine that isn't answering.
    pub fn fail_sends(&self) {
        self.state().sends_fail = true;
    }

    /// From now on, its copy of a message sent comes without the files sent with it.
    pub fn drop_files(&self) {
        self.state().drops_files = true;
    }

    /// A file on its computer, at `path`, for `kuvo/file`.
    pub fn put_file(&self, path: &str, bytes: Vec<u8>) {
        self.state().files.insert(path.to_owned(), bytes);
    }

    /// `session` is a coding agent's subagent of `parent`, whose approval it runs with.
    /// It's listed on `parent`'s facts (`subagents`), as the box lists them.
    pub fn subagent_of(&self, session: &str, parent: &str) {
        let mut state = self.state();
        state.work_bars.parents.insert(session.to_owned(), parent.to_owned());
        if let Some(facts) = state.sessions.get_mut(parent) {
            facts["subagents"] = json!([session]);
        }
        state.list_changed();
    }

    /// `message` (as the engine has one) added at the end of `session`'s conversation.
    pub fn add_message(&self, session: &str, message: Value) {
        self.state().messages.entry(session.to_owned()).or_default().push(message);
    }

    /// From now on, messages sent go again (after `fail_sends`).
    pub fn sends_go(&self) {
        self.state().sends_fail = false;
    }

    /// From now on, no model runs here (`kuvo/models/offered` says `starting: null`).
    pub fn nothing_runs(&self) {
        self.state().nothing_runs = true;
    }

    /// From now on, it says this of `session`'s model (`kuvo/sessions/models`); null leaves it out.
    pub fn set_model_state(&self, session: &str, state: Value) {
        self.state().model_states.insert(session.to_owned(), state);
    }

    /// Whether its sessions' models can be read (`kuvo/sessions/models` answers `503` while not).
    pub fn models_readable(&self, readable: bool) {
        self.state().models_fail = !readable;
    }

    /// Your starred models, from now on.
    pub fn set_starred(&self, starred: Vec<(&str, &str)>) {
        self.state().starred = starred.into_iter().map(|(provider, id)| json!({ "providerID": provider, "id": id })).collect();
    }

    /// Your starred models, as it keeps them now.
    pub fn starred(&self) -> Vec<(String, String)> {
        self.state().starred.iter().map(|each| (each["providerID"].as_str().unwrap_or_default().to_owned(), each["id"].as_str().unwrap_or_default().to_owned())).collect()
    }

    /// `count` more models besides, from other providers (a box with hundreds).
    pub fn more_models(&self, count: usize) {
        self.state().more_models = count;
    }

    /// Your accounts with Claude Code from now on, by name and plan, its own sign-in first: each
    /// lists Claude Code's models, as `claude-code` and `claude-code@account<N>`.
    pub fn claude_accounts(&self, accounts: &[(&str, Option<&str>)]) {
        self.state().claude_accounts = accounts.iter().map(|(name, plan)| ((*name).to_owned(), plan.map(str::to_owned))).collect();
    }

    /// Codex's models are offered from now on (`codex-cli`), each with its efforts, and Fast where
    /// it has it.
    pub fn offers_codex(&self) {
        self.state().codex = true;
    }

    /// A model with a long name and efforts is offered from now on (`openrouter/long-preview`).
    pub fn offers_a_long_name(&self) {
        self.state().long_name = true;
    }

    /// The speed picked for `provider`, as `kuvo/models/speed` keeps it (`None`: Standard, or
    /// never picked).
    pub fn speed_of(&self, provider: &str) -> Option<String> {
        self.state().speeds.get(provider).and_then(|speed| speed.as_str().map(str::to_owned))
    }

    /// It keeps no stars, as a box before `models.starred`.
    pub fn keeps_no_stars(&self) {
        self.state().no_stars = true;
    }

    /// Whether the coding agents' bridge can say its models (`known.agents`).
    pub fn agents_known(&self, known: bool) {
        self.state().agents_unknown = !known;
    }

    /// What `session` waits on you for, from now on (as the box words each ask).
    pub fn set_asks(&self, session: &str, asks: Vec<Value>) {
        let mut state = self.state();
        state.asks.insert(session.to_owned(), asks);
        state.told(session, Followed::Asks);
    }

    /// `session`'s queue held by a Stop (`true`) until Resume, or going on.
    pub fn pause_queue(&self, session: &str, paused: bool) {
        let mut state = self.state();
        if paused {
            state.paused.insert(session.to_owned());
        } else {
            state.paused.remove(session);
        }
        state.told(session, Followed::Queue);
    }

    /// Why `session`'s queue waits, as its box says it (its engine being updated), or none.
    pub fn queue_line(&self, session: &str, line: Option<&str>) {
        let mut state = self.state();
        match line {
            Some(line) => state.queue_lines.insert(session.to_owned(), line.to_owned()),
            None => state.queue_lines.remove(session),
        };
        state.told(session, Followed::Queue);
    }

    /// Whether `session` runs, as its engine says (`api/session/active`).
    pub fn set_running(&self, session: &str, running: bool) {
        let mut state = self.state();
        if running {
            state.running.insert(session.to_owned());
        } else {
            state.running.remove(session);
        }
        if let Some(facts) = state.sessions.get_mut(session) {
            facts["running"] = json!(running);
        }
        state.list_changed();
    }

    /// Steer and Send Now as they were asked of its queue (`steer <id>`, `now <id>`), in order.
    pub fn queue_asked(&self) -> Vec<String> {
        self.state().queue_asked.clone()
    }

    /// `session`'s queue as its box shows it now.
    pub fn queue_of(&self, session: &str) -> Vec<Value> {
        self.state().queues.get(session).cloned().unwrap_or_default()
    }

    /// `session`'s queue from now on, in the box's order (each as `items` lists it; a `draft` on one
    /// is what its box puts back for Edit, else its words).
    pub fn set_queue(&self, session: &str, items: Vec<Value>) {
        let mut state = self.state();
        state.queues.insert(session.to_owned(), items);
        state.told(session, Followed::Queue);
    }

    /// An engine event, to those following the engine's events.
    pub fn emit(&self, kind: &str, data: Value) {
        self.state().emit(kind, data);
    }

    /// From now on, `session` is held as moving to another computer (with `words`), or released (`None`).
    pub fn hold_session(&self, session: &str, words: Option<&str>) {
        let mut state = self.state();
        match words {
            Some(words) => {
                state.held_sessions.insert(session.to_owned(), words.to_owned());
                if let Some(facts) = state.sessions.get_mut(session) {
                    facts["away"] = json!(true);
                }
            }
            None => {
                state.held_sessions.remove(session);
                if let Some(facts) = state.sessions.get_mut(session) {
                    facts.as_object_mut().map(|m| m.remove("away"));
                }
            }
        }
        state.list_changed();
    }

    /// Marks `session` as archived by the engine itself (`time.archived`), so only the engine can bring it back.
    pub fn set_engine_archived(&self, session: &str, engine_archived: bool) {
        let mut state = self.state();
        if engine_archived {
            state.engine_archived.insert(session.to_owned());
            if let Some(facts) = state.sessions.get_mut(session) {
                facts["archived"] = json!(true);
            }
        } else {
            state.engine_archived.remove(session);
        }
        state.list_changed();
    }

    /// Marks a removed worktree directory as still holding kept changes for Restore Worktree.
    pub fn set_kept_worktree_changes(&self, directory: &str, kept: bool) {
        let mut state = self.state();
        if kept {
            state.kept_worktree_changes.insert(directory.to_owned());
        } else {
            state.kept_worktree_changes.remove(directory);
        }
    }

    /// The changed files of the repository at `root` from now on (each as `kuvo/git/files` lists
    /// one), and its followers hear it.
    pub fn set_git_files(&self, root: &str, files: Vec<Value>) {
        let mut state = self.state();
        if let Some(repo) = state.git.as_mut().and_then(|git| git.repos.get_mut(root)) {
            repo.files = files;
        }
        state.git_changed(root);
    }

    /// A file's diff from now on, as `kuvo/git/changes` sends it.
    pub fn set_git_patch(&self, root: &str, path: &str, patch: &str) {
        if let Some(repo) = self.state().git.as_mut().and_then(|git| git.repos.get_mut(root)) {
            repo.patches.insert(path.to_owned(), patch.to_owned());
        }
    }

    /// Staged files outside the folder from now on, and what git is in the middle of there.
    pub fn set_git_state(&self, root: &str, staged_elsewhere: usize, underway: Option<&'static str>) {
        let mut state = self.state();
        if let Some(repo) = state.git.as_mut().and_then(|git| git.repos.get_mut(root)) {
            repo.staged_elsewhere = staged_elsewhere;
            repo.underway = underway;
        }
        state.git_changed(root);
    }

    /// The next commit that pushes lands, but its push doesn't go through.
    pub fn fail_next_push(&self) {
        if let Some(git) = self.state().git.as_mut() {
            git.push_fails = true;
        }
    }

    /// From now on, a merge stops on these files, and nothing is merged.
    pub fn merges_conflict(&self, files: &[&str]) {
        if let Some(git) = self.state().git.as_mut() {
            git.conflicts = files.iter().map(|file| (*file).to_owned()).collect();
        }
    }

    fn fill(&self, now: f64) {
        let mut state = self.state();
        state.made = Some(now);
        state.settings.finishes = self.inner.replies == Replies::Live;
        let website = format!("{HOME}/Projects/website");
        let api = format!("{HOME}/Projects/api");
        let mobile = format!("{HOME}/Projects/mobile");
        let infra = format!("{HOME}/Projects/infra");
        let chats = format!("{HOME}/.kuvo-code/chats");
        state.projects.insert(website.clone(), json!({ "name": "Website", "updated": now - 2.0 * MINUTE }));
        state.projects.insert(api.clone(), json!({ "name": "API", "updated": now - 20.0 * MINUTE }));
        state.projects.insert(mobile.clone(), json!({ "name": "Mobile", "updated": now - 2.0 * DAY }));
        state.projects.insert(infra.clone(), json!({ "name": "Infra" }));

        let add = |state: &mut State, id: &str, folder: &str, title: &str, ago: f64, extra: Value| {
            let kind = if folder == chats { "chat" } else { "project" };
            let mut facts = json!({ "kind": kind, "directory": folder, "title": title, "created": now - ago - HOUR, "updated": now - ago });
            if kind == "project" {
                facts["project"] = json!(folder);
            }
            if let (Some(facts), Some(extra)) = (facts.as_object_mut(), extra.as_object()) {
                facts.extend(extra.clone());
            }
            state.sessions.insert(id.to_owned(), facts);
            state.models.insert(id.to_owned(), json!({ "providerID": "anthropic", "id": "claude-opus-5-5", "variant": "high" }));
        };
        add(&mut state, "ses_signup", &website, "Fix the sign-up form wiping on a taken email", 2.0 * MINUTE, json!({ "running": true }));
        add(&mut state, "ses_dark", &website, "Dark mode for the settings page", 2.0 * HOUR, json!({}));
        add(&mut state, "ses_cms", &website, "Move the blog to the new CMS", DAY, json!({ "asked": now - 3.0 * HOUR }));
        add(&mut state, "ses_lighthouse", &website, "Get the Lighthouse score above 90", 3.0 * DAY, json!({}));
        add(&mut state, "ses_limits", &api, "Rate limits per API key", 20.0 * MINUTE, json!({ "asked": now - 15.0 * MINUTE }));
        add(&mut state, "ses_webhook", &api, "Flaky webhook retry test", 5.0 * HOUR, json!({}));
        add(&mut state, "ses_push", &mobile, "Push notifications on Android 15", 2.0 * DAY, json!({}));
        add(&mut state, "ses_recipes", &chats, "Recipe ideas for the weekend", 4.0 * HOUR, json!({}));
        add(&mut state, "ses_lifetimes", &chats, "Explain Rust lifetimes with an example", 8.0 * DAY, json!({}));
        // A subagent's session: the lists show it nowhere.
        add(&mut state, "ses_sub", &website, "Check the other forms", MINUTE, json!({ "kind": "subagent", "parent": "ses_signup", "messages": { "via": "parent" } }));
        state.models.insert("ses_recipes".into(), json!({ "providerID": "openai", "id": "gpt-6", "variant": "medium" }));
        state.running.insert("ses_signup".into());

        state.messages.insert("ses_signup".into(), signup(now));
        state.messages.insert("ses_limits".into(), limits(now));
        state.messages.insert("ses_lighthouse".into(), long_history(now));
        state.messages.insert("ses_recipes".into(), recipes(now));
        state.asks.insert("ses_cms".into(), vec![cms_question()]);
        state.queues.insert("ses_signup".into(), signup_queue());
        // Dark mode is worked on in a worktree of the website, on a branch of its own.
        if let Some(facts) = state.sessions.get_mut("ses_dark") {
            facts["directory"] = json!(git::dark_mode_tree());
        }
        state.git = Some(git::Git::new());
        state.bots = bots::Bots::sample(now);
        state.queue = queue::Queue::sample(now);
        state.groups = groups::Groups::sample(now);
        state.sync_bots();
    }
}

/// A permission waiting in the rate limits' conversation: to install Redis's client.
pub fn redis_permission() -> Value {
    json!({
        "id": "per_redis", "kind": "permission", "session": "ses_limits", "title": "Allow running this command?",
        "message": null, "command": "npm install ioredis@5 --save", "files": [], "resources": ["npm install ioredis@5 --save"],
        "choices": [
            { "answer": "once", "title": "Allow Once", "named": null, "placeholder": null },
            { "answer": "always", "title": "Always Allow", "named": "npm install", "placeholder": null },
            { "answer": "reject", "title": "Don’t Allow", "named": null, "placeholder": null },
            { "answer": "instead", "title": "Tell It What to Do Instead…", "named": null, "placeholder": "Tell it what to do instead…" }
        ]
    })
}

/// A command the sandbox stopped in the rate limits' conversation, which its agent asks to run
/// outside it: Always names the one folder it was stopped writing in.
pub fn sandbox_ask() -> Value {
    json!({
        "id": "sq_redis", "kind": "sandbox", "session": "ses_limits", "title": "Scout wants to run a command outside the sandbox",
        "message": "The sandbox stopped it from changing a file outside api.", "command": "brew install redis",
        "stopped": [{ "label": "Write", "detail": "/opt/homebrew/Cellar/redis" }], "error": null,
        "asker": "Scout", "why": "The rate limiter's tests need a local Redis server.",
        "choices": [
            { "answer": "once", "title": "Allow Once", "named": null, "placeholder": null, "help": "Runs it outside the sandbox this once", "spoken": "Allow once: run it outside the sandbox" },
            { "answer": "always", "title": "Always Allow", "named": "/opt/homebrew/Cellar", "placeholder": null, "help": "Runs it, and from now on lets api’s commands write in /opt/homebrew/Cellar too, in the sandbox.", "spoken": "Always Allow /opt/homebrew/Cellar" },
            { "answer": "reject", "title": "Don’t Allow", "named": null, "placeholder": null, "help": "It goes on without running it", "spoken": null }
        ]
    })
}

/// An item the rate limits' agent needs from the vault: Give for each item; Always Give makes a
/// grant, which a client makes sure is you first.
pub fn vault_ask() -> Value {
    json!({
        "id": "q_redis", "kind": "vault", "session": "ses_limits", "need": "item", "title": "Scout needs the staging Redis password",
        "message": "To run the load test against staging.", "site": "redis.staging.example.com", "note": null,
        "choices": [
            { "answer": "once", "title": "Give", "named": "Staging Redis", "placeholder": null, "help": "Gives it “Staging Redis” this once", "spoken": "Give Staging Redis, this once", "item": "itm_redis" },
            { "answer": "always", "title": "Always Give", "named": "Staging Redis", "placeholder": null, "help": "Gives it “Staging Redis”, and lets this project use it from now on, without asking", "spoken": "Always give Staging Redis", "item": "itm_redis", "confirm": true },
            { "answer": "once", "title": "Give", "named": "Redis Cloud", "placeholder": null, "help": "Gives it “Redis Cloud” this once", "spoken": "Give Redis Cloud, this once", "item": "itm_cloud" },
            { "answer": "always", "title": "Always Give", "named": "Redis Cloud", "placeholder": null, "help": null, "spoken": null, "item": "itm_cloud", "confirm": true },
            { "answer": "reject", "title": "Don’t Allow", "named": null, "placeholder": null, "help": "It goes on without it", "spoken": null }
        ]
    })
}

/// What waits behind the sign-up fix's run: one of yours, and a letter from a bot.
fn signup_queue() -> Vec<Value> {
    vec![
        json!({ "id": "msg_q1", "words": "Then do the same for the password field: keep it, and say what's wrong with it", "files": [], "keyword": null, "from": null, "on": "engine", "delivery": "queue", "held": false }),
        json!({ "id": "msg_q2", "words": "The design review is at 3: the error copy should match the new tone guide.", "files": ["tone-guide.pdf"], "keyword": null, "from": "Mira", "on": "box", "delivery": "queue", "held": false }),
    ]
}

/// The files "@" finds in a session's folder.
const FILES: &[&str] = &["web/src/signup/Form.tsx", "web/src/signup/Form.test.tsx", "web/src/signup/schema.ts", "web/src/settings/Profile.tsx", "web/src/api/client.ts", "web/package.json", "README.md"];

/// The agents and sessions "&" names: bots, then sessions with titles.
fn named() -> Vec<(&'static str, &'static str, &'static str, Option<&'static str>)> {
    vec![("Mira", "bot", "ses_mira", Some("Design reviews")), ("Scout", "bot", "ses_scout", Some("Watches CI")), ("Rate limits per API key", "session", "ses_limits", Some("API")), ("Move the blog to the new CMS", "session", "ses_cms", Some("Website"))]
}

/// The agent's question in the CMS move.
pub fn cms_question() -> Value {
    json!({
        "id": "frm_cms", "kind": "question", "session": "ses_cms", "title": "Question",
        "questions": [{
            "key": "q0", "title": "Old posts", "question": "The blog has 214 posts. Should the old URLs keep working once they move?",
            "options": [
                { "value": "redirect", "label": "Redirect them", "description": "301s from each old URL to its new one" },
                { "value": "keep", "label": "Keep the old URLs", "description": "the CMS serves them as they are" },
                { "value": "drop", "label": "Let them go", "description": null }
            ],
            "multiple": false, "custom": true, "required": true
        }]
    })
}

/// The messages of the session at work: a finished turn, then one under way.
fn signup(now: f64) -> Vec<Value> {
    let t = now - 30.0 * MINUTE;
    vec![
        user("msg_s1", t, "Users say the sign-up form gets completely wiped if the email is already taken. Can you look?", None),
        json!({ "type": "assistant", "id": "msg_s2", "time": { "created": t + 1000.0, "completed": t + 25_000.0 }, "agent": "build", "model": { "providerID": "anthropic", "id": "claude-opus-5-5" }, "finish": "stop", "content": [
            { "type": "reasoning", "text": "The form likely resets its state on any error from the server, not only on success.", "time": { "created": t + 1000.0, "completed": t + 3000.0 } },
            tool("call_1", "read", "completed", json!({ "filePath": "/Users/sam/Projects/website/src/components/SignupForm.tsx" })),
            tool_with("call_2", "grep", "completed", json!({ "pattern": "reset\\(" }), json!({ "content": [{ "type": "text", "text": "Found 3 matches\nweb/src/signup/Form.tsx:\n  Line 48:     reset();\n  Line 71:   const { reset, setError } = useForm();\nweb/src/settings/Profile.tsx:\n  Line 33:     reset(profile);" }] })),
            { "type": "text", "text": "Found it. When the server answers **409** (the email is taken), the error path calls `reset()` before it shows the message, so every field is cleared:\n\n```tsx\n} catch (error) {\n  reset();\n  setError(error.message);\n}\n```\n\nMoving `reset()` into the success path keeps what was typed. Want me to do that, and put the error under the email field?" }
        ] }),
        json!({ "type": "idle", "id": "msg_s3", "time": { "created": t + 25_000.0 }, "outcome": "succeeded" }),
        // Another agent's message, and the box's note of a session's news, as their box keeps them.
        json!({ "type": "user", "id": "msg_s3a", "time": { "created": t + 60_000.0 }, "text": "[From &Check the other forms] The profile form has the same bug.", "metadata": { "kuvo": { "parts": {
            "words": "The profile form has the same bug: `Profile.tsx` calls `reset(profile)` on any error, so a taken username clears it too.\n\nI left it alone; it's yours if you want it in the same fix.",
            "mail": { "name": "Check the other forms", "session": "ses_sub" } } } } }),
        json!({ "type": "user", "id": "msg_s3b", "time": { "created": t + 90_000.0 }, "text": "[Session finished] \"Write a test for a taken email\" in website (address ses_sub3), after 2 min:\nAdded signup.taken.test.ts; it fails on main and passes with the fix.", "metadata": { "kuvo": {
            "note": { "from": "Its sessions", "line": "“Write a test for a taken email” finished" },
            "news": [{ "session": "ses_sub3", "title": "Write a test for a taken email", "status": "finished", "said": "Added signup.taken.test.ts; it fails on main and passes with the fix." }] } } }),
        // A wake-up of its own, as its box marks it on the page.
        json!({ "type": "user", "id": "msg_s3c", "time": { "created": t + 100_000.0 }, "text": "[Wake-up] Check the preview deploy: open its URL, sign up with a taken email, and say whether the form keeps what was typed.\n\nIf it's still building, schedule another wake-up in 10 minutes.", "metadata": { "kuvo": {
            "wake": { "summary": "Check the preview deploy", "note": "Check the preview deploy: open its URL, sign up with a taken email, and say whether the form keeps what was typed.\n\nIf it's still building, schedule another wake-up in 10 minutes." } } } }),
        user("msg_s4", t + 120_000.0, "Keep what they typed, and say which field is wrong", Some("/plan")),
        json!({ "type": "assistant", "id": "msg_s5", "time": { "created": t + 121_000.0 }, "agent": "build", "model": { "providerID": "anthropic", "id": "claude-opus-5-5" }, "content": [
            // Kuvo Code's own tool, by the title its box gives it.
            json!({ "type": "tool", "id": "call_k1", "name": "kuvo-browser_read_conversation", "title": "Read Check the other forms’s conversation", "doing": "Reading Check the other forms’s conversation", "state": { "status": "completed", "input": { "of": "Check the other forms" } } }),
            tool("call_r2", "read", "completed", json!({ "filePath": "/Users/sam/Projects/website/src/components/SignupForm.tsx" })),
            tool_with("call_3", "edit", "completed", json!({ "filePath": "/Users/sam/Projects/website/src/components/SignupForm.tsx" }), json!({ "metadata": { "files": [{ "file": "web/src/signup/Form.tsx", "additions": 4, "deletions": 2, "status": "modified", "patch": SIGNUP_PATCH }] } })),
            tool_with("call_t1", "shell", "failed", json!({ "command": "npx tsc --noEmit" }), json!({ "error": "error TS2304: Cannot find name 'setFieldError'.\nFound 1 error." })),
            tool_with("call_4", "shell", "completed", json!({ "command": "npm test -- signup" }), json!({ "content": [{ "type": "text", "text": test_output() }], "metadata": { "exit": 0 } })),
            { "type": "text", "text": "Moved the reset into the success path, and the 409 now sets the error on the email field. The sign-up tests pass; checking the lint" },
            tool("call_r3", "read", "completed", json!({ "filePath": "web/.eslintrc.json" })),
            tool("call_5", "shell", "running", json!({ "command": "npm run lint" }))
        ] }),
    ]
}

/// A conversation waiting on you.
fn limits(now: f64) -> Vec<Value> {
    let t = now - 40.0 * MINUTE;
    vec![
        user("msg_l1", t, "Add rate limits per API key: 100 requests a minute, with a header saying what's left.", None),
        json!({ "type": "assistant", "id": "msg_l2", "time": { "created": t + 1000.0, "completed": t + 40_000.0 }, "agent": "build", "finish": "stop", "content": [
            tool("call_l1", "read", "completed", json!({ "filePath": "api/src/middleware/auth.ts" })),
            tool("call_l2", "glob", "completed", json!({ "pattern": "api/src/**/*.ts" })),
            { "type": "text", "text": "There are two places this could live:\n\n1. **In the auth middleware**, where the key is already read. Simple, but each server counts on its own.\n2. **In Redis**, shared by every server, with a sliding window.\n\nYou run three servers behind the load balancer, so only the second gives a real 100 a minute. Should I add Redis for it?" }
        ] }),
        json!({ "type": "idle", "id": "msg_l3", "time": { "created": t + 40_000.0 }, "outcome": "succeeded" }),
    ]
}

fn recipes(now: f64) -> Vec<Value> {
    let t = now - 5.0 * HOUR;
    vec![
        user("msg_r1", t, "Three dinner ideas for Saturday, vegetarian, under an hour.", None),
        json!({ "type": "assistant", "id": "msg_r2", "time": { "created": t + 1000.0, "completed": t + 9000.0 }, "agent": "build", "finish": "stop", "content": [
            { "type": "text", "text": "- **Mushroom risotto** with lemon and thyme (45 min)\n- **Chickpea and spinach curry** with rice (35 min)\n- **Sheet-pan gnocchi** with peppers and feta (30 min)" }
        ] }),
        json!({ "type": "idle", "id": "msg_r3", "time": { "created": t + 9000.0 }, "outcome": "succeeded" }),
    ]
}

/// A long conversation, more than one page.
fn long_history(now: f64) -> Vec<Value> {
    let start = now - 4.0 * DAY;
    let audits = ["the images", "the fonts", "the scripts", "the CSS", "the caching", "the third-party tags"];
    let mut messages = Vec::new();
    for turn in 0..70 {
        let t = start + turn as f64 * 10.0 * MINUTE;
        let what = audits[turn % audits.len()];
        messages.push(user(&format!("msg_h{turn:03}a"), t, &format!("Look at {what} next (pass {}).", turn / audits.len() + 1), None));
        messages.push(json!({ "type": "assistant", "id": format!("msg_h{turn:03}b"), "time": { "created": t + 1000.0, "completed": t + 20_000.0 }, "agent": "build", "finish": "stop", "content": [
            { "type": "text", "text": format!("Checked {what}. The score went from {} to {}.", 60 + turn / 3, 61 + turn / 3) }
        ] }));
        messages.push(json!({ "type": "idle", "id": format!("msg_h{turn:03}c"), "time": { "created": t + 20_000.0 }, "outcome": "succeeded" }));
    }
    messages
}

fn user(id: &str, at: f64, words: &str, keyword: Option<&str>) -> Value {
    let mut parts = json!({ "words": words });
    if let Some(keyword) = keyword {
        parts["keyword"] = json!(keyword);
    }
    json!({ "type": "user", "id": id, "time": { "created": at }, "text": words, "metadata": { "kuvo": { "parts": parts } } })
}

fn tool(id: &str, name: &str, status: &str, input: Value) -> Value {
    json!({ "type": "tool", "id": id, "name": name, "state": { "status": status, "input": input } })
}

/// A tool step with more to its state: what it printed (`content`), what it changed (`metadata`).
fn tool_with(id: &str, name: &str, status: &str, input: Value, more: Value) -> Value {
    let mut step = tool(id, name, status, input);
    if let (Some(state), Some(more)) = (step["state"].as_object_mut(), more.as_object()) {
        state.extend(more.clone());
    }
    step
}

/// The sign-up form's fix, as the engine keeps an edit's changes.
const SIGNUP_PATCH: &str = "Index: web/src/signup/Form.tsx\n===================================================================\n--- web/src/signup/Form.tsx\n+++ web/src/signup/Form.tsx\n@@ -44,9 +44,11 @@ export function SignupForm() {\n   async function save(values: Values) {\n     try {\n       await api.post(\"/accounts\", values);\n+      reset();\n     } catch (error) {\n-      reset();\n-      setError(error.message);\n+      if (error.status === 409) {\n+        setError(\"email\", { message: \"That email already has an account.\" });\n+      }\n     }\n   }\n \n";

/// What the sign-up tests printed: long enough to fold.
fn test_output() -> String {
    let mut lines = vec!["> web@2.4.0 test".to_owned(), "> vitest run signup".to_owned(), String::new(), " RUN  v3.2.4 /Users/sam/Projects/website/web".to_owned(), String::new()];
    let cases = ["keeps what was typed when the email is taken", "says which field is wrong", "clears the form once the account is made", "shows the server's message for other errors", "doesn't send twice on a double click", "trims spaces around the email", "accepts a plus in the email", "asks for a longer password", "remembers the plan picked", "focuses the first field with an error"];
    for (n, case) in cases.iter().enumerate() {
        lines.push(format!(" ✓ signup/Form.test.tsx > SignupForm > {case} {}ms", 12 + n * 7));
    }
    lines.extend([String::new(), " Test Files  1 passed (1)".to_owned(), "      Tests  10 passed (10)".to_owned(), "   Start at  10:42:17".to_owned(), "   Duration  1.84s (transform 210ms, setup 0ms, collect 640ms, tests 96ms)".to_owned()]);
    lines.join("\n")
}

/// A cursor's words, as the box says them (`s<shown>.<step>`): how far a project's list shows, and
/// how many times "Show more" opened it.
fn shown_of(cursor: &str) -> Option<(usize, u32)> {
    let (shown, step) = cursor.strip_prefix('s')?.split_once('.')?;
    Some((shown.parse().ok()?, step.parse().ok()?))
}

impl State {
    /// A project's list as far as `shown`, as the box pages it: pinned, then at work or waiting on
    /// you, then newest; "Show more" adding 10, then twice as many each time.
    fn page(&self, project: &str, shown: usize, step: u32) -> Value {
        let mut rows: Vec<(&String, &Value)> = self
            .sessions
            .iter()
            .filter(|(_, facts)| facts["kind"] == json!("project") && facts["archived"] != json!(true) && facts["project"] == json!(project))
            .collect();
        let rank = |facts: &Value| if facts["pinned"] == json!(true) { 0 } else if facts["running"] == json!(true) || facts["asked"].is_number() { 1 } else { 2 };
        rows.sort_by(|a, b| rank(a.1).cmp(&rank(b.1)).then_with(|| b.1["updated"].as_f64().unwrap_or(0.).partial_cmp(&a.1["updated"].as_f64().unwrap_or(0.)).unwrap_or(std::cmp::Ordering::Equal)).then_with(|| a.0.cmp(b.0)));
        let ids: Vec<&String> = rows.iter().take(shown).map(|(id, _)| *id).collect();
        let count = (rows.len() - ids.len()).min(10 * 2usize.pow(step));
        let more = (count > 0).then(|| json!({ "cursor": format!("s{}.{}", shown + count, step + 1), "count": count }));
        json!({ "project": project, "sessions": ids, "total": rows.len(), "more": more })
    }

    /// The session list, as a box says it: each project with its sums, and "Needs you".
    fn line(&self, full: bool) -> Value {
        let listed = |facts: &Value| matches!(facts["kind"].as_str(), Some("project" | "chat")) && facts["archived"] != json!(true);
        let mut projects = self.projects.clone();
        for (directory, project) in &mut projects {
            let its: Vec<&Value> = self.sessions.values().filter(|facts| listed(facts) && facts["project"] == json!(directory)).collect();
            // (One that waits on you isn't counted at work.)
            project["asking"] = json!(its.iter().filter(|facts| facts["asked"].is_number()).count());
            project["working"] = json!(its.iter().filter(|facts| facts["running"] == json!(true) && !facts["asked"].is_number()).count());
            // Its first six, as the box pages lists.
            let page = self.page(directory, 6, 0);
            project["shown"] = page["sessions"].clone();
            project["total"] = page["total"].clone();
            if !page["more"].is_null() {
                project["more"] = page["more"].clone();
            }
        }
        // The stand-in acts as the server; these are its archived-list answer rows.
        let mut archived: Vec<(&String, &Value)> = self.sessions.iter().filter(|(_, facts)| facts["archived"] == json!(true)).collect();
        archived.sort_by(|a, b| b.1["updated"].as_f64().unwrap_or(0.).partial_cmp(&a.1["updated"].as_f64().unwrap_or(0.)).unwrap_or(std::cmp::Ordering::Equal));
        let mut groups: Vec<Value> = Vec::new();
        for (id, facts) in archived {
            let directory = facts["project"].as_str().unwrap_or_else(|| facts["directory"].as_str().unwrap_or(""));
            let kind = facts["kind"].as_str().unwrap_or("project");
            if let Some(group) = groups.iter_mut().find(|group| group["directory"] == directory && group["kind"] == kind) {
                group["sessions"].as_array_mut().unwrap().push(json!(id));
            } else {
                groups.push(json!({"directory": directory, "title": if kind == "chat" { "Chats" } else { projects.get(directory).and_then(|p| p["name"].as_str()).unwrap_or(directory) }, "kind": kind, "sessions": [id]}));
            }
        }
        let sessions = serde_json::from_value(json!(self.sessions)).unwrap_or_default();
        let listed_projects = serde_json::from_value(json!(projects)).unwrap_or_default();
        let order: Vec<String> = kuvo_box::sessions::sidebar(&sessions, &listed_projects).projects.into_iter().map(|group| group.directory).collect();
        json!({ "at": self.at.to_string(), "full": full, "sessions": self.sessions, "projects": projects, "projectOrder": order, "startProject": order.first(), "needsYou": self.needs_you(), "archivedGroups": groups })
    }

    /// The computer's name, as its `kuvo/info` says it.
    fn name(&self) -> String {
        self.name.clone().unwrap_or_else(|| if self.elsewhere { "build-linux".into() } else { NAME.into() })
    }

    /// The sessions waiting on you that the lists show, the one that asked last first, each with
    /// what it asks.
    fn needs_you(&self) -> Vec<Value> {
        let mut asked: Vec<(&String, f64)> = self
            .sessions
            .iter()
            .filter(|(_, facts)| matches!(facts["kind"].as_str(), Some("project" | "chat")) && facts["archived"] != json!(true))
            .filter_map(|(id, facts)| facts["asked"].as_f64().map(|asked| (id, asked)))
            .collect();
        asked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        asked.into_iter().map(|(session, _)| json!({ "session": session, "asks": areas::asks_of(session) })).collect()
    }

    /// One of the areas' answers (`kuvo/waiting`, `kuvo/computers`…), as it is now.
    fn area(&self, method: &str, route: &str) -> Option<(u16, Value)> {
        let route = if route == "kuvo/setup/waiting/counts" { "kuvo/waiting" } else { route };
        areas::answer(&self.areas, method, route, self.made.unwrap_or_else(now_ms), self.needs_you().len(), self.bots.asking())
    }

    /// Each bot's own conversation is one of the box's sessions (kind `bot`, which the lists show
    /// nowhere), and goes with its bot.
    fn sync_bots(&mut self) {
        // Those following the bots hear them as they are now.
        if let Some((200, list)) = self.bots.answer("GET", "kuvo/bots", &|_| None, &Value::Null, &[], 0.) {
            let line = list.to_string();
            self.bots_followers.retain(|_, sender| sender.send(line.clone()).is_ok());
        }
        let now = self.made.unwrap_or_else(now_ms);
        let theirs: Vec<(String, String, String, String)> = self.bots.bots.iter().filter_map(|bot| Some((bot["session"].as_str()?.to_owned(), bot["name"].as_str()?.to_owned(), bot["folder"].as_str()?.to_owned(), bot["id"].as_str()?.to_owned()))).collect();
        let before = self.sessions.len();
        self.sessions.retain(|id, facts| facts["kind"] != json!("bot") || theirs.iter().any(|(session, ..)| session == id));
        let mut changed = self.sessions.len() != before;
        for (session, name, folder, bot) in theirs {
            if !self.sessions.contains_key(&session) {
                self.sessions.insert(session.clone(), json!({ "kind": "bot", "directory": folder, "title": name, "created": now, "updated": now, "bot": bot }));
                self.models.insert(session.clone(), json!({ "providerID": "anthropic", "id": "claude-opus-5-5", "variant": "high" }));
                self.messages.insert(session.clone(), bots::conversation(&session, now).unwrap_or_default());
                changed = true;
            }
        }
        if changed {
            self.list_changed();
        }
    }

    fn wake_schedule(&self, session: &str) -> Value {
        self.wakes.get(session).cloned().unwrap_or_else(|| json!({ "wakes": [], "watches": [], "paused": false }))
    }

    /// A conversation's asks, queue, or wake-ups, as its follower reads them.
    fn followed(&self, session: &str, what: Followed) -> String {
        match what {
            Followed::Wakes => self.wake_schedule(session),
            Followed::Outputs => json!({ "artifacts": self.outputs.get(session).cloned().unwrap_or_default(), "next": null }),
            Followed::Asks => json!({ "asks": self.asks.get(session).cloned().unwrap_or_default() }),
            Followed::Queue => json!({ "paused": self.paused.contains(session), "held": [], "items": self.queues.get(session).cloned().unwrap_or_default(), "line": self.queue_lines.get(session) }),
        }
        .to_string()
    }

    /// Those following `session`'s asks or queue hear it as it is now.
    fn told(&mut self, session: &str, what: Followed) {
        let line = self.followed(session, what);
        self.followers.retain(|_, (followed, which, sender)| followed != session || *which != what || sender.send(line.clone()).is_ok());
    }

    /// Those following a folder in `root` hear its changed files as they are now.
    fn git_changed(&mut self, root: &str) {
        let Some(git) = self.git.as_ref() else { return };
        let lines: Vec<(u64, String)> = self
            .git_followers
            .iter()
            .filter(|(_, (directory, _))| git.repo_of(directory) == Some(root))
            .map(|(id, (directory, _))| (*id, git.files(directory).to_string()))
            .collect();
        for (id, line) in lines {
            if let Some((_, sender)) = self.git_followers.get(&id)
                && sender.send(line).is_err()
            {
                self.git_followers.remove(&id);
            }
        }
    }

    /// Those following `group` hear it as it is now (one deleted: their follow ends).
    fn group_changed(&mut self, group: &str) {
        let line = self.groups.line(group);
        self.group_followers.retain(|_, (followed, sender)| followed != group || line.as_ref().is_some_and(|line| sender.send(line.clone()).is_ok()));
        let list = self.groups_list().to_string();
        self.groups_followers.retain(|_, sender| sender.send(list.clone()).is_ok());
    }

    /// The chats as `kuvo/groups` lists them now.
    fn groups_list(&self) -> Value {
        let bots: Vec<Value> = self.bots.bots.iter().chain(&self.bots.elsewhere).cloned().collect();
        self.groups.list(&bots)
    }

    fn list_changed(&mut self) {
        self.at += 1;
        let line = self.line(true).to_string();
        self.lists.retain(|_, sender| sender.send(line.clone()).is_ok());
        // What waits on you may have changed with it.
        if let Some((_, waiting)) = self.area("GET", "kuvo/waiting") {
            let line = waiting.to_string();
            self.waits.retain(|_, sender| sender.send(line.clone()).is_ok());
        }
    }

    fn emit(&mut self, kind: &str, data: Value) {
        let event = json!({ "id": format!("evt_{}", self.next), "type": kind, "created": now_ms(), "data": data }).to_string();
        self.next += 1;
        self.events.retain(|_, sender| sender.send(event.clone()).is_ok());
    }

    fn id(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}_{:06}", self.next)
    }

    fn is_conversation(facts: &Value) -> bool {
        matches!(facts["kind"].as_str(), Some("project" | "chat"))
    }

    fn is_subagent(facts: &Value) -> bool {
        facts["kind"] == json!("subagent") || facts["parent"].is_string()
    }

    fn worktree_delete_sentence(&self, excluding: &[&str]) -> &'static str {
        let mut has_uncommitted_worktree = false;
        let mut has_clean_worktree = false;
        let mut has_kept = false;
        for id in excluding {
            let Some(facts) = self.sessions.get(*id) else { continue };
            let Some(dir) = facts["directory"].as_str() else { continue };
            if self.kept_worktree_changes.contains(dir) {
                let other_kept = self.sessions.iter().any(|(other_id, other)| {
                    !excluding.contains(&other_id.as_str())
                        && Self::is_conversation(other)
                        && other["directory"] == json!(dir)
                });
                if !other_kept {
                    has_kept = true;
                }
            }
            if let Some(repo) = self.git.as_ref().and_then(|git| git.repos.get(dir))
                && repo.into.is_some()
            {
                let other_active = self.sessions.iter().any(|(other_id, other)| {
                    !excluding.contains(&other_id.as_str())
                        && Self::is_conversation(other)
                        && other["archived"] != json!(true)
                        && other["directory"] == json!(dir)
                });
                if !other_active {
                    let working_here = self.sessions.iter().any(|(other_id, other)| {
                        Self::is_conversation(other)
                            && other["directory"] == json!(dir)
                            && self.running.contains(other_id)
                    });
                    if !repo.files.is_empty() || repo.staged_elsewhere > 0 || repo.underway.is_some() || working_here {
                        has_uncommitted_worktree = true;
                    } else {
                        has_clean_worktree = true;
                    }
                }
            }
        }
        if has_uncommitted_worktree {
            "Its worktree and changes that weren’t committed are deleted too."
        } else if has_clean_worktree {
            "Its worktree is deleted too."
        } else if has_kept {
            "The changes kept to restore its worktree are deleted too."
        } else {
            "This can’t be undone."
        }
    }

    fn delete_confirm_for(&self, session: &str, facts: &Value) -> Value {
        let title = facts["title"]
            .as_str()
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .unwrap_or("New Session");
        let message = self.worktree_delete_sentence(&[session]);
        json!({
            "title": format!("Delete “{title}”?"),
            "message": message,
            "label": "Delete"
        })
    }

    fn session_actions_response(&self, session: &str) -> (u16, Value) {
        let Some(facts) = self.sessions.get(session) else {
            return (404, json!({ "message": "There’s no such session." }));
        };
        if Self::is_subagent(facts) {
            return (409, json!({ "message": "Subagent sessions change with their parent session." }));
        }
        let archived = facts["archived"] == json!(true);
        let pinned = facts["pinned"] == json!(true);
        let engine_archived = self.engine_archived.contains(session);
        let delete = json!({
            "id": "delete",
            "label": "Delete",
            "destructive": true,
            "confirm": self.delete_confirm_for(session, facts)
        });
        let mut actions = Vec::new();
        if archived {
            if !engine_archived {
                actions.push(json!({ "id": "unarchive", "label": "Unarchive" }));
            }
            actions.push(delete);
        } else {
            actions.push(if pinned { json!({ "id": "unpin", "label": "Unpin" }) } else { json!({ "id": "pin", "label": "Pin" }) });
            actions.push(json!({ "id": "rename", "label": "Rename" }));
            actions.push(json!({ "id": "archive", "label": "Archive" }));
            actions.push(delete);
        }
        let mut body = json!({ "session": session, "actions": actions });
        if let Some(held) = self.held_sessions.get(session) {
            body["held"] = json!(held);
        }
        (200, body)
    }
}

/// A followed answer read as it comes: each string sent is a piece of it; it ends once its sender's
/// gone (the stand-in let it go, or what followed it closed it).
struct Pieces {
    receiver: Receiver<String>,
    piece: Vec<u8>,
    at: usize,
    sse: bool,
}

impl Read for Pieces {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let available = self.fill_buf()?;
        let count = available.len().min(out.len());
        out[..count].copy_from_slice(&available[..count]);
        self.consume(count);
        Ok(count)
    }
}

impl BufRead for Pieces {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        if self.at >= self.piece.len() {
            match self.receiver.recv() {
                Ok(next) => {
                    self.piece = if self.sse { format!("data: {next}\n\n") } else { format!("{next}\n") }.into_bytes();
                    self.at = 0;
                }
                Err(_) => return Ok(&[]),
            }
        }
        Ok(&self.piece[self.at..])
    }

    fn consume(&mut self, count: usize) {
        self.at += count;
    }
}

fn answer(status: u16, body: Value) -> Result<Response> {
    Ok(Response { status, content_type: Some("application/json".into()), body: body.to_string().into_bytes() })
}

fn query<'a>(path: &'a str, name: &str) -> Option<&'a str> {
    path.split_once('?')?.1.split('&').find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
}

fn decoded(value: &str) -> String {
    percent_decode(value)
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(byte) = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The models offered here, and their efforts.
fn models() -> Vec<(Value, Vec<&'static str>)> {
    vec![
        (json!({ "providerID": "anthropic", "id": "claude-opus-5-5", "name": "Claude Opus 5.5" }), vec!["low", "medium", "high", "max"]),
        (json!({ "providerID": "anthropic", "id": "claude-sonnet-5-5", "name": "Claude Sonnet 5.5" }), vec!["low", "medium", "high"]),
        (json!({ "providerID": "openai", "id": "gpt-6", "name": "GPT-6" }), vec!["low", "medium", "high", "xhigh"]),
        (json!({ "providerID": "google", "id": "gemini-3-pro", "name": "Gemini 3 Pro" }), vec![]),
    ]
}

/// The providers' names, as the box names them.
const PROVIDERS: &[(&str, &str)] = &[("anthropic", "Anthropic"), ("openai", "OpenAI"), ("google", "Google"), ("opencode", "OpenCode Zen"), ("openrouter", "OpenRouter"), ("deepseek", "DeepSeek"), ("mistral", "Mistral"), ("groq", "Groq"), ("claude-code", "Claude Code")];

/// `count` more models, spread over other providers (made up).
fn more_models(count: usize) -> Vec<(Value, Vec<&'static str>)> {
    let providers = [("openrouter", "OpenRouter"), ("deepseek", "DeepSeek"), ("mistral", "Mistral"), ("groq", "Groq")];
    (0..count)
        .map(|ix| {
            let (provider, maker) = providers[ix % providers.len()];
            (json!({ "providerID": provider, "id": format!("model-{ix}"), "name": format!("{maker} Model {ix}") }), vec![])
        })
        .collect()
}

/// Claude Code's models, as each account lists them.
const CLAUDE_MODELS: &[(&str, &str)] = &[("sonnet", "Sonnet 5.5"), ("fable", "Fable 5.2"), ("opus", "Opus 5.5"), ("haiku", "Haiku 5.5")];

/// Each Claude Code account's provider, and the provider as the box says it (`models.offered.accounts`).
fn claude_accounts(accounts: &[(String, Option<String>)]) -> Vec<(String, Value)> {
    accounts
        .iter()
        .enumerate()
        .map(|(ix, (name, plan))| {
            let (provider, id) = if ix == 0 { ("claude-code".to_owned(), "default".to_owned()) } else { (format!("claude-code@account{ix}"), format!("account{ix}")) };
            let mut account = json!({ "agent": "claude", "id": id, "agentName": "Claude Code", "name": name });
            if let Some(plan) = plan {
                account["plan"] = json!(plan);
            }
            let title = if ix == 0 { "Claude Code".to_owned() } else { format!("Claude Code · {name}") };
            (provider.clone(), json!({ "id": provider, "name": title, "account": account }))
        })
        .collect()
}

/// Codex's models, as its app-server lists them (made up), the first six with Fast.
fn codex_models() -> Vec<(Value, Vec<&'static str>)> {
    let names = [("gpt-6.2-sol", "GPT-6.2-Sol"), ("gpt-6.2-astra", "GPT-6.2-Astra"), ("gpt-6.1-terra", "GPT-6.1 Terra"), ("gpt-6.1-sol", "GPT-6.1-Sol"), ("gpt-6-astra", "GPT-6-Astra"), ("gpt-6-luna", "GPT-6-Luna"), ("gpt-6-mini", "GPT-6-Mini"), ("gpt-5.5-codex", "GPT-5.5 Codex"), ("gpt-5.5", "GPT-5.5")];
    names
        .iter()
        .enumerate()
        .map(|(ix, (id, name))| {
            let mut model = json!({ "providerID": "codex-cli", "id": id, "name": name });
            if ix < 6 {
                model["speeds"] = json!([{ "id": "priority", "name": "Fast", "description": "1.5x speed" }]);
            }
            (model, vec!["low", "medium", "high", "xhigh", "max"])
        })
        .collect()
}

/// The coding agents' models, as their bridge lists them.
fn agents_models() -> Vec<(Value, Vec<&'static str>)> {
    vec![(json!({ "providerID": "claude-code", "id": "opus", "name": "Claude Code · Opus", "aliases": ["default"] }), vec!["low", "medium", "high"])]
}

fn model_name(model: &Value) -> Option<String> {
    models().into_iter().chain(agents_models()).chain(first_run::free_models()).find(|(each, _)| each["providerID"] == model["providerID"] && each["id"] == model["id"]).and_then(|(each, _)| each["name"].as_str().map(str::to_owned))
}

impl StandIn {
    fn route(&self, request: &Request) -> Result<Response> {
        let path = request.path.as_str();
        let route = request.route();
        let method = request.method.as_str();
        let mut state = self.state();
        state.seen.push(Seen { method: method.into(), path: path.into(), body: request.body.clone() });
        let segments: Vec<&str> = route.split('/').collect();
        if route.starts_with("api/pty") {
            let directory = request.headers.iter().find(|(name, _)| name == "x-opencode-directory").map(|(_, value)| decoded(value));
            if let Some((status, value)) = state.terminals.answer(method, route, request.body.as_ref(), directory) {
                return answer(status, value);
            }
        }
        if let Some((status, value)) = moves::answer(&mut state, request) {
            return answer(status, value);
        }
        // The coding agents' bridge not running: what needs it answers with their state.
        if let Some((status, value)) = state.settings.agents_away_answer(method, route, &|name| query(path, name).map(decoded)) {
            return answer(status, value);
        }
        {
            let (body, made) = (request.body.clone().unwrap_or(Value::Null), state.made.unwrap_or_else(now_ms));
            // Your setup, where the stand-in is in one with others.
            if (method, route) == ("GET", "kuvo/setup")
                && !state.first_run.setup_fails
                && let Some(((status, value), _)) = areas::setup(&mut state.areas, method, route, &body)
            {
                return answer(status, value);
            }
            // Whether a first run has anything to ask, as a box decides it (`kuvo/setup/ready`).
            if !state.older && (method, route) == ("GET", "kuvo/setup/ready") {
                if state.first_run.ready_fails {
                    return answer(503, json!({ "message": "Kuvo on this computer isn’t answering." }));
                }
                let runs = !state.first_run.only_free && !state.nothing_runs;
                let needing = state.settings.needs_you();
                let ready = state.first_run.ready_says.unwrap_or(runs && needing.as_ref().is_some_and(Vec::is_empty));
                let pending: &[&str] = if needing.is_none() { &["agents"] } else { &[] };
                return answer(200, json!({ "ready": ready, "runs": runs, "needsYou": needing.unwrap_or_default(), "pending": pending }));
            }
            // (A folder its git lists is git's to answer, with its files; the home's are these.)
            let gits = route == "kuvo/folders" && query(path, "path").map(decoded).is_some_and(|folder| state.git.as_ref().is_some_and(|git| git.folders.contains_key(&folder)));
            if !state.older
                && !gits
                && let Some((status, value)) = state.first_run.answer(method, route, &|name| query(path, name).map(decoded), &body, made)
            {
                return answer(status, value);
            }
            if let Some((status, value)) = state.permissions.answer(method, route, &body) {
                return answer(status, value);
            }
        }
        {
            let now = state.made.unwrap_or_else(now_ms);
            let body = request.body.clone().unwrap_or(Value::Null);
            let state = &mut *state;
            state.panel.now = now;
            if let Some((status, value)) = state.panel.answer(method, route, |name| query(path, name).map(decoded), &body, &state.work_bars, state.git.as_ref()) {
                return answer(status, value);
            }
        }
        if let Some(git) = state.git.as_mut() {
            if (method, route) == ("GET", "kuvo/file/text") {
                use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
                let file = query(path, "path64").and_then(|path| URL_SAFE_NO_PAD.decode(path).ok()).and_then(|bytes| String::from_utf8(bytes).ok()).unwrap_or_else(|| {
                    let uri = query(path, "uri").map(decoded).unwrap_or_default();
                    percent_decode(uri.trim_start_matches("file://"))
                });
                return match git.text(&file) {
                    Some(said) => answer(200, said),
                    None => answer(404, json!({ "message": "That file isn’t there anymore." })),
                };
            }
            let body = request.body.clone().unwrap_or(Value::Null);
            if let Some((status, value, changed)) = git.answer(method, route, |name| query(path, name).map(decoded), &body) {
                if let Some(root) = changed {
                    state.git_changed(&root);
                }
                return answer(status, value);
            }
        }
        if (method, route) == ("GET", "kuvo/artifacts") {
            let session = query(path, "session").map(decoded).unwrap_or_default();
            return answer(200, json!({ "artifacts": state.outputs.get(&session).cloned().unwrap_or_default(), "next": null }));
        }
        if method == "GET" && let Some(bytes) = state.output_bytes.get(route) {
            return Ok(Response { status: 200, content_type: Some("application/octet-stream".into()), body: bytes.clone() });
        }
        // A file on its computer, by its address (`kuvo/file` and `kuvo/file/info`).
        if (method, route) == ("GET", "kuvo/file") || route == "kuvo/file/info" {
            let info = route == "kuvo/file/info";
            if method != "GET" { return answer(405, json!({})); }
            let uri = query(path, "uri").map(decoded).unwrap_or_default();
            let Some(file) = uri.strip_prefix("file://").map(decoded) else { return answer(400, json!({ "message": "Give a file:// address." })) };
            let name = file.rsplit('/').next().unwrap_or(&file);
            let (kind, mime) = kuvo_box::shown_files::guess_type(&file);
            return match state.files.get(&file) {
                Some(bytes) if info => answer(200, json!({ "exists": true, "name": name, "size": bytes.len(), "kind": kind, "mime": mime })),
                None if info => answer(200, json!({ "exists": false, "name": name, "size": null, "kind": null, "mime": null })),
                Some(bytes) if query(path, "withMode") == Some("1") => Ok(Response { status: 200, content_type: Some("application/vnd.kuvo.file; executable=0".into()), body: bytes.clone() }),
                Some(bytes) => Ok(Response { status: 200, content_type: Some(mime.into()), body: bytes.clone() }),
                None => answer(404, json!({ "message": "There's no such file.", "reason": "fileNotFound" })),
            };
        }
        let body = request.body.clone().unwrap_or(Value::Null);
        let made = state.made.unwrap_or_else(now_ms);
        if method == "GET" && route.starts_with("kuvo/work-queue/items/")
            && let Some(bytes) = state.files.get(route)
        {
            return Ok(Response { status: 200, content_type: Some("application/octet-stream".into()), body: bytes.clone() });
        }
        if let Some((status, value)) = state.queue.answer(method, path, &body, made) {
            return answer(status, value);
        }
        let made = state.made.unwrap_or_else(now_ms);
        let bots: Vec<Value> = state.bots.bots.iter().chain(&state.bots.elsewhere).cloned().collect();
        if let ("GET", ["kuvo", "groups", group, "mentions"]) = (method, route.split('/').collect::<Vec<_>>().as_slice()) {
            let typed = query(path, "q").map(decoded).unwrap_or_default();
            return match state.groups.mentions(group, &typed, &bots) {
                Some(mentions) => answer(200, json!({ "mentions": mentions })),
                None => answer(404, json!({ "message": "There’s no such group chat here." })),
            };
        }
        if let Some((status, value, changed)) = state.groups.answer(method, route, &body, &bots, if method == "GET" { made } else { now_ms() }) {
            if let Some(group) = changed {
                state.group_changed(&group);
                if self.inner.replies == Replies::Live && method == "POST" && route == format!("kuvo/groups/{group}") {
                    drop(state);
                    self.group_reply(group);
                }
            }
            return answer(status, value);
        }
        // Using a reset (`limits/reset`): asked, it offers one; confirmed, it's used.
        if (method, route) == ("POST", "kuvo/agents/api/kuvo/limits/reset") {
            let row = body["row"].clone();
            return if body.get("confirm").is_none() {
                answer(200, json!({ "data": { "confirm": "rsc_stand_in", "row": row, "service": "codex", "count": 2, "on": { "key": "here-key", "name": "This Mac", "here": true }, "always": false, "expires": made + 60_000.0 } }))
            } else {
                answer(200, json!({ "data": { "outcome": "reset" } }))
            };
        }
        if (method, route) == ("POST", "kuvo/agents/api/kuvo/limits/refresh") && !state.areas.usage_refresh_failed {
            state.areas.usage_refresh_pending = state.areas.usage_refresh_delayed;
            state.areas.usage_refresh_completed = !state.areas.usage_refresh_pending;
        }
        if route.starts_with("kuvo/marketplace") {
            let asked = |name: &str| query(path, name).map(decoded);
            let body = request.body.clone().unwrap_or(Value::Null);
            let name = state.name();
            let now = if method == "GET" { made } else { now_ms() };
            let state = &mut *state;
            if let Some((status, kind, bytes)) = state.marketplace.answer(method, route, &asked, &body, &name, &mut state.bots, now) {
                if method == "POST" && route.ends_with("/add") && status == 201 {
                    state.sync_bots();
                }
                return Ok(Response { status, content_type: Some(kind.into()), body: bytes });
            }
        }
        if route.starts_with("kuvo/library") {
            let asked = |name: &str| query(path, name).map(decoded);
            if let Some((status, kind, body)) = state.library.answer(method, route, &asked, made) {
                if method == "POST" && status < 300 {
                    state.library.tell(made);
                }
                return Ok(Response { status, content_type: Some(kind.into()), body });
            }
        }
        let sample = state.areas.insights.unwrap_or(if state.areas.empty { insights::Sample::Little } else { insights::Sample::Full });
        if let Some((status, value)) = insights::answer(method, route, |name| query(path, name).map(decoded), made, sample) {
            return answer(status, value);
        }
        if let Some((status, value)) = state.area(method, route) {
            return answer(status, value);
        }
        if let Some((status, value)) = areas::fleet(&mut state.areas, method, route, &request.body.clone().unwrap_or(Value::Null)) {
            // Those following your computers hear the list as it is now.
            if status < 300 && let Some((_, now)) = state.area("GET", "kuvo/computers") {
                let line = now.to_string();
                state.computers.retain(|_, sender| sender.send(line.clone()).is_ok());
            }
            return answer(status, value);
        }
        let added = areas::adding(&mut state.areas, method, route, &request.body.clone().unwrap_or(Value::Null));
        let body = request.body.clone().unwrap_or(Value::Null);
        let setup = areas::setup(&mut state.areas, method, route, &body);
        if let Some(((status, value), changed)) = setup.or(added).or_else(|| areas::manage(&mut state.areas, method, route, &body)) {
            // Those following your computers hear the list as it is now.
            if changed && let Some((_, now)) = state.area("GET", "kuvo/computers") {
                let line = now.to_string();
                state.computers.retain(|_, sender| sender.send(line.clone()).is_ok());
            }
            return answer(status, value);
        }
        // A folder kept as a project, as the box's `PUT kuvo/projects` keeps it (one kept stays).
        if (method, route) == ("PUT", "kuvo/projects") {
            let Some(directory) = query(path, "directory").map(decoded).filter(|directory| directory.starts_with('/')) else {
                return answer(400, json!({ "message": "Say which folder (directory)." }));
            };
            let name = directory.rsplit('/').find(|part| !part.is_empty()).unwrap_or(&directory).to_owned();
            state.projects.entry(directory.clone()).or_insert_with(|| json!({ "name": name }));
            state.list_changed();
            return answer(200, json!({ "project": { "directory": directory, "folders": [] } }));
        }
        let projects: Vec<(String, String)> = state.projects.iter().map(|(directory, project)| (directory.clone(), project["name"].as_str().unwrap_or_default().to_owned())).collect();
        let now = state.made.unwrap_or_else(now_ms);
        // (A box from before a bot's job fields doesn't know the route that takes them: it answers
        // as an older box does.)
        if method == "PATCH" && route.starts_with("kuvo/bots/") && (body.get("jobLine").is_some() || body.get("jobDetails").is_some())
            && state.lacks.iter().any(|name| name == "bots.job-fields")
        {
            return answer(404, json!({}));
        }
        if let Some((status, value)) = state.bots.answer(method, route, &|name| query(path, name).map(decoded), &body, &projects, now) {
            state.sync_bots();
            return answer(status, value);
        }
        let (name, here) = (state.name(), !state.elsewhere);
        if route == "kuvo/sign-in/action" && state.lacks.iter().any(|name| name == kuvo_box::accounts::SIGN_IN_ACTION) {
            if path.contains("%40") {
                return Ok(Response { status: 403, content_type: Some("text/plain".into()), body: b"That isn't available over the link.".to_vec() });
            }
            return answer(404, json!({}));
        }
        // (Its own page says what macOS allows its helper, as its permissions' route does.)
        state.settings.computer.permissions = state.permissions.clone();
        if let Some((status, value)) = state.settings.answer(method, route, &|name| query(path, name).map(decoded), &body, &name, here) {
            return answer(status, value);
        }
        let folder = request.headers.iter().find(|(name, _)| name == "x-opencode-directory").map(|(_, value)| decoded(value));
        let bars = {
            let state = &mut *state;
            state.work_bars.answer(method, route, &body, folder.as_deref(), &mut state.settings.approval)
        };
        if let Some((status, value)) = bars {
            return answer(status, value);
        }
        match (method, segments.as_slice()) {
            ("GET", ["kuvo", "wakes", session]) => answer(200, state.wake_schedule(session)),
            ("POST", ["kuvo", "wakes", session, "cancel"]) => {
                let Some(schedule) = state.wakes.get_mut(*session) else { return answer(404, json!({})); };
                let (kind, field, key) = if let Some(key) = body["wake"].as_str() { ("wakes", "cancelKey", key) }
                    else if let Some(key) = body["watch"].as_str() { ("watches", "letters", key) }
                    else { return answer(400, json!({})); };
                let entries = schedule[kind].as_array_mut().expect("schedule entries");
                let before = entries.len();
                entries.retain(|entry| entry[field] != key);
                if entries.len() == before { return answer(404, json!({})); }
                let schedule = schedule.clone();
                if let Some(bot) = state.bots.bots.iter_mut().find(|bot| bot["session"] == *session) {
                    bot["wakes"] = schedule["wakes"][0]["due"].clone();
                    bot["watching"] = json!(schedule["watches"].as_array().map_or(0, Vec::len));
                }
                state.sync_bots();
                state.told(session, Followed::Wakes);
                answer(200, json!({}))
            }
            ("GET", ["kuvo", "info"]) => {
                let mut features: Vec<&str> = kuvo_box::gate::REQUIRED.to_vec();
                features.extend([
                    "queue", "asks", "file.info", "sign-in.return", kuvo_box::accounts::ACCOUNT_CATALOG,
                    kuvo_box::accounts::SIGN_IN_ACTION, kuvo_box::folders::FOLDER_PATHS, kuvo_box::folders::FILE_TEXT_PATH,
                    kuvo_box::sessions::LIST_START, kuvo_box::models::MODEL_DEFAULTS, kuvo_box::models::MODEL_START_CHOICE,
                ]);
                features.extend(state.queue.features());
                if !state.before_update_here { features.push("computers.update.here"); }
                if state.areas.fleet.is_some() { features.push(crate::fleet::FEATURE); }
                // (A machine's box shows its browser live; a Mac's has none of its own.)
                if state.elsewhere { features.push(kuvo_box::browser_view::FEATURE); }
                if state.delivery_enabled { features.push(kuvo_box::queue::DELIVERY_FEATURE); }
                if state.setup_lists { features.extend([kuvo_box::sessions::SETUP_LISTS, kuvo_box::sessions::MERGED_SESSIONS]); }
                features.push(crate::insights::SUMMARY);
                features.retain(|name| !state.lacks.iter().any(|lacks| lacks == name));
                if state.older { features.clear(); }
                // (As a box says it of itself, by the list the app draws.)
                let missing: Vec<&str> = kuvo_box::gate::REQUIRED.iter().copied().filter(|name| !features.contains(name)).collect();
                let compatibility = if state.older { "held" } else if missing.is_empty() { "open" } else { "behind" };
                answer(200, json!({ "kind": if state.elsewhere { "linux" } else { "mac" }, "name": state.name(), "app": if state.older { "0.9.0" } else { "1.0.0" }, "engine": "2.3.0", "features": features, "compatibility": { "state": compatibility, "missing": missing }, "agents": true, "started": 1.0, "chats": format!("{HOME}/.kuvo-code/chats") }))
            }
            // Update Now on itself (`computers.update.here`): the release waiting for it is being
            // taken.
            ("POST", ["kuvo", "update"]) if body["now"] == json!(true) => match state.areas.here_update.as_mut() {
                Some(update) => {
                    update.state = "updating";
                    answer(202, json!({ "updating": "checking" }))
                }
                None => answer(409, json!({ "message": "It has the newest Kuvo already." })),
            },
            ("POST", ["kuvo", "sessions", session, "delivery"]) if state.delivery_enabled => {
                let owner = query(path, "computer").map(decoded).unwrap_or_else(|| "here".into());
                let id = body["id"].as_str().unwrap();
                if !state.delivery_done.contains(id) && !state.delivery_waiting.iter().any(|(_, _, draft)| draft["id"] == body["id"]) {
                    state.delivery_waiting.push((owner.clone(), (*session).into(), body.clone()));
                }
                answer(202, json!({ "operation": id, "owner": owner, "status": "pending" }))
            }
            // Another computer's conversation, carried: its newest page as that computer answers
            // it, kept; while it's away, the copy kept, said to be its last.
            ("GET", ["kuvo", "peer-session", computer, session, "messages"]) => {
                let (computer, session) = ((*computer).to_owned(), (*session).to_owned());
                let peer = state.peers.get(&computer).cloned();
                let standing = state.setup_standing.get(&computer).copied();
                drop(state);
                let Some(peer) = peer else { return answer(404, json!({ "message": "There’s no such computer in this setup." })) };
                let (down, name) = { let there = peer.state(); (there.down, there.name()) };
                let page = path.split_once('?').map_or("", |(_, query)| query);
                if !down {
                    for prefix in ["", "kuvo/agents/"] {
                        let read = Transport::send(&peer, &Request::get(format!("{prefix}api/session/{session}/message?{page}")));
                        if let Ok(response) = read && response.is_success() {
                            if let Ok(page) = serde_json::from_slice::<Value>(&response.body) {
                                self.state().carried.insert((computer.clone(), session.clone()), page);
                            }
                            return Ok(response);
                        }
                    }
                }
                let kept = self.state().carried.get(&(computer.clone(), session.clone())).map(|page| page["data"].clone());
                let mut away = json!({ "computer": { "id": computer, "name": name } });
                match standing {
                    Some((_, true)) => away["updating"] = json!(true),
                    Some((since, false)) => away["lastSeen"] = json!(since),
                    None => {}
                }
                answer(200, json!({ "data": kept.unwrap_or_else(|| json!([])), "away": away }))
            }
            ("GET", ["kuvo", "data", "messages", id]) if state.delivery_enabled => {
                let status = if state.delivery_done.contains(*id) { "committed" } else { "pending" };
                answer(200, json!({ "operation": id, "owner": "key_devbox", "status": status }))
            }
            ("GET", ["kuvo", "sessions", session, "delivery"]) if state.delivery_enabled => {
                let computer = query(path, "computer").map(decoded).unwrap_or_else(|| "here".into());
                let items: Vec<_> = state.delivery_waiting.iter().filter(|(key, target, _)| key == &computer && target.as_str() == *session)
                    .map(|(_, _, draft)| json!({ "id": draft["id"], "words": draft["text"], "on": "box", "delivery": "queue", "held": false }))
                    .collect();
                let line = state.peers.get(&computer).and_then(|peer| {
                    let peer = peer.state();
                    (peer.down && !items.is_empty()).then(|| format!("{} isn’t answering · your messages will go when it’s back", peer.name()))
                });
                answer(200, json!({ "paused": false, "items": items, "deliveryLine": line }))
            }
            ("GET", ["kuvo", "setup", "sessions", "list", "events"]) if state.setup_lists => {
                drop(state);
                answer(200, self.merged_sessions())
            }
            ("GET", ["kuvo", "setup", "sessions", "events"]) if state.setup_lists => {
                drop(state);
                answer(200, self.setup_sessions())
            }
            ("GET", ["kuvo", "setup", "runs-on", "events"]) if state.setup_lists => {
                drop(state);
                answer(200, self.runs_on())
            }
            ("GET", ["kuvo", "sessions", "list"]) if state.sessions_down => answer(500, json!({ "message": "Sessions couldn’t be read." })),
            ("GET", ["kuvo", "sessions", "list"]) => answer(200, state.line(true)),
            ("GET", ["kuvo", "sessions", "page"]) => {
                let project = query(path, "project").map(decoded).unwrap_or_default();
                match query(path, "cursor").map(decoded).as_deref().and_then(shown_of) {
                    Some((shown, step)) => answer(200, state.page(&project, shown, step)),
                    None => answer(400, json!({ "message": "That cursor isn't one this box gave. Read the list again." })),
                }
            }
            ("GET", ["kuvo", "search"]) => {
                let Some(query) = query(path, "q").map(decoded).map(|q| q.replace('+', " ")).filter(|q| !q.trim().is_empty()) else {
                    return answer(400, json!({ "message": "Say what to search for: q=<words>." }));
                };
                let wanted = query.trim().to_lowercase();
                // The latest first, each with the words around where they were found.
                let mut results: Vec<(f64, Value)> = Vec::new();
                for (session, messages) in &state.messages {
                    let said = messages.iter().flat_map(said_in).find(|text| text.to_lowercase().contains(&wanted));
                    if let Some(text) = said {
                        let updated = state.sessions.get(session).and_then(|facts| facts["updated"].as_f64()).unwrap_or(0.0);
                        results.push((updated, json!({ "session": session, "snippet": snippet(&text, &wanted), "info": { "id": session } })));
                    }
                }
                results.sort_by(|a, b| b.0.total_cmp(&a.0));
                answer(200, json!({ "results": results.into_iter().map(|(_, result)| result).collect::<Vec<_>>() }))
            }
            ("GET", ["kuvo", "sessions", "models"]) if state.models_fail => answer(503, json!({ "message": "The engine isn't answering." })),
            ("GET", ["kuvo", "sessions", "models"]) => {
                let asked = query(path, "sessions").map(decoded).unwrap_or_default();
                let mut told = serde_json::Map::new();
                for session in asked.split(',') {
                    if let Some(said) = state.model_states.get(session) {
                        // (Null: left out, as the box leaves out one it can't say about yet.)
                        if !said.is_null() {
                            told.insert(session.into(), said.clone());
                        }
                    } else if let Some(model) = state.models.get(session) {
                        told.insert(session.into(), json!({ "model": model, "name": model_name(model), "runs": true }));
                    }
                }
                answer(200, json!({ "sessions": told }))
            }
            ("GET", ["kuvo", "models", "offered"]) => {
                let defaults = query(path, "for") == Some("defaults");
                if state.nothing_runs {
                    return answer(200, json!({ "models": [], "known": { "engine": true, "agents": true }, "starting": null, "scoped": defaults, "startChoice": { "label": "Choose a Model", "refusal": "Choose a model to send it." } }));
                }
                // Each with its side and its efforts, as the box says them.
                let with = |side: &'static str, (mut model, efforts): (Value, Vec<&str>)| -> Value {
                    model["side"] = json!(side);
                    model["efforts"] = json!(efforts);
                    if model.get("aliases").is_none() {
                        model["aliases"] = json!([]);
                    }
                    model
                };
                // (A new computer: only the models that need no account run, and a new session
                // starts on one of them.)
                if state.first_run.only_free {
                    let listed: Vec<Value> = first_run::free_models().into_iter().map(|model| with("engine", model)).collect();
                    return answer(200, json!({ "models": listed, "known": { "engine": true, "agents": true }, "starting": first_run::free_start(), "scoped": defaults }));
                }
                let mut listed: Vec<Value> = models().into_iter().map(|model| with("engine", model)).collect();
                if state.first_run.free {
                    listed.extend(first_run::free_models().into_iter().map(|model| with("engine", model)));
                }
                listed.extend(more_models(state.more_models).into_iter().map(|model| with("engine", model)));
                if state.long_name {
                    listed.push(with("engine", (json!({ "providerID": "openrouter", "id": "long-preview", "name": "DeepSeek R2 Distill Qwen 72B Instruct Preview" }), vec!["low", "medium", "high", "xhigh"])));
                }
                if !state.agents_unknown && state.claude_accounts.is_empty() {
                    listed.extend(agents_models().into_iter().map(|model| with("agents", model)));
                }
                if state.codex && !state.agents_unknown {
                    listed.extend(codex_models().into_iter().map(|model| with("agents", model)));
                }
                let accounts = claude_accounts(&state.claude_accounts);
                if !state.agents_unknown {
                    listed.extend(accounts.iter().flat_map(|(provider, _)| CLAUDE_MODELS.iter().map(move |(id, name)| (json!({ "providerID": provider, "id": id, "name": name }), vec!["low", "medium", "high"]))).map(|model| with("agents", model)));
                }
                let mut said = json!({ "models": listed, "known": { "engine": true, "agents": !state.agents_unknown }, "starting": { "providerID": "anthropic", "id": "claude-opus-5-5", "variant": "high" } });
                if query(path, "session").map(decoded).is_some_and(|id| state.bots.bots.iter().any(|bot| bot["session"].as_str() == Some(id.as_str()))) {
                    said["scoped"] = json!(true);
                    said["models"].as_array_mut().unwrap().retain(|model| model["side"] == "engine");
                    said["note"] = json!("To use a coding agent in this bot, change its model in Edit Bot.");
                }
                if defaults {
                    said["scoped"] = json!(true);
                    said["models"].as_array_mut().unwrap().retain(|model| model["side"] == "engine" && model["hidden"] != true && model["retiring"] != true);
                }
                if !state.no_stars {
                    said["starred"] = json!(state.starred);
                    let mut providers: Vec<Value> = PROVIDERS.iter().filter(|(id, _)| accounts.is_empty() || *id != "claude-code").map(|(id, name)| json!({ "id": id, "name": name })).collect();
                    providers.extend(accounts.into_iter().map(|(_, provider)| provider));
                    if state.codex {
                        let mut codex = json!({ "id": "codex-cli", "name": "Codex" });
                        if let Some(speed) = state.speeds.get("codex-cli").filter(|speed| !speed.is_null()) {
                            codex["speed"] = speed.clone();
                        }
                        providers.push(codex);
                    }
                    said["providers"] = json!(providers);
                }
                answer(200, said)
            }
            ("POST", ["kuvo", "models", "speed"]) => {
                let body = request.body.clone().unwrap_or(Value::Null);
                let (Some(provider), Some(speed)) = (body["providerID"].as_str(), body.get("speed")) else {
                    return answer(400, json!({ "message": "Say which provider (providerID) and its speed (speed: a speed's id, or null for Standard)." }));
                };
                state.speeds.insert(provider.to_owned(), speed.clone());
                answer(200, json!({ "providerID": provider, "speed": speed }))
            }
            ("POST", ["kuvo", "models", "starred"]) if !state.no_stars => {
                let body = request.body.clone().unwrap_or_default();
                let (Some(provider), Some(id), Some(starred)) = (body["providerID"].as_str(), body["id"].as_str(), body["starred"].as_bool()) else {
                    return answer(400, json!({ "message": "Say which model (providerID, id) and whether it's starred (starred, true or false)." }));
                };
                let model = json!({ "providerID": provider, "id": id });
                let had = state.starred.contains(&model);
                if starred && !had {
                    state.starred.push(model);
                } else if !starred {
                    state.starred.retain(|each| *each != model);
                }
                answer(200, json!({ "starred": state.starred }))
            }
            ("POST", ["kuvo", "messages", "parts"]) => {
                let texts = request.body.as_ref().and_then(|body| body["texts"].as_array().cloned()).unwrap_or_default();
                answer(200, json!({ "parts": texts.iter().map(|text| json!({ "words": text })).collect::<Vec<_>>() }))
            }
            ("POST", ["kuvo", "sessions"]) => {
                let body = request.body.clone().unwrap_or(Value::Null);
                let id = state.id("ses_new");
                let now = now_ms();
                let (kind, mut folder, project) = match body["project"].as_str() {
                    Some(project) => ("project", project.to_owned(), Some(project.to_owned())),
                    None => ("chat", format!("{HOME}/.kuvo-code/chats"), None),
                };
                // (A worktree of its own, as the box makes one, on a branch of its own.)
                let worktree = body["worktree"] == json!(true) && project.is_some();
                if worktree {
                    folder = format!("{HOME}/.kuvo-code/worktrees/{}", id.trim_start_matches("ses_"));
                }
                if let Some(approval) = body["approval"].as_str() {
                    if approval == "fullAccess" && state.settings.approval["fullAccess"] != json!(true) {
                        return answer(409, json!({ "message": "Full Access isn’t on for this computer.", "reason": "fullAccessOff" }));
                    }
                    state.work_bars.approvals.insert(id.clone(), approval.to_owned());
                }
                state.starts.push(json!({ "session": id, "worktree": worktree, "approval": body["approval"] }));
                let mut facts = json!({ "kind": kind, "directory": folder, "created": now, "updated": now });
                if let Some(project) = &project {
                    facts["project"] = json!(project);
                }
                state.sessions.insert(id.clone(), facts);
                let model = body.get("model").filter(|model| model.is_object()).cloned().unwrap_or(json!({ "providerID": "anthropic", "id": "claude-opus-5-5", "variant": "high" }));
                state.models.insert(id.clone(), model);
                state.messages.insert(id.clone(), Vec::new());
                state.list_changed();
                answer(200, json!({ "session": id, "title": null, "folder": folder, "project": project }))
            }
            ("POST", ["kuvo", "sessions", session, "stop"]) => {
                let session = (*session).to_owned();
                state.running.remove(&session);
                state.pending_compacts.remove(&session);
                if let Some(facts) = state.sessions.get_mut(&session) {
                    facts["running"] = json!(false);
                }
                state.list_changed();
                state.emit("session.execution.interrupted", json!({ "sessionID": session }));
                answer(200, json!({}))
            }
            ("GET", ["kuvo", "sessions", session, "actions"]) => {
                let (status, value) = state.session_actions_response(session);
                answer(status, value)
            }
            ("POST", ["kuvo", "sessions", session, "pin"]) => {
                let session = (*session).to_owned();
                if let Some(held) = state.held_sessions.get(&session).cloned() {
                    return answer(409, json!({ "message": held }));
                }
                let Some(facts) = state.sessions.get_mut(&session) else {
                    return answer(404, json!({ "message": "There’s no such session." }));
                };
                if State::is_subagent(facts) {
                    return answer(409, json!({ "message": "Subagent sessions change with their parent session." }));
                }
                let pinned = body.get("pinned").and_then(Value::as_bool).unwrap_or(false);
                if pinned {
                    facts["pinned"] = json!(true);
                } else if let Some(obj) = facts.as_object_mut() {
                    obj.remove("pinned");
                }
                let info = facts.clone();
                state.list_changed();
                answer(200, json!({ "session": session, "info": info }))
            }
            ("POST", ["kuvo", "sessions", session, "archive"]) => {
                let session = (*session).to_owned();
                if let Some(held) = state.held_sessions.get(&session).cloned() {
                    return answer(409, json!({ "message": held }));
                }
                let archived = body.get("archived").and_then(Value::as_bool).unwrap_or(false);
                if !archived && state.engine_archived.contains(&session) {
                    return answer(409, json!({ "message": "This session was archived by the engine and can’t be unarchived here." }));
                }
                let info = {
                    let Some(facts) = state.sessions.get_mut(&session) else {
                        return answer(404, json!({ "message": "There’s no such session." }));
                    };
                    if State::is_subagent(facts) {
                        return answer(409, json!({ "message": "Subagent sessions change with their parent session." }));
                    }
                    if archived {
                        facts["archived"] = json!(true);
                        if let Some(obj) = facts.as_object_mut() {
                            obj.remove("asked");
                        }
                    } else if let Some(obj) = facts.as_object_mut() {
                        obj.remove("archived");
                    }
                    facts.clone()
                };
                if archived {
                    state.asks.remove(&session);
                    state.told(&session, Followed::Asks);
                }
                state.list_changed();
                answer(200, json!({ "session": session, "info": info }))
            }
            ("POST", ["kuvo", "sessions", session, "rename"]) => {
                let session = (*session).to_owned();
                if let Some(held) = state.held_sessions.get(&session).cloned() {
                    return answer(409, json!({ "message": held }));
                }
                let title = body.get("title").and_then(Value::as_str).unwrap_or("").trim().to_owned();
                if title.is_empty() {
                    return answer(400, json!({ "message": "Give the session a name." }));
                }
                let Some(facts) = state.sessions.get_mut(&session) else {
                    return answer(404, json!({ "message": "There’s no such session." }));
                };
                if State::is_subagent(facts) {
                    return answer(409, json!({ "message": "Subagent sessions change with their parent session." }));
                }
                facts["title"] = json!(title);
                let info = facts.clone();
                state.list_changed();
                answer(200, json!({ "session": session, "info": info }))
            }
            ("DELETE", ["kuvo", "sessions", session]) => {
                let session = (*session).to_owned();
                if let Some(held) = state.held_sessions.get(&session).cloned() {
                    return answer(409, json!({ "message": held }));
                }
                if let Some(facts) = state.sessions.get(&session) {
                    if State::is_subagent(facts) {
                        return answer(409, json!({ "message": "Subagent sessions change with their parent session." }));
                    }
                    let directory = facts["directory"].as_str().map(str::to_owned);
                    state.sessions.remove(&session);
                    state.models.remove(&session);
                    state.messages.remove(&session);
                    state.running.remove(&session);
                    state.pending_compacts.remove(&session);
                    state.asks.remove(&session);
                    state.queues.remove(&session);
                    state.engine_archived.remove(&session);
                    if let Some(dir) = directory {
                        let any_left = state.sessions.values().any(|other| State::is_conversation(other) && other["directory"] == json!(dir));
                        if !any_left && let Some(git) = state.git.as_mut()
                            && git.repos.get(&dir).is_some_and(|repo| repo.into.is_some())
                        {
                            git.repos.remove(&dir);
                        }
                    }
                    state.list_changed();
                }
                answer(200, json!({ "session": session }))
            }
            ("POST", ["kuvo", "sessions", "deletes"]) => {
                let ids: Vec<String> = body
                    .get("sessions")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
                if ids.is_empty() {
                    return answer(400, json!({ "message": "Choose at least one session to delete." }));
                }
                let present: Vec<&str> = ids
                    .iter()
                    .map(String::as_str)
                    .filter(|id| state.sessions.get(*id).is_some_and(State::is_conversation))
                    .collect();
                if present.is_empty() {
                    return answer(404, json!({ "message": "None of those sessions are here anymore." }));
                }
                if present.len() == 1 {
                    let id = present[0];
                    return answer(200, state.delete_confirm_for(id, &state.sessions[id]));
                }
                let worktree = state.worktree_delete_sentence(&present);
                let mut parts = vec!["Their messages and undo snapshots go too.".to_owned()];
                if worktree != "This can’t be undone." {
                    parts.push(worktree.to_owned());
                }
                parts.push("This can’t be undone.".to_owned());
                answer(
                    200,
                    json!({
                        "title": format!("Delete {} sessions?", ids.len()),
                        "message": parts.join(" "),
                        "label": "Delete"
                    }),
                )
            }
            ("GET", ["kuvo", "sessions", session, "mentions"]) if state.sessions.contains_key(*session) => {
                let wanted = query(path, "q").map(decoded).unwrap_or_default().to_lowercase();
                let mentions: Vec<Value> = match query(path, "for") {
                    Some("files") => FILES
                        .iter()
                        .filter(|file| file.to_lowercase().contains(&wanted))
                        .take(8)
                        .map(|file| {
                            let (folder, name) = file.rsplit_once('/').unwrap_or(("", file));
                            json!({ "text": format!("@{file}"), "title": name, "detail": if folder.is_empty() { Value::Null } else { json!(folder) }, "kind": "file", "session": null })
                        })
                        .collect(),
                    Some("agents") => named()
                        .into_iter()
                        .filter(|(name, _, id, _)| *id != *session && name.to_lowercase().contains(&wanted))
                        .map(|(name, kind, id, detail)| json!({ "text": format!("&{name}"), "title": name, "detail": detail, "kind": kind, "session": id }))
                        .collect(),
                    _ => return answer(400, json!({ "message": "Say which: for=files or for=agents." })),
                };
                answer(200, json!({ "mentions": mentions }))
            }
            ("POST", ["kuvo", "sessions", session, "queue", "resume"]) => {
                state.paused.remove(*session);
                state.told(*session, Followed::Queue);
                let bot = state.bots.bots.iter().find(|bot| bot["session"] == json!(session)).and_then(|bot| bot["id"].as_str()).map(str::to_owned);
                if let Some(bot) = bot {
                    for group in state.groups.resumed(&bot) {
                        state.group_changed(&group);
                    }
                }
                answer(200, json!({}))
            }
            ("GET", ["kuvo", "sessions", session, "queue"]) if state.sessions.contains_key(*session) => {
                answer(200, json!({ "paused": state.paused.contains(*session), "held": [], "items": state.queues.get(*session).cloned().unwrap_or_default(), "line": state.queue_lines.get(*session) }))
            }
            // Steer: for the agent's next step (one on the box, or with nothing running, goes now).
            // Send Now: to the agent at once, setting a stopped queue going.
            ("POST", ["kuvo", "sessions", session, "queue", "reorder"]) => {
                let Some(queue) = state.queues.get_mut(*session) else { return answer(404, json!({})) };
                let (item, anchor) = (&body["item"], &body["anchor"]);
                let (Some(from), Some(mut at)) = (queue.iter().position(|row| row["id"] == *item), queue.iter().position(|row| row["id"] == *anchor)) else {
                    return answer(409, json!({ "message": "Those messages can’t be moved now." }));
                };
                let row = queue.remove(from);
                if from < at { at -= 1; }
                queue.insert(at + usize::from(body["before"] == false), row);
                state.told(session, Followed::Queue);
                answer(200, json!({ "paused": state.paused.contains(*session), "items": state.queues.get(*session) }))
            }
            ("POST", ["kuvo", "sessions", session, "queue", item, how @ ("steer" | "now")]) => {
                let session = (*session).to_owned();
                let running = state.running.contains(&session);
                let Some(queue) = state.queues.get_mut(&session) else { return answer(404, json!({ "message": "That message isn't waiting any more." })) };
                let Some(at) = queue.iter().position(|each| each["id"] == json!(item)) else { return answer(404, json!({ "message": "That message isn't waiting any more." })) };
                let goes_now = *how == "now" || !running || queue[at]["on"] == json!("box");
                if goes_now {
                    let gone = queue.remove(at);
                    let now = now_ms();
                    let message = user(gone["id"].as_str().unwrap_or_default(), now, gone["words"].as_str().unwrap_or_default(), gone["keyword"].as_str());
                    state.messages.entry(session.clone()).or_default().push(message);
                } else {
                    queue[at]["delivery"] = json!("steer");
                }
                state.queue_asked.push(format!("{how} {item}"));
                // (Steer lets only that one go: a stopped queue stays stopped.)
                if goes_now {
                    state.paused.remove(&session);
                }
                state.told(&session, Followed::Queue);
                answer(200, json!({ "paused": false, "held": [] }))
            }
            ("DELETE", ["kuvo", "sessions", session, "queue", item]) => {
                let Some(queue) = state.queues.get_mut(*session) else { return answer(404, json!({})) };
                let Some(at) = queue.iter().position(|each| each["id"] == json!(item)) else { return answer(404, json!({})) };
                let removed = queue.remove(at);
                let session = (*session).to_owned();
                // Each as it was sent (a picture with its bytes as it is); a fixture's from Downloads.
                let files: Vec<Value> = state.queued_files.remove(*item).unwrap_or_else(|| removed["files"].as_array().into_iter().flatten().filter_map(Value::as_str).map(|name| json!({ "uri": format!("file://{HOME}/Downloads/{name}"), "name": name })).collect());
                let words = removed["words"].clone();
                state.told(&session, Followed::Queue);
                answer(200, json!({ "paused": false, "held": [], "removed": { "id": item, "text": words, "words": words, "keyword": removed["keyword"], "draft": removed.get("draft").cloned().unwrap_or(words), "files": files, "attached": files, "delivery": removed["delivery"] } }))
            }
            ("GET", ["kuvo", "sessions", session, "asks"]) if state.sessions.contains_key(*session) => answer(200, json!({ "asks": state.asks.get(*session).cloned().unwrap_or_default() })),
            ("POST", ["kuvo", "sessions", session, "asks", ask]) => {
                let Some(asks) = state.asks.get_mut(*session) else { return answer(404, json!({})) };
                let Some(at) = asks.iter().position(|each| each["id"] == json!(ask)) else { return answer(404, json!({})) };
                asks.remove(at);
                let asking = (*session).to_owned();
                if asks.is_empty()
                    && let Some(facts) = state.sessions.get_mut(*session)
                {
                    facts.as_object_mut().map(|facts| facts.remove("asked"));
                }
                state.list_changed();
                state.told(&asking, Followed::Asks);
                answer(200, json!({}))
            }
            ("POST", ["api", "session", session, "shell"]) | ("POST", ["kuvo", "agents", "api", "session", session, "shell"]) => {
                let Some(facts) = state.sessions.get(*session) else {
                    return answer(404, json!({ "message": "There's no such session." }));
                };
                if State::is_subagent(facts) {
                    return answer(400, json!({ "message": "This subagent works for the conversation that started it: run it there." }));
                }
                if state.sends_fail {
                    return answer(503, json!({ "message": "The engine isn't answering. Try again in a moment." }));
                }
                let command = body["command"].as_str().unwrap_or("").trim();
                if command.is_empty() {
                    return answer(400, json!({ "message": "Type a command after the !." }));
                }
                let shell = state.id("shl");
                let id = state.id("msg");
                let now = now_ms();
                state.emit("session.shell.started", json!({ "sessionID": session, "shell": { "id": shell, "command": command, "status": "running" } }));
                // A fixture, never executed: the stand-in touches no process or filesystem.
                let output = json!({ "output": "src/\nCargo.toml\nREADME.md\n", "truncated": false });
                state.messages.entry((*session).to_owned()).or_default().push(json!({
                    "type": "shell", "id": id, "time": { "created": now, "completed": now },
                    "shellID": shell, "command": command, "status": "completed", "exit": 0, "output": output
                }));
                state.emit("session.shell.ended", json!({ "sessionID": session, "shell": { "id": shell, "command": command, "status": "completed", "exit": 0 }, "output": output }));
                if let Some(facts) = state.sessions.get_mut(*session) {
                    facts["updated"] = json!(now);
                }
                state.list_changed();
                answer(200, json!({}))
            }
            ("POST", ["kuvo", "sessions", session, "send"]) => {
                let session = (*session).to_owned();
                if !state.sessions.contains_key(&session) {
                    return answer(404, json!({ "message": "There's no such session." }));
                }
                if state.sends_fail {
                    return answer(503, json!({ "message": "The engine isn't answering. Try again in a moment." }));
                }
                let body = request.body.clone().unwrap_or(Value::Null);
                let words = body["words"].as_str().unwrap_or("").to_owned();
                let id = body["id"].as_str().map(str::to_owned).unwrap_or_else(|| state.id("msg"));
                // For after the run, while one runs (or a Stop holds the queue): it waits in the queue.
                if body["delivery"] == json!("queue") && body["command"].is_null() && (state.running.contains(&session) || state.paused.contains(&session)) {
                    let on = if state.paused.contains(&session) { "box" } else { "engine" };
                    let files: Vec<Value> = body["files"].as_array().into_iter().flatten().filter_map(|file| file["name"].as_str().map(|name| json!(name))).collect();
                    if let Some(sent) = body["files"].as_array().filter(|sent| !sent.is_empty()) {
                        state.queued_files.insert(id.clone(), sent.clone());
                    }
                    state.queues.entry(session.clone()).or_default().push(json!({ "id": id, "words": words, "files": files, "keyword": body["keyword"], "from": null, "on": on, "delivery": "queue", "held": false }));
                    state.told(&session, Followed::Queue);
                    return answer(200, if on == "box" { json!({ "waits": true }) } else { json!({}) });
                }
                let now = now_ms();
                let message = match body["command"].as_str() {
                    // A command's message is the prompt it sent, with no parts kept.
                    Some(command) => json!({ "type": "user", "id": id, "time": { "created": now }, "text": format!("Run the project's {command} command. {words}").trim().to_owned() }),
                    None => {
                        let keyword = body["keyword"].as_str();
                        let mut message = user(&id, now, &words, keyword);
                        // The files it brought, as the box keeps them with its parts.
                        let paths: Vec<String> = body["files"].as_array().into_iter().flatten().filter_map(|file| file["uri"].as_str()).filter(|uri| uri.starts_with("file://")).map(|uri| decoded(uri.trim_start_matches("file://"))).collect();
                        // Those that came with their bytes, kept with it (the box keeps them on its disk).
                        let carried: Vec<Value> = body["files"].as_array().into_iter().flatten().filter_map(|file| {
                            let uri = file["uri"].as_str().filter(|uri| uri.starts_with("data:"))?;
                            let mime = uri.trim_start_matches("data:").split([';', ',']).next().unwrap_or("");
                            Some(json!({ "mime": mime, "name": file["name"], "source": { "type": "uri", "uri": uri } }))
                        }).collect();
                        if !paths.is_empty() && !state.drops_files {
                            message["metadata"]["kuvo"]["parts"]["paths"] = json!(paths);
                        }
                        if !carried.is_empty() && !state.drops_files {
                            message["files"] = json!(carried);
                        }
                        message
                    }
                };
                state.messages.entry(session.clone()).or_default().push(message);
                if let Some(facts) = state.sessions.get_mut(&session) {
                    facts["updated"] = json!(now);
                    if facts["title"].is_null() {
                        facts["title"] = json!(title_from(&words));
                    }
                }
                state.list_changed();
                state.emit("session.inbox.delivered", json!({ "sessionID": session }));
                drop(state);
                if self.inner.replies == Replies::Live {
                    self.reply(session, words);
                }
                answer(200, json!({}))
            }
            ("POST", ["api", "session", session, "compact"]) | ("POST", ["kuvo", "agents", "api", "session", session, "compact"]) => {
                let Some(facts) = state.sessions.get(*session) else {
                    return answer(404, json!({ "message": "There's no such session." }));
                };
                if State::is_subagent(facts) {
                    return answer(400, json!({ "message": "A subagent's conversation can't be summarized: summarize the conversation that started it." }));
                }
                if state.sends_fail {
                    return answer(503, json!({ "message": "The engine isn't answering. Try again in a moment." }));
                }
                if state.running.contains(*session) {
                    if state.pending_compacts.insert((*session).to_owned()) {
                        let inbox_id = state.id("msg");
                        state.emit("session.inbox.enqueued", json!({ "sessionID": session, "inboxID": inbox_id, "item": { "type": "compaction", "payload": {}, "delivery": "steer" } }));
                    }
                    return answer(200, json!({ "data": { "state": "waiting" } }));
                }
                if state.messages.get(*session).is_none_or(Vec::is_empty) {
                    return answer(400, json!({ "message": "There’s nothing to summarize yet." }));
                }
                let id = state.id("msg");
                let now = now_ms();
                state.emit("session.compaction.started", json!({ "sessionID": session, "reason": "manual" }));
                state.messages.entry((*session).to_owned()).or_default().push(json!({
                    "type": "compaction", "id": id, "time": { "created": now, "completed": now },
                    "status": "completed", "title": "Conversation summarized", "reason": "manual", "summary": "Kept the decisions and current work for the next turn."
                }));
                state.emit("session.compaction.ended", json!({ "sessionID": session }));
                if let Some(facts) = state.sessions.get_mut(*session) {
                    facts["updated"] = json!(now);
                }
                state.list_changed();
                answer(200, json!({ "data": { "state": "compacting" } }))
            }
            ("POST", ["api", "session", session, "model"]) | ("POST", ["kuvo", "agents", "api", "session", session, "model"]) => {
                let session = (*session).to_owned();
                let model = request.body.as_ref().map(|body| body["model"].clone()).unwrap_or(Value::Null);
                state.models.insert(session.clone(), model);
                state.emit("session.model.selected", json!({ "sessionID": session }));
                answer(200, json!({}))
            }
            ("GET", ["kuvo", "sessions", session, "location"]) => {
                if state.sessions.contains_key(*session) {
                    return answer(200, json!({ "place": "engine", "running": state.running.contains(*session) }));
                }
                // Another computer's, as the setup's lists say it: on that computer, as it stands.
                let peers: Vec<(String, StandIn)> = state.peers.iter().map(|(key, peer)| (key.clone(), peer.clone())).collect();
                let standing = state.setup_standing.clone();
                drop(state);
                for (key, peer) in peers {
                    let (has, name) = { let there = peer.state(); (there.sessions.contains_key(*session), there.name()) };
                    if !has { continue; }
                    let mut on = json!({ "id": key, "name": name, "away": false, "actions": ["open", "manage"] });
                    match standing.get(&key) {
                        Some((_, true)) => on["updating"] = json!(true),
                        Some((since, false)) => {
                            on["away"] = json!(true);
                            on["lastSeen"] = json!(since);
                            on["actions"] = json!([]);
                        }
                        None => {}
                    }
                    return answer(200, json!({ "on": on }));
                }
                answer(404, json!({ "message": "That conversation isn’t there any more." }))
            }
            // Where another computer's runs, asked of it through this box.
            ("GET", ["kuvo", "peer-session", computer, session, "location"]) => {
                let peer = state.peers.get(*computer).cloned();
                let session = (*session).to_owned();
                drop(state);
                match peer {
                    Some(peer) if !peer.state().down => Transport::send(&peer, &Request::get(format!("kuvo/sessions/{session}/location"))),
                    Some(_) => answer(503, json!({ "message": "The conversation’s computer isn’t answering." })),
                    None => answer(404, json!({ "message": "There’s no such computer in this setup." })),
                }
            }
            ("GET", ["api", "session", "active"]) => {
                let active: serde_json::Map<String, Value> = state.running.iter().map(|session| (session.clone(), json!({}))).collect();
                answer(200, json!({ "data": active }))
            }
            ("GET", ["api", "session", session]) if state.sessions.contains_key(*session) => {
                let mut data = json!({ "id": session });
                if let Some(context) = state.contexts.get(*session) { data["context"] = context.clone(); }
                if let Some(mut summarize) = state.summarize.get(*session).cloned() {
                    // (Its choice in use is the setting's, as a box reads it.)
                    if let Some(value) = state.settings.row_value("conversations", "compaction") { summarize["value"] = value; }
                    data["summarize"] = summarize;
                }
                match state.contexts.contains_key(*session) || state.summarize.contains_key(*session) {
                    true => answer(200, json!({ "data": data })),
                    false => answer(200, json!({ "id": session })),
                }
            }
            ("GET", ["api", "session", session, "message"]) if state.sessions.contains_key(*session) => {
                let all = state.messages.get(*session).cloned().unwrap_or_default();
                let limit: usize = query(path, "limit").and_then(|limit| limit.parse().ok()).unwrap_or(100);
                // Newest first; the cursor is how many of the newest are behind it.
                let skip: usize = query(path, "cursor").and_then(|cursor| cursor.parse().ok()).unwrap_or(0);
                let page: Vec<Value> = all.iter().rev().skip(skip).take(limit).cloned().collect();
                let next = (skip + page.len() < all.len()).then(|| (skip + page.len()).to_string());
                answer(200, json!({ "data": page, "cursor": { "next": next } }))
            }
            _ => answer(404, json!({})),
        }
    }

    /// An answer streamed back as an engine streams one: the run starts, a step reads a file, its
    /// words come a few at a time, the step and the run end.
    /// The bot talking in `group` answers a moment later, as a chat's bridge would have it.
    fn group_reply(&self, group: String) {
        let stand_in = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1500));
            let mut state = stand_in.state();
            let speaking = state.groups.line(&group).and_then(|line| serde_json::from_str::<Value>(&line).ok()).and_then(|chat| chat["speaking"].as_str().map(str::to_owned));
            if let Some(bot) = speaking {
                state.groups.say(&group, &bot, "Good question. Here’s how I see it: the first two are on track, and I’ll keep an eye on the third.", now_ms());
                state.group_changed(&group);
            }
        });
    }

    fn reply(&self, session: String, words: String) {
        let stand_in = self.clone();
        std::thread::spawn(move || {
            let pause = |ms: u64| std::thread::sleep(Duration::from_millis(ms));
            let step = stand_in.state().id("msg");
            let reply = format!(
                "On it. I read through the code this touches first.\n\nHere's what I'll do for “{}”:\n\n- Find where it's handled today\n- Make the change, with a test that shows it working\n- Run the tests before I say it's done",
                title_from(&words)
            );
            {
                let mut state = stand_in.state();
                state.running.insert(session.clone());
                if let Some(facts) = state.sessions.get_mut(&session) {
                    facts["running"] = json!(true);
                }
                state.list_changed();
                let now = now_ms();
                state.messages.entry(session.clone()).or_default().push(json!({ "type": "assistant", "id": step, "time": { "created": now }, "agent": "build", "content": [] }));
                state.emit("session.execution.started", json!({ "sessionID": session }));
                state.emit("session.step.started", json!({ "sessionID": session, "assistantMessageID": step, "started": now }));
            }
            pause(600);
            {
                let mut state = stand_in.state();
                if let Some(found) = state.messages.get_mut(&session).and_then(|messages| messages.iter_mut().rev().find(|message| message["id"] == json!(step)))
                    && let Some(content) = found["content"].as_array_mut()
                {
                    content.push(tool("call_live", "read", "completed", json!({ "filePath": "README.md" })));
                }
                state.emit("session.tool.called", json!({ "sessionID": session, "assistantMessageID": step }));
                state.emit("session.text.started", json!({ "sessionID": session, "assistantMessageID": step, "ordinal": 0 }));
            }
            for piece in reply.split_inclusive(' ') {
                pause(45);
                if !stand_in.state().running.contains(&session) {
                    break;
                }
                stand_in.emit("session.text.delta", json!({ "sessionID": session, "assistantMessageID": step, "ordinal": 0, "delta": piece }));
            }
            let mut state = stand_in.state();
            let now = now_ms();
            if let Some(found) = state.messages.get_mut(&session).and_then(|messages| messages.iter_mut().rev().find(|message| message["id"] == json!(step))) {
                if let Some(content) = found["content"].as_array_mut() {
                    content.push(json!({ "type": "text", "text": reply }));
                }
                found["time"]["completed"] = json!(now);
                found["finish"] = json!("stop");
            }
            state.emit("session.text.ended", json!({ "sessionID": session, "assistantMessageID": step, "ordinal": 0, "text": reply }));
            state.emit("session.step.ended", json!({ "sessionID": session, "assistantMessageID": step, "finish": "stop" }));
            let idle = state.id("msg");
            state.messages.entry(session.clone()).or_default().push(json!({ "type": "idle", "id": idle, "time": { "created": now }, "outcome": "succeeded" }));
            state.running.remove(&session);
            if state.pending_compacts.remove(&session) {
                let compact_id = state.id("msg");
                state.emit("session.compaction.started", json!({ "sessionID": session, "reason": "manual" }));
                state.messages.entry(session.clone()).or_default().push(json!({
                    "type": "compaction", "id": compact_id, "time": { "created": now, "completed": now },
                    "status": "completed", "title": "Conversation summarized", "reason": "manual", "summary": "Kept the decisions and current work for the next turn."
                }));
                state.emit("session.compaction.ended", json!({ "sessionID": session }));
                let compact_idle = state.id("msg");
                state.messages.entry(session.clone()).or_default().push(json!({ "type": "idle", "id": compact_idle, "time": { "created": now }, "outcome": "succeeded" }));
            }
            if let Some(facts) = state.sessions.get_mut(&session) {
                facts["running"] = json!(false);
                facts["updated"] = json!(now);
            }
            state.list_changed();
            state.emit("session.execution.succeeded", json!({ "sessionID": session }));
        });
    }
}

/// What a message says, as the box's search reads it: your words, the agent's text.
fn said_in(message: &Value) -> Vec<String> {
    let mut said: Vec<String> = message["text"].as_str().map(str::to_owned).into_iter().collect();
    for block in message["content"].as_array().into_iter().flatten() {
        if block["type"] == "text"
            && let Some(text) = block["text"].as_str()
        {
            said.push(text.to_owned());
        }
    }
    said
}

/// A line's worth of `text` around where `wanted` is.
fn snippet(text: &str, wanted: &str) -> String {
    let flat = text.replace(['*', '`'], "").split_whitespace().collect::<Vec<_>>().join(" ");
    let at = flat.to_lowercase().find(wanted).unwrap_or(0);
    let start = flat.get(..at).unwrap_or("").char_indices().rev().nth(40).map_or(0, |(ix, _)| ix);
    let rest: String = flat[start..].chars().take(110).collect();
    if start > 0 { format!("…{rest}") } else { rest }
}

/// A new session's title, from its first words (the box asks a model for one; the stand-in cuts).
fn title_from(words: &str) -> String {
    let line = words.lines().next().unwrap_or("").trim();
    let mut title: String = line.chars().take(60).collect();
    if line.chars().count() > 60 {
        title.push('…');
    }
    if title.is_empty() { "New session".into() } else { title }
}

/// A terminal's socket on the stand-in, as the app sends on it: what's typed is kept for a test to
/// read, and echoed back where it answers live (the app's `--stand-in`), as a shell would.
struct Shell {
    stand_in: StandIn,
    id: String,
}

impl SocketOut for Shell {
    fn send_text(&self, text: &str) -> Result<()> {
        let live = self.stand_in.inner.replies == Replies::Live;
        let mut state = self.stand_in.state();
        let Some(pty) = state.terminals.get(&self.id).filter(|pty| pty.socket.is_some()) else {
            return Err(kuvo_box::Error::Unreachable("The connection closed.".into()));
        };
        pty.typed.push(text.to_owned());
        if live {
            let echoed = text.replace('\r', "\r\n").replace('\u{7f}', "\u{8} \u{8}");
            pty.printed.extend_from_slice(echoed.as_bytes());
            if let Some(socket) = &pty.socket {
                let _ = socket.send_blocking(SocketMessage::Text(echoed.into_bytes()));
            }
        }
        Ok(())
    }

    fn close(&self) {
        let mut state = self.stand_in.state();
        if let Some(pty) = state.terminals.get(&self.id) {
            pty.closed = true;
            if let Some(socket) = pty.socket.take() {
                let _ = socket.send_blocking(SocketMessage::Ended(None));
            }
        }
    }
}

/// Its browser's socket, as the app sends on it: each command kept for a test to read, and those with
/// an id answered.
struct Viewer {
    stand_in: StandIn,
}

impl SocketOut for Viewer {
    fn send_text(&self, text: &str) -> Result<()> {
        let mut state = self.stand_in.state();
        if state.browser.socket.is_none() {
            return Err(kuvo_box::Error::Unreachable("The connection closed.".into()));
        }
        state.browser.heard(text);
        Ok(())
    }

    fn close(&self) {
        let mut state = self.stand_in.state();
        state.browser.closed = true;
        if let Some(socket) = state.browser.socket.take() {
            let _ = socket.send_blocking(SocketMessage::Ended(None));
        }
    }
}

impl Transport for StandIn {
    fn send(&self, request: &Request) -> Result<Response> {
        if self.state().down {
            return Err(kuvo_box::Error::Unreachable("ssh: connect to host 192.168.1.101 port 22: No route to host".into()));
        }
        if let Some(answer) = kuvo_box::line::by_routes::send(request) {
            return answer;
        }
        self.route(request)
    }

    /// It stands in for this computer's own box, unless a test says it's another's.
    fn here(&self) -> bool {
        !self.state().elsewhere
    }

    fn peer(&self, key: &str) -> Option<Arc<dyn Transport>> {
        self.state().peers.get(key).map(|peer| Arc::new(peer.clone()) as Arc<dyn Transport>)
    }

    /// A terminal's socket, as its engine opens one: what its shell printed so far first.
    fn socket(&self, request: &Request) -> Result<Socket> {
        let mut state = self.state();
        state.seen.push(Seen { method: request.method.clone(), path: request.path.clone(), body: None });
        if state.down {
            return Err(kuvo_box::Error::Unreachable("The computer is away.".into()));
        }
        if let Some(why) = state.terminals.not_here.clone() {
            return Err(kuvo_box::transport::sockets_not_here(why));
        }
        if request.route() == "kuvo/browser/view" {
            // A Mac's box has no live view; a machine's opens it on its keeper, where that runs.
            if !state.elsewhere {
                let said = json!({ "message": browser::NO_VIEW_ON_MAC }).to_string();
                return Err(kuvo_box::Error::Refused(kuvo_box::Refusal::from_answer(404, said.as_bytes())));
            }
            if !state.browser.running {
                let said = json!({ "message": format!("connect ENOENT {HOME}/.kuvo-code/run/browser.sock") }).to_string();
                return Err(kuvo_box::Error::Refused(kuvo_box::Refusal::from_answer(502, said.as_bytes())));
            }
            let incoming = state.browser.open();
            return Ok(Socket { incoming, outgoing: Arc::new(Viewer { stand_in: self.clone() }) });
        }
        let id = request.route().strip_prefix("api/pty/").and_then(|rest| rest.strip_suffix("/connect")).map(decoded).unwrap_or_default();
        let Some(pty) = state.terminals.get(&id).filter(|pty| pty.info["status"] == "running") else {
            let said = json!({ "message": "That terminal isn’t there any more." }).to_string();
            return Err(kuvo_box::Error::Refused(kuvo_box::Refusal::from_answer(404, said.as_bytes())));
        };
        let (sender, incoming) = async_channel::unbounded();
        if !pty.printed.is_empty() {
            let _ = sender.send_blocking(SocketMessage::Text(pty.printed.clone()));
        }
        // (The engine's note on where its output had got to.)
        let _ = sender.send_blocking(SocketMessage::Binary(format!("\0{{\"cursor\":{}}}", pty.printed.len()).into_bytes()));
        pty.socket = Some(sender);
        pty.closed = false;
        Ok(Socket { incoming, outgoing: Arc::new(Shell { stand_in: self.clone(), id }) })
    }

    fn open(&self, request: &Request, hold: &dyn Fn(Closer) -> bool) -> Result<Stream> {
        // The one line, carrying the routes below as a box's does.
        if self.state().down && request.route() == "kuvo/line" {
            return Err(kuvo_box::Error::Unreachable("The computer is away.".into()));
        }
        let me = self.clone();
        if let Some(line) = kuvo_box::line::by_routes::open(request, hold, Arc::new(move |request: &Request, hold: &dyn Fn(Closer) -> bool| me.open(request, hold))) {
            return line;
        }
        let route = request.route();
        // A conversation's asks or queue, followed: as it is now, then a line as it changes.
        let parts: Vec<&str> = route.split('/').collect();
        let output_session = (route == "kuvo/setup/outputs/events").then(|| query(&request.path, "session").map(decoded)).flatten();
        let followed = match parts.as_slice() {
            ["kuvo", "sessions", session, "asks", "events"] => Some((*session, Followed::Asks)),
            ["kuvo", "sessions", session, "queue", "events"] => Some((*session, Followed::Queue)),
            ["kuvo", "artifacts", "events"] => output_session.as_deref().map(|session| (session, Followed::Outputs)),
            ["kuvo", "wakes", session, "events"] => {
                if self.state().down {
                    return Err(kuvo_box::Error::Unreachable("The computer is away.".into()));
                }
                Some((*session, Followed::Wakes))
            }
            _ => None,
        };
        if let Some((session, what)) = followed {
            let (sender, receiver) = channel();
            let id = {
                let mut state = self.state();
                state.seen.push(Seen { method: request.method.clone(), path: request.path.clone(), body: None });
                let id = state.next;
                state.next += 1;
                let _ = sender.send(state.followed(session, what));
                state.followers.insert(id, (session.to_owned(), what, sender));
                id
            };
            let inner = self.inner.clone();
            let closer = Closer::new(move || {
                inner.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).followers.remove(&id);
            });
            if !hold(closer.clone()) {
                closer.close();
            }
            return Ok(Stream { status: 200, content_type: Some("application/x-ndjson".into()), reader: Box::new(Pieces { receiver, piece: Vec::new(), at: 0, sse: false }), closer });
        }
        // A folder's changed files, followed: as they are now, then a line as they change.
        if route == "kuvo/git/files/events" {
            let directory = query(&request.path, "directory").map(decoded).unwrap_or_default();
            let (sender, receiver) = channel();
            let id = {
                let mut state = self.state();
                state.seen.push(Seen { method: request.method.clone(), path: request.path.clone(), body: None });
                let id = state.next;
                state.next += 1;
                if let Some(git) = state.git.as_ref() {
                    let _ = sender.send(git.files(&directory).to_string());
                }
                state.git_followers.insert(id, (directory, sender));
                id
            };
            let inner = self.inner.clone();
            let closer = Closer::new(move || {
                inner.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).git_followers.remove(&id);
            });
            if !hold(closer.clone()) {
                closer.close();
            }
            return Ok(Stream { status: 200, content_type: Some("application/x-ndjson".into()), reader: Box::new(Pieces { receiver, piece: Vec::new(), at: 0, sse: false }), closer });
        }
        // The bots, followed: as they are now, then again as they change.
        if route == "kuvo/bots/events" {
            let (sender, receiver) = channel();
            let id = {
                let mut state = self.state();
                state.seen.push(Seen { method: request.method.clone(), path: request.path.clone(), body: None });
                if state.bots.down {
                    drop(state);
                    return self.open(&Request::get("kuvo/bots"), hold);
                }
                if let Some((200, list)) = state.bots.answer("GET", "kuvo/bots", &|_| None, &Value::Null, &[], 0.) {
                    let _ = sender.send(list.to_string());
                }
                let id = state.next;
                state.next += 1;
                state.bots_followers.insert(id, sender);
                id
            };
            let inner = self.inner.clone();
            let closer = Closer::new(move || {
                inner.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).bots_followers.remove(&id);
            });
            if !hold(closer.clone()) {
                closer.close();
            }
            return Ok(Stream { status: 200, content_type: Some("application/x-ndjson".into()), reader: Box::new(Pieces { receiver, piece: Vec::new(), at: 0, sse: false }), closer });
        }
        // The chats, followed: as they are now, then the list again as it changes.
        if route == "kuvo/groups/events" {
            let (sender, receiver) = channel();
            let id = {
                let mut state = self.state();
                state.seen.push(Seen { method: request.method.clone(), path: request.path.clone(), body: None });
                let _ = sender.send(state.groups_list().to_string());
                let id = state.next;
                state.next += 1;
                state.groups_followers.insert(id, sender);
                id
            };
            let inner = self.inner.clone();
            let closer = Closer::new(move || {
                inner.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).groups_followers.remove(&id);
            });
            if !hold(closer.clone()) {
                closer.close();
            }
            return Ok(Stream { status: 200, content_type: Some("application/x-ndjson".into()), reader: Box::new(Pieces { receiver, piece: Vec::new(), at: 0, sse: false }), closer });
        }
        // One group chat, followed: as it is now, then a line as it changes.
        if let ["kuvo", "groups", group, "events"] = parts.as_slice() {
            let (sender, receiver) = channel();
            let group = (*group).to_owned();
            let id = {
                let mut state = self.state();
                state.seen.push(Seen { method: request.method.clone(), path: request.path.clone(), body: None });
                let Some(line) = state.groups.line(&group) else {
                    return Ok(Stream { status: 404, content_type: Some("application/json".into()), reader: Box::new(std::io::Cursor::new(json!({ "message": "There’s no such group chat here." }).to_string().into_bytes())), closer: Closer::nothing() });
                };
                let _ = sender.send(line);
                let id = state.next;
                state.next += 1;
                state.group_followers.insert(id, (group, sender));
                id
            };
            let inner = self.inner.clone();
            let closer = Closer::new(move || {
                inner.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).group_followers.remove(&id);
            });
            if !hold(closer.clone()) {
                closer.close();
            }
            return Ok(Stream { status: 200, content_type: Some("application/x-ndjson".into()), reader: Box::new(Pieces { receiver, piece: Vec::new(), at: 0, sse: false }), closer });
        }
        // What waits on you, or your computers, followed: as it is now, then a line as it changes.
        if let Some(once) = route.strip_suffix("/events").filter(|once| matches!(*once, "kuvo/waiting" | "kuvo/setup/waiting/counts" | "kuvo/computers" | "kuvo/agents/api/kuvo/limits")) {
            let (sender, receiver) = channel();
            let id = {
                let mut state = self.state();
                state.seen.push(Seen { method: request.method.clone(), path: request.path.clone(), body: None });
                let id = state.next;
                state.next += 1;
                if once == "kuvo/agents/api/kuvo/limits" {
                    let _ = sender.send("{}".into());
                    state.usage_followers.insert(id, sender);
                } else {
                    if let Some((_, now)) = state.area("GET", once) {
                        let _ = sender.send(now.to_string());
                    }
                    if matches!(once, "kuvo/waiting" | "kuvo/setup/waiting/counts") { state.waits.insert(id, sender) } else { state.computers.insert(id, sender) };
                }
                id
            };
            let inner = self.inner.clone();
            let closer = Closer::new(move || {
                let mut state = inner.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                state.waits.remove(&id);
                state.computers.remove(&id);
                state.usage_followers.remove(&id);
            });
            if !hold(closer.clone()) {
                closer.close();
            }
            return Ok(Stream { status: 200, content_type: Some("application/x-ndjson".into()), reader: Box::new(Pieces { receiver, piece: Vec::new(), at: 0, sse: false }), closer });
        }
        if let ["kuvo", "work", "background", session, "events"] = route.split('/').collect::<Vec<_>>().as_slice() {
            let session = decoded(session);
            let (sender, receiver) = channel();
            let id = {
                let mut state = self.state();
                state.seen.push(Seen { method: request.method.clone(), path: request.path.clone(), body: None });
                let id = state.next;
                state.next += 1;
                let _ = sender.send(state.work_bars.of(&session).to_string());
                state.work_bars.session_followers.insert(id, (session, sender));
                id
            };
            let inner = self.inner.clone();
            let closer = Closer::new(move || {
                let mut state = inner.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                state.work_bars.session_followers.remove(&id);
            });
            if !hold(closer.clone()) {
                closer.close();
            }
            return Ok(Stream { status: 200, content_type: Some("application/x-ndjson".into()), reader: Box::new(Pieces { receiver, piece: Vec::new(), at: 0, sse: false }), closer });
        }
        // The Library, followed: as its query lists it now, then again as something's removed.
        if route == "kuvo/setup/library/events" {
            let (sender, receiver) = channel();
            let id = {
                let mut state = self.state();
                state.seen.push(Seen { method: request.method.clone(), path: request.path.clone(), body: None });
                let id = state.next;
                state.next += 1;
                let made = state.made.unwrap_or_else(now_ms);
                let _ = sender.send(state.library.now_line(&request.path, made));
                state.library.followers.insert(id, (request.path.clone(), sender));
                id
            };
            let inner = self.inner.clone();
            let closer = Closer::new(move || {
                inner.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).library.followers.remove(&id);
            });
            if !hold(closer.clone()) {
                closer.close();
            }
            return Ok(Stream { status: 200, content_type: Some("application/x-ndjson".into()), reader: Box::new(Pieces { receiver, piece: Vec::new(), at: 0, sse: false }), closer });
        }
        if matches!(route, "kuvo/work/background/events" | "kuvo/work/events") {
            let background = route == "kuvo/work/background/events";
            let (sender, receiver) = channel();
            let id = {
                let mut state = self.state();
                state.seen.push(Seen { method: request.method.clone(), path: request.path.clone(), body: None });
                let id = state.next;
                state.next += 1;
                let bars = &mut state.work_bars;
                if background {
                    let _ = sender.send(bars.counts().to_string());
                    bars.followers.insert(id, sender);
                } else {
                    let _ = sender.send(json!({ "changed": bars.work_changed }).to_string());
                    bars.work_followers.insert(id, sender);
                }
                id
            };
            let inner = self.inner.clone();
            let closer = Closer::new(move || {
                let mut state = inner.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                state.work_bars.followers.remove(&id);
                state.work_bars.work_followers.remove(&id);
            });
            if !hold(closer.clone()) {
                closer.close();
            }
            return Ok(Stream { status: 200, content_type: Some("application/x-ndjson".into()), reader: Box::new(Pieces { receiver, piece: Vec::new(), at: 0, sse: false }), closer });
        }
        let lists = route == "kuvo/sessions/list/events";
        if !lists && route != "api/event" {
            let response = self.route(request)?;
            return Ok(Stream { status: response.status, content_type: response.content_type, reader: Box::new(std::io::Cursor::new(response.body)), closer: Closer::nothing() });
        }
        let (sender, receiver) = channel();
        let id = {
            let mut state = self.state();
            state.seen.push(Seen { method: request.method.clone(), path: request.path.clone(), body: None });
            let id = state.next;
            state.next += 1;
            if lists {
                if state.sessions_down {
                    drop(state);
                    return self.open(&Request::get("kuvo/sessions/list"), hold);
                }
                // The list now, then a line for each change.
                let _ = sender.send(state.line(true).to_string());
                state.lists.insert(id, sender);
            } else {
                state.events.insert(id, sender);
            }
            id
        };
        let inner = self.inner.clone();
        let closer = Closer::new(move || {
            let mut state = inner.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            state.lists.remove(&id);
            state.events.remove(&id);
        });
        if !hold(closer.clone()) {
            closer.close();
        }
        let content_type = if lists { "application/x-ndjson" } else { "text/event-stream" };
        Ok(Stream { status: 200, content_type: Some(content_type.into()), reader: Box::new(Pieces { receiver, piece: Vec::new(), at: 0, sse: !lists }), closer })
    }

    fn streams(&self) -> bool {
        true
    }

    fn describe(&self) -> String {
        NAME.into()
    }
}

#[cfg(test)]
mod shell_tests {
    use super::*;

    #[test]
    fn shell_joins_history_without_waking_agent_and_emits_lifecycle() {
        let stand_in = StandIn::new(Replies::Never);
        let (sender, receiver) = channel();
        stand_in.state().events.insert(999, sender);
        let before = stand_in.state().messages.get("ses_dark").map_or(0, Vec::len);
        let response = stand_in.route(&Request::post("api/session/ses_dark/shell", json!({ "command": "  ls  " }))).unwrap();
        assert_eq!(response.status, 200);
        let state = stand_in.state();
        let messages = &state.messages["ses_dark"];
        assert_eq!(messages.len(), before + 1);
        let shell = messages.last().unwrap();
        assert_eq!(shell["type"], "shell");
        assert_eq!(shell["command"], "ls");
        assert_eq!(shell["status"], "completed");
        assert_eq!(shell["exit"], 0);
        assert!(!state.running.contains("ses_dark"));
        let started: Value = serde_json::from_str(&receiver.try_recv().unwrap()).unwrap();
        let ended: Value = serde_json::from_str(&receiver.try_recv().unwrap()).unwrap();
        assert_eq!(started["type"], "session.shell.started");
        assert_eq!(ended["type"], "session.shell.ended");
        assert_eq!(started["data"]["shell"]["id"], shell["shellID"]);
        assert_eq!(ended["data"]["output"], shell["output"]);
    }

    #[test]
    fn shell_refusals_do_not_add_messages() {
        let stand_in = StandIn::new(Replies::Never);
        let before = stand_in.state().messages.get("ses_dark").map_or(0, Vec::len);
        for (session, command, status) in [("missing", "ls", 404), ("ses_dark", "  ", 400), ("ses_sub", "ls", 400)] {
            assert_eq!(stand_in.route(&Request::post(format!("api/session/{session}/shell"), json!({ "command": command }))).unwrap().status, status);
        }
        assert_eq!(stand_in.state().messages.get("ses_dark").map_or(0, Vec::len), before);
    }

    #[test]
    fn compact_waits_mid_turn_and_completes_at_rest() {
        let stand_in = StandIn::new(Replies::Never);
        let (sender, receiver) = channel();
        stand_in.state().events.insert(999, sender);

        // Mid-turn (`ses_signup` is running): /compact queues once and returns state: "waiting" without appending a completed row.
        let before = stand_in.state().messages["ses_signup"].len();
        let first = stand_in.route(&Request::post("api/session/ses_signup/compact", json!({}))).unwrap();
        assert_eq!(first.status, 200);
        assert_eq!(serde_json::from_slice::<Value>(&first.body).unwrap(), json!({ "data": { "state": "waiting" } }));
        assert_eq!(stand_in.state().messages["ses_signup"].len(), before);
        assert!(stand_in.state().pending_compacts.contains("ses_signup"));
        let enqueued: Value = serde_json::from_str(&receiver.try_recv().unwrap()).unwrap();
        assert_eq!(enqueued["type"], "session.inbox.enqueued");
        assert_eq!(enqueued["data"]["item"]["type"], "compaction");

        // Asking twice while waiting stays a single queued compaction.
        let second = stand_in.route(&Request::post("kuvo/agents/api/session/ses_signup/compact", json!({}))).unwrap();
        assert_eq!(second.status, 200);
        assert_eq!(serde_json::from_slice::<Value>(&second.body).unwrap(), json!({ "data": { "state": "waiting" } }));
        assert!(receiver.try_recv().is_err());

        // Stopping the session drops the pending compaction.
        stand_in.route(&Request::post("kuvo/sessions/ses_signup/stop", json!({}))).unwrap();
        assert!(!stand_in.state().pending_compacts.contains("ses_signup"));
        let _interrupted = receiver.try_recv().unwrap();

        // At rest (`ses_recipes` is idle): /compact emits started/ended and appends a completed row.
        let before_idle = stand_in.state().messages["ses_recipes"].len();
        let at_rest = stand_in.route(&Request::post("api/session/ses_recipes/compact", json!({}))).unwrap();
        assert_eq!(at_rest.status, 200);
        assert_eq!(serde_json::from_slice::<Value>(&at_rest.body).unwrap(), json!({ "data": { "state": "compacting" } }));
        assert_eq!(stand_in.state().messages["ses_recipes"].len(), before_idle + 1);
        assert_eq!(stand_in.state().messages["ses_recipes"].last().unwrap()["type"], "compaction");
        assert_eq!(stand_in.state().messages["ses_recipes"].last().unwrap()["status"], "completed");
        let started: Value = serde_json::from_str(&receiver.try_recv().unwrap()).unwrap();
        let ended: Value = serde_json::from_str(&receiver.try_recv().unwrap()).unwrap();
        assert_eq!(started["type"], "session.compaction.started");
        assert_eq!(ended["type"], "session.compaction.ended");
    }

    #[test]
    fn compact_refuses_subagents_empty_sessions_and_down_engines() {
        let stand_in = StandIn::new(Replies::Never);
        assert_eq!(stand_in.route(&Request::post("api/session/missing/compact", json!({}))).unwrap().status, 404);
        assert_eq!(stand_in.route(&Request::post("api/session/ses_sub/compact", json!({}))).unwrap().status, 400);

        // Newly created empty session refuses /compact with 400.
        let started = stand_in.route(&Request::post("kuvo/sessions", json!({}))).unwrap();
        let new_id = serde_json::from_slice::<Value>(&started.body).unwrap()["session"].as_str().unwrap().to_owned();
        assert_eq!(stand_in.route(&Request::post(format!("api/session/{new_id}/compact"), json!({}))).unwrap().status, 400);

        stand_in.fail_sends();
        assert_eq!(stand_in.route(&Request::post("api/session/ses_recipes/compact", json!({}))).unwrap().status, 503);
    }
}
