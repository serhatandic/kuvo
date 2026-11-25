use std::{io, path::Path, sync::Arc};

use futures_util::future::BoxFuture;
use serde_json::json;

use super::*;
use crate::{
    account_check::{AccountStatus, CheckFailure},
    accounts::AgentKind,
    acp::{
        flavour::{Launch, Mode},
        wire::{Capabilities, Setup},
    },
    catalog::Models,
    program_updates::Definition,
    rpc::Rpc,
    runtime::AdapterCall,
    sign_in::{Attempt, Request},
};

/// A flavour whose reads end only as its turn does, as Antigravity's.
struct Reads;

impl Flavour for Reads {
    fn kind(&self) -> AgentKind {
        AgentKind::Antigravity
    }
    fn name(&self) -> &'static str {
        "Agent"
    }
    fn probe(&self) -> BoxFuture<'_, Result<AccountStatus, CheckFailure>> {
        unimplemented!()
    }
    fn models(&self, _: bool) -> AdapterCall<'_, Models> {
        unimplemented!()
    }
    fn sign_in(&self, _: Request) -> AdapterCall<'_, Arc<dyn Attempt>> {
        unimplemented!()
    }
    fn sign_out(&self) -> AdapterCall<'_, ()> {
        unimplemented!()
    }
    fn remove_credentials(&self) -> AdapterCall<'_, ()> {
        unimplemented!()
    }
    fn program(&self) -> Option<Definition> {
        None
    }
    fn credential(&self) -> Option<String> {
        None
    }
    fn launch_key(&self, _: &Mode, _: &Path, _: &str) -> io::Result<String> {
        Ok(String::new())
    }
    fn launch(&self, _: &Mode, _: &Path, _: &str) -> io::Result<Launch> {
        unimplemented!()
    }
    fn auth_method(&self, _: &Capabilities) -> Option<String> {
        None
    }
    fn reads_unsaid(&self) -> bool {
        true
    }
    fn select_model<'a>(
        &'a self,
        _: &'a Rpc,
        _: &'a str,
        _: &'a str,
        _: Option<&'a str>,
        _: &'a mut Setup,
        _: Option<&'a str>,
    ) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

fn update(value: Value) -> Update {
    Update::read(&value)
}

