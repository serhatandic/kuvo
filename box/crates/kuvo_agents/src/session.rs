//! Shared contract for conversations run by an agent adapter.

use crate::{hold::TurnNote, runtime::AdapterCall};
use serde_json::{Map, Value};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

/// Stored as the agent's own slot of the conversation's `agent.json`
/// (`threads.<provider>.state`), beside the inbox and other agents' slots.
/// Keys and values within this object belong to the adapter.
pub type VendorState = Map<String, Value>;

pub struct SessionContext {
    pub session_id: String,
    pub directory: PathBuf,
    pub state: VendorState,
    /// Where its agent says what it keeps going besides its turn: commands it left running, and
    /// other tasks.
    pub background: crate::background::Background,
    /// Where its agent offers a turn it began on its own, between prompts.
    pub own_turns: OwnTurns,
}

/// A turn an agent began on its own (Grok after a task of its own ended), offered to the
/// resident: `taken` says whether a turn there takes it (`TurnKind::AgentStarted`).
pub struct OwnTurn {
    pub session: String,
    pub provider: String,
    /// What the conversation shows for it.
    pub text: String,
    pub taken: tokio::sync::oneshot::Sender<bool>,
}

/// The resident's end for turns agents begin on their own: none takes them until it's set.
#[derive(Clone, Default)]
pub struct OwnTurns(Option<tokio::sync::mpsc::UnboundedSender<OwnTurn>>);

impl OwnTurns {
    pub fn new(sender: tokio::sync::mpsc::UnboundedSender<OwnTurn>) -> Self {
        Self(Some(sender))
    }

