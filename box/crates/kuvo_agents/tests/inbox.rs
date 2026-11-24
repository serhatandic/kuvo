//! What waits for a coding agent and when it goes, through the resident's
//! sessions with a scripted stand-in agent. Each case names the bridge's
//! `sessions.test.ts` case it ports.

use kuvo_agents::{
    account_check::{AccountStatus, CheckFailure},
    accounts::{Account, AgentKind},
    catalog::Models,
    hold::{Controller, Record, TurnNote},
    inbox::{Delivery, InboxEvent, Input},
    permissions::ConfigLocation,
    resident::ResidentAgents,
    roster::{Agent, Factory},
    runtime::{AdapterCall, Adoption, Runtime},
    session::{
        ActiveTurn, EventEnvelope, Outcome, Prompt, SessionContext, Steer, SteerReceipt, TextKind,
        Thread, TurnError, TurnEvent, TurnInput, TurnKind, TurnResult, TurnSink,
    },
    session_runs::{Cancelled, SessionRuns},
    session_store::{MessageOrder, SessionStore},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{Notify, mpsc, oneshot};

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// Answers and ends on its own.
    Replies,
    /// Works until stopped or released.
    Works,
}

#[derive(Clone, Copy, PartialEq)]
enum Steering {
    Takes,
    TurnsAway,
    InDoubt,
    /// Its turns take nothing partway.
    Never,
}

struct Script {
    mode: Mode,
    steering: Steering,
    receipts: bool,
    /// A steer waits for this before answering.
    gate: Option<Arc<Notify>>,
    resolve: Option<bool>,
    fails_to_open: bool,
    runs: Vec<String>,
    steered: Vec<String>,
    release: Vec<oneshot::Sender<Outcome>>,
}

type Shared = Arc<Mutex<Script>>;

fn script() -> Shared {
    Arc::new(Mutex::new(Script {
        mode: Mode::Replies,
        steering: Steering::Never,
        receipts: false,
        gate: None,
        resolve: None,
        fails_to_open: false,
        runs: Vec::new(),
        steered: Vec::new(),
        release: Vec::new(),
    }))
}

struct Stand(Shared);

impl Factory for Stand {
    fn create(&self, _: &Account, _: Option<PathBuf>) -> std::io::Result<Arc<dyn Agent>> {
        Ok(Arc::new(StandAgent(self.0.clone())))
    }
}

struct StandAgent(Shared);

impl Adoption for StandAgent {
    fn accepts(&self, _: &Record, _: &BTreeMap<String, TurnNote>) -> bool {
        false
    }

    fn attach<'a>(
        &'a self,
        _: &'a Account,
        _: Record,
        _: BTreeMap<String, TurnNote>,
        _: Controller,
    ) -> AdapterCall<'a, ()> {
        Box::pin(async { Ok(()) })
    }
}

impl Agent for StandAgent {
    fn probe_account(
        &self,
    ) -> futures_util::future::BoxFuture<'_, Result<AccountStatus, CheckFailure>> {
        Box::pin(async { Err(CheckFailure::TimedOut) })
    }

    fn models(&self, _: bool) -> AdapterCall<'_, Models> {
        Box::pin(async { Ok(Models::default()) })
    }

    fn open_thread(&self, _: SessionContext) -> AdapterCall<'_, Arc<dyn Thread>> {
        let script = self.0.clone();
        Box::pin(async move {
            if script.lock().unwrap().fails_to_open {
                return Err(std::io::Error::other("It couldn't start."));
            }
            let thread: Arc<dyn Thread> = Arc::new(StandThread(script));
            Ok(thread)
        })
    }

    fn shutdown(&self) -> AdapterCall<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

struct StandThread(Shared);

impl Thread for StandThread {
    fn start(
        &self,
        input: TurnInput,
        sink: Arc<dyn TurnSink>,
    ) -> AdapterCall<'_, Arc<dyn ActiveTurn>> {
        let script = self.0.clone();
        Box::pin(async move {
            let text = match &input.kind {
                TurnKind::Prompt(prompt) => prompt.text.clone(),
                _ => String::new(),
            };
            let (stop, stopped) = mpsc::channel(1);
            let (release, released) = oneshot::channel();
            let mode = {
                let mut script = script.lock().unwrap();
                script.runs.push(text.clone());
                script.release.push(release);
                script.mode
            };
            tokio::spawn(run(sink, mode, text, stopped, released));
            let turn: Arc<dyn ActiveTurn> = Arc::new(StandTurn { script, stop });
            Ok(turn)
        })
    }

    fn takes_steers(&self) -> bool {
        self.0.lock().unwrap().steering != Steering::Never
    }

    fn resolve_steer(&self, _: SteerReceipt) -> AdapterCall<'_, Option<bool>> {
        let answer = self.0.lock().unwrap().resolve;
        Box::pin(async move { Ok(answer) })
    }
}

