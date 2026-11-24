//! A bot's work, as every client shows it (`GET kuvo/work?bot=<id>`): its sessions and where each
//! stands, the asks it answered for them, what it started today and the limits you set, read from
//! the session list as it keeps each session's work beside its row.

use std::path::Path;

use serde_json::{Map, Value, json};

use super::Work;
use crate::bots::{here::BotHere, list::limits_from};

/// Recent sessions shown that are no longer at work.
const RECENT: usize = 60;

fn read(file: &Path) -> Value {
    kuvo_store::files::text_of(file)
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Where a finished session's turn ended up.
fn finished_state(outcome: Option<&str>, ended: Option<f64>) -> &'static str {
    match outcome {
        Some("failed") => "failed",
        Some("interrupted") => "stopped",
        Some("succeeded") => "finished",
        _ if ended.is_some_and(|ended| ended != 0.0) => "finished",
        _ => "idle",
    }
}

impl Work {
    /// The midnight that starts today, in your time zone.
    fn midnight(&self) -> i64 {
        let zone = self
            .setup
            .time_zone()
            .and_then(|(zone, _)| jiff::tz::TimeZone::get(&zone).ok())
            .unwrap_or_else(jiff::tz::TimeZone::system);
        jiff::Timestamp::now()
            .to_zoned(zone)
            .start_of_day()
            .map_or(0, |start| start.timestamp().as_millisecond())
    }

