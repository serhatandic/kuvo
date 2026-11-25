//! start_session here, from a conversation's agent: started as its conversation's, counted
//! (a few at a time; a bot's work with only a net against a runaway) and held to the limits you set
//! for its bot (Edit Bot › Work), and the Work Queue item it names linked before it sees its task.

use std::{collections::HashSet, path::Path, time::Duration};

use kuvo_engine::{ParentLead, StartSteps};
use kuvo_store::locks::{OnBusy, StaleLock};
use serde_json::{Map, Value, json};

use super::{Caller, Said, refused};
use crate::{
    bots::list::limits_from,
    work::{
        Work,
        starts::started_words,
        watch::{
            is_archived,
            tools::{all_work, refusal_words},
            works_for,
        },
    },
};

/// What's counted, at most `most` in `within` ms: sessions a conversation starts, and a bot's.
const SESSIONS: (&str, usize) = ("sessions", 6);
const BOT_SESSIONS: (&str, usize) = ("botSessions", 40);
const WITHIN: i64 = 600_000;
/// How long a bot's start is counted on its own, before its session shows where it works.
const STARTING_FOR: i64 = 120_000;

fn now() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

/// What each caller did lately, in `run/tool-limits.json` (the letters' count's file too), as
/// `change` leaves it, the old left out.
fn change_counts<T>(run: &Path, change: impl FnOnce(&mut Map<String, Value>) -> T) -> Option<T> {
    let _lock = StaleLock::acquire(
        &run.join(".tool-limits.lock"),
        Duration::from_secs(10),
        Duration::from_secs(3),
        OnBusy::Proceed,
    );
    let file = run.join("tool-limits.json");
    let mut counts = std::fs::read(&file)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let at = now();
    for kind in [SESSIONS.0, BOT_SESSIONS.0] {
        let mut by_key = counts
            .get(kind)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        by_key.retain(|_, times| {
            let lately: Vec<Value> = times
                .as_array()
                .into_iter()
                .flatten()
                .filter(|when| when.as_i64().is_some_and(|when| at - when < WITHIN))
                .cloned()
                .collect();
            *times = Value::Array(lately);
            times.as_array().is_some_and(|times| !times.is_empty())
        });
        counts.insert(kind.into(), Value::Object(by_key));
    }
    let mut starting = counts
        .get("starting")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    starting.retain(|_, starts| {
        if let Some(starts) = starts.as_object_mut() {
            starts.retain(|_, start| {
                start["at"]
                    .as_i64()
                    .is_some_and(|when| at - when < STARTING_FOR)
            });
            return !starts.is_empty();
        }
        false
    });
    counts.insert("starting".into(), Value::Object(starting));
    let value = change(&mut counts);
    let _ = serde_json::to_vec(&counts).map(|bytes| kuvo_store::files::write_secret(&file, &bytes));
    Some(value)
}

/// Whether `key` may start one more (`kind`) now: when it's counted (to take back if it doesn't
/// start), or none when it's started that many lately.
fn allowed(run: &Path, (kind, most): (&str, usize), key: &str) -> Option<i64> {
    let at = now();
    change_counts(run, |counts| {
        let by_key = counts.entry(kind).or_insert_with(|| json!({}));
        let lately = by_key[key].as_array().cloned().unwrap_or_default();
        if lately.len() >= most {
            return None;
        }
        let mut lately = lately;
        lately.push(json!(at));
        by_key[key] = Value::Array(lately);
        Some(at)
    })
    .flatten()
}

/// One counted at `at` that didn't start, so one tried again isn't counted twice.
fn take_back(run: &Path, kind: &str, key: &str, at: i64) {
    change_counts(run, |counts| {
        let by_key = counts.entry(kind).or_insert_with(|| json!({}));
        if let Some(lately) = by_key.get_mut(key).and_then(Value::as_array_mut)
            && let Some(index) = lately.iter().position(|when| when.as_i64() == Some(at))
        {
            lately.remove(index);
        }
    });
}

/// A start a bot's limits count, noted before they're looked at, so starts asked for at once each
/// count those before them that the engine doesn't show yet.
struct Starting {
    run: std::path::PathBuf,
    bot: String,
    token: String,
}

