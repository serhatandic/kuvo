//! A saved coding-agent conversation as clients read it: its header with the
//! computer's "Summarize long conversations at" setting where its agent takes
//! it (PROTOCOL.md, "Session context").

use crate::accounts::{AgentKind, Provider};
use kuvo_store::files;
use serde_json::{Map, Value, json};
use std::path::Path;

/// The computer's choice, as saved in `box/conversation-compaction.json`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compaction {
    At(u32),
    Never,
}

pub const CHOICES: [Compaction; 4] = [
    Compaction::At(200_000),
    Compaction::At(400_000),
    Compaction::At(600_000),
    Compaction::Never,
];
const DEFAULT: Compaction = Compaction::At(400_000);

impl Compaction {
    pub fn id(self) -> String {
        match self {
            Self::At(tokens) => tokens.to_string(),
            Self::Never => "never".into(),
        }
    }

    fn title(self) -> String {
        match self {
            Self::At(tokens) => format!("{}k", tokens / 1000),
            Self::Never => "Never".into(),
        }
    }

    /// The saved choice, or the default for a missing or unreadable file.
    pub fn read(home: &Path) -> Self {
        files::read(&home.join("box/conversation-compaction.json"))
            .ok()
            .flatten()
            .and_then(|bytes| serde_json::from_slice::<String>(&bytes).ok())
            .and_then(|saved| CHOICES.into_iter().find(|choice| choice.id() == saved))
            .unwrap_or(DEFAULT)
    }
}

/// The agents that summarize at this computer's choice.
fn chooses(kind: AgentKind) -> bool {
    matches!(kind, AgentKind::Claude | AgentKind::Codex | AgentKind::Grok)
}

fn setting(choice: Compaction) -> Value {
    let choices = CHOICES
        .into_iter()
        .map(|each| {
            let mut row = json!({ "id": each.id(), "title": each.title() });
            if each == DEFAULT {
                row["line"] = json!("Default");
            }
            row
        })
        .collect::<Vec<_>>();
    json!({
        "value": choice.id(),
        "label": "Summarize at",
        "choices": choices,
        "line": "All conversations",
    })
}

/// Where a conversation on a model holding `window` tokens is summarized by
/// itself, or `None` where its agent decides instead.
pub fn summarize_at(kind: AgentKind, window: Option<u64>, choice: Compaction) -> Option<u64> {
    if !chooses(kind) {
        return None;
    }
    let at = match choice {
        Compaction::At(tokens) => u64::from(tokens),
        Compaction::Never if kind == AgentKind::Claude && window.is_none_or(|w| w < 900_000) => {
            400_000
        }
        Compaction::Never => return None,
    };
    // Each agent's own margin wins near the window (Codex 90%, Grok 85%).
    match window {
        Some(window) if window > 0 && at as f64 > window as f64 * 0.85 => None,
        _ => Some(at),
    }
}

/// The header with what a client shows for it.
pub fn for_clients(mut header: Map<String, Value>, home: &Path) -> Map<String, Value> {
    let kind = header
        .get("contextModel")
        .or_else(|| header.get("model"))
        .and_then(|model| model["providerID"].as_str())
        .and_then(Provider::of_model)
        .map(|provider| provider.kind);
    if let Some(time) = header.get_mut("time").and_then(Value::as_object_mut)
        && !time.contains_key("activity")
        && let Some(at) = time.get("idle").or_else(|| time.get("created")).cloned()
    {
        time.insert("activity".into(), at);
    }
    let Some(kind) = kind.filter(|kind| chooses(*kind)) else {
        return header;
    };
    let choice = Compaction::read(home);
    if let Some(context) = header.get_mut("context").and_then(Value::as_object_mut)
        && let Some(at) = summarize_at(kind, context.get("window").and_then(Value::as_u64), choice)
    {
        context.insert("summarizeAt".into(), at.into());
    }
    header.insert("summarize".into(), setting(choice));
    header
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizing_follows_each_agents_margin() {
        let at = Compaction::At(400_000);
        assert_eq!(
            summarize_at(AgentKind::Codex, Some(1_000_000), at),
            Some(400_000)
        );
        assert_eq!(summarize_at(AgentKind::Codex, Some(400_000), at), None);
        assert_eq!(summarize_at(AgentKind::Cursor, None, at), None);
        let never = Compaction::Never;
        assert_eq!(summarize_at(AgentKind::Claude, Some(200_000), never), None);
        assert_eq!(
            summarize_at(AgentKind::Claude, Some(500_000), never),
            Some(400_000)
        );
        assert_eq!(
            summarize_at(AgentKind::Claude, Some(1_000_000), never),
            None
        );
        assert_eq!(summarize_at(AgentKind::Grok, None, never), None);
    }

    #[test]
    fn an_unknown_saved_choice_reads_as_the_default() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(Compaction::read(home.path()), DEFAULT);
        std::fs::create_dir_all(home.path().join("box")).unwrap();
        let file = home.path().join("box/conversation-compaction.json");
        std::fs::write(&file, "\"600000\"").unwrap();
        assert_eq!(Compaction::read(home.path()), Compaction::At(600_000));
        std::fs::write(&file, "600000").unwrap();
        assert_eq!(Compaction::read(home.path()), DEFAULT);
    }
}
