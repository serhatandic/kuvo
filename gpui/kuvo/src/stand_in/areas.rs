//! What the stand-in box says of the rail's areas, in a box's own shapes (PROTOCOL.md): its bots,
//! each with what it's doing now, and group chats; the Work Queue's view with its counts; your
//! computers; the accounts' limits, each row with its rings and every window its level; and how
//! many things wait on you. As a box does, it counts, levels and picks the rings itself: the app
//! draws what it says.

use serde_json::{Value, json};

use super::{DAY, HOUR, MINUTE};

/// How the stand-in answers for the areas.
#[derive(Clone, Copy, Debug, Default)]
pub struct Areas {
    /// A first run: no bots, chats, other computers or accounts.
    pub empty: bool,
    /// Its list of computers can't be read now (503).
    pub computers_down: bool,
}

/// Its answer to `method route`, if it's one of the areas' routes. `needing`: how many of its
/// sessions "Needs you" lists.
pub fn answer(areas: Areas, method: &str, route: &str, now: f64, needing: usize, bots_asking: usize) -> Option<(u16, Value)> {
    if method != "GET" {
        return None;
    }
    if route == "kuvo/computers" && areas.computers_down {
        return Some((503, json!({ "message": "The box isn’t answering." })));
    }
    let answer = match route {
        "kuvo/groups" => json!({ "groups": if areas.empty { Vec::new() } else { groups(now) } }),
        "kuvo/work-queue" => {
            let counts = if areas.empty {
                json!({ "inProgress": 0, "categories": { "next": 0, "later": 0, "cat_design": 0, "parked": 0, "done": 0 }, "sources": {}, "computers": { "": 0 }, "removed": 0 })
            } else {
                json!({ "inProgress": 2, "categories": { "next": 6, "later": 6, "cat_design": 3, "parked": 4, "done": 31 }, "sources": { "github": 9 }, "computers": { "": 38, "key_devbox": 12 }, "removed": 3 })
            };
            json!({ "owner": "all", "categories": categories(), "items": [], "owners": [], "removed": [], "computers": [], "counts": counts })
        }
        "kuvo/computers" => json!({ "computers": computers(areas.empty) }),
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

fn groups(now: f64) -> Vec<Value> {
    vec![
        json!({ "id": "grp_launch", "name": "Launch crew", "members": [], "discussing": false, "paused": false, "unread": true, "last": { "id": "m1", "text": "Shipped.", "time": now - HOUR, "kind": "message" } }),
        json!({ "id": "grp_design", "name": "Design review", "members": [], "discussing": false, "paused": false, "unread": false, "last": { "id": "m2", "text": "Agreed.", "time": now - 2.0 * DAY, "kind": "message" } }),
    ]
}

fn categories() -> Vec<Value> {
    vec![
        json!({ "id": "next", "name": "Next", "builtIn": true }),
        json!({ "id": "later", "name": "Later", "builtIn": true }),
        json!({ "id": "cat_design", "name": "Design", "builtIn": false }),
        json!({ "id": "parked", "name": "Parked", "builtIn": true }),
        json!({ "id": "done", "name": "Done", "builtIn": true }),
    ]
}

/// Your computers, the stand-in itself first: a machine at work, a Mac that's away, and a
/// machine shared with another setup.
fn computers(alone: bool) -> Vec<Value> {
    let mut computers = vec![json!({
        "key": "key_stand_in", "name": super::NAME, "kind": "mac", "here": true, "shared": false, "reach": "connected",
        "version": "1.0.0", "engine": "2.3.0", "system": "macOS 15.1", "via": "here", "working": 1, "limits": ["claude", "codex", "copilot", "grok", "openrouter"],
    })];
    if !alone {
        computers.extend([
            json!({ "key": "key_devbox", "name": "devbox", "kind": "machine", "id": "devbox", "shared": false, "reach": "connected", "version": "1.0.0", "engine": "2.3.0", "system": "Ubuntu 24.04.1 LTS", "via": "relay", "working": 2, "limits": ["claude"] }),
            json!({ "key": "key_studio", "name": "studio-mini", "kind": "mac", "shared": false, "reach": "away", "version": "1.0.0", "engine": "2.3.0", "system": "macOS 15.1", "via": "relay", "working": 0, "limits": [] }),
            json!({ "key": "key_build", "name": "build-box", "kind": "machine", "id": "build-box", "shared": true, "apart": true, "reach": "connected", "system": "Debian 12", "via": "relay", "working": 0, "limits": [] }),
        ]);
    }
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
