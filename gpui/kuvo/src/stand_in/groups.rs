//! The stand-in box's group chats, in a box's own shapes (PROTOCOL.md, `kuvo/groups`): the chats
//! that live on it with their bots, one chat's messages, who can be in one
//! (`kuvo/groups/joinable`), and writing, stopping, keeping going, marking read, handing out a
//! wrap-up's parts, making, changing and deleting one. As a box's bridge does, it decides what a
//! chat may be (a name, two bots of those it offers) and words its refusals: the app draws what
//! it says. Times are the box's seconds.
//!
//! Its samples: "Launch crew" mid-discussion (three bots, one talking, and a message from a bot
//! that's gone since), "Design review" paused after its turns with a wrap-up (one bot that left
//! it is still a bot here), and "Weekly planning", made and nothing said yet.

use serde_json::{Value, json};

/// One chat as the stand-in keeps it: its bots by id.
#[derive(Clone, Debug)]
struct Chat {
    id: String,
    name: String,
    purpose: Option<String>,
    members: Vec<String>,
    messages: Vec<Value>,
    speaking: Option<String>,
    discussing: bool,
    paused: bool,
    unread: bool,
    activity: Vec<Value>,
    /// The bots whose working part waits on their own conversation, stopped.
    stopped: Vec<String>,
}

/// The stand-in's group chats.
#[derive(Default)]
pub struct Groups {
    chats: Vec<Chat>,
    next: u64,
    /// The chats can't be reached now (503, as with the coding agents' bridge down).
    pub down: bool,
}

/// Who wrote a message, and when (seconds).
fn message(id: &str, author: Option<&str>, at: f64, text: &str) -> Value {
    let mut message = json!({ "id": id, "text": text, "time": at, "kind": "message" });
    if let Some(author) = author {
        message["author"] = json!(author);
    }
    message
}

fn step(id: &str, name: &str, input: Value) -> Value {
    json!({ "type": "tool", "id": id, "name": name, "state": { "status": "completed", "input": input, "content": [] } })
}

