//! The line of messages waiting for a conversation's agent: when each goes,
//! into the turn under way or as a turn of its own.

use super::{Cancelled, Command, Doing, Live, Reply, Service, State};
use crate::{
    inbox::{Delivery, InboxEvent, Input, SavedReceipt, next_in_line},
    session::{Steer, TurnKind},
    turn_projection::user,
    turn_runtime::RunningTurn,
    turn_setup::turn_setup,
};
use serde_json::Value;
use std::{
    io::{self, ErrorKind},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

/// How often uncertain handoffs are looked at again.
const CHECK_STEERS: Duration = Duration::from_secs(1);

pub(super) enum Request {
    Prompt {
        session: String,
        input: Box<Input>,
        handoff: Option<String>,
        reply: Reply<()>,
    },
    SetDelivery {
        session: String,
        id: String,
        delivery: Delivery,
        into: bool,
        reply: Reply<()>,
    },
    Cancel {
        session: String,
        id: String,
        defer: bool,
        reply: Reply<Cancelled>,
    },
    Confirm {
        session: String,
        id: String,
        reply: Reply<bool>,
    },
    CancelHandoff {
        session: String,
        id: String,
        token: String,
        reply: Reply<bool>,
    },
    Interrupt {
        session: String,
        resume: bool,
        reply: Reply<Stopping>,
    },
}

pub(super) enum Stopping {
    Turn(RunningTurn),
    /// Its agent was getting ready: the turn stops as soon as it can.
    Starting,
    Nothing,
}

pub(super) struct Steered {
    session: String,
    input: Input,
    provider: String,
    /// The receipt saved before the handoff, if the agent gave one.
    receipt: Option<SavedReceipt>,
    answer: Steer,
    cancelled: Arc<AtomicBool>,
}

fn missing(session: &str) -> io::Error {
    io::Error::new(ErrorKind::NotFound, format!("No session {session}."))
}

impl Service {
    pub(super) async fn line_request(&self, state: &mut State, request: Request) {
        match request {
            Request::Prompt {
                session,
                input,
                handoff,
                reply,
            } => {
                let answer = self.enqueue(state, &session, *input, handoff).await;
                let _ = reply.send(answer);
            }
            Request::SetDelivery {
                session,
                id,
                delivery,
                into,
                reply,
            } => {
                let answer = self
                    .set_delivery(state, &session, &id, delivery, into)
                    .await;
                let _ = reply.send(answer);
            }
            Request::Cancel {
                session,
                id,
                defer,
                reply,
            } => {
                let answer = self.cancel(state, &session, &id, defer).await;
                let _ = reply.send(answer);
            }
            Request::Confirm { session, id, reply } => {
                let _ = reply.send(self.confirm(&session, &id).await);
            }
            Request::CancelHandoff {
                session,
                id,
                token,
                reply,
            } => {
                let _ = reply.send(self.cancel_handoff(&session, id, token).await);
            }
            Request::Interrupt {
                session,
                resume,
                reply,
            } => {
                let answer = self.interrupt(state, &session, resume).await;
                let _ = reply.send(Ok(answer));
            }
        }
    }

    /// A message (yours, or a note from the app) in line: into the turn
    /// under way, now, or after it.
    async fn enqueue(
        &self,
        state: &mut State,
        session: &str,
        mut input: Input,
        handoff: Option<String>,
    ) -> io::Result<()> {
        if state.conversation(session).retired {
            return Err(missing(session));
        }
        let id = input.id.clone();
        let line = self
            .store
            .line(session)
            .await?
            .ok_or_else(|| missing(session))?;
        // Sent twice (a retry after a timeout): once is enough.
        let starting = match &state.conversation(session).doing {
            Doing::Starting { id: starting, .. } => starting == &id,
            _ => false,
        };
        if starting
            || line.iter().any(|waiting| waiting.id == id)
            || self.store.has_message(session, &id).await?
        {
            return Ok(());
        }
        let conversation = state.conversation(session);
        let into = match &conversation.doing {
            Doing::Running(live)
                if input.delivery != Delivery::Queue && live.thread.takes_steers() =>
            {
                Some(live.clone())
            }
            _ => None,
        };
        let nothing_runs = matches!(conversation.doing, Doing::Idle);
        input.delivery = if into.is_some()
            || (nothing_runs && (input.delivery != Delivery::Queue || line.is_empty()))
        {
            Delivery::Steer
        } else {
            Delivery::Queue
        };
        let token = handoff;
        let saved = input.clone();
        let accepted = self
            .store
            .edit_line(session, move |line| {
                if let Some(token) = &token
                    && line.cancelled_handoffs.get(token).and_then(Value::as_str)
                        == Some(saved.id.as_str())
                {
                    return (false, false);
                }
                line.inputs.push(saved);
                (true, true)
            })
            .await?
            .ok_or_else(|| missing(session))?;
        if !accepted {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "That handoff was cancelled.",
            ));
        }
        let delivery = input.delivery;
        let _ = self.events.send(InboxEvent::Enqueued {
            session: session.to_owned(),
            input: Box::new(input.clone()),
        });
        if let Some(live) = into {
            self.steer_now(state, session, live, input);
        } else if delivery == Delivery::Steer {
            self.wake(state, session).await;
        }
        Ok(())
    }

    /// Hand a message to the turn under way, at its next step. One the agent
    /// takes is sent even if its turn ends then or you cancel it meanwhile.
    fn steer_now(&self, state: &mut State, session: &str, live: Live, input: Input) {
        let cancelled = Arc::new(AtomicBool::new(false));
        state
            .conversation(session)
            .handing
            .insert(input.id.clone(), cancelled.clone());
        let store = self.store.clone();
        let commands = self.commands.clone();
        let session = session.to_owned();
        tokio::spawn(async move {
            let prompt = input.prompt();
            let mut receipt = None;
            let mut handed = !cancelled.load(Ordering::SeqCst);
            if handed && let Some(native) = live.turn.steer_receipt(&prompt).await {
                let saved = SavedReceipt {
                    provider: live.provider.clone(),
                    receipt: native.into(),
                    order: None,
                };
                let id = input.id.clone();
                let keep = saved.clone();
                let flag = cancelled.clone();
                // Saved before it's sent: a crash between the two is settled
                // by the receipt, never by sending it again.
                handed = store
                    .edit_line(&session, move |line| match line.find_mut(&id) {
                        Some(waiting) if !flag.load(Ordering::SeqCst) => {
                            waiting.steer_receipt = Some(keep);
                            (true, true)
                        }
                        _ => (false, false),
                    })
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or(false);
                receipt = handed.then_some(saved);
            }
            let answer = if handed {
                live.turn.steer(prompt).await.unwrap_or(Steer::Declined)
            } else {
                Steer::Declined
            };
            if let Some(commands) = commands.upgrade() {
                let _ = commands
                    .send(Command::Steered(Box::new(Steered {
                        session,
                        input,
                        provider: live.provider,
                        receipt,
                        answer,
                        cancelled,
                    })))
                    .await;
            }
        });
    }

    pub(super) async fn steered(&self, state: &mut State, steered: Steered) {
        let Steered {
            session,
            input,
            provider,
            receipt,
            answer,
            cancelled,
        } = steered;
        let conversation = state.conversation(&session);
        conversation.handing.remove(&input.id);
        if conversation.retired {
            return;
        }
        let id = input.id.clone();
        let Ok(Some(line)) = self.store.line(&session).await else {
            return;
        };
        // Gone: the conversation is being deleted.
        if !line.iter().any(|waiting| waiting.id == id) {
            return;
        }
        let taken = match answer {
            Steer::Accepted => true,
            Steer::Declined => false,
            Steer::InDoubt(native) => {
                if receipt.is_none() {
                    let saved = SavedReceipt {
                        provider,
                        receipt: native.into(),
                        order: None,
                    };
                    let id = id.clone();
                    let _ = self
                        .store
                        .edit_line(&session, move |line| match line.find_mut(&id) {
                            Some(waiting) => {
                                waiting.steer_receipt = Some(saved);
                                ((), true)
                            }
                            None => ((), false),
                        })
                        .await;
                }
                self.check_steers_soon(state, &session);
                return;
            }
        };
        if !taken && receipt.is_some() {
            let id = id.clone();
            let cleared = self
                .store
                .edit_line(&session, move |line| match line.find_mut(&id) {
                    Some(waiting) => {
                        waiting.steer_receipt = None;
                        ((), true)
                    }
                    None => ((), false),
                })
                .await;
            if cleared.is_err() {
                self.check_steers_soon(state, &session);
                return;
            }
        }
        let cancelled = cancelled.load(Ordering::SeqCst);
        let succeeded = self.last_succeeded(&session).await;
        let conversation = state.conversation(&session);
        // Its turn ended meanwhile, or is ending: it goes next as you asked
        // (Send Now), or as after a turn that went through; otherwise it
        // waits, held (Stop, an error).
        let next = !taken
            && !cancelled
            && !matches!(
                conversation.doing,
                Doing::Running(_) | Doing::Starting { .. }
            )
            && (conversation.go
                || (!matches!(conversation.doing, Doing::Stopping(_)) && succeeded));
        if matches!(conversation.doing, Doing::Idle) && conversation.handing.is_empty() {
            conversation.go = next;
        }
        let continue_after_exit = conversation.continue_after_exit;
        if !taken {
            if cancelled {
                let _ = self.cancel(state, &session, &id, true).await;
            } else if next || continue_after_exit {
                self.advance(state, &session, None).await;
            } else {
                let _ = self
                    .set_delivery(state, &session, &id, Delivery::Queue, false)
                    .await;
            }
            return;
        }
        // In the conversation first: a crash before it leaves the line can't
        // send it again (what's in the transcript doesn't go).
        self.record(&session, &input).await;
        let noted = receipt.map(|saved| saved.provider);
        let taken_id = id.clone();
        let _ = self
            .store
            .edit_line(&session, move |line| {
                let removed = line.take(&taken_id).is_some();
                if removed && let Some(provider) = &noted {
                    line.note_seen_steer(provider, &taken_id);
                }
                (removed, removed)
            })
            .await;
        if continue_after_exit {
            self.advance(state, &session, None).await;
        }
    }

    /// The row clients see as soon as the message leaves the line.
    async fn record(&self, session: &str, input: &Input) {
        let mut row = user(&input.prompt_as_sent());
        if let Some(metadata) = &input.metadata {
            row["metadata"] = Value::Object(metadata.clone());
        }
        if let Some(synthetic) = &input.synthetic {
            if let Some(description) = &synthetic.description {
                row["description"] = Value::String(description.clone());
            }
            if let Some(metadata) = &synthetic.metadata {
                row["metadata"] = Value::Object(metadata.clone());
            }
        }
        if matches!(
            self.store.write_messages(session, vec![row.clone()]).await,
            Ok(true)
        ) {
            let _ = self.events.send(InboxEvent::Delivered {
                session: session.to_owned(),
                id: input.id.clone(),
                row,
            });
        }
    }

    fn check_steers_soon(&self, state: &mut State, session: &str) {
        let conversation = state.conversation(session);
        if conversation.checking {
            return;
        }
        conversation.checking = true;
        let service = self.clone();
        let session = session.to_owned();
        tokio::spawn(async move {
            tokio::time::sleep(CHECK_STEERS).await;
            // (A resident that has stopped leaves it to the next.)
            let Some(commands) = service.commands.upgrade() else {
                return;
            };
            let (retry, remaining) = service.reconcile_steers(&session).await;
            let _ = commands
                .send(Command::Reconciled {
                    session,
                    retry,
                    remaining,
                })
                .await;
        });
    }

    /// Settle uncertain handoffs from the agent's own conversation, never by
    /// sending them again. Answers whether one is to go again, and whether
    /// any is still uncertain.
    async fn reconcile_steers(&self, session: &str) -> (bool, bool) {
        let Ok(Some(line)) = self.store.line(session).await else {
            return (false, false);
        };
        let mut retry = false;
        for input in line
            .into_iter()
            .filter(|input| input.steer_receipt.is_some())
        {
            let Some(saved) = input.steer_receipt.clone() else {
                continue;
            };
            let recorded = self
                .store
                .has_message(session, &input.id)
                .await
                .unwrap_or(false);
            let delivered = if recorded {
                Some(true)
            } else {
                match self.thread(session, &saved.provider).await {
                    Ok(thread) => thread
                        .resolve_steer(saved.receipt.clone().into())
                        .await
                        .ok()
                        .flatten(),
                    Err(_) => None,
                }
            };
            let Some(delivered) = delivered else { continue };
            if delivered && !recorded {
                self.record(session, &input).await;
            }
            let id = input.id.clone();
            let provider = saved.provider.clone();
            let changed = self
                .store
                .edit_line(session, move |line| {
                    let Some(waiting) = line.find_mut(&id) else {
                        return (false, false);
                    };
                    if delivered {
                        line.take(&id);
                        line.note_seen_steer(&provider, &id);
                    } else {
                        waiting.steer_receipt = None;
                    }
                    (true, true)
                })
                .await;
            retry |= !delivered && matches!(changed, Ok(Some(true)));
        }
        let remaining = self
            .store
            .line(session)
            .await
            .ok()
            .flatten()
            .is_some_and(|line| line.iter().any(|input| input.steer_receipt.is_some()));
        (retry, remaining)
    }

    pub(super) async fn reconciled(
        &self,
        state: &mut State,
        session: &str,
        retry: bool,
        remaining: bool,
    ) {
        let conversation = state.conversation(session);
        conversation.checking = false;
        if conversation.retired {
            return;
        }
        if remaining {
            self.check_steers_soon(state, session);
        }
        let succeeded = self.last_succeeded(session).await;
        let conversation = state.conversation(session);
        if matches!(conversation.doing, Doing::Idle)
            && (retry || conversation.go || conversation.continue_after_exit)
        {
            let ended = retry.then_some(succeeded);
            self.advance(state, session, ended).await;
        }
    }

    /// The conversation's last turn went through on its own, as its saved
    /// header says.
    async fn last_succeeded(&self, session: &str) -> bool {
        matches!(
            self.store.read(session).await,
            Ok(Some(header)) if header.outcome() == Some("succeeded")
        )
    }

    async fn set_delivery(
        &self,
        state: &mut State,
        session: &str,
        id: &str,
        delivery: Delivery,
        into: bool,
    ) -> io::Result<()> {
        let wanted = id.to_owned();
        let changed = self
            .store
            .edit_line(session, move |line| match line.find_mut(&wanted) {
                None => (Err(ErrorKind::NotFound), false),
                Some(input) if input.steer_receipt.is_some() => {
                    (Err(ErrorKind::InvalidInput), false)
                }
                Some(input) if input.delivery == delivery => (Ok((false, input.clone())), false),
                Some(input) => {
                    input.delivery = delivery;
                    (Ok((true, input.clone())), true)
                }
            })
            .await?
            .ok_or_else(|| missing(session))?;
        let (changed, input) = changed.map_err(|kind| match kind {
            ErrorKind::NotFound => io::Error::new(kind, "That message was sent already."),
            _ => io::Error::new(kind, "Kuvo is checking whether Codex took this message."),
        })?;
        if changed {
            let _ = self.events.send(InboxEvent::DeliveryChanged {
                session: session.to_owned(),
                id: id.to_owned(),
                delivery,
            });
        }
        if delivery != Delivery::Steer {
            return Ok(());
        }
        let conversation = state.conversation(session);
        if into
            && let Doing::Running(live) = &conversation.doing
            && live.thread.takes_steers()
            && !conversation.handing.contains_key(id)
        {
            let live = live.clone();
            self.steer_now(state, session, live, input);
            return Ok(());
        }
        self.wake(state, session).await;
        Ok(())
    }

    pub(super) async fn cancel(
        &self,
        state: &mut State,
        session: &str,
        id: &str,
        defer: bool,
    ) -> io::Result<Cancelled> {
        // Being handed to the agent: cancelled if it doesn't take it.
        if let Some(handing) = state.conversation(session).handing.get(id) {
            if defer {
                handing.store(true, Ordering::SeqCst);
            }
            return Ok(Cancelled::Busy);
        }
        let wanted = id.to_owned();
        let outcome = self
            .store
            .edit_line(session, move |line| match line.find(&wanted) {
                None => (Cancelled::Missing, false),
                Some(input) if input.steer_receipt.is_some() => (Cancelled::Busy, false),
                Some(_) => {
                    line.take(&wanted);
                    (Cancelled::Removed, true)
                }
            })
            .await?
            .unwrap_or(Cancelled::Missing);
        if outcome == Cancelled::Removed {
            let _ = self.events.send(InboxEvent::Cancelled {
                session: session.to_owned(),
                id: id.to_owned(),
            });
            if state.conversation(session).continue_after_exit {
                self.advance(state, session, None).await;
            }
        }
        Ok(outcome)
    }

    async fn confirm(&self, session: &str, id: &str) -> io::Result<bool> {
        let wanted = id.to_owned();
        let waiting = self
            .store
            .edit_line(session, move |line| {
                let waiting = line.find(&wanted).is_some();
                // Written again (synced) before the box lets go of its copy.
                (waiting, waiting)
            })
            .await?
            .ok_or_else(|| missing(session))?;
        Ok(waiting || self.store.has_message(session, id).await?)
    }

    async fn cancel_handoff(&self, session: &str, id: String, token: String) -> io::Result<bool> {
        if self.store.has_message(session, &id).await? {
            return Ok(false);
        }
        self.store
            .edit_line(session, move |line| {
                if line.find(&id).is_some() {
                    return (false, false);
                }
                line.cancelled_handoffs.insert(token, Value::String(id));
                (true, true)
            })
            .await?
            .ok_or_else(|| missing(session))
    }

    async fn interrupt(&self, state: &mut State, session: &str, resume: bool) -> Stopping {
        let conversation = state.conversation(session);
        match std::mem::take(&mut conversation.doing) {
            Doing::Running(live) | Doing::Stopping(live) => {
                let turn = live.turn.clone();
                conversation.doing = Doing::Stopping(live);
                conversation.go = resume;
                Stopping::Turn(turn)
            }
            Doing::Starting { id, .. } => {
                conversation.doing = Doing::Starting { stop: true, id };
                conversation.go = resume;
                Stopping::Starting
            }
            Doing::Idle => {
                if resume {
                    self.wake(state, session).await;
                } else {
                    conversation.continue_after_exit = false;
                }
                Stopping::Nothing
            }
        }
    }

    /// Starts the first message you steered, now if nothing runs. (A turn
    /// under way decides at its end.)
    async fn wake(&self, state: &mut State, session: &str) {
        let conversation = state.conversation(session);
        if !matches!(conversation.doing, Doing::Idle) {
            return;
        }
        conversation.go = true;
        self.advance(state, session, None).await;
    }

    /// What starts next when nothing runs: after a turn that ended on its
    /// own, the next message in line, steered ones first; otherwise the first
    /// you steered, if you asked (`go`). After a Stop or an error, nothing
    /// more starts until you say. `ended`: whether the turn that just ended
    /// went through.
    pub(super) async fn advance(&self, state: &mut State, session: &str, ended: Option<bool>) {
        let conversation = state.conversation(session);
        if !matches!(conversation.doing, Doing::Idle) || conversation.retired {
            return;
        }
        let Ok(Some(line)) = self.store.line(session).await else {
            return;
        };
        let conversation = state.conversation(session);
        let handing = |id: &str| conversation.handing.contains_key(id);
        let index = if conversation.continue_after_exit {
            // An uncertain handoff is settled before the next in line goes.
            if line
                .first()
                .is_some_and(|first| first.steer_receipt.is_some() || handing(&first.id))
            {
                return;
            }
            conversation.continue_after_exit = false;
            (!line.is_empty()).then_some(0)
        } else {
            next_in_line(&line, ended == Some(true), conversation.go, handing)
        };
        // (With nothing to start but one being handed, Send Now waits for it.)
        if index.is_some() || conversation.handing.is_empty() {
            conversation.go = false;
        }
        let Some(index) = index else { return };
        let id = line[index].id.clone();
        let sent = self.store.has_message(session, &id).await.unwrap_or(false);
        let taken = self
            .store
            .edit_line(session, move |line| {
                let input = line.take(&id);
                let changed = input.is_some();
                (input, changed)
            })
            .await;
        let Ok(Some(Some(input))) = taken else {
            return;
        };
        if sent {
            // Already in the conversation (it left the line as a crash cut
            // in): it isn't sent again, and the next goes.
            Box::pin(self.advance(state, session, ended)).await;
            return;
        }
        self.start_from_line(state, session, input).await;
    }

    async fn start_from_line(&self, state: &mut State, session: &str, input: Input) {
        let header = match self.store.read(session).await {
            Ok(Some(header)) => header,
            _ => return,
        };
        let Some(folder) = header.directory().map(PathBuf::from) else {
            return;
        };
        let note = input.metadata.clone();
        let setup = turn_setup(
            &self.settings.home,
            folder,
            &header.into_fields(),
            &input.id,
            TurnKind::Prompt(input.prompt()),
        );
        let (turn, policy) = match setup {
            Ok(setup) => setup,
            Err(error) => {
                eprintln!("Kuvo could not start coding-agent session {session}: {error}");
                return;
            }
        };
        let _ = self.events.send(InboxEvent::Delivered {
            session: session.to_owned(),
            id: input.id.clone(),
            row: user(&input.prompt_as_sent()),
        });
        state.conversation(session).doing = Doing::Starting {
            stop: false,
            id: input.id.clone(),
        };
        let shown = input.prompt_as_sent();
        self.spawn_start(
            state,
            session.to_owned(),
            turn,
            policy,
            note,
            Some(shown),
            None,
        );
    }

    /// As the resident starts: uncertain handoffs are looked at, and what an
    /// earlier resident held for this one goes, each once.
    pub(super) async fn wake_saved(&self, state: &mut State) {
        let Ok(headers) = self.store.headers().await else {
            return;
        };
        for header in headers {
            let session = header.id().to_owned();
            let Ok(Some(line)) = self.store.line(&session).await else {
                continue;
            };
            if line.iter().any(|input| input.steer_receipt.is_some()) {
                self.check_steers_soon(state, &session);
            }
            if !line.iter().any(|input| input.held) {
                continue;
            }
            let released = self
                .store
                .edit_line(&session, |line| {
                    for input in &mut line.inputs {
                        input.held = false;
                    }
                    ((), true)
                })
                .await;
            if released.is_ok() {
                self.wake(state, &session).await;
            }
        }
    }
}
