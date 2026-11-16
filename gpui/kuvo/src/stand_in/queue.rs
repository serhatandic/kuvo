//! The stand-in box's Work Queue, in a box's own shapes (PROTOCOL.md, "The Work Queue"): the view
//! across your computers with its statuses, priorities, labels, who an item may be for and its
//! counts, the same view narrowed by `q`, one item whole, adding one, changing one (what it says,
//! where it stands, its priority, labels, assignee and due day), deleting it and bringing it back,
//! and its labels (made, changed, removed). As a box does, it orders, counts, searches, says what
//! may be done to each item (`can`) and refuses itself: the app draws what it says.

use serde_json::{Value, json};

use super::{DAY, HOUR, MINUTE};
use crate::time;

/// The stand-in's Work Queue.
#[derive(Default)]
pub struct Queue {
    labels: Vec<Value>,
    /// Its items whole, each with its `body`, `notes`, `files` and `links`.
    items: Vec<Value>,
    removed: Vec<Value>,
    next_number: u64,
    next_label: u64,
    /// Its view can't be read now (503).
    pub down: bool,
    /// What it says a new item starts as (a status and a priority), where a test says otherwise
    /// than Todo and Normal.
    pub starts: Option<(String, String)>,
    /// It lists a status this app has never heard of (a newer box's), with an item in it.
    pub newer_status: bool,
    /// It lists a closed status this app has never heard of (Shelved), and a label colour (teal):
    /// a newer box's, with an item in it.
    pub newer_closed: bool,
    /// The moment it was last asked at: what "today" is, for what's overdue.
    at: f64,
    /// The day it says it is, where a test says otherwise than that moment's (a box's day is its
    /// own: another zone's, or a clock that's off).
    pub today_said: Option<String>,
    /// What it says a status counts, where a test says otherwise than its items do (a box counts
    /// across what a view may not list whole).
    pub counted: Vec<(String, usize)>,
}

const DEVBOX: (&str, &str, &str) = ("key_devbox", "devbox", "machine");
const STUDIO: (&str, &str, &str) = ("key_studio", "studio-mini", "mac");
/// A Mac on a Kuvo Code from before a client's changes went between boxes.
const OLD: (&str, &str, &str) = ("key_old", "old-mini", "mac");
/// A machine Macs of different setups share: it hands nothing on to another computer.
const SHARED: (&str, &str, &str) = ("key_shared", "shared-box", "machine");

/// The colours a label may be: the looks' names.
const COLORS: [&str; 10] = ["coral", "orange", "amber", "lime", "mint", "sky", "indigo", "violet", "pink", "graphite"];

fn there(computer: (&str, &str, &str)) -> Value {
    json!({ "key": computer.0, "name": computer.1, "kind": computer.2 })
}

/// The statuses, the same on every box, in the order a client shows them; with `newer`, one a
/// newer box would list, which this app has no mark for; with `shelved`, a closed one a newer box
/// would list.
fn statuses(newer: bool, shelved: bool) -> Value {
    let mut statuses = vec![
        json!({ "id": "needs-you", "name": "Needs you", "closed": false }),
        json!({ "id": "in-review", "name": "In review", "closed": false }),
        json!({ "id": "in-progress", "name": "In progress", "closed": false }),
        json!({ "id": "waiting", "name": "Waiting", "closed": false }),
        json!({ "id": "todo", "name": "Todo", "closed": false }),
        json!({ "id": "backlog", "name": "Backlog", "closed": false }),
        json!({ "id": "done", "name": "Done", "closed": true }),
        json!({ "id": "canceled", "name": "Canceled", "closed": true }),
    ];
    if newer {
        statuses.insert(4, json!({ "id": "blocked", "name": "Blocked", "closed": false }));
    }
    if shelved {
        statuses.push(json!({ "id": "shelved", "name": "Shelved", "closed": true }));
    }
    json!(statuses)
}

/// The conversation a bot of this box keeps its queue in.
fn bot_session(assignee: &Value) -> Option<&'static str> {
    match (assignee["kind"].as_str(), assignee["id"].as_str()) {
        (Some("bot"), Some("bot_mira")) => Some("ses_mira"),
        (Some("bot"), Some("bot_scout")) => Some("ses_scout"),
        _ => None,
    }
}

fn priorities() -> Value {
    json!([{ "id": "urgent", "name": "Urgent" }, { "id": "high", "name": "High" }, { "id": "normal", "name": "Normal" }, { "id": "low", "name": "Low" }])
}

/// Who an item may be for: the box's bots first, each with its look, then the coding agents.
fn assignees() -> Value {
    json!([
        { "kind": "bot", "id": "bot_mira", "name": "Mira", "look": { "color": "sky", "shape": "round", "face": "calm" } },
        { "kind": "bot", "id": "bot_scout", "name": "Scout", "look": { "color": "amber", "shape": "gem", "face": "calm" } },
        { "kind": "agent", "id": "claude-code", "name": "Claude Code" },
        { "kind": "agent", "id": "codex-cli", "name": "Codex" },
        { "kind": "agent", "id": "cursor", "name": "Cursor" },
    ])
}

/// One of a list by its id or its name, any case: its id.
fn named(list: &Value, said: &str) -> Option<String> {
    list.as_array()?.iter().find(|each| each["id"] == json!(said) || each["name"].as_str().is_some_and(|name| name.eq_ignore_ascii_case(said.trim()))).and_then(|each| each["id"].as_str().map(str::to_owned))
}

fn closed(status: &str) -> bool {
    matches!(status, "done" | "canceled" | "shelved")
}

fn refuse(message: impl Into<String>) -> (u16, Value) {
    (400, json!({ "message": message.into() }))
}