impl Groups {
    /// The sample chats, as they'd be at `now` (ms).
    pub fn sample(now: f64) -> Groups {
        let now = now / 1000.0;
        let (minute, hour, day) = (60.0, 3600.0, 86_400.0);
        let mut release = message(
            "gm_l2",
            Some("bot_release"),
            now - 40.0 * minute,
            "Three things are still open:\n\n1. The sign-up fix (#482), in review\n2. The new pricing page copy\n3. The release notes\n\nThe first two should land Thursday. &Docs Keeper owns the notes.",
        );
        release["steps"] = json!([step("st_l2", "shell", json!({ "command": "gh pr list --label 2.4" }))]);
        let mut mira = message(
            "gm_l5",
            Some("bot_mira"),
            now - 12.0 * minute,
            "The sign-up fix is good to go: I ran its tests against staging and all 10 pass.\n\nOne thing before Friday: a taken email still gets “Something went wrong”. It should say the email already has an account.",
        );
        mira["steps"] = json!([step("st_l5a", "read", json!({ "filePath": "/Users/sam/Projects/website/web/src/signup/Form.tsx" })), step("st_l5b", "shell", json!({ "command": "npm test -- signup" }))]);
        let launch = Chat {
            id: "grp_launch".into(),
            name: "Launch crew".into(),
            purpose: Some("Get 2.4 out the door on Friday".into()),
            members: vec!["bot_mira".into(), "bot_release".into(), "bot_docs".into()],
            messages: vec![
                message("gm_l1", None, now - 42.0 * minute, "We ship 2.4 on Friday. &Release Captain, what’s still open? Everyone else, say if anything worries you."),
                release,
                // A bot that's gone since (deleted): the chat keeps what it said.
                message("gm_l3", Some("bot_quill"), now - 30.0 * minute, "I’d hold the pricing page until legal has read the new copy."),
                json!({ "id": "gm_l4", "text": "Quill left the chat.", "time": now - 20.0 * minute, "kind": "note" }),
                mira,
            ],
            speaking: Some("bot_docs".into()),
            discussing: true,
            paused: false,
            unread: true,
            activity: vec![json!({ "kind": "tool", "call": { "type": "tool", "id": "st_l6", "name": "read", "state": { "status": "running", "input": { "filePath": "/Users/sam/Projects/website/CHANGELOG.md" } } } })],
            stopped: Vec::new(),
        };
        let mut wrap = message(
            "gm_d4",
            Some("bot_mira"),
            now - 2.0 * day + 20.0 * minute,
            "We agree on the cards; the FAQ needs answers first. Here’s who does what:",
        );
        wrap["kind"] = json!("wrapUp");
        wrap["tasks"] = json!([
            { "id": "tk_1", "bot": "bot_mira", "task": "Tighten the spacing on the pricing cards", "state": "done", "title": "Done", "started": now - 2.0 * day + 30.0 * minute, "finished": now - 2.0 * day + hour },
            { "id": "tk_2", "bot": "bot_inbox", "task": "Draft answers for the five questions support gets most", "state": "working", "title": "Waiting: Inbox Helper’s conversation is stopped", "started": now - 2.0 * day + 30.0 * minute, "after": [] },
        ]);
        let design = Chat {
            id: "grp_design".into(),
            name: "Design review".into(),
            purpose: None,
            members: vec!["bot_mira".into(), "bot_inbox".into()],
            messages: vec![
                json!({ "id": "gm_d1", "text": "The pricing page, before it ships", "time": now - 2.0 * day, "kind": "meeting" }),
                message("gm_d2", Some("bot_mira"), now - 2.0 * day + 2.0 * minute, "The three cards read well now. The gap between the price and the button is too big on narrow screens."),
                // A bot still here that has left this chat since.
                message("gm_d3", Some("bot_docs"), now - 2.0 * day + 5.0 * minute, "The FAQ under the cards links to two pages that don’t exist yet."),
                wrap,
            ],
            speaking: None,
            discussing: false,
            paused: true,
            unread: false,
            activity: Vec::new(),
            // Inbox Helper's conversation was stopped while it did its part.
            stopped: vec!["bot_inbox".into()],
        };
        let planning = Chat {
            id: "grp_planning".into(),
            name: "Weekly planning".into(),
            purpose: Some("Plan the week on Monday mornings".into()),
            members: vec!["bot_release".into(), "bot_docs".into()],
            messages: Vec::new(),
            speaking: None,
            discussing: false,
            paused: false,
            unread: false,
            activity: Vec::new(),
            stopped: Vec::new(),
        };
        Groups { chats: vec![launch, design, planning], next: 1, down: false }
    }

