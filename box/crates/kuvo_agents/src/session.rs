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

    /// Whether its turns take a message partway through. Messages sent to one
    /// that doesn't wait for the turn's end.
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
    Context {
        used: Option<u64>,
        window: Option<u64>,
    },
    ContextCleared,
    RanOn(String),
    Notice(String),
    CompactionStarted,
    CompactionEnded {
        succeeded: bool,
    },
    ReplayCaughtUp,
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
