//! Only linked into the dedicated test build. Quoted agent lines are a stand-in;
//! the resident, roster, holder, sockets, files and keeper are the shipped implementations.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use kuvo_agents::{
    account_check::{AccountStatus, CheckFailure},
    accounts::Account,
    catalog::Models,
    hold::{Controller, HoldKind, Limits, Output, Record, Start, TurnNote},
    roster::{Agent, Factory},
    runtime::{AdapterCall, Adoption, Runtime},
    session::{
        ActiveTurn, Delivery, EventEnvelope, Outcome, TextKind, Thread, TurnEvent, TurnInput,
        TurnKind, TurnResult, TurnSink,
    },
};
use kuvo_store::{files, home::Home};
use serde_json::json;

#[path = "agent_turn.rs"]
mod turn;

#[path = "agent_held.rs"]
mod held;

const ASK: &str = r#"{"id":17,"method":"fixture/approve"}"#;
const REPLY: &str = r#"{"id":17,"result":{"allow":true}}"#;

pub(super) fn for_home(root: &Path, relay: kuvo_agents::tool_relay::ToolRelay) -> Option<Arc<dyn Factory>> {
    let path = root.join("run/agent-fixture-program");
    let program = PathBuf::from(fs::read_to_string(path).ok()?);
    if !program.is_absolute() || !program.is_file() {
        return None;
    }
    let home = Home::new(root.to_owned()).ok()?;
    Some(Arc::new(Fixture {
        home,
        program,
        recovered: Arc::new(tokio::sync::Mutex::new(None)),
        parked: Arc::default(),
        provider: String::new(),
        relay,
    }))
}

#[derive(Clone)]
struct Fixture {
    home: Home,
    program: PathBuf,
    recovered: Arc<tokio::sync::Mutex<Option<Controller>>>,
    parked: held::Parked,
    provider: String,
    relay: kuvo_agents::tool_relay::ToolRelay,
}

impl Factory for Fixture {
    fn create(&self, account: &Account, _: Option<PathBuf>) -> io::Result<Arc<dyn Agent>> {
        if account.id == "default" {
            let fixture = self.clone();
            tokio::spawn(async move {
                if let Err(error) = fixture.wait_and_start().await {
                    eprintln!("Agent fixture failed: {error}");
                }
            });
        }
        let mut agent = self.clone();
        agent.provider = account.provider().to_string();
        Ok(Arc::new(agent))
    }
}

impl Agent for Fixture {
    fn probe_account(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<AccountStatus, CheckFailure>> + Send + '_>,
    > {
        Box::pin(async { Err(CheckFailure::TimedOut) })
    }

    fn models(&self, _: bool) -> AdapterCall<'_, Models> {
        Box::pin(async { Ok(Models::default()) })
    }

    fn open_thread(
        &self,
        context: kuvo_agents::session::SessionContext,
    ) -> AdapterCall<'_, Arc<dyn kuvo_agents::session::Thread>> {
        Box::pin(async move {
            let thread: Arc<dyn Thread> = if context.session_id == "ses_keeper_fixture" {
                Arc::new(HeldFixture(self.clone()))
            } else if context.session_id.starts_with("ses_held_") {
                Arc::new(held::HeldThread {
                    home: self.home.clone(),
                    program: self.program.clone(),
                    session: context.session_id,
                    provider: self.provider.clone(),
                    parked: self.parked.clone(),
                    relay: self.relay.clone(),
                })
            } else {
                Arc::new(turn::FixtureThread)
            };
            Ok(thread)
        })
    }

    fn shutdown(&self) -> AdapterCall<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

impl Adoption for Fixture {
    fn accepts(&self, record: &Record, turns: &BTreeMap<String, TurnNote>) -> bool {
        if record.meta.get("fixture") == Some(&json!(held::KIND)) {
            return true;
        }
        record.meta.get("fixture") == Some(&json!(true))
            && turns
                .values()
                .any(|turn| turn.session == "ses_keeper_fixture")
    }

    fn attach<'a>(
        &'a self,
        _: &'a Account,
        record: Record,
        turns: BTreeMap<String, TurnNote>,
        controller: Controller,
    ) -> AdapterCall<'a, ()> {
        let home = self.home.clone();
        Box::pin(async move {
            if record.meta.get("fixture") == Some(&json!(held::KIND)) {
                if let Some(turn) = turns.values().next() {
                    self.parked
                        .lock()
                        .await
                        .insert(turn.session.clone(), controller);
                }
                return Ok(());
            }
            if turns.values().any(|turn| turn.turn.is_some()) {
                *self.recovered.lock().await = Some(controller);
            } else {
                tokio::spawn(replay(home, controller, None));
            }
            Ok(())
        })
    }

    fn let_go<'a>(&'a self, turn: &'a TurnNote) -> AdapterCall<'a, ()> {
        Box::pin(async move {
            held::let_go(&self.parked, turn).await;
            Ok(())
        })
    }
}