    /// Its answer to `method route`, if it's one of the group chats' routes, and the chat it changed.
    /// `bots`: the box's bots, then your other computers' (`computer`), as `kuvo/bots` lists them;
    /// `now` in ms.
    pub fn answer(&mut self, method: &str, route: &str, body: &Value, bots: &[Value], now: f64) -> Option<(u16, Value, Option<String>)> {
        let now = now / 1000.0;
        let segments: Vec<&str> = route.split('/').collect();
        if !matches!(segments.as_slice(), ["kuvo", "groups", ..]) {
            return None;
        }
        if self.down {
            return Some((503, json!({ "message": "This Mac’s coding agents aren’t running." }), None));
        }
        let joinable = || -> Vec<Value> { bots.iter().map(member).collect() };
        Some(match (method, segments.as_slice()) {
            ("GET", ["kuvo", "groups"]) => (200, self.list(bots), None),
            ("GET", ["kuvo", "groups", "joinable"]) => (200, json!({ "bots": joinable() }), None),
            ("POST", ["kuvo", "groups"]) => {
                let members = match self.check(body, bots, true) {
                    Ok(checked) => checked.unwrap_or_default(),
                    Err(refused) => return Some((refused.0, refused.1, None)),
                };
                let id = format!("grp_new{}", self.next);
                self.next += 1;
                let purpose = body["purpose"].as_str().map(str::trim).filter(|purpose| !purpose.is_empty()).map(str::to_owned);
                let chat = Chat { id: id.clone(), name: body["name"].as_str().unwrap_or("").trim().to_owned(), purpose, members, messages: Vec::new(), speaking: None, discussing: false, paused: false, unread: false, activity: Vec::new(), stopped: Vec::new() };
                let said = listed(&chat, bots);
                self.chats.push(chat);
                (201, json!({ "group": said }), Some(id))
            }
            (_, ["kuvo", "groups", id, rest @ ..]) => {
                let Some(at) = self.chats.iter().position(|chat| chat.id == *id) else { return Some((404, json!({ "message": "There’s no such group chat here." }), None)) };
                let id = (*id).to_owned();
                match (method, rest) {
                    ("GET", []) => (200, said(&self.chats[at]), None),
                    ("POST", []) => {
                        let text = body["text"].as_str().unwrap_or("").trim().to_owned();
                        if text.is_empty() {
                            return Some((400, json!({ "message": "Write something to send." }), None));
                        }
                        let chat = &mut self.chats[at];
                        let message_id = format!("gm_new{}", self.next);
                        self.next += 1;
                        chat.messages.push(message(&message_id, None, now, &text));
                        // Who talks next is the bridge's to say: here, the first it names, else the first.
                        let named = chat.members.iter().find(|member| bots.iter().any(|bot| bot["id"] == json!(member) && bot["name"].as_str().is_some_and(|name| text.contains(&format!("&{name}"))))).cloned();
                        chat.speaking = named.or_else(|| chat.members.first().cloned());
                        chat.discussing = true;
                        chat.paused = false;
                        chat.activity = vec![json!({ "kind": "thinking", "text": "Reading what you wrote" })];
                        (200, json!({}), Some(id))
                    }
                    ("POST", ["stop"]) => {
                        let chat = &mut self.chats[at];
                        chat.discussing = false;
                        chat.speaking = None;
                        chat.activity.clear();
                        (200, json!({}), Some(id))
                    }
                    ("POST", ["keep-going"]) => {
                        let chat = &mut self.chats[at];
                        chat.paused = false;
                        chat.discussing = true;
                        chat.speaking = chat.members.first().cloned();
                        (200, json!({}), Some(id))
                    }
                    ("POST", ["viewed"]) => {
                        self.chats[at].unread = false;
                        (200, json!({}), Some(id))
                    }
                    ("POST", ["start"]) => {
                        let wanted = body["message"].as_str().unwrap_or("");
                        let chat = &mut self.chats[at];
                        let Some(wrap) = chat.messages.iter_mut().find(|message| message["id"] == json!(wanted)) else { return Some((404, json!({ "message": "That wrap-up isn’t in the chat." }), None)) };
                        for task in wrap["tasks"].as_array_mut().into_iter().flatten() {
                            if task["state"] == json!("proposed") {
                                task["state"] = json!("waiting");
                                task["title"] = json!("Waiting");
                            }
                        }
                        (200, json!({}), Some(id))
                    }
                    ("PATCH", []) => {
                        let members = match self.check(body, bots, false) {
                            Ok(members) => members,
                            Err(refused) => return Some((refused.0, refused.1, None)),
                        };
                        let chat = &mut self.chats[at];
                        if let Some(name) = body["name"].as_str() {
                            chat.name = name.trim().to_owned();
                        }
                        if let Some(purpose) = body["purpose"].as_str() {
                            chat.purpose = Some(purpose.trim().to_owned()).filter(|purpose| !purpose.is_empty());
                        }
                        if let Some(members) = members {
                            chat.members = members;
                        }
                        (200, json!({ "group": listed(chat, bots) }), Some(id))
                    }
                    ("DELETE", []) => {
                        self.chats.remove(at);
                        (200, json!({}), Some(id))
                    }
                    _ => (404, json!({ "message": "There’s no such route." }), None),
                }
            }
            _ => return None,
        })
    }

