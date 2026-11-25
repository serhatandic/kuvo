use std::{
    collections::HashMap,
    io::{self, BufRead, BufReader, Write},
    path::PathBuf,
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
    path: PathBuf,
    feed: Option<crate::feed::Feed>,
}

#[derive(Default)]
struct Cache {
    generation: u64,
    identity: Option<String>,
    initialized: bool,
    loading: Option<Loading>,
    models: Option<Models>,
    /// How much each model holds, as Claude Code said: by its value (`default`, `opus`, or an id).
    heard: HashMap<String, u64>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Saved {
    identity: Option<String>,
    models: Models,
    heard: HashMap<String, u64>,
}

impl Catalog {
    pub fn new(path: PathBuf, feed: Option<crate::feed::Feed>) -> io::Result<Self> {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).map_err(io::Error::other)?;
        Ok(Self {
            prefix: hex::encode(bytes).into(),
            cache: Arc::default(),
            path,
            feed,
        })
    }

    fn stamp(&self, mut models: Models, generation: u64) -> Models {
        models.generation = Some(format!("{}-{generation}", self.prefix));
        models
    }

    /// How much `model` holds, as the account's listed models say (none while they're a guess).
    pub async fn window(&self, model: &str) -> Option<u64> {
        let cache = self.cache.lock().await;
        cache
            .models
            .as_ref()?
            .models
            .iter()
            .find(|each| {
                !each.provisional
                    && (each.id == model || each.aliases.iter().any(|alias| alias == model))
            })?
            .context
    }

    /// How much `model` holds, as Claude Code said (its `/context` as the models were listed, or a
    /// turn's result since): by its value, or as the model a listed alias stands for.
    pub async fn heard_window(&self, model: &str) -> Option<u64> {
        let cache = self.cache.lock().await;
        cache.heard.get(model).copied().or_else(|| {
            let listed = cache.models.as_ref()?.models.iter();
            let named = listed
                .filter(|each| each.aliases.iter().any(|alias| alias == model))
                .map(|each| each.id.as_str())
                .next()
                .unwrap_or(model);
            cache.heard.get(named).copied()
        })
    }

    /// What Claude Code said `model` holds, as a turn ended.
    pub async fn note(&self, model: &str, size: u64) {
        if size > 0 {
            let mut cache = self.cache.lock().await;
            if cache.heard.insert(model.to_owned(), size) != Some(size) {
                self.save(&cache).await;
            }
        }
    }

    pub async fn clear(&self) {
        let mut cache = self.cache.lock().await;
        *cache = Cache {
            generation: cache.generation + 1,
            ..Cache::default()
        };
        let _ = tokio::fs::remove_file(&self.path).await;
    }

    async fn save(&self, cache: &Cache) {
        let Some(models) = cache.models.clone() else {
            return;
        };
        let Ok(bytes) = serde_json::to_vec(&Saved {
            identity: cache.identity.clone(),
            models,
            heard: cache.heard.clone(),
        }) else {
            return;
        };
        let path = self.path.clone();
        let _ = tokio::task::spawn_blocking(move || kuvo_store::files::write_atomic(&path, &bytes))
            .await;
    }

    async fn prepare(&self, program: &Program) {
        let identity = program.sign_in_mark().await;
        let mut cache = self.cache.lock().await;
        if cache.initialized && cache.identity == identity {
            return;
        }
        *cache = Cache {
            generation: cache.generation + 1,
            identity: identity.clone(),
            initialized: true,
            ..Cache::default()
        };
        if let Ok(bytes) = tokio::fs::read(&self.path).await
            && let Ok(saved) = serde_json::from_slice::<Saved>(&bytes)
            && saved.identity == identity
        {
            cache.models = Some(self.stamp(saved.models, cache.generation));
            cache.heard = saved.heard;
        }
    }

    pub async fn read(&self, program: Program, _refresh: bool) -> io::Result<Models> {
        self.prepare(&program).await;
        let cache = self.cache.lock().await;
        Ok(cache
            .models
            .clone()
            .unwrap_or_else(|| self.stamp(fallback(), cache.generation)))
    }