/// A view's filters, as a box takes them from its route.
#[derive(Default)]
struct Filters {
    computers: Option<Vec<String>>,
    sources: Option<Vec<String>>,
    owners: Option<Vec<String>>,
    labels: Option<Vec<String>>,
}

impl Filters {
    /// Whether an item is one of those it leaves: any one of each kind asked for.
    fn keeps(&self, item: &Value) -> bool {
        let among = |list: &Option<Vec<String>>, value: &str| list.as_ref().is_none_or(|list| list.iter().any(|each| each == value));
        let labels: Vec<&str> = item["labels"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        among(&self.computers, item["computer"]["key"].as_str().unwrap_or(""))
            && among(&self.sources, item["source"]["kind"].as_str().unwrap_or("none"))
            && among(&self.owners, item["owner"].as_str().unwrap_or_default())
            && self.labels.as_ref().is_none_or(|list| if labels.is_empty() { list.iter().any(|each| each == "none") } else { labels.iter().any(|label| list.iter().any(|each| each == label)) })
    }

    fn said(&self) -> Option<Value> {
        let mut said = serde_json::Map::new();
        for (name, list) in [("computers", &self.computers), ("sources", &self.sources), ("owners", &self.owners), ("labels", &self.labels)] {
            if let Some(list) = list {
                said.insert(name.into(), json!(list));
            }
        }
        (!said.is_empty()).then_some(Value::Object(said))
    }
}

impl Queue {
    /// A box with no items yet, and no labels: none is built in.
    pub fn none() -> Queue {
        Queue { next_number: 1, next_label: 1, ..Queue::default() }
    }