    /// What New Group or a change gives, as the bridge checks it: a name, and at least two of the
    /// bots it offers. `whole`: New Group (a name and bots are needed); else only what's given.
    fn check(&self, body: &Value, bots: &[Value], whole: bool) -> Result<Option<Vec<String>>, (u16, Value)> {
        let name = body["name"].as_str().map(str::trim);
        if (whole && name.is_none()) || name == Some("") {
            return Err((400, json!({ "message": "A group chat needs a name." })));
        }
        let Some(members) = body["members"].as_array() else {
            return if whole { Err((400, json!({ "message": "A group chat needs at least two bots." }))) } else { Ok(None) };
        };
        let members: Vec<String> = members.iter().filter_map(Value::as_str).map(str::to_owned).collect();
        if members.len() < 2 {
            return Err((400, json!({ "message": "A group chat needs at least two bots." })));
        }
        if let Some(stranger) = members.iter().find(|id| !bots.iter().any(|bot| bot["id"] == json!(id))) {
            return Err((404, json!({ "message": format!("There’s no bot {stranger} that can be in it.") })));
        }
        Ok(Some(members))
    }

    /// `bot`'s conversation resumed (`kuvo/sessions/<its session>/queue/resume`): its parts go on.
    /// The chats it changed.
    pub fn resumed(&mut self, bot: &str) -> Vec<String> {
        let mut changed = Vec::new();
        for chat in &mut self.chats {
            if chat.stopped.iter().any(|each| each == bot) {
                chat.stopped.retain(|each| each != bot);
                changed.push(chat.id.clone());
            }
        }
        changed
    }

    /// What "&" offers in chat `id` for `typed` (`kuvo/groups/<id>/mentions`), as a box ranks them:
    /// its bots, then the guests a message can name (`bots` not in it, then the sessions), each
    /// with names starting with what's typed first; none for no such chat.
    pub fn mentions(&self, id: &str, typed: &str, bots: &[Value]) -> Option<Vec<Value>> {
        let chat = self.chats.iter().find(|chat| chat.id == id)?;
        let typed = typed.to_lowercase();
        let ranked = |mut found: Vec<(String, Value)>| -> Vec<Value> {
            found.retain(|(name, _)| name.to_lowercase().contains(&typed));
            found.sort_by_key(|(name, _)| !name.to_lowercase().starts_with(&typed));
            found.into_iter().map(|(_, mention)| mention).collect()
        };
        let members: Vec<Value> = chat.members.iter().filter_map(|id| bots.iter().find(|bot| bot["id"] == json!(id))).map(member).collect();
        let theirs = ranked(
            members
                .iter()
                .map(|member| {
                    let name = member["name"].as_str().unwrap_or("").to_owned();
                    let detail = match member["computer"].as_str() {
                        Some(computer) => format!("In this chat · on {computer}"),
                        None => "In this chat".to_owned(),
                    };
                    let session = member["session"].as_str().filter(|session| !session.is_empty());
                    (name.clone(), json!({ "text": format!("&{name}"), "title": name, "detail": detail, "kind": "bot", "session": session }))
                })
                .collect(),
        );
        let taken = |name: &str, session: &str| members.iter().any(|member| member["name"].as_str().is_some_and(|each| each.eq_ignore_ascii_case(name)) || member["session"] == json!(session));
        let guest_bots = bots.iter().filter(|bot| !chat.members.iter().any(|id| bot["id"] == json!(id))).map(|bot| {
            let name = bot["name"].as_str().unwrap_or("").to_owned();
            // (As the box says it: its job, else "Bot", then where it is.)
            let job = bot["job"].as_str().filter(|job| !job.is_empty()).unwrap_or("Bot");
            let detail = match bot["computer"].as_str() {
                Some(computer) => format!("{job} · on {computer}"),
                None => job.to_owned(),
            };
            (name.clone(), json!({ "text": format!("&{name}"), "title": name, "detail": detail, "kind": "bot", "session": bot["session"] }))
        });
        let sessions = super::named().into_iter().filter(|(_, kind, _, _)| *kind == "session").map(|(name, kind, session, detail)| (name.to_owned(), json!({ "text": format!("&{name}"), "title": name, "detail": detail, "kind": kind, "session": session })));
        let guests: Vec<(String, Value)> = guest_bots.chain(sessions).filter(|(name, mention)| !taken(name, mention["session"].as_str().unwrap_or(""))).collect();
        let mut offered = theirs;
        offered.extend(ranked(guests));
        offered.truncate(8);
        Some(offered)
    }