impl Starting {
    fn note(run: &Path, bot: &str) -> (Self, Vec<Value>) {
        let mut bytes = [0u8; 6];
        let _ = getrandom::fill(&mut bytes);
        let token = format!(
            "{}-{}",
            now(),
            bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        let before = change_counts(run, |counts| {
            let starting = counts.entry("starting").or_insert_with(|| json!({}));
            let starts = starting
                .get(bot)
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let before: Vec<Value> = starts.values().cloned().collect();
            let mut starts = starts;
            starts.insert(token.clone(), json!({ "at": now() }));
            starting[bot] = Value::Object(starts);
            before
        })
        .unwrap_or_default();
        let starting = Self {
            run: run.to_owned(),
            bot: bot.to_owned(),
            token,
        };
        (starting, before)
    }

    /// How it went: its session (and whether it was given its task), or none, it didn't start.
    fn went(&self, session: Option<&str>, prompted: bool) {
        change_counts(&self.run, |counts| {
            let starting = counts.entry("starting").or_insert_with(|| json!({}));
            let Some(starts) = starting.get_mut(&self.bot).and_then(Value::as_object_mut) else {
                return;
            };
            match session {
                Some(session) if starts.contains_key(&self.token) => {
                    starts[&self.token]["session"] = json!(session);
                    if prompted {
                        starts[&self.token]["prompted"] = json!(true);
                    }
                }
                _ => {
                    starts.remove(&self.token);
                }
            }
        });
    }
}

/// Which limit you set a new session at `depth` would be past, in the words its bot reads, and
/// whether it's a day's (it resets at midnight).
fn past_limit(
    limits: &Map<String, Value>,
    depth: u64,
    working: usize,
    started: usize,
    tokens: u64,
) -> Option<(String, bool)> {
    let limit = |name: &str| limits.get(name).and_then(Value::as_u64);
    let ask = " Wait for one to finish, or ask the user to raise it (Edit Bot › Work).";
    if let Some(most) = limit("depth").filter(|most| depth > *most) {
        let deep = if most == 1 {
            "one level deep".into()
        } else {
            format!("{most} levels deep")
        };
        return Some((
            format!(
                "The user set your sessions to go {deep} at most, so this one can't start another. Do it in this session, or ask the user."
            ),
            false,
        ));
    }
    if let Some(most) = limit("atOnce").filter(|most| working as u64 >= *most) {
        let s = if most == 1 { "" } else { "s" };
        return Some((
            format!(
                "The user set you to {most} session{s} at once at most, and that many are working.{ask}"
            ),
            false,
        ));
    }
    if let Some(most) = limit("perDay").filter(|most| started as u64 >= *most) {
        let s = if most == 1 { "" } else { "s" };
        return Some((
            format!(
                "The user set you to {most} new session{s} a day at most, and you've started that many today."
            ),
            true,
        ));
    }
    if let Some(most) = limit("tokensPerDay").filter(|most| tokens >= *most) {
        return Some((
            format!(
                "Your sessions have used the {} tokens a day the user set.",
                super::sessions::counted(most)
            ),
            true,
        ));
    }
    None
}

/// What a start keeps of itself: its session counted for the bot's limits as it goes, and the Work
/// Queue item it names linked before it sees its task.
struct Steps<'a> {
    work: &'a Work,
    starting: Option<&'a Starting>,
    item: Option<Value>,
    task: String,
    starter: Value,
}

impl StartSteps for Steps<'_> {
    fn made(&self, session: &str) {
        if let Some(starting) = self.starting {
            starting.went(Some(session), false);
        }
    }

    fn prompting<'a>(
        &'a self,
        session: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if let (Some(link), Some(item)) = (self.work.links.get(), &self.item) {
                link.link(Some(item), &self.task, self.starter.clone(), session)
                    .await;
            }
        })
    }

    fn prompted(&self) {}
}

