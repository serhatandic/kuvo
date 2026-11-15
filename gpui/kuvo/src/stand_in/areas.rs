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
    /// What renaming or removing says instead of doing it, as a box that can't ask the computer
    /// writing its record says (`503`).
    pub manage_refused: Option<String>,
    /// The old Kuvo Code app is open on the Mac that writes your machines' records: removing waits.
    pub old_app: bool,
}

/// Its answer to `method route`, if it's one of the areas' routes. `needing`: how many of its
/// sessions "Needs you" lists.
pub fn answer(areas: &Areas, method: &str, route: &str, now: f64, needing: usize, bots_asking: usize) -> Option<(u16, Value)> {
    if method != "GET" {
        return None;
    }
    if route == "kuvo/computers" && areas.computers_down {
        return Some((503, json!({ "message": "The box isn’t answering." })));
    }
    let answer = match route {
        "kuvo/computers" => json!({ "computers": listed(areas) }),
        "kuvo/agents/api/kuvo/limits" => json!({ "data": { "accounts": [], "services": ["claude", "codex", "copilot", "grok", "openrouter"], "rows": if areas.empty { Vec::new() } else { accounts(now) } } }),
        "kuvo/waiting" => json!({ "sessions": needing, "bots": bots_asking, "queue": 0 }),
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

/// The computers as listed now: those removed gone, those being reached again so.
fn listed(areas: &Areas) -> Vec<Value> {
    computers(areas.empty || areas.computers_alone, areas.more_computers)
        .into_iter()
        .filter(|computer| !areas.removed.iter().any(|key| computer["key"] == json!(key)))
        .map(|mut computer| {
            // (Removing waits while the old app is open on the Mac, saying so.)
            if areas.old_app {
                let name = computer["name"].as_str().unwrap_or_default().to_owned();
                for action in computer["actions"].as_array_mut().into_iter().flatten() {
                    if action["id"] == json!("remove") {
                        *action = json!({ "id": "remove", "label": "Remove", "why": format!("Quit the old Kuvo Code app on {} first: it keeps its own list of machines, and would add {name} back.", super::NAME) });
                    }
                }
            }
            if areas.reaching.iter().any(|key| computer["key"] == json!(key)) {
                computer["reach"] = json!("connecting");
                if let Some(actions) = computer["actions"].as_array_mut() {
                    actions.retain(|action| action["id"] != json!("connect"));
                }
            }
            computer
        })
        .collect()
}

/// Your computers, the stand-in itself first: a machine at work, a Mac that's away, a Mac on an
/// older Kuvo Code, and a machine shared with another setup; `more` machines after them. Each with
/// what can be done with it from here, as a Mac's box that writes its machines' records offers it.
fn computers(alone: bool, more: usize) -> Vec<Value> {
    let remove = |name: &str| json!({ "id": "remove", "label": "Remove", "confirm": { "title": format!("Remove {name}?"), "message": format!("Your other computers stop reaching {name}. It keeps running, with its sessions, and you can add it again later."), "label": "Remove" } });
    let mut computers = vec![json!({
        "key": "key_stand_in", "name": super::NAME, "kind": "mac", "here": true, "shared": false, "reach": "connected",
        "version": "1.0.0", "engine": "2.3.0", "system": "macOS 15.1", "via": "here", "working": 1, "atWork": [{ "id": "ses_limits", "title": "Plan limits" }], "limits": ["claude", "codex", "copilot", "grok", "openrouter"],
        "features": kuvo_box::gate::REQUIRED, "actions": [],
    })];
    if !alone {
        computers.extend([
            json!({ "key": "key_devbox", "name": "devbox", "kind": "machine", "id": "devbox", "shared": false, "reach": "connected", "version": "1.0.0", "engine": "2.3.0", "system": "Ubuntu 24.04.1 LTS", "via": "relay", "working": 2,
                "atWork": [{ "id": "ses_dev_1", "title": "Migrate the billing tables" }, { "id": "ses_dev_2", "title": "Nightly test run" }], "limits": ["claude"], "features": kuvo_box::gate::REQUIRED,
                "actions": [{ "id": "rename", "label": "Rename" }, remove("devbox")] }),
            json!({ "key": "key_studio", "name": "studio-mini", "kind": "mac", "shared": false, "reach": "away", "version": "1.0.0", "engine": "2.3.0", "system": "macOS 15.1", "via": "relay", "working": 0, "atWork": [], "limits": [], "features": kuvo_box::gate::REQUIRED,
                "actions": [{ "id": "connect", "label": "Connect" }] }),
            // A Mac on an older Kuvo Code: what it answers lacks what this app needs, and it never said what's at work.
            json!({ "key": "key_old", "name": "old-mini", "kind": "mac", "shared": false, "reach": "connected", "version": "0.9.180", "engine": "1.14.0", "system": "macOS 14.6", "via": "relay", "limits": ["claude"], "features": ["sessions.list", "messages.send"], "actions": [] }),
            json!({ "key": "key_build", "name": "build-box", "kind": "machine", "id": "build-box", "shared": true, "apart": true, "reach": "connected", "system": "Debian 12", "via": "relay", "limits": [], "actions": [] }),
        ]);
    }
    let reaches = ["connected", "connected", "away", "connecting", "unknown"];
    computers.extend((0..more).map(|ix| {
        let reach = reaches[ix % reaches.len()];
        let name = format!("worker-{:02}", ix + 1);
        let mut computer = json!({ "key": format!("key_more_{ix}"), "name": name, "kind": "machine", "id": name, "shared": false, "reach": reach, "via": if reach == "unknown" { "peer" } else { "relay" }, "limits": [], "actions": [] });
        if reach == "unknown" {
            computer["through"] = json!("devbox");
        } else {
            computer["working"] = json!(if reach == "connected" { ix % 3 } else { 0 });
            computer["features"] = json!(kuvo_box::gate::REQUIRED);
            let mut actions = vec![json!({ "id": "rename", "label": "Rename" })];
            if reach == "away" {
                actions.push(json!({ "id": "connect", "label": "Connect" }));
            }
            actions.push(remove(&name));
            computer["actions"] = json!(actions);
            computer["version"] = json!("1.0.0");
            computer["engine"] = json!("2.3.0");
            computer["system"] = json!(if ix % 2 == 0 { "Ubuntu 24.04.1 LTS" } else { "Debian 12" });
        }
        computer
    }));
    computers
}

fn accounts(now: f64) -> Vec<Value> {
    let window = |id: &str, used: f64, resets: f64, minutes: Option<u32>, model: Option<&str>, name: Option<&str>| {
        let mut window = json!({ "id": id, "used": used, "resetsAt": now + resets, "read": now - MINUTE, "source": "usage" });
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
    // is `long`.
    let row = |service: &str, agent: Option<&str>, label: &str, plan: &str, mut windows: Vec<Value>| {
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
        let mut row = json!({ "service": service, "label": label, "plan": plan, "windows": windows, "rings": rings, "checked": now - MINUTE });
        if let Some(agent) = agent {
            row["agent"] = json!(agent);
        }
        row
    };
    vec![
        row("claude", Some("claude"), "Work", "Max 20x", vec![window("five_hour", 0.46, 3.7 * HOUR, Some(300), None, None), window("seven_day", 0.38, 4.0 * DAY, Some(10080), None, None), window("seven_day_sonnet", 0.12, 4.0 * DAY, Some(10080), Some("Sonnet"), None)]),
        row("claude", Some("claude"), "Personal", "Pro", vec![window("five_hour", 0.18, 4.1 * HOUR, Some(300), None, None), window("seven_day", 0.22, 2.0 * DAY, Some(10080), None, None)]),
        row("codex", Some("codex"), "", "Pro", vec![window("primary", 0.74, 52.0 * MINUTE, Some(300), None, None), window("secondary", 0.41, 5.0 * DAY, Some(10080), None, None)]),
        row("copilot", None, "", "Pro+", vec![window("premium", 0.35, 17.0 * DAY, None, None, Some("Premium requests"))]),
        row("grok", Some("grok"), "", "SuperGrok", vec![window("week", 0.12, 3.0 * DAY, Some(10080), None, None)]),
        row("openrouter", None, "Team key", "Credits", vec![window("credits", 0.58, 20.0 * DAY, None, None, Some("Credits"))]),
    ]
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
