use std::{
    fs, io,
    path::{Component, Path},
};

use serde::Serialize;
use serde_json::{Value, json};

use super::{Found, Legacy, OWN_BOX_RAN, Ours, Verdict, card, describe, key, name, time, verdict};
use crate::model::Role;

#[derive(Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Keeper {
    Running,
    Stopped,
    Gone,
}

const USAGE: &str = "kuvo-box here needs KUVO_HOME, the app's own box's folder, and the machines' folder's name: here <root>.";

/// Read the two named installation folders before either is opened for writing.
pub fn locate(
    account: &Path,
    root: &Path,
    app: &Path,
    engine_data: Option<&Path>,
) -> Result<Value, String> {
    if root.as_os_str().is_empty()
        || root
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || !account.is_absolute()
        || !app.is_absolute()
    {
        return Err(USAGE.into());
    }
    let machine = account.join(root);
    if machine == app
        || fs::canonicalize(&machine)
            .ok()
            .zip(fs::canonicalize(app).ok())
            .is_some_and(|(machine, app)| machine == app)
    {
        return Err(USAGE.into());
    }
    if moving(&machine) {
        return Ok(json!({"verdict": "moving", "use": "wait", "opens": "main",
            "runs": null, "box": null, "own": null}));
    }
    let app_folder = display_path(app, account);
    let app_box = app_ran(app, engine_data)
        .then(|| read_found(app, &app_folder, true))
        .transpose()?;
    let machine_folder = display_path(&machine, account);
    let other = read_found(&machine, &machine_folder, false)?;
    let known_machine = other.card.is_some()
        || other.legacy.is_some()
        || fs::read_to_string(machine.join("box/kind")).is_ok_and(|kind| kind.trim() == "mac");
    let runs = known_machine.then(|| keeper(&machine));
    let own = app_box
        .as_ref()
        .map(|own| describe(own, Verdict::ThisComputer));
    let own_card = app_box.as_ref().and_then(|own| own.card.as_ref());
    let own_opens = if own_card.is_some_and(|card| card.role == Some(Role::Member)) {
        "window"
    } else {
        "main"
    };
    if !known_machine || runs == Some(Keeper::Gone) {
        return Ok(json!({"verdict": "free", "use": "own", "opens": own_opens,
            "runs": runs, "box": null, "own": own}));
    }

    let own_key = own_card.and_then(|card| card.key.as_deref());
    let ours = Ours {
        key: own_key,
        setup: own_card.and_then(|card| card.setup.as_deref()),
        keys: own_key.into_iter().collect(),
    };
    let judged = verdict(&other, &ours);
    let shown = describe(&other, judged);
    let (judged, action, opens) = if own.is_some() {
        let combined = match judged {
            Verdict::Ours | Verdict::Free => judged,
            _ => Verdict::Two,
        };
        (combined, "own", own_opens)
    } else {
        let opens = if shown.role == Some(Role::Member)
            || matches!(judged, Verdict::Older | Verdict::Elsewhere)
        {
            "window"
        } else {
            "main"
        };
        (judged, "attach", opens)
    };
    Ok(
        json!({"verdict": judged, "use": action, "opens": opens, "runs": runs, "box": shown, "own": own}),
    )
}

fn display_path(folder: &Path, account: &Path) -> String {
    folder
        .strip_prefix(account)
        .unwrap_or(folder)
        .to_string_lossy()
        .replace('\\', "/")
}

fn moving(machine: &Path) -> bool {
    match fs::read(machine.join("updates/rehome/journal.json")) {
        Ok(data) => !serde_json::from_slice::<Value>(&data)
            .is_ok_and(|value| value["v"] == 1 && value["step"] == "done"),
        Err(error) => error.kind() != io::ErrorKind::NotFound,
    }
}

fn app_ran(app: &Path, engine_data: Option<&Path>) -> bool {
    OWN_BOX_RAN
        .iter()
        .any(|marker| app.join("run").join(marker).exists())
        || engine_data
            .and_then(|folder| fs::read_dir(folder).ok())
            .is_some_and(|entries| {
                entries.flatten().any(|entry| {
                    entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| name.starts_with("opencode") && name.ends_with(".db"))
                })
            })
}

fn read_found<'a>(folder: &Path, label: &'a str, app: bool) -> Result<Found<'a>, String> {
    let read = |file: &str| {
        fs::read(folder.join("run").join(file))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
    };
    let card = read("setup-card.json")
        .map(|value| card(&value))
        .transpose()?
        .flatten();
    let legacy = read("link.json").and_then(|value| {
        if !value.is_object() {
            return None;
        }
        let mut link = Legacy {
            phones: value["phones"]
                .as_array()
                .is_some_and(|rows| !rows.is_empty()),
            ..Legacy::default()
        };
        for key in [value.get("mac")]
            .into_iter()
            .flatten()
            .chain(value["macs"].as_array().into_iter().flatten())
        {
            if let Some(key) = key.as_str() {
                link.read("macs", key, None);
            }
        }
        if let Some(fields) = value["macSeen"].as_object() {
            link.seen.extend(
                fields
                    .iter()
                    .filter(|(k, _)| key(k))
                    .take(16)
                    .filter_map(|(k, v)| Some((k.clone(), time(v)?))),
            );
        }
        if let Some(fields) = value["macNames"].as_object() {
            link.names
                .extend(
                    fields
                        .iter()
                        .filter(|(k, _)| key(k))
                        .take(16)
                        .filter_map(|(k, v)| {
                            let name = name(v);
                            (!name.is_empty()).then(|| (k.clone(), name))
                        }),
                );
        }
        Some(link)
    });
    Ok(Found {
        folder: label,
        app,
        card,
        legacy,
    })
}

fn keeper(machine: &Path) -> Keeper {
    if !machine.join("bin/kuvo-start").exists() || machine.join("run/off").exists() {
        return Keeper::Gone;
    }
    let pid = fs::read_to_string(machine.join("run/box.pid"))
        .ok()
        .and_then(|pid| pid.trim().parse::<u32>().ok())
        .filter(|pid| *pid > 1);
    if pid.is_some_and(keeper_running) {
        Keeper::Running
    } else {
        Keeper::Stopped
    }
}

#[cfg(unix)]
fn keeper_running(pid: u32) -> bool {
    std::process::Command::new("/bin/ps")
        .args(["-ww", "-p", &pid.to_string(), "-o", "command="])
        .output()
        .is_ok_and(|reply| {
            reply.status.success() && String::from_utf8_lossy(&reply.stdout).contains("kuvo-start")
        })
}

#[cfg(windows)]
fn keeper_running(_pid: u32) -> bool {
    // Legacy kuvo-start's shell PID is only recognized on Unix.
    false
}
