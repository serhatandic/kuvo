//! Through a resident update: the turns an earlier resident's agents went on
//! with, taken up by the next with what they were asking and what you
//! approved; the rest closed, and what waited behind them kept. Real
//! sessions, store and asks; a scripted agent. The first three cases port
//! the bridge's `held-resume.test.ts`.

use kuvo_agents::{
    account_check::{AccountStatus, CheckFailure},
    accounts::{Account, AgentKind, Provider},
    catalog::Models,
    hold::{Controller, Record, TurnNote},
    inbox::{Delivery, Input},
    live_asks::AskKey,
    permissions::{Allowance, ConfigLocation},
    resident::ResidentAgents,
    roster::{Agent, Factory},
    runtime::{AdapterCall, AdoptedTurn, Adoption, Runtime},
    session::{
        ActiveTurn, Decision, EventEnvelope, Outcome, PermissionAsk, SessionContext, TextKind,
        Thread, TurnEvent, TurnInput, TurnKind, TurnResult, TurnSink,
    },
    session_runs::{HandedTurn, Handover, SessionRuns, Update},
    session_store::{MessageOrder, SessionStore},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

const TURN: &str = "msg_heldresume00000001";

#[derive(Clone, Copy, PartialEq)]
enum Recover {
    /// Its agent can't take the turn up.
    Fails,
    /// The agent's lines come again, saying something new.
    Replays,
    /// The agent asks again what it asked before, then asks something you
    /// approved for the rest of the turn.
    Asks,
}

struct Script {
    recover: Recover,
    let_go: Vec<String>,
    prompts: Vec<String>,
}

type Shared = Arc<Mutex<Script>>;

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

    fn let_go<'a>(&'a self, turn: &'a TurnNote) -> AdapterCall<'a, ()> {
        self.0.lock().unwrap().let_go.push(turn.session.clone());
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
        let thread: Arc<dyn Thread> = Arc::new(StandThread(self.0.clone()));
        Box::pin(async move { Ok(thread) })
    }

    fn shutdown(&self) -> AdapterCall<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

struct StandThread(Shared);
struct Quiet;

impl ActiveTurn for Quiet {
    fn interrupt(&self) -> AdapterCall<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

impl Thread for StandThread {
    fn start(
        &self,
        input: TurnInput,
        sink: Arc<dyn TurnSink>,
    ) -> AdapterCall<'_, Arc<dyn ActiveTurn>> {
        let script = self.0.clone();
        Box::pin(async move {
            let recover = match &input.kind {
                TurnKind::Recover { .. } => script.lock().unwrap().recover,
                TurnKind::Prompt(prompt) => {
                    script.lock().unwrap().prompts.push(prompt.text.clone());
                    // ("wait" works until the resident goes.)
                    if prompt.text == "wait" {
                        tokio::spawn(async move {
                            let _working = sink;
                            std::future::pending::<()>().await;
                        });
                    } else {
                        tokio::spawn(say_and_end(sink, vec!["done".into()]));
                    }
                    return Ok(Arc::new(Quiet) as Arc<dyn ActiveTurn>);
                }
                _ => return Err(std::io::ErrorKind::Unsupported.into()),
            };
            match recover {
                Recover::Fails => Err(std::io::Error::other(
                    "Claude Code isn't installed any more.",
                )),
                Recover::Replays => {
                    tokio::spawn(say_and_end(sink, vec!["After the crash.".into()]));
                    Ok(Arc::new(Quiet) as Arc<dyn ActiveTurn>)
                }
                Recover::Asks => {
                    tokio::spawn(async move {
                        let first = sink.permission(edit("900", "a.txt")).await;
                        let second = sink.permission(edit("901", "b.txt")).await;
                        say_and_end(sink, vec![format!("{first:?} {second:?}")]).await;
                    });
                    Ok(Arc::new(Quiet) as Arc<dyn ActiveTurn>)
                }
            }
        })
    }
}

fn edit(id: &str, file: &str) -> PermissionAsk {
    PermissionAsk {
        id: id.into(),
        tool_call_id: None,
        action: "edit".into(),
        resources: vec![file.into()],
        save: Vec::new(),
        message: None,
        metadata: Default::default(),
        sandboxed: None,
        already_weighed: false,
        unclassified: false,
    }
}

async fn say_and_end(sink: Arc<dyn TurnSink>, words: Vec<String>) {
    let say = |event| EventEnvelope {
        position: None,
        delivery: kuvo_agents::session::Delivery::Live,
        event,
    };
    let _ = sink.event(say(TurnEvent::StepStarted { model: None })).await;
    for delta in words {
        let _ = sink
            .event(say(TurnEvent::TextDelta {
                kind: TextKind::Text,
                block: None,
                delta,
            }))
            .await;
    }
    let _ = sink
        .event(say(TurnEvent::StepEnded {
            usage: None,
            finish: None,
            error: None,
        }))
        .await;
    let _ = sink
        .event(say(TurnEvent::Finished(TurnResult {
            outcome: Outcome::Succeeded,
            took_prompt: true,
            missed: Vec::new(),
        })))
        .await;
}

struct Case {
    _scratch: tempfile::TempDir,
    root: PathBuf,
    store: SessionStore,
    script: Shared,
}

impl Case {
    /// A conversation whose turn an earlier resident left under way: it said
    /// something, ran a tool, and was cut off.
    async fn left_under_way(recover: Recover) -> Self {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().to_owned();
        let store = SessionStore::new(root.join("agents/sessions"));
        let case = Self {
            _scratch: scratch,
            root,
            store,
            script: Arc::new(Mutex::new(Script {
                recover,
                let_go: Vec::new(),
                prompts: Vec::new(),
            })),
        };
        let folder = case.root.join("agents/sessions/ses_held");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("session.json"),
            json!({
                "id": "ses_held", "projectID": "stand", "agent": "build",
                "location": {"directory": case.root},
                "model": {"providerID": "codex-cli", "id": "one"},
                "time": {"created": 1, "updated": 1},
                "permissions": [{"action": "edit", "resource": "*", "effect": "ask"}],
            })
            .to_string(),
        )
        .unwrap();
        let step = format!("msg_{TURN}_1");
        assert!(
            case.store
                .write_messages(
                    "ses_held",
                    vec![
                        json!({"type":"user","id":TURN,"text":"build it","time":{"created":10}}),
                        json!({"type":"assistant","id":step,"agent":"build",
                            "model":{"providerID":"codex-cli","id":"one"},"time":{"created":11},
                            "content":[
                                {"type":"text","text":"Before the crash.","time":{"created":11,"streamed":12}},
                                {"type":"tool","id":"call_1","name":"shell",
                                    "state":{"status":"running","input":{"command":"make"}},
                                    "time":{"created":12,"ran":12}},
                            ]}),
                    ],
                )
                .await
                .unwrap()
        );
        case
    }

    fn adopted(&self) -> AdoptedTurn {
        AdoptedTurn {
            provider: Provider::parse("codex-cli").unwrap(),
            note: TurnNote {
                session: "ses_held".into(),
                from: 0,
                turn: Some(TURN.into()),
                request: None,
                sent_at: None,
                state: None,
            },
        }
    }

    /// The next resident, taking up `adopted` with `handover`.
    async fn next(&self, adopted: Vec<AdoptedTurn>, handover: Option<Handover>) -> SessionRuns {
        let home = kuvo_store::home::Home::new(self.root.clone()).unwrap();
        let factories = HashMap::from([(
            AgentKind::Codex,
            Arc::new(Stand(self.script.clone())) as Arc<dyn Factory>,
        )]);
        let (agents, _) = ResidentAgents::start(Runtime::for_home(&home), factories)
            .await
            .unwrap();
        SessionRuns::start_after(
            agents,
            self.store.clone(),
            self.root.clone(),
            ConfigLocation::Machine,
            Update { adopted, handover },
        )
    }

    async fn rows(&self) -> Vec<Value> {
        self.store
            .message_page("ses_held", None, 500, MessageOrder::OldestFirst)
            .await
            .unwrap()
            .unwrap()
            .messages
    }

    /// The conversation once its last turn has ended.
    async fn ended(&self) -> Vec<Value> {
        for _ in 0..500 {
            let rows = self.rows().await;
            if rows.last().is_some_and(|row| row["type"] == "idle") {
                return rows;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the turn didn't end: {:#?}", self.rows().await);
    }
}

fn texts(rows: &[Value]) -> Vec<String> {
    rows.iter()
        .filter(|row| row["type"] == "assistant")
        .flat_map(|row| row["content"].as_array().cloned().unwrap_or_default())
        .filter(|part| part["type"] == "text")
        .map(|part| part["text"].as_str().unwrap().to_owned())
        .collect()
}

fn outcomes(rows: &[Value]) -> Vec<Value> {
    rows.iter()
        .filter(|row| row["type"] == "idle")
        .map(|row| row["outcome"].clone())
        .collect()
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

// held-resume.test.ts: "a take-up that fails keeps what was shown of the
// turn, its tool closed, and stops the agent"
#[tokio::test]
async fn a_take_up_that_fails_keeps_what_was_shown_closes_its_tool_and_lets_the_agent_go() {
    let case = Case::left_under_way(Recover::Fails).await;
    let _runs = case.next(vec![case.adopted()], None).await;
    let rows = case.ended().await;
    assert_eq!(texts(&rows), ["Before the crash."]);
    let tool = &rows[1]["content"][1];
    assert_eq!(tool["state"]["status"], "error");
    assert_eq!(tool["state"]["error"]["message"], "Stopped.");
    assert!(rows[1]["time"]["completed"].is_u64());
    assert_eq!(rows.last().unwrap()["outcome"], "failed");
    assert_eq!(
        rows.last().unwrap()["error"]["message"],
        "Claude Code isn't installed any more."
    );
    let header = case.store.read("ses_held").await.unwrap().unwrap();
    assert_eq!(header.outcome(), Some("failed"));
    until("the agent let go", || {
        case.script.lock().unwrap().let_go == ["ses_held"]
    })
    .await;
}

// held-resume.test.ts: "a take-up that goes through makes the turn again
// from the agent's lines, once"
#[tokio::test]
async fn a_take_up_that_goes_through_makes_the_turn_again_from_the_agents_lines_once() {
    let case = Case::left_under_way(Recover::Replays).await;
    let _runs = case.next(vec![case.adopted()], None).await;
    let rows = case.ended().await;
    assert_eq!(texts(&rows), ["After the crash."]);
    assert_eq!(outcomes(&rows), [json!("succeeded")]);
    assert_eq!(rows.len(), 3, "{rows:#?}");
    assert!(case.script.lock().unwrap().let_go.is_empty());
}

// held-resume.test.ts: "a checkpoint without its holder cannot block
// approvals for the session"
#[tokio::test]
async fn a_handover_without_its_held_agent_cannot_hold_up_the_conversations_approvals() {
    let case = Case::left_under_way(Recover::Asks).await;
    let handover = Handover {
        sessions: BTreeMap::from([(
            "ses_held".into(),
            HandedTurn {
                turn: TURN.into(),
                asks: vec!["missing".into()],
                always: Vec::new(),
            },
        )]),
    };
    let runs = case.next(Vec::new(), Some(handover)).await;
    tokio::time::timeout(Duration::from_millis(200), runs.asks_restored("ses_held"))
        .await
        .expect("approvals ready at once");
    assert!(runs.asks().pending().borrow().is_empty());
    // (Its turn, no agent going on with it, is closed as a restart's.)
    let rows = case.ended().await;
    assert_eq!(rows.last().unwrap()["outcome"], "interrupted");
    assert_eq!(rows.last().unwrap()["reason"], "restart");
    assert_eq!(rows[1]["content"][1]["state"]["status"], "error");
}

#[tokio::test]
async fn a_turn_taken_up_shows_its_ask_again_once_and_keeps_what_you_approved() {
    let case = Case::left_under_way(Recover::Asks).await;
    let handover = Handover {
        sessions: BTreeMap::from([(
            "ses_held".into(),
            HandedTurn {
                turn: TURN.into(),
                asks: vec!["900".into()],
                always: vec![Allowance {
                    action: "edit".into(),
                    patterns: vec!["b.txt".into()],
                }],
            },
        )]),
    };
    let runs = case.next(vec![case.adopted()], Some(handover)).await;
    // A client's read waits for the ask to come again, and finds it once,
    // under the ID it had before the update.
    tokio::time::timeout(Duration::from_secs(5), runs.asks_restored("ses_held"))
        .await
        .expect("the ask came again");
    let key = AskKey {
        session: "ses_held".into(),
        turn: TURN.into(),
        id: "900".into(),
    };
    let shown = runs.asks().pending().borrow().keys().cloned().collect::<Vec<_>>();
    assert_eq!(shown, std::slice::from_ref(&key));
    // An answer for an ask that isn't waiting is refused.
    let stale = AskKey {
        id: "899".into(),
        ..key.clone()
    };
    let refused = runs
        .reply_permission(stale, Decision::Once, || false)
        .await
        .unwrap_err();
    assert_eq!(refused.kind(), std::io::ErrorKind::NotFound);
    runs.reply_permission(key.clone(), Decision::Once, || false)
        .await
        .unwrap();
    // The second ask is covered by what you approved before the update.
    let rows = case.ended().await;
    assert_eq!(texts(&rows), ["Ok(Once) Ok(Once)"]);
    assert_eq!(outcomes(&rows), [json!("succeeded")]);
    let again = runs
        .reply_permission(key, Decision::Once, || false)
        .await
        .unwrap_err();
    assert_eq!(again.kind(), std::io::ErrorKind::NotFound);
}

// sessions.test.ts 8: "the bridge stopping keeps what waits for the next,
// which sends it once, in order"
#[tokio::test]
async fn what_waits_behind_a_turn_that_wont_go_on_goes_once_in_order_after_the_update() {
    let case = Case::left_under_way(Recover::Replays).await;
    let runs = case.next(Vec::new(), None).await;
    // (This resident closed the turn it found unfinished.)
    case.ended().await;
    let script = case.script.clone();
    let send = |id: &str, text: &str, delivery| {
        let runs = runs.clone();
        let input = Input::new(id.into(), text.into(), Vec::new(), delivery);
        async move { runs.prompt("ses_held".into(), input, None).await.unwrap() }
    };
    // A turn under way, whose agent won't go on with it, and one behind it.
    send("msg_held00000002", "wait", Delivery::Steer).await;
    until("the turn under way", || script.lock().unwrap().prompts.len() == 1).await;
    send("msg_held00000003", "second", Delivery::Queue).await;
    let handover = runs.hand_over(Default::default()).await.unwrap();
    assert!(handover.sessions.is_empty());
    // One sent as the resident makes way is kept too, not started.
    send("msg_held00000004", "third", Delivery::Queue).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(script.lock().unwrap().prompts, ["wait"]);
    let line = case.store.line("ses_held").await.unwrap().unwrap();
    assert_eq!(line.len(), 2);
    assert!(line.iter().all(|input| input.held && input.delivery == Delivery::Steer));
    drop(runs);
    // The next resident closes the turn and sends them, once each, in order.
    let runs = case.next(Vec::new(), None).await;
    until("both went", || script.lock().unwrap().prompts.len() == 3).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(script.lock().unwrap().prompts, ["wait", "second", "third"]);
    assert!(runs.inbox("ses_held").await.unwrap().unwrap().is_empty());
    let rows = case.ended().await;
    let ends = rows
        .iter()
        .filter(|row| row["type"] == "idle")
        .map(|row| (row["outcome"].clone(), row["reason"].clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        ends,
        [
            (json!("interrupted"), json!("restart")),
            (json!("interrupted"), json!("restart")),
            (json!("succeeded"), Value::Null),
            (json!("succeeded"), Value::Null),
        ]
    );
}
