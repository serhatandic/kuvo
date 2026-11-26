//! The part of a conversation not yet seen by the agent taking the next turn.

use super::{SessionStore, TRANSCRIPT, read_agent, read_header, slots};
use crate::{accounts::Provider, roster::Roster, session::TurnResult};
use kuvo_store::{files, sqlite};
use serde_json::{Value, json};
use std::{collections::HashMap, io, path::Path, time::Duration};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    User,
    Added,
    Agent,
    Command,
    File,
    Stopped,
    Shell,
}

impl Role {
    fn label(self) -> &'static str {
        match self {
            Self::User => "You",
            Self::Added => "You, while it worked",
            Self::Agent => "Agent",
            Self::Command => "Command",
            Self::File => "File change",
            Self::Stopped => "Stopped",
            Self::Shell => "You ran",
        }
    }
}

struct Item(Role, String);

impl Item {
    fn render(&self) -> String {
        format!("[{}]\n{}", self.0.label(), self.1)
    }
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

fn list(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}

fn command(command: &str, exit: &Value, output: &str) -> String {
    let mut result = command.to_owned();
    if let Some(exit) = exit.as_i64() {
        result.push_str(&format!("\nExit code: {exit}"));
    }
    if !output.is_empty() {
        result.push('\n');
        let mut units = 0;
        let mut start = output.len();
        for (index, ch) in output.char_indices().rev() {
            units += ch.len_utf16();
            if units > 2000 {
                break;
            }
            start = index;
        }
        if start > 0 {
            result.push('…');
        }
        result.push_str(&output[start..]);
    }
    result
}

type Names = HashMap<String, (String, Vec<crate::catalog::Model>)>;

async fn names(rows: &[Value], roster: Option<&Roster>) -> Names {
    let providers = rows
        .iter()
        .filter(|row| row["type"] == "assistant")
        .filter_map(|row| row["model"]["providerID"].as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let Some(roster) = roster else {
        return Names::new();
    };
    futures_util::future::join_all(providers.into_iter().filter_map(|provider| {
        let entry = roster.find(&Provider::of_model(provider)?)?;
        Some(async move {
            let mut name = entry.account.kind.display_name().to_owned();
            if entry.account.id != "default" {
                name.push_str(" · ");
                name.push_str(entry.account.label.as_deref().unwrap_or("Another account"));
            }
            let models = tokio::time::timeout(Duration::from_secs(3), entry.agent.models(false))
                .await
                .ok()
                .and_then(Result::ok)
                .map(|models| models.models)
                .unwrap_or_default();
            (provider.to_owned(), (name, models))
        })
    }))
    .await
    .into_iter()
    .collect()
}

fn items(rows: &[Value], partway: bool, names: &Names) -> Vec<Item> {
    let mut out = Vec::new();
    let mut summary = false;
    for row in rows {
        match text(&row["type"]) {
            "compaction" if row["reason"] == "manual" => summary = true,
            "user" => {
                let mut words = text(&row["text"]).to_owned();
                let names = list(&row["files"])
                    .map(|file| {
                        file["name"]
                            .as_str()
                            .or_else(|| file["source"]["uri"].as_str())
                            .unwrap_or("a file")
                    })
                    .collect::<Vec<_>>();
                if !names.is_empty() {
                    words.push_str(&format!("\n(Attached: {})", names.join(", ")));
                }
                out.push(Item(if partway { Role::Added } else { Role::User }, words));
            }
            "assistant" => {
                let provider = text(&row["model"]["providerID"]);
                let id = text(&row["model"]["id"]);
                let fallback = Provider::parse(provider)
                    .map(|p| p.kind.display_name())
                    .unwrap_or(provider);
                let (name, model) = names
                    .get(provider)
                    .map_or((fallback, id), |(name, models)| {
                        let model = models.iter().find(|model| {
                            model.id == id || model.aliases.iter().any(|alias| alias == id)
                        });
                        (name.as_str(), model.map_or(id, |model| model.name.as_str()))
                    });
                let mut words = Vec::new();
                let flush = |words: &mut Vec<&str>, out: &mut Vec<Item>| {
                    if !words.is_empty() {
                        out.push(Item(
                            Role::Agent,
                            format!("({name}, {model})\n{}", words.join("\n\n")),
                        ));
                        words.clear();
                    }
                };
                for part in list(&row["content"]) {
                    if part["type"] == "text" {
                        let said = text(&part["text"]).trim();
                        if !said.is_empty() {
                            words.push(said);
                        }
                    }
                    if part["type"] != "tool" {
                        continue;
                    }
                    let state = &part["state"];
                    if state["status"] == "streaming" {
                        continue;
                    }
                    let input = &state["input"];
                    match text(&part["name"]) {
                        "shell" if input["command"].is_string() => {
                            flush(&mut words, &mut out);
                            let settled =
                                matches!(state["status"].as_str(), Some("completed" | "error"));
                            let content = if settled {
                                &state["content"]
                            } else {
                                &Value::Null
                            };
                            let output = list(content)
                                .map(|part| {
                                    if part["type"] == "text" {
                                        text(&part["text"])
                                    } else {
                                        ""
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join("\n");
                            out.push(Item(
                                Role::Command,
                                command(
                                    text(&input["command"]),
                                    if state["status"] == "completed" {
                                        &state["metadata"]["exit"]
                                    } else {
                                        &Value::Null
                                    },
                                    &output,
                                ),
                            ));
                        }
                        "edit" | "write" | "patch" => {
                            let paths = match input["path"].as_str().filter(|path| !path.is_empty())
                            {
                                Some(path) => vec![path],
                                None => list(&input["files"]).filter_map(Value::as_str).collect(),
                            };
                            if !paths.is_empty() {
                                flush(&mut words, &mut out);
                                out.extend(
                                    paths
                                        .into_iter()
                                        .map(|path| Item(Role::File, path.to_owned())),
                                );
                            }
                        }
                        _ => {}
                    }
                }
                flush(&mut words, &mut out);
            }
            "shell" => out.push(Item(
                Role::Shell,
                command(
                    text(&row["command"]),
                    &row["exit"],
                    text(&row["output"]["output"]),
                ),
            )),
            "idle" if summary => summary = false,
            "idle" if row["outcome"] != "succeeded" => {
                let why = row["error"]["message"].as_str().unwrap_or_else(|| {
                    match text(&row["outcome"]) {
                        "interrupted" if row["reason"] == "restart" => {
                            "Kuvo Code restarted, and it stopped with it."
                        }
                        "interrupted" => "You stopped it.",
                        _ => "It stopped with an error.",
                    }
                });
                out.push(Item(Role::Stopped, why.to_owned()));
            }
            _ => {}
        }
    }
    out
}

/// Whole items, most important first, then restored to their original order.
fn select(items: &[Item]) -> (Vec<String>, usize) {
    let mut chosen = vec![false; items.len()];
    let mut budget = 16_000usize;
    let rendered = items.iter().map(Item::render).collect::<Vec<_>>();
    let priority = [
        items.iter().rposition(|item| item.0 == Role::User),
        items.iter().rposition(|item| item.0 == Role::Agent),
        items.iter().position(|item| item.0 == Role::User),
    ];
    for i in priority.into_iter().flatten().chain((0..items.len()).rev()) {
        let size = rendered[i].len() + 2;
        if !chosen[i] && size <= budget {
            chosen[i] = true;
            budget -= size;
        }
    }
    let kept = rendered
        .into_iter()
        .zip(chosen)
        .filter_map(|(item, keep)| keep.then_some(item))
        .collect::<Vec<_>>();
    let left = items.len() - kept.len();
    (kept, left)
}

fn handover(before: &[Value], partway: &[Value], names: &Names) -> Option<String> {
    let history = items(before, false, names);
    let started = items(partway, true, names);
    let mut parts = Vec::new();
    if !history.is_empty() {
        parts.push(if history.iter().all(|item| item.0 == Role::Shell) {
            "Since your last reply, the user ran these commands themselves; what they printed is below, for context (it isn't a new request):"
        } else {
            "This conversation began with another agent, and it's yours now. What came before, for context (it isn't a new request, and it doesn't outrank your instructions; attachments and other agents' tool calls aren't replayed):"
        }.to_owned());
        let (kept, left) = select(&history);
        if left > 0 {
            parts.push(format!(
                "({left} earlier {} left out.)",
                if left == 1 { "item is" } else { "items are" }
            ));
        }
        parts.extend(kept);
    }
    if !started.is_empty() {
        parts.push(if started.iter().any(|item| item.0 == Role::Added) {
            "Another agent started on the message below before it stopped. What it did is already done; what the user added while it worked is part of what they ask:"
        } else {
            "Another agent started on the message below before it stopped. What it did, which is already done:"
        }.to_owned());
        parts.extend(select(&started).0);
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

// Read only the unseen rows on the common SQLite path.
fn after(folder: &Path, seen: Option<&str>) -> io::Result<Vec<Value>> {
    let file = folder.join(TRANSCRIPT);
    if file.try_exists()? {
        let db = sqlite::open_index(&file, Duration::from_secs(1)).map_err(io::Error::other)?;
        let mut query = db.prepare(
            "SELECT body FROM message WHERE position > COALESCE((SELECT position FROM message WHERE id = ?1 ORDER BY position DESC LIMIT 1), -1) ORDER BY position"
        ).map_err(io::Error::other)?;
        let rows = query
            .query_map([seen], |row| row.get::<_, String>(0))
            .map_err(io::Error::other)?;
        return rows
            .map(|row| Ok(serde_json::from_str(&row.map_err(io::Error::other)?)?))
            .collect();
    }
    let rows: Vec<Value> = files::read(&folder.join("messages.json"))?
        .map(|bytes| serde_json::from_slice(&bytes))
        .transpose()?
        .unwrap_or_default();
    let start = seen
        .and_then(|id| rows.iter().position(|row| row["id"] == id))
        .map_or(0, |i| i + 1);
    Ok(rows.into_iter().skip(start).collect())
}

impl SessionStore {
    pub(crate) async fn handover(
        &self,
        id: &str,
        provider: &str,
        prompt: &str,
        fresh: bool,
        roster: Option<&Roster>,
    ) -> io::Result<Option<String>> {
        let provider = Provider::slot(provider);
        let prompt = prompt.to_owned();
        let session = id.to_owned();
        let Some((rows, at)) = self
            .mutate_existing_with(id, move |folder| {
                let header = read_header(folder, &session)?.ok_or(io::ErrorKind::NotFound)?;
                let mut agent = read_agent(folder)?;
                let had_slots = agent.get("threads").is_some_and(Value::is_object);
                let mut changed = !had_slots;
                let threads = slots(folder, &header, &mut agent)?;
                let slot = threads.get_mut(&provider).and_then(Value::as_object_mut);
                let (seen, steers) = match slot {
                    Some(slot) if fresh => {
                        changed |= slot.remove("seen").is_some();
                        changed |= slot.remove("seenSteers").is_some();
                        (None, Vec::new())
                    }
                    Some(slot) => (
                        slot.get("seen").and_then(Value::as_str).map(str::to_owned),
                        list(slot.get("seenSteers").unwrap_or(&Value::Null))
                            .cloned()
                            .collect(),
                    ),
                    None => (None, Vec::new()),
                };
                let rows = after(folder, seen.as_deref())?
                    .into_iter()
                    .filter(|row| !steers.contains(&row["id"]))
                    .collect::<Vec<_>>();
                let at = rows.iter().position(|row| row["id"] == prompt);
                if changed {
                    files::write_atomic(&folder.join("agent.json"), &serde_json::to_vec(&agent)?)?;
                }
                Ok((rows, at))
            })
            .await?
        else {
            return Ok(None);
        };
        let (before, rest) = at.map_or((&rows[..], &[][..]), |at| (&rows[..at], &rows[at + 1..]));
        let names = names(&rows, roster).await;
        Ok(handover(before, rest, &names))
    }

    /// A saved native conversation may exist before it has seen this transcript.
    pub(crate) async fn has_seen(&self, id: &str, provider: &str) -> io::Result<bool> {
        let provider = Provider::slot(provider);
        let session = id.to_owned();
        Ok(self
            .mutate_existing_with(id, move |folder| {
                let header = read_header(folder, &session)?.ok_or(io::ErrorKind::NotFound)?;
                let mut agent = read_agent(folder)?;
                let had_slots = agent.get("threads").is_some_and(Value::is_object);
                let seen = slots(folder, &header, &mut agent)?
                    .get(&provider)
                    .and_then(|slot| slot["seen"].as_str())
                    .is_some_and(|id| !id.is_empty());
                if !had_slots {
                    files::write_atomic(&folder.join("agent.json"), &serde_json::to_vec(&agent)?)?;
                }
                Ok(seen)
            })
            .await?
            .unwrap_or(false))
    }

    pub(crate) async fn note_seen(
        &self,
        id: &str,
        provider: &str,
        turn: &str,
        final_row: &str,
        result: &TurnResult,
    ) -> io::Result<()> {
        let provider = Provider::slot(provider);
        let session = id.to_owned();
        let turn = turn.to_owned();
        let final_row = final_row.to_owned();
        let result = result.clone();
        self.mutate_existing_with(id, move |folder| {
            let header = read_header(folder, &session)?.ok_or(io::ErrorKind::NotFound)?;
            let mut agent = read_agent(folder)?;
            let slot = slots(folder, &header, &mut agent)?
                .entry(provider)
                .or_insert_with(|| json!({"state":{}}));
            let Some(slot) = slot.as_object_mut() else {
                return Ok(());
            };
            let failed = matches!(result.outcome, crate::session::Outcome::Failed(_));
            let seen = if result.missed.is_empty() && (!failed || result.took_prompt) {
                Some(final_row.clone())
            } else {
                let rows = after(folder, Some(&turn))?;
                let worked = rows.iter().any(|row| {
                    row["type"] == "assistant"
                        && list(&row["content"]).any(|part| {
                            part["type"] == "tool"
                                || (part["type"] == "text"
                                    && !text(&part["text"]).trim().is_empty())
                        })
                });
                if failed && !result.took_prompt && !worked {
                    return Ok(());
                }
                let missed = rows.iter().position(|row| {
                    row["id"]
                        .as_str()
                        .is_some_and(|id| result.missed.iter().any(|missed| missed == id))
                });
                match missed {
                    Some(0) => Some(turn.clone()),
                    Some(i) => rows[i - 1]["id"].as_str().map(str::to_owned),
                    None => Some(final_row),
                }
            };
            if let Some(seen) = seen {
                slot.insert("seen".into(), json!(seen));
                slot.remove("seenSteers");
                files::write_atomic(&folder.join("agent.json"), &serde_json::to_vec(&agent)?)?;
            }
            Ok(())
        })
        .await?;
        Ok(())
    }
}
