//! The resident adapter and two runtimes share a real Rust holder and a recorded CLI protocol.

use std::{
    collections::HashMap,
    env, fs, io,
    io::{BufRead, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use kuvo_agents::{
    accounts::AgentKind,
    claude::ClaudeFactory,
    roster::{Agent, Factory, Roster},
    runtime::{AdapterCall, Runtime},
    session::{
        Answer, Decision, Delivery, EventEnvelope, ModelRef, Outcome, PermissionAsk,
        PermissionMode, Preflight, Prompt, QuestionAsk, SessionContext, Steer, SteerReceipt,
        TurnEvent, TurnInput, TurnKind, TurnSink, VendorState,
    },
};
use kuvo_store::home::Home;
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc};

const INITIAL: &str = "e84dbe6c-64ec-4fbf-aafa-b1ec9db07891";
const CLEARED: &str = "ee376aaf-ec88-40f9-bb24-f292f6e6ad0d";
const SESSION: &str = "ses_claude_holder";

#[test]
fn claude_reconnects_with_the_same_pids_and_answers_an_open_ask_once() {
    #[cfg(unix)]
    let root = tempfile::Builder::new()
        .prefix("kc-")
        .tempdir_in("/tmp")
        .unwrap();
    #[cfg(windows)]
    let root = tempfile::Builder::new().prefix("kc-").tempdir().unwrap();
    let bin = root.path().join("bin");
    fs::create_dir(&bin).unwrap();
    write_launcher(&bin);
    #[cfg(windows)]
    kuvo_store::files::write_atomic(
        &root.path().join("run/socket.key"),
        "ab".repeat(32).as_bytes(),
    )
    .unwrap();

    let mut process = Command::new(env::current_exe().unwrap());
    process.env_clear();
    for name in ["SystemRoot", "WINDIR"] {
        if let Some(value) = env::var_os(name) {
            process.env(name, value);
        }
    }
    for name in ["HOME", "USERPROFILE", "TMPDIR", "TEMP", "TMP"] {
        process.env(name, root.path());
    }
    let binary = env::var_os("KUVO_CLAUDE_TEST_BOX")
        .map_or_else(
            || PathBuf::from(env!("CARGO_BIN_EXE_kuvo-box")),
            PathBuf::from,
        )
        .canonicalize()
        .unwrap();
    let output = process
        .env("CLAUDE_FIXTURE_BINARY", binary)
        .env("PATH", bin)
        .env("KUVO_TEST_PATH_ONLY", "1")
        .env("KUVO_SANDBOX", "off")
        .env("CLAUDE_FIXTURE_ROOT", root.path())
        .args(["--exact", "run_claude_adapter", "--ignored", "--nocapture"])
        .output()
        .unwrap();
    if !output.status.success() {
        stop_fixture_holders(root.path());
    }
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn stop_fixture_holders(root: &Path) {
    let home = Home::new(root.to_owned()).unwrap();
    let holders = kuvo_agents::hold::Directory::for_home(&home);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let _ = tokio::time::timeout(Duration::from_secs(5), async {
            for record in holders.records().await.unwrap_or_default() {
                if let Ok(mut controller) = holders
                    .adopt(&record, kuvo_agents::hold::Replay::Idle)
                    .await
                {
                    let _ = controller.input.unpin().await;
                    let _ = controller.input.end().await;
                    let _ = controller.input.signal("SIGKILL").await;
                    let _ = controller.input.bye().await;
                }
            }
        })
        .await;
    });
}

fn write_launcher(bin: &Path) {
    let program = env::current_exe().unwrap();
    let args = "--exact claude_wire_fixture --ignored --nocapture";
    // `claude auth status`: signed in, as Claude Code says even after a run's sign-in was refused.
    let status = r#"{"loggedIn":true,"authMethod":"claude.ai"}"#;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let file = bin.join("claude");
        fs::write(
            &file,
            format!(
                "#!/bin/sh\nif [ \"$1\" = auth ]; then echo '{status}'; exit 0; fi\nexec '{}' {args} -- \"$@\"\n",
                program.display().to_string().replace('\'', "'\\''")
            ),
        )
        .unwrap();
        fs::set_permissions(file, fs::Permissions::from_mode(0o700)).unwrap();
    }
    #[cfg(windows)]
    fs::write(
        bin.join("claude.cmd"),
        format!(
            "@if \"%~1\"==\"auth\" (echo {status}& exit /b 0)\r\n@\"{}\" {args} -- %*\r\n",
            program.display()
        ),
    )
    .unwrap();
}