async fn run(
    sink: Arc<dyn TurnSink>,
    mode: Mode,
    text: String,
    mut stopped: mpsc::Receiver<()>,
    released: oneshot::Receiver<Outcome>,
) {
    let say = |event| EventEnvelope {
        position: None,
        delivery: kuvo_agents::session::Delivery::Live,
        event,
    };
    let _ = sink
        .event(say(TurnEvent::StepStarted { model: None }))
        .await;
    let mut state = kuvo_agents::session::VendorState::new();
    state.insert("nativeID".into(), json!("stand-native"));
    let _ = sink.save_state(state).await;
    let words = match mode {
        Mode::Replies => format!("on: {text}"),
        Mode::Works => "working".into(),
    };
    let _ = sink
        .event(say(TurnEvent::TextDelta {
            kind: TextKind::Text,
            block: None,
            delta: words,
        }))
        .await;
    let outcome = match mode {
        Mode::Replies => Outcome::Succeeded,
        Mode::Works => tokio::select! {
            _ = stopped.recv() => Outcome::Interrupted,
            outcome = released => outcome.unwrap_or(Outcome::Interrupted),
        },
    };
    let _ = sink
        .event(say(TurnEvent::StepEnded {
            usage: None,
            finish: None,
            error: None,
        }))
        .await;
    let _ = sink
        .event(say(TurnEvent::Finished(TurnResult {
            outcome,
            took_prompt: true,
            missed: Vec::new(),
        })))
        .await;
}

struct StandTurn {
    script: Shared,
    stop: mpsc::Sender<()>,
}

impl ActiveTurn for StandTurn {
    fn steer_receipt(&self, prompt: &Prompt) -> Option<SteerReceipt> {
        self.script.lock().unwrap().receipts.then(|| SteerReceipt {
            thread: "native".into(),
            turn: "turn".into(),
            message: prompt.id.clone(),
        })
    }

    fn steer(&self, prompt: Prompt) -> AdapterCall<'_, Steer> {
        Box::pin(async move {
            let gate = self.script.lock().unwrap().gate.clone();
            if let Some(gate) = gate {
                gate.notified().await;
            }
            let mut script = self.script.lock().unwrap();
            Ok(match script.steering {
                Steering::Takes => {
                    script.steered.push(prompt.text);
                    Steer::Accepted
                }
                Steering::InDoubt => Steer::InDoubt(SteerReceipt {
                    thread: "native".into(),
                    turn: "turn".into(),
                    message: prompt.id,
                }),
                Steering::TurnsAway | Steering::Never => Steer::Declined,
            })
        })
    }

    fn interrupt(&self) -> AdapterCall<'_, ()> {
        Box::pin(async move {
            let _ = self.stop.try_send(());
            Ok(())
        })
    }
}

struct Case {
    _scratch: tempfile::TempDir,
    root: PathBuf,
    store: SessionStore,
    script: Shared,
    runs: SessionRuns,
    sent: u32,
}

impl Case {
    async fn new() -> Self {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().to_owned();
        let store = SessionStore::new(root.join("agents/sessions"));
        let script = script();
        let runs = start(&root, &store, &script).await;
        Self {
            _scratch: scratch,
            root,
            store,
            script,
            runs,
            sent: 0,
        }
    }

    /// The resident starting again on the same conversations.
    async fn restart(&mut self) {
        self.runs = start(&self.root, &self.store, &self.script).await;
    }

