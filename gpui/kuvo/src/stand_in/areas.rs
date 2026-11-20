//! What the stand-in box says of the rail's areas, in a box's own shapes (PROTOCOL.md): your
//! computers; the accounts' limits, each row with its rings and every window its level; and how
//! many things wait on you. As a box does, it counts, levels and picks the rings itself: the app
//! draws what it says.

use serde_json::{Value, json};

use super::{DAY, HOUR, MINUTE};

/// How the stand-in answers for the areas.
#[derive(Clone, Debug, Default)]
pub struct Areas {
    /// A first run: no bots, chats, other computers or accounts.
    pub empty: bool,
    pub usage: UsageSample,
    pub insights_partial: bool,
    pub usage_refresh_failed: bool,
    pub usage_refresh_delayed: bool,
    pub usage_refresh_pending: bool,
    pub usage_refresh_completed: bool,
    /// Its list of computers can't be read now (503).
    pub computers_down: bool,
    /// Its list of computers has this computer alone (the rest of the sample stays).
    pub computers_alone: bool,
    /// So many more machines listed (a long list).
    pub more_computers: usize,
    /// The computers removed (`DELETE kuvo/computers/<key>`), by key.
    pub removed: Vec<String>,
    /// The computers being reached again (`POST kuvo/computers/<key>/connect`), by key.
    pub reaching: Vec<String>,
    /// Why it adds no computers (`can: false`), in its words; none: it does.
    pub add_cannot: Option<String>,
    /// Its tries at adding, setting up again and uninstalling (`kuvo/computers/add`), newest first.
    pub tries: Vec<Value>,
    /// The machines it added over SSH, by the address each was added under.
    pub added: Vec<String>,
    /// How many tries it has begun (their ids).
    pub tried: usize,
    /// The computers whose app was removed from your devices: their boxes add none until allowed.
    pub vouch_blocked: Vec<String>,
    /// What renaming or removing says instead of doing it, as a box that can't ask the computer
    /// writing its record says (`503`).
    pub manage_refused: Option<String>,
    /// The old Kuvo Code app is open on the Mac that writes your machines' records: removing waits.
    pub old_app: bool,
    /// It isn't a Mac's own box, which Kuvo Helper runs and draws in the menu bar (`kuvo/box/at-work`'s `applies`).
    pub no_menu_bar: bool,
    /// The machines whose Kuvo it brings up to date (`computers.update`), by key: how that stands
    /// (`waiting`, `updating`, `failed`), whether it has waited long, and how many sessions are at
    /// work there (Update Now asks first while any are).
    pub updates: Vec<Updating>,
    /// The release its app carries, waiting for it (`computers.update.here`): its own row says how
    /// that stands, with Update Now.
    pub here_update: Option<Updating>,
    /// The machines it offers to set up again over SSH (Set Up Again) without bringing them up to
    /// date itself, by key.
    pub set_up_again: Vec<String>,
    /// The older machines it didn't set up over SSH, by key: it says to set each up over SSH from
    /// here (`add`).
    pub to_set_up: Vec<String>,
    /// The computers it lists as Macs (`kind: mac`), by key.
    pub as_macs: Vec<String>,
    /// Override `system` strings for computers, by key (`None` removes `system`).
    pub systems: Vec<(String, Option<String>)>,
    /// The older computers it can't bring up itself, by key, with why in its words (no action).
    pub cant_bring_up: Vec<(String, String)>,
    /// An empty box that lists your computers all the same (another computer's, in a test).
    pub lists_yours: bool,
}

/// Usage responses drawn by the screenshot harness.
#[derive(Clone, Copy, Debug, Default)]
pub enum UsageSample {
    #[default]
    Default,
    Many,
    One,
    UnmeasuredClaude,
    Failed,
    Stale,
}

/// A machine whose Kuvo the stand-in brings up to date (`computers.update`).
#[derive(Clone, Debug)]
pub struct Updating {
    pub key: String,
    pub state: &'static str,
    pub long: bool,
    pub working: usize,
}

/// What the stand-in says of bringing `name`'s Kuvo up to date, and Update Now where it offers it,
/// in the words a box that does it says.
fn update_said(updating: &Updating, name: &str) -> (Value, Option<Value>) {
    let at_work = match updating.working {
        0 => None,
        1 => Some("1 session is".to_owned()),
        count => Some(format!("{count} sessions are")),
    };
    // (What's true now, and the next step, as the box says them.)
    let (title, detail) = match (updating.state, updating.long) {
        ("updating", _) => (format!("Updating Kuvo on {name}…"), String::new()),
        ("failed", _) => (format!("Couldn’t reach {name}"), "Make sure it’s on and connected, then Try Again. Kuvo also tries again by itself later.".to_owned()),
        (_, true) => (format!("Kuvo on {name} hasn’t been free for a day"), "It updates once nothing’s at work there, or now with Update Now.".to_owned()),
        _ if updating.working == 0 => (format!("Kuvo on {name} updates in a moment"), String::new()),
        _ => (format!("Kuvo on {name} updates once nothing’s at work there"), "Or now with Update Now: what’s at work there keeps going.".to_owned()),
    };
    let mut update = json!({ "state": updating.state, "title": title, "detail": detail });
    if updating.long {
        update["long"] = json!(true);
    }
    let action = (updating.state != "updating").then(|| {
        let mut action = json!({ "id": "update", "label": if updating.state == "failed" { "Try Again" } else { "Update Now" } });
        if let Some(at_work) = at_work {
            action["confirm"] = json!({ "title": format!("Update Kuvo on {name} now?"), "message": format!("{at_work} at work on {name}. They keep going, but {name} may be out of reach for a moment as Kuvo there restarts."), "label": "Update Now" });
        }
        action
    });
    (update, action)
}

