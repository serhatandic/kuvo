use std::{
    io::{self, BufRead, BufReader, Read, Write},
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use futures_util::future::{BoxFuture, FutureExt, Shared};
use kuvo_os::process::{Spawn, Stdio};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::catalog::{Model, Models};

use super::{program::Program, wire};

type Loading = Shared<BoxFuture<'static, Result<Models, Arc<io::Error>>>>;

#[derive(Clone)]
pub(super) struct Catalog {
    prefix: Arc<str>,
    cache: Arc<Mutex<Cache>>,
}

#[derive(Default)]
struct Cache {
    generation: u64,
    loading: Option<Loading>,
    models: Option<Models>,
}

impl Catalog {
    pub fn new() -> io::Result<Self> {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).map_err(io::Error::other)?;
        Ok(Self {
            prefix: hex::encode(bytes).into(),
            cache: Arc::default(),
        })
    }

    fn stamp(&self, mut models: Models, generation: u64) -> Models {
        models.generation = Some(format!("{}-{generation}", self.prefix));
        models
    }

    pub async fn clear(&self) {
        let mut cache = self.cache.lock().await;
        *cache = Cache {
            generation: cache.generation + 1,
            ..Cache::default()
        };
    }

    pub async fn read(&self, program: Program, refresh: bool) -> io::Result<Models> {
        loop {
            let (generation, loading, previous) = {
                let mut cache = self.cache.lock().await;
                if !refresh && let Some(models) = &cache.models {
                    return Ok(models.clone());
                }
                let generation = cache.generation;
                let loading = cache
                    .loading
                    .get_or_insert_with(|| {
                        let catalog = self.clone();
                        let program = program.clone();
                        let task = async move {
                            let result = load(program)
                                .await
                                .map(|models| catalog.stamp(models, generation))
                                .map_err(Arc::new);
                            let mut cache = catalog.cache.lock().await;
                            if generation == cache.generation {
                                cache.loading = None;
                                if let Ok(models) = &result {
                                    cache.models = Some(models.clone());
                                }
                            }
                            result
                        }
                        .boxed()
                        .shared();
                        tokio::spawn(task.clone());
                        task
                    })
                    .clone();
                (generation, loading, cache.models.clone())
            };
            if !refresh {
                return Ok(previous.unwrap_or_else(|| self.stamp(fallback(), generation)));
            }
            let answer = loading.await;
            if self.cache.lock().await.generation == generation {
                return answer.map_err(|error| io::Error::new(error.kind(), error));
            }
        }
    }
}

async fn load(program: Program) -> io::Result<Models> {
    let response = tokio::task::spawn_blocking(move || initialize(program)).await??;
    let list: Vec<Info> =
        serde_json::from_value(response.get("models").cloned().unwrap_or(Value::Null))
            .map_err(|_| io::Error::other("Claude Code didn’t list its models."))?;
    if list.is_empty() {
        return Err(io::Error::other("Claude Code didn’t list its models."));
    }
    Ok(Models {
        complete: true,
        models: named_models(list),
        ..Models::default()
    })
}

