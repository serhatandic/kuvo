//! A bot's work, as every client shows it (`GET kuvo/work?bot=<id>`): its sessions and where each
//! stands, the asks it answered for them, what it started today and the limits you set, read from
//! the session list as it keeps each session's work beside its row.

use std::{collections::HashMap, path::Path};

use serde_json::{Value, json};

use super::Work;
use crate::bots::{here::BotHere, list::limits_from};

fn read(file: &Path) -> Value {
    kuvo_store::files::text_of(file)
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn num(value: &Value) -> f64 {
    value.as_f64().unwrap_or(0.0)
}

fn list(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}

impl Work {
    /// The midnight that starts today, in your time zone.
    fn midnight(&self) -> f64 {
        let zone = self
            .setup
            .time_zone()
            .and_then(|(zone, _)| jiff::tz::TimeZone::get(&zone).ok());
        let today =
            jiff::Timestamp::now().to_zoned(zone.unwrap_or_else(jiff::tz::TimeZone::system));
        today
            .start_of_day()
            .map_or(0.0, |start| start.timestamp().as_millisecond() as f64)
    }

    /// What each session waiting on you asks, in a few words: the first asked.
    fn open_asks(&self) -> HashMap<String, String> {
        let kept = read(&self.home.join("run/glance/asks.json"));
        let mut asks: Vec<&Value> = kept["asks"]
            .as_object()
            .into_iter()
            .flat_map(|asks| asks.values())
            .collect();
        asks.sort_by(|a, b| num(&a["at"]).total_cmp(&num(&b["at"])));
        let mut first = HashMap::new();
        for ask in asks
            .into_iter()
            .filter(|ask| ask["at"].is_number() && ask["asks"]["kind"].is_string())
        {
            if let (Some(session), Some(summary)) =
                (ask["session"].as_str(), ask["asks"]["summary"].as_str())
            {
                first
                    .entry(session.to_owned())
                    .or_insert_with(|| summary.to_owned());
            }
        }
        first
    }

    /// `kuvo/work?bot=`: its work, or a status and why not.
    pub fn bot_work(&self, bot: &str) -> Result<Value, (u16, String)> {
        if bot.is_empty() {
            return Err((400, "Name the bot: kuvo/work?bot=<id>.".into()));
        }
        let here =
            BotHere::read(&self.bots, bot).ok_or((404, "There's no such bot here.".to_owned()))?;
        let record = self.sessions.latest_record().unwrap_or_default();
        let activity =
            self.peers
                .view()
                .activity(Some(&here.session), Some(bot), false, self.peers.now());
        let activity = serde_json::to_value(&activity).unwrap_or_default();
        let state = |id: &str| {
            list(&activity["sessions"])
                .find(|row| row["session"] == id)
                .map(|row| row["state"].clone())
        };
        let rows = &record["sessions"];
        let mut ids: Vec<&str> = list(&record["workByBot"][bot])
            .filter_map(Value::as_str)
            .filter(|id| *id != here.session && rows[*id].is_object())
            .collect();
        ids.sort_by(|a, b| {
            num(&rows[*b]["facts"]["created"])
                .total_cmp(&num(&rows[*a]["facts"]["created"]))
                .then(b.cmp(a))
        });
        let (asks, stopped, midnight) = (
            self.open_asks(),
            read(&self.records.folder().join("stopped.json")),
            self.midnight(),
        );
        let (mut sessions, mut answered, mut recent) = (Vec::new(), Vec::new(), 0);
        let mut today =
            json!({"started": 0, "answered": 0, "working": activity["working"], "tokens": 0});
        let mut cost = 0.0;
        for id in &ids {
            let (facts, work) = (&rows[*id]["facts"], &rows[*id]["work"]);
            if num(&facts["created"]) >= midnight {
                today["started"] = json!(num(&today["started"]) as u64 + 1);
                today["tokens"] = json!(num(&today["tokens"]) as u64 + num(&work["tokens"]) as u64);
                cost += num(&work["cost"]);
            }
            for answer in list(&work["answered"]) {
                if num(&answer["at"]) >= midnight {
                    today["answered"] = json!(num(&today["answered"]) as u64 + 1);
                }
                let mut answer = answer.clone();
                answer["session"] = json!(id);
                answered.push(answer);
            }
            let state = state(id);
            if state.is_none() {
                recent += 1;
                if recent > 60 {
                    continue;
                }
            }
            let project = rows[*id]["facts"]["project"]
                .as_str()
                .or(facts["directory"].as_str())
                .unwrap_or_default();
            let mut row = work_row(
                id,
                facts,
                work,
                record["projects"][project]["name"].as_str(),
            );
            row["state"] = match &state {
                Some(state) => state.clone(),
                None if stopped[*id].is_number()
                    && num(ended(work, facts)) < num(&stopped[*id]) =>
                {
                    json!("stopped")
                }
                None => json!(finished(work, facts)),
            };
            if state.as_ref().is_some_and(|state| state == "asking") {
                row["asks"] = json!(asks.get(*id).map_or("Waiting for you", String::as_str));
            }
            sessions.push(row);
        }
        for row in
            list(&activity["sessions"]).filter(|row| !ids.iter().any(|id| row["session"] == *id))
        {
            let mut row = row.clone();
            row["folder"] = json!(
                rows[row["session"].as_str().unwrap_or_default()]["facts"]["directory"]
                    .as_str()
                    .unwrap_or_default()
            );
            for (key, value) in [
                ("startedBy", json!("")),
                ("depth", json!(1)),
                ("started", json!(0)),
                ("tokens", json!(0)),
            ] {
                row[key] = value;
            }
            sessions.push(row);
        }
        if cost > 0.0 {
            today["cost"] = json!(cost);
        }
        answered.sort_by(|a, b| num(&b["at"]).total_cmp(&num(&a["at"])));
        answered.truncate(100);
        let projects = read(&self.home.join("box/projects.json"));
        let limits = here
            .described
            .get("work")
            .and_then(|work| limits_from(work.get("limits")));
        Ok(json!({
            "bot": bot, "activity": activity, "sessions": sessions, "today": today, "answered": answered,
            "limits": limits,
            "asks": self.asks().into_iter().filter(|ask| ask["bot"] == bot).collect::<Vec<_>>(),
            "projects": list(&projects["projects"]).filter(|each| each["removed"] != true && each["madeBy"]["bot"] == bot).collect::<Vec<_>>(),
        }))
    }
}

/// When its last turn ended.
fn ended<'a>(work: &'a Value, facts: &'a Value) -> &'a Value {
    if work["idle"].is_null() {
        &facts["unread"]
    } else {
        &work["idle"]
    }
}

/// Where a session's last turn ended up, as its work says.
fn finished(work: &Value, facts: &Value) -> &'static str {
    let ended = ended(work, facts);
    match work["outcome"].as_str() {
        Some("failed") => "failed",
        Some("interrupted") => "stopped",
        Some("succeeded") => "finished",
        _ if num(ended) != 0.0 => "finished",
        _ => "idle",
    }
}

/// One of a bot's sessions as its work lists it (its state is the caller's).
fn work_row(id: &str, facts: &Value, work: &Value, project_name: Option<&str>) -> Value {
    let directory = facts["directory"].as_str().unwrap_or_default();
    let line = facts["line"].as_array();
    let mut row = json!({
        "session": id,
        "title": facts["title"].as_str().filter(|title| !title.is_empty()).unwrap_or("New Session"),
        "folder": directory,
        "startedBy": work["starter"].as_str().or_else(|| line?.last()?.as_str()).unwrap_or_default(),
        "depth": work["depth"].as_u64().or_else(|| Some(line?.len() as u64)).unwrap_or(1),
        "started": facts["created"],
        "tokens": num(&work["tokens"]) as u64,
    });
    let project = project_name
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            let path = Path::new(facts["project"].as_str().unwrap_or(directory));
            Some(path.file_name()?.to_string_lossy().into_owned())
        });
    if let Some(project) = project.filter(|_| facts["kind"] != "chat") {
        row["project"] = json!(project);
    }
    for key in ["model", "mode", "cost"] {
        if !work[key].is_null() && work[key] != 0 {
            row[key] = work[key].clone();
        }
    }
    row
}
