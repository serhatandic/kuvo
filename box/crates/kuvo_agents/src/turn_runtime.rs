//! Drive a thread's startup, transcript writes and controls independently.

use crate::{
    runtime::AdapterCall,
    session::{
        ActiveTurn, EventEnvelope, Outcome, Prompt, Steer, SteerReceipt, Thread, TurnEvent,
        TurnInput,
    },
    turn_sink::{EventWrite, ResidentSink},
};
use std::{
    io::{self, ErrorKind},
    sync::Arc,
};
use tokio::sync::{mpsc, watch};

pub trait TurnWriter: Send {
    fn apply<'a>(&'a mut self, event: &'a EventEnvelope) -> AdapterCall<'a, ()>;
}

#[derive(Clone)]
enum State {
    Starting,
    Active(Arc<dyn ActiveTurn>),
    /// How the turn ended, as its agent said; none when it stopped without
    /// saying.
    Done(Result<Option<Outcome>, Arc<io::Error>>),
}

#[derive(Clone)]
pub struct RunningTurn {
    state: watch::Receiver<State>,
}

impl RunningTurn {
    pub fn launch(
        thread: Arc<dyn Thread>,
        input: TurnInput,
        sink: Arc<ResidentSink>,
        writes: mpsc::Receiver<EventWrite>,
        writer: impl TurnWriter + 'static,
    ) -> Self {
        let (publish, state) = watch::channel(State::Starting);
        tokio::spawn(run(thread, input, sink, writes, writer, publish));
        Self { state }
    }

    async fn active(&self) -> io::Result<Arc<dyn ActiveTurn>> {
        let mut state = self.state.clone();
        let latest = state
            .wait_for(|state| !matches!(state, State::Starting))
            .await;
        match latest.as_deref() {
            Ok(State::Active(turn)) => Ok(turn.clone()),
            Ok(State::Done(Err(error))) => Err(io::Error::new(error.kind(), error.to_string())),
            _ => Err(io::Error::new(ErrorKind::NotFound, "That turn has ended.")),
        }
    }

    pub async fn ready(&self) -> io::Result<()> {
        let mut state = self.state.clone();
        let latest = state
            .wait_for(|state| !matches!(state, State::Starting))
            .await;
        match latest.as_deref() {
            Ok(State::Active(_) | State::Done(Ok(_))) => Ok(()),
            Ok(State::Done(Err(error))) => Err(io::Error::new(error.kind(), error.to_string())),
            _ => Err(io::Error::new(
                ErrorKind::BrokenPipe,
                "The agent stopped while starting.",
            )),
        }
    }

    pub async fn steer(&self, prompt: Prompt) -> io::Result<Steer> {
        self.active().await?.steer(prompt).await
    }

    pub async fn steer_receipt(&self, prompt: &Prompt) -> Option<SteerReceipt> {
        self.active().await.ok()?.steer_receipt(prompt)
    }

    pub async fn interrupt(&self) -> io::Result<()> {
        self.active().await?.interrupt().await
    }

    pub async fn finished(&self) -> io::Result<Option<Outcome>> {
        let mut state = self.state.clone();
        let latest = state
            .wait_for(|state| matches!(state, State::Done(_)))
            .await;
        match latest.as_deref() {
            Ok(State::Done(Ok(outcome))) => Ok(outcome.clone()),
            Ok(State::Done(Err(error))) => Err(io::Error::new(error.kind(), error.to_string())),
            _ => Err(io::Error::new(
                ErrorKind::BrokenPipe,
                "The turn stopped without a result.",
            )),
        }
    }
}

async fn run(
    thread: Arc<dyn Thread>,
    input: TurnInput,
    sink: Arc<ResidentSink>,
    mut writes: mpsc::Receiver<EventWrite>,
    mut writer: impl TurnWriter,
    publish: watch::Sender<State>,
) {
    let retirement = sink.retirement();
    let start = thread.start(input, sink);
    tokio::pin!(start);
    let mut started = false;
    let outcome = loop {
        tokio::select! {
            reply = &mut start, if !started => {
                match reply {
                    Ok(turn) => {
                        started = true;
                        publish.send_replace(State::Active(turn));
                    }
                    Err(error) => break Err(Arc::new(error)),
                }
            }
            update = writes.recv() => {
                let Some(update) = update else {
                    break Err(Arc::new(io::Error::new(
                        ErrorKind::BrokenPipe,
                        "The adapter stopped reporting its turn.",
                    )));
                };
                let last = match &update.event.event {
                    TurnEvent::Finished(result) => Some(result.outcome.clone()),
                    _ => None,
                };
                let result = writer.apply(&update.event).await;
                let error = result.as_ref().err()
                    .map(|error| Arc::new(io::Error::new(error.kind(), error.to_string())));
                update.finish(result);
                if let Some(error) = error {
                    break Err(error);
                }
                if let Some(outcome) = last {
                    break Ok(Some(outcome));
                }
            }
        }
    };
    // Stop accepting writes before finishing client-facing controls and asks.
    writes.close();
    let _ = retirement.end().await;
    publish.send_replace(State::Done(outcome));
}
