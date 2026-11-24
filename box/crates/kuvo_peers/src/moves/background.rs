use std::{collections::BTreeMap, path::Path};

use serde::Deserialize;

use crate::reach::Millis;

const WORK_LIMIT: Millis = 6 * 3_600_000;

#[derive(Deserialize, Default)]
#[serde(default)]
struct WorkPlace {
    sessions: BTreeMap<String, Work>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Work {
    commands: Vec<Command>,
    subagents: Vec<Helper>,
    tasks: Vec<Helper>,
}

#[derive(Deserialize)]
struct Command {
    started: Millis,
}

#[derive(Deserialize)]
struct Helper {
    since: Millis,
}

/// Commands and helpers on the source would be interrupted by a move.
pub(super) fn refusal<'a>(
    run: &Path,
    sessions: impl IntoIterator<Item = &'a str>,
    name: &str,
    now: Millis,
) -> Option<String> {
    let data: BTreeMap<String, WorkPlace> = crate::kept::read(&run.join("work/background.json"))?;
    let counts = sessions.into_iter().flat_map(|session| {
        data.values()
            .filter_map(move |place| place.sessions.get(session))
    });
    let live = |at| now.saturating_sub(at) < WORK_LIMIT;
    let mut commands = 0;
    let mut helpers = 0;
    for work in counts {
        commands += work.commands.iter().filter(|job| live(job.started)).count();
        helpers += work
            .subagents
            .iter()
            .chain(&work.tasks)
            .filter(|job| live(job.since))
            .count();
    }
    let active = [
        work_count(commands, "a command", "commands"),
        work_count(helpers, "a subagent", "subagents"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    (!active.is_empty()).then(|| {
        format!(
            "{name} has {} running. Wait for it to finish, or stop it, then move {name}.",
            active.join(" and ")
        )
    })
}

fn work_count(count: usize, one: &str, many: &str) -> Option<String> {
    match count {
        0 => None,
        1 => Some(one.into()),
        _ => Some(format!("{count} {many}")),
    }
}