    /// A real turn or sign-in, not a read, can discover models and their actual windows.
    pub async fn refresh(&self, program: Program) -> io::Result<Models> {
        self.prepare(&program).await;
        loop {
            let (generation, loading) = {
                let mut cache = self.cache.lock().await;
                if let Some(models) = &cache.models {
                    return Ok(models.clone());
                }
                let generation = cache.generation;
                let loading = cache
                    .loading
                    .get_or_insert_with(|| {
                        let catalog = self.clone();
                        let program = program.clone();
                        let task = async move {
                            let result = load(program).await.map_err(Arc::new);
                            let mut cache = catalog.cache.lock().await;
                            if generation == cache.generation {
                                cache.loading = None;
                                if let Ok((models, heard)) = &result {
                                    cache.models = Some(catalog.stamp(models.clone(), generation));
                                    cache
                                        .heard
                                        .extend(heard.iter().map(|(k, v)| (k.clone(), *v)));
                                    catalog.save(&cache).await;
                                    if let Some(feed) = &catalog.feed {
                                        feed.emit("model.updated", json!({"program":"claude"}));
                                    }
                                }
                            }
                            result.map(|(models, _)| catalog.stamp(models, generation))
                        }
                        .boxed()
                        .shared();
                        tokio::spawn(task.clone());
                        task
                    })
                    .clone();
                (generation, loading)
            };
            let answer = loading.await;
            if self.cache.lock().await.generation == generation {
                return answer.map_err(|error| io::Error::new(error.kind(), error));
            }
            // The sign-in changed while this probe ran. It belongs to that old turn.
            return Err(io::Error::other(
                "Claude's sign-in changed while listing models.",
            ));
        }
    }
}

/// The account's models, and how much each holds as Claude Code said.
async fn load(program: Program) -> io::Result<(Models, HashMap<String, u64>)> {
    let (list, windows) = tokio::task::spawn_blocking(move || probe(program)).await??;
    let models = Models {
        complete: true,
        models: named_models(list, &windows),
        ..Models::default()
    };
    Ok((models, windows))
}

/// Claude Code's models on this account, as it lists them, and how much each holds, as its
/// `/context` says once a model is chosen (an older Claude Code that doesn't say leaves them to its
/// list's word).
fn probe(program: Program) -> io::Result<(Vec<Info>, HashMap<String, u64>)> {
    ask_native(program, true, Asking::models)
}

/// A plan read asks Claude itself; it never extracts the Mac's OAuth token.
pub(super) async fn usage(program: Program) -> io::Result<Value> {
    tokio::task::spawn_blocking(move || {
        ask_native(program, false, |asking| {
            if !matches!(
                asking.ask(
                    "usage-init",
                    json!({"subtype":"initialize"}),
                    Duration::from_secs(20)
                )?,
                wire::ReplyBody::Success { .. }
            ) {
                return Err(io::Error::other(
                    "Claude Code didn't initialize for its plan reading.",
                ));
            }
            match asking.ask(
                "usage",
                json!({"subtype":"get_usage","skip_behaviors":true}),
                Duration::from_secs(20),
            )? {
                wire::ReplyBody::Success { response, .. } => Ok(response),
                _ => Err(io::Error::other("Claude Code couldn't say its plan usage.")),
            }
        })
    })
    .await?
}
fn ask_native<T>(
    program: Program,
    legacy: bool,
    call: impl FnOnce(&mut Asking) -> io::Result<T>,
) -> io::Result<T> {
    let mut env = program.environment(legacy);
    if !legacy {
        env.insert("DISABLE_AUTOUPDATER".into(), "1".into());
    }
    let executable = program
        .executable(&env)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Claude Code isn't installed."))?;
    // A file rather than JSON on the command line: a Claude Code installed as a `.cmd` script
    // can't be given quotes.
    let settings = program.scratch().join("claude/catalog-settings.json");
    kuvo_store::files::write_atomic(&settings, br#"{"disableAllHooks":true}"#)?;
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
    ]
    .into_iter()
    .map(Into::into)
    .chain([settings.into_os_string()])
    .collect();
    let mut child = spawn.spawn()?;
    let stdin = child.stdin.take().expect("requested stdin");
    let stdout = child.stdout.take().expect("requested stdout");
    let (send, replies) = mpsc::sync_channel(16);
    let reader = thread::Builder::new()
        .name("claude-models".into())
        .spawn(move || read_replies(BufReader::new(stdout), &send))?;
    let mut asking = Asking { stdin, replies };
    let result = call(&mut asking);
    drop(asking);
    child.stop(Duration::from_secs(1), |_| {})?;
    let _ = reader.join();
    result
}

/// The probe's requests to its Claude Code, each with its answer.
struct Asking {
    stdin: std::io::PipeWriter,
    replies: mpsc::Receiver<(String, wire::ReplyBody)>,
}

