//! Bots' own keys, gone (PROTOCOL.md, "Bots have no keys of their own"). A bot once could have a
//! key of its own for an integration, kept as `<variable>__BOT_<ID>` and named by a server in its
//! folder's settings (`workspace/.opencode/opencode.json`). Now a bot uses the integrations of the
//! computer it lives on, as every session there does.
//!
//! So as the resident starts, with no client there, each bot's folder's settings lose the servers
//! that name such a key, and the keys go from where they were kept. An integration a bot had its
//! own key for is noted in its folder (`keys-gone.json`); while this computer hasn't that
//! integration connected, the bot is told so, to say it plainly when its work needs it.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
    time::Duration,
};

use kuvo_services::{
    config::{ConfigPaths, SERVERS, SettingsRead, map_value_at},
    integrations::{keys::KeyStore, upgrade},
};
use regex::Regex;
use serde_json::{Map, Value, json};

const GONE_FILE: &str = "keys-gone.json";

static BOT_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("KUVO_KEY_[A-Z0-9_]+__BOT_[A-Z0-9]+").expect("a pattern"));

/// A settings file's text as the engine reads it (JSONC: comments, and a comma after the last of
/// a list): its value; none for what isn't that either.
pub fn settings_value(text: &str) -> Option<Value> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if let Ok(value) = serde_json::from_str(text) {
        return Some(value);
    }
    let chars: Vec<char> = text.chars().collect();
    let line_end = |c: char| "\n\r\u{2028}\u{2029}".contains(c);
    // Past a comma: the space and comments before what comes next.
    let after = |mut at: usize| {
        loop {
            match (chars.get(at), chars.get(at + 1)) {
                (Some(c), _) if c.is_whitespace() => at += 1,
                (Some('/'), Some('/')) => {
                    while chars.get(at).is_some_and(|c| !line_end(*c)) {
                        at += 1;
                    }
                }
                (Some('/'), Some('*')) => {
                    let close = (at + 2..chars.len().saturating_sub(1))
                        .find(|i| chars[*i] == '*' && chars[i + 1] == '/');
                    match close {
                        Some(close) => at = close + 2,
                        None => return at,
                    }
                }
                _ => return at,
            }
        }
    };
    let (mut plain, mut at) = (String::new(), 0);
    while at < chars.len() {
        let c = chars[at];
        match (c, chars.get(at + 1)) {
            ('"', _) => {
                // A string, whole, with what it escapes.
                let mut end = at + 1;
                while end < chars.len() && chars[end] != '"' {
                    end += if chars[end] == '\\' { 2 } else { 1 };
                }
                plain.extend(&chars[at..(end + 1).min(chars.len())]);
                at = end;
            }
            ('/', Some('/')) => {
                while at < chars.len() && !line_end(chars[at]) {
                    at += 1;
                }
                plain.push('\n');
            }
            ('/', Some('*')) => {
                let close = (at + 2..chars.len().saturating_sub(1))
                    .find(|i| chars[*i] == '*' && chars[i + 1] == '/')?;
                at = close + 1;
                plain.push(' ');
            }
            // (One before what closes its list goes.)
            (',', _) if matches!(chars.get(after(at + 1)), Some(']' | '}')) => {}
            _ => plain.push(c),
        }
        at += 1;
    }
    serde_json::from_str(&plain).ok()
}

fn settings_file_of(folder: &Path) -> PathBuf {
    folder.join("workspace/.opencode/opencode.json")
}

/// How a bot's own keys end, by its folder (`…__BOT_<ID>`, as an earlier box named them).
fn own_suffix(folder: &Path) -> String {
    let name = folder
        .file_name()
        .map(|name| name.to_string_lossy().to_uppercase())
        .unwrap_or_default();
    let name: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    format!("__{name}")
}

/// Whether a bot's folder's settings still name a key of its own (ones that couldn't be put
/// right): its keys stay for as long, so its server goes on working, and it's told to tell you.
fn still_names_own_key(folder: &Path) -> bool {
    let file = settings_file_of(folder);
    let Some(text) = kuvo_store::files::text_of(&file) else {
        // (There, but not to be read: it may.)
        return file.exists();
    };
    let named = settings_value(&text).map_or(text, |value| value.to_string());
    let suffix = regex::escape(&own_suffix(folder));
    Regex::new(&format!("KUVO_KEY_[A-Z0-9_]+{suffix}(?:[^A-Z0-9_]|$)"))
        .is_ok_and(|own| own.is_match(&named))
}