/// Its answer to `method route`, if it's one of the areas' routes. `needing`: how many of its
/// sessions "Needs you" lists.
pub fn answer(areas: &Areas, method: &str, route: &str, now: f64, needing: usize, bots_asking: usize) -> Option<(u16, Value)> {
    let refresh = (method, route) == ("POST", "kuvo/agents/api/kuvo/limits/refresh");
    if refresh && areas.usage_refresh_failed {
        return Some((503, json!({ "message": "Usage could not be refreshed." })));
    }
    if method != "GET" && !refresh {
        return None;
    }
    if route == "kuvo/computers" && areas.computers_down {
        return Some((503, json!({ "message": "Kuvo on this computer isn’t answering." })));
    }
    let answer = match route {
        "kuvo/computers" => json!({ "computers": listed(areas) }),
        "kuvo/agents/api/kuvo/limits" | "kuvo/agents/api/kuvo/limits/refresh" => {
            let rows = if areas.empty { Vec::new() } else { accounts(now, areas.usage) };
            let updated = rows.iter().filter_map(|row| row["usage"]["updatedAt"].as_f64()).max_by(f64::total_cmp);
            json!({ "data": { "accounts": [], "services": ["claude", "codex", "copilot", "grok", "openrouter"],
                "rows": rows, "usageUpdatedAt": updated, "refreshPending": areas.usage_refresh_pending,
                "refreshCompletedAt": areas.usage_refresh_completed.then_some(now) } })
        }
        "kuvo/waiting" => json!({ "sessions": needing, "bots": bots_asking, "queue": 0 }),
        "kuvo/box/at-work" => json!({ "applies": !areas.no_menu_bar, "working": 0, "sessions": [], "asking": [], "waiting": { "sessions": needing, "bots": bots_asking, "queue": 0 }, "background": 0, "busy": false, "rings": [] }),
        _ => return None,
    };
    Some((200, answer))
}

/// What a session waiting on you asks, as the box words it: its kind, and a few words written to
/// follow a name.
pub fn asks_of(session: &str) -> Value {
    let (kind, summary) = match session {
        "ses_limits" => ("permission", "asks to run a command"),
        "ses_cms" => ("question", "has a question for you"),
        _ => ("other", "waits on you"),
    };
    json!({ "kind": kind, "summary": summary })
}

/// What renaming, removing or reaching one of your computers again comes to, as a box answers it:
/// done, and the list changed (`true`); `None` for a route that isn't one of these.
pub fn manage(areas: &mut Areas, method: &str, route: &str) -> Option<((u16, Value), bool)> {
    let rest = route.strip_prefix("kuvo/computers/")?;
    if let Some(key) = rest.strip_suffix("/allow-devices") {
        if method != "POST" {
            return Some(((405, json!({ "message": "Only POST." })), false));
        }
        areas.vouch_blocked.retain(|blocked| blocked != key);
        return Some(((200, json!({})), true));
    }
    let (key, connect) = match rest.strip_suffix("/connect") {
        Some(key) => (key, true),
        None => (rest, false),
    };
    if key == "events" || key.contains('/') {
        return None;
    }
    let Some(computer) = listed(areas).into_iter().find(|computer| computer["key"] == json!(key)) else { return Some(((404, json!({ "message": "That isn't one of your computers." })), false)) };
    let offers = |id: &str| computer["actions"].as_array().is_some_and(|actions| actions.iter().any(|action| action["id"] == json!(id)));
    let wanted = match (method, connect) {
        ("POST", true) => "connect",
        ("DELETE", false) => "remove",
        ("PATCH", false) => "rename",
        _ => return Some(((405, json!({ "message": if connect { "Only POST." } else { "Rename (PATCH) or remove (DELETE)." } })), false)),
    };
    let waits = computer["actions"].as_array().and_then(|actions| actions.iter().find(|action| action["id"] == json!(wanted))).and_then(|action| action["why"].as_str().map(str::to_owned));
    if let Some(why) = waits {
        return Some(((409, json!({ "message": why })), false));
    }
    if !offers(wanted) {
        return Some(((409, json!({ "message": format!("{} can’t be {} from here.", computer["name"].as_str().unwrap_or_default(), if wanted == "connect" { "connected to" } else { wanted }) })), false));
    }
    if let Some(refused) = areas.manage_refused.clone().filter(|_| wanted != "connect") {
        return Some(((503, json!({ "message": refused })), false));
    }
    match wanted {
        "connect" => {
            areas.reaching.push(key.to_owned());
            Some(((202, json!({})), true))
        }
        "remove" => {
            areas.removed.push(key.to_owned());
            Some(((200, json!({})), true))
        }
        // (Renaming waits on its design: the stand-in answers as a box would.)
        _ => Some(((200, json!({ "name": computer["name"] })), false)),
    }
}

