//! Import the app's local bots and group chats into the bridge's storage.
use crate::{Disk, Error, metadata};
use kuvo_store::{files, format::Fields};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

struct LegacyBot<'a> {
    id: &'a str,
    name: &'a str,
    folder: &'a str,
    session: &'a str,
    fields: &'a Value,
}

impl<'a> LegacyBot<'a> {
    fn read(fields: &'a Value) -> Option<Self> {
        let id = fields["id"].as_str()?;
        safe_id(id, "bot_", true).then_some(())?;
        Some(Self {
            id,
            name: fields["name"].as_str()?,
            folder: fields["folder"].as_str()?,
            session: fields["sessionID"].as_str()?,
            fields,
        })
    }

    fn remote(&self) -> bool {
        truthy(&self.fields["machine"])
    }

    fn project(&self) -> Result<String, Error> {
        self.fields["project"]
            .as_str()
            .map(str::to_owned)
            .map_or_else(
                || {
                    Path::new(self.folder)
                        .join("workspace")
                        .into_os_string()
                        .into_string()
                        .map_err(|_| super::invalid("The bot's workspace needs a UTF-8 path."))
                },
                Ok,
            )
    }

    fn import(&self, stage: &Disk) -> Result<(), Error> {
        let folder = stage.path(Path::new("bots").join(self.id))?;
        if !metadata(&folder)?.is_some_and(|meta| meta.is_dir()) {
            return Ok(());
        }
        let record = folder.join("bot.json");
        let old = read(&record);
        if !(old["kuvo"] == 1 && old["id"] == self.id && old["name"].is_string()) {
            let mut next = Fields::from_iter([
                ("id".into(), json!(self.id)),
                ("name".into(), json!(self.name)),
                ("kuvo".into(), json!(1)),
                (
                    "job".into(),
                    json!(self.fields["job"].as_str().unwrap_or_default()),
                ),
            ]);
            for key in ["created", "look", "model"] {
                if let Some(value) = self.fields.get(key) {
                    next.insert(key.into(), value.clone());
                }
            }
            for key in ["personality", "project"] {
                if truthy(&self.fields[key]) {
                    next.insert(key.into(), self.fields[key].clone());
                }
            }
            if let Some(value) = self
                .fields
                .get("updated")
                .filter(|value| !value.is_null())
                .or_else(|| self.fields.get("created"))
            {
                next.insert("updated".into(), value.clone());
            }
            write(&record, &next)?;
        }
        let session = folder.join("session");
        if !self.session.is_empty()
            && files::read(&session)?
                .as_deref()
                .and_then(|bytes| std::str::from_utf8(bytes).ok())
                .map(str::trim)
                != Some(self.session)
        {
            files::write_atomic(&session, self.session.as_bytes())?;
        }
        let directory = folder.join("directory");
        if metadata(&directory)?.is_none() {
            files::write_new(&directory, self.project()?.as_bytes())?;
        }
        let via = folder.join("via");
        if self.fields["agents"] == true && metadata(&via)?.is_none() {
            files::write_new(&via, b"bridge")?;
        }
        Ok(())
    }
}

pub(super) fn take(source: &Path, stage: &Disk, prefs: &Fields) -> Result<Vec<String>, Error> {
    let list = source.join("bots.json");
    let bots = read(&list);
    let mut dropped = Vec::new();
    if !bots.is_array() && metadata(&list)?.is_some() {
        dropped.push("The earlier Kuvo Code's list of bots couldn't be read. Bots made with a version before 0.7 may be missing: their folders are brought along, in ~/.kuvo-code/bots.".into());
    }
    let list: Vec<_> = array(&bots).iter().filter_map(LegacyBot::read).collect();
    let remote: BTreeSet<_> = list
        .iter()
        .filter(|bot| bot.remote())
        .map(|bot| bot.id)
        .collect();
    if !prefs.contains_key("calibration") {
        match list.iter().filter(|bot| bot.remote()).count() {
            0 => {}
            1 => dropped.push("A bot on your machines was described only on this Mac by the earlier Kuvo Code, and may not show there. Its work is still on that machine: make it again if it's missing.".into()),
            count => dropped.push(format!("{count} bots on your machines were described only on this Mac by the earlier Kuvo Code, and may not show there. Their work is still on those machines: make them again if they're missing.")),
        }
    }
    for bot in list.iter().filter(|bot| !bot.remote()) {
        bot.import(stage)?;
    }
    import_groups(source, stage, prefs, &remote, &mut dropped)?;
    Ok(dropped)
}