    /// The sample queue, as it'd be at `now`.
    pub fn sample(now: f64) -> Queue {
        let mut queue = Queue { next_number: 263, next_label: 1, ..Queue::default() };
        queue.labels = vec![
            json!({ "id": "lbl_bug", "name": "Bug", "color": "coral" }),
            json!({ "id": "lbl_design", "name": "Design", "color": "violet" }),
            json!({ "id": "lbl_docs", "name": "Docs", "color": "sky" }),
            json!({ "id": "lbl_api", "name": "API", "color": "mint" }),
            json!({ "id": "lbl_launch", "name": "Launch", "color": "amber" }),
        ];
        let user = json!({ "user": true });
        let mira = json!({ "session": "ses_mira", "name": "Mira" });
        let mut number = 230;
        let mut add = |queue: &mut Queue, owner: &str, title: &str, status: &str, age: f64, change: &dyn Fn(&mut Value)| {
            number += 1;
            let mut item = json!({
                "id": format!("wqi_{number}"), "number": number, "owner": owner, "title": title, "status": status, "order": number, "priority": "normal", "labels": [],
                "sessions": [], "created": { "at": now - age - 2.0 * DAY, "by": user }, "updated": { "at": now - age, "by": mira },
                "body": "", "notes": [], "events": [], "files": [], "links": [], "handed": [],
            });
            change(&mut item);
            if closed(status) && item.get("doneAt").is_none() {
                item["doneAt"] = json!(now - age);
            }
            queue.items.push(item);
        };
        let plain = |_: &mut Value| {};
        let day = |offset: i64| time::day(now, 0, offset);

        // 231: at work, with everything an item's page shows.
        add(&mut queue, "ses_mira", "Keep what was typed when the sign-up email is taken", "in-progress", 20.0 * MINUTE, &|item| {
            item["priority"] = json!("high");
            item["labels"] = json!(["lbl_bug"]);
            item["assignee"] = json!({ "kind": "bot", "id": "bot_mira", "name": "Mira", "look": { "color": "sky", "shape": "round", "face": "calm" } });
            item["due"] = json!(day(2));
            item["progress"] = json!({ "value": 0.6, "note": "Writing the test for the password field" });
            item["sessions"] = json!([
                { "session": "ses_signup", "title": "Fix the sign-up form wiping on a taken email", "working": true },
                { "session": "ses_cms", "title": "Move the blog to the new CMS", "working": false },
            ]);
            item["body"] = json!("When the email is already taken, the form comes back **empty**. People retype everything, or leave.\n\n## What it should do\n\n- Keep every field as it was typed\n- Say what's wrong under the email field, in the new tone\n- Put the keyboard back in that field\n\n## How to check\n\nSign up twice with the same address:\n\n```sh\nnpm run test -- signup\n```\n\nThe copy is in the [tone guide](https://example.com/tone-guide).");
            item["notes"] = json!([
                { "at": now - 5.0 * HOUR, "by": { "user": true }, "text": "The design review is on Thursday: the error copy should match the new tone guide." },
                { "at": now - 2.0 * HOUR, "by": { "session": "ses_signup", "name": "Mira" }, "text": "The email field keeps its value now. The password field is cleared by the browser on purpose; I kept that and say so under it." },
                { "at": now - 25.0 * MINUTE, "by": { "session": "ses_signup", "name": "Mira" }, "text": "`signup.test.ts` covers the taken email. Writing the one for the password field." },
            ]);
            item["files"] = json!([
                { "name": "before.png", "size": 184_320, "type": "image/png", "at": now - 5.0 * HOUR },
                { "name": "tone-guide.pdf", "size": 1_258_291, "type": "application/pdf", "at": now - 5.0 * HOUR },
            ]);
            item["links"] = json!([
                { "url": "https://github.com/acme/website/pull/482", "title": "Keep the sign-up form's fields on a taken email", "kind": "github-pr" },
                { "url": "https://example.com/tone-guide", "title": "Tone guide" },
            ]);
        });
        // 232: only you can move it on.
        add(&mut queue, "ses_limits", "Rate limits for the public API", "needs-you", 50.0 * MINUTE, &|item| {
            item["priority"] = json!("urgent");
            item["labels"] = json!(["lbl_api"]);
            item["assignee"] = json!({ "kind": "agent", "id": "claude-code", "name": "Claude Code" });
            item["progress"] = json!({ "note": "Waiting on you to allow a command" });
            item["sessions"] = json!([{ "session": "ses_limits", "title": "Rate limits for the API", "working": true }]);
            item["body"] = json!("60 requests a minute for a key, 600 for a paid one. Answer `429` with `Retry-After`.");
        });
        // 233: an issue, open on GitHub.
        add(&mut queue, "ses_mira", "The pricing page's table breaks under 400 px", "todo", 3.0 * HOUR, &|item| {
            item["labels"] = json!(["lbl_bug", "lbl_design"]);
            item["source"] = json!({ "kind": "github", "ref": "gh:acme/website#318", "url": "https://github.com/acme/website/issues/318", "host": "github.com", "name": "website#318" });
            item["seen"] = json!({ "state": "open", "at": now - 2.0 * DAY });
            item["closesOn"] = json!("github");
            item["body"] = json!("The third column is cut off on small phones.");
        });
        add(&mut queue, "ses_mira", "Dark mode for the settings page", "todo", 5.0 * HOUR, &plain);
        // 235: a long title, more labels than a row shows, and a day gone by.
        add(&mut queue, "grp_launch", "Write the launch post, with the three screenshots and the short video from the design review", "in-review", 9.0 * HOUR, &|item| {
            item["priority"] = json!("high");
            item["labels"] = json!(["lbl_docs", "lbl_launch", "lbl_design"]);
            item["assignee"] = json!({ "kind": "bot", "id": "bot_scout", "name": "Scout", "look": { "color": "amber", "shape": "gem", "face": "calm" } });
            item["due"] = json!(day(-1));
        });
        add(&mut queue, "ses_wren", "Nightly builds for the Linux packages", "todo", 2.0 * HOUR, &|item| {
            item["priority"] = json!("low");
            item["computer"] = there(DEVBOX);
            // (For a bot of that computer: said by name, with no look from this box.)
            item["assignee"] = json!({ "kind": "bot", "id": "bot_wren", "name": "Wren" });
        });

        // 237 to 241.
        for (ix, title) in ["Get the Lighthouse score above 90", "Move the blog to the new CMS", "Search across the docs", "An RSS feed for the changelog", "Passkeys on sign-in"].iter().enumerate() {
            add(&mut queue, "ses_mira", title, "backlog", 2.0 * DAY, &|item| {
                if ix == 2 {
                    item["labels"] = json!(["lbl_docs"]);
                }
                if ix == 4 {
                    // (On its way to a conversation on another computer.)
                    item["handing"] = json!({ "to": "ses_wren", "name": "Wren" });
                }
                if ix == 3 {
                    // (Said only in what it says: a search finds it, its title doesn't.)
                    item["body"] = json!("Atom, with one entry for each release. Link it from the docs footer.");
                }
            });
        }
        // 242: on a computer that isn't answering.
        add(&mut queue, "ses_scout", "Watch the checks on the release branch", "waiting", 3.0 * DAY, &|item| item["computer"] = there(STUDIO));

        // 243 to 245.
        for title in ["A calmer empty state for the dashboard", "New icons for the mobile tab bar", "The onboarding's second step, simpler"] {
            add(&mut queue, "grp_design", title, "todo", 4.0 * DAY, &|item| item["labels"] = json!(["lbl_design"]));
        }
        // 246 to 249.
        for (title, status) in [("Offline mode for the mobile app", "backlog"), ("A plugin API", "backlog"), ("Self-hosted relay", "canceled"), ("Import from the old wiki", "canceled")] {
            add(&mut queue, "ses_mira", title, status, 12.0 * DAY, &|item| {
                // 247: a pull request (its kind is its own: `github-pr`).
                if title == "A plugin API" {
                    item["source"] = json!({ "kind": "github-pr", "ref": "gh:acme/website#482", "url": "https://github.com/acme/website/pull/482", "host": "github.com", "name": "website#482" });
                    item["seen"] = json!({ "state": "draft", "at": now - 2.0 * DAY });
                    item["closesOn"] = json!("github");
                }
            });
        }
        // 250: an issue GitHub closed.
        add(&mut queue, "ses_mira", "The footer links 404 on the docs site", "done", 6.0 * HOUR, &|item| {
            item["source"] = json!({ "kind": "github", "ref": "gh:acme/website#301", "url": "https://github.com/acme/website/issues/301", "host": "github.com", "name": "website#301" });
            item["seen"] = json!({ "state": "completed", "at": now - 6.0 * HOUR });
            item["closesOn"] = json!("github");
            item["keptIn"] = json!("todo");
            item["doneAs"] = json!("Closed on GitHub");
        });
        // 251 to 261.
        for (ix, title) in ["Cache the avatars", "A sitemap for the docs", "Two-factor codes by mail", "Faster cold start on mobile", "Tidy the CI cache", "Weekly usage mail", "Retire the old API keys", "The status page", "Invoices as PDF", "Team invites by link", "A dark favicon"].iter().enumerate() {
            add(&mut queue, "ses_mira", title, "done", (1.0 + ix as f64) * DAY, &|item| {
                // 251: done after its day; nothing closed is overdue.
                if ix == 0 {
                    item["due"] = json!(day(-3));
                }
            });
        }
        // 262: deleted.
        add(&mut queue, "ses_mira", "A carousel on the home page", "backlog", 2.0 * DAY, &|item| item["due"] = json!(day(-5)));
        // 228 and 229: on a computer that runs an older Kuvo Code, and on a machine shared outside
        // your setup. Neither takes a change from here.
        for (number, title, computer) in [(228, "Rotate the signing certificate", OLD), (229, "Clean up the shared build cache", SHARED)] {
            queue.items.push(json!({
                "id": format!("wqi_{number}"), "number": number, "owner": "ses_scout", "title": title, "status": "waiting", "order": 242.0 + number as f64 / 1000.0, "priority": "normal", "labels": [],
                "sessions": [], "created": { "at": now - 8.0 * DAY, "by": user }, "updated": { "at": now - 6.0 * DAY, "by": mira }, "computer": there(computer),
                "body": "", "notes": [], "events": [], "files": [], "links": [], "handed": [],
            }));
        }
        let mut gone = queue.items.remove(queue.items.len() - 3);
        gone["removed"] = json!({ "at": now - DAY, "by": { "user": true } });
        queue.removed.push(gone);
        queue
    }