    /// Offers one: whether it's taken (never, with no resident to take it).
    pub async fn offer(&self, session: &str, provider: &str, text: String) -> bool {
        let Some(sender) = &self.0 else { return false };
        let (taken, answer) = tokio::sync::oneshot::channel();
        let offer = OwnTurn {
            session: session.into(),
            provider: provider.into(),
            text,
            taken,
        };
        sender.send(offer).is_ok() && answer.await.unwrap_or(false)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelRef {
    /// Model provider ID, including any account suffix.
    pub provider: String,
    pub id: String,
    pub variant: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PromptFile {
    pub uri: String,
    pub mime: String,
    pub name: Option<String>,
    pub path: Option<PathBuf>,
    /// Base64 for an inline attachment.
    pub data: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Prompt {
    /// Kuvo's stable ID for this message. Keep it when retrying or steering.
    pub id: String,
    pub text: String,
    pub files: Vec<PromptFile>,
    pub synthetic: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionMode {
    AskFirst,
    ApproveAll,
}

pub enum TurnKind {
    Prompt(Prompt),
    Compact,
    /// A turn the agent started itself between prompts.
    AgentStarted,
    /// `Adoption::attach` has already installed the holder's reader. Connect
    /// that reader to this sink and resume at the saved replay boundary.
    Recover {
        note: TurnNote,
    },
}

pub struct TurnInput {
    pub id: String,
    pub kind: TurnKind,
    pub model: ModelRef,
    pub instructions: String,
    pub permissions: PermissionMode,
    pub plan: bool,
    /// On Auto, surface a usage limit promptly so the resident can switch.
    pub auto: bool,
    /// Withdraw a new process start that's waiting for memory before this turn.
    pub start_cancel: tokio_util::sync::CancellationToken,
}

/// A conversation on an account. Implementations may share a process between
/// threads. The resident serializes starts on each thread.
pub trait Thread: Send + Sync {
    /// Begin a run or connect a recovered run to the sink. Return once the
    /// adapter can accept turn controls. A running protocol task may emit
    /// events and await answers before and after this method returns.
    fn start(
        &self,
        input: TurnInput,
        sink: Arc<dyn TurnSink>,
    ) -> AdapterCall<'_, Arc<dyn ActiveTurn>>;

    /// The adapter's own conversation files, which count as part of its
    /// working area when the resident checks tool permissions.
    fn owns_path(&self, _path: &Path) -> bool {
        false
    }

    /// Whether its turns take a message partway through. Others use the
    /// resident's cancel/resume at the next reported step.
    fn takes_steers(&self) -> bool {
        true
    }

    /// Look for a previously uncertain delivery in the native conversation.
    /// Return `None` while it is still uncertain.
    fn resolve_steer(&self, _receipt: SteerReceipt) -> AdapterCall<'_, Option<bool>> {
        Box::pin(async { Ok(None) })
    }
}

pub trait ActiveTurn: Send + Sync {
    /// The receipt a message handed to this turn will have, known before it
    /// is sent. The resident saves it first, so after a crash it settles the
    /// message through `Thread::resolve_steer` instead of sending it again.
    fn steer_receipt(&self, _prompt: &Prompt) -> Option<SteerReceipt> {
        None
    }

    fn steer(&self, _prompt: Prompt) -> AdapterCall<'_, Steer> {
        Box::pin(async { Ok(Steer::Declined) })
    }

    /// Ask the adapter to stop. The protocol task then reports the outcome
    /// through the sink.
    fn interrupt(&self) -> AdapterCall<'_, ()>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Steer {
    Accepted,
    Declined,
    /// The vendor may have taken it. Resolve by its receipt before retrying.
    InDoubt(SteerReceipt),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SteerReceipt {
    pub thread: String,
    pub turn: String,
    pub message: String,
}

/// Events are sent in adapter order. When a holder frame generates several
/// events, assign them stable ordinals so recovery can recognize each event.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Position {
    pub sequence: u64,
    pub ordinal: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Delivery {
    Live,
    Replay,
}

#[derive(Clone, Debug)]
pub struct EventEnvelope {
    pub position: Option<Position>,
    pub delivery: Delivery,
    pub event: TurnEvent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextKind {
    Text,
    Reasoning,
}

#[derive(Clone, Debug)]
pub enum ToolOutput {
    Text(String),
    File {
        uri: String,
        mime: String,
        name: Option<String>,
    },
}

#[derive(Clone, Debug)]
pub enum ToolState {
    Streaming(String),
    Running {
        input: Value,
    },
    Completed {
        input: Value,
        content: Vec<ToolOutput>,
    },
    Failed {
        input: Value,
        content: Vec<ToolOutput>,
        error: TurnError,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub reasoning: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct TurnError {
    pub kind: String,
    pub message: String,
    pub status: Option<u16>,
    pub limit: Option<Limit>,
}

impl TurnError {
    /// As clients read it: `{type, message, status?, limit?}`.
    pub fn reason(&self) -> Value {
        let mut reason = serde_json::json!({"type":self.kind,"message":self.message});
        if let Some(status) = self.status {
            reason["status"] = serde_json::json!(status);
        }
        if let Some(limit) = &self.limit {
            let scope = match limit.scope {
                LimitScope::Provider => "provider",
                LimitScope::Model => "model",
            };
            reason["limit"] = serde_json::json!({"scope":scope});
            if let Some(at) = limit.resets_at {
                reason["limit"]["resetsAt"] = serde_json::json!(at);
            }
        }
        reason
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LimitScope {
    Provider,
    Model,
}

#[derive(Clone, Debug)]
pub struct Limit {
    pub scope: LimitScope,
    pub resets_at: Option<u64>,
}

#[derive(Clone, Debug)]
pub enum Outcome {
    Succeeded,
    Failed(TurnError),
    Interrupted,
    AgentExited(Option<TurnError>),
}

#[derive(Clone, Debug)]
pub struct TurnResult {
    pub outcome: Outcome,
    pub took_prompt: bool,
    pub missed: Vec<String>,
}

/// One reading of the plan's limits an agent heard in a turn, as the limits
/// ring keeps it.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanLimitReading {
    pub windows: Vec<LimitWindow>,
    /// The sign-in it was heard under, as a fingerprint of who's signed in
    /// (never the secret). A reading under one that isn't the account's now
    /// counts for no one.
    pub sign_in: Option<String>,
}

/// Whether the maker could supply a plan reading for this sign-in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectPlanStatus {
    Ready,
    NoPlan,
    Failed,
}

/// Facts available from a direct account read, not a turn notification.
#[derive(Clone, Debug)]
pub struct DirectPlanReading {
    pub reading: PlanLimitReading,
    pub plan: Option<String>,
    pub extra_usage: Option<PlanSpend>,
    pub credits: Option<f64>,
    pub resets: Option<ResetInventory>,
    pub reset_unreadable: Option<String>,
    pub status: DirectPlanStatus,
    pub only_resets: bool,
}
impl DirectPlanReading {
    pub fn windows(reading: PlanLimitReading) -> Self {
        Self {
            reading,
            plan: None,
            extra_usage: None,
            credits: None,
            resets: None,
            reset_unreadable: None,
            status: DirectPlanStatus::Ready,
            only_resets: false,
        }
    }
}
#[derive(Clone, Debug)]
pub struct PlanSpend {
    pub amount: f64,
    pub limit: Option<f64>,
    pub resets_at: Option<u64>,
}
#[derive(Clone, Debug)]
pub struct ResetInventory {
    pub count: u32,
    pub targets: Vec<ResetTarget>,
    pub refills: Vec<String>,
    pub display_expiries: Option<Vec<u64>>,
}
#[derive(Clone, Debug)]
pub struct ResetTarget {
    pub id: String,
    pub expires_at: Option<u64>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetOutcome {
    Reset,
    AlreadyRedeemed,
    NothingToReset,
    NoReset,
    NotNow,
    MakerNotNow(Option<u64>),
    MakerNoReset,
    MakerFailed,
    Failed,
    Unknown,
    Unconfirmed,
}

/// One of a plan's limits, as its maker says it stands.
#[derive(Clone, Debug, PartialEq)]
pub struct LimitWindow {
    /// Its maker's own name for it: `five_hour`, `seven_day`, `seven_day_sonnet`.
    pub id: String,
    /// How much of it is used, from 0 to 1.
    pub used: f64,
    /// How long it is, in minutes (300: five hours; 10080: a week).
    pub minutes: Option<u32>,
    /// When it starts over, in ms since 1970, when its maker says.
    pub resets_at: Option<u64>,
    /// The model it's for, as its maker names it; none for the whole plan's.
    pub model: Option<String>,
}

#[derive(Clone, Debug)]
pub enum TurnEvent {
    StepStarted {
        model: Option<ModelRef>,
    },
    TextDelta {
        kind: TextKind,
        /// Native content block key when text and reasoning can interleave.
        block: Option<String>,
        delta: String,
    },
    Tool {
        id: String,
        name: String,
        state: ToolState,
        metadata: Map<String, Value>,
    },
    StepEnded {
        usage: Option<Usage>,
        finish: Option<String>,
        error: Option<TurnError>,
    },
    /// The agent reached a real boundary during its turn (not the synthetic closing rows).
    StepBoundary,
    Context {
        used: Option<u64>,
        window: Option<u64>,
    },
    ContextCleared,
    /// What the turn cost besides its steps' own costs, in dollars: an agent
    /// that says what it spent once per turn.
    Cost(f64),
    /// Where the plan's limits stand, as the agent heard in this turn: for
    /// the account the turn runs on (the plan limits' ring, 5.4).
    PlanLimit(PlanLimitReading),
    RanOn(String),
    Notice(String),
    CompactionStarted,
    CompactionEnded {
        succeeded: bool,
    },
    ReplayCaughtUp,
    /// The title its agent gave the conversation, for one still without one.
    Titled(String),
    Finished(TurnResult),
}

#[derive(Clone, Debug)]
pub struct PermissionAsk {
    /// Stable within this turn, including when the native ask is replayed.
    pub id: String,
    pub tool_call_id: Option<String>,
    pub action: String,
    pub resources: Vec<String>,
    pub save: Vec<String>,
    pub message: Option<String>,
    pub metadata: Map<String, Value>,
    pub sandboxed: Option<bool>,
    pub already_weighed: bool,
    pub unclassified: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decision {
    Once,
    Always,
    Reject,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Preflight {
    /// The resident has no policy that should override the adapter here.
    NoOverride,
    Ask,
    Deny,
}

#[derive(Clone, Debug)]
pub struct FormOption {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug)]
pub enum FieldKind {
    String {
        options: Vec<FormOption>,
        custom: bool,
        format: Option<String>,
    },
    MultiSelect {
        options: Vec<FormOption>,
        custom: bool,
    },
    Boolean,
    External {
        url: Option<String>,
    },
}

#[derive(Clone, Debug)]
pub struct Field {
    pub key: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub required: bool,
    pub kind: FieldKind,
}

#[derive(Clone, Debug)]
pub struct QuestionAsk {
    pub id: String,
    pub title: String,
    pub fields: Vec<Field>,
    pub tool_call_id: Option<String>,
    pub metadata: Map<String, Value>,
}

#[derive(Clone, Debug)]
pub enum Answer {
    /// Field values submitted by the client.
    Submitted(Map<String, Value>),
    Dismissed {
        reason: Option<String>,
    },
}

/// The resident owns the transcript, account policy and outstanding asks.
/// Keep ask futures off the native output reader so it can process cancellation
/// and further output. A cancelled or retired sink completes pending asks.
pub trait TurnSink: Send + Sync {
    fn event(&self, event: EventEnvelope) -> AdapterCall<'_, ()>;

    /// Replace the vendor object and persist it before acknowledging. Removing
    /// a key here also removes it from disk, for example after clearing a
    /// native conversation.
    fn save_state(&self, state: VendorState) -> AdapterCall<'_, ()>;

    /// Apply resident policy before the adapter applies its native policy.
    /// This never creates an interactive ask. Set `native_plan` if the
    /// adapter enforces plan mode on its own.
    fn preflight(&self, ask: PermissionAsk, native_plan: bool) -> AdapterCall<'_, Preflight>;
    /// Show the ask until it's answered. Keep `ask.id` the agent's own, so a
    /// replay after a resident update shows the same ask, once. An `Err`
    /// means the resident is going away: leave the native request unanswered
    /// for the next resident, which asks again.
    fn permission(&self, ask: PermissionAsk) -> AdapterCall<'_, Decision>;
    fn question(&self, ask: QuestionAsk) -> AdapterCall<'_, Answer>;

    /// Withdraw an ask the vendor cancelled. The matching future resolves as
    /// a rejection or dismissal. Calling again after resolution is harmless.
    fn cancel_ask<'a>(&'a self, id: &'a str) -> AdapterCall<'a, ()>;
}