/// The hosts in its `~/.ssh/config`, as Add a Machine suggests them.
const SSH_HOSTS: [&str; 3] = ["mac-mini", "gpu-box", "build.internal"];

fn step(id: &str, title: &str, state: &str) -> Value {
    json!({ "id": id, "title": title, "state": state })
}

/// A try's steps for adding, the first `done` of them done and the next `now` (none: all waiting after).
fn add_steps(done: usize, now: Option<Value>) -> Vec<Value> {
    let titles = [("checking", "Checking it"), ("files", "Copying Kuvo Code"), ("engine", "Copying the engine"), ("starting", "Starting Kuvo Code there"), ("seeding", "Copying your keys and sign-ins"), ("joining", "Adding it to your computers")];
    titles.iter().enumerate().map(|(ix, (id, title))| if ix < done { step(id, title, "done") } else if ix == done { now.clone().unwrap_or_else(|| step(id, title, "waiting")) } else { step(id, title, "waiting") }).collect()
}

/// What adding a computer over SSH comes to, as a box answers it (`kuvo/computers/add…`,
/// `kuvo/computers/<key>/setup` and `/uninstall`): its answer, and whether the list of computers
/// changed. An unknown host is asked about; Trust goes on to the engine's copy, where it waits
/// for `adding_goes_on`; a host named `nowhere…` can't be reached.
pub fn adding(areas: &mut Areas, method: &str, route: &str, body: &Value) -> Option<((u16, Value), bool)> {
    let said = |areas: &Areas| {
        let hosts: Vec<&str> = if areas.add_cannot.is_some() { Vec::new() } else { SSH_HOSTS.iter().copied().filter(|host| !areas.added.iter().any(|added| added == host)).collect() };
        let mut said = json!({ "can": areas.add_cannot.is_none(), "hosts": hosts, "tries": areas.tries });
        if let Some(why) = &areas.add_cannot {
            said["why"] = json!(why);
        }
        said
    };
    let rest = route.strip_prefix("kuvo/computers/")?;
    if let Some(key) = rest.strip_suffix("/update") {
        let Some(updating) = areas.updates.iter_mut().find(|updating| updating.key == key) else {
            return Some(((404, json!({ "message": "That isn’t a computer this one set up over SSH." })), false));
        };
        updating.state = "updating";
        updating.long = false;
        areas.tried += 1;
        let tried = json!({ "id": format!("add_{}", areas.tried), "kind": "update", "destination": key, "name": key, "state": "working", "title": "Updating Kuvo…", "steps": add_steps(1, None) });
        areas.tries.insert(0, tried.clone());
        return Some(((202, json!({ "try": tried })), true));
    }
    if let Some(key) = rest.strip_suffix("/setup").filter(|key| areas.set_up_again.iter().any(|again| again == key)) {
        let destination = key.strip_prefix("key_").unwrap_or(key).to_owned();
        areas.tried += 1;
        let tried = json!({ "id": format!("add_{}", areas.tried), "kind": "setup", "destination": destination, "name": destination, "state": "working", "title": "Setting it up again…", "steps": add_steps(1, None) });
        areas.tries.insert(0, tried.clone());
        return Some(((202, json!({ "try": tried })), true));
    }
    if let Some(key) = rest.strip_suffix("/setup").or_else(|| rest.strip_suffix("/uninstall")) {
        let uninstall = rest.ends_with("/uninstall");
        let Some(name) = key.strip_prefix("key_added_").filter(|name| areas.added.iter().any(|added| added == name)).map(str::to_owned) else {
            return Some(((404, json!({ "message": "That isn’t a computer this one set up over SSH." })), false));
        };
        areas.tried += 1;
        let tried = if uninstall {
            areas.added.retain(|added| *added != name);
            json!({ "id": format!("add_{}", areas.tried), "kind": "uninstall", "destination": name, "name": name, "state": "done", "title": format!("{name} no longer has Kuvo Code running"),
                "steps": [step("checking", "Reaching it", "done"), step("removing", "Stopping and removing Kuvo Code", "done"), step("forgetting", "Taking it off your computers", "done")] })
        } else {
            let mut steps = add_steps(3, Some(json!({ "id": "starting", "title": "Restarting Kuvo Code there", "state": "now" })));
            steps[1]["detail"] = json!("It has them already");
            steps[2]["detail"] = json!("It has it already");
            json!({ "id": format!("add_{}", areas.tried), "kind": "setup", "destination": name, "name": name, "state": "working", "title": format!("Setting up {name}…"), "steps": steps })
        };
        areas.tries.insert(0, tried.clone());
        return Some(((202, json!({ "try": tried })), uninstall));
    }
    let rest = rest.strip_prefix("add")?;
    if !rest.is_empty() && !rest.starts_with('/') {
        return None;
    }
    let rest = rest.trim_start_matches('/');
    if rest.is_empty() {
        if method == "GET" {
            return Some(((200, said(areas)), false));
        }
        if let Some(why) = &areas.add_cannot {
            return Some(((409, json!({ "message": why })), false));
        }
        let destination = body["destination"].as_str().unwrap_or_default().trim().to_owned();
        if destination.is_empty() || destination.starts_with('-') || destination.contains([':', '/', ' ']) {
            return Some(((400, json!({ "message": "Give a host name, or user@host. For a port or an IPv6 address, add a Host for the machine to ~/.ssh/config and give its name." })), false));
        }
        areas.tried += 1;
        let id = format!("add_{}", areas.tried);
        let tried = if destination.starts_with("nowhere") {
            json!({ "id": id, "kind": "add", "destination": destination, "name": destination, "state": "failed", "title": format!("Couldn’t add {destination}"),
                "steps": add_steps(0, Some(step("checking", "Checking it", "failed"))),
                "failure": { "title": format!("Couldn’t add {destination}"), "message": format!("Couldn’t reach {destination}. ssh: Could not resolve hostname {destination}: nodename nor servname provided, or not known."), "again": true } })
        } else {
            json!({ "id": id, "kind": "add", "destination": destination, "name": destination, "state": "asking", "title": format!("Adding {destination}…"),
                "steps": add_steps(0, Some(json!({ "id": "checking", "title": "Checking it", "state": "now", "detail": format!("Connecting to {destination}") }))),
                "question": { "id": "hostKey", "at": areas.tried, "title": format!("Is this {destination}?"),
                    "message": format!("{} hasn’t connected to {destination} before. Check that the fingerprint is its own (on that computer: ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub), then trust it.", super::NAME),
                    "facts": [{ "label": "Address", "value": destination }, { "label": "Fingerprint (ED25519)", "value": "SHA256:Vq0bYJx3mD2Wc7PpN1tKfUo9sLhGe5RaZi8XwCnEyTM", "code": true }],
                    "answers": [{ "id": "cancel", "label": "Cancel", "kind": "plain" }, { "id": "trust", "label": "Trust", "kind": "primary" }] } })
        };
        // (One try a destination: a new one takes the place of what's over.)
        areas.tries.retain(|each| each["destination"] != json!(destination));
        areas.tries.insert(0, tried.clone());
        return Some(((202, json!({ "try": tried })), false));
    }
    if rest == "events" {
        return Some(((200, said(areas)), false));
    }
    let (id, answering) = match rest.strip_suffix("/answer") {
        Some(id) => (id, true),
        None => (rest, false),
    };
    let Some(at) = areas.tries.iter().position(|each| each["id"] == json!(id)) else { return Some(((404, json!({ "message": "There’s no such try." })), false)) };
    if !answering {
        areas.tries.remove(at);
        return Some(((200, json!({})), false));
    }
    if areas.tries[at]["state"] != json!("asking") {
        return Some(((409, json!({ "message": "It isn’t asking that now." })), false));
    }
    // (An answer names the ask it's for: one that doesn't, or names another, isn't taken.)
    if body.get("at") != Some(&areas.tries[at_ix(&areas.tries, id)]["question"]["at"]) {
        return Some(((409, json!({ "message": "It isn’t asking that now." })), false));
    }
    if body["answer"] != json!("trust") {
        areas.tries.remove(at);
        return Some(((200, json!({ "try": { "id": id, "state": "stopping", "title": "Stopping…" } })), false));
    }
    let tried = &mut areas.tries[at];
    tried.as_object_mut().expect("a try").remove("question");
    tried["state"] = json!("working");
    let mut steps = add_steps(2, Some(json!({ "id": "engine", "title": "Copying the engine", "state": "now", "detail": "41 of 98 MB", "sent": 41_000_000u64, "total": 98_000_000u64 })));
    steps[0]["detail"] = tried["steps"][0]["detail"].clone();
    tried["steps"] = json!(steps);
    Some(((200, json!({ "try": tried })), false))
}