    /// It lists a status a newer box would (Blocked), with an item in it.
    pub fn add_newer_status(&mut self, now: f64) {
        self.newer_status = true;
        self.items.push(json!({
            "id": "wqi_227", "number": 227, "owner": "ses_mira", "title": "Wait for the vendor's new API key", "status": "blocked", "order": 1, "priority": "normal", "labels": [],
            "sessions": [], "created": { "at": now - 3.0 * DAY, "by": { "user": true } }, "updated": { "at": now - DAY, "by": { "user": true } },
            "body": "", "notes": [], "events": [], "files": [], "links": [], "handed": [],
        }));
    }

    /// It lists a closed status (Shelved) and a label colour (teal) a newer box would, with an item
    /// in Shelved.
    pub fn add_newer_closed(&mut self, now: f64) {
        self.newer_closed = true;
        self.items.push(json!({
            "id": "wqi_225", "number": 225, "owner": "ses_mira", "title": "Rework the onboarding emails", "status": "shelved", "order": 1, "priority": "normal", "labels": [],
            "sessions": [], "created": { "at": now - 3.0 * DAY, "by": { "user": true } }, "updated": { "at": now - DAY, "by": { "user": true } }, "doneAt": now - DAY,
            "body": "", "notes": [], "events": [], "files": [], "links": [], "handed": [],
        }));
    }

    /// The colours a label may be, in the order a picker offers them.
    fn colors(&self) -> Vec<&'static str> {
        let mut colors = COLORS.to_vec();
        if self.newer_closed {
            colors.push("teal");
        }
        colors
    }

    /// One of its items stands in a status its view doesn't list (kept as it came from a newer box).
    pub fn add_unlisted_status(&mut self, now: f64) {
        self.items.push(json!({
            "id": "wqi_226", "number": 226, "owner": "ses_mira", "title": "Sort the support inbox", "status": "triage", "order": 1, "priority": "normal", "labels": [],
            "sessions": [], "created": { "at": now - 3.0 * DAY, "by": { "user": true } }, "updated": { "at": now - DAY, "by": { "user": true } },
            "body": "", "notes": [], "events": [], "files": [], "links": [], "handed": [],
        }));
    }

    /// The sample queue with many more labels than a menu shows at once.
    pub fn add_many_labels(&mut self) {
        for (ix, name) in ["Accessibility", "Billing", "Performance", "Security", "Onboarding", "Mobile", "Infrastructure", "Research", "Support", "Growth", "Compliance", "Localisation", "Analytics", "Editor"].iter().enumerate() {
            self.labels.push(json!({ "id": format!("lbl_m{ix}"), "name": name, "color": COLORS[ix % COLORS.len()] }));
        }
    }