#[derive(Serialize)]
struct Group<'a> {
    id: &'a str,
    name: &'a str,
    members: Vec<&'a str>,
    seats: BTreeMap<&'a str, &'a str>,
    heard: BTreeMap<&'a str, &'a str>,
    created: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    purpose: Option<&'a str>,
    #[serde(rename = "agentSeats", skip_serializing_if = "Option::is_none")]
    agent_seats: Option<Vec<&'a str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    viewed: Option<&'a str>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    retracted: BTreeMap<&'a str, &'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    meeting: Option<Meeting<'a>>,
}

#[derive(Serialize)]
struct Meeting<'a> {
    next: &'a str,
    topic: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    every: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    weekday: Option<i64>,
}

impl<'a> Meeting<'a> {
    fn read(value: &'a Value) -> Option<Self> {
        Some(Self {
            next: date(&value["next"])?,
            topic: value["topic"].as_str()?,
            every: value["every"].as_str(),
            at: value["at"].as_i64(),
            weekday: value["weekday"].as_i64(),
        })
    }
}

fn import_groups(
    source: &Path,
    stage: &Disk,
    prefs: &Fields,
    remote: &BTreeSet<&str>,
    dropped: &mut Vec<String>,
) -> Result<(), Error> {
    if let Some(waiting) = prefs.get("groupMemoryWaiting").and_then(Value::as_object) {
        let file = stage.path("groups/pending-memory.json")?;
        let existing = read(&file);
        let mut merged = BTreeMap::<&str, BTreeSet<&str>>::new();
        for (bot, chats) in existing.as_object().into_iter().flatten().chain(waiting) {
            merged
                .entry(bot)
                .or_default()
                .extend(array(chats).iter().filter_map(Value::as_str));
        }
        files::write_atomic(
            &file,
            &serde_json::to_vec(&merged).map_err(std::io::Error::other)?,
        )?;
    }
    let list = source.join("groups.json");
    let groups = read(&list);
    if !groups.is_array() {
        if metadata(&list)?.is_some() {
            dropped.push("Your group chats couldn't be read, and weren't brought along. They're still in the earlier Kuvo Code's folder (groups.json).".into());
        }
        return Ok(());
    }
    let held: BTreeSet<_> = prefs
        .get("movesUnderWay")
        .into_iter()
        .flat_map(array)
        .filter(|entry| entry["kind"] == "group")
        .filter_map(|entry| entry["id"].as_str())
        .collect();
    let imported = read(&stage.path("groups/imported.json")?);
    let deleted: BTreeSet<_> = array(&imported["ids"])
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let mut failed = 0;
    let mut unread = 0;
    let mut seats = 0;
    for chat in array(&groups) {
        let Some(id) = chat["id"].as_str().filter(|id| safe_id(id, "grp_", false)) else {
            continue;
        };
        let folder = Path::new("groups").join(id);
        let record = stage.path(folder.join("group.json"))?;
        if deleted.contains(id) || metadata(&record)?.is_some() {
            continue;
        }
        let Some(members) = strings(&chat["members"]) else {
            failed += 1;
            continue;
        };
        let old_name = Path::new("groups").join(format!("{id}.json"));
        let old = read(&source.join(&old_name));
        let messages: Vec<_> = array(&old).iter().filter_map(message).collect();
        unread += array(&old).len() - messages.len();
        let created = date(&chat["created"])
            .or_else(|| messages.first().and_then(|entry| entry["time"].as_str()))
            .map_or_else(
                || {
                    jiff::Timestamp::now()
                        .strftime("%Y-%m-%dT%H:%M:%SZ")
                        .to_string()
                },
                str::to_owned,
            );
        let mut group = Group {
            id,
            name: nonempty(&chat["name"]).unwrap_or("Group Chat"),
            members,
            seats: string_map(&chat["seats"]),
            heard: string_map(&chat["heard"]),
            created,
            purpose: nonempty(&chat["purpose"]),
            agent_seats: strings(&chat["agentSeats"]),
            viewed: date(&chat["viewed"]),
            retracted: string_map(&chat["retracted"]),
            meeting: Meeting::read(&chat["meeting"]),
        };
        for bot in remote {
            if let Some(seat) = group.seats.remove(bot) {
                seats += 1;
                group.heard.remove(bot);
                if let Some(seat_ids) = &mut group.agent_seats {
                    seat_ids.retain(|id| *id != seat);
                }
            }
        }
        write(&stage.path(folder.join("messages.json"))?, &messages)?;
        files::remove_file_synced(&stage.path(old_name)?)?;
        let status = if held.contains(id) { "held" } else { "active" };
        write(
            &record,
            &json!({ "kuvo": 1, "status": status, "group": group }),
        )?;
    }
    match failed {
        0 => {},
        1 => dropped.push("One group chat couldn't be read, and wasn't brought along. It's still in the earlier Kuvo Code's folder (groups.json).".into()),
        count => dropped.push(format!("{count} group chats couldn't be read, and weren't brought along. They're still in the earlier Kuvo Code's folder (groups.json).")),
    }
    match unread {
        0 => {}
        1 => dropped.push(
            "One message in your group chats couldn't be read, and wasn't brought along.".into(),
        ),
        count => dropped.push(format!(
            "{count} messages in your group chats couldn't be read, and weren't brought along."
        )),
    }
    match seats {
        0 => {},
        1 => dropped.push("In your group chats, a bot on one of your machines takes its seat again at its next turn: the conversation it had there before stays on that machine, unused.".into()),
        count => dropped.push(format!("In your group chats, {count} bots on your machines take their seats again at their next turn: the conversation they had there before stays on that machine, unused.")),
    }
    Ok(())
}

