//! Through a resident update: what the resident being replaced hands to the
//! next, and how the next takes up the turns its agents went on with.

use super::{Doing, Service, State};
use crate::{
    accounts::Provider,
    inbox::Delivery,
    live_asks::AskKey,
    permissions::Allowance,
    runtime::AdoptedTurn,
    session::TurnKind,
    turn_setup::turn_setup,
};
use kuvo_store::files;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{sync::watch, time::Instant};

/// How long a client's read of a conversation's approvals waits for the
/// ones an earlier resident showed to come again.
pub(super) const ASK_REPLAY: Duration = Duration::from_secs(10);

const FORMAT: &str = "kuvo-agents-handover/1";

/// What a resident making way for the next says of each conversation whose
/// turn goes on in its held agent: the turn, the approvals and questions it
/// was waiting on, and the approvals you gave for the rest of it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Handover {
    pub sessions: BTreeMap<String, HandedTurn>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct HandedTurn {
    pub turn: String,
    /// The agent's own IDs of what it was asking.
    #[serde(default)]
    pub asks: Vec<String>,
    #[serde(default)]
    pub always: Vec<Allowance>,
}

#[derive(Deserialize, Serialize)]
struct Saved {
    format: String,
    #[serde(flatten)]
    handover: Handover,
}

impl Handover {
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let saved = Saved {
            format: FORMAT.into(),
            handover: self.clone(),
        };
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        files::write_atomic(path, &serde_json::to_vec(&saved)?)
    }

    /// The handover an earlier resident left, read once: it's removed.
    pub fn take(path: &Path) -> io::Result<Option<Self>> {
        let Some(bytes) = files::read(path)? else {
            return Ok(None);
        };
        files::remove_file_synced(path)?;
        Ok(serde_json::from_slice::<Saved>(&bytes)
            .ok()
            .filter(|saved| saved.format == FORMAT)
            .map(|saved| saved.handover))
    }
}

/// What a resident starts with after an update.
#[derive(Default)]
pub struct Update {
    /// The turns its agents went on with, attached to their adapters.
    pub adopted: Vec<AdoptedTurn>,
    pub handover: Option<Handover>,
}

/// Asks a resumed turn is expected to show again, and until when a client's
/// read waits for them.
#[derive(Clone)]
pub(super) struct Restoring {
    asks: BTreeSet<AskKey>,
    until: Instant,
}

pub(super) type Restorings = watch::Sender<BTreeMap<String, Restoring>>;

/// What the handover says each adopted turn was asking: a client's read
/// waits for those from the start, before the turns are taken up.
pub(super) fn expected_asks(update: &Update) -> BTreeMap<String, Restoring> {
    let Some(handover) = &update.handover else {
        return BTreeMap::new();
    };
    let until = Instant::now() + ASK_REPLAY;
    update
        .adopted
        .iter()
        .filter_map(|adopted| {
            let turn = adopted.note.turn.as_ref()?;
            let session = &adopted.note.session;
            let handed = handover
                .sessions
                .get(session)
                .filter(|handed| &handed.turn == turn && !handed.asks.is_empty())?;
            let asks = handed
                .asks
                .iter()
                .map(|id| AskKey {
                    session: session.clone(),
                    turn: turn.clone(),
                    id: id.clone(),
                })
                .collect();
            Some((session.clone(), Restoring { asks, until }))
        })
        .collect()
}

impl Service {
    /// Take up the turns an earlier resident left under way in held agents:
    /// each in its conversation, as a turn of the message that began it,
    /// sending nothing new. One whose conversation is gone or has moved on
    /// is let go of. The conversations taken up.
    pub(super) async fn take_up(&self, state: &mut State, update: Update) -> BTreeSet<String> {
        let handover = update.handover.unwrap_or_default();
        let roster = self.agents.roster();
        let mut resumed = BTreeSet::new();
        for adopted in update.adopted {
            let Some(turn) = adopted.note.turn.clone() else {
                continue;
            };
            let session = adopted.note.session.clone();
            let setup = match self.resumable(&session, &turn, &adopted.provider).await {
                Some((folder, header)) if !resumed.contains(&session) => turn_setup(
                    &self.settings.home,
                    folder,
                    &header,
                    &turn,
                    TurnKind::Recover {
                        note: adopted.note.clone(),
                    },
                )
                .map_err(|error| {
                    eprintln!("Kuvo could not take up coding-agent turn {turn}: {error}");
                })
                .ok(),
                _ => None,
            };
            let Some((mut input, policy)) = setup else {
                stopped_restoring(&self.restoring, &session);
                if let Some(entry) = roster.find(&adopted.provider) {
                    let _ = entry.agent.let_go(&adopted.note).await;
                }
                continue;
            };
            input.model.provider = adopted.provider.to_string();
            if let Some(handed) = handover
                .sessions
                .get(&session)
                .filter(|handed| handed.turn == turn)
            {
                state
                    .approvals
                    .insert(session.clone(), watch::channel(handed.always.clone()).0);
            }
            let conversation = state.conversation(&session);
            conversation.doing = Doing::Starting {
                stop: false,
                id: turn.clone(),
            };
            conversation.recovered = Some(adopted);
            self.spawn_start(state, session.clone(), input, policy, None, None, None);
            resumed.insert(session);
        }
        if !self.restoring.borrow().is_empty() {
            tokio::spawn(follow_replays(
                self.restoring.clone(),
                self.asks.pending(),
            ));
        }
        resumed
    }

