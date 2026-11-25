//! What each ACP agent does its own way: how it starts and signs in, picks a model and a mode, the
//! lines and methods of its own, and its account. The client in `acp` does the rest the same for
//! all of them.

use std::{io, path::Path};

use futures_util::future::BoxFuture;
use serde_json::Value;

use crate::{
    account_check::{AccountStatus, CheckFailure},
    accounts::AgentKind,
    catalog::Models,
    hold::Start,
    program_updates::Definition,
    rpc::{Rpc, RpcError},
    runtime::AdapterCall,
    session::{LimitScope, ToolOutput},
    sign_in::{Attempt, Request},
};

use super::{
    tools::Call,
    wire::{Capabilities, Setup},
};

/// How a process is to run: what the turn needs of it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Mode {
    pub approve_all: bool,
    pub plan: bool,
}

/// A process ready for its holder to start, and what tells it apart.
pub(crate) struct Launch {
    pub start: Start,
    /// A folder of the process's own, removed as it ends.
    pub temporary: Option<std::path::PathBuf>,
    /// A file the agent writes when it wants a new sign-in (its "browser" notes the page).
    pub sign_in_file: Option<std::path::PathBuf>,
    /// Who it's signed in as as it starts, for the models it lists to be theirs.
    pub credential: Option<String>,
}

/// What one of the agent's lines is.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Line {
    Protocol,
    /// Not ACP (a sign-in link printed): dropped.
    Drop,
    /// It wants a new sign-in.
    AsksSignIn,
    /// It can't go on: why, in words.
    Fatal(String),
}

/// Something the agent keeps going in the background, which wakes it as it ends.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Task {
    pub id: String,
    pub command: Option<String>,
    pub description: Option<String>,
    pub kind: Option<String>,
    /// When it started (ms).
    pub started: Option<u64>,
    pub output_file: Option<String>,
}

/// A notification of the agent's own.
#[derive(Clone, Debug)]
pub(crate) enum Extension {
    /// What it keeps going, listed whole at each change (`replayed`: a load's, from before).
    Background { session: Option<String>, tasks: Vec<Task>, replayed: bool },
    /// A prompt is over (it can say so before answering): one of its own (`own`) or one sent.
    PromptComplete { own: bool, stop: String, error: Option<String>, prompt: Option<String> },
}

/// A question of the agent's own.
#[derive(Clone, Debug)]
pub(crate) struct Question {
    pub id: String,
    pub question: String,
    pub header: Option<String>,
    pub options: Vec<(String, Option<String>)>,
    pub multiple: bool,
}

/// A request of the agent's own.
#[derive(Clone, Debug)]
pub(crate) enum OwnRequest {
    Questions { tool_call: Option<String>, questions: Vec<Question> },
    /// Its plan, done in plan mode.
    ExitPlan { plan: Option<String> },
}

/// How an own request was answered: values by question ID, or turned down (with words, or none).
pub(crate) enum OwnAnswer {
    Answered(serde_json::Map<String, Value>),
    Declined(Option<String>),
}

pub(crate) trait Flavour: Send + Sync + 'static {
    fn kind(&self) -> AgentKind;

    /// As people read it: "Grok".
    fn name(&self) -> &'static str;

    // Its account: never starts a session's process.

    fn probe(&self) -> BoxFuture<'_, Result<AccountStatus, CheckFailure>>;
    /// Its models, as last listed: never by starting it.
    fn models(&self, refresh: bool) -> AdapterCall<'_, Models>;
    fn sign_in(&self, request: Request) -> AdapterCall<'_, std::sync::Arc<dyn Attempt>>;
    fn sign_out(&self) -> AdapterCall<'_, ()>;
    fn remove_credentials(&self) -> AdapterCall<'_, ()>;
    fn program(&self) -> Option<Definition>;
    /// What its sign-in is now, never the secret: a process started on another is started again.
    fn credential(&self) -> Option<String>;

    // Its process.

    /// What a process for `directory`'s conversations with this mode is started with, without
    /// starting it: conversations whose keys match share one process. Read again before each
    /// turn: a new program, sign-in, mode or skills starts another.
    fn launch_key(&self, mode: &Mode, directory: &Path, model: &str) -> io::Result<String>;
    /// The process for that key, for its holder to start.
    fn launch(&self, mode: &Mode, directory: &Path, model: &str) -> io::Result<Launch>;
    /// The version a held process runs (Antigravity's `agy`, or its earlier agent), in its record.
    fn version(&self) -> Option<String> {
        None
    }
    /// Reads and writes files through Kuvo (it asks first itself).
    fn file_system(&self) -> bool {
        false
    }
    /// Its reads end only as its turn does: each shows done once it goes on.
    fn reads_unsaid(&self) -> bool {
        false
    }
    fn initialize_meta(&self) -> Option<Value> {
        None
    }
    /// The way it signs in when asked to (`authenticate`).
    fn auth_method(&self, capabilities: &Capabilities) -> Option<String>;
    /// Signs in before its first session, not only when it asks.
    fn authenticates_first(&self) -> bool {
        false
    }
    /// Prefers `session/resume` over `session/load` (which replays the conversation).
    fn resumes(&self) -> bool {
        false
    }
    /// Can't go on after a stop: started again for the next message.
    fn restarts_after_stop(&self) -> bool {
        false
    }
    fn limit_scope(&self) -> LimitScope {
        LimitScope::Provider
    }
    fn line(&self, _line: &str) -> Line {
        Line::Protocol
    }
    /// `authenticate`'s refusal says its sign-in is gone.
    fn sign_in_refused(&self, _error: &RpcError) -> bool {
        false
    }
    /// Its sign-in is gone: shown, until it's signed in again.
    fn sign_in_ended(&self) {}
    /// Its whole answer is its plan's limit, said as a reply that ends as done.
    fn limit_reply(&self, _text: &str) -> bool {
        false
    }
    fn select_model<'a>(
        &'a self,
        rpc: &'a Rpc,
        session: &'a str,
        model: &'a str,
        variant: Option<&'a str>,
        setup: &'a mut Setup,
        credential: Option<&'a str>,
    ) -> BoxFuture<'a, io::Result<()>>;
    /// Sets the mode the turn needs, if the agent has modes. An error: it can't be set, and the
    /// one it's in would do more without asking than the turn may.
    fn select_mode<'a>(
        &'a self,
        _rpc: &'a Rpc,
        _session: &'a str,
        _mode: &'a Mode,
        _setup: &'a mut Setup,
    ) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn prompt_meta(&self) -> Option<Value> {
        None
    }
    fn extension(&self, _method: &str, _params: &Value) -> Option<Extension> {
        None
    }
    fn own_request(&self, _method: &str, _params: &Value) -> Option<OwnRequest> {
        None
    }
    fn answer_own(&self, _request: &OwnRequest, _answer: OwnAnswer) -> Value {
        Value::Null
    }
    /// A call not worth a row.
    fn hides(&self, _call: &Call) -> bool {
        false
    }
    /// Files a finished call made that it doesn't hand back.
    fn made_files(&self, _call: &Call, _session: &str, _since_ms: u64) -> Vec<ToolOutput> {
        Vec::new()
    }
    /// How much the model holds, as it said.
    fn note_context(&self, _model: &str, _size: u64) {}
    /// What it said as a turn opened its session (its models).
    fn note_setup(&self, _setup: &Setup, _credential: Option<&str>) {}
}
