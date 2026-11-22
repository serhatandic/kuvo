//! What a command a conversation left running has printed, as it prints (PROTOCOL.md, "Work ›
//! Background work", `work.background.output`): the box reads it where the command runs, cuts it
//! to its end, and says whether it still runs; a client only shows it.

use serde::Deserialize;

use crate::background::Command;
use crate::error::Result;
use crate::follow::{Followed, follow_lines};
use crate::paths;
use crate::{BoxClient, Follow};

pub const OUTPUT_STREAM: &str = "work.background.output";

/// Whether the command still runs, as the box last read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PrintedState {
    Running,
    /// Its engine no longer has it: it ended.
    Ended,
    /// It couldn't be read now (its computer away); a later read may.
    Unread,
    #[serde(other)]
    Unknown,
}

/// The end of what a command has printed.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Printed {
    /// The command, while the box still lists it.
    #[serde(default)]
    pub command: Option<Command>,
    #[serde(default)]
    pub output: String,
    /// Its start was cut: what shows is its end.
    #[serde(default)]
    pub cut: bool,
    pub state: PrintedState,
}

fn route(session: &str, command: &str) -> String {
    format!("kuvo/work/background/{}/commands/{}/output", paths::segment(session), paths::segment(command))
}

impl BoxClient {
    /// What `command`, left running by `session`, has printed now.
    pub fn printed(&self, session: &str, command: &str) -> Result<Printed> {
        self.get(&route(session, command))
    }

    /// What `command` has printed, now and as it prints more (and once, when it ends).
    pub fn follow_printed(&self, session: &str, command: &str) -> Follow<Followed<Printed>> {
        let once = route(session, command);
        follow_lines(self, format!("{once}/events"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_command_printed_reads_with_whether_it_still_runs() {
        let printed: Printed = serde_json::from_str(r#"{"command":{"id":"sh_1","command":"bun run dev","started":5},"output":"Ready in 612ms","cut":false,"state":"running"}"#).unwrap();
        assert_eq!(printed.state, PrintedState::Running);
        assert_eq!(printed.command.unwrap().command, "bun run dev");
        let ended: Printed = serde_json::from_str(r#"{"command":null,"output":"done","cut":true,"state":"ended"}"#).unwrap();
        assert_eq!((ended.state, ended.cut), (PrintedState::Ended, true));
        assert_eq!(route("ses 1", "sh/2"), "kuvo/work/background/ses%201/commands/sh%2F2/output");
    }
}