    /// Whether `turn` can go on in `session`: it's the conversation's last,
    /// still open, began with a message, and runs on `provider`'s agent.
    async fn resumable(
        &self,
        session: &str,
        turn: &str,
        provider: &Provider,
    ) -> Option<(PathBuf, Map<String, Value>)> {
        let header = self.store.read(session).await.ok()??;
        let folder = header.directory().map(PathBuf::from)?;
        let fields = header.into_fields();
        let runs_on = fields
            .get("model")
            .and_then(|model| model.get("providerID"))
            .and_then(Value::as_str);
        if fields.get("parentID").is_some_and(|parent| !parent.is_null())
            || runs_on != Some(provider.to_string().as_str())
        {
            return None;
        }
        let open = self.store.open_turn(session).await.ok()??;
        let began = open.start["id"].as_str() == Some(turn)
            && matches!(open.start["type"].as_str(), Some("user" | "synthetic"));
        began.then_some((folder, fields))
    }

    /// A recovered turn that didn't go on after all: its agent is let go of.
    pub(super) fn let_go(&self, adopted: AdoptedTurn) {
        let roster = self.agents.roster();
        tokio::spawn(async move {
            if let Some(entry) = roster.find(&adopted.provider) {
                let _ = entry.agent.let_go(&adopted.note).await;
            }
        });
    }

    /// The resident making way for the next: no new turn starts here, what
    /// waits behind a turn its agent won't go on with is kept for the next
    /// resident (`held`, steered: it goes once, in order, as that one
    /// starts), and each turn that goes on is handed over with what it was
    /// asking and what you approved for it. `held`: the conversations whose
    /// turns go on in held agents.
    pub(super) async fn hand_over(&self, state: &mut State, held: &BTreeSet<String>) -> Handover {
        state.closing = true;
        if let Err(error) = self.asks.keep_unanswered().await {
            eprintln!("Kuvo could not keep the agents' asks for the next resident: {error}");
        }
        let pending = self.asks.pending().borrow().clone();
        let mut handover = Handover::default();
        let busy = state
            .conversations
            .iter()
            .filter(|(_, conversation)| !matches!(conversation.doing, Doing::Idle))
            .map(|(session, conversation)| {
                let handing = conversation.handing.keys().cloned().collect::<BTreeSet<_>>();
                (session.clone(), conversation.doing.live().map(|live| live.id.clone()), handing)
            })
            .collect::<Vec<_>>();
        for (session, turn, handing) in busy {
            if let Some(turn) = turn.filter(|_| held.contains(&session)) {
                let asks = pending
                    .keys()
                    .filter(|key| key.session == session && key.turn == turn)
                    .map(|key| key.id.clone())
                    .collect();
                let always = state
                    .approvals
                    .get(&session)
                    .map(|approvals| approvals.borrow().clone())
                    .unwrap_or_default();
                handover
                    .sessions
                    .insert(session, HandedTurn { turn, asks, always });
                continue;
            }
            let kept = self
                .store
                .edit_line(&session, move |line| {
                    let mut kept = false;
                    for input in &mut line.inputs {
                        if input.steer_receipt.is_some()
                            || handing.contains(&input.id)
                            || input.is_compaction()
                        {
                            continue;
                        }
                        input.held = true;
                        input.delivery = Delivery::Steer;
                        kept = true;
                    }
                    ((), kept)
                })
                .await;
            if let Err(error) = kept {
                eprintln!("Kuvo could not keep what waits in {session}: {error}");
            }
        }
        handover
    }
}

/// Asks a resumed turn showed again stop being waited for, as do all once
/// their time is up.
async fn follow_replays(
    restoring: std::sync::Arc<Restorings>,
    mut pending: watch::Receiver<std::sync::Arc<BTreeMap<AskKey, crate::live_asks::Ask>>>,
) {
    loop {
        let shown = pending.borrow_and_update().clone();
        let now = Instant::now();
        restoring.send_if_modified(|sessions| {
            let before = sessions.len();
            let mut shrank = false;
            sessions.retain(|_, restoring| {
                let count = restoring.asks.len();
                restoring.asks.retain(|key| !shown.contains_key(key));
                shrank |= restoring.asks.len() != count;
                !restoring.asks.is_empty() && restoring.until > now
            });
            shrank || sessions.len() != before
        });
        let Some(next) = restoring.borrow().values().map(|each| each.until).min() else {
            return;
        };
        tokio::select! {
            changed = pending.changed() => if changed.is_err() { return },
            () = tokio::time::sleep_until(next) => {}
        }
    }
}

/// Wait until the asks a resumed turn in `session` showed before the update
/// are shown again, or have had their time.
pub(super) async fn restored(restoring: &Restorings, session: &str) {
    let mut watching = restoring.subscribe();
    let _ = watching
        .wait_for(|sessions| !sessions.contains_key(session))
        .await;
}

pub(super) fn stopped_restoring(restoring: &Restorings, session: &str) {
    restoring.send_if_modified(|sessions| sessions.remove(session).is_some());
}
