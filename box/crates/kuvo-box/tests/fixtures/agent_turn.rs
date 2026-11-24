//! A labelled stand-in for the shared resident turn contract.

use kuvo_agents::{
    runtime::AdapterCall,
    session::{
        ActiveTurn, Decision, Delivery, EventEnvelope, Outcome, PermissionAsk, Prompt, Steer,
        TextKind, Thread, TurnEvent, TurnInput, TurnKind, TurnResult, TurnSink,
    },
};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

pub(super) struct FixtureThread;

enum Control {
    Steer(Prompt, oneshot::Sender<()>),
    Interrupt,
}

struct Controls(mpsc::Sender<Control>);

impl Thread for FixtureThread {
    fn start(
        &self,
        input: TurnInput,
        sink: Arc<dyn TurnSink>,
    ) -> AdapterCall<'_, Arc<dyn ActiveTurn>> {
        Box::pin(async move {
            let (control, mut incoming) = mpsc::channel(4);
            tokio::spawn(async move {
                let result: std::io::Result<()> = async {
                    let (text, wait, approval) = match input.kind {
                        TurnKind::Prompt(prompt) => {
                            let wait = prompt.text == "wait for interrupt";
                            let approval = prompt.text == "wait for approval";
                            (format!("Fixture received {}", prompt.text), wait, approval)
                        }
                        _ => ("Fixture resumed".into(), false, false),
                    };
                    event(
                        &sink,
                        TurnEvent::StepStarted {
                            model: Some(input.model),
                        },
                    )
                    .await?;
                    let state =
                        serde_json::from_value(serde_json::json!({ "nativeID":"fixture-native" }))?;
                    sink.save_state(state).await?;
                    event(
                        &sink,
                        TurnEvent::TextDelta {
                            kind: TextKind::Text,
                            block: None,
                            delta: text,
                        },
                    )
                    .await?;
                    if approval {
                        let decision = sink
                            .permission(PermissionAsk {
                                id: "fixture-edit".into(),
                                tool_call_id: None,
                                action: "edit".into(),
                                resources: vec!["fixture.txt".into()],
                                save: Vec::new(),
                                message: None,
                                metadata: Default::default(),
                                sandboxed: None,
                                already_weighed: false,
                                unclassified: false,
                            })
                            .await?;
                        event(
                            &sink,
                            TurnEvent::TextDelta {
                                kind: TextKind::Text,
                                block: None,
                                delta: if decision == Decision::Reject {
                                    "; refused".into()
                                } else {
                                    "; approved".into()
                                },
                            },
                        )
                        .await?;
                    }
                    if wait {
                        while let Some(control) = incoming.recv().await {
                            match control {
                                Control::Steer(steer, accepted) => {
                                    let _ = accepted.send(());
                                    event(
                                        &sink,
                                        TurnEvent::TextDelta {
                                            kind: TextKind::Text,
                                            block: None,
                                            delta: format!("; steered with {}", steer.text),
                                        },
                                    )
                                    .await?;
                                }
                                Control::Interrupt => {
                                    return finish(&sink, Outcome::Interrupted).await;
                                }
                            }
                        }
                    }
                    finish(&sink, Outcome::Succeeded).await
                }
                .await;
                if let Err(error) = result {
                    eprintln!("The fixture turn could not be reported: {error}");
                }
            });
            Ok(Arc::new(Controls(control)) as Arc<dyn ActiveTurn>)
        })
    }
}

impl ActiveTurn for Controls {
    fn steer(&self, prompt: Prompt) -> AdapterCall<'_, Steer> {
        Box::pin(async move {
            let (accepted, read) = oneshot::channel();
            if self.0.send(Control::Steer(prompt, accepted)).await.is_ok() && read.await.is_ok() {
                Ok(Steer::Accepted)
            } else {
                Ok(Steer::Declined)
            }
        })
    }

    fn interrupt(&self) -> AdapterCall<'_, ()> {
        Box::pin(async {
            self.0
                .send(Control::Interrupt)
                .await
                .map_err(std::io::Error::other)
        })
    }
}

async fn event(sink: &Arc<dyn TurnSink>, event: TurnEvent) -> std::io::Result<()> {
    sink.event(EventEnvelope {
        position: None,
        delivery: Delivery::Live,
        event,
    })
    .await
}

async fn finish(sink: &Arc<dyn TurnSink>, outcome: Outcome) -> std::io::Result<()> {
    event(
        sink,
        TurnEvent::Finished(TurnResult {
            outcome,
            took_prompt: true,
            missed: Vec::new(),
        }),
    )
    .await
}