fn at_ix(tries: &[Value], id: &str) -> usize {
    tries.iter().position(|each| each["id"] == json!(id)).unwrap_or_default()
}

/// The tries under way come to their end: one being added is one of your computers, one being set
/// up again is. True when the list of computers changed.
pub fn adding_goes_on(areas: &mut Areas) -> bool {
    let mut changed = false;
    for tried in areas.tries.iter_mut().filter(|each| each["state"] == json!("working")) {
        let name = tried["name"].as_str().unwrap_or_default().to_owned();
        let again = tried["kind"] == json!("setup");
        for step in tried["steps"].as_array_mut().into_iter().flatten() {
            step["state"] = json!("done");
            if step["id"] == json!("seeding") {
                step["detail"] = json!(if again { "It keeps its own".to_owned() } else { format!("Copied 3 keys and 2 accounts. One sign-in renews itself, so sign in to it on {name}.") });
            }
            if let Some(step) = step.as_object_mut() {
                step.remove("sent");
                step.remove("total");
            }
        }
        tried["state"] = json!("done");
        tried["title"] = json!(if again { format!("{name} is set up again") } else { format!("{name} is one of your computers") });
        tried["computer"] = json!({ "key": format!("key_added_{name}"), "name": name });
        if !again && !areas.added.contains(&name) {
            areas.added.push(name);
            changed = true;
        }
    }
    changed
}