fn message(value: &Value) -> Option<Fields> {
    let mut fields = value.as_object()?.clone();
    value["id"].as_str()?;
    value["text"].as_str()?;
    date(&value["time"])?;
    match value.get("kind") {
        None => {
            fields.insert("kind".into(), json!("message"));
        }
        Some(kind)
            if matches!(
                kind.as_str(),
                Some("message" | "wrapUp" | "note" | "meeting")
            ) => {}
        _ => return None,
    }
    match value.get("author") {
        Some(Value::Null) => {
            fields.remove("author");
        }
        Some(author) if !author.is_string() => return None,
        _ => {}
    }
    if let Some(tasks) = value.get("tasks") {
        if !tasks.as_array()?.iter().all(|task| {
            ["id", "bot", "task"]
                .iter()
                .all(|key| task[*key].is_string())
                && matches!(
                    task["state"].as_str(),
                    Some("proposed" | "waiting" | "working" | "done" | "failed")
                )
        }) {
            return None;
        }
    }
    Some(fields)
}

fn read(path: &Path) -> Value {
    crate::json_at(path).ok().flatten().unwrap_or_default()
}

fn write(path: &Path, value: &impl Serialize) -> Result<(), Error> {
    let bytes = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    Ok(files::write_atomic(path, &bytes)?)
}

fn safe_id(id: &str, prefix: &str, upper: bool) -> bool {
    id.strip_prefix(prefix).is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix.bytes().all(|c| {
                c.is_ascii_digit() || c.is_ascii_lowercase() || (upper && c.is_ascii_uppercase())
            })
    })
}

fn array(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}

fn strings(value: &Value) -> Option<Vec<&str>> {
    value.as_array()?.iter().map(Value::as_str).collect()
}

fn string_map(value: &Value) -> BTreeMap<&str, &str> {
    value
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(key, value)| Some((key.as_str(), value.as_str()?)))
        .collect()
}

fn nonempty(value: &Value) -> Option<&str> {
    value.as_str().filter(|text| !text.is_empty())
}

fn date(value: &Value) -> Option<&str> {
    let text = value.as_str()?;
    (text.as_bytes().get(10) == Some(&b'T') && text.parse::<jiff::Timestamp>().is_ok())
        .then_some(text)
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(yes) => *yes,
        Value::String(text) => !text.is_empty(),
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
        _ => true,
    }
}