impl Work {
    /// The limits you set for `bot`, looked at for a new session at `depth`: why it can't start,
    /// or its start noted, so those asked for at once count it. Its sessions here and, as they last
    /// told, on your other computers.
    async fn held_by_limits(
        &self,
        bot: &str,
        limits: &Map<String, Value>,
        depth: u64,
    ) -> Result<Starting, String> {
        let run = self.home.join("run");
        let (starting, before) = Starting::note(&run, bot);
        let engine = self.list.engine().running_bot_engine().await;
        let all = match &engine {
            Some(engine) => all_work(engine).await.unwrap_or_default(),
            None => Vec::new(),
        };
        let work: Vec<&Value> = all
            .iter()
            .filter(|data| works_for(data) == Some(bot))
            .collect();
        let (working, _) = match &engine {
            Some(engine) => self.at_work_now(engine).await,
            None => Default::default(),
        };
        let midnight = jiff::Zoned::now()
            .start_of_day()
            .map(|day| day.timestamp().as_millisecond())
            .unwrap_or(0);
        let created = |data: &Value| data["time"]["created"].as_f64().unwrap_or(0.0) as i64;
        let today: Vec<&&Value> = work
            .iter()
            .filter(|data| created(data) >= midnight)
            .collect();
        let ids: HashSet<&str> = work.iter().filter_map(|data| data["id"].as_str()).collect();
        let today_ids: HashSet<&str> = today
            .iter()
            .filter_map(|data| data["id"].as_str())
            .collect();
        let started_today = today
            .iter()
            .filter(|data| {
                data["metadata"]["kuvo.carriedTo"]
                    .as_str()
                    .is_none_or(|to| !ids.contains(to))
            })
            .count();
        // Its sessions on your other computers too, as those last told this one.
        let own = crate::bots::here::BotHere::read(&self.bots, bot).map(|here| here.session);
        let mut far = Vec::new();
        for lead in own.iter().map(String::as_str).chain(ids.iter().copied()) {
            for there in kuvo_peers::work_rows::for_lead(&self.peers, lead)
                .await
                .unwrap_or_default()
            {
                far.extend(
                    there
                        .rows
                        .into_iter()
                        .filter(|row| !ids.contains(row.session.as_str())),
                );
            }
        }
        let far_today: Vec<_> = far
            .iter()
            .filter(|row| row.started as i64 >= midnight)
            .collect();
        let tokens = |data: &Value| {
            ["input", "output", "reasoning"]
                .iter()
                .map(|field| data["tokens"][field].as_u64().unwrap_or(0))
                .sum::<u64>()
        };
        let tokens = today.iter().map(|data| tokens(data)).sum::<u64>()
            + far_today.iter().map(|row| row.tokens as u64).sum::<u64>();
        let known =
            |session: Option<&str>, ids: &HashSet<&str>, rows: &[&kuvo_peers::work_rows::Row]| {
                session.is_some_and(|session| {
                    ids.contains(session) || rows.iter().any(|row| row.session == session)
                })
            };
        let far_all: Vec<_> = far.iter().collect();
        let earlier_working = before
            .iter()
            .filter(|start| {
                start["prompted"] != true || !known(start["session"].as_str(), &ids, &far_all)
            })
            .count();
        let earlier_started = before
            .iter()
            .filter(|start| !known(start["session"].as_str(), &today_ids, &far_today))
            .count();
        let busy = |row: &&kuvo_peers::work_rows::Row| {
            matches!(
                serde_json::to_value(&row.state)
                    .ok()
                    .as_ref()
                    .and_then(Value::as_str),
                Some("working" | "asking")
            )
        };
        let working = work
            .iter()
            .filter(|data| {
                data["id"].as_str().is_some_and(|id| working.contains(id)) && !is_archived(data)
            })
            .count()
            + far.iter().filter(busy).count()
            + earlier_working;
        let started = started_today + far_today.len() + earlier_started;
        match past_limit(limits, depth, working, started, tokens) {
            None => Ok(starting),
            Some((words, daily)) => {
                starting.went(None, false);
                Err(if daily {
                    format!(
                        "{words} Ask the user to raise it (Edit Bot › Work), or go on tomorrow."
                    )
                } else {
                    words
                })
            }
        }
    }