/// The computers as listed now: those removed gone, those being reached again so.
fn reach_titles() -> Value {
    json!({ "connecting": "Connecting…", "held": "Needs an update", "away": "Can’t be reached",
        "notPaired": "Hasn’t taken this app yet",
        "unreached": "Can’t be reached from this app" })
}

fn listed(areas: &Areas) -> Vec<Value> {
    let added = areas.added.iter().map(|name| {
        let busy = areas.tries.iter().any(|each| each["destination"] == json!(name) && each["state"] == json!("working"));
        let mut setup = json!({ "id": "setup", "label": "Set Up Again" });
        if busy {
            setup["why"] = json!("It’s being set up now.");
        }
        json!({ "key": format!("key_added_{name}"), "name": name, "kind": "machine", "id": name, "reach": "connected", "version": "0.8.0", "engine": "2.3.0", "system": "macOS 15.1", "via": "relay", "working": 0, "atWork": [], "limits": [],
            "features": kuvo_box::gate::REQUIRED,
            "actions": [{ "id": "rename", "label": "Rename" },
                { "id": "remove", "label": "Remove", "confirm": { "title": format!("Remove {name}?"), "message": format!("Your other computers stop reaching {name}. It keeps running, with its sessions, and you can add it again later."), "label": "Remove" } },
                setup,
                { "id": "uninstall", "label": "Uninstall", "confirm": { "title": format!("Uninstall Kuvo Code from {name}?"), "message": format!("{} connects to {name} over SSH and stops Kuvo Code there: its engine and agents, and what starts them after a restart. The sign-ins and keys it keeps are deleted. Its sessions stay in ~/.kuvo-code on {name}.", super::NAME), "label": "Uninstall" } }] })
    });
    computers((areas.empty && !areas.lists_yours) || areas.computers_alone, areas.more_computers)
        .into_iter()
        .chain(added)
        .filter(|computer| !areas.removed.iter().any(|key| computer["key"] == json!(key)))
        .map(|mut computer| {
            computer["reachTitles"] = reach_titles();
            // (Removing waits while the old app is open on the Mac, saying so.)
            if areas.old_app {
                let name = computer["name"].as_str().unwrap_or_default().to_owned();
                for action in computer["actions"].as_array_mut().into_iter().flatten() {
                    if action["id"] == json!("remove") {
                        *action = json!({ "id": "remove", "label": "Remove", "why": format!("Quit the old Kuvo Code app on {} first: it keeps its own list of machines, and would add {name} back.", super::NAME) });
                    }
                }
            }
            if areas.vouch_blocked.iter().any(|key| computer["key"] == json!(key)) {
                let name = computer["name"].as_str().unwrap_or_default().to_owned();
                computer["vouching"] = json!({ "blocked": true, "words": format!("Kuvo on {name} was removed from your devices. {name} can’t add Kuvo Code’s app again until you allow it.") });
                if let Some(actions) = computer["actions"].as_array_mut() {
                    actions.push(json!({ "id": "allow-devices", "label": "Allow Its App Again", "confirm": { "title": format!("Allow Kuvo Code’s app on {name} again?"), "message": format!("Kuvo Code on {name} can add itself to your devices again, and reach every one of your computers."), "label": "Allow" } }));
                }
            }
            if areas.as_macs.iter().any(|key| computer["key"] == json!(key)) {
                computer["kind"] = json!("mac");
            }
            if let Some((_, system)) = areas.systems.iter().find(|(key, _)| computer["key"] == json!(key)) {
                match system {
                    Some(system) => computer["system"] = json!(system),
                    None => {
                        if let Some(map) = computer.as_object_mut() {
                            map.remove("system");
                        }
                    }
                }
            }
            if let Some((_, why)) = areas.cant_bring_up.iter().find(|(key, _)| computer["key"] == json!(key)) {
                let name = computer["name"].as_str().unwrap_or_default().to_owned();
                computer["update"] = json!({ "state": "waiting", "title": format!("Kuvo on {name} needs an update"), "detail": why });
            }
            if areas.to_set_up.iter().any(|key| computer["key"] == json!(key)) {
                let name = computer["name"].as_str().unwrap_or_default().to_owned();
                computer["update"] = json!({ "state": "waiting", "title": format!("Kuvo on {name} needs an update"), "detail": format!("{} hasn’t set it up over SSH, so it can’t update it yet. Set it up over SSH once: what runs there stays.", super::NAME) });
                if let Some(actions) = computer["actions"].as_array_mut() {
                    actions.push(json!({ "id": "add", "label": "Set Up Over SSH…" }));
                }
            }
            if areas.set_up_again.iter().any(|key| computer["key"] == json!(key))
                && let Some(actions) = computer["actions"].as_array_mut()
            {
                actions.push(json!({ "id": "setup", "label": "Set Up Again" }));
            }
            if computer["here"] == json!(true)
                && let Some(updating) = &areas.here_update
            {
                let name = computer["name"].as_str().unwrap_or_default().to_owned();
                computer["update"] = json!({ "state": updating.state, "title": if updating.state == "updating" { format!("Updating Kuvo on {name}…") } else { format!("Kuvo on {name} updates once your sessions there finish.") }, "detail": "" });
                if updating.state != "updating"
                    && let Some(actions) = computer["actions"].as_array_mut()
                {
                    let mut action = json!({ "id": "update-here", "label": "Update Now" });
                    if updating.working > 0 {
                        action["confirm"] = json!({ "title": format!("Update Kuvo on {name} now?"), "message": format!("{} at work on {name} stop as Kuvo restarts there.", if updating.working == 1 { "1 session is".to_owned() } else { format!("{} sessions are", updating.working) }), "label": "Update Now" });
                    }
                    actions.push(action);
                }
            }
            if let Some(updating) = areas.updates.iter().find(|updating| computer["key"] == json!(updating.key)) {
                let name = computer["name"].as_str().unwrap_or_default().to_owned();
                let (update, action) = update_said(updating, &name);
                computer["update"] = update;
                if let (Some(action), Some(actions)) = (action, computer["actions"].as_array_mut()) {
                    actions.insert(0, action);
                }
            }
            if areas.reaching.iter().any(|key| computer["key"] == json!(key)) {
                computer["reach"] = json!("connecting");
                computer["title"] = json!("Connecting");
                computer["detail"] = json!("Studio hasn’t heard from it yet.");
                computer["notice"] = json!({"title": format!("Connecting to {}…", computer["name"].as_str().unwrap()), "detail": "Studio hasn’t heard from it yet."});
                computer["settingsNotice"] = json!({"title": format!("Connecting to {}…", computer["name"].as_str().unwrap()), "detail": "Its settings show here once Studio hears from it."});
                if let Some(actions) = computer["actions"].as_array_mut() {
                    actions.retain(|action| action["id"] != json!("connect"));
                }
            }
            computer
        })
        .collect()
}

