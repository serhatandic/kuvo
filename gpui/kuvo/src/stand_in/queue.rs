//! The stand-in box's Work Queue, in a box's own shapes (PROTOCOL.md, "The Work Queue"): the view
//! across your computers with its counts, one item whole, moving one (Done included), deleting it
//! and bringing it back, and its categories (made, renamed, removed). As a box does, it orders,
//! counts and refuses itself: the app draws what it says.
//!
//! It also answers the two routes proposed for a client that writes (`POST kuvo/work-queue/items`,
//! `PATCH kuvo/work-queue/items/<item>`; `work-queue.edit` in its features), which no box has yet.

use serde_json::{Value, json};

use super::{DAY, HOUR, MINUTE};

/// The stand-in's Work Queue.
#[derive(Default)]
pub struct Queue {
    categories: Vec<Value>,
    /// Its items whole, each with its `body`, `notes`, `files` and `links`.
    items: Vec<Value>,
    removed: Vec<Value>,
    next_number: u64,
    /// Its view can't be read now (503).
    pub down: bool,
    /// It takes no item added or rewritten by a client (as every box today): the feature is left
    /// out of its `features`, and the routes aren't there.
    pub read_only: bool,
    /// What it says a category counts, where a test says otherwise than its items do (a box
    /// counts across what a view may not list whole).
    pub counted: Vec<(String, usize)>,
}

const DEVBOX: (&str, &str, &str) = ("key_devbox", "devbox", "machine");
const STUDIO: (&str, &str, &str) = ("key_studio", "studio-mini", "mac");

fn there(computer: (&str, &str, &str)) -> Value {
    json!({ "key": computer.0, "name": computer.1, "kind": computer.2 })
}

impl Queue {
    /// A box with no items yet: its built-in categories only.
    pub fn none() -> Queue {
        Queue { categories: built_in(false), next_number: 1, ..Queue::default() }
    }