fn tools(events: &[TurnEvent]) -> Vec<(String, &'static str)> {
    events
        .iter()
        .filter_map(|event| match event {
            TurnEvent::Tool { id, state, .. } => Some((
                id.clone(),
                match state {
                    ToolState::Running { .. } | ToolState::Streaming(_) => "running",
                    ToolState::Completed { .. } => "completed",
                    ToolState::Failed { .. } => "failed",
                },
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn words_after_a_tool_begin_a_new_step_and_a_call_merges_its_updates() {
    let known = KnownTools::default();
    let cx = Context {
        flavour: &Reads,
        directory: Path::new("/work"),
        model: "m",
        native: "s",
        known: &known,
    };
    let mut turn = Projection::new(0, false);
    let started = turn.update(&update(json!({"sessionUpdate":"tool_call","toolCallId":"t","kind":"execute","title":"Run `ls`","status":"in_progress"})), &cx);
    assert_eq!(tools(&started), [("t".into(), "running")]);
    let done = turn.update(&update(json!({"sessionUpdate":"tool_call_update","toolCallId":"t","status":"completed","rawOutput":{"stdout":"a b","exit_code":0}})), &cx);
    let TurnEvent::Tool {
        name,
        state: ToolState::Completed { input, .. },
        metadata,
        ..
    } = &done[0]
    else {
        panic!("{done:?}")
    };
    assert_eq!(
        (name.as_str(), input["command"].as_str()),
        ("shell", Some("ls"))
    );
    assert_eq!(metadata["exit"], 0);
    let said = turn.update(
        &update(
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Done."}}),
        ),
        &cx,
    );
    assert!(
        matches!(
            said[..],
            [
                TurnEvent::StepEnded { .. },
                TurnEvent::StepStarted { .. },
                TurnEvent::TextDelta { .. }
            ]
        ),
        "{said:?}"
    );
    assert_eq!(turn.said, "Done.");
}

#[test]
fn a_read_kuvo_did_shows_as_read_though_the_agent_says_so_only_as_the_turn_ends() {
    let known = KnownTools::default();
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("notes.txt");
    let cx = Context {
        flavour: &Reads,
        directory: folder.path(),
        model: "m",
        native: "s",
        known: &known,
    };
    let mut turn = Projection::new(0, false);
    // Read before it names the call: shown once it does.
    assert!(
        turn.served(&file, Served::Text("hello".into()), &cx)
            .is_empty()
    );
    let read = turn.update(&update(json!({"sessionUpdate":"tool_call","toolCallId":"r","kind":"read","title":"client_view_file","status":"pending","locations":[{"path": file}]})), &cx);
    assert_eq!(
        tools(&read),
        [("r".into(), "running"), ("r".into(), "completed")]
    );
    // Its own read of a file that isn't there fails as the turn ends; its own "done" after changes nothing.
    turn.update(&update(json!({"sessionUpdate":"tool_call","toolCallId":"v","kind":"read","title":"view_file","status":"pending","locations":[{"path":"/work/gone.txt"}]})), &cx);
    let ended = turn.end(&cx);
    assert_eq!(tools(&ended), [("v".into(), "failed")]);
    assert!(
        turn.update(
            &update(
                json!({"sessionUpdate":"tool_call_update","toolCallId":"r","status":"completed"})
            ),
            &cx
        )
        .is_empty()
    );
}

#[test]
fn its_cost_is_the_conversation_s_so_far_each_turn_adds_what_s_new() {
    let known = KnownTools::default();
    let cx = Context {
        flavour: &Reads,
        directory: Path::new("/"),
        model: "m",
        native: "s",
        known: &known,
    };
    let mut turn = Projection::new(0, false);
    let cost = |events: Vec<TurnEvent>| {
        events.into_iter().find_map(|event| {
            if let TurnEvent::Cost(cost) = event {
                Some(cost)
            } else {
                None
            }
        })
    };
    assert_eq!(
        cost(turn.update(
            &update(json!({"sessionUpdate":"usage_update","used":10,"cost":{"amount":0.5}})),
            &cx
        )),
        Some(0.5)
    );
    assert_eq!(
        cost(turn.update(
            &update(json!({"sessionUpdate":"usage_update","used":20,"cost":{"amount":0.75}})),
            &cx
        )),
        Some(0.25)
    );
}

#[test]
fn its_plan_is_one_list_done_as_the_turn_ends_and_a_subagent_s_task_names_its_row() {
    let known = KnownTools::default();
    let cx = Context {
        flavour: &Reads,
        directory: Path::new("/"),
        model: "m",
        native: "s",
        known: &known,
    };
    let mut turn = Projection::new(0, false);
    let first = turn.update(
        &update(json!({"sessionUpdate":"plan","entries":[{"content":"a","status":"pending"}]})),
        &cx,
    );
    let again = turn.update(
        &update(json!({"sessionUpdate":"plan","entries":[{"content":"a","status":"completed"}]})),
        &cx,
    );
    assert_eq!(tools(&first)[0].0, tools(&again)[0].0);
    assert!(turn.child("sub", &update(json!({"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"Find the bug\nin detail"}}))).is_empty());
    let row = turn.child(
        "sub",
        &update(
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"x"}}),
        ),
    );
    let TurnEvent::Tool {
        name,
        state: ToolState::Running { input },
        ..
    } = &row[0]
    else {
        panic!()
    };
    assert_eq!(
        (name.as_str(), input["description"].as_str()),
        ("subagent", Some("Find the bug"))
    );
    let ended = tools(&turn.end(&cx));
    assert!(
        ended.contains(&("acp-subagent:sub".into(), "completed"))
            && ended.contains(&(tools(&first)[0].0.clone(), "completed"))
    );
}