/// Your computers, the stand-in itself first: a machine at work, a Mac that's away, a Mac on an
/// older Kuvo Code; `more` machines after them. Each with
/// what can be done with it from here, as a Mac's box that writes its machines' records offers it.
fn computers(alone: bool, more: usize) -> Vec<Value> {
    let remove = |name: &str| json!({ "id": "remove", "label": "Remove", "confirm": { "title": format!("Remove {name}?"), "message": format!("Your other computers stop reaching {name}. It keeps running, with its sessions, and you can add it again later."), "label": "Remove" } });
    let mut computers = vec![json!({
        "key": "key_stand_in", "name": super::NAME, "kind": "mac", "here": true, "reach": "connected",
        "version": "0.8.0-beta.12", "kuvo": "0.8.0-beta.33", "engine": "2.3.0", "system": "macOS 15.1", "via": "here", "working": 1, "atWork": [{ "id": "ses_limits", "title": "Plan limits" }], "limits": ["claude", "codex", "copilot", "grok", "openrouter"],
        "features": kuvo_box::gate::REQUIRED, "actions": [],
    })];
    if !alone {
        computers.extend([
            json!({ "key": "key_devbox", "name": "devbox", "kind": "machine", "id": "devbox", "reach": "connected", "version": "0.8.0", "engine": "2.3.0", "system": "Ubuntu 24.04.1 LTS", "via": "relay", "working": 2,
                "atWork": [{ "id": "ses_dev_1", "title": "Migrate the billing tables" }, { "id": "ses_dev_2", "title": "Nightly test run" }], "limits": ["claude"], "features": kuvo_box::gate::REQUIRED,
                "actions": [{ "id": "rename", "label": "Rename" }, remove("devbox")] }),
            json!({ "key": "key_studio", "name": "studio-mini", "kind": "mac", "reach": "away", "version": "0.8.0-beta.10", "kuvo": "0.8.0-beta.32", "engine": "2.3.0", "system": "macOS 15.1", "via": "relay", "working": 0, "atWork": [], "limits": [], "features": kuvo_box::gate::REQUIRED,
                "actions": [{ "id": "connect", "label": "Connect" }] }),
            // A Mac on an older Kuvo Code: what it answers lacks what this app needs, and it never said what's at work.
            json!({ "key": "key_old", "name": "old-mini", "kind": "mac", "reach": "connected", "version": "0.9.180", "engine": "1.14.0", "system": "macOS 14.6", "via": "relay", "limits": ["claude"], "features": kuvo_box::gate::BASELINE, "actions": [] }),
        ]);
    }
    let reaches = ["connected", "connected", "away", "connecting"];
    computers.extend((0..more).map(|ix| {
        let reach = reaches[ix % reaches.len()];
        let name = format!("worker-{:02}", ix + 1);
        let mut computer = json!({ "key": format!("key_more_{ix}"), "name": name, "kind": "machine", "id": name, "reach": reach, "via": "relay", "limits": [], "actions": [] });
        computer["working"] = json!(if reach == "connected" { ix % 3 } else { 0 });
        computer["features"] = json!(kuvo_box::gate::REQUIRED);
        let mut actions = vec![json!({ "id": "rename", "label": "Rename" })];
        if reach == "away" {
            actions.push(json!({ "id": "connect", "label": "Connect" }));
        }
        actions.push(remove(&name));
        computer["actions"] = json!(actions);
        computer["version"] = json!("0.8.0");
        computer["engine"] = json!("2.3.0");
        computer["system"] = json!(if ix % 2 == 0 { "Ubuntu 24.04.1 LTS" } else { "Debian 12" });
        computer
    }));
    for computer in &mut computers {
        computer["reachTitles"] = reach_titles();
        let reach = computer["reach"].as_str().unwrap_or("connected");
        let title = match reach {
            "away" => "Can’t be reached".to_owned(),
            "connecting" => "Connecting".to_owned(),
            _ => "Connected".to_owned(),
        };
        computer["title"] = json!(title);
        if let Some(count) = computer["working"].as_u64() {
            computer["workingTitle"] = json!(format!("{count} {} at work", if count == 1 { "session" } else { "sessions" }));
        }
        if computer["reach"] == "away" {
            let detail = "Studio hasn’t heard from it for a while: it may be asleep, off, or offline. It shows here again by itself once it’s back, with what it runs.";
            computer["detail"] = json!(detail);
            computer["notice"] = json!({"title": format!("{} can’t be reached", computer["name"].as_str().unwrap()), "detail": detail});
            let name = computer["name"].as_str().unwrap().to_owned();
            computer["settingsNotice"] = json!({"title": format!("{name} can’t be reached"), "detail": format!("Its settings are kept on {name}, so they can’t be read or changed until Studio hears from it again. They show here by themselves once it’s back.")});
        }
    }
    computers
}

