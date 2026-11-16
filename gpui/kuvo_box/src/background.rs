//! What runs for a conversation besides its own turn (PROTOCOL.md, "Work › Background work"), for the
//! bars over its composer: each command it left running and each subagent at work under it, as the
//! box lists them (`kuvo/work/background/<session>`), read again as the box says it changed
//! (`kuvo/work/background/events`). A command's output is read and it's stopped where it runs.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::json;

use crate::error::{Error, Refusal, Result};
use crate::follow::{Followed, follow_lines};
use crate::paths;
use crate::transport::Request;
use crate::{BoxClient, Follow, Timing};

/// A command a conversation left running.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Command {
    pub id: String,
    pub command: String,
    /// When it started (ms).
    pub started: f64,
    /// The folder its engine knows it in.
    #[serde(default)]
    pub folder: Option<String>,
    /// A coding agent's: read through their bridge.
    #[serde(default)]
    pub agents: bool,
}

/// A subagent at work under a conversation.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Subagent {
    pub session: String,
    /// Since when (ms).
    pub since: f64,
    #[serde(default)]
    pub title: Option<String>,
    /// It waits on you.
    #[serde(default)]
    pub asking: bool,
}

/// What runs for one conversation besides its turn, the oldest first.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Background {
    #[serde(default)]
    pub commands: Vec<Command>,
    #[serde(default)]
    pub subagents: Vec<Subagent>,
}

/// How many of each run for a session, as `kuvo/work/background` counts them.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Counted {
    #[serde(default)]
    pub commands: u32,
    #[serde(default)]
    pub subagents: u32,
    #[serde(default)]
    pub tasks: u32,
    #[serde(default)]
    pub since: f64,
}

/// Every session with something running besides its turn, by its id.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Counts {
    #[serde(default)]
    pub sessions: BTreeMap<String, Counted>,
}

/// What a read of a command's output found, as Output shows it (KuvoClient's `ShellRead`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    /// The end of what it has printed.
    Printed(String),
    /// Its engine no longer has it: it ended, and what it printed went with it.
    Ended,
    /// It couldn't be read now (its computer away); a later read may.
    Unread,
}

/// How much of the end of a command's output Output shows, in bytes.
pub const SHOWN_OUTPUT: usize = 24_000;

#[derive(Deserialize)]
struct Stretch {
    #[serde(default)]
    output: String,
    #[serde(default)]
    size: usize,
}

#[derive(Deserialize)]
struct Located<T> {
    data: T,
}

impl Command {
    fn shell_route(&self, rest: &str) -> String {
        let prefix = if self.agents { "kuvo/agents/" } else { "" };
        format!("{prefix}api/shell/{}{rest}", paths::segment(&self.id))
    }

    fn in_folder(&self, request: Request) -> Request {
        match &self.folder {
            Some(folder) => request.header("x-opencode-directory", paths::query_value(folder)),
            None => request,
        }
    }
}

impl BoxClient {
    /// What runs for `session` besides its turn, each one.
    pub fn session_background(&self, session: &str) -> Result<Background> {
        self.get(&format!("kuvo/work/background/{}", paths::segment(session)))
    }

    /// How many run for each session, now and as it changes.
    pub fn follow_background(&self, timing: Timing) -> Follow<Followed<Counts>> {
        follow_lines(self, "kuvo-box background", "kuvo/work/background/events".into(), "kuvo/work/background".into(), timing)
    }

    /// Stops a command it left running: its agent hears that it ended, with what it printed.
    pub fn stop_command(&self, command: &Command) -> Result<()> {
        let request = command.in_folder(Request::post(format!("kuvo/shell/{}/stop", paths::segment(&command.id)), json!({})));
        let response = self.send(&request)?;
        // (Gone already: it ended as it was stopped.)
        if !response.is_success() && response.status != 404 {
            return Err(Error::Refused(Refusal::from_answer(response.status, &response.body)));
        }
        Ok(())
    }

    /// The end of what `command` has printed: its last [`SHOWN_OUTPUT`] bytes, from the first whole
    /// line in them; with none, all but the character the start cut in two.
    pub fn command_output(&self, command: &Command) -> Output {
        let read = |from: Option<usize>, limit: usize| -> std::result::Result<Stretch, u16> {
            let cursor = from.map(|from| format!("&cursor={from}")).unwrap_or_default();
            let request = command.in_folder(Request::get(command.shell_route(&format!("/output?limit={limit}{cursor}"))));
            let response = self.send(&request).map_err(|_| 0u16)?;
            if !response.is_success() {
                // (The engine's own word that it has no such command, not any 404 on the way.)
                let said = Refusal::from_answer(response.status, &response.body);
                let gone = response.status == 404 && (said.message.starts_with("Shell command not found") || said.message.starts_with("No such command"));
                return Err(if gone { 404 } else { response.status });
            }
            serde_json::from_slice::<Located<Stretch>>(&response.body).map(|located| located.data).map_err(|_| 0)
        };
        let size = match read(None, 1) {
            Ok(stretch) => stretch.size,
            Err(404) => return Output::Ended,
            Err(_) => return Output::Unread,
        };
        let start = size.saturating_sub(SHOWN_OUTPUT);
        match read(Some(start), SHOWN_OUTPUT) {
            Ok(stretch) => Output::Printed(end_of(&stretch.output, start > 0)),
            Err(404) => Output::Ended,
            Err(_) => Output::Unread,
        }
    }
}

/// What's shown of a stretch read from `cut` into the output: from its first whole line, or with
/// none, all but a character the cut left half of.
fn end_of(output: &str, cut: bool) -> String {
    let mut shown = output;
    if cut {
        shown = match output.find(['\n', '\r']) {
            Some(at) if !output[at + 1..].trim_matches(['\n', '\r']).is_empty() => &output[at + 1..],
            _ => output.trim_start_matches('\u{FFFD}'),
        };
    }
    shown.trim_matches(['\n', '\r']).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_cut_at_its_start_shows_from_its_first_whole_line() {
        assert_eq!(end_of("half a line\nwhole one\nlast\n", true), "whole one\nlast");
        assert_eq!(end_of("\u{FFFD}one long line with no end", true), "one long line with no end");
        assert_eq!(end_of("all of it\n", false), "all of it");
    }

    #[test]
    fn a_command_of_the_coding_agents_is_read_through_their_bridge() {
        let background: Background = serde_json::from_str(r#"{"commands":[{"id":"sh_1","command":"npm run dev","started":5,"folder":"/p"},{"id":"sh_2","command":"make","started":6,"agents":true}],"subagents":[{"session":"ses_2","since":4,"title":"Look","asking":true}]}"#).unwrap();
        assert_eq!(background.commands[0].shell_route("/output"), "api/shell/sh_1/output");
        assert_eq!(background.commands[1].shell_route("/output"), "kuvo/agents/api/shell/sh_2/output");
        assert!(background.subagents[0].asking);
    }
}
