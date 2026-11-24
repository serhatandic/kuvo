//! What still runs for each conversation besides its own turn (PROTOCOL.md, "Work › Background
//! work"), as clients read it: from `run/work/background.json`, where each place that runs work
//! keeps what it has going (the coding agents here as `bridge`); and what a command a coding agent
//! left running printed, read through the agents and cut here, so no client reads or cuts it.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

use http::StatusCode;
use kuvo_agents::background::{AT_WORK_LIMIT, Background, Helper, now};
use kuvo_engine::LocalSessions;
use kuvo_peers::setup_lists::Reader;
use kuvo_protocol::routes::{
    get_kuvo_work_background as list, get_kuvo_work_background_events as list_events,
    get_kuvo_work_background_session as one,
    get_kuvo_work_background_session_commands_id_output as output,
    get_kuvo_work_background_session_commands_id_output_events as output_events,
    get_kuvo_work_background_session_events as one_events,
};
use kuvo_server::{Access, JsonLine, Refusal, Registry, RouteReply};
use serde::Deserialize;
use serde_json::{Value, json};

/// How much of the end of what it printed is shown, in bytes.
const SHOWN_OUTPUT: u64 = 24_000;
/// How often a followed command's output is read again.
const OUTPUT_EVERY: Duration = Duration::from_secs(1);
/// How often a followed list is read again with nothing said to change (another place's writes).
const LOOK: Duration = Duration::from_secs(2);

#[derive(Default, Deserialize)]
struct Command {
    id: String,
    #[serde(default)]
    command: String,
    #[serde(default)]
    started: u64,
    folder: Option<String>,
    ports: Option<Vec<u16>>,
}

#[derive(Default, Deserialize)]
struct Work {
    #[serde(default)]
    commands: Vec<Command>,
    #[serde(default)]
    subagents: Vec<Helper>,
    #[serde(default)]
    tasks: Vec<Helper>,
}

#[derive(Default, Deserialize)]
struct Place {
    #[serde(default)]
    sessions: BTreeMap<String, Work>,
}