fn accounts(now: f64, sample: UsageSample) -> Vec<Value> {
    // Each window named as the box names it (`title`).
    let window = |id: &str, title: &str, used: f64, resets: f64, minutes: Option<u32>, model: Option<&str>, name: Option<&str>| {
        let mut window = json!({ "id": id, "title": title, "used": used, "resetsAt": now + resets, "read": now - 2.0 * MINUTE, "source": "usage" });
        if let Some(minutes) = minutes {
            window["minutes"] = json!(minutes);
        }
        if let Some(model) = model {
            window["model"] = json!(model);
        }
        if let Some(name) = name {
            window["name"] = json!(name);
        }
        window["level"] = json!(level(used));
        window
    };
    // The box picks a row's rings: among the whole plan's windows, the shortest under a day is
    // `short` and the longest of a day or more `long`; a plan's only window that states no length
    // is `long`; one bar shows the fuller (`bar`, the short one where they're as full). Each row is
    // named as the box names it (`title`).
    let row = |service: &str, title: &str, agent: Option<&str>, label: &str, plan: &str, mut windows: Vec<Value>| {
        let whole: Vec<usize> = (0..windows.len()).filter(|ix| windows[*ix].get("model").is_none()).collect();
        let minutes = |ix: &usize| windows[*ix]["minutes"].as_u64();
        let short = whole.iter().copied().filter(|ix| minutes(ix).is_some_and(|minutes| minutes < 1440)).min_by_key(|ix| minutes(ix));
        let long = whole.iter().copied().filter(|ix| minutes(ix).is_some_and(|minutes| minutes >= 1440)).max_by_key(|ix| minutes(ix)).or_else(|| (short.is_none() && whole.len() == 1).then(|| whole[0]));
        let mut rings = json!({});
        for (span, ix) in [("short", short), ("long", long)] {
            if let Some(ix) = ix {
                windows[ix]["span"] = json!(span);
                let window = &windows[ix];
                rings[span] = json!({ "window": window["id"], "used": window["used"], "resetsAt": window["resetsAt"], "level": window["level"] });
            }
        }
        let used = |span: &str| rings[span]["used"].as_f64();
        let bar = match (used("short"), used("long")) {
            (Some(short), Some(long)) if long > short => Some("long"),
            (Some(_), _) => Some("short"),
            (None, Some(_)) => Some("long"),
            _ => None,
        };
        // (Signed in on this computer, as its box says: `boxes`, `here`.)
        let mut row = json!({ "id": title, "service": service, "title": title, "label": label, "plan": plan, "windows": windows, "rings": rings, "checked": now - MINUTE, "boxes": [{ "key": "here-key", "name": "This Mac", "here": true }] });
        if let Some(bar) = bar {
            row["bar"] = json!(bar);
        }
        if let Some(agent) = agent {
            row["agent"] = json!(agent);
        }
        row
    };
    let mut rows = vec![
        row("claude", "Claude · Work", Some("claude"), "Work", "Max 20x", vec![window("five_hour", "Session", 0.46, 3.7 * HOUR, Some(300), None, None), window("seven_day", "Weekly", 0.38, 4.0 * DAY, Some(10080), None, None), window("seven_day_sonnet", "Sonnet", 0.12, 4.0 * DAY, Some(10080), Some("Sonnet"), None)]),
        // A plan that ended (Cursor said no active subscription): no windows, no rings, `ended`.
        {
            let mut ended = row("cursor", "Cursor", Some("cursor"), "", "Pro", Vec::new());
            ended["ended"] = json!(true);
            ended["problem"] = json!({ "kind": "ended", "message": "Its Cursor plan isn't active now." });
            ended
        },
        // Signed in only on another computer (its `boxes` don't say `here`): listed in Usage, no ring here.
        {
            let mut elsewhere = row("zai", "Z.ai · mini-local", None, "", "Coding Pro", vec![window("five_hour", "Session", 0.3, 2.0 * HOUR, Some(300), None, None)]);
            elsewhere["boxes"] = json!([{ "key": "mini-key", "name": "mini-local" }]);
            elsewhere
        },
        row("claude", "Claude · Personal", Some("claude"), "Personal", "Pro", vec![window("five_hour", "Session", 0.18, 4.1 * HOUR, Some(300), None, None), window("seven_day", "Weekly", 0.22, 2.0 * DAY, Some(10080), None, None)]),
        row("codex", "Codex", Some("codex"), "", "Pro", vec![window("primary", "Session", 0.74, 52.0 * MINUTE, Some(300), None, None), window("secondary", "Weekly", 0.41, 5.0 * DAY, Some(10080), None, None)]),
        row("copilot", "Copilot", None, "", "Pro+", vec![window("premium", "Premium requests", 0.35, 17.0 * DAY, None, None, Some("Premium requests"))]),
        row("grok", "Grok", Some("grok"), "", "SuperGrok", vec![window("week", "Weekly", 0.12, 3.0 * DAY, Some(10080), None, None)]),
        row("openrouter", "OpenRouter · Team key", None, "Team key", "Credits", vec![window("credits", "Credits", 0.58, 20.0 * DAY, None, None, Some("Credits"))]),
    ];
    match sample {
        UsageSample::Default => {}
        UsageSample::Many => {
            rows.insert(4, row("antigravity", "Antigravity", Some("antigravity"), "", "Pro", vec![
                window("gemini-5h", "Gemini · 5-hour", 0.35, 4.0 * HOUR, Some(300), None, Some("Gemini · 5-hour")),
                window("gemini-weekly", "Gemini · Weekly", 0.72, 6.0 * DAY, Some(10080), None, Some("Gemini · Weekly")),
                window("third-party-5h", "Claude and GPT · 5-hour", 0.18, 3.0 * HOUR, Some(300), None, Some("Claude and GPT · 5-hour")),
                window("third-party-weekly", "Claude and GPT · Weekly", 0.65, 6.0 * DAY, Some(10080), None, Some("Claude and GPT · Weekly")),
            ]));
            rows.insert(6, row("opencode", "OpenCode Go", None, "", "Go", vec![
                window("five_hour", "Session", 0.14, 3.0 * HOUR, Some(300), None, None),
                window("weekly", "Weekly", 0.54, 5.0 * DAY, Some(10080), None, None),
                window("monthly", "Monthly", 0.91, 12.0 * DAY, Some(43200), None, None),
            ]));
        }
        UsageSample::One => rows.truncate(1),
        UsageSample::UnmeasuredClaude => {
            rows = vec![row("claude", "Claude · Work", Some("claude"), "Work", "Max 20x", Vec::new())];
        }
        UsageSample::Failed => {
            rows = vec![row("codex", "Codex", Some("codex"), "", "Pro", Vec::new())];
            rows[0]["problem"] = json!({ "kind": "failed" });
        }
        UsageSample::Stale => {
            rows = vec![rows.remove(2)];
            rows[0]["boxes"][0]["away"] = json!(true);
            rows[0]["windows"][0]["read"] = json!(now - 47.0 * MINUTE);
        }
    }
    for row in &mut rows {
        row["usage"] = usage_display(row, now);
    }
    rows
}