    /// The sample queue, as it'd be at `now`.
    pub fn sample(now: f64) -> Queue {
        let mut queue = Queue { categories: built_in(true), next_number: 260, ..Queue::default() };
        let user = json!({ "user": true });
        let mira = json!({ "session": "ses_mira", "name": "Mira" });
        let mut number = 230;
        let mut add = |queue: &mut Queue, owner: &str, title: &str, category: &str, age: f64, change: &dyn Fn(&mut Value)| {
            number += 1;
            let mut item = json!({
                "id": format!("wqi_{number}"), "number": number, "owner": owner, "title": title, "category": category, "order": number,
                "inProgress": false, "sessions": [], "created": { "at": now - age - 2.0 * DAY, "by": user }, "updated": { "at": now - age, "by": mira },
                "body": "", "notes": [], "events": [], "files": [], "links": [], "handed": [],
            });
            change(&mut item);
            if category == "done" && item.get("doneAt").is_none() {
                item["doneAt"] = json!(now - age);
            }
            queue.items.push(item);
        };
        let plain = |_: &mut Value| {};

        // Next: one at work with everything an item's page shows, one at work elsewhere, an issue.
        add(&mut queue, "ses_mira", "Keep what was typed when the sign-up email is taken", "next", 20.0 * MINUTE, &|item| {
            item["inProgress"] = json!(true);
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
        add(&mut queue, "ses_limits", "Rate limits for the public API", "next", 50.0 * MINUTE, &|item| {
            item["inProgress"] = json!(true);
            item["progress"] = json!({ "note": "Waiting on you to allow a command" });
            item["sessions"] = json!([{ "session": "ses_limits", "title": "Rate limits for the API", "working": true }]);
            item["body"] = json!("60 requests a minute for a key, 600 for a paid one. Answer `429` with `Retry-After`.");
        });
        add(&mut queue, "ses_mira", "The pricing page's table breaks under 400 px", "next", 3.0 * HOUR, &|item| {
            item["source"] = json!({ "kind": "github", "ref": "gh:acme/website#318", "url": "https://github.com/acme/website/issues/318", "host": "github.com", "name": "website#318" });
            item["seen"] = json!({ "state": "open", "at": now - 2.0 * DAY });
            item["closesOn"] = json!("github");
            item["body"] = json!("The third column is cut off on small phones.");
        });
        add(&mut queue, "ses_mira", "Dark mode for the settings page", "next", 5.0 * HOUR, &plain);
        add(&mut queue, "grp_launch", "Write the launch post, with the three screenshots and the short video from the design review", "next", 9.0 * HOUR, &plain);
        add(&mut queue, "ses_wren", "Nightly builds for the Linux packages", "next", 2.0 * HOUR, &|item| item["computer"] = there(DEVBOX));

        for title in ["Get the Lighthouse score above 90", "Move the blog to the new CMS", "Search across the docs", "An RSS feed for the changelog", "Passkeys on sign-in"] {
            add(&mut queue, "ses_mira", title, "later", 2.0 * DAY, &plain);
        }
        add(&mut queue, "ses_scout", "Watch the checks on the release branch", "later", 3.0 * DAY, &|item| item["computer"] = there(STUDIO));

        for title in ["A calmer empty state for the dashboard", "New icons for the mobile tab bar", "The onboarding's second step, simpler"] {
            add(&mut queue, "grp_design", title, "cat_design", 4.0 * DAY, &plain);
        }
        for title in ["Offline mode for the mobile app", "A plugin API", "Self-hosted relay", "Import from the old wiki"] {
            add(&mut queue, "ses_mira", title, "parked", 12.0 * DAY, &plain);
        }
        add(&mut queue, "ses_mira", "The footer links 404 on the docs site", "done", 6.0 * HOUR, &|item| {
            item["source"] = json!({ "kind": "github", "ref": "gh:acme/website#301", "url": "https://github.com/acme/website/issues/301", "host": "github.com", "name": "website#301" });
            item["seen"] = json!({ "state": "completed", "at": now - 6.0 * HOUR });
            item["closesOn"] = json!("github");
            item["keptIn"] = json!("next");
            item["doneAs"] = json!("Closed on GitHub");
        });
        for (ix, title) in ["Cache the avatars", "A sitemap for the docs", "Two-factor codes by mail", "Faster cold start on mobile", "Tidy the CI cache", "Weekly usage mail", "Retire the old API keys", "The status page", "Invoices as PDF", "Team invites by link", "A dark favicon"].iter().enumerate() {
            add(&mut queue, "ses_mira", title, "done", (1.0 + ix as f64) * DAY, &plain);
        }
        // Deleted, newest first.
        add(&mut queue, "ses_mira", "A carousel on the home page", "later", 2.0 * DAY, &plain);
        if let Some(mut gone) = queue.items.pop() {
            gone["removed"] = json!({ "at": now - DAY, "by": { "user": true } });
            queue.removed.push(gone);
        }
        queue
    }

    /// The feature names a box with this queue says.
    pub fn features(&self) -> Vec<&'static str> {
        let mut features = vec!["work-queue", "work-queue.counts", "work-queue.elsewhere", "work-queue.forward"];
        if !self.read_only {
            features.push(kuvo_box::work_queue::WRITES);
        }
        features
    }

    /// Its answer to `method route`, if it's one of the Work Queue's.
    pub fn answer(&mut self, method: &str, route: &str, body: &Value, now: f64) -> Option<(u16, Value)> {
        let segments: Vec<&str> = route.split('/').collect();
        let ["kuvo", "work-queue", rest @ ..] = segments.as_slice() else { return None };
        Some(match (method, rest) {
            ("GET", []) if self.down => (503, json!({ "message": "The box isn’t answering." })),
            ("GET", []) => (200, self.view(now)),
            ("GET", ["categories"]) => (200, json!({ "categories": self.categories })),
            ("POST", ["categories"]) => self.change_categories(body),
            ("POST", ["items"]) if !self.read_only => self.add(body, now),
            ("GET", ["items", id]) => match self.find(id) {
                Some(item) => (200, json!({ "item": whole(item) })),
                None => (404, json!({ "message": "There’s no such item." })),
            },
            ("PATCH", ["items", id]) if !self.read_only => self.edit(id, body, now),
            ("POST", ["items", id, "move"]) => self.move_item(id, body, now),
            ("POST", ["items", id, "restore"]) => self.restore(id, now),
            ("DELETE", ["items", id]) => self.delete(id, now),
            _ => (404, json!({ "message": "Not found." })),
        })
    }

    fn find(&self, id: &str) -> Option<&Value> {
        self.items.iter().chain(&self.removed).find(|item| item["id"] == json!(id))
    }

    /// Why an item on another computer can't be changed now, if it can't.
    fn away(item: &Value) -> Option<(u16, Value)> {
        (item["computer"]["key"] == json!(STUDIO.0)).then(|| (503, json!({ "message": "studio-mini isn’t answering. Nothing was changed.", "computer": item["computer"] })))
    }

    /// The view of every queue: the items in the categories' order, then each one's own.
    fn view(&self, _now: f64) -> Value {
        let place = |item: &Value| self.categories.iter().position(|category| category["id"] == item["category"]).unwrap_or(usize::MAX);
        let mut items: Vec<&Value> = self.items.iter().collect();
        items.sort_by(|a, b| place(a).cmp(&place(b)).then(a["order"].as_f64().unwrap_or(0.).total_cmp(&b["order"].as_f64().unwrap_or(0.))));
        let mut categories = serde_json::Map::new();
        for category in &self.categories {
            categories.insert(category["id"].as_str().unwrap_or_default().to_owned(), json!(self.items.iter().filter(|item| item["category"] == category["id"]).count()));
        }
        for (category, count) in &self.counted {
            categories.insert(category.clone(), json!(count));
        }
        let on = |key: &str| self.items.iter().filter(|item| item["computer"]["key"].as_str().unwrap_or("") == key).count();
        let github = self.items.iter().filter(|item| item["source"]["kind"] == json!("github")).count();
        let counts = json!({
            "inProgress": self.items.iter().filter(|item| item["inProgress"] == json!(true)).count(),
            "categories": categories,
            "sources": if github > 0 { json!({ "github": github }) } else { json!({}) },
            "computers": { "": on(""), DEVBOX.0: on(DEVBOX.0), STUDIO.0: on(STUDIO.0) },
            "removed": self.removed.len(),
        });
        let computers = if self.items.iter().any(|item| item.get("computer").is_some()) {
            json!([
                { "key": DEVBOX.0, "name": DEVBOX.1, "kind": DEVBOX.2, "id": "devbox", "at": _now - 2.0 * MINUTE },
                { "key": STUDIO.0, "name": STUDIO.1, "kind": STUDIO.2, "at": _now - 3.0 * HOUR, "away": _now - 2.0 * HOUR },
            ])
        } else {
            json!([])
        };
        json!({
            "owner": "all",
            "categories": self.categories,
            "items": items.into_iter().map(summary).collect::<Vec<_>>(),
            // (Each owner in the view: those with an item in it.)
            "owners": owners().as_array().map(|owners| owners.iter().filter(|owner| self.items.iter().chain(&self.removed).any(|item| item["owner"] == owner["owner"])).cloned().collect::<Vec<_>>()).unwrap_or_default(),
            "removed": self.removed.iter().map(summary).collect::<Vec<_>>(),
            "computers": computers,
            "counts": counts,
        })
    }

    fn category(&self, named: &str) -> Option<String> {
        self.categories.iter().find(|category| category["id"] == json!(named) || category["name"].as_str().is_some_and(|name| name.eq_ignore_ascii_case(named))).and_then(|category| category["id"].as_str().map(str::to_owned))
    }

    fn last_order(&self) -> f64 {
        self.items.iter().filter_map(|item| item["order"].as_f64()).fold(0., f64::max)
    }

    fn move_item(&mut self, id: &str, body: &Value, now: f64) -> (u16, Value) {
        let order = self.last_order() + 1.;
        let Some(to) = body["category"].as_str().and_then(|named| self.category(named)) else {
            return (400, json!({ "message": "There’s no such category." }));
        };
        let Some(item) = self.items.iter_mut().find(|item| item["id"] == json!(id)) else {
            return (404, json!({ "message": "There’s no such item." }));
        };
        if let Some(refusal) = Queue::away(item) {
            return refusal;
        }
        if item["closesOn"] == json!("github") && (to == "done" || item["category"] == json!("done")) {
            return (400, json!({ "message": "This one is done when its issue is closed on GitHub." }));
        }
        if item["category"] != json!(to) {
            item["category"] = json!(to);
            item["order"] = json!(order);
            item["updated"] = json!({ "at": now, "by": { "user": true } });
            if to == "done" {
                // Done unlinks everyone.
                item["doneAt"] = json!(now);
                item["inProgress"] = json!(false);
                item["sessions"] = json!([]);
            } else if let Some(fields) = item.as_object_mut() {
                fields.remove("doneAt");
            }
        }
        (200, json!({ "item": summary(item) }))
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
        item["inProgress"] = json!(false);
        item["sessions"] = json!([]);
        let said = summary(&item);
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
        let said = summary(&item);
        self.items.push(item);
        (200, json!({ "item": said }))
    }

    fn add(&mut self, body: &Value, now: f64) -> (u16, Value) {
        let title = body["title"].as_str().unwrap_or_default().trim().to_owned();
        if title.is_empty() {
            return (400, json!({ "message": "An item needs a title." }));
        }
        let category = match body["category"].as_str() {
            Some(named) => match self.category(named) {
                Some(category) => category,
                None => return (400, json!({ "message": "There’s no such category." })),
            },
            None => self.categories.first().and_then(|category| category["id"].as_str()).unwrap_or("next").to_owned(),
        };
        let number = self.next_number;
        self.next_number += 1;
        let item = json!({
            "id": format!("wqi_{number}"), "number": number, "owner": "ses_yours", "title": title, "category": category, "order": self.last_order() + 1.,
            "inProgress": false, "sessions": [], "created": { "at": now, "by": { "user": true } }, "updated": { "at": now, "by": { "user": true } },
            "body": body["body"].as_str().unwrap_or_default(), "notes": [], "events": [], "files": [], "links": [], "handed": [],
        });
        let said = summary(&item);
        self.items.push(item);
        (200, json!({ "item": said }))
    }

    fn edit(&mut self, id: &str, body: &Value, now: f64) -> (u16, Value) {
        let Some(item) = self.items.iter_mut().find(|item| item["id"] == json!(id)) else {
            return (404, json!({ "message": "There’s no such item." }));
        };
        if let Some(refusal) = Queue::away(item) {
            return refusal;
        }
        if item.get("source").is_some() {
            return (400, json!({ "message": "Its title and description are GitHub’s: change them there." }));
        }
        if let Some(title) = body["title"].as_str() {
            if title.trim().is_empty() {
                return (400, json!({ "message": "An item needs a title." }));
            }
            item["title"] = json!(title.trim());
        }
        if let Some(text) = body["body"].as_str() {
            item["body"] = json!(text);
        }
        item["updated"] = json!({ "at": now, "by": { "user": true } });
        (200, json!({ "item": summary(item) }))
    }

    fn change_categories(&mut self, body: &Value) -> (u16, Value) {
        let custom = |queue: &Queue, named: &str| queue.categories.iter().position(|category| (category["id"] == json!(named) || category["name"] == json!(named)) && category["builtIn"] == json!(false));
        if let Some(name) = body["add"].as_str() {
            let name = name.trim();
            if name.is_empty() {
                return (400, json!({ "message": "A category needs a name." }));
            }
            if self.category(name).is_some() {
                return (400, json!({ "message": format!("There’s a category called {name} already.") }));
            }
            let slug: String = name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
            // Custom ones go between Later and Parked.
            let at = self.categories.iter().position(|category| category["id"] == json!("parked")).unwrap_or(self.categories.len());
            self.categories.insert(at, json!({ "id": format!("cat_{slug}"), "name": name, "builtIn": false }));
        } else if let Some(named) = body["rename"].as_str() {
            let to = body["to"].as_str().unwrap_or_default().trim().to_owned();
            if to.is_empty() {
                return (400, json!({ "message": "A category needs a name." }));
            }
            match custom(self, named) {
                Some(at) => self.categories[at]["name"] = json!(to),
                None => return (400, json!({ "message": "Only a category you made can be renamed." })),
            }
        } else if let Some(named) = body["remove"].as_str() {
            match custom(self, named) {
                Some(at) => {
                    let id = self.categories[at]["id"].clone();
                    if self.items.iter().chain(&self.removed).any(|item| item["category"] == id) {
                        return (400, json!({ "message": "Move its items somewhere else first: deleted ones too." }));
                    }
                    self.categories.remove(at);
                }
                None => return (400, json!({ "message": "Only a category you made can be removed." })),
            }
        } else {
            return (400, json!({ "message": "Say what to do: add, rename or remove." }));
        }
        (200, json!({ "categories": self.categories }))
    }
}

fn built_in(with_custom: bool) -> Vec<Value> {
    let mut categories = vec![json!({ "id": "next", "name": "Next", "builtIn": true }), json!({ "id": "later", "name": "Later", "builtIn": true })];
    if with_custom {
        categories.push(json!({ "id": "cat_design", "name": "Design", "builtIn": false }));
    }
    categories.extend([json!({ "id": "parked", "name": "Parked", "builtIn": true }), json!({ "id": "done", "name": "Done", "builtIn": true })]);
    categories
}

fn owners() -> Value {
    json!([
        { "owner": "ses_mira", "kind": "bot", "title": "Mira", "bot": { "id": "bot_mira", "name": "Mira" } },
        { "owner": "ses_limits", "kind": "session", "title": "Rate limits for the API" },
        { "owner": "grp_launch", "kind": "group", "title": "Launch crew" },
        { "owner": "grp_design", "kind": "group", "title": "Design review" },
        { "owner": "ses_wren", "kind": "bot", "title": "Wren", "bot": { "id": "bot_wren", "name": "Wren" } },
        { "owner": "ses_scout", "kind": "bot", "title": "Scout", "bot": { "id": "bot_scout", "name": "Scout" } },
    ])
}

/// An item as a list shows it: without what it says, with how many notes, files and links it has.
fn summary(item: &Value) -> Value {
    let mut said = item.clone();
    let count = |field: &str| item[field].as_array().map_or(0, Vec::len);
    said["noteCount"] = json!(count("notes"));
    said["fileCount"] = json!(count("files"));
    said["linkCount"] = json!(count("links"));
    if let Some(last) = item["notes"].as_array().and_then(|notes| notes.last()) {
        said["lastNote"] = last.clone();
    }
    if let Some(fields) = said.as_object_mut() {
        for field in ["body", "notes", "events", "files", "links", "handed"] {
            fields.remove(field);
        }
    }
    // (Nothing on a computer that isn't answering can be seen at work now.)
    if item["computer"]["key"] == json!(STUDIO.0) {
        said["inProgress"] = json!(false);
    }
    said
}

/// An item whole. One on a computer that answers is read there (`told: false`); one on a computer
/// that's away is as it last told it, without what only it keeps.
fn whole(item: &Value) -> Value {
    let mut said = summary(item);
    let away = item["computer"]["key"] == json!(STUDIO.0);
    if item.get("computer").is_some() {
        said["told"] = json!(away);
    }
    for field in ["body", "notes", "events", "files", "links", "handed"] {
        said[field] = if away { if field == "body" { json!("") } else { json!([]) } } else { item[field].clone() };
    }
    said
}
