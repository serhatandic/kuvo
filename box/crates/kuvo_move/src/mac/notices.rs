use crate::{Error, entries, metadata};
use kuvo_store::format::Fields;
use serde_json::Value;
use std::{collections::BTreeSet, path::Path};

pub(super) fn append(
    source: &Path,
    prefs: &Fields,
    dropped: &mut Vec<String>,
) -> Result<(), Error> {
    let count = |key| match prefs.get(key) {
        Some(Value::Object(value)) => value.len(),
        Some(Value::Array(value)) => value.len(),
        _ => 0,
    };
    let keys = |key| {
        prefs
            .get(key)
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|value| value.keys().map(String::as_str))
    };
    let strings = |key| {
        prefs
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
    };
    match count("composerDrafts") {
        0 => {}
        1 => dropped.push("A message you'd typed but not sent wasn't brought along.".into()),
        n => dropped.push(format!(
            "{n} messages you'd typed but not sent weren't brought along."
        )),
    }
    if metadata(&source.join("Browser/history.json"))?.is_some() {
        dropped.push("Websites you were signed in to in Kuvo Code's browser need signing in again. Your history comes along.".into());
    }
    match keys("botEditsWaiting").chain(keys("botEditOutbox")).collect::<BTreeSet<_>>().len() {
        0 => {}
        1 => dropped.push("A change you made to a bot on a machine that was away (a name, look, personality or model) wasn't sent there. Make it again once that machine is connected.".into()),
        n => dropped.push(format!("{n} changes you made to bots on a machine that was away (a name, look, personality or model) weren't sent there. Make them again once that machine is connected.")),
    }
    let terminals = prefs
        .get("projectSettings")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|map| map.values())
        .filter(|value| {
            value["terminal"]
                .as_str()
                .is_some_and(|text| !text.is_empty())
        })
        .count();
    match terminals {
        0 => {}
        1 => dropped
            .push("The command that opened in place of the chat for a project isn't kept.".into()),
        n => dropped.push(format!(
            "The command that opened in place of the chat for {n} projects isn't kept."
        )),
    }
    let moves = prefs
        .get("movesUnderWay")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|value| value["stage"] == "switched")
        .count();
    match moves {
        0 => {}
        1 => dropped.push("A move to another computer that hadn't finished cleaning up left its original folder where it was.".into()),
        n => dropped.push(format!("{n} moves to another computer that hadn't finished cleaning up left their original folder where it was.")),
    }
    match count("machineSignOuts") {
        0 => {}
        1 => dropped.push("You signed out of an account while a machine was away: it may still be signed in to it. Sign out there too once it's connected.".into()),
        n => dropped.push(format!("You signed out of an account while {n} machines were away: they may still be signed in to it. Sign out there too once they're connected.")),
    }
    match count("machines.waiting") {
        0 => {}
        1 => dropped.push("A machine that was away still has files of something you removed here (a bot, or Kuvo Code itself): remove them from there once connected.".into()),
        n => dropped.push(format!("{n} machines that were away still have files of something you removed here (a bot, or Kuvo Code itself): remove them from there once connected.")),
    }
    let remote = |folder: &&str| folder.starts_with("ssh://");
    let projects = keys("projectSettings")
        .chain(strings("projectFolders"))
        .filter(remote)
        .collect::<BTreeSet<_>>()
        .len();
    match projects {
        0 => {}
        1 => dropped.push("A project on your machines may have lost its name, extra folders or Run command, which the earlier Kuvo Code kept on this Mac. Set them again in each project's settings.".into()),
        n => dropped.push(format!("{n} projects on your machines may have lost their name, extra folders or Run command, which the earlier Kuvo Code kept on this Mac. Set them again in each project's settings.")),
    }
    let worktrees = ["worktrees", "removedWorktrees"]
        .into_iter()
        .map(|key| keys(key).chain(strings(key)).filter(remote).count())
        .sum::<usize>();
    match worktrees {
        0 => {}
        1 => dropped.push("A worktree on your machines may show its sessions under another project: the earlier Kuvo Code kept which project it belonged to on this Mac.".into()),
        n => dropped.push(format!("{n} worktrees on your machines may show their sessions under another project: the earlier Kuvo Code kept which project each belonged to on this Mac.")),
    }
    if prefs
        .keys()
        .any(|key| key.starts_with("machines.unreadable-"))
    {
        dropped.push("The earlier Kuvo Code had set aside a list of machines it couldn't read. It isn't brought along: add any machine that's missing again.".into());
    }
    if strings("kuvo.usage.connectedProviders.v1").any(|provider| {
        let lower = provider.to_ascii_lowercase();
        ["openrouter", "zai", "z.ai"]
            .iter()
            .any(|name| lower.contains(name))
    }) {
        dropped.push("An API key you pasted into the usage panel (OpenRouter's or Z.ai's) isn't brought along. Paste it again in the usage panel.".into());
    }
    if prefs.contains_key("lastModelRef") || prefs.contains_key("lastModel") {
        dropped.push("The model new sessions start with is chosen again in the new app.".into());
    }
    if [
        "notifications.enabled",
        "notifications.sound",
        "notifications.badge",
    ]
    .iter()
    .any(|key| prefs.get(*key).and_then(Value::as_bool) == Some(false))
    {
        dropped.push("You had turned off some of Kuvo Code's notifications (or their sound or badge). Set that again in the new app.".into());
    }
    if prefs.get("analytics.enabled").and_then(Value::as_bool) == Some(false) {
        dropped.push("You had turned off sharing anonymous usage in the earlier Kuvo Code. That choice isn't brought along: check it in the new app's settings.".into());
    }
    if entries(source)?.iter().any(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        name.contains(".unreadable-") && name.ends_with(".json")
    }) {
        dropped.push("The earlier Kuvo Code had set aside a list it couldn't read. It's still in its folder (a file named …unreadable…json).".into());
    }
    for name in ["openrouter.json", "zai.json", "proxy.json"] {
        if metadata(&source.join("Usage").join(name))?.is_some() {
            dropped.push("API keys or a proxy you set up by hand for the usage panel (files in the earlier Kuvo Code's Usage folder) aren't brought along.".into());
            break;
        }
    }
    Ok(())
}