/// The stand-in supplies display facts the same way the live route does.
fn usage_display(row: &Value, now: f64) -> Value {
    let windows = row["windows"].as_array().unwrap();
    let title = row["title"].as_str().unwrap_or_default();
    let computer = &row["boxes"][0];
    let name = computer["name"].as_str().unwrap_or_default();
    let suffix = format!(" · {name}");
    let title = title.strip_suffix(&suffix).unwrap_or(title);
    let status = match row["problem"]["kind"].as_str() {
        Some("ended") => json!({ "words": "Plan ended.", "warning": false }),
        Some("failed") => json!({ "words": "Couldn’t read limits. Check the connection, then try again.", "warning": true }),
        _ => Value::Null,
    };
    let meters: Vec<Value> = windows.iter().map(|window| {
        let share = window["used"].as_f64().unwrap_or_default();
        let title = match window["title"].as_str() {
            Some("Session") => "5-hour",
            Some("Sonnet") => "Sonnet · Weekly",
            Some(title) => title,
            None => "Limit",
        };
        let reset = window["resetsAt"].as_f64();
        let read = window["read"].as_f64().unwrap_or(now);
        json!({
            "title": title, "share": share, "value": format!("{:.0}%", share * 100.0),
            "level": window["level"], "resetsAt": reset, "readAt": read,
            "resetLabels": reset.filter(|at| at - now < DAY).map(|at| json!([
                { "at": now, "words": format!("resets in {} h", ((at - now) / HOUR).ceil()) }
            ])).unwrap_or_else(|| json!([])), "freshForMs": if row["boxes"][0]["away"] == json!(true) { 0 } else { 28 * MINUTE as u64 },
        })
    }).collect();
    let updated = meters.iter().filter_map(|meter| meter["readAt"].as_f64()).max_by(f64::total_cmp);
    json!({ "title": title, "computers": [computer], "status": status, "meters": meters, "updatedAt": updated })
}

fn level(used: f64) -> &'static str {
    if used >= 0.9 {
        "over"
    } else if used >= 0.7 {
        "near"
    } else {
        "calm"
    }
}
