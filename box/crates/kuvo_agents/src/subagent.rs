//! A subagent's portable conversation, owned by the resident while its
//! parent adapter reports the native work. A child has its own turn writer;
//! it never takes an independent native thread through the normal inbox.

use crate::{
    accounts::Provider,
    feed::Feed,
    hold::TurnNote,
    session::{
        Delivery, EventEnvelope, ModelRef, Outcome, PermissionMode, Prompt, TurnEvent, TurnInput,
        TurnKind, TurnResult,
    },
    session_store::SessionStore,
    turn_projection::{Projection, now, user},
    turn_runtime::TurnWriter,
};
use serde::Serialize;
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use std::io::{self, ErrorKind};
use tokio::sync::{mpsc, oneshot};

/// How a user can write to the child. The parent adapter supplies this when
/// it knows how its maker accepts messages for a spawned agent.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "via", rename_all = "lowercase")]
pub enum Messages {
    None { note: String },
    Direct,
    Parent,
}

impl Messages {
    pub fn unavailable(provider: &str) -> Self {
        let name = Provider::of_model(provider)
            .map(|agent| agent.kind.display_name())
            .unwrap_or("Its agent");
        Self::None {
            note: format!("{name} can't take messages for its subagents."),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Task {
    /// The native parent's call that started this child, stable during replay.
    pub call: String,
    pub title: String,
    pub prompt: Option<String>,
    pub agent: Option<String>,
    pub model: ModelRef,
    pub background: bool,
    pub messages: Messages,
}

/// A stable child ID in the parent's turn: replaying a held turn finds the
/// same saved conversation rather than creating a second child.
fn child_id(parent: &str, turn: &str, call: &str) -> String {
    let mut hash = Sha1::new();
    for part in [parent, turn, call] {
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part.as_bytes());
    }
    format!("ses_{}", hex::encode(hash.finalize()))
}

struct Write {
    event: EventEnvelope,
    done: oneshot::Sender<io::Result<()>>,
}

/// The adapter's handle to a serialized transcript writer for a child. A
/// completed child ignores late native frames, including a replay of those
/// already committed by a previous resident.
pub struct ChildTurn {
    id: String,
    writes: Option<mpsc::Sender<Write>>,
}

impl ChildTurn {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub async fn event(&self, event: TurnEvent) -> io::Result<()> {
        let Some(writes) = &self.writes else {
            return Ok(());
        };
        let (done, written) = oneshot::channel();
        if writes
            .send(Write {
                event: EventEnvelope {
                    position: None,
                    delivery: Delivery::Live,
                    event,
                },
                done,
            })
            .await
            .is_err()
        {
            // The child was stopped locally meanwhile.
            return Ok(());
        }
        written.await.unwrap_or(Ok(()))
    }

    pub async fn finish(&self, outcome: Outcome) -> io::Result<()> {
        self.event(ended(outcome)).await
    }

    pub(crate) async fn start(
        store: SessionStore,
        feed: Option<Feed>,
        parent: &str,
        turn: &str,
        task: Task,
    ) -> io::Result<Self> {
        let id = child_id(parent, turn, &task.call);
        let parent_header = store.read(parent).await?.ok_or(ErrorKind::NotFound)?;
        let parent_fields = Value::Object(parent_header.into_fields());
        let directory = parent_fields["location"]["directory"]
            .as_str()
            .ok_or_else(|| io::Error::other("The conversation has no working directory."))?
            .to_owned();
        let agent = task.agent.unwrap_or_else(|| {
            parent_fields["agent"]
                .as_str()
                .unwrap_or("build")
                .to_owned()
        });
        let previous = store.read(&id).await?;
        if let Some(saved) = &previous {
            if saved.parent() != Some(parent) {
                return Err(io::Error::other(
                    "That child conversation belongs to another parent.",
                ));
            }
            if saved.outcome().is_some() {
                return Ok(Self { id, writes: None });
            }
        }
        if previous.is_none() {
            let mut model = json!({"providerID":task.model.provider,"id":task.model.id});
            if let Some(variant) = &task.model.variant {
                model["variant"] = json!(variant);
            }
            let at = now();
            let maker = parent_fields["metadata"]["kuvoAgent"]
                .as_str()
                .or_else(|| Provider::of_model(&task.model.provider).map(|p| p.kind.name()));
            let mut header = json!({
                "id":id, "parentID":parent,
                "agent":agent, "model":model, "cost":0,
                "tokens":{"input":0,"output":0,"reasoning":0,"cache":{"read":0,"write":0}},
                "time":{"created":at,"updated":at}, "title":task.title,
                "location":{"directory":directory},
                "metadata":{"kuvoSubagent":true,"kuvo":{"messages":task.messages}},
            });
            if let Some(project) = parent_fields
                .get("projectID")
                .filter(|value| !value.is_null())
            {
                header["projectID"] = project.clone();
            }
            if let Some(maker) = maker {
                header["metadata"]["kuvoAgent"] = json!(maker);
            }
            store
                .create(header.as_object().cloned().expect("child header"))
                .await?;
            if let Some(feed) = &feed {
                feed.emit_from(
                    "session.created",
                    json!({"sessionID":id,"session":header}),
                    Some(directory.clone()),
                );
            }
        }
        let child_turn = format!("msg_{}_task", id.trim_start_matches("ses_"));
        if let Some(text) = task.prompt.filter(|words| !words.is_empty())
            && !store.has_message(&id, &child_turn).await?
        {
            store
                .write_messages(
                    &id,
                    vec![user(&Prompt {
                        id: child_turn.clone(),
                        text,
                        files: vec![],
                        synthetic: false,
                    })],
                )
                .await?;
        }
        let kind = if previous.is_some() {
            TurnKind::Recover {
                note: TurnNote {
                    session: id.clone(),
                    from: 0,
                    turn: Some(child_turn.clone()),
                    request: None,
                    sent_at: None,
                    state: None,
                },
            }
        } else {
            TurnKind::AgentStarted
        };
        let input = TurnInput {
            id: child_turn,
            kind,
            model: task.model,
            instructions: String::new(),
            permissions: PermissionMode::AskFirst,
            plan: false,
            auto: false,
            start_cancel: Default::default(),
        };
        let activity = store.background().begin_child(parent, &id, task.background);
        let mut writer = Projection::start(store, id.clone(), agent, &input, None, None)
            .await?
            .for_child();
        let (writes, mut inbox) = mpsc::channel::<Write>(32);
        let session = id.clone();
        if let Some(feed) = &feed {
            feed.emit_from(
                "session.execution.started",
                json!({"sessionID":id}),
                Some(directory.clone()),
            );
        }
        tokio::spawn(async move {
            let stop = activity.cancelled();
            let outcome = loop {
                tokio::select! {
                    biased;
                    () = stop.cancelled() => break Outcome::Interrupted,
                    write = inbox.recv() => {
                        let Some(write) = write else { break Outcome::Interrupted };
                        let ending = match &write.event.event {
                            TurnEvent::Finished(result) => Some(result.outcome.clone()),
                            _ => None,
                        };
                        let result = writer.apply(&write.event).await;
                        let failed = result.is_err();
                        let _ = write.done.send(result);
                        if failed { return; }
                        if let Some(outcome) = ending {
                            inbox.close();
                            drop(activity);
                            tell_end(feed.as_ref(), &session, &directory, &outcome);
                            return;
                        }
                    }
                }
            };
            inbox.close();
            let event = EventEnvelope {
                position: None,
                delivery: Delivery::Live,
                event: ended(outcome.clone()),
            };
            let _ = writer.apply(&event).await;
            drop(activity);
            tell_end(feed.as_ref(), &session, &directory, &outcome);
        });
        Ok(Self {
            id,
            writes: Some(writes),
        })
    }
}

fn ended(outcome: Outcome) -> TurnEvent {
    TurnEvent::Finished(TurnResult {
        outcome,
        took_prompt: true,
        missed: vec![],
    })
}

fn tell_end(feed: Option<&Feed>, id: &str, directory: &str, outcome: &Outcome) {
    let Some(feed) = feed else { return };
    let (kind, data) = match outcome {
        Outcome::Succeeded => ("session.execution.succeeded", json!({"sessionID":id})),
        Outcome::Failed(error) => (
            "session.execution.failed",
            json!({"sessionID":id,"error":error.reason()}),
        ),
        Outcome::AgentExited(_) => (
            "session.execution.interrupted",
            json!({"sessionID":id,"reason":"agent-exit"}),
        ),
        _ => ("session.execution.interrupted", json!({"sessionID":id})),
    };
    feed.emit_from(kind, data, Some(directory.to_owned()));
    feed.emit_from(
        "session.idle",
        json!({"sessionID":id}),
        Some(directory.to_owned()),
    );
}