    /// The feature names a box with this queue says.
    pub fn features(&self) -> Vec<&'static str> {
        vec!["work-queue", "work-queue.elsewhere", "work-queue.forward"]
    }

    /// Its answer to `method path` (a route, with its query), if it's one of the Work Queue's.
    pub fn answer(&mut self, method: &str, path: &str, body: &Value, now: f64) -> Option<(u16, Value)> {
        let route = path.split('?').next().unwrap_or(path);
        let segments: Vec<&str> = route.split('/').collect();
        let ["kuvo", "work-queue", rest @ ..] = segments.as_slice() else { return None };
        self.at = now;
        Some(match (method, rest) {
            ("GET", []) if self.down => (503, json!({ "message": "Kuvo on this computer isn’t answering." })),
            ("GET", []) => {
                let words = super::query(path, "q").map(super::decoded).map(|q| q.replace('+', " ")).filter(|q| !q.trim().is_empty());
                // Its filters, each a list separated by commas (an empty name is this box, among
                // computers; nothing, among the rest).
                let list = |name: &str| super::query(path, name).map(|said| said.split(',').map(super::decoded).map(|each| each.trim().to_owned()).filter(|each| !each.is_empty() || name == "computers").collect::<Vec<_>>());
                let filters = Filters { computers: list("computers"), sources: list("sources"), owners: list("owners"), labels: list("labels") };
                (200, self.view(words.as_deref(), &filters, now))
            }
            ("GET", ["labels"]) => (200, json!({ "labels": self.labels })),
            ("POST", ["labels"]) => self.change_labels(body),
            ("POST", ["items"]) => self.add(body, now),
            ("GET", ["items", id]) => match self.find(id) {
                Some(item) => (200, json!({ "item": whole(item, &self.today()) })),
                None => (404, json!({ "message": "There’s no such item." })),
            },
            ("PATCH", ["items", id]) => self.change(id, body, now),
            ("POST", ["items", id, "restore"]) => self.restore(id, now),
            ("DELETE", ["items", id]) => self.delete(id, now),
            _ => (404, json!({ "message": "Not found." })),
        })
    }

    fn find(&self, id: &str) -> Option<&Value> {
        self.items.iter().chain(&self.removed).find(|item| item["id"] == json!(id))
    }

    /// Today, as this box writes a day.
    fn today(&self) -> String {
        self.today_said.clone().unwrap_or_else(|| time::day(self.at, 0, 0))
    }

    /// Why an item on another computer can't be changed now, if it can't.
    fn away(item: &Value) -> Option<(u16, Value)> {
        let key = item["computer"]["key"].as_str().unwrap_or_default();
        if key == STUDIO.0 {
            return Some((503, json!({ "message": "studio-mini isn’t answering. Nothing was changed.", "computer": item["computer"] })));
        }
        // (One that doesn't take it from another box: changed there.)
        (key == OLD.0 || key == SHARED.0).then(|| (409, json!({ "message": can(item)["why"], "computer": item["computer"] })))
    }

    /// An item as a change answers it: a box answers a change with no word of its sessions (each
    /// "New Session", none at work), which the view says again when it's next read.
    fn answered(&self, item: &Value) -> Value {
        let mut said = summary(item, &self.today());
        for session in said["sessions"].as_array_mut().into_iter().flatten() {
            session["title"] = json!("New Session");
            session["working"] = json!(false);
        }
        said
    }

    /// Whether every word is in what an item says: its title, its `#n` or its issue's name, its
    /// labels' names, its description and its notes, any case.
    fn has(&self, item: &Value, words: &str) -> bool {
        let mut said = vec![item["title"].as_str().unwrap_or_default().to_lowercase(), item["body"].as_str().unwrap_or_default().to_lowercase()];
        said.push(match item["source"]["name"].as_str() {
            Some(name) => name.to_lowercase(),
            None => item["number"].as_u64().map(|number| format!("#{number}")).unwrap_or_default(),
        });
        for label in item["labels"].as_array().into_iter().flatten() {
            said.extend(self.labels.iter().filter(|each| &each["id"] == label).filter_map(|each| each["name"].as_str()).map(str::to_lowercase));
        }
        said.extend(item["notes"].as_array().into_iter().flatten().filter_map(|note| note["text"].as_str()).map(str::to_lowercase));
        words.to_lowercase().split_whitespace().all(|word| said.iter().any(|said| said.contains(word)))
    }

    /// The view of every queue: the items in the statuses' order, then each one's own; with `q`,
    /// only those its words are in, and the counts of those.
    fn view(&self, q: Option<&str>, filters: &Filters, now: f64) -> Value {
        let statuses = statuses(self.newer_status, self.newer_closed);
        let place = |item: &Value| statuses.as_array().and_then(|statuses| statuses.iter().position(|status| status["id"] == item["status"])).unwrap_or(usize::MAX);
        let takes = |item: &&Value| q.is_none_or(|q| self.has(item, q)) && filters.keeps(item);
        let mut items: Vec<&Value> = self.items.iter().filter(takes).collect();
        items.sort_by(|a, b| place(a).cmp(&place(b)).then(a["order"].as_f64().unwrap_or(0.).total_cmp(&b["order"].as_f64().unwrap_or(0.))));
        let removed: Vec<&Value> = self.removed.iter().filter(takes).collect();
        let count = |list: &Value, of: &dyn Fn(&Value, &Value) -> bool| {
            let mut counts = serde_json::Map::new();
            for each in list.as_array().into_iter().flatten() {
                counts.insert(each["id"].as_str().unwrap_or_default().to_owned(), json!(items.iter().filter(|item| of(item, &each["id"])).count()));
            }
            counts
        };
        let mut by_status = count(&statuses, &|item, id| &item["status"] == id);
        for (status, counted) in &self.counted {
            by_status.insert(status.clone(), json!(counted));
        }
        let on = |key: &str| items.iter().filter(|item| item["computer"]["key"].as_str().unwrap_or("") == key).count();
        // From each outside source, by its kind: an issue's and a pull request's are two.
        let mut sources = serde_json::Map::new();
        for kind in items.iter().filter_map(|item| item["source"]["kind"].as_str()) {
            let counted = sources.get(kind).and_then(Value::as_u64).unwrap_or(0);
            sources.insert(kind.to_owned(), json!(counted + 1));
        }
        let today = self.today();
        let counts = json!({
            "open": items.iter().filter(|item| !closed(item["status"].as_str().unwrap_or_default())).count(),
            "statuses": by_status,
            "priorities": count(&priorities(), &|item, id| &item["priority"] == id),
            "labels": count(&json!(self.labels), &|item, id| item["labels"].as_array().is_some_and(|labels| labels.contains(id))),
            "sources": sources,
            "computers": { "": on(""), DEVBOX.0: on(DEVBOX.0), STUDIO.0: on(STUDIO.0), OLD.0: on(OLD.0), SHARED.0: on(SHARED.0) },
            "removed": removed.len(),
        });
        let computers = if self.items.iter().any(|item| item.get("computer").is_some()) {
            json!([
                { "key": DEVBOX.0, "name": DEVBOX.1, "kind": DEVBOX.2, "id": "devbox", "at": now - 2.0 * MINUTE },
                { "key": STUDIO.0, "name": STUDIO.1, "kind": STUDIO.2, "at": now - 3.0 * HOUR, "away": now - 2.0 * HOUR },
                { "key": OLD.0, "name": OLD.1, "kind": OLD.2, "at": now - 5.0 * MINUTE },
                { "key": SHARED.0, "name": SHARED.1, "kind": SHARED.2, "id": "shared-box", "at": now - 4.0 * MINUTE },
            ])
        } else {
            json!([])
        };
        let mut view = json!({
            "owner": "all",
            "statuses": statuses,
            "priorities": priorities(),
            "labels": self.labels,
            "colors": self.colors(),
            "assignees": assignees(),
            // What an item added with nothing said starts as.
            "new": { "status": self.starts().0, "priority": self.starts().1 },
            // The day it is on this box: what a client counts every day from.
            "today": today,
            "items": items.into_iter().map(|item| summary(item, &today)).collect::<Vec<_>>(),
            // (Each owner in the view: those with an item in it.)
            "owners": owners().as_array().map(|owners| owners.iter().filter(|owner| self.items.iter().chain(&self.removed).any(|item| item["owner"] == owner["owner"])).cloned().collect::<Vec<_>>()).unwrap_or_default(),
            "removed": removed.into_iter().map(|item| summary(item, &today)).collect::<Vec<_>>(),
            "computers": computers,
            "counts": counts,
        });
        if let Some(q) = q {
            view["q"] = json!(q);
        }
        if let Some(filters) = filters.said() {
            view["filters"] = filters;
        }
        view
    }

    fn starts(&self) -> (String, String) {
        self.starts.clone().unwrap_or_else(|| ("todo".into(), "normal".into()))
    }

    fn last_order(&self) -> f64 {
        self.items.iter().filter_map(|item| item["order"].as_f64()).fold(0., f64::max)
    }

    /// The labels a body names (by id or name), as their ids; one that isn't a label is refused.
    fn labels_named(&self, said: &Value) -> Result<Vec<Value>, (u16, Value)> {
        let Some(said) = said.as_array() else { return Err(refuse("Labels are a list of their names.")) };
        if said.len() > 10 {
            return Err(refuse("An item has 10 labels at most."));
        }
        let mut ids: Vec<Value> = Vec::new();
        for label in said {
            let name = label.as_str().unwrap_or_default();
            match named(&json!(self.labels), name) {
                Some(id) if !ids.contains(&json!(id)) => ids.push(json!(id)),
                Some(_) => {}
                None => return Err(refuse(format!("There’s no label called {name}."))),
            }
        }
        Ok(ids)
    }

    /// Who a body says an item is for, as an item names it.
    fn assignee_named(said: &Value) -> Result<Value, (u16, Value)> {
        let list = assignees();
        let found = list.as_array().into_iter().flatten().find(|each| each["kind"] == said["kind"] && each["id"] == said["id"]);
        // (A bot of this box is named with its look.)
        found
            .map(|each| {
                let mut named = json!({ "kind": each["kind"], "id": each["id"], "name": each["name"] });
                if each.get("look").is_some() {
                    named["look"] = each["look"].clone();
                }
                named
            })
            .ok_or_else(|| refuse("There’s no such bot or coding agent on this computer."))
    }

    fn due_said(said: &Value) -> Result<String, (u16, Value)> {
        said.as_str().map(str::trim).filter(|day| time::parse_day(day).is_some()).map(str::to_owned).ok_or_else(|| refuse("A due day is written as a day: 2026-11-30."))
    }

    fn delete(&mut self, id: &str, now: f64) -> (u16, Value) {
        let Some(at) = self.items.iter().position(|item| item["id"] == json!(id)) else {
            return (404, json!({ "message": "There’s no such item." }));
        };
        if let Some(refusal) = Queue::away(&self.items[at]) {
            return refusal;
        }
        let mut item = self.items.remove(at);
        item["removed"] = json!({ "at": now, "by": { "user": true } });
        item["sessions"] = json!([]);
        let said = self.answered(&item);
        self.removed.insert(0, item);
        (200, json!({ "item": said }))
    }

    fn restore(&mut self, id: &str, now: f64) -> (u16, Value) {
        let Some(at) = self.removed.iter().position(|item| item["id"] == json!(id)) else {
            return (404, json!({ "message": "There’s no such deleted item." }));
        };
        let mut item = self.removed.remove(at);
        if let Some(fields) = item.as_object_mut() {
            fields.remove("removed");
        }
        item["updated"] = json!({ "at": now, "by": { "user": true } });
        let said = self.answered(&item);
        self.items.push(item);
        (200, json!({ "item": said }))
    }

    fn add(&mut self, body: &Value, now: f64) -> (u16, Value) {
        let title: String = body["title"].as_str().unwrap_or_default().trim().lines().next().unwrap_or_default().chars().take(200).collect();
        if title.is_empty() {
            return refuse("An item needs a title.");
        }
        let status = match body["status"].as_str() {
            Some(said) => match named(&statuses(self.newer_status, self.newer_closed), said) {
                Some(status) => status,
                None => return refuse(format!("There’s no status called {said}.")),
            },
            None => self.starts().0,
        };
        let priority = match body["priority"].as_str() {
            Some(said) => match named(&priorities(), said) {
                Some(priority) => priority,
                None => return refuse(format!("There’s no priority called {said}.")),
            },
            None => self.starts().1,
        };
        let labels = match body.get("labels").map(|labels| self.labels_named(labels)) {
            Some(Ok(labels)) => labels,
            Some(Err(refusal)) => return refusal,
            None => Vec::new(),
        };
        let number = self.next_number;
        self.next_number += 1;
        let mut item = json!({
            "id": format!("wqi_{number}"), "number": number, "owner": "usr_you", "title": title, "status": status, "order": self.last_order() + 1., "priority": priority, "labels": labels,
            "sessions": [], "created": { "at": now, "by": { "user": true } }, "updated": { "at": now, "by": { "user": true } },
            "body": body["body"].as_str().unwrap_or_default(), "notes": [], "events": [], "files": [], "links": [], "handed": [],
        });
        if let Some(said) = body.get("assignee").filter(|said| !said.is_null()) {
            match Queue::assignee_named(said) {
                Ok(assignee) => {
                    // For a bot of this box, it goes in that bot's queue.
                    if let Some(session) = bot_session(&assignee) {
                        item["owner"] = json!(session);
                    }
                    item["assignee"] = assignee;
                }
                Err(refusal) => return refusal,
            }
        }
        if let Some(said) = body.get("due").filter(|said| !said.is_null()) {
            match Queue::due_said(said) {
                Ok(due) => item["due"] = json!(due),
                Err(refusal) => return refusal,
            }
        }
        if closed(&status) {
            item["doneAt"] = json!(now);
        }
        let said = self.answered(&item);
        self.items.push(item);
        (200, json!({ "item": said }))
    }

    /// A change to an item: only what's sent changes, all of it or none.
    fn change(&mut self, id: &str, body: &Value, now: f64) -> (u16, Value) {
        let order = self.last_order() + 1.;
        let Some(at) = self.items.iter().position(|item| item["id"] == json!(id)) else {
            return (404, json!({ "message": "There’s no such item." }));
        };
        let mut item = self.items[at].clone();
        if let Some(refusal) = Queue::away(&item) {
            return refusal;
        }
        if item.get("handing").is_some() {
            return refuse("It’s on its way to another computer: it can be changed once it’s there.");
        }
        let issue = item.get("source").is_some();
        if issue && (body.get("title").is_some() || body.get("body").is_some()) {
            return refuse("Its title and description are GitHub’s: change them there.");
        }
        if let Some(title) = body["title"].as_str() {
            let title: String = title.trim().lines().next().unwrap_or_default().chars().take(200).collect();
            if title.is_empty() {
                return refuse("An item needs a title.");
            }
            item["title"] = json!(title);
        }
        if let Some(text) = body["body"].as_str() {
            item["body"] = json!(text);
        }
        if let Some(said) = body["status"].as_str() {
            let Some(status) = named(&statuses(self.newer_status, self.newer_closed), said) else { return refuse(format!("There’s no status called {said}.")) };
            let was = item["status"].as_str().unwrap_or_default().to_owned();
            if issue && (closed(&status) || closed(&was)) {
                return refuse("This one is closed when its issue is closed on GitHub.");
            }
            if was != status {
                item["status"] = json!(status);
                item["order"] = json!(order);
                if closed(&status) {
                    // Closing it unlinks everyone.
                    item["doneAt"] = json!(now);
                    item["sessions"] = json!([]);
                } else if let Some(fields) = item.as_object_mut() {
                    fields.remove("doneAt");
                }
            }
        }
        if let Some(said) = body["priority"].as_str() {
            let Some(priority) = named(&priorities(), said) else { return refuse(format!("There’s no priority called {said}.")) };
            item["priority"] = json!(priority);
        }
        if let Some(said) = body.get("labels") {
            match self.labels_named(said) {
                Ok(labels) => item["labels"] = json!(labels),
                Err(refusal) => return refusal,
            }
        }
        if let Some(said) = body.get("assignee") {
            if said.is_null() {
                item.as_object_mut().map(|fields| fields.remove("assignee"));
            } else {
                match Queue::assignee_named(said) {
                    Ok(assignee) => {
                        // One of your own, assigned to a bot of this box, goes into that bot's
                        // queue: a hand-off.
                        if let (true, Some(session)) = (item["owner"] == json!("usr_you"), bot_session(&assignee)) {
                            item["owner"] = json!(session);
                            item["handedBy"] = json!("usr_you");
                        }
                        item["assignee"] = assignee;
                    }
                    Err(refusal) => return refusal,
                }
            }
        }
        if let Some(said) = body.get("due") {
            if said.is_null() {
                item.as_object_mut().map(|fields| fields.remove("due"));
            } else {
                match Queue::due_said(said) {
                    Ok(due) => item["due"] = json!(due),
                    Err(refusal) => return refusal,
                }
            }
        }
        item["updated"] = json!({ "at": now, "by": { "user": true } });
        let said = self.answered(&item);
        self.items[at] = item;
        (200, json!({ "item": said }))
    }

    fn change_labels(&mut self, body: &Value) -> (u16, Value) {
        let colors = self.colors();
        let color_said = |body: &Value| match body["color"].as_str() {
            Some(color) if colors.contains(&color) => Ok(Some(color.to_owned())),
            Some(color) => Err(refuse(format!("There’s no colour called {color}."))),
            None => Ok(None),
        };
        let taken = |queue: &Queue, name: &str, but: Option<usize>| queue.labels.iter().enumerate().any(|(ix, label)| Some(ix) != but && label["name"].as_str().is_some_and(|each| each.eq_ignore_ascii_case(name)));
        let label;
        if let Some(name) = body["add"].as_str() {
            let name = name.trim();
            if name.is_empty() {
                return refuse("A label needs a name.");
            }
            if taken(self, name, None) {
                return refuse(format!("There’s a label called {name} already."));
            }
            if self.labels.len() >= 60 {
                return refuse("There are 60 labels at most: remove one first.");
            }
            let color = match color_said(body) {
                // (The colour the fewest labels have, unless said.)
                Ok(color) => color.unwrap_or_else(|| self.colors().into_iter().min_by_key(|color| self.labels.iter().filter(|label| label["color"] == json!(color)).count()).unwrap_or("graphite").to_string()),
                Err(refusal) => return refusal,
            };
            let made = json!({ "id": format!("lbl_n{}", self.next_label), "name": name, "color": color });
            self.next_label += 1;
            self.labels.push(made.clone());
            label = Some(made);
        } else if let Some(said) = body["change"].as_str() {
            let Some(at) = named(&json!(self.labels), said).and_then(|id| self.labels.iter().position(|label| label["id"] == json!(id))) else {
                return refuse(format!("There’s no label called {said}."));
            };
            if let Some(name) = body["name"].as_str() {
                let name = name.trim();
                if name.is_empty() {
                    return refuse("A label needs a name.");
                }
                if taken(self, name, Some(at)) {
                    return refuse(format!("There’s a label called {name} already."));
                }
                self.labels[at]["name"] = json!(name);
            }
            match color_said(body) {
                Ok(Some(color)) => self.labels[at]["color"] = json!(color),
                Ok(None) => {}
                Err(refusal) => return refusal,
            }
            label = Some(self.labels[at].clone());
        } else if let Some(said) = body["remove"].as_str() {
            let Some(id) = named(&json!(self.labels), said) else { return refuse(format!("There’s no label called {said}.")) };
            // Off every item first, deleted ones too.
            for item in self.items.iter_mut().chain(&mut self.removed) {
                if let Some(labels) = item["labels"].as_array_mut() {
                    labels.retain(|label| label != &json!(id));
                }
            }
            self.labels.retain(|label| label["id"] != json!(id));
            label = None;
        } else {
            return refuse("Say what to do: add, change or remove.");
        }
        let mut answer = json!({ "labels": self.labels });
        if let Some(label) = label {
            answer["label"] = label;
        }
        (200, answer)
    }
}

