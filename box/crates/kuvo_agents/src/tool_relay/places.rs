//! Where Kuvo Code's own tools are, as the box knows them: the program its
//! engine's settings start for them (`kuvo-browser`), which an agent's
//! relay runs with the conversation in `KUVO_SESSION`.

use super::{ToolSource, Tools};
use kuvo_os::platform::Environment;
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf, sync::Mutex, time::Duration};

/// The name Kuvo Code's own tools have among the engine's servers.
pub const TOOL_SERVER_NAME: &str = "kuvo-browser";

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
struct ToolProgram {
    command: Vec<String>,
    environment: BTreeMap<String, String>,
    #[serde(skip)]
    timeout: Duration,
}

/// Where each conversation's Kuvo Code tools are: none until the box is told.
#[derive(Default)]
pub struct ToolPlaces {
    program: Mutex<Option<ToolProgram>>,
}

const CALL_TIME: Duration = Duration::from_secs(960);

impl ToolPlaces {
    /// The program as the settings the engine starts with name it
    /// (`OPENCODE_CONFIG_CONTENT`), if they do.
    pub fn set_program_settings(&self, settings: Option<&str>) {
        let settings = settings.and_then(|settings| serde_json::from_str::<Value>(settings).ok());
        let entry = settings.as_ref().map(|settings| &settings["mcp"]["servers"][TOOL_SERVER_NAME]);
        let program = entry.and_then(|entry| {
            let command = entry["command"].as_array()?;
            let command = command.iter().map(|part| part.as_str().map(str::to_owned)).collect::<Option<Vec<_>>>()?;
            let environment = entry["environment"].as_object().into_iter().flatten();
            let environment = environment.filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_owned())));
            let timeout = entry["timeout"]["execution"].as_f64().filter(|time| *time > 0.0);
            let timeout = timeout.map_or(CALL_TIME, |time| Duration::from_millis(time as u64));
            (!command.is_empty()).then(|| ToolProgram { command, environment: environment.collect(), timeout })
        });
        *self.program.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = program;
    }
}

impl ToolSource for ToolPlaces {
    fn tools(&self, session: &str) -> Option<Tools> {
        let program = self.program.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()?;
        let mut env: Environment = std::env::vars_os().collect();
        env.extend(program.environment.iter().map(|(name, value)| (name.into(), value.into())));
        env.insert("KUVO_SESSION".into(), session.into());
        let (command, args) = program.command.split_first()?;
        Some(Tools {
            key: serde_json::to_string(&program).unwrap_or_default(),
            command: PathBuf::from(command),
            args: args.iter().map(Into::into).collect(),
            env,
            timeout: program.timeout,
        })
    }
}