    /// The chats as `kuvo/groups` lists them (for its followers).
    pub fn list(&self, bots: &[Value]) -> Value {
        json!({ "groups": self.chats.iter().map(|chat| listed(chat, bots)).collect::<Vec<_>>() })
    }

    /// The chat's line for those following it (`kuvo/groups/<id>/events`), as it is now.
    pub fn line(&self, id: &str) -> Option<String> {
        self.chats.iter().find(|chat| chat.id == id).map(|chat| said(chat).to_string())
    }

    /// Makes `bot` say `text` in `group` now (ms), and ends its turn.
    pub fn say(&mut self, group: &str, bot: &str, text: &str, now: f64) {
        let Some(chat) = self.chats.iter_mut().find(|chat| chat.id == group) else { return };
        let id = format!("gm_said{}", self.next);
        self.next += 1;
        chat.messages.push(message(&id, Some(bot), now / 1000.0, text));
        chat.speaking = None;
        chat.discussing = false;
        chat.activity.clear();
        chat.unread = true;
    }
}

/// A bot as a chat lists it: one of another computer's with that computer's name, its folder
/// empty (it's there).
fn member(bot: &Value) -> Value {
    let mut member = json!({ "id": bot["id"], "name": bot["name"], "look": bot["look"], "session": bot["session"], "directory": bot["folder"].as_str().unwrap_or(""), "agents": bot["agents"] == json!(true) });
    if let Some(computer) = bot["computer"].as_str() {
        member["computer"] = json!(computer);
        member["directory"] = json!("");
    }
    member
}

/// A chat as `kuvo/groups` lists it: what it doesn't have is left out, never null.
fn listed(chat: &Chat, bots: &[Value]) -> Value {
    let members: Vec<Value> = chat.members.iter().filter_map(|id| bots.iter().find(|bot| bot["id"] == json!(id))).map(member).collect();
    let mut listed = json!({ "id": chat.id, "name": chat.name, "members": members, "discussing": chat.discussing, "paused": chat.paused, "unread": chat.unread });
    if let Some(purpose) = &chat.purpose {
        listed["purpose"] = json!(purpose);
    }
    if let Some(speaking) = &chat.speaking {
        listed["speaking"] = json!(speaking);
    }
    if let Some(last) = chat.messages.last() {
        let mut last = last.clone();
        last.as_object_mut().map(|last| last.remove("steps"));
        listed["last"] = last;
    }
    listed
}

/// One chat as `kuvo/groups/<id>` says it.
fn said(chat: &Chat) -> Value {
    let mut said = json!({ "messages": chat.messages, "discussing": chat.discussing, "paused": chat.paused });
    if !chat.stopped.is_empty() {
        said["stopped"] = json!(chat.stopped);
    }
    if let Some(speaking) = &chat.speaking {
        said["speaking"] = json!(speaking);
        if !chat.activity.is_empty() {
            said["activity"] = json!(chat.activity);
        }
    }
    said
}