fn owners() -> Value {
    json!([
        { "owner": "usr_you", "kind": "you", "title": "Your own" },
        { "owner": "ses_mira", "kind": "bot", "title": "Mira", "bot": { "id": "bot_mira", "name": "Mira" } },
        { "owner": "ses_limits", "kind": "session", "title": "Rate limits for the API" },
        { "owner": "grp_launch", "kind": "group", "title": "Launch crew" },
        { "owner": "grp_design", "kind": "group", "title": "Design review" },
        { "owner": "ses_wren", "kind": "bot", "title": "Wren", "bot": { "id": "bot_wren", "name": "Wren" } },
        { "owner": "ses_scout", "kind": "bot", "title": "Scout", "bot": { "id": "bot_scout", "name": "Scout" } },
    ])
}

/// What may be done to an item, as a box says it to a client: a deleted one is only brought back;
/// nothing on a computer that isn't answering; only a delete while it's on its way elsewhere; an
/// issue's words and its closing are GitHub's, and so is where it stands once GitHub closed it.
fn can(item: &Value) -> Value {
    let none = |why: String| json!({ "edit": false, "set": false, "assign": false, "close": false, "delete": false, "restore": false, "why": why });
    match item["computer"]["key"].as_str() {
        Some(key) if key == STUDIO.0 => return none(format!("{} isn't answering now (it may be asleep, off or offline): it can be changed once it's back.", STUDIO.1)),
        Some(key) if key == SHARED.0 => return none(format!("It's on {}: change it there.", SHARED.1)),
        Some(key) if key == OLD.0 => return none(format!("{} runs an older Kuvo Code: update it to change this from here.", OLD.1)),
        _ => {}
    }
    // (Who an item may be for are this box's bots: one another computer keeps is assigned there.)
    let here = item.get("computer").is_none();
    if item.get("removed").is_some() {
        let mut can = none("It's deleted: bring it back to change it.".into());
        can["restore"] = json!(true);
        return can;
    }
    if let Some(handing) = item.get("handing") {
        let mut can = none(format!("It's on its way to {} on another computer: it can be changed there once it's arrived.", handing["name"].as_str().unwrap_or("a conversation")));
        can["delete"] = json!(true);
        return can;
    }
    if item.get("source").is_some() {
        // (An issue GitHub closed stays Done until it's reopened there.)
        let kept = item.get("keptIn").is_some();
        let mut can = none(if kept { "It's closed on GitHub: it's back where it stood once it's reopened there." } else { "Its title, its description and closing it are GitHub's." }.into());
        can["set"] = json!(!kept);
        can["assign"] = json!(!kept && here);
        can["delete"] = json!(true);
        return can;
    }
    json!({ "edit": true, "set": true, "assign": here, "close": true, "delete": true, "restore": false })
}