#[test]
#[ignore = "Runs in a private environment with the local Claude fixture"]
fn run_claude_adapter() {
    let root = PathBuf::from(env::var_os("CLAUDE_FIXTURE_ROOT").expect("private test home"));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(20), scenario(root))
            .await
            .unwrap();
    });
}

async fn scenario(root: PathBuf) {
    let home = Home::new(root.clone()).unwrap();
    let runtime = Runtime::for_home(&home);
    let factory = Arc::new(ClaudeFactory::new(
        home,
        PathBuf::from(env::var_os("CLAUDE_FIXTURE_BINARY").expect("private test binary")),
    ));
    let first = activate(&runtime, factory.clone()).await;
    let old = claude(&first);
    let saved: VendorState = serde_json::from_value(json!({"claudeSession": INITIAL})).unwrap();
    let holder = old.open_thread(context(&root, saved)).await.unwrap();
    assert!(holder.owns_path(&root.join(".claude/plans/fixture.md")));
    assert!(holder.owns_path(&root.join(".claude/projects/project/memory/fixture.md")));
    assert!(!holder.owns_path(&root.join(".claude/projects/project/credentials.json")));
    assert!(!holder.owns_path(&root.join(".claude/plans/../../fixture.md")));
    let (sink, mut events, mut questions) = Sink::new();
    let active = holder
        .start(
            input(
                "turn_1",
                TurnKind::Prompt(prompt("msg_first", "ask and reply")),
            ),
            sink.clone(),
        )
        .await
        .unwrap();
    assert_eq!(questions.recv().await.unwrap(), "tool-approve");
    let receipt = active
        .steer_receipt(&prompt("msg_second", "one more"))
        .unwrap();
    assert_eq!(
        (receipt.thread.as_str(), receipt.turn.as_str()),
        (SESSION, "turn_1")
    );
    assert_eq!(
        active
            .steer(prompt("msg_second", "one more"))
            .await
            .unwrap(),
        Steer::Accepted
    );
    // Handed, and Claude hasn't said whether it took it.
    assert_eq!(holder.resolve_steer(receipt.clone()).await.unwrap(), None);
    let before = runtime.holders().records().await.unwrap().pop().unwrap();
    assert!(before.child.is_some());
    old.prepare_replacement().await.unwrap();
    // The turn goes on with the next resident: this one doesn't end it, so what it reports to
    // stays open for as long as this resident is there.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        Arc::strong_count(&sink) > 1,
        "the handed-over turn was let go"
    );
    drop(active);
    drop(holder);
    drop(first);

    let replacement = activate(&runtime, factory.clone()).await;
    let next = claude(&replacement);
    let after = runtime.holders().records().await.unwrap().pop().unwrap();
    assert_eq!((after.pid, after.child), (before.pid, before.child));
    assert_ne!(after.generation, before.generation);
    let note = after.turns().unwrap().remove("main").unwrap();
    let state = sink.state.lock().unwrap().clone();
    assert_eq!(state["claudeSession"], INITIAL);
    let holder = next.open_thread(context(&root, state)).await.unwrap();
    // The recovered turn still waits to hear about it.
    assert_eq!(holder.resolve_steer(receipt.clone()).await.unwrap(), None);
    let (new_sink, mut replayed, mut asks) = Sink::new();
    holder
        .start(
            input("turn_1", TurnKind::Recover { note }),
            new_sink.clone(),
        )
        .await
        .unwrap();
    assert_eq!(asks.recv().await.unwrap(), "tool-approve");
    new_sink.allow.notify_one();
    let end = finish(&mut replayed).await;
    assert!(
        end.iter()
            .any(|event| matches!(event.event, TurnEvent::ReplayCaughtUp))
    );
    assert!(end.iter().any(|event| event.delivery == Delivery::Replay
        && matches!(
            &event.event, TurnEvent::TextDelta { delta, .. } if delta == "starts here"
        )));
    assert!(end.iter().any(|event| matches!(
        &event.event, TurnEvent::TextDelta { delta, .. } if delta == "second message answered"
    )));
    assert!(matches!(
        asks.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    assert!(
        matches!(&end.last().unwrap().event, TurnEvent::Finished(result) if
            matches!(result.outcome, Outcome::Succeeded) && result.missed.is_empty()
        )
    );
    assert_eq!(
        holder.resolve_steer(receipt.clone()).await.unwrap(),
        Some(true)
    );
    let never = SteerReceipt {
        message: "5a0f6a3e-2c1b-4d7e-9f80-1a2b3c4d5e6f".into(),
        ..receipt.clone()
    };
    assert_eq!(holder.resolve_steer(never).await.unwrap(), Some(false));
    let elsewhere = SteerReceipt {
        thread: "ses_other".into(),
        ..receipt
    };
    assert_eq!(holder.resolve_steer(elsewhere).await.unwrap(), None);

    let (cancelled, mut canceled_events, mut asks) = Sink::new();
    let active = holder
        .start(
            input(
                "turn_cancel",
                TurnKind::Prompt(prompt("msg_cancel", "ask then withdraw")),
            ),
            cancelled.clone(),
        )
        .await
        .unwrap();
    assert_eq!(asks.recv().await.unwrap(), "tool-approve");
    assert_eq!(
        active
            .steer(prompt("msg_withdraw", "withdraw"))
            .await
            .unwrap(),
        Steer::Accepted
    );
    let end = finish(&mut canceled_events).await;
    assert_eq!(*cancelled.withdrawn.lock().unwrap(), ["tool-approve"]);
    assert!(
        matches!(&end.last().unwrap().event, TurnEvent::Finished(result) if
            matches!(result.outcome, Outcome::Succeeded) && result.missed.is_empty()
        )
    );

    let (stop, mut stopped_events, mut asks) = Sink::new();
    let active = holder
        .start(
            input(
                "turn_stop",
                TurnKind::Prompt(prompt("msg_stop", "stop while asking")),
            ),
            stop.clone(),
        )
        .await
        .unwrap();
    assert_eq!(asks.recv().await.unwrap(), "tool-approve");
    active.interrupt().await.unwrap();
    let end = finish(&mut stopped_events).await;
    assert_eq!(*stop.withdrawn.lock().unwrap(), ["tool-approve"]);
    assert!(
        matches!(&end.last().unwrap().event, TurnEvent::Finished(result) if
            matches!(result.outcome, Outcome::Interrupted)
        )
    );

    let (clear, mut cleared, _) = Sink::new();
    holder
        .start(
            input("turn_2", TurnKind::Prompt(prompt("msg_clear", "/clear"))),
            clear.clone(),
        )
        .await
        .unwrap();
    let end = finish(&mut cleared).await;
    assert!(
        end.iter()
            .any(|event| matches!(event.event, TurnEvent::ContextCleared))
    );
    let saved = clear.state.lock().unwrap().clone();
    assert_eq!(saved["claudeSession"], CLEARED);
    // A turn under way as the resident makes way: the next one finds it ended after all (its end
    // was written just before the resident went) and lets it go. Its conversation goes on.
    let (asking, _gone, mut asks) = Sink::new();
    let active = holder
        .start(
            input("turn_gone", TurnKind::Prompt(prompt("msg_gone", "ask"))),
            asking.clone(),
        )
        .await
        .unwrap();
    assert_eq!(asks.recv().await.unwrap(), "tool-approve");
    next.prepare_replacement().await.unwrap();
    drop(active);
    drop(holder);
    drop(replacement);
    let saved = asking.state.lock().unwrap().clone();

    let restarted = activate(&runtime, factory).await;
    let restored = claude(&restarted);
    let note = runtime
        .holders()
        .records()
        .await
        .unwrap()
        .pop()
        .unwrap()
        .turns()
        .unwrap()
        .remove("main")
        .unwrap();
    assert_eq!(note.turn.as_deref(), Some("turn_gone"));
    restored.let_go(&note).await.unwrap();
    let holder = restored
        .open_thread(context(&root, saved.clone()))
        .await
        .unwrap();
    let (sink, mut frames, _) = Sink::new();
    holder
        .start(
            input("turn_3", TurnKind::Prompt(prompt("msg_resume", "resume"))),
            sink,
        )
        .await
        .unwrap();
    let end = finish(&mut frames).await;
    let expected = format!("resumed {}", saved["claudeSession"].as_str().unwrap());
    assert!(end.iter().any(
        |event| matches!(&event.event, TurnEvent::TextDelta { delta, .. } if delta == &expected)
    ));
    let (sink, mut diagnostics, _) = Sink::new();
    holder
        .start(
            input(
                "turn_exit",
                TurnKind::Prompt(prompt("msg_exit", "exit with error")),
            ),
            sink,
        )
        .await
        .unwrap();
    let end = finish(&mut diagnostics).await;
    assert!(
        matches!(&end.last().unwrap().event, TurnEvent::Finished(result) if
            matches!(&result.outcome, Outcome::AgentExited(Some(error)) if error.message == "Claude Code stopped: Could not authenticate with Bearer …")
        ),
        "{end:?}"
    );

    // `claude auth status` still says signed in after Claude turned the sign-in away in a run.
    assert!(restored.probe_account().await.unwrap().signed_in);
    let (sink, mut refused, _) = Sink::new();
    holder
        .start(
            input(
                "turn_refused",
                TurnKind::Prompt(prompt("msg_refused", "turn the sign-in away")),
            ),
            sink,
        )
        .await
        .unwrap();
    let end = finish(&mut refused).await;
    assert!(
        matches!(&end.last().unwrap().event, TurnEvent::Finished(result) if
            matches!(&result.outcome, Outcome::Failed(error) if error.kind == "signin")),
        "{end:?}"
    );
    assert!(!end.iter().any(|event| matches!(
        &event.event, TurnEvent::TextDelta { delta, .. } if delta.contains("Failed to authenticate")
    )));
    // That Claude Code goes, and the account needs a sign-in until it's written again.
    let gone = Instant::now();
    while !runtime.holders().records().await.unwrap().is_empty() {
        assert!(
            gone.elapsed() < Duration::from_secs(5),
            "the turned-away Claude Code stayed"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let account = restored.probe_account().await.unwrap();
    assert!(!account.signed_in && account.ended, "{account:?}");
    tokio::time::sleep(Duration::from_millis(20)).await;
    fs::write(root.join(".claude/.credentials.json"), "{}").unwrap();
    assert!(restored.probe_account().await.unwrap().signed_in);

    // Where the plan's limits stand, as Claude Code hears in a turn: for the sign-in it started
    // under, as a fingerprint of who that is (the reference's own, from `.claude.json`).
    let account = |id: &str| {
        fs::write(
            root.join(".claude.json"),
            json!({"oauthAccount": {"accountUuid": id, "organizationUuid": "O-1"}}).to_string(),
        )
        .unwrap();
    };
    let heard = |end: &[EventEnvelope]| {
        end.iter()
            .filter_map(|event| match &event.event {
                TurnEvent::PlanLimit(reading) => Some(reading.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    account("A-1");
    let limits =
        |id: &str, text: &str| input(id, TurnKind::Prompt(prompt(&format!("msg_{id}"), text)));
    let (sink, mut first, _) = Sink::new();
    holder
        .start(limits("turn_limits", "hear the limits"), sink)
        .await
        .unwrap();
    let readings = heard(&finish(&mut first).await);
    assert_eq!(readings.len(), 1, "{readings:?}");
    assert_eq!(readings[0].sign_in.as_deref(), Some("ddfa9296fabdbb7b"));
    assert_eq!(
        readings[0]
            .windows
            .iter()
            .map(|window| (window.id.as_str(), window.used))
            .collect::<Vec<_>>(),
        [("five_hour", 0.3), ("seven_day", 0.81)]
    );
    // Signed in as another account since: its next turn starts a Claude Code under that one, and
    // what it hears counts for it.
    account("B-2");
    let (sink, mut second, _) = Sink::new();
    holder
        .start(limits("turn_limits_b", "hear the limits"), sink)
        .await
        .unwrap();
    let readings = heard(&finish(&mut second).await);
    assert_eq!(readings.len(), 1, "{readings:?}");
    assert_eq!(readings[0].sign_in.as_deref(), Some("fc35f91bb5886e9b"));
    // A sign-in that changes while it runs: what it hears then counts for no one.
    let (sink, mut third, _) = Sink::new();
    holder
        .start(
            limits("turn_limits_c", "hear the limits after a sign-in"),
            sink,
        )
        .await
        .unwrap();
    assert!(heard(&finish(&mut third).await).is_empty());
    restored.shutdown().await.unwrap();
    assert!(events.try_recv().is_ok());
}

async fn activate(runtime: &Runtime, factory: Arc<ClaudeFactory>) -> Roster {
    let (roster, failures) = runtime
        .activate(&HashMap::from([(
            AgentKind::Claude,
            factory as Arc<dyn Factory>,
        )]))
        .await
        .unwrap();
    assert!(failures.is_empty());
    roster
}

fn claude(roster: &Roster) -> &Arc<dyn Agent> {
    &roster.all()[0].agent
}

fn context(root: &Path, state: VendorState) -> SessionContext {
    SessionContext {
        session_id: SESSION.into(),
        directory: root.to_owned(),
        state,
        background: Default::default(),
        own_turns: Default::default(),
    }
}

fn input(id: &str, kind: TurnKind) -> TurnInput {
    TurnInput {
        id: id.into(),
        kind,
        model: ModelRef {
            provider: "claude-code".into(),
            id: "haiku".into(),
            variant: Some("low".into()),
        },
        instructions: "Fixture instructions".into(),
        permissions: PermissionMode::AskFirst,
        plan: false,
        auto: false,
    }
}

fn prompt(id: &str, text: &str) -> Prompt {
    Prompt {
        id: id.into(),
        text: text.into(),
        files: Vec::new(),
        synthetic: false,
    }
}

async fn finish(events: &mut mpsc::UnboundedReceiver<EventEnvelope>) -> Vec<EventEnvelope> {
    let mut result = Vec::new();
    loop {
        let event = events.recv().await.unwrap();
        let done = matches!(event.event, TurnEvent::Finished(_));
        result.push(event);
        if done {
            return result;
        }
    }
}

struct Sink {
    events: mpsc::UnboundedSender<EventEnvelope>,
    asks: mpsc::UnboundedSender<String>,
    state: Mutex<VendorState>,
    withdrawn: Mutex<Vec<String>>,
    allow: Notify,
}

impl Sink {
    fn new() -> (
        Arc<Self>,
        mpsc::UnboundedReceiver<EventEnvelope>,
        mpsc::UnboundedReceiver<String>,
    ) {
        let (send, events) = mpsc::unbounded_channel();
        let (ask, asks) = mpsc::unbounded_channel();
        (
            Arc::new(Self {
                events: send,
                asks: ask,
                state: Mutex::default(),
                withdrawn: Mutex::default(),
                allow: Notify::new(),
            }),
            events,
            asks,
        )
    }
}

impl TurnSink for Sink {
    fn event(&self, event: EventEnvelope) -> AdapterCall<'_, ()> {
        Box::pin(async move {
            self.events.send(event).unwrap();
            Ok(())
        })
    }

    fn save_state(&self, state: VendorState) -> AdapterCall<'_, ()> {
        Box::pin(async move {
            *self.state.lock().unwrap() = state;
            Ok(())
        })
    }

    fn preflight(&self, ask: PermissionAsk, _: bool) -> AdapterCall<'_, Preflight> {
        Box::pin(async move {
            if ask.id == "old-hook" {
                return std::future::pending().await;
            }
            Ok(Preflight::Ask)
        })
    }

    fn permission(&self, ask: PermissionAsk) -> AdapterCall<'_, Decision> {
        Box::pin(async move {
            assert_eq!(ask.tool_call_id.as_deref(), Some("tool-approve"));
            assert_eq!(ask.action, "shell");
            assert_eq!(ask.resources, ["echo fixture"]);
            self.asks.send(ask.id).unwrap();
            self.allow.notified().await;
            Ok(Decision::Always)
        })
    }

    fn question(&self, _: QuestionAsk) -> AdapterCall<'_, Answer> {
        Box::pin(async { panic!("no question expected") })
    }

    fn cancel_ask<'a>(&'a self, id: &'a str) -> AdapterCall<'a, ()> {
        Box::pin(async move {
            self.withdrawn.lock().unwrap().push(id.into());
            Ok(())
        })
    }
}

#[test]
#[ignore = "Fixture subprocess launched by the real holder"]
fn claude_wire_fixture() {
    // With one test thread the harness starts a "test … " line first: end it, so the next line
    // Claude's reader sees is the first frame.
    println!();
    let args: Vec<_> = env::args().collect();
    let resume = args
        .iter()
        .position(|arg| arg == "--resume")
        .map(|index| args[index + 1].clone());
    let native = args
        .iter()
        .position(|arg| arg == "--resume" || arg == "--session-id")
        .map(|index| args[index + 1].clone())
        .unwrap();
    // Claude Code's transcript keeps each user message it takes, by the UUID it was sent with.
    let transcript = {
        let home = env::var_os("HOME")
            .or_else(|| env::var_os("USERPROFILE"))
            .unwrap();
        let folder = PathBuf::from(home).join(".claude/projects/fixture-project");
        fs::create_dir_all(&folder).unwrap();
        folder.join(format!("{native}.jsonl"))
    };
    let took = |uuid: &Value| {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&transcript)
            .unwrap();
        writeln!(file, "{}", json!({"type":"user","uuid":uuid})).unwrap();
    };
    let ask = json!({
        "type":"control_request","request_id":"approval","request":{
            "subtype":"can_use_tool","tool_name":"Bash","input":{"command":"echo fixture"},
            "tool_use_id":"tool-approve","permission_suggestions":[{
                "type":"addRules","destination":"localSettings","behavior":"allow","rules":[]
            }]
        }
    });
    let mut open = false;
    let mut steered = None;
    for line in io::stdin().lock().lines() {
        let message: Value = serde_json::from_str(&line.unwrap()).unwrap();
        match message["type"].as_str() {
            Some("control_request") => {
                let pending = if open && message["request"]["subtype"] == "initialize" {
                    vec![ask.clone()]
                } else {
                    Vec::new()
                };
                emit(json!({"type":"control_response","response":{
                    "subtype":"success","request_id":message["request_id"],"response":{},
                    "pending_permission_requests":pending
                }}));
                if open && message["request"]["subtype"] == "initialize" {
                    emit(json!({"type":"control_cancel_request","request_id":"old-hook"}));
                    emit(ask.clone());
                }
                if open && message["request"]["subtype"] == "interrupt" {
                    open = false;
                    emit(json!({"type":"control_cancel_request","request_id":"approval"}));
                    emit(
                        json!({"type":"result","subtype":"error_during_execution","is_error":true}),
                    );
                }
            }
            Some("user") if message["priority"] == "next" => {
                if message["message"]["content"][0]["text"] == "withdraw" {
                    emit(json!({"type":"control_cancel_request", "request_id":"approval"}));
                    open = false;
                    took(&message["uuid"]);
                    result(vec![message["uuid"].clone()]);
                } else {
                    assert!(steered.is_none(), "a message reached the CLI twice");
                    steered = Some(message["uuid"].clone());
                }
            }
            Some("user") => {
                took(&message["uuid"]);
                let text = message["message"]["content"][0]["text"].as_str().unwrap();
                match text {
                    "/clear" => {
                        emit(
                            json!({"type":"conversation_reset","session_id":INITIAL,"new_conversation_id":CLEARED}),
                        );
                        result(Vec::new());
                    }
                    "exit with error" => {
                        eprintln!("Could not authenticate with Bearer fixture.secret");
                        std::process::exit(1);
                    }
                    "turn the sign-in away" => {
                        let words = "Failed to authenticate: OAuth session expired and could not be refreshed";
                        emit(
                            json!({"type":"assistant","error":"authentication_failed","message":{
                                "id":"refused","model":"haiku","content":[{"type":"text","text":words}]
                            }}),
                        );
                        emit(
                            json!({"type":"result","subtype":"success","is_error":true,"result":words,
                            "user_message_uuids":[message["uuid"]]}),
                        );
                    }
                    "hear the limits" | "hear the limits after a sign-in" => {
                        if text.ends_with("sign-in") {
                            let home = env::var_os("HOME")
                                .or_else(|| env::var_os("USERPROFILE"))
                                .unwrap();
                            fs::write(
                                PathBuf::from(home).join(".claude.json"),
                                json!({"oauthAccount": {"accountUuid": "C-3"}}).to_string(),
                            )
                            .unwrap();
                        }
                        emit(json!({"type":"rate_limit_event","rate_limit_info":{
                            "status":"allowed","rateLimitType":"five_hour","utilization":0.3,
                            "unifiedWindows":{
                                "five_hour":{"utilization":0.3,"resetsAt":1_794_100_000},
                                "seven_day":{"utilization":0.81,"resetsAt":1_794_200_000}
                            }
                        }}));
                        result(vec![message["uuid"].clone()]);
                    }
                    "resume" => {
                        assistant("resume", &format!("resumed {}", resume.as_deref().unwrap()));
                        result(Vec::new());
                    }
                    _ => {
                        emit(json!({"type":"system","subtype":"init","session_id":INITIAL}));
                        assistant("first", "starts here");
                        if text == "ask and reply" {
                            emit(
                                json!({"type":"control_request","request_id":"old-hook","request":{
                                    "subtype":"hook_callback","callback_id":"kuvo-pre-tool-use",
                                    "input":{"hook_event_name":"PreToolUse","tool_name":"Bash",
                                        "tool_input":{"command":"echo fixture"},"tool_use_id":"tool-hook"}
                                }}),
                            );
                        }
                        emit(ask.clone());
                        open = true;
                    }
                }
            }
            // Stopped while it asked: it's told no.
            Some("control_response") if message["response"]["response"]["behavior"] == "deny" => {
                assert!(!open);
                assert_eq!(message["response"]["request_id"], "approval");
            }
            Some("control_response") => {
                assert!(open);
                assert_eq!(message["response"]["request_id"], "approval");
                assert_eq!(message["response"]["response"]["behavior"], "allow");
                assert_eq!(
                    message["response"]["response"]["updatedPermissions"][0]["destination"],
                    "session"
                );
                open = false;
                result(Vec::new());
                let steered = steered.take().unwrap();
                took(&steered);
                assistant("second", "second message answered");
                result(vec![steered]);
            }
            _ => panic!("unexpected fixture input"),
        }
    }
}

fn assistant(id: &str, text: &str) {
    emit(
        json!({"type":"assistant","message":{"id":id,"model":"haiku","content":[{"type":"text","text":text}]}}),
    );
}

fn result(ids: Vec<Value>) {
    emit(json!({"type":"result","subtype":"success","is_error":false,"user_message_uuids":ids}));
}

fn emit(value: Value) {
    let mut out = io::stdout().lock();
    writeln!(out, "{value}").unwrap();
    out.flush().unwrap();
}
