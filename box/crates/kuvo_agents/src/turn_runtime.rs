//! Drive a thread's startup, transcript writes and controls independently.

use crate::{
    foreground::Foreground,
    inbox::InboxEvent,
    runtime::AdapterCall,
    session::{
        ActiveTurn, EventEnvelope, Outcome, Prompt, Steer, SteerReceipt, TextKind, Thread,
        TurnEvent, TurnInput,
    },
    turn_sink::{EventWrite, ResidentSink},
};
use std::{
    io::{self, ErrorKind},
    sync::Arc,
};
use tokio::sync::{broadcast, mpsc, watch};
use tokio_util::sync::CancellationToken;

/// What a turn is doing now, told on its line in the engine's words as it changes (it reasons,
/// writes words, calls a tool): what shows a conversation thinking.
pub(crate) struct Moments {
    session: String,
    events: broadcast::Sender<InboxEvent>,
    last: Option<&'static str>,
}

impl Moments {
    pub(crate) fn new(session: &str, events: broadcast::Sender<InboxEvent>) -> Self {
        Self {
            session: session.to_owned(),
            events,
            last: None,
        }
    }

    fn heard(&mut self, event: &TurnEvent) {
        let kind = match event {
            TurnEvent::TextDelta {
                kind: TextKind::Reasoning,
                ..
            } => "session.reasoning.started",
            TurnEvent::TextDelta {
                kind: TextKind::Text,
                ..
            } => "session.text.started",
            TurnEvent::Tool { .. } => "session.tool.called",
            _ => return,
        };
        if self.last.replace(kind) != Some(kind) {
            let _ = self.events.send(InboxEvent::Moment {
                session: self.session.clone(),
                kind,
            });
        }
    }
}

pub trait TurnWriter: Send {
    fn apply<'a>(&'a mut self, event: &'a EventEnvelope) -> AdapterCall<'a, ()>;

    /// The turn ended without its agent saying how: it couldn't start, or
    /// stopped reporting. Write it as failed with `error`.
    fn abandon<'a>(&'a mut self, _error: &'a io::Error) -> AdapterCall<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    /// The turn was stopped before its agent started it, and couldn't
    /// start: write it as stopped.
    fn stopped(&mut self) -> AdapterCall<'_, ()> {
        Box::pin(async { Ok(()) })
    }
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
    /// Stop was asked for, maybe before the agent started the turn.
    stop: CancellationToken,
}

impl RunningTurn {
    pub(crate) fn launch<F, Fut>(
        thread: Arc<dyn Thread>,
        input: TurnInput,
        sink: Arc<ResidentSink>,
        writes: mpsc::Receiver<EventWrite>,
        writer: impl TurnWriter + 'static,
        foreground: Foreground,
        moments: Moments,
        boundary: F,
    ) -> Self
    where
        F: Fn() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let (publish, state) = watch::channel(State::Starting);
        let stop = input.start_cancel.clone();
        tokio::spawn(run(
            thread,
            input,
            sink,
            writes,
            writer,
            foreground,
            moments,
            publish,
            stop.clone(),
            boundary,
        ));
        Self { state, stop }
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

    /// Stop it. One its agent couldn't start after all, asked to stop as it
    /// started, ends stopped.
    pub async fn interrupt(&self) -> io::Result<()> {
        self.stop.cancel();
        match self.active().await {
            Ok(turn) => turn.interrupt().await,
            Err(_)
                if matches!(
                    *self.state.borrow(),
                    State::Done(Ok(Some(Outcome::Interrupted)))
                ) =>
            {
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    /// Whether the turn has ended (its end written), though the resident may
    /// not have heard yet.
    pub fn is_done(&self) -> bool {
        matches!(*self.state.borrow(), State::Done(_))
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

#[allow(clippy::too_many_arguments)]
async fn run<F, Fut>(
    thread: Arc<dyn Thread>,
    input: TurnInput,
    sink: Arc<ResidentSink>,
    mut writes: mpsc::Receiver<EventWrite>,
    mut writer: impl TurnWriter,
    mut foreground: Foreground,
    mut moments: Moments,
    publish: watch::Sender<State>,
    stop: CancellationToken,
    boundary: F,
) where
    F: Fn() -> Fut + Send,
    Fut: std::future::Future<Output = ()> + Send,
{
    let native_steer = thread.takes_steers();
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
                    Err(error) => break couldnt_start(&mut writer, error, &stop).await,
                }
            }
            update = writes.recv() => {
                let Some(update) = update else {
                    let quiet = || io::Error::new(
                        ErrorKind::BrokenPipe,
                        "The adapter stopped reporting its turn.",
                    );
                    // A turn that couldn't start lets go of its sink before it says why: why it
                    // couldn't is what it ends with.
                    let error = if started {
                        quiet()
                    } else {
                        match (&mut start).await {
                            Ok(turn) => {
                                publish.send_replace(State::Active(turn));
                                quiet()
                            }
                            Err(error) => break couldnt_start(&mut writer, error, &stop).await,
                        }
                    };
                    if let Err(unwritten) = writer.abandon(&error).await {
                        eprintln!("Kuvo could not write a turn its agent left: {unwritten}");
                    }
                    break Err(Arc::new(error));
                };
                let last = match &update.event.event {
                    TurnEvent::Finished(result) => Some(result.outcome.clone()),
                    _ => None,
                };
                let step = !native_steer && matches!(&update.event.event, TurnEvent::StepBoundary);
                foreground.heard(&update.event.event);
                moments.heard(&update.event.event);
                let result = writer.apply(&update.event).await;
                let error = result.as_ref().err()
                    .map(|error| Arc::new(io::Error::new(error.kind(), error.to_string())));
                update.finish(result);
                if let Some(error) = error {
                    break Err(error);
                }
                if step { boundary().await; }
                if let Some(outcome) = last {
                    break Ok(Some(outcome));
                }
            }
        }
    };
    // Stop accepting writes before finishing client-facing controls and asks.
    writes.close();
    foreground.end_all();
    let _ = retirement.end().await;
    publish.send_replace(State::Done(outcome));
}

/// A turn its agent couldn't start ends failed with why, or stopped if you
/// stopped it meanwhile.
async fn couldnt_start(
    writer: &mut impl TurnWriter,
    error: io::Error,
    stop: &CancellationToken,
) -> Result<Option<Outcome>, Arc<io::Error>> {
    if stop.is_cancelled() {
        if let Err(unwritten) = writer.stopped().await {
            eprintln!("Kuvo could not write a turn stopped as it started: {unwritten}");
        }
        return Ok(Some(Outcome::Interrupted));
    }
    if let Err(unwritten) = writer.abandon(&error).await {
        eprintln!("Kuvo could not write a turn that failed to start: {unwritten}");
    }
    Err(Arc::new(error))
}