/// The integrations a bot (its folder) had a key of its own for, as noted there.
fn keys_gone_of(folder: &Path) -> Vec<String> {
    let noted = kuvo_store::files::text_of(&folder.join(GONE_FILE));
    let noted: Value = noted
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let mut ids: Vec<String> = Vec::new();
    for id in noted["integrations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let fits = (1..=100).contains(&id.len())
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
        if fits && !ids.iter().any(|each| each == id) {
            ids.push(id.to_owned());
        }
    }
    ids
}

fn note_keys_gone(folder: &Path, ids: &[String]) -> std::io::Result<()> {
    let file = folder.join(GONE_FILE);
    if ids.is_empty() {
        return match std::fs::remove_file(&file) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    kuvo_store::files::write_secret(&file, json!({ "integrations": ids }).to_string().as_bytes())
}

/// A bot's folder's settings without the servers that name a key of its own, the rest as it was (a
/// file left empty goes): the integrations they were for, noted in its folder first. Settings that
/// can't be read are left alone.
pub fn drop_own_servers(folder: &Path) -> std::io::Result<Vec<String>> {
    let file = settings_file_of(folder);
    let Some(Value::Object(mut settings)) = kuvo_store::files::text_of(&file)
        .as_deref()
        .and_then(settings_value)
    else {
        return Ok(Vec::new());
    };
    let Some(Value::Object(servers)) = settings.get("mcp").and_then(|mcp| mcp.get("servers"))
    else {
        return Ok(Vec::new());
    };
    let gone: Vec<String> = servers
        .iter()
        .filter(|(_, server)| BOT_KEY.is_match(&server.to_string()))
        .map(|(id, _)| id.clone())
        .collect();
    if gone.is_empty() {
        return Ok(gone);
    }
    let kept: Map<String, Value> = servers
        .iter()
        .filter(|(id, _)| !gone.contains(id))
        .map(|(id, server)| (id.clone(), server.clone()))
        .collect();
    let mut mcp = settings["mcp"].as_object().cloned().unwrap_or_default();
    if kept.is_empty() {
        mcp.remove("servers");
    } else {
        mcp.insert("servers".to_owned(), Value::Object(kept));
    }
    if mcp.is_empty() {
        settings.remove("mcp");
    } else {
        settings.insert("mcp".to_owned(), Value::Object(mcp));
    }
    // Noted before its settings change: a start cut short notes it again the next time.
    let mut noted = keys_gone_of(folder);
    noted.extend(
        gone.iter()
            .filter(|id| !noted.contains(id))
            .cloned()
            .collect::<Vec<_>>(),
    );
    note_keys_gone(folder, &noted)?;
    if settings.is_empty() {
        std::fs::remove_file(&file)?;
        let _ = std::fs::remove_dir(file.parent().unwrap_or(folder));
    } else {
        let text = serde_json::to_string_pretty(&settings).map_err(std::io::Error::other)?;
        kuvo_store::files::write_secret(&file, format!("{text}\n").as_bytes())?;
    }
    Ok(gone)
}

/// Of the integrations noted for a bot, those this computer hasn't connected now; none while that
/// can't be told (its settings unreadable, or not its own yet).
fn still_to_connect(folder: &Path, config: &ConfigPaths) -> Option<(Vec<String>, Vec<String>)> {
    let noted = keys_gone_of(folder);
    if noted.is_empty() {
        return Some((noted, Vec::new()));
    }
    let SettingsRead::Ok(settings) = config.read_settings() else {
        return None;
    };
    if !config.owns_settings() {
        return None;
    }
    let left = noted
        .iter()
        .filter(|id| map_value_at(&settings, &[SERVERS[0], SERVERS[1], id]).is_none())
        .cloned()
        .collect();
    Some((noted, left))
}

/// Its note without the integrations connected since (as its run starts).
pub fn forget_connected_keys(folder: &Path, config: &ConfigPaths) {
    if let Some((noted, left)) = still_to_connect(folder, config)
        && left.len() != noted.len()
    {
        let _ = note_keys_gone(folder, &left);
    }
}

fn listed(names: &[String]) -> String {
    match names {
        [one] => one.clone(),
        [first, second] => format!("{first} and {second}"),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
        [] => String::new(),
    }
}

/// What a bot is told, after the rest, while an integration it had a key of its own for isn't
/// connected on this computer (`here`): that it's so, and to say it plainly when its work needs it;
/// and while its settings still name a key of its own. Nothing otherwise.
pub fn keys_gone_note(folder: &Path, config: &ConfigPaths, here: &str) -> String {
    let left = still_to_connect(folder, config)
        .map(|(_, left)| left)
        .unwrap_or_default();
    let catalog = if left.is_empty() {
        Vec::new()
    } else {
        upgrade::integration_catalog(
            &upgrade::integrations_file_for(&config.home, config.is_mac),
            config.is_mac,
        )
    };
    // (Only integrations this computer's catalog has: its note is a file in its own folder.)
    let names: Vec<String> = left
        .iter()
        .filter_map(|id| {
            catalog
                .iter()
                .find(|entry| &entry.id == id)
                .map(|entry| entry.title.clone())
        })
        .filter(|title| !title.is_empty())
        .collect();
    let mut note = String::new();
    if !names.is_empty() {
        let (all, one) = (listed(&names), names.len() == 1);
        let (is, it, one_of) = if one {
            ("isn't", "it", "it")
        } else {
            ("aren't", "them", "one of them")
        };
        note.push_str(&format!(
            "\n\nYou used to have a key of your own for {all}. Kuvo Code no longer keeps keys for single bots: that key was removed, and you now use the integrations connected on {here}, as every session there does. {all} {is} connected there, so you have no tools for {it} now. When your work needs {one_of}, don't look for another way around: say plainly that it isn't connected on {here}, and ask the user to connect it in Settings › Integrations."
        ));
    }
    if !upgrade::update_on_trial(&config.home) && still_names_own_key(folder) {
        note.push_str(&format!(
            "\n\nYour folder's settings file ({}) names a key of your own for one of its servers. Kuvo Code no longer keeps keys for single bots, so that server and its key are to go, but Kuvo Code couldn't change the file: it can't be read as JSON, or can't be opened or written. Tell the user so plainly the next time you talk: once the file is put right, or that server is taken out of it, the key is removed the next time Kuvo Code starts on {here}, and you use the integrations connected there, as every session does.",
            settings_file_of(folder).display()
        ));
    }
    note
}

/// Every bot's folder's settings without the servers that name a key of its own (not while an
/// update is on trial): the folders whose servers went, and how the keys end of the bots whose
/// settings couldn't be put right, which stay.
pub fn drop_bots_own_servers(home: &Path, bots: &Path) -> (Vec<PathBuf>, Vec<String>) {
    let (mut changed, mut keep) = (Vec::new(), Vec::new());
    if upgrade::update_on_trial(home) {
        return (changed, keep);
    }
    for name in crate::wakes::folder::names(bots) {
        let folder = bots.join(&name);
        match drop_own_servers(&folder) {
            Ok(gone) if !gone.is_empty() => changed.push(folder.clone()),
            Ok(_) => {}
            Err(error) => eprintln!(
                "Kuvo Code: {name}'s folder's settings couldn't be put right, so its own keys stay: {error}"
            ),
        }
        if still_names_own_key(&folder) {
            keep.push(own_suffix(&folder));
        }
    }
    (changed, keep)
}

/// Whether this Mac's box just moved into a home of its own and its app still owes the Keychain:
/// it deletes its originals only once it finds each one's copy, so no copy goes before that.
fn keychain_owed(home: &Path, is_mac: bool) -> bool {
    if !kuvo_services::config::is_own_home_mac(home, is_mac) {
        return false;
    }
    let record = kuvo_store::files::text_of(&home.join("updates/rehome/state.json"));
    let record: Value = record
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    record["movedBack"] != true
        && record["owed"]
            .as_array()
            .is_some_and(|owed| owed.iter().any(|each| each == "keychain"))
}

/// What's left here of bots' own keys gone: every bot's folder's settings first, then the keys, then
/// an earlier box's note of deleted bots' keys still to go. Whether it's all done: not while an
/// update is on trial (a key can't come back with the version before), nor while this Mac's keys
/// can't be reached or its move still owes the Keychain. `reconnect`: told each folder whose
/// servers went, for a conversation open there to take this computer's own.
pub async fn drop_bots_own_keys(
    home: &Path,
    is_mac: bool,
    bots: &Path,
    keys: &KeyStore,
    reconnect: &(dyn Fn(PathBuf, Vec<String>) + Send + Sync),
) -> bool {
    keys.key_work(|| async {
        if upgrade::update_on_trial(home) {
            return false;
        }
        let before: Vec<(PathBuf, Vec<String>)> = crate::wakes::folder::names(bots)
            .into_iter()
            .map(|name| bots.join(name))
            .map(|folder| {
                let had = keys_gone_of(&folder);
                (folder, had)
            })
            .collect();
        let (changed, keep) = drop_bots_own_servers(home, bots);
        for folder in changed {
            let had = before
                .iter()
                .find(|(each, _)| *each == folder)
                .map(|(_, had)| had.clone())
                .unwrap_or_default();
            let gone = keys_gone_of(&folder)
                .into_iter()
                .filter(|id| !had.contains(id))
                .collect();
            reconnect(folder, gone);
        }
        if keychain_owed(home, is_mac) || keys.drop_bot_keys(&keep).await.is_err() {
            return false;
        }
        let _ = std::fs::remove_file(home.join("run/bot-keys-left.json"));
        true
    })
    .await
}

/// The same as the resident runs: now, and again while this Mac's keys can't be reached (a minute
/// after the first time, twice as long after each one more, twice a day at most).
pub fn keep_bots_own_keys_gone(
    home: PathBuf,
    is_mac: bool,
    bots: PathBuf,
    keys: Arc<KeyStore>,
    reconnect: Arc<dyn Fn(PathBuf, Vec<String>) + Send + Sync>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut wait = Duration::from_secs(60);
        loop {
            if drop_bots_own_keys(&home, is_mac, &bots, &keys, reconnect.as_ref()).await
                || upgrade::update_on_trial(&home)
            {
                return;
            }
            tokio::time::sleep(wait).await;
            wait = (wait * 2).min(Duration::from_secs(12 * 3600));
        }
    })
}

