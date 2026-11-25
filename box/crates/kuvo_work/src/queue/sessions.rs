use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

/// Work Queue views use the engine's in-memory session catalog.
#[derive(Default, Deserialize)]
pub(super) struct Sessions {
    sessions: HashMap<String, Row>,
}

#[derive(Deserialize)]
struct Row {
    facts: Facts,
}

#[derive(Deserialize)]
struct Facts {
    title: Option<String>,
    #[serde(default)]
    line: Vec<String>,
    #[serde(default)]
    running: bool,
}

impl Sessions {
    pub fn read(snapshot: Option<Value>) -> Self {
        snapshot
            .and_then(|snapshot| serde_json::from_value(snapshot).ok())
            .unwrap_or_default()
    }

    pub fn known_title(&self, id: &str) -> Option<&str> {
        self.sessions
            .get(id)
            .and_then(|row| row.facts.title.as_deref())
            .filter(|title| !title.is_empty())
    }

    pub fn title(&self, id: &str) -> &str {
        self.known_title(id).unwrap_or("New Session")
    }

    pub fn started_by(&self, id: &str) -> Option<&str> {
        self.sessions.get(id)?.facts.line.last().map(String::as_str)
    }

    pub fn under<'a>(&'a self, root: &str, mut id: &'a str) -> bool {
        for _ in 0..20 {
            let Some(row) = self.sessions.get(id) else {
                return false;
            };
            if row.facts.line.iter().any(|ancestor| ancestor == root) {
                return true;
            }
            let Some(parent) = row.facts.line.last() else {
                return false;
            };
            id = parent;
        }
        false
    }

    pub fn running(&self, id: &str) -> bool {
        self.sessions.get(id).is_some_and(|row| row.facts.running)
    }
}