fn kept(file: &Path) -> BTreeMap<String, Place> {
    std::fs::read(file)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Every session here with something running besides its turn: how many of each, and since when.
fn read_list(file: &Path, now: u64) -> Value {
    let counts = |at: u64| now.saturating_sub(at) < AT_WORK_LIMIT;
    let mut sessions: BTreeMap<String, (usize, usize, usize, u64)> = BTreeMap::new();
    for place in kept(file).into_values() {
        for (id, work) in place.sessions {
            let commands: Vec<u64> = work
                .commands
                .iter()
                .map(|each| each.started)
                .filter(|at| counts(*at))
                .collect();
            let subagents: Vec<u64> = work
                .subagents
                .iter()
                .map(|each| each.since)
                .filter(|at| counts(*at))
                .collect();
            let tasks: Vec<u64> = work
                .tasks
                .iter()
                .map(|each| each.since)
                .filter(|at| counts(*at))
                .collect();
            let Some(since) = commands
                .iter()
                .chain(&subagents)
                .chain(&tasks)
                .min()
                .copied()
            else {
                continue;
            };
            let was = sessions.entry(id).or_insert((0, 0, 0, since));
            was.0 += commands.len();
            was.1 += subagents.len();
            was.2 += tasks.len();
            was.3 = was.3.min(since);
        }
    }
    let sessions: serde_json::Map<String, Value> = sessions
        .into_iter()
        .map(|(id, (commands, subagents, tasks, since))| {
            (id, json!({ "commands": commands, "subagents": subagents, "tasks": tasks, "since": since }))
        })
        .collect();
    json!({ "sessions": sessions })
}

/// What runs for `session`, each one, from every place it runs: the commands it left running (a
/// coding agent's marked `agents`), however long they've run, and the subagents at work under it.
fn read_session(file: &Path, session: &str) -> (Vec<Value>, Vec<Value>) {
    let mut commands = Vec::new();
    let mut subagents = Vec::new();
    for (place, kept) in kept(file) {
        let Some(work) = kept.sessions.get(session) else {
            continue;
        };
        for command in &work.commands {
            let mut row =
                json!({ "id": command.id, "command": command.command, "started": command.started });
            if let Some(folder) = &command.folder {
                row["folder"] = json!(folder);
            }
            if let Some(ports) = command.ports.as_ref().filter(|ports| !ports.is_empty()) {
                row["ports"] = json!(ports);
            }
            if place == "bridge" {
                row["agents"] = json!(true);
            }
            commands.push(row);
        }
        subagents.extend(work.subagents.iter().map(|each| {
            (
                each.since,
                json!({ "session": each.id, "since": each.since }),
            )
        }));
    }
    commands.sort_by_key(|row| row["started"].as_u64());
    subagents.sort_by_key(|(since, _)| *since);
    (
        commands,
        subagents.into_iter().map(|(_, row)| row).collect(),
    )
}

/// What the session lists say of the sessions a session's bars name: each subagent's title and
/// whether it waits on you, and what's at work under the session (its bot's, when it's one).
#[derive(Clone)]
pub(in crate::serve) struct Listed {
    pub sessions: LocalSessions,
    pub peers: Reader,
    pub bots: PathBuf,
}

impl Listed {
    fn session(&self, file: &Path, session: &str) -> Value {
        let (commands, subagents) = read_session(file, session);
        let list = self.sessions.latest_record().unwrap_or_default();
        let facts = |id: &str| &list["sessions"][id]["facts"];
        let bot = facts(session)["bot"].as_str().filter(|bot| {
            kuvo_work::bots::here::BotHere::read(&self.bots, bot)
                .is_some_and(|here| here.session == session)
        });
        let activity = self
            .peers
            .view()
            .activity(Some(session), bot, true, self.peers.now());
        let subagents: Vec<Value> = subagents
            .into_iter()
            .map(|mut row| {
                let facts = facts(row["session"].as_str().unwrap_or_default());
                if let Some(title) = facts["title"].as_str().filter(|title| !title.is_empty()) {
                    row["title"] = json!(title);
                }
                if facts.get("asked").is_some() {
                    row["asking"] = json!(true);
                }
                row
            })
            .collect();
        json!({ "commands": commands, "activity": activity, "subagents": subagents })
    }
}

/// What's shown of a stretch read from `cut` into the output: from its first whole line (with none,
/// all but a character the cut left half of), without newlines at either end.
fn end_of(output: &str, cut: bool) -> String {
    let trim = |text: &str| text.trim_matches(['\n', '\r']).to_owned();
    let mut shown = output;
    if cut {
        shown = match output.find(['\n', '\r']) {
            Some(at) if !trim(&output[at + 1..]).is_empty() => &output[at + 1..],
            _ => output.trim_start_matches('\u{FFFD}'),
        };
    }
    trim(shown)
}

/// The command a session lists, or one its agents kept after it ended; none for one not known.
fn command_of(file: &Path, background: &Background, session: &str, id: &str) -> Option<Value> {
    if let Some(found) = read_session(file, session)
        .0
        .into_iter()
        .find(|row| row["id"] == id)
    {
        return Some(found);
    }
    let job = background
        .command(id)
        .filter(|job| job.session == session)?;
    Some(json!({ "id": job.id, "command": job.command, "started": job.started, "agents": true }))
}

/// One read of `command`: whether it still runs, and the end of what it printed. One that isn't the
/// agents' (the engine's own) isn't read here yet: what was seen last stands.
async fn read_output(
    background: &Background,
    command: &Option<Value>,
    last: Option<&Value>,
) -> Value {
    let Some(command) = command else {
        return json!({ "command": null, "output": "", "cut": false, "state": "ended" });
    };
    let unread = |state: &str| {
        json!({
            "command": command,
            "output": last.and_then(|last| last["output"].as_str()).unwrap_or_default(),
            "cut": last.is_some_and(|last| last["cut"] == true),
            "state": state,
        })
    };
    let id = command["id"].as_str().unwrap_or_default();
    if command["agents"] != true {
        return unread("unread");
    }
    let Some(job) = background.command(id) else {
        return unread("ended");
    };
    let size = background
        .output(id, 0, Some(1))
        .await
        .map_or(0, |read| read.size);
    let start = size.saturating_sub(SHOWN_OUTPUT);
    let stretch = background
        .output(id, start, Some(SHOWN_OUTPUT))
        .await
        .map(|read| read.output)
        .unwrap_or_default();
    json!({
        "command": command,
        "output": end_of(&stretch, start > 0),
        "cut": start > 0,
        "state": if job.running { "running" } else { "ended" },
    })
}

fn typed<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, Refusal> {
    serde_json::from_value(value)
        .map_err(|error| Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
}

fn decoded(value: Option<&str>) -> String {
    percent_encoding::percent_decode_str(value.unwrap_or_default())
        .decode_utf8_lossy()
        .into_owned()
}

/// `read`, now and as it changes: when the agents say so, or a while on (another place's writes).
fn followed<F>(
    background: &Background,
    mut read: F,
    cancel: tokio_util::sync::CancellationToken,
) -> impl futures_core::Stream<Item = std::io::Result<Value>> + Send + 'static
where
    F: FnMut() -> Value + Send + 'static,
{
    let mut changes = background.changes();
    async_stream::try_stream! {
        let mut previous: Option<Value> = None;
        loop {
            let value = read();
            if previous.as_ref() != Some(&value) {
                previous = Some(value.clone());
                yield value;
            }
            tokio::select! {
                () = cancel.cancelled() => break,
                _ = changes.changed() => {}
                () = tokio::time::sleep(LOOK) => {}
            }
        }
    }
}

pub(in crate::serve) fn register(
    home: &Path,
    background: Background,
    listed: Listed,
    routes: &mut Registry,
) {
    let file: PathBuf = home.join("run/work/background.json");
    let here = file.clone();
    routes.register::<list::Spec, _, _>(Access::Any, move |_| {
        let file = here.clone();
        async move { Ok(RouteReply::json::<200>(typed(read_list(&file, now()))?)) }
    });
    let (here, agents) = (file.clone(), background.clone());
    routes.register::<list_events::Spec, _, _>(Access::Any, move |call| {
        let (file, agents) = (here.clone(), agents.clone());
        async move {
            let lines = followed(
                &agents,
                move || read_list(&file, now()),
                call.cancel.clone(),
            );
            Ok(RouteReply::ndjson::<200>(async_stream::try_stream! {
                for await value in lines {
                    yield JsonLine::new(serde_json::from_value(value?)?)?;
                }
            }))
        }
    });
    let (here, lists) = (file.clone(), listed.clone());
    routes.register::<one::Spec, _, _>(Access::Any, move |call| {
        let (file, listed) = (here.clone(), lists.clone());
        async move {
            let session = decoded(call.param("session"));
            Ok(RouteReply::json::<200>(typed(
                listed.session(&file, &session),
            )?))
        }
    });
    let (here, agents) = (file.clone(), background.clone());
    routes.register::<one_events::Spec, _, _>(Access::Any, move |call| {
        let (file, agents, listed) = (here.clone(), agents.clone(), listed.clone());
        async move {
            let session = decoded(call.param("session"));
            let lines = followed(
                &agents,
                move || listed.session(&file, &session),
                call.cancel.clone(),
            );
            Ok(RouteReply::ndjson::<200>(async_stream::try_stream! {
                for await value in lines {
                    yield JsonLine::new(serde_json::from_value(value?)?)?;
                }
            }))
        }
    });
    let (here, agents) = (file.clone(), background.clone());
    routes.register::<output::Spec, _, _>(Access::Any, move |call| {
        let (file, agents) = (here.clone(), agents.clone());
        async move {
            let session = decoded(call.param("session"));
            let id = decoded(call.param("id"));
            let command = command_of(&file, &agents, &session, &id);
            Ok(RouteReply::json::<200>(typed(
                read_output(&agents, &command, None).await,
            )?))
        }
    });
    routes.register::<output_events::Spec, _, _>(Access::Any, move |call| {
        let (file, agents) = (file.clone(), background.clone());
        async move {
            let session = decoded(call.param("session"));
            let id = decoded(call.param("id"));
            let command = command_of(&file, &agents, &session, &id);
            let cancel = call.cancel.clone();
            // A line now and again about once a second while it's followed, each time it says
            // something new; once it says it ended, that line's the last.
            Ok(RouteReply::ndjson::<200>(async_stream::try_stream! {
                let mut last: Option<Value> = None;
                loop {
                    let now = read_output(&agents, &command, last.as_ref()).await;
                    let ended = now["state"] == "ended";
                    if last.as_ref() != Some(&now) {
                        yield JsonLine::new(serde_json::from_value(now.clone())?)?;
                    }
                    last = Some(now);
                    if ended {
                        break;
                    }
                    tokio::select! {
                        () = cancel.cancelled() => break,
                        () = tokio::time::sleep(OUTPUT_EVERY) => {}
                    }
                }
            }))
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stretch_cut_from_the_output_shows_from_its_first_whole_line() {
        assert_eq!(
            end_of("\nCompiling\nFinished\n", false),
            "Compiling\nFinished"
        );
        assert_eq!(end_of("ling\nFinished\n", true), "Finished");
        assert_eq!(end_of("\u{FFFD}half a line", true), "half a line");
    }

    #[test]
    fn the_lists_count_each_place_and_leave_out_what_ran_out() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("background.json");
        let now = 10 * AT_WORK_LIMIT;
        std::fs::write(&file, json!({
            "bridge": { "sessions": { "ses_a": { "commands": [{ "id": "shl_1", "command": "npm run dev", "started": now - 5 }], "subagents": [], "tasks": [{ "id": "t", "since": now - AT_WORK_LIMIT }] } } },
            "engine": { "sessions": { "ses_a": { "commands": [{ "id": "c2", "command": "make", "started": now - 9, "folder": "/w", "ports": [5173] }], "subagents": [{ "id": "ses_sub", "since": now - 2 }], "tasks": [] } } }
        }).to_string()).unwrap();
        assert_eq!(
            read_list(&file, now),
            json!({ "sessions": { "ses_a": { "commands": 2, "subagents": 1, "tasks": 0, "since": now - 9 } } })
        );
        let (commands, subagents) = read_session(&file, "ses_a");
        assert_eq!(
            commands,
            [
                json!({ "id": "c2", "command": "make", "started": now - 9, "folder": "/w", "ports": [5173] }),
                json!({ "id": "shl_1", "command": "npm run dev", "started": now - 5, "agents": true }),
            ]
        );
        assert_eq!(
            subagents,
            [json!({ "session": "ses_sub", "since": now - 2 })]
        );
    }
}