/// What's done for each folder whose servers went: in a conversation open there (its server
/// started with the bot's key in memory), each connected again a moment later, as its settings are
/// now: this computer's own.
pub fn reconnecting(
    engine: kuvo_engine::Engine,
) -> Arc<dyn Fn(PathBuf, Vec<String>) + Send + Sync> {
    Arc::new(move |folder, gone| {
        let engine = engine.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let Some(engine) = engine.running_bot_engine().await else {
                return;
            };
            let workspace = folder.join("workspace").to_string_lossy().into_owned();
            for id in gone {
                let id =
                    percent_encoding::utf8_percent_encode(&id, percent_encoding::NON_ALPHANUMERIC);
                let path = format!("api/experimental/mcp/{id}/connect");
                let _ = engine
                    .call_in(
                        http::Method::POST,
                        &path,
                        &workspace,
                        Duration::from_secs(10),
                    )
                    .await;
            }
        });
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_read_as_the_engine_reads_them() {
        let text = "\u{feff}{\n  // a note\n  \"a\": [1, 2,], /* more */\n  \"b\": \"x // not a comment\",\n}";
        assert_eq!(
            settings_value(text),
            Some(json!({ "a": [1, 2], "b": "x // not a comment" }))
        );
        assert_eq!(settings_value("{ \"a\": 1 /* open"), None);
        assert_eq!(settings_value("not json"), None);
    }

    #[test]
    fn a_bots_own_servers_go_and_the_rest_stay() {
        let root = std::env::temp_dir().join(format!("kuvo-own-keys-{}", std::process::id()));
        let folder = root.join("bot_ab1");
        let file = settings_file_of(&folder);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "{\n // mine\n \"mcp\": {\"servers\": {\"github\": {\"env\": {\"T\": \"{env:KUVO_KEY_GITHUB_TOKEN__BOT_AB1}\"}}, \"docs\": {\"url\": \"x\"},}}}").unwrap();
        assert!(still_names_own_key(&folder));
        assert_eq!(
            drop_own_servers(&folder).unwrap(),
            vec!["github".to_owned()]
        );
        assert_eq!(keys_gone_of(&folder), vec!["github".to_owned()]);
        assert!(!still_names_own_key(&folder));
        let left: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(
            left,
            json!({ "mcp": { "servers": { "docs": { "url": "x" } } } })
        );
        // Another bot's key named here counts for nothing.
        std::fs::write(
            &file,
            "{\"mcp\": {\"servers\": {\"x\": {\"k\": \"KUVO_KEY_X__BOT_OTHER\"}}}}",
        )
        .unwrap();
        assert!(!still_names_own_key(&folder));
        let _ = std::fs::remove_dir_all(root);
    }
}