/// An item as a list shows it: without what it says, with how many notes, files and links it has
/// and what may be done to it.
fn summary(item: &Value, today: &str) -> Value {
    let mut said = item.clone();
    // Late: its day is before today, and it's still open (not closed, not deleted).
    if item.get("removed").is_none() && !closed(item["status"].as_str().unwrap_or_default()) && item["due"].as_str().is_some_and(|due| due < today) {
        said["overdue"] = json!(true);
    }
    let count = |field: &str| item[field].as_array().map_or(0, Vec::len);
    said["noteCount"] = json!(count("notes"));
    said["fileCount"] = json!(count("files"));
    said["linkCount"] = json!(count("links"));
    said["can"] = can(item);
    if let Some(last) = item["notes"].as_array().and_then(|notes| notes.last()) {
        said["lastNote"] = last.clone();
    }
    if let Some(fields) = said.as_object_mut() {
        for field in ["body", "notes", "events", "files", "links", "handed"] {
            fields.remove(field);
        }
    }
    // (Nothing on a computer that isn't answering can be seen at work now.)
    if item["computer"]["key"] == json!(STUDIO.0)
        && let Some(sessions) = said["sessions"].as_array_mut()
    {
        for session in sessions {
            session["working"] = json!(false);
        }
    }
    said
}

/// An item whole. One on a computer that answers is read there (`told: false`); one on a computer
/// that's away is as it last told it, without what only it keeps.
fn whole(item: &Value, today: &str) -> Value {
    let mut said = summary(item, today);
    let away = item["computer"]["key"] == json!(STUDIO.0);
    if item.get("computer").is_some() {
        said["told"] = json!(away);
    }
    for field in ["body", "notes", "events", "files", "links", "handed"] {
        said[field] = if away { if field == "body" { json!("") } else { json!([]) } } else { item[field].clone() };
    }
    said
}
