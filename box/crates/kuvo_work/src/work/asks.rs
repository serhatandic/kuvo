//! What a bot asks you before it does it (PROTOCOL.md, "Work": `kuvo/work/asks`): making a
//! project, or cloning one; and what any agent asks before it uses one of your plans' resets. Kept
//! in `run/work/asks.json` until you answer, on whichever client you use.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use unicode_normalization::UnicodeNormalization;

use super::Work;
use crate::bots::here::BotHere;

const ACTIONS: &[&str] = &["makeProject", "cloneProject", "resetLimits"];

/// A project's folder name, from its name: plain letters, digits, `.`, `_` and `-`.
pub fn folder_name(name: &str) -> Option<String> {
    let folded: String = name
        .nfkd()
        .filter(|ch| !('\u{300}'..='\u{36f}').contains(ch))
        .collect::<String>()
        .to_lowercase();
    let mut dashed = String::new();
    for ch in folded.chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '.' | '_' | '-') {
            dashed.push(ch);
        } else if !dashed.ends_with('-') {
            dashed.push('-');
        }
    }
    while dashed.contains("--") {
        dashed = dashed.replace("--", "-");
    }
    let trimmed = dashed.trim_matches(|ch| ch == '-' || ch == '.');
    let cut: String = trimmed.chars().take(60).collect();
    let cut = cut.trim_end_matches(['-', '.']);
    (!cut.is_empty()).then(|| cut.to_owned())
}

/// Where a project named so would go in `root`.
pub fn project_place(name: &str, root: Option<&Path>) -> Option<PathBuf> {
    Some(root?.join(folder_name(name)?))
}

impl Work {
    pub(super) fn kept_asks(&self) -> Vec<Value> {
        let kept: Value = kuvo_store::files::text_of(&self.records.folder().join("asks.json"))
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        kept["asks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|each| {
                each["id"].is_string()
                    && each["action"]
                        .as_str()
                        .is_some_and(|action| ACTIONS.contains(&action))
            })
            .cloned()
            .collect()
    }

    /// What's asked now (not answered), as clients list them: of bots still here, and of other
    /// conversations.
    pub fn asks(&self) -> Vec<Value> {
        let root = self.settings.projects_folder();
        self.kept_asks()
            .into_iter()
            .filter(|each| {
                each.get("answered").is_none_or(Value::is_null)
                    && each["bot"]
                        .as_str()
                        .is_none_or(|bot| BotHere::read(&self.bots, bot).is_some())
            })
            .map(|kept| {
                let mut listed = kept.clone();
                let listed_map = listed.as_object_mut().expect("an ask");
                for private in ["given", "late", "answering", "answered", "toldLate", "suggested"] {
                    listed_map.remove(private);
                }
                if kept["action"] == "resetLimits" {
                    listed_map.insert("folder".into(), Value::Null);
                    let reset = &kept["reset"];
                    if reset.is_object() {
                        let on = reset["on"].as_str().map(|on| format!(", on {on}")).unwrap_or_default();
                        listed_map.insert(
                            "words".into(),
                            json!({
                                "title": format!("{} asks to use a reset", kept["name"].as_str().unwrap_or_default()),
                                "body": format!("{}: 1 of {}{on}", reset["account"].as_str().unwrap_or_default(), reset["count"]),
                            }),
                        );
                    }
                    return listed;
                }
                let named = kept["given"]["folder"].as_str().map(PathBuf::from);
                let folder = named.clone().or_else(|| {
                    project_place(kept["project"].as_str().unwrap_or_default(), root.as_deref())
                });
                listed_map.insert(
                    "folder".into(),
                    json!(folder.map(|folder| folder.display().to_string())),
                );
                if root.is_none()
                    && named.is_none()
                    && let Some(suggested) = self.settings.suggested()
                {
                    listed_map.insert("suggested".into(), json!(suggested.display().to_string()));
                }
                listed
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::folder_name;

    #[test]
    fn a_projects_folder_is_named_from_its_name() {
        assert_eq!(folder_name("Café Menu!").as_deref(), Some("cafe-menu"));
        assert_eq!(folder_name("  --My..App--  ").as_deref(), Some("my..app"));
        assert_eq!(folder_name("???"), None);
        assert_eq!(
            folder_name(&"a".repeat(80)).map(|name| name.len()),
            Some(60)
        );
    }
}
