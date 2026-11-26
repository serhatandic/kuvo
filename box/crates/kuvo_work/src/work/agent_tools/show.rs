//! The show tools answer from the named file; clients draw the tool call in
//! the conversation itself. No browser, engine or artifact copy is needed.
use super::{Said, refused, said};
use serde_json::{Map, Value};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

pub(super) fn is_show(name: &str) -> bool {
    matches!(
        name,
        "show_in_panel" | "show_file" | "mark_final" | "show_widget"
    )
}
fn full(path: &str) -> bool {
    let path = Path::new(path);
    path.is_absolute()
        && !path
            .to_string_lossy()
            .split(['/', std::path::MAIN_SEPARATOR])
            .any(|part| matches!(part, "." | ".."))
}
fn expand(given: &str, home: &Path) -> PathBuf {
    if given == "~" {
        return home.to_owned();
    }
    if let Some(rest) = given.strip_prefix("~/") {
        return home.join(rest);
    }
    PathBuf::from(given)
}
fn resolved(base: &Path, given: &Path) -> PathBuf {
    let mut path = if given.is_absolute() {
        PathBuf::new()
    } else {
        base.to_owned()
    };
    for component in given.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                path.pop();
            }
            part => path.push(part.as_os_str()),
        }
    }
    path
}
fn trimmed(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', std::path::MAIN_SEPARATOR]);
    if trimmed.is_empty() { path } else { trimmed }
}

pub(super) fn answer(
    name: &str,
    args: &Map<String, Value>,
    base: Option<&Path>,
    in_view: Option<bool>,
) -> Said {
    let raw = args.get("path").and_then(Value::as_str).unwrap_or_default();
    if name == "show_widget" {
        return if args
            .get("html")
            .and_then(Value::as_str)
            .is_some_and(|h| !h.trim().is_empty())
        {
            said(
                "Shown in the conversation, live. The user sees it now; what they click in it that calls kuvo.send comes to you as their message.",
            )
        } else {
            refused("Missing html.")
        };
    }
    let home = kuvo_os::platform::Account::current()
        .and_then(|account| {
            account.home_folder(
                kuvo_os::platform::Platform::current(),
                &std::env::vars_os().collect(),
            )
        })
        .unwrap_or_default();
    match name {
        "show_in_panel" => {
            if raw.is_empty() {
                return refused("Missing path.");
            }
            let expanded = expand(trimmed(raw), &home);
            let given = expanded.to_string_lossy();
            let path = if full(&given) {
                expanded.clone()
            } else {
                resolved(base.unwrap_or(&home), &expanded)
            };
            let path = path.to_string_lossy();
            if path != trimmed(raw) {
                return refused(format!("Give the full path: {path}"));
            }
            let Ok(found) = fs::metadata(path.as_ref()) else {
                return refused(format!("There's nothing at {path}."));
            };
            let kind = if found.is_dir() { "folder" } else { "file" };
            let what = format!("the {kind} {path}");
            said(match in_view {
                Some(true) => format!(
                    "Shown: {what} is open in Kuvo Code's side panel, beside this conversation."
                ),
                Some(false) => format!(
                    "Kept for later: {what} opens in Kuvo Code's side panel when the user comes back to this conversation. They're in another one right now, so don't say it's open."
                ),
                None => format!(
                    "Sent: {what} opens in Kuvo Code's side panel beside this conversation, at once if the user is looking at it, or else when they come back to it."
                ),
            })
        }
        "show_file" => {
            if raw.is_empty() {
                return refused("Missing path.");
            }
            if !full(raw) {
                return refused(if raw.starts_with('~') {
                    format!(
                        "Give the file's full path: {}",
                        expand(raw, &home).display()
                    )
                } else {
                    "Give the file's full path.".into()
                });
            }
            let Ok(found) = fs::metadata(raw) else {
                return refused(format!("There's nothing at {raw}."));
            };
            if found.is_dir() {
                return refused(format!(
                    "{raw} is a folder: show a file in it, or the folder with show_in_panel."
                ));
            }
            said(format!(
                "Shown in the conversation: the file {raw}. The user sees it there, on their Mac and their phone."
            ))
        }
        "mark_final" => {
            if !full(raw) {
                return refused("Give the file's full path.");
            }
            if fs::metadata(raw).map_or(true, |m| m.is_dir()) {
                return refused(format!("There's no file at {raw}."));
            }
            said(format!(
                "Marked final: {raw}. It's kept at the top of what you made, for the user."
            ))
        }
        _ => refused(format!("There's no tool {name}.")),
    }
}