    fn create(&self, id: &str) -> String {
        let folder = self.root.join("agents/sessions").join(id);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("session.json"),
            json!({
                "id": id, "projectID": "stand", "agent": "build",
                "location": {"directory": self.root},
                "model": {"providerID": "codex-cli", "id": "one"},
                "time": {"created": 1, "updated": 1},
            })
            .to_string(),
        )
        .unwrap();
        id.to_owned()
    }

    async fn send(&mut self, id: &str, text: &str, delivery: Delivery) -> String {
        self.sent += 1;
        let message = format!("msg_test{:08}", self.sent);
        self.runs
            .prompt(
                id.into(),
                Input::new(message.clone(), text.into(), Vec::new(), delivery),
                None,
            )
            .await
            .unwrap();
        message
    }

    fn with<T>(&self, read: impl FnOnce(&mut Script) -> T) -> T {
        read(&mut self.script.lock().unwrap())
    }

    fn runs(&self) -> Vec<String> {
        self.with(|script| script.runs.clone())
    }

    fn release(&self, outcome: Outcome) {
        let waiting = self.with(|script| std::mem::take(&mut script.release));
        for turn in waiting {
            let _ = turn.send(outcome.clone());
        }
    }

    async fn rows(&self, id: &str) -> Vec<Value> {
        self.store
            .message_page(id, None, 500, MessageOrder::OldestFirst)
            .await
            .unwrap()
            .map(|page| page.messages)
            .unwrap_or_default()
    }

    async fn idles(&self, id: &str) -> Vec<Value> {
        let rows = self.rows(id).await;
        rows.into_iter()
            .filter(|row| row["type"] == "idle")
            .collect()
    }

    /// `count` turns over, and nothing running.
    async fn settled(&self, id: &str, count: usize) {
        let mut tries = 0;
        loop {
            if self.idles(id).await.len() >= count && !self.runs.running().contains(id) {
                // A moment for what the end starts.
                tokio::time::sleep(Duration::from_millis(30)).await;
                if self.idles(id).await.len() >= count && !self.runs.running().contains(id) {
                    return;
                }
            }
            tries += 1;
            assert!(tries < 500, "{id} didn't settle at {count} turns");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn inbox(&self, id: &str) -> Vec<Input> {
        self.runs.inbox(id).await.unwrap().unwrap_or_default()
    }
}

async fn start(root: &Path, store: &SessionStore, script: &Shared) -> SessionRuns {
    let home = kuvo_store::home::Home::new(root.to_owned()).unwrap();
    let factories = HashMap::from([(
        AgentKind::Codex,
        Arc::new(Stand(script.clone())) as Arc<dyn Factory>,
    )]);
    let (agents, _) = ResidentAgents::start(Runtime::for_home(&home), factories)
        .await
        .unwrap();
    SessionRuns::start(
        agents,
        store.clone(),
        root.to_owned(),
        ConfigLocation::Machine,
    )
}

async fn until(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..500 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out: {what}");
}

fn deliveries(line: &[Input]) -> Vec<Delivery> {
    line.iter().map(|input| input.delivery).collect()
}

// sessions.test.ts: "a message starts at once, and one sent meanwhile waits for it"
#[tokio::test]
async fn a_message_starts_at_once_and_one_sent_meanwhile_waits_for_it() {
    let mut case = Case::new().await;
    case.with(|script| script.mode = Mode::Works);
    let id = case.create("ses_waits");
    case.send(&id, "first", Delivery::Steer).await;
    until("the first turn", || case.runs().len() == 1).await;
    case.send(&id, "second", Delivery::Steer).await;
    assert_eq!(deliveries(&case.inbox(&id).await), [Delivery::Queue]);
    case.release(Outcome::Interrupted);
    // Stopped: the one waiting stays.
    case.settled(&id, 1).await;
    assert_eq!(case.inbox(&id).await.len(), 1);
    assert_eq!(case.runs().len(), 1);
}

// "after a turn that ends on its own, the next one goes"
#[tokio::test]
async fn after_a_turn_that_ends_on_its_own_the_next_one_goes() {
    let mut case = Case::new().await;
    case.with(|script| script.mode = Mode::Works);
    let id = case.create("ses_next");
    case.send(&id, "first", Delivery::Steer).await;
    until("the first turn", || case.runs().len() == 1).await;
    case.send(&id, "second", Delivery::Steer).await;
    case.with(|script| script.mode = Mode::Replies);
    case.release(Outcome::Succeeded);
    case.settled(&id, 2).await;
    assert_eq!(case.runs(), ["first", "second"]);
    assert!(case.inbox(&id).await.is_empty());
}

// "Stop holds what waits; steering it starts it"
#[tokio::test]
async fn stop_holds_what_waits_and_steering_it_starts_it() {
    let mut case = Case::new().await;
    case.with(|script| script.mode = Mode::Works);
    let id = case.create("ses_stop");
    case.send(&id, "first", Delivery::Steer).await;
    until("the first turn", || case.runs().len() == 1).await;
    let waiting = case.send(&id, "second", Delivery::Steer).await;
    assert!(case.runs.interrupt(id.clone()).await.unwrap());
    case.settled(&id, 1).await;
    assert_eq!(case.idles(&id).await[0]["outcome"], "interrupted");
    assert_eq!(case.runs().len(), 1);
    case.with(|script| script.mode = Mode::Replies);
    case.runs
        .set_delivery(id.clone(), waiting, Delivery::Steer, false)
        .await
        .unwrap();
    case.settled(&id, 2).await;
    assert_eq!(case.runs(), ["first", "second"]);
}

// "Send Now: the message you picked goes next, not the first in line"
#[tokio::test]
async fn send_now_starts_the_message_you_picked_then_the_rest_in_line() {
    let mut case = Case::new().await;
    case.with(|script| script.mode = Mode::Works);
    let id = case.create("ses_now");
    case.send(&id, "first", Delivery::Steer).await;
    until("the first turn", || case.runs().len() == 1).await;
    case.send(&id, "second", Delivery::Steer).await;
    let picked = case.send(&id, "third", Delivery::Steer).await;
    case.with(|script| script.mode = Mode::Replies);
    case.runs
        .set_delivery(id.clone(), picked, Delivery::Steer, false)
        .await
        .unwrap();
    assert!(case.runs.interrupt_and(id.clone(), true).await.unwrap());
    case.settled(&id, 3).await;
    assert_eq!(case.runs(), ["first", "third", "second"]);
    assert!(case.inbox(&id).await.is_empty());
}

// "a queued message while nothing runs goes at once, as the engine's do;
// behind others held, it waits"
#[tokio::test]
async fn a_queued_message_goes_at_once_when_nothing_runs_but_waits_behind_held_ones() {
    let mut case = Case::new().await;
    let id = case.create("ses_queued");
    case.send(&id, "now", Delivery::Queue).await;
    case.settled(&id, 1).await;
    assert_eq!(case.runs().len(), 1);
    case.with(|script| script.mode = Mode::Works);
    case.send(&id, "working", Delivery::Steer).await;
    until("the second turn", || case.runs().len() == 2).await;
    let held = case.send(&id, "held", Delivery::Queue).await;
    case.runs.interrupt(id.clone()).await.unwrap();
    case.settled(&id, 2).await;
    // Held after the Stop: one queued behind it waits too, until a nudge.
    case.send(&id, "behind", Delivery::Queue).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(case.runs().len(), 2);
    case.with(|script| script.mode = Mode::Replies);
    case.runs
        .set_delivery(id.clone(), held, Delivery::Steer, false)
        .await
        .unwrap();
    case.settled(&id, 4).await;
    assert_eq!(case.runs(), ["now", "working", "held", "behind"]);
}

// "after an error, nothing more starts on its own"
#[tokio::test]
async fn after_an_error_nothing_more_starts_on_its_own() {
    let mut case = Case::new().await;
    case.with(|script| script.mode = Mode::Works);
    let id = case.create("ses_error");
    case.send(&id, "first", Delivery::Steer).await;
    until("the first turn", || case.runs().len() == 1).await;
    case.send(&id, "second", Delivery::Steer).await;
    case.release(Outcome::Failed(TurnError {
        kind: "agent".into(),
        message: "broke".into(),
        status: None,
        limit: None,
    }));
    case.settled(&id, 1).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(case.runs().len(), 1);
    assert_eq!(case.idles(&id).await[0]["error"]["message"], "broke");
}

// "an agent that couldn't start fails the message, and the next starts it again"
#[tokio::test]
async fn an_agent_that_cannot_start_fails_the_message_and_the_next_starts_again() {
    let mut case = Case::new().await;
    case.with(|script| script.fails_to_open = true);
    let id = case.create("ses_broken");
    let first = case.send(&id, "first", Delivery::Steer).await;
    case.settled(&id, 1).await;
    let rows = case.rows(&id).await;
    assert_eq!(rows[0]["id"], first.as_str());
    assert_eq!(rows[0]["text"], "first");
    assert_eq!(rows.last().unwrap()["outcome"], "failed");
    assert_eq!(
        rows.last().unwrap()["error"]["message"],
        "It couldn't start."
    );
    case.with(|script| script.fails_to_open = false);
    case.send(&id, "second", Delivery::Steer).await;
    case.settled(&id, 2).await;
    assert_eq!(case.runs(), ["second"]);
}

// "a message sent twice goes once"
#[tokio::test]
async fn a_message_sent_twice_goes_once() {
    let case = Case::new().await;
    let id = case.create("ses_twice");
    for _ in 0..2 {
        case.runs
            .prompt(
                id.clone(),
                Input::new(
                    "msg_twice0000001".into(),
                    "hi".into(),
                    Vec::new(),
                    Delivery::Steer,
                ),
                None,
            )
            .await
            .unwrap();
    }
    case.settled(&id, 1).await;
    case.runs
        .prompt(
            id.clone(),
            Input::new(
                "msg_twice0000001".into(),
                "hi".into(),
                Vec::new(),
                Delivery::Steer,
            ),
            None,
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(case.runs().len(), 1);
}

// "a message's metadata stays with it, waiting and sent"
#[tokio::test]
async fn a_messages_metadata_stays_with_it_waiting_and_sent() {
    let mut case = Case::new().await;
    case.with(|script| script.mode = Mode::Works);
    let mut events = case.runs.events();
    let id = case.create("ses_marked");
    case.send(&id, "first", Delivery::Steer).await;
    until("the first turn", || case.runs().len() == 1).await;
    let metadata = json!({"kuvo": {"note": {"from": "Goal", "line": "Goal started"}}});
    let mut marked = Input::new(
        "msg_marked000001".into(),
        "[Goal started] Work through it now.".into(),
        Vec::new(),
        Delivery::Steer,
    );
    marked.metadata = metadata.as_object().cloned();
    case.runs.prompt(id.clone(), marked, None).await.unwrap();
    let listed = case.inbox(&id).await[0].listed(&id);
    assert_eq!(listed["payload"]["metadata"], metadata);
    let enqueued = loop {
        match events.recv().await.unwrap() {
            InboxEvent::Enqueued { input, .. } if input.id == "msg_marked000001" => break *input,
            _ => {}
        }
    };
    assert_eq!(enqueued.item().1["metadata"], metadata);
    case.with(|script| script.mode = Mode::Replies);
    case.release(Outcome::Interrupted);
    case.settled(&id, 1).await;
    case.runs
        .set_delivery(
            id.clone(),
            "msg_marked000001".into(),
            Delivery::Steer,
            false,
        )
        .await
        .unwrap();
    case.settled(&id, 2).await;
    assert_eq!(
        case.runs(),
        ["first", "[Goal started] Work through it now."]
    );
    let rows = case.rows(&id).await;
    let row = rows
        .iter()
        .find(|row| row["id"] == "msg_marked000001")
        .unwrap();
    assert_eq!(row["type"], "user");
    assert_eq!(row["metadata"], metadata);
}

// "what waits outlasts the bridge"; "a prompt receipt requires the accepted
// message ID"
#[tokio::test]
async fn what_waits_outlasts_the_resident_and_is_confirmed_by_its_id() {
    let mut case = Case::new().await;
    case.with(|script| script.mode = Mode::Works);
    let id = case.create("ses_outlasts");
    case.send(&id, "first", Delivery::Steer).await;
    until("the first turn", || case.runs().len() == 1).await;
    let waiting = case.send(&id, "second", Delivery::Steer).await;
    assert!(
        !case
            .runs
            .confirm_prompt(id.clone(), "msg_Unknown".into())
            .await
            .unwrap()
    );
    assert!(
        case.runs
            .confirm_prompt(id.clone(), waiting.clone())
            .await
            .unwrap()
    );
    case.restart().await;
    let line = case.inbox(&id).await;
    assert_eq!(line.len(), 1);
    assert_eq!(line[0].text, "second");
    let saved: Value = serde_json::from_slice(
        &std::fs::read(
            case.root
                .join("agents/sessions")
                .join(&id)
                .join("agent.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(saved["inbox"][0]["id"], waiting.as_str());
    assert_eq!(saved["inbox"][0]["delivery"], "queue");
}

// "a message sent while a session is being deleted starts nothing";
// "deleting a session starts nothing that waited"
#[tokio::test]
async fn deleting_a_session_starts_nothing_that_waited_or_came_late() {
    let mut case = Case::new().await;
    case.with(|script| script.mode = Mode::Works);
    let id = case.create("ses_deleted");
    case.send(&id, "first", Delivery::Steer).await;
    until("the first turn", || case.runs().len() == 1).await;
    case.send(&id, "second", Delivery::Steer).await;
    assert!(case.runs.delete(id.clone()).await.unwrap());
    let late = case
        .runs
        .prompt(
            id.clone(),
            Input::new(
                "msg_late00000001".into(),
                "late".into(),
                Vec::new(),
                Delivery::Steer,
            ),
            None,
        )
        .await;
    assert_eq!(late.unwrap_err().kind(), std::io::ErrorKind::NotFound);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(case.runs().len(), 1);
    assert!(!case.runs.running().contains(&id));
    assert!(case.store.read(&id).await.unwrap().is_none());
}

// "a message for an agent that takes one partway goes into its turn at once"
#[tokio::test]
async fn a_message_for_an_agent_that_takes_one_partway_goes_into_its_turn() {
    let mut case = Case::new().await;
    case.with(|script| {
        script.mode = Mode::Works;
        script.steering = Steering::Takes;
    });
    let id = case.create("ses_steer");
    case.send(&id, "first", Delivery::Steer).await;
    until("the first turn", || case.runs().len() == 1).await;
    let steered = case.send(&id, "also this", Delivery::Steer).await;
    until("the steer", || {
        case.with(|script| script.steered.len()) == 1
    })
    .await;
    let mut tries = 0;
    while !case.inbox(&id).await.is_empty() {
        tries += 1;
        assert!(tries < 100);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        case.rows(&id)
            .await
            .iter()
            .any(|row| row["id"] == steered.as_str())
    );
    case.release(Outcome::Succeeded);
    case.settled(&id, 1).await;
    assert_eq!(case.runs(), ["first"]);
}

// "Steer on one queued: it goes into the turn under way, not after it"
#[tokio::test]
async fn steer_on_one_queued_puts_it_into_the_turn_under_way() {
    let mut case = Case::new().await;
    case.with(|script| {
        script.mode = Mode::Works;
        script.steering = Steering::Takes;
    });
    let id = case.create("ses_into");
    case.send(&id, "first", Delivery::Steer).await;
    until("the first turn", || case.runs().len() == 1).await;
    let queued = case.send(&id, "later", Delivery::Queue).await;
    assert_eq!(deliveries(&case.inbox(&id).await), [Delivery::Queue]);
    case.runs
        .set_delivery(id.clone(), queued, Delivery::Steer, true)
        .await
        .unwrap();
    until("the steer", || {
        case.with(|script| script.steered.clone()) == ["later"]
    })
    .await;
    case.release(Outcome::Succeeded);
    case.settled(&id, 1).await;
    assert_eq!(case.runs(), ["first"]);
}

// "Steer on one queued, for an agent that can't take one partway: it goes
// next, after the turn"
#[tokio::test]
async fn steer_for_an_agent_that_cannot_take_one_partway_goes_after_the_turn() {
    let mut case = Case::new().await;
    case.with(|script| script.mode = Mode::Works);
    let id = case.create("ses_after");
    case.send(&id, "first", Delivery::Steer).await;
    until("the first turn", || case.runs().len() == 1).await;
    let queued = case.send(&id, "later", Delivery::Queue).await;
    case.runs
        .set_delivery(id.clone(), queued, Delivery::Steer, true)
        .await
        .unwrap();
    assert_eq!(deliveries(&case.inbox(&id).await), [Delivery::Steer]);
    case.with(|script| script.mode = Mode::Replies);
    case.release(Outcome::Succeeded);
    case.settled(&id, 2).await;
    assert_eq!(case.runs(), ["first", "later"]);
}

// "a text file sent partway goes in the message, as for one that starts a turn"
#[tokio::test]
async fn a_text_file_sent_partway_goes_in_the_message() {
    let mut case = Case::new().await;
    case.with(|script| {
        script.mode = Mode::Works;
        script.steering = Steering::Takes;
    });
    let id = case.create("ses_file");
    case.send(&id, "first", Delivery::Steer).await;
    until("the first turn", || case.runs().len() == 1).await;
    let notes = case.root.join("notes.txt");
    std::fs::write(&notes, "the notes").unwrap();
    let uri = url::Url::from_file_path(&notes).unwrap().to_string();
    case.runs
        .prompt(
            id.clone(),
            Input::new(
                "msg_file00000001".into(),
                "read this".into(),
                vec![json!({"uri": uri})],
                Delivery::Steer,
            ),
            None,
        )
        .await
        .unwrap();
    until("the steer", || {
        case.with(|script| script.steered.len()) == 1
    })
    .await;
    assert_eq!(
        case.with(|script| script.steered[0].clone()),
        format!(
            "read this\n\n<file path=\"{}\">\nthe notes\n</file>",
            notes.display()
        )
    );
    case.release(Outcome::Succeeded);
    case.settled(&id, 1).await;
    let rows = case.rows(&id).await;
    let row = rows
        .iter()
        .find(|row| row["id"] == "msg_file00000001")
        .unwrap();
    assert_eq!(row["text"], "read this");
}

// "one cancelled while the agent takes it is sent all the same; one it turns
// away is cancelled"
#[tokio::test]
async fn one_cancelled_while_handed_is_sent_if_taken_and_cancelled_if_turned_away() {
    let mut case = Case::new().await;
    let gate = Arc::new(Notify::new());
    case.with(|script| {
        script.mode = Mode::Works;
        script.steering = Steering::Takes;
        script.gate = Some(gate.clone());
    });
    let id = case.create("ses_cancel");
    case.send(&id, "first", Delivery::Steer).await;
    until("the first turn", || case.runs().len() == 1).await;
    let taken = case.send(&id, "taken", Delivery::Steer).await;
    assert_eq!(
        case.runs
            .cancel_input(id.clone(), taken.clone(), true)
            .await
            .unwrap(),
        Cancelled::Busy
    );
    gate.notify_one();
    until("the steer", || {
        case.with(|script| script.steered.len()) == 1
    })
    .await;
    case.with(|script| script.steering = Steering::TurnsAway);
    let refused = case.send(&id, "refused", Delivery::Steer).await;
    assert_eq!(
        case.runs
            .cancel_input(id.clone(), refused, true)
            .await
            .unwrap(),
        Cancelled::Busy
    );
    gate.notify_one();
    let mut tries = 0;
    while !case.inbox(&id).await.is_empty() {
        tries += 1;
        assert!(tries < 100, "the turned-away message stayed");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        case.rows(&id)
            .await
            .iter()
            .any(|row| row["id"] == taken.as_str())
    );
    case.release(Outcome::Succeeded);
    case.settled(&id, 1).await;
    assert_eq!(case.runs(), ["first"]);
}

// "one it turns away waits for the turn to end"
#[tokio::test]
async fn one_it_turns_away_waits_for_the_turn_to_end() {
    let mut case = Case::new().await;
    case.with(|script| {
        script.mode = Mode::Works;
        script.steering = Steering::TurnsAway;
    });
    let id = case.create("ses_away");
    case.send(&id, "first", Delivery::Steer).await;
    until("the first turn", || case.runs().len() == 1).await;
    case.send(&id, "after", Delivery::Steer).await;
    let mut tries = 0;
    while deliveries(&case.inbox(&id).await) != [Delivery::Queue] {
        tries += 1;
        assert!(tries < 100, "the turned-away message wasn't queued");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    case.with(|script| script.mode = Mode::Replies);
    case.release(Outcome::Succeeded);
    case.settled(&id, 2).await;
    assert_eq!(case.runs(), ["first", "after"]);
}

// Codex's uncertain handoff: its receipt is saved before it's sent, nothing
// else in line overtakes it, and it's settled from the agent's conversation,
// never sent again.
#[tokio::test]
async fn an_uncertain_handoff_is_settled_by_its_receipt_never_sent_again() {
    let mut case = Case::new().await;
    case.with(|script| {
        script.mode = Mode::Works;
        script.steering = Steering::InDoubt;
        script.receipts = true;
    });
    let id = case.create("ses_doubt");
    case.send(&id, "first", Delivery::Steer).await;
    until("the first turn", || case.runs().len() == 1).await;
    let doubtful = case.send(&id, "maybe", Delivery::Steer).await;
    let mut tries = 0;
    while case
        .inbox(&id)
        .await
        .first()
        .and_then(|input| input.steer_receipt.clone())
        .is_none()
    {
        tries += 1;
        assert!(tries < 100, "the receipt wasn't saved");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let saved = case.inbox(&id).await[0].steer_receipt.clone().unwrap();
    assert_eq!(saved.provider, "codex-cli");
    assert_eq!(saved.receipt.message, doubtful);
    assert_eq!(
        case.runs
            .cancel_input(id.clone(), doubtful.clone(), true)
            .await
            .unwrap(),
        Cancelled::Busy
    );
    let refused = case
        .runs
        .set_delivery(id.clone(), doubtful.clone(), Delivery::Queue, false)
        .await
        .unwrap_err();
    assert_eq!(
        refused.to_string(),
        "Kuvo is checking whether Codex took this message."
    );
    // The turn ends: the uncertain one doesn't go as a turn of its own.
    case.with(|script| script.mode = Mode::Replies);
    case.release(Outcome::Succeeded);
    case.settled(&id, 1).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(case.runs(), ["first"]);
    // The agent's conversation shows it took it.
    case.with(|script| script.resolve = Some(true));
    let mut tries = 0;
    while !case.inbox(&id).await.is_empty() {
        tries += 1;
        assert!(tries < 300, "the uncertain message wasn't settled");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let rows = case.rows(&id).await;
    assert_eq!(
        rows.iter()
            .filter(|row| row["id"] == doubtful.as_str())
            .count(),
        1
    );
    assert_eq!(case.runs(), ["first"]);
    let saved: Value = serde_json::from_slice(
        &std::fs::read(
            case.root
                .join("agents/sessions")
                .join(&id)
                .join("agent.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        saved["threads"]["codex-cli"]["seenSteers"],
        json!([doubtful])
    );
}

// The same, where the agent's conversation shows it never took it: it goes
// next, once, as a turn of its own; and one restarted resident settles one
// left uncertain by the one before.
#[tokio::test]
async fn an_uncertain_handoff_the_agent_never_took_goes_next_also_after_a_restart() {
    let mut case = Case::new().await;
    case.with(|script| {
        script.mode = Mode::Works;
        script.steering = Steering::InDoubt;
        script.receipts = true;
    });
    let id = case.create("ses_missed");
    case.send(&id, "first", Delivery::Steer).await;
    until("the first turn", || case.runs().len() == 1).await;
    case.send(&id, "maybe", Delivery::Steer).await;
    let mut tries = 0;
    while case
        .inbox(&id)
        .await
        .first()
        .and_then(|input| input.steer_receipt.clone())
        .is_none()
    {
        tries += 1;
        assert!(tries < 100);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    case.with(|script| script.mode = Mode::Replies);
    case.release(Outcome::Succeeded);
    case.settled(&id, 1).await;
    case.restart().await;
    case.with(|script| script.resolve = Some(false));
    case.settled(&id, 2).await;
    assert_eq!(case.runs(), ["first", "maybe"]);
    assert!(case.inbox(&id).await.is_empty());
}

// "a late POST for this attempt can't arrive after a successful Delete":
// a cancelled handoff refuses its message; a new token (Undo) goes.
#[tokio::test]
async fn a_cancelled_handoff_refuses_its_late_message() {
    let case = Case::new().await;
    let id = case.create("ses_handoff");
    let token = "6f9619ff-8b86-d011-b42d-00cf4fc964ff".to_owned();
    assert!(
        case.runs
            .cancel_handoff(id.clone(), "msg_late00000001".into(), token.clone())
            .await
            .unwrap()
    );
    let late = case
        .runs
        .prompt(
            id.clone(),
            Input::new(
                "msg_late00000001".into(),
                "late".into(),
                Vec::new(),
                Delivery::Steer,
            ),
            Some(token),
        )
        .await
        .unwrap_err();
    assert_eq!(late.to_string(), "That handoff was cancelled.");
    case.runs
        .prompt(
            id.clone(),
            Input::new(
                "msg_late00000001".into(),
                "again".into(),
                Vec::new(),
                Delivery::Steer,
            ),
            Some("0f9619ff-8b86-d011-b42d-00cf4fc964ff".into()),
        )
        .await
        .unwrap();
    case.settled(&id, 1).await;
    assert_eq!(case.runs(), ["again"]);
    // Once it has the message, the handoff can't be cancelled.
    assert!(
        !case
            .runs
            .cancel_handoff(
                id,
                "msg_late00000001".into(),
                "1f9619ff-8b86-d011-b42d-00cf4fc964ff".into()
            )
            .await
            .unwrap()
    );
}
