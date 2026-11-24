//! A bot that lives here: its folder, its description (`bot.json`, as the Mac's app wrote it or as
//! it's changed here), its conversation, and whether that runs with the coding agents.

use std::{
    io,
    path::{Path, PathBuf},
    time::Duration,
};

use serde_json::{Map, Value};

use crate::wakes::folder::text;

/// The version of `bot.json` this reads (the app's `Bot.Descriptor`).
pub const DESCRIPTION: u64 = 1;

/// A bot's id, as its folder is named.
pub fn bot_id(id: &str) -> bool {
    id.strip_prefix("bot_")
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric()))
}

#[derive(Clone, Debug)]
pub struct BotHere {
    pub id: String,
    pub folder: PathBuf,
    /// Its description whole: what this reads of it, and the rest as it's kept.
    pub described: Map<String, Value>,
    pub session: String,
    pub agents: bool,
}

impl BotHere {
    /// The bot `id` in `bots`, as its folder has it now; none without a description or a
    /// conversation.
    pub fn read(bots: &Path, id: &str) -> Option<Self> {
        if !bot_id(id) {
            return None;
        }
        let folder = bots.join(id);
        let described: Map<String, Value> =
            serde_json::from_str(&kuvo_store::files::text_of(&folder.join("bot.json"))?).ok()?;
        let session = text(&folder.join("session")).trim().to_owned();
        if described.get("kuvo").and_then(Value::as_u64) != Some(DESCRIPTION)
            || described.get("id").and_then(Value::as_str) != Some(id)
            || !described.get("name").is_some_and(Value::is_string)
            || session.is_empty()
        {
            return None;
        }
        Some(Self {
            id: id.to_owned(),
            agents: text(&folder.join("via")).trim() == "bridge",
            folder,
            described,
            session,
        })
    }

    pub fn name(&self) -> &str {
        self.described
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    pub fn field(&self, name: &str) -> Option<&str> {
        self.described.get(name).and_then(Value::as_str)
    }
}

/// A bot's description written as `change` makes it from what's there now, one process at a
/// time (an Edit Bot save and an "Always" answered at once both land).
pub fn change_description(
    folder: &Path,
    change: impl FnOnce(Option<Map<String, Value>>) -> io::Result<Option<Map<String, Value>>>,
    durable: bool,
) -> io::Result<()> {
    let _lock =
        kuvo_store::locks::OwnedLock::acquire(&folder.join(".bot.lock"), Duration::from_secs(10))?;
    let file = folder.join("bot.json");
    let now = kuvo_store::files::text_of(&file).and_then(|text| serde_json::from_str(&text).ok());
    let offline = now
        .as_ref()
        .is_some_and(|now: &Map<String, Value>| now.contains_key("offlineEdits"));
    let Some(next) = change(now)? else {
        return Ok(());
    };
    let bytes = serde_json::to_vec_pretty(&Value::Object(next))?;
    if durable || offline {
        kuvo_store::files::write_atomic_through(&file, &bytes, folder)
    } else {
        let fresh = folder.join("bot.json.new");
        std::fs::write(&fresh, &bytes)?;
        std::fs::rename(&fresh, &file)
    }
}

/// A name as one line of plain text, as the Mac's name field has it.
pub fn plain_name(text: &str) -> String {
    text.chars()
        .map(|ch| {
            let hidden = matches!(
                ch,
                '\u{0}'..='\u{1f}'
                    | '\u{7f}'
                    | '\u{200e}'
                    | '\u{200f}'
                    | '\u{202a}'..='\u{202e}'
                    | '\u{2066}'..='\u{2069}'
            );
            if hidden { ' ' } else { ch }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