    /// `kuvo/work?bot=`: its work, or a status and why not.
    pub fn bot_work(&self, bot: &str) -> Result<Value, (u16, String)> {
        if bot.is_empty() {
            return Err((400, "Name the bot: kuvo/work?bot=<id>.".into()));
        }
        let Some(here) = BotHere::read(&self.bots, bot) else {
            return Err((404, "There's no such bot here.".into()));
        };
        let list = self.sessions.latest_record().unwrap_or_default();
        let view = self.peers.view();
        let activity = view.activity(Some(&here.session), Some(bot), false, self.peers.now());
        let activity = serde_json::to_value(&activity).unwrap_or_default();
        let states: Map<String, Value> = activity["sessions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| Some((row["session"].as_str()?.to_owned(), row["state"].clone())))
            .collect();
        let listed = list["sessions"].as_object().cloned().unwrap_or_default();
        let mut ids: Vec<&String> = match list["workByBot"].get(bot).and_then(Value::as_array) {
            Some(ids) => ids
                .iter()
                .filter_map(Value::as_str)
                .filter_map(|id| listed.get_key_value(id).map(|(id, _)| id))
                .collect(),
            None => Vec::new(),
        };
        ids.retain(|id| **id != here.session);
        ids.sort_by(|a, b| {
            let created = |id: &str| listed[id]["facts"]["created"].as_f64().unwrap_or(0.0);
            created(b).total_cmp(&created(a)).then_with(|| b.cmp(a))
        });
        let asks = self.open_asks();
        let stopped = read(&self.records.folder().join("stopped.json"));
        let midnight = self.midnight() as f64;
        let mut sessions = Vec::new();
        let mut answered = Vec::new();
        let (mut started, mut answered_today, mut tokens, mut cost) = (0, 0, 0u64, 0.0);
        let mut recent = 0;
        let mut shown = std::collections::HashSet::new();
        for id in ids {
            let row = &listed[id.as_str()];
            let (facts, work) = (&row["facts"], &row["work"]);
            if facts["created"].as_f64().unwrap_or(0.0) >= midnight {
                started += 1;
                tokens += work["tokens"].as_u64().unwrap_or(0);
                cost += work["cost"].as_f64().unwrap_or(0.0);
            }
            for answer in work["answered"].as_array().into_iter().flatten() {
                let mut answer = answer.clone();
                answer["session"] = json!(id);
                if answer["at"].as_f64().is_some_and(|at| at >= midnight) {
                    answered_today += 1;
                }
                answered.push(answer);
            }
            let state = states.get(id.as_str()).and_then(Value::as_str);
            if state.is_none() {
                if recent >= RECENT {
                    recent += 1;
                    continue;
                }
                recent += 1;
            }
            let project_name = list["projects"][facts["project"]
                .as_str()
                .or(facts["directory"].as_str())
                .unwrap_or_default()]["name"]
                .as_str();
            sessions.push(self.work_row(
                id,
                facts,
                work,
                project_name,
                state,
                stopped[id.as_str()].as_f64(),
                asks.get(id.as_str()),
            ));
            shown.insert(id.clone());
        }
        for row in activity["sessions"].as_array().into_iter().flatten() {
            let Some(session) = row["session"].as_str() else {
                continue;
            };
            if shown.contains(session) {
                continue;
            }
            let mut row = row.clone();
            row["folder"] = json!(
                listed
                    .get(session)
                    .and_then(|row| row["facts"]["directory"].as_str())
                    .unwrap_or_default()
            );
            row["startedBy"] = json!("");
            row["depth"] = json!(1);
            row["started"] = json!(0);
            row["tokens"] = json!(0);
            sessions.push(row);
        }
        answered.sort_by(|a, b| {
            b["at"]
                .as_f64()
                .unwrap_or(0.0)
                .total_cmp(&a["at"].as_f64().unwrap_or(0.0))
        });
        answered.truncate(100);
        let mut today = json!({"started": started, "answered": answered_today, "working": activity["working"], "tokens": tokens});
        if cost > 0.0 {
            today["cost"] = json!(cost);
        }
        let projects: Vec<Value> = read(&self.home.join("box/projects.json"))["projects"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|project| project["removed"] != true && project["madeBy"]["bot"] == bot)
            .cloned()
            .collect();
        let limits = here
            .described
            .get("work")
            .and_then(|work| limits_from(work.get("limits")))
            .map_or(Value::Null, Value::Object);
        Ok(json!({
            "bot": bot, "activity": activity, "sessions": sessions, "today": today,
            "answered": answered, "limits": limits,
            "asks": self.asks().into_iter().filter(|ask| ask["bot"] == bot).collect::<Vec<_>>(),
            "projects": projects,
        }))
    }

    /// What each session waiting on you asks, in a few words, the first asked first.
    fn open_asks(&self) -> std::collections::HashMap<String, String> {
        let kept = read(&self.home.join("run/glance/asks.json"));
        let mut asks: Vec<&Value> = kept["asks"]
            .as_object()
            .into_iter()
            .flat_map(|asks| asks.values())
            .filter(|ask| ask["session"].is_string() && ask["at"].is_number())
            .collect();
        asks.sort_by(|a, b| {
            a["at"]
                .as_f64()
                .unwrap_or(0.0)
                .total_cmp(&b["at"].as_f64().unwrap_or(0.0))
        });
        let mut first = std::collections::HashMap::new();
        for ask in asks {
            if let (Some(session), Some(summary)) =
                (ask["session"].as_str(), ask["asks"]["summary"].as_str())
                && ask["asks"]["kind"].is_string()
            {
                first
                    .entry(session.to_owned())
                    .or_insert_with(|| summary.to_owned());
            }
        }
        first
    }

    #[allow(clippy::too_many_arguments)]
    fn work_row(
        &self,
        session: &str,
        facts: &Value,
        work: &Value,
        project_name: Option<&str>,
        state: Option<&str>,
        stopped: Option<f64>,
        asks: Option<&String>,
    ) -> Value {
        let ended = work["idle"].as_f64().or_else(|| facts["unread"].as_f64());
        let directory = facts["directory"].as_str().unwrap_or_default();
        let line = facts["line"].as_array();
        let mut row = json!({
            "session": session,
            "title": facts["title"].as_str().filter(|title| !title.is_empty()).unwrap_or("New Session"),
            "folder": directory,
            "startedBy": work["starter"].as_str().or_else(|| line.and_then(|line| line.last()?.as_str())).unwrap_or_default(),
            "depth": work["depth"].as_u64().or_else(|| line.map(|line| line.len() as u64)).unwrap_or(1),
            "started": facts["created"],
            "tokens": work["tokens"].as_u64().unwrap_or(0),
        });
        if facts["kind"] != "chat" {
            let project = project_name
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    Path::new(facts["project"].as_str().unwrap_or(directory))
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default()
                });
            if !project.is_empty() {
                row["project"] = json!(project);
            }
        }
        for key in ["model", "mode"] {
            if !work[key].is_null() {
                row[key] = work[key].clone();
            }
        }
        row["state"] = json!(match state {
            Some(state) => state,
            None if stopped.is_some_and(|stopped| ended.unwrap_or(0.0) < stopped) => "stopped",
            None => finished_state(work["outcome"].as_str(), ended),
        });
        if state == Some("asking") {
            row["asks"] = json!(asks.map_or("Waiting for you", String::as_str));
        }
        if work["cost"].as_f64().is_some_and(|cost| cost != 0.0) {
            row["cost"] = work["cost"].clone();
        }
        row
    }
}