impl Asking {
    fn ask(&mut self, id: &str, request: Value, within: Duration) -> io::Result<wire::ReplyBody> {
        writeln!(self.stdin, "{}", wire::control_request(id, request))?;
        let deadline = std::time::Instant::now() + within;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match self.replies.recv_timeout(left) {
                Ok((answered, body)) if answered == id => return Ok(body),
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "Claude Code didn't answer in time.",
                    ));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::other(
                        "Claude Code stopped before listing its models.",
                    ));
                }
            }
        }
    }

    fn models(&mut self) -> io::Result<(Vec<Info>, HashMap<String, u64>)> {
        let wire::ReplyBody::Success { response, .. } = self.ask(
            "catalog",
            json!({"subtype":"initialize"}),
            Duration::from_secs(30),
        )?
        else {
            return Err(io::Error::other("Claude Code refused to list its models."));
        };
        let list: Vec<Info> =
            serde_json::from_value(response.get("models").cloned().unwrap_or(Value::Null))
                .map_err(|_| io::Error::other("Claude Code didn’t list its models."))?;
        if list.is_empty() {
            return Err(io::Error::other("Claude Code didn’t list its models."));
        }
        let windows = self.windows(&list);
        Ok((list, windows))
    }

    /// Each model's window, once per model (its aliases share it); none past one it can't say.
    fn windows(&mut self, list: &[Info]) -> HashMap<String, u64> {
        let within = Duration::from_secs(5);
        let mut said: HashMap<String, u64> = HashMap::new();
        let mut windows = HashMap::new();
        for (index, info) in list.iter().enumerate() {
            let key = info
                .resolved_model
                .clone()
                .unwrap_or_else(|| info.value.clone());
            let window = match said.get(&key) {
                Some(window) => *window,
                None => {
                    let chosen = self.ask(
                        &format!("model-{index}"),
                        json!({"subtype":"set_model","model":info.value}),
                        within,
                    );
                    if !matches!(chosen, Ok(wire::ReplyBody::Success { .. })) {
                        return windows;
                    }
                    let Ok(wire::ReplyBody::Success { response, .. }) = self.ask(
                        &format!("context-{index}"),
                        json!({"subtype":"get_context_usage","detail":"summary"}),
                        within,
                    ) else {
                        return windows;
                    };
                    let Some(window) = response["maxTokens"].as_u64().filter(|window| *window > 0)
                    else {
                        return windows;
                    };
                    said.insert(key, window);
                    window
                }
            };
            windows.insert(info.value.clone(), window);
        }
        windows
    }
}

/// Each control response Claude Code writes, until it stops (a catalog is short: an unbounded
/// stream from a broken CLI isn't taken).
fn read_replies(stdout: impl BufRead, send: &mpsc::SyncSender<(String, wire::ReplyBody)>) {
    let mut stdout = stdout.take(8_000_001);
    let mut line = String::new();
    loop {
        line.clear();
        if !matches!(stdout.read_line(&mut line), Ok(read) if read > 0) || !line.ends_with('\n') {
            return;
        }
        if let Ok(wire::Frame::ControlResponse { response }) =
            serde_json::from_str::<wire::Frame>(&line)
            && send.send((response.request_id, response.body)).is_err()
        {
            return;
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

    /// How much it holds: as Claude Code said, or as its list says (a million where it says 1M).
    fn window(&self, windows: &HashMap<String, u64>) -> u64 {
        windows.get(&self.value).copied().unwrap_or_else(|| {
            if self
                .description
                .as_deref()
                .is_some_and(|text| text.to_lowercase().contains("1m"))
            {
                1_000_000
            } else {
                200_000
            }
        })
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

fn named_models(list: Vec<Info>, windows: &HashMap<String, u64>) -> Vec<Model> {
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
                let context = info.window(windows);
                return Some((
                    index,
                    model(
                        info.resolved_model.unwrap_or(info.value),
                        name,
                        info.supported_effort_levels,
                        concrete,
                        false,
                    )
                    .holding(context),
                ));
            }
            let place = default
                .filter(|_| Some(index) == equivalent)
                .unwrap_or(index);
            let name = info.name().to_owned();
            let context = info.window(windows);
            Some((
                place,
                model(
                    info.value,
                    name,
                    info.supported_effort_levels,
                    Some(index) == equivalent,
                    false,
                )
                .holding(context),
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
        context: None,
    }
}

trait Holding {
    fn holding(self, context: u64) -> Self;
}

impl Holding for Model {
    fn holding(mut self, context: u64) -> Self {
        self.context = Some(context);
        self
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