    /// start_session here, for `caller` (none: an agent outside any conversation here).
    pub async fn start_here(
        &self,
        caller: Option<&Caller>,
        args: &Map<String, Value>,
        item: Option<Value>,
    ) -> Said {
        let Some(caller) = caller else {
            // (The tool servers here always say which conversation calls.)
            return refused("Only a conversation on this computer can start sessions here.");
        };
        let run = self.home.join("run");
        let bot = caller.works_for();
        let own = caller.bot.is_some();
        let depth = if own {
            1
        } else {
            caller.data["metadata"]["kuvo"]["depth"]
                .as_u64()
                .unwrap_or(1)
                .max(1)
                + 1
        };
        let limits = bot.as_ref().and_then(|bot| {
            let here = crate::bots::here::BotHere::read(&self.bots, &bot.bot);
            here.and_then(|here| {
                limits_from(
                    here.described
                        .get("work")
                        .and_then(|work| work.get("limits")),
                )
            })
            .or_else(|| limits_from(caller.data["metadata"]["kuvo"].get("limits")))
        });
        let starting = match (&bot, &limits) {
            (Some(bot), Some(limits)) => match self.held_by_limits(&bot.bot, limits, depth).await {
                Ok(starting) => Some(starting),
                Err(why) => return refused(why),
            },
            _ => None,
        };
        let went = |session: Option<&str>| {
            if let Some(starting) = &starting {
                starting.went(session, session.is_some());
            }
        };
        let (kind, key) = match &bot {
            Some(bot) => (BOT_SESSIONS, format!("bot:{}", bot.bot)),
            None => (SESSIONS, caller.session.clone()),
        };
        let Some(counted) = allowed(&run, kind, &key) else {
            went(None);
            return refused(if bot.is_some() {
                "You've started a great many sessions in the last few minutes, which looks like a loop. Stop, check on the ones you have (list_sessions), and tell the user."
            } else {
                "You've started several sessions in the last few minutes. Write to those (send_message), or ask the user before starting more."
            });
        };
        let mut given = args.clone();
        given.remove("computer");
        given.remove("wait");
        given.remove("item");
        given.insert("startedBy".into(), json!(caller.session));
        let works = bot
            .as_ref()
            .map(|bot| json!({ "bot": bot.bot, "name": bot.name }));
        let lead = ParentLead {
            name: caller.name.clone(),
            works,
            own,
            depth,
        };
        let task = args
            .get("task")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let steps = Steps {
            work: self,
            starting: starting.as_ref(),
            item,
            task,
            starter: json!({ "session": caller.session, "name": caller.name }),
        };
        match self
            .list
            .engine()
            .start_for_parent(&given, &lead, &steps)
            .await
        {
            Ok(started) => {
                went(started["session"].as_str());
                if let Some(bot) = &bot {
                    self.records.note_changed(&bot.bot);
                    if own
                        && started["wakes"].is_array()
                        && let Some(session) = started["session"].as_str()
                    {
                        self.records.note_oversight(&bot.bot, session, "started");
                    }
                }
                Said {
                    text: started_words(&started),
                    refused: false,
                }
            }
            Err(refusal) => {
                take_back(&run, kind.0, &key, counted);
                went(None);
                refused(refusal_words(refusal).await)
            }
        }
    }
}

/// A start's fields a call carries to another computer (all but the item and the wait, which are
/// this one's).
const START_FIELDS: &[&str] = &[
    "task", "title", "project", "folder", "chat", "worktree", "model", "effort", "mode",
    "approval", "wake", "review",
];
/// The most rules a start carries.
const MOST_HELD: usize = 300;