impl Fixture {
    async fn wait_and_start(&self) -> io::Result<()> {
        let arm = self.home.run().join("agent-fixture-begin");
        let mut clock = tokio::time::interval(Duration::from_millis(25));
        loop {
            clock.tick().await;
            match fs::remove_file(&arm) {
                Ok(()) => break,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        let binary = std::env::current_exe()?;
        let runtime = Runtime::for_home(&self.home);
        // (The message that begins the turn, as a resident writes it.)
        runtime
            .sessions()
            .write_messages(
                "ses_keeper_fixture",
                vec![json!({"type":"user","id":"keeper_fixture","text":"begin","time":{"created":1}})],
            )
            .await?;
        let mut controller = runtime
            .holders()
            .start(
                &binary,
                Start {
                    command: vec![
                        self.program.clone(),
                        "--exact".into(),
                        "agent_probe".into(),
                        "--ignored".into(),
                        "--nocapture".into(),
                    ],
                    cwd: Some(self.home.root().to_owned()),
                    env: std::env::vars_os().collect(),
                    kind: HoldKind::JsonRpc,
                    meta: json!({
                        "provider":"codex-cli", "fixture":true,
                        "turns":{"thread":{"session":"ses_keeper_fixture","from":0,
                            "turn": self.home.root().join("agents/sessions/ses_keeper_fixture/session.json")
                                .is_file().then_some("keeper_fixture"),
                        }},
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                    portable_requests: None,
                    limits: Limits {
                        orphan: Some(Duration::from_secs(90)),
                        ..Limits::default()
                    },
                },
            )
            .await?;
        controller.input.write("begin").await?;
        let ask = line(&mut controller, ASK).await?;
        controller.input.acknowledge(ask).await?;
        controller.input.prepare(0).await?;
        if !matches!(
            controller.output.next().await?,
            Output::Prepared { prepared: true }
        ) {
            return Err(io::Error::other("holder did not reserve its replay"));
        }
        write_checkpoint(&self.home, "agent-fixture-started", &controller)?;
        while controller.output.next().await.is_ok() {}
        Ok(())
    }
}

struct HeldFixture(Fixture);
struct HeldControl;

impl ActiveTurn for HeldControl {
    fn interrupt(&self) -> AdapterCall<'_, ()> {
        Box::pin(async { Err(io::ErrorKind::Unsupported.into()) })
    }
}

impl Thread for HeldFixture {
    fn start(
        &self,
        input: TurnInput,
        sink: Arc<dyn TurnSink>,
    ) -> AdapterCall<'_, Arc<dyn ActiveTurn>> {
        Box::pin(async move {
            if !matches!(input.kind, TurnKind::Recover { .. }) {
                return Err(io::ErrorKind::Unsupported.into());
            }
            let holder = self
                .0
                .recovered
                .lock()
                .await
                .take()
                .ok_or_else(|| io::Error::other("The fixture holder was not attached."))?;
            tokio::spawn(replay(self.0.home.clone(), holder, Some((input, sink))));
            Ok(Arc::new(HeldControl) as Arc<dyn ActiveTurn>)
        })
    }
}

async fn replay(
    home: Home,
    mut controller: Controller,
    sink: Option<(TurnInput, Arc<dyn TurnSink>)>,
) {
    let target = sink.as_ref().map(|(_, sink)| sink);
    let report = |delivery, event| async move {
        if let Some(sink) = target {
            sink.event(EventEnvelope {
                position: None,
                delivery,
                event,
            })
            .await?;
        }
        Ok::<_, io::Error>(())
    };
    let result: io::Result<()> = async {
        line(&mut controller, "before").await?;
        report(
            Delivery::Replay,
            TurnEvent::StepStarted {
                model: sink.as_ref().map(|(input, _)| input.model.clone()),
            },
        )
        .await?;
        report(
            Delivery::Replay,
            TurnEvent::TextDelta {
                kind: TextKind::Text,
                block: None,
                delta: "before".into(),
            },
        )
        .await?;
        line(&mut controller, ASK).await?;
        report(Delivery::Live, TurnEvent::ReplayCaughtUp).await?;
        controller.input.unpin().await?;
        controller.input.write(REPLY).await?;
        line(&mut controller, "after").await?;
        report(
            Delivery::Live,
            TurnEvent::TextDelta {
                kind: TextKind::Text,
                block: None,
                delta: " after".into(),
            },
        )
        .await?;
        report(
            Delivery::Live,
            TurnEvent::Finished(TurnResult {
                outcome: Outcome::Succeeded,
                took_prompt: true,
                missed: Vec::new(),
            }),
        )
        .await?;
        write_checkpoint(&home, "agent-fixture-finished", &controller)?;
        controller.input.end().await?;
        while !matches!(controller.output.next().await?, Output::Exited { .. }) {}
        controller.input.bye().await
    }
    .await;
    if let Err(error) = result {
        eprintln!("Agent fixture replay failed: {error}");
    }
}

async fn line(controller: &mut Controller, expected: &str) -> io::Result<u64> {
    loop {
        match controller.output.next().await? {
            Output::Line { sequence, line, .. } if line == expected => return Ok(sequence),
            Output::Exited { .. } => {
                return Err(io::Error::other("agent exited before its answer"));
            }
            _ => {}
        }
    }
}

fn write_checkpoint(home: &Home, name: &str, controller: &Controller) -> io::Result<()> {
    let result = json!({
        "agent": controller.welcome.child,
        "holder": controller.welcome.pid,
        "generation": controller.welcome.generation,
        "resident": std::process::id(),
    });
    files::write_atomic(&home.run().join(name), &serde_json::to_vec(&result)?)
}
