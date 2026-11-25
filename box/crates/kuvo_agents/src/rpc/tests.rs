use super::*;
use crate::hold::Exit;

struct Agent {
    frames: mpsc::Sender<Output>,
    writes: mpsc::Receiver<Write>,
    sequence: u64,
}

impl Agent {
    async fn say(&mut self, line: &str) {
        self.sequence += 1;
        self.frames
            .send(Output::Line {
                sequence: self.sequence,
                line: line.into(),
                again: false,
            })
            .await
            .unwrap();
    }

    async fn written(&mut self) -> Write {
        tokio::time::timeout(Duration::from_secs(2), self.writes.recv())
            .await
            .expect("the connection wrote nothing")
            .expect("the connection closed its writer")
    }

    async fn line(&mut self) -> Value {
        loop {
            match self.written().await {
                Write::Line(line) => return serde_json::from_str(&line).unwrap(),
                Write::Ack(_) => continue,
                other => panic!("expected a line, got {other:?}"),
            }
        }
    }
}

fn connect(options: Options) -> (Rpc, mpsc::Receiver<Incoming>, Agent) {
    let (frames, read) = mpsc::channel(16);
    let (writes, written) = mpsc::channel(16);
    let (rpc, incoming) = Rpc::over_frames(read, writes, options, String::new());
    (
        rpc,
        incoming,
        Agent {
            frames,
            writes: written,
            sequence: 0,
        },
    )
}

async fn next(incoming: &mut mpsc::Receiver<Incoming>) -> Incoming {
    tokio::time::timeout(Duration::from_secs(2), incoming.recv())
        .await
        .expect("nothing came in")
        .expect("the connection ended")
}

