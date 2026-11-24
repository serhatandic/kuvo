//! A stand-in agent whose turns run in a held process and speak JSON-RPC
//! through the shared connection, so they go on through a resident update.
//! Only its few lines are a stand-in: the holder, the connection, the sink,
//! the asks and the transcript are the shipped ones.

use kuvo_agents::{
    hold::{Controller, HoldKind, Limits, Start, TurnNote},
    rpc::{Incoming, Options, Rpc},
    runtime::{AdapterCall, Runtime},
    session::{
        ActiveTurn, Decision, Delivery, EventEnvelope, Outcome, PermissionAsk, TextKind, Thread,
        TurnEvent, TurnInput, TurnKind, TurnResult, TurnSink,
    },
};
use kuvo_store::home::Home;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex;

/// Agent processes an earlier resident left, by conversation, until a turn
/// takes them up.
pub(super) type Parked = Arc<Mutex<HashMap<String, Controller>>>;

pub(super) const KIND: &str = "rpc-held";

/// The agent: says what it was asked, asks to edit a file, and says what you
/// answered. It ends when its input does.
const AGENT: &str = r#"
const send = m => process.stdout.write(JSON.stringify(m) + "\n");
require("readline").createInterface({ input: process.stdin })
  .on("line", line => {
    const m = JSON.parse(line);
    if (m.method === "prompt") {
      send({ method: "text", params: { text: "Asked to " + m.params.text } });
      send({ id: 900, method: "approve", params: { action: "edit", resources: ["held.txt"] } });
    } else if (m.id === 900) {
      send({ method: "text", params: { text: m.result.allow ? "; approved" : "; refused" } });
      send({ method: "done" });
    } else if (m.method === "interrupt") {
      send({ method: "done", params: { interrupted: true } });
    }
  })
  .on("close", () => process.exit(0));
"#;

pub(super) struct HeldThread {
    pub home: Home,
    pub program: PathBuf,
    pub session: String,
    pub provider: String,
    pub parked: Parked,
}

struct Controls(Rpc);

impl ActiveTurn for Controls {
    fn interrupt(&self) -> AdapterCall<'_, ()> {
        Box::pin(async {
            self.0.notify("interrupt", None).await;
            Ok(())
        })
    }
}

impl Thread for HeldThread {
    fn start(
        &self,
        input: TurnInput,
        sink: Arc<dyn TurnSink>,
    ) -> AdapterCall<'_, Arc<dyn ActiveTurn>> {
        Box::pin(async move {
            let (controller, prompt) = match &input.kind {
                TurnKind::Prompt(prompt) => (self.launch(&input.id).await?, Some(prompt.text.clone())),
                TurnKind::Recover { .. } => {
                    if self.home.run().join("agent-fixture-refuse").exists() {
                        return Err(io::Error::other("The stand-in can't go on with its turn."));
                    }
                    if self.home.run().join("agent-fixture-slow-replay").exists() {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                    let parked = self.parked.lock().await.remove(&self.session);
                    (parked.ok_or_else(|| io::Error::other("Nothing was held for this turn."))?, None)
                }
                _ => return Err(io::ErrorKind::Unsupported.into()),
            };
            let (rpc, incoming) = Rpc::over(controller, Options::default());
            if let Some(text) = prompt {
                rpc.notify("prompt", Some(json!({ "text": text }))).await;
            }
            say(&sink, TurnEvent::StepStarted { model: Some(input.model) }).await?;
            tokio::spawn(follow(rpc.clone(), incoming, sink));
            Ok(Arc::new(Controls(rpc)) as Arc<dyn ActiveTurn>)
        })
    }
}

impl HeldThread {
    async fn launch(&self, turn: &str) -> io::Result<Controller> {
        let runtime = Runtime::for_home(&self.home);
        runtime
            .holders()
            .start(
                &std::env::current_exe()?,
                Start {
                    command: vec![self.program.clone(), "-e".into(), AGENT.into()],
                    cwd: Some(self.home.root().to_owned()),
                    env: std::env::vars_os().collect(),
                    kind: HoldKind::JsonRpc,
                    meta: json!({
                        "provider": self.provider, "fixture": KIND,
                        "turns": {"t": {"session": self.session, "from": 0, "turn": turn}},
                    })
                    .as_object()
                    .expect("an object")
                    .clone(),
                    portable_requests: None,
                    limits: Limits {
                        orphan: Some(Duration::from_secs(90)),
                        ..Limits::default()
                    },
                },
            )
            .await
    }
}

/// The agent's lines, as the turn's events and asks.
async fn follow(rpc: Rpc, mut incoming: tokio::sync::mpsc::Receiver<Incoming>, sink: Arc<dyn TurnSink>) {
    let result: io::Result<()> = async {
        while let Some(message) = incoming.recv().await {
            match message {
                Incoming::Notification { method, params } if method == "text" => {
                    let delta = params["text"].as_str().unwrap_or_default().to_owned();
                    say(&sink, TurnEvent::TextDelta { kind: TextKind::Text, block: None, delta }).await?;
                }
                Incoming::Notification { method, params } if method == "done" => {
                    let outcome = if params["interrupted"] == true {
                        Outcome::Interrupted
                    } else {
                        Outcome::Succeeded
                    };
                    say(&sink, TurnEvent::StepEnded { usage: None, finish: None, error: None }).await?;
                    say(&sink, TurnEvent::Finished(TurnResult { outcome, took_prompt: true, missed: Vec::new() })).await?;
                    rpc.stop().await;
                    return Ok(());
                }
                Incoming::Request { id, method, params } if method == "approve" => {
                    let ask = PermissionAsk {
                        id: id.to_string(),
                        tool_call_id: None,
                        action: params["action"].as_str().unwrap_or("edit").into(),
                        resources: params["resources"]
                            .as_array()
                            .map(|all| all.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                            .unwrap_or_default(),
                        save: Vec::new(),
                        message: None,
                        metadata: Default::default(),
                        sandboxed: None,
                        already_weighed: false,
                        unclassified: false,
                    };
                    let sink = sink.clone();
                    let rpc = rpc.clone();
                    tokio::spawn(async move {
                        // (No answer: the resident is going, and the next asks again.)
                        let Ok(decision) = sink.permission(ask).await else { return };
                        let allow = matches!(decision, Decision::Once | Decision::Always);
                        rpc.respond(id, Ok(json!({ "allow": allow }))).await;
                    });
                }
                Incoming::Exited { code, .. } if code != -1 => {
                    let error = kuvo_agents::session::TurnError {
                        kind: "agent".into(),
                        message: format!("The stand-in exited with {code}."),
                        status: None,
                        limit: None,
                    };
                    say(&sink, TurnEvent::Finished(TurnResult {
                        outcome: Outcome::AgentExited(Some(error)),
                        took_prompt: true,
                        missed: Vec::new(),
                    }))
                    .await?;
                    return Ok(());
                }
                _ => {}
            }
        }
        Ok(())
    }
    .await;
    if let Err(error) = result {
        eprintln!("The held stand-in could not report its turn: {error}");
    }
}

async fn say(sink: &Arc<dyn TurnSink>, event: TurnEvent) -> io::Result<()> {
    sink.event(EventEnvelope { position: None, delivery: Delivery::Live, event }).await
}

/// An attached agent nothing will take up: its input ends, and so does it.
pub(super) async fn let_go(parked: &Parked, turn: &TurnNote) {
    if let Some(mut controller) = parked.lock().await.remove(&turn.session) {
        let _ = controller.input.end().await;
        let _ = controller.input.bye().await;
    }
}