impl Work {
    /// start_session on another of your computers (`peer`), from `caller` here: checked here first
    /// as a start is (its mode and approval, where it works, the limits you set and the net against
    /// a runaway), then sent there with who's calling as this computer knows it.
    pub async fn start_elsewhere(
        &self,
        peer: kuvo_peers::directory::Member,
        caller: &Caller,
        args: &Map<String, Value>,
        item: Option<Value>,
    ) -> Said {
        use super::calls::{AWAY_WORDS, Keep, Went, asleep, name_of};
        let name = name_of(&peer);
        let text = |field: &str| {
            args.get(field)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
        };
        let Some(task) = text("task") else {
            return refused("Say what it's to do (task).");
        };
        let older = format!(
            "{name} runs an older Kuvo Code that can't take work from other computers. Updating Kuvo Code on {name} lets it: ask the user."
        );
        if !self
            .peers
            .view()
            .advertises(&peer.key, "work.elsewhere.start")
        {
            return refused(older);
        }
        let data = &caller.data;
        let mode = args.get("mode").filter(|mode| !mode.is_null());
        if mode.is_some_and(|mode| mode != "build" && mode != "plan") {
            return refused("mode is build or plan.");
        }
        if mode.is_some_and(|mode| mode == "build") && data["agent"] == "plan" {
            return refused(
                "This conversation is in Plan mode, so the sessions it starts plan too. Start it in plan mode, or ask the user to switch to Build.",
            );
        }
        let approval = args.get("approval").filter(|approval| !approval.is_null());
        if approval.is_some_and(|approval| crate::approval::Approval::of(approval).is_none()) {
            return refused("approval is askFirst, acceptEdits, auto or fullAccess.");
        }
        let named = args.get("chat") == Some(&Value::Bool(true))
            || text("project").is_some()
            || text("folder").is_some();
        if !named {
            return refused(format!(
                "Say where it works on {name}: one of its projects (project; list_projects with computer \"{name}\" lists them), a folder there (folder, a full path), or chat."
            ));
        }
        // Who's calling, as this computer knows it: its approval as it comes to here now, and its
        // Don't Allows.
        let approvals = crate::approval::Approvals::new(&self.home, self.settings.is_mac());
        let bot = caller.works_for();
        let own = caller.bot.is_some();
        let effective = approvals
            .of(
                data,
                bot.is_some(),
                approvals.since(),
                approvals.default_approval(),
            )
            .effective;
        let lead = caller.lead(self);
        let held: Vec<Value> = lead
            .rules
            .iter()
            .filter(|rule| rule.effect != "allow" && rule.effect != "ask")
            .map(|rule| json!({ "action": rule.action, "resource": rule.resource, "effect": rule.effect }))
            .collect();
        if held.len() > MOST_HELD {
            return refused(format!(
                "Your rules are too many to carry to {name}. Start it here, or ask the user."
            ));
        }
        let depth = if own {
            1
        } else {
            data["metadata"]["kuvo"]["depth"]
                .as_u64()
                .unwrap_or(1)
                .max(1)
                + 1
        };
        let limits = bot.as_ref().and_then(|bot| {
            let here = crate::bots::here::BotHere::read(&self.bots, &bot.bot);
            here.and_then(|here| {
                limits_from(
                    here.described
                        .get("work")
                        .and_then(|work| work.get("limits")),
                )
            })
            .or_else(|| limits_from(data["metadata"]["kuvo"].get("limits")))
        });
        let starting = match (&bot, &limits) {
            (Some(bot), Some(limits)) => match self.held_by_limits(&bot.bot, limits, depth).await {
                Ok(starting) => Some(starting),
                Err(why) => return refused(why),
            },
            _ => None,
        };
        let went = |session: Option<&str>, prompted: bool| {
            if let Some(starting) = &starting {
                starting.went(session, prompted);
            }
        };
        let run = self.home.join("run");
        let (kind, key) = match &bot {
            Some(bot) => (BOT_SESSIONS, format!("bot:{}", bot.bot)),
            None => (SESSIONS, caller.session.clone()),
        };
        let Some(counted) = allowed(&run, kind, &key) else {
            went(None, false);
            return refused(if bot.is_some() {
                "You've started a great many sessions in the last few minutes, which looks like a loop. Stop, check on the ones you have (list_sessions), and tell the user."
            } else {
                "You've started several sessions in the last few minutes. Write to those (send_message), or ask the user before starting more."
            });
        };
        let failed = |words: String| {
            take_back(&run, kind.0, &key, counted);
            went(None, false);
            refused(words)
        };
        let view = self.peers.view();
        let own_key = view.roster.here.map(|key| kuvo_wire::keys::encode(&key));
        let mut line: Vec<String> = super::super::watch::tools::line_of(data)
            .into_iter()
            .map(str::to_owned)
            .collect();
        line.push(caller.session.clone());
        let line: Vec<String> = line[line.len().saturating_sub(20)..].to_vec();
        let homes: Map<String, Value> = line
            .iter()
            .filter_map(|session| {
                let at = kuvo_peers::calls::known_home(&run, session)
                    .map(|key| kuvo_wire::keys::encode(&key))
                    .or(own_key.clone())?;
                Some((session.clone(), json!(at)))
            })
            .collect();
        let model = &data["model"];
        let model = match (model["providerID"].as_str(), model["id"].as_str()) {
            (Some(provider), Some(id)) if !provider.is_empty() && !id.is_empty() => {
                let mut model = json!({ "providerID": provider, "id": id });
                if let Some(variant) = data["model"]["variant"]
                    .as_str()
                    .filter(|variant| !variant.is_empty())
                {
                    model["variant"] = json!(variant);
                }
                model
            }
            _ => Value::Null,
        };
        let start = json!({
            "approval": effective, "held": held, "agent": if data["agent"] == "plan" { json!("plan") } else { Value::Null },
            "model": model, "autopilot": data["metadata"]["kuvo"]["autopilot"] == true,
            "works": bot.as_ref().map(|bot| json!({ "bot": bot.bot, "name": bot.name })),
            "own": own, "depth": depth, "line": line, "limits": limits, "homes": homes,
        });
        let mut sent = Map::new();
        for field in START_FIELDS {
            if let Some(value) = args.get(*field).filter(|value| !value.is_null()) {
                sent.insert((*field).into(), value.clone());
            }
        }
        sent.insert("task".into(), json!(task));
        if let Some(item) = item.filter(|item| !item.is_null()) {
            sent.insert("item".into(), item);
        }
        match self
            .call_peer(&peer, "start_session", sent, caller, Keep::No, Some(start))
            .await
        {
            Went::Answer(answer) => {
                // (Its words are that computer's: said so, as they would be had they come as a
                // message.)
                let text = if answer.text.starts_with("[On ") {
                    answer.text
                } else {
                    format!("[On {name}] {}", answer.text)
                };
                let session = super::super::watch::address_in(&text)
                    .filter(|id| super::session_id(id))
                    .map(str::to_owned);
                let Some(session) = session else {
                    // Refused there, with nothing made: it didn't start.
                    if answer.refused {
                        return failed(text);
                    }
                    went(None, false);
                    return Said {
                        text,
                        refused: false,
                    };
                };
                // Made there: it lives there, so a stop or an answer for it goes there at once,
                // before its first rows come.
                let _ = kuvo_peers::calls::note_homes(
                    &run,
                    peer.key,
                    now(),
                    std::iter::once(session.as_str()),
                );
                self.note_started_on(&caller.session, &peer.key);
                went(Some(&session), !answer.refused);
                Said {
                    text,
                    refused: answer.refused,
                }
            }
            Went::Failed(why) => failed(format!("On {name}: {why}")),
            Went::Older => failed(older),
            Went::Offline { .. } => failed(self.offline_words(&name)),
            Went::Away {
                why, sent: false, ..
            } if why == AWAY_WORDS => failed(asleep(&name, "nothing was started")),
            Went::Away { why, .. } => {
                // It went, and no answer came: it may have started. Not tried again, so it's
                // never started twice.
                went(None, false);
                let (why, back) = if why == AWAY_WORDS {
                    (
                        format!("it stopped answering ({AWAY_WORDS}) after the start went"),
                        format!(" once {name} is back"),
                    )
                } else {
                    ("it didn't answer in time".to_owned(), String::new())
                };
                refused(format!(
                    "Couldn't confirm the session started on {name}: {why}. It may still have started, and if it did you'll hear as a message{back}. Check with list_sessions before you start it again."
                ))
            }
        }
    }
}