#[tokio::test]
async fn a_request_gets_its_own_answer_in_either_framing() {
    let (rpc, _incoming, mut agent) = connect(Options {
        first_id: 7,
        ..Options::default()
    });
    let asking = tokio::spawn({
        let rpc = rpc.clone();
        async move {
            rpc.request("thread/start", Some(json!({"cwd":"/p"})), None)
                .await
        }
    });
    let sent = agent.line().await;
    assert_eq!(
        sent.to_string(),
        r#"{"id":7,"method":"thread/start","params":{"cwd":"/p"}}"#
    );
    agent.say(r#"{"id":7,"result":{"thread":"t1"}}"#).await;
    assert_eq!(asking.await.unwrap().unwrap(), json!({"thread":"t1"}));

    let (rpc, _incoming, mut agent) = connect(Options {
        framing: Framing::JsonRpc2,
        ..Options::default()
    });
    let asking = tokio::spawn(async move { rpc.request("initialize", None, None).await });
    assert_eq!(
        agent.line().await.to_string(),
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#
    );
    agent
        .say(r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"Authentication required","data":{"methods":["oauth"]}}}"#)
        .await;
    match asking.await.unwrap() {
        Err(Failure::Refused(error)) => {
            assert_eq!(error.code, -32000);
            assert_eq!(error.message, "Authentication required");
            assert_eq!(error.data, Some(json!({"methods":["oauth"]})));
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn the_agents_own_requests_and_notifications_reach_the_adapter() {
    let (rpc, mut incoming, mut agent) = connect(Options {
        framing: Framing::JsonRpc2,
        filter: Some(Box::new(|line: &str| !line.starts_with("debug"))),
        ..Options::default()
    });
    agent.say("debug: not protocol").await;
    agent.say("not json").await;
    agent
        .say("{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{\"n\":1}}\r")
        .await;
    agent
        .say(r#"{"jsonrpc":"2.0","id":"ask-1","method":"session/request_permission","params":{"tool":"x"}}"#)
        .await;
    let Incoming::Notification {
        method,
        params,
        sequence,
    } = next(&mut incoming).await
    else {
        panic!("expected the agent's notification");
    };
    assert_eq!(
        (method.as_str(), &params),
        ("session/update", &json!({"n":1}))
    );
    let Incoming::Request {
        id,
        method,
        params,
        sequence: asked,
    } = next(&mut incoming).await
    else {
        panic!("expected the agent's request");
    };
    assert_eq!(
        (method.as_str(), &params),
        ("session/request_permission", &json!({"tool":"x"}))
    );
    // Each says the holder's number for its own line.
    assert_eq!(asked, sequence + 1);
    rpc.respond(id.clone(), Ok(json!({"outcome":"allow"})))
        .await;
    assert_eq!(
        agent.line().await.to_string(),
        r#"{"jsonrpc":"2.0","id":"ask-1","result":{"outcome":"allow"}}"#
    );
    rpc.respond(id, Err(RpcError::not_found("fs/read"))).await;
    assert_eq!(
        agent.line().await.to_string(),
        r#"{"jsonrpc":"2.0","id":"ask-1","error":{"code":-32601,"message":"Method not found: fs/read"}}"#
    );
}

#[tokio::test]
async fn replayed_lines_come_once_and_late_answers_are_strays() {
    let (rpc, mut incoming, mut agent) = connect(Options::default());
    agent.say(r#"{"method":"turn/started"}"#).await;
    // The holder sends a line again only when it was a request left open.
    agent
        .frames
        .send(Output::Line {
            sequence: 1,
            line: r#"{"method":"turn/started"}"#.into(),
            again: false,
        })
        .await
        .unwrap();
    agent
        .frames
        .send(Output::Line {
            sequence: 1,
            line: r#"{"id":40,"method":"item/approve"}"#.into(),
            again: true,
        })
        .await
        .unwrap();
    assert!(matches!(
        next(&mut incoming).await,
        Incoming::Notification { .. }
    ));
    assert!(matches!(
        next(&mut incoming).await,
        Incoming::Request { .. }
    ));

    let asking = tokio::spawn({
        let rpc = rpc.clone();
        async move {
            rpc.request("slow", None, Some(Duration::from_millis(50)))
                .await
        }
    });
    let id = agent.line().await["id"].clone();
    assert!(matches!(asking.await.unwrap(), Err(Failure::TookTooLong(method)) if method == "slow"));
    agent
        .say(&json!({"id": id, "result": true}).to_string())
        .await;
    agent.say(r#"{"id":3,"error":{"message":"old"}}"#).await;
    assert_eq!(
        next(&mut incoming).await,
        Incoming::Stray {
            id: json!(1),
            answer: Ok(json!(true))
        }
    );
    assert_eq!(
        next(&mut incoming).await,
        Incoming::Stray {
            id: json!(3),
            answer: Err(RpcError::new(-32000, "old"))
        }
    );
}

#[tokio::test]
async fn an_exit_ends_open_requests_with_what_the_agent_last_said() {
    let (rpc, mut incoming, mut agent) = connect(Options::default());
    let asking = tokio::spawn({
        let rpc = rpc.clone();
        async move { rpc.request("turn/start", None, None).await }
    });
    agent.line().await;
    agent.say(r#"{"method":"turn/started"}"#).await;
    for text in ["one\n", "two\n", "three\n", "four\n"] {
        agent
            .frames
            .send(Output::Stderr { text: text.into() })
            .await
            .unwrap();
    }
    agent
        .frames
        .send(Output::Exited {
            exit: Exit {
                code: Some(3),
                signal: None,
            },
        })
        .await
        .unwrap();
    assert!(
        matches!(asking.await.unwrap(), Err(Failure::Closed(why)) if why == "two\nthree\nfour")
    );
    assert!(matches!(
        next(&mut incoming).await,
        Incoming::Notification { .. }
    ));
    assert_eq!(
        next(&mut incoming).await,
        Incoming::Exited {
            code: 3,
            signal: None
        }
    );
    // Everything it read is acknowledged before it lets go.
    assert_eq!(agent.written().await, Write::Ack(1));
    assert_eq!(agent.written().await, Write::Bye);
    assert_eq!(rpc.stderr(), "one\ntwo\nthree\nfour\n");
    assert!(matches!(
        rpc.request("again", None, None).await,
        Err(Failure::Closed(_))
    ));

    let (rpc, _incoming, mut agent) = connect(Options::default());
    let asking = tokio::spawn(async move { rpc.request("x", None, None).await });
    agent.line().await;
    agent
        .frames
        .send(Output::Exited {
            exit: Exit {
                code: Some(9),
                signal: None,
            },
        })
        .await
        .unwrap();
    assert!(matches!(asking.await.unwrap(),
        Err(Failure::Closed(why)) if why == "The agent stopped (exit code 9)."));
}

#[tokio::test]
async fn a_lost_holder_ends_the_agent_but_a_fenced_one_is_handed_over() {
    let (rpc, mut incoming, agent) = connect(Options::default());
    let asking = tokio::spawn({
        let rpc = rpc.clone();
        async move { rpc.request("x", None, None).await }
    });
    let Agent {
        frames, mut writes, ..
    } = agent;
    assert!(matches!(writes.recv().await, Some(Write::Line(_))));
    drop(frames);
    assert!(matches!(asking.await.unwrap(), Err(Failure::Closed(_))));
    assert_eq!(
        next(&mut incoming).await,
        Incoming::Exited {
            code: -1,
            signal: Some("lost".into())
        }
    );

    let (rpc, mut incoming, agent) = connect(Options::default());
    let Agent {
        frames, mut writes, ..
    } = agent;
    let asking = tokio::spawn({
        let rpc = rpc.clone();
        async move { rpc.request("x", None, None).await }
    });
    assert!(matches!(writes.recv().await, Some(Write::Line(_))));
    frames.send(Output::Fenced { fenced: true }).await.unwrap();
    drop(frames);
    assert!(matches!(asking.await.unwrap(), Err(Failure::Closed(_))));
    rpc.stop().await;
    drop(rpc);
    // Nothing more is told: the newer resident has the agent.
    assert!(incoming.recv().await.is_none());
}

#[tokio::test]
async fn lines_taken_in_are_acknowledged_together() {
    let (_rpc, mut incoming, mut agent) = connect(Options::default());
    agent.say(r#"{"method":"a"}"#).await;
    agent.say(r#"{"method":"b"}"#).await;
    next(&mut incoming).await;
    next(&mut incoming).await;
    assert_eq!(agent.written().await, Write::Ack(2));
}