fn initialize(program: Program) -> io::Result<Value> {
    let env = program.environment(true);
    let executable = program
        .executable(&env)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Claude Code isn't installed."))?;
    let mut spawn = Spawn::new(executable.path(), env);
    spawn.executable = executable;
    spawn.cwd = Some(program.home.run());
    spawn.stdin = Stdio::Pipe;
    spawn.stdout = Stdio::Pipe;
    spawn.args = [
        "--output-format",
        "stream-json",
        "--verbose",
        "--input-format",
        "stream-json",
        "--no-session-persistence",
        "--strict-mcp-config",
        "--settings",
        r#"{"disableAllHooks":true}"#,
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    let mut child = spawn.spawn()?;
    let mut stdin = child.stdin.take().expect("requested stdin");
    let stdout = child.stdout.take().expect("requested stdout");
    let (send, receive) = mpsc::sync_channel(1);
    let reader = thread::Builder::new()
        .name("claude-models".into())
        .spawn(move || {
            let _ = send.send(model_reply(BufReader::new(stdout)));
        })?;
    writeln!(
        stdin,
        "{}",
        wire::control_request("catalog", json!({"subtype":"initialize"}))
    )?;
    let result = receive.recv_timeout(Duration::from_secs(30));
    drop(stdin);
    child.stop(Duration::from_secs(1), |_| {})?;
    let _ = reader.join();
    result.map_err(|error| match error {
        mpsc::RecvTimeoutError::Timeout => io::Error::new(
            io::ErrorKind::TimedOut,
            "Claude Code didn't answer in time.",
        ),
        mpsc::RecvTimeoutError::Disconnected => {
            io::Error::other("Claude Code stopped before listing its models.")
        }
    })?
}

fn model_reply(mut stdout: impl BufRead) -> io::Result<Value> {
    // A catalog is short. Do not accept an unbounded stream from a broken CLI.
    let mut stdout = stdout.by_ref().take(8_000_001);
    let mut line = String::new();
    loop {
        line.clear();
        if stdout.read_line(&mut line)? == 0 || !line.ends_with('\n') {
            return Err(io::Error::other(
                "Claude Code stopped before listing its models.",
            ));
        }
        if let Ok(wire::Frame::ControlResponse { response }) =
            serde_json::from_str::<wire::Frame>(&line)
            && response.request_id == "catalog"
        {
            return match response.body {
                wire::ReplyBody::Success { response, .. } => Ok(response),
                wire::ReplyBody::Error => {
                    Err(io::Error::other("Claude Code refused to list its models."))
                }
            };
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Info {
    value: String,
    display_name: String,
    resolved_model: Option<String>,
    description: Option<String>,
    #[serde(default)]
    supported_effort_levels: Vec<String>,
}

impl Info {
    fn described_name(&self) -> Option<&str> {
        let name = self.description.as_deref()?.split(" · ").next()?.trim();
        ["opus", "sonnet", "haiku", "fable", "claude"]
            .into_iter()
            .any(|family| name.to_ascii_lowercase().starts_with(family))
            .then_some(name)
    }

    fn name(&self) -> &str {
        self.described_name().unwrap_or(&self.display_name)
    }

    fn stands_for(&self, other: &Self) -> bool {
        match (&self.resolved_model, &other.resolved_model) {
            (Some(a), Some(b)) => a == b,
            _ => self.name() == other.name(),
        }
    }

    fn default_name(&self) -> String {
        if let Some(name) = self.described_name() {
            return name.into();
        }
        let Some(native) = self.resolved_model.as_deref() else {
            return "Claude Code's recommended model".into();
        };
        let mut parts = native
            .strip_prefix("claude-")
            .unwrap_or(native)
            .split('-')
            .filter(|part| {
                !part.is_empty()
                    && (part.bytes().all(|b| b.is_ascii_alphabetic())
                        || part.len() <= 2 && part.bytes().all(|b| b.is_ascii_digit()))
            });
        let Some(maker) = parts.next() else {
            return "Claude Code's recommended model".into();
        };
        let family = maker[..1].to_ascii_uppercase() + &maker[1..];
        let version = parts.collect::<Vec<_>>().join(".");
        if version.is_empty() {
            family
        } else {
            format!("{family} {version}")
        }
    }
}

fn named_models(list: Vec<Info>) -> Vec<Model> {
    let default = list.iter().position(|info| info.value == "default");
    let equivalent = default.and_then(|standing| {
        list.iter()
            .enumerate()
            .position(|(index, info)| index != standing && list[standing].stands_for(info))
    });
    let mut models: Vec<_> = list
        .into_iter()
        .enumerate()
        .filter_map(|(index, info)| {
            if Some(index) == default {
                if equivalent.is_some() {
                    return None;
                }
                let name = info.default_name();
                let concrete = info.resolved_model.is_some();
                return Some((
                    index,
                    model(
                        info.resolved_model.unwrap_or(info.value),
                        name,
                        info.supported_effort_levels,
                        concrete,
                        false,
                    ),
                ));
            }
            let place = default
                .filter(|_| Some(index) == equivalent)
                .unwrap_or(index);
            let name = info.name().to_owned();
            Some((
                place,
                model(
                    info.value,
                    name,
                    info.supported_effort_levels,
                    Some(index) == equivalent,
                    false,
                ),
            ))
        })
        .collect();
    models.sort_by_key(|(index, _)| *index);
    models.into_iter().map(|(_, model)| model).collect()
}

fn model(
    id: String,
    name: String,
    efforts: Vec<String>,
    default: bool,
    provisional: bool,
) -> Model {
    Model {
        id,
        name,
        efforts,
        aliases: if default {
            vec!["default".into()]
        } else {
            Vec::new()
        },
        speeds: Vec::new(),
        hidden: false,
        retiring: false,
        provisional,
    }
}

fn fallback() -> Models {
    let models = [
        (
            "opus",
            "Opus",
            &["low", "medium", "high", "xhigh", "max"][..],
        ),
        ("sonnet", "Sonnet", &["low", "medium", "high", "max"][..]),
        ("haiku", "Haiku", &[][..]),
    ]
    .into_iter()
    .map(|(id, name, efforts)| {
        model(
            id.into(),
            name.into(),
            efforts.iter().map(|effort| (*effort).into()).collect(),
            id == "opus",
            true,
        )
    })
    .collect();
    Models {
        models,
        ..Models::default()
    }
}
