//! The conversation's line of waiting messages, kept in `agent.json`.

use super::{SessionStore, read_agent, read_header, slots};
use crate::inbox::Input;
use kuvo_store::files;
use serde_json::{Map, Value};
use std::io::{self, ErrorKind};

/// The saved line and what goes with it, read and written together.
pub struct Line {
    pub inputs: Vec<Input>,
    /// Handoff tokens whose late POST must not arrive (`cancelledPromptHandoffs`).
    pub cancelled_handoffs: Map<String, Value>,
    seen: Vec<(String, String)>,
}

impl Line {
    pub fn find(&self, id: &str) -> Option<&Input> {
        self.inputs.iter().find(|input| input.id == id)
    }

    pub fn find_mut(&mut self, id: &str) -> Option<&mut Input> {
        self.inputs.iter_mut().find(|input| input.id == id)
    }

    pub fn take(&mut self, id: &str) -> Option<Input> {
        let at = self.inputs.iter().position(|input| input.id == id)?;
        Some(self.inputs.remove(at))
    }

    /// The agent took this one partway through its turn (`seenSteers`), so
    /// a handover later doesn't give it the message again.
    pub fn note_seen_steer(&mut self, provider: &str, id: &str) {
        self.seen.push((crate::accounts::Provider::slot(provider), id.to_owned()));
    }
}

impl SessionStore {
    /// What waits in the conversation's line. None when it isn't here.
    pub async fn line(&self, id: &str) -> io::Result<Option<Vec<Input>>> {
        self.edit_line(id, |line| (line.inputs.clone(), false))
            .await
    }

    /// Change the saved line of a conversation that still exists, and write
    /// it to disk before returning when `change` says it changed.
    pub async fn edit_line<T: Send + 'static>(
        &self,
        id: &str,
        change: impl FnOnce(&mut Line) -> (T, bool) + Send + 'static,
    ) -> io::Result<Option<T>> {
        let session = id.to_owned();
        self.mutate_existing_with(id, move |folder| {
            let header = read_header(folder, &session)?.ok_or(ErrorKind::NotFound)?;
            let mut agent = read_agent(folder)?;
            let inputs = match agent.get("inbox") {
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|item| serde_json::from_value(item.clone()))
                    .collect::<Result<Vec<Input>, _>>()
                    .map_err(|error| io::Error::new(ErrorKind::InvalidData, error))?,
                _ => Vec::new(),
            };
            let cancelled_handoffs = agent
                .get("cancelledPromptHandoffs")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let mut line = Line {
                inputs,
                cancelled_handoffs,
                seen: Vec::new(),
            };
            let (value, changed) = change(&mut line);
            if changed {
                agent.insert("inbox".into(), serde_json::to_value(&line.inputs)?);
                if !line.cancelled_handoffs.is_empty() {
                    agent.insert(
                        "cancelledPromptHandoffs".into(),
                        Value::Object(line.cancelled_handoffs),
                    );
                }
                if !line.seen.is_empty() {
                    let threads = slots(folder, &header, &mut agent)?;
                    for (provider, input) in line.seen {
                        let Some(slot) = threads.get_mut(&provider).and_then(Value::as_object_mut)
                        else {
                            continue;
                        };
                        let seen = slot
                            .entry("seenSteers")
                            .or_insert_with(|| Value::Array(Vec::new()));
                        if let Value::Array(seen) = seen
                            && !seen.iter().any(|id| id == input.as_str())
                        {
                            seen.push(Value::String(input));
                        }
                    }
                }
                files::write_atomic(&folder.join("agent.json"), &serde_json::to_vec(&agent)?)?;
            }
            Ok(value)
        })
        .await
    }
}
