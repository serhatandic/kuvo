//! Hand approved values to an owned child, and redact output inside the vault.
use std::{
    collections::{HashSet, VecDeque},
    env,
    ffi::OsString,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use kuvo_os::process::{Spawn, Stdio};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;
use zeroize::{Zeroize, Zeroizing};

use super::{
    Error, Hidden, Item, Kind, Result, Secret,
    activity::Action,
    broker::{UseDenied, Where},
    fill::text,
    helper::millis,
    identity::Identity,
    items::one_line,
    mask,
    routes::Resident,
    tools::Answer,
};

const MOST_KEPT: usize = 4 * 1024 * 1024;
const MOST_OUTPUT: usize = 64 * 1024;
const CALL_BUDGET: Duration = Duration::from_secs(15 * 60 + 30);

struct Wanted {
    variable: String,
    item: Item,
    field: String,
}

impl Resident {
    pub(super) async fn run_tool(
        self: &Arc<Self>,
        caller: &Identity,
        args: &Map<String, Value>,
        cancel: &CancellationToken,
    ) -> Result<Answer> {
        let command = text(args, "command").unwrap_or_default().trim();
        let why = one_line(text(args, "why").unwrap_or_default(), 300);
        if command.is_empty() {
            return Ok(Answer::failure("Give the command to run."));
        }
        if why.trim().is_empty() {
            return Ok(Answer::failure("Say why you need these values."));
        }
        let Some(variables) = args
            .get("variables")
            .and_then(Value::as_object)
            .filter(|variables| !variables.is_empty())
        else {
            return Ok(Answer::failure(
                "Name at least one variable and the item it gets.",
            ));
        };

        let started = Instant::now();
        let program = Path::new(command.split_whitespace().next().unwrap_or_default())
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        let place = Where {
            program: Some(program),
            why: Some(why.trim().into()),
            site: None,
        };
        let wanted = self.block(|local| variables.iter().map(|(variable, value)| {
            if !valid_variable(variable) {
                return Err(Error::refused(400, format!("“{}” isn't a variable's name.", one_line(variable, 40))));
            }
            let name = value.as_str().ok_or_else(|| Error::refused(400, format!("Name the item {variable} gets.")))?;
            let (item, field) = if let Some(item) = local.vault.named(name)? {
                (Some(item), None)
            } else if let Some((item, field)) = name.rsplit_once('.').filter(|(item, _)| !item.is_empty()) {
                (local.vault.named(item)?, Some(field.trim().to_lowercase()))
            } else {
                (None, None)
            };
            let item = item.ok_or_else(|| Error::refused(404, format!("There's no item “{}” in the vault. See what you may use with vault_items.", one_line(name, 80))))?;
            if item.sign_in_only == Some(true) {
                return Err(Error::refused(403, format!("“{}” is only for signing in, in the browser: use browser_sign_in. It's never handed to a command.", item.name)));
            }
            let field = field_of(&item, field.as_deref())?.into();
            Ok(Wanted { variable: variable.clone(), item, field })
        }).collect::<Result<Vec<_>>>())?;
        let mut unique = HashSet::new();
        let mut allowed = Vec::new();
        for wanted in &wanted {
            if !unique.insert(&wanted.item.id) {
                continue;
            }
            match self
                .authorize(&wanted.item, caller, &place, true, cancel)
                .await?
            {
                Ok(how) => allowed.push((&wanted.item, how)),
                Err(UseDenied::Refused(message)) => return Ok(Answer::failure(message)),
            }
        }
        let folder = caller
            .folder
            .as_ref()
            .filter(|folder| folder.is_dir())
            .unwrap_or(&self.broker.account);
        let timeout = args
            .get("timeout")
            .and_then(|value| {
                value
                    .as_f64()
                    .or_else(|| value.as_str()?.trim().parse().ok())
            })
            .filter(|n| !n.is_nan() && *n != 0.0)
            .unwrap_or(300.0);
        let left = CALL_BUDGET.saturating_sub(started.elapsed()).as_secs() as f64;
        let seconds = timeout.min(600.0).min(left).max(1.0);
        let (spawn, scratch, hidden, variables) = self.block(|local| {
            let scratch_root = self.broker.home.join("run/vault-run");
            kuvo_store::files::create_private_dirs(&scratch_root).map_err(|error| {
                Error::disk("Couldn't prepare the vault's temporary folder.", error)
            })?;
            let scratch = tempfile::Builder::new()
                .tempdir_in(scratch_root)
                .map_err(|error| {
                    Error::disk("Couldn't prepare the vault's temporary folder.", error)
                })?;
            let mut environment = command_environment();
            let mut hidden = Vec::new();
            let mut credentials = Vec::new();
            for wanted in &wanted {
                let mut values = local.vault.values(&wanted.item.id)?;
                let value = values.remove(&wanted.field).ok_or_else(|| {
                    Error::refused(
                        400,
                        format!("“{}” has no {} kept.", wanted.item.name, wanted.field),
                    )
                })?;
                let env_value = if wanted.item.kind == Kind::File && wanted.field == "file" {
                    let file = prepare_file(scratch.path(), wanted, value, &mut hidden)?;
                    file.into_os_string()
                } else {
                    let env_value = OsString::from(value.expose());
                    hidden.push(Hidden {
                        item: wanted.item.name.clone(),
                        value,
                    });
                    credentials.push(OsString::from(&wanted.variable));
                    env_value
                };
                environment.insert(wanted.variable.clone().into(), env_value);
            }
            for (item, how) in &allowed {
                local.vault.note_used(&item.id, millis())?;
                local.note_agent(
                    Action::Used,
                    Some(item),
                    caller,
                    &place,
                    Some(how.as_str()),
                    None,
                )?;
            }
            let mut spawn = shell(environment, &folder, command)?;
            spawn.stdout = Stdio::Pipe;
            spawn.stderr = Stdio::Pipe;
            Ok((spawn, scratch, hidden, credentials))
        })?;
        let cancel = cancel.clone();
        let output = tokio::task::spawn_blocking(move || {
            let _scratch = scratch;
            let outcome = run(spawn, &variables, Duration::from_secs_f64(seconds), &cancel);
            let answer = match outcome {
                Ok(ended) => ended.answer(&hidden, seconds),
                Err(_) => Answer::failure("The vault command couldn't be started or read."),
            };
            answer
        })
        .await
        .map_err(|_| Error::refused(503, "The vault's command worker stopped."))?;
        Ok(output)
    }
}

/// The user's shell, as a login shell, or Git for Windows' Bash.
fn shell(environment: kuvo_os::platform::Environment, folder: &Path, command: &str) -> Result<Spawn> {
    #[cfg(windows)]
    return kuvo_os::platform::git_bash_spawn(environment, folder, command)
        .map_err(|error| Error::refused(503, error.to_string()));
    #[cfg(unix)]
    {
        let shell = environment
            .get(std::ffi::OsStr::new("SHELL"))
            .map(PathBuf::from)
            .filter(|file| file.is_absolute() && file.is_file())
            .unwrap_or_else(|| PathBuf::from("/bin/sh"));
        let mut spawn = Spawn::new(shell, environment);
        spawn.args = vec!["-lc".into(), command.into()];
        spawn.cwd = Some(folder.to_owned());
        Ok(spawn)
    }
}

fn valid_variable(name: &str) -> bool {
    name.len() <= 64
        && name
            .as_bytes()
            .first()
            .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn field_of<'a>(item: &'a Item, requested: Option<&'a str>) -> Result<&'a str> {
    if let Some(field) = requested.filter(|field| !field.is_empty()) {
        if item.fields.iter().any(|each| each == field) {
            return Ok(field);
        }
        let fields = if item.fields.is_empty() {
            String::new()
        } else {
            format!(": it has {}", item.fields.join(", "))
        };
        return Err(Error::refused(
            400,
            format!("“{}” has no field “{field}”{fields}.", item.name),
        ));
    }
    let field = match item.kind {
        Kind::Login => "password",
        Kind::File => "file",
        Kind::Note => "text",
        Kind::Key if item.fields.len() == 1 => &item.fields[0],
        Kind::Key => "value",
    };
    if item.fields.iter().any(|each| each == field) {
        return Ok(field);
    }
    let fields = if item.fields.is_empty() {
        "it has none".into()
    } else {
        item.fields.join(", ")
    };
    Err(Error::refused(
        400,
        format!(
            "Say which of “{}”'s fields: {fields} (`{}.<field>`).",
            item.name, item.name
        ),
    ))
}

fn prepare_file(
    scratch: &Path,
    wanted: &Wanted,
    value: Secret,
    hidden: &mut Vec<Hidden>,
) -> Result<PathBuf> {
    let basename = wanted
        .item
        .file_name
        .as_deref()
        .and_then(|name| Path::new(name).file_name())
        .and_then(|name| name.to_str());
    let filename = basename
        .filter(|name| !name.is_empty() && *name != "." && *name != "..")
        .map_or_else(|| wanted.variable.to_lowercase(), str::to_owned);
    let file = scratch.join(&wanted.variable).join(filename);
    let bytes = Zeroizing::new(super::crypto::decode_base64(value.expose()));
    kuvo_store::files::write_secret(&file, &bytes)
        .map_err(|error| Error::disk("Couldn't prepare the vault's file.", error))?;
    let encoded = Zeroizing::new(STANDARD.encode(&bytes));
    let wrap = |size| {
        encoded
            .as_bytes()
            .chunks(size)
            .map(|chunk| std::str::from_utf8(chunk).expect("base64 is ASCII"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    for value in [
        String::from_utf8_lossy(&bytes).into_owned(),
        encoded.to_string(),
        wrap(76),
        wrap(64),
        hex::encode(&bytes),
    ] {
        hidden.push(Hidden {
            item: wanted.item.name.clone(),
            value: Secret::new(value),
        });
    }
    Ok(file)
}

pub(crate) fn command_environment() -> kuvo_os::platform::Environment {
    let original: kuvo_os::platform::Environment = env::vars_os().collect();
    let mut environment = original
        .iter()
        .filter(|(key, _)| {
            let key = key.to_string_lossy();
            !key.starts_with("KUVO_")
                && !key.starts_with("OPENCODE_")
                && !matches!(
                    key.as_ref(),
                    "BUN_BE_BUN" | "BUN_RUNTIME_TRANSPILER_CACHE_PATH"
                )
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<kuvo_os::platform::Environment>();
    for (key, value) in &original {
        if let Some(name) = key
            .to_str()
            .and_then(|key| key.strip_prefix("KUVO_OWN_"))
            .filter(|name| !name.is_empty())
        {
            if value.is_empty() {
                environment.remove(std::ffi::OsStr::new(name));
            } else {
                environment.insert(name.into(), value.clone());
            }
        }
    }
    environment
}

struct Ended {
    output: Zeroizing<Tail>,
    code: Option<i32>,
    timed_out: bool,
}

impl Ended {
    fn answer(mut self, hidden: &[Hidden], seconds: f64) -> Answer {
        let text = String::from_utf8_lossy(self.output.0.make_contiguous()).into_owned();
        let masked = mask(text, hidden);
        let tail = utf16_tail(&masked, MOST_OUTPUT);
        let shown = if tail.len() != masked.len() {
            let rest = tail.split_once('\n').map_or(tail, |(_, rest)| rest);
            format!("…{rest}")
        } else {
            masked
        };
        let ended = if self.timed_out {
            format!("It was stopped after {seconds} seconds.")
        } else {
            format!(
                "It ended with {}.",
                self.code
                    .map_or_else(|| "null".into(), |code| code.to_string())
            )
        };
        let head = if shown.trim().is_empty() {
            "It printed nothing."
        } else {
            shown.trim_end()
        };
        Answer {
            text: format!(
                "{head}\n\n{ended} (Values from the vault are shown as [vault: <item>].)"
            ),
            is_error: self.timed_out || self.code != Some(0),
        }
    }
}

fn utf16_tail(text: &str, maximum: usize) -> &str {
    let mut units = 0;
    for (index, character) in text.char_indices().rev() {
        units += character.len_utf16();
        if units > maximum {
            return &text[index + character.len_utf8()..];
        }
    }
    text
}

fn run(
    mut spawn: Spawn,
    credentials: &[OsString],
    timeout: Duration,
    cancel: &CancellationToken,
) -> io::Result<Ended> {
    let child = spawn.spawn();
    for name in credentials {
        if let Some(value) = spawn.env.remove(name) {
            let mut bytes = value.into_encoded_bytes();
            bytes.zeroize();
        }
    }
    let mut child = child?;
    let joined = Arc::new(Mutex::new(Zeroizing::new(Tail(VecDeque::new()))));
    let read = |pipe: std::io::PipeReader| {
        let joined = joined.clone();
        thread::spawn(move || take_output(pipe, &joined))
    };
    let stdout = read(child.stdout.take().expect("vault command stdout"));
    let stderr = read(child.stderr.take().expect("vault command stderr"));
    let deadline = Instant::now() + timeout;
    let (status, timed_out) = loop {
        if child.has_exited()? {
            break (child.wait()?, false);
        }
        let timed_out = Instant::now() >= deadline;
        if timed_out || cancel.is_cancelled() {
            let status = child.stop(Duration::from_secs(3), |_| {})?;
            break (status, timed_out);
        }
        thread::sleep(Duration::from_millis(10));
    };
    for handle in [stdout, stderr] {
        let _ = handle.join();
    }
    let output = Arc::try_unwrap(joined)
        .map_err(|_| io::Error::other("command output still in use"))?
        .into_inner()
        .map_err(|_| io::Error::other("command output is unavailable"))?;
    Ok(Ended {
        output,
        code: status.code(),
        timed_out,
    })
}

fn take_output(mut pipe: impl Read, joined: &Mutex<Zeroizing<Tail>>) {
    let mut bytes = Zeroizing::new([0; 8192]);
    while let Ok(count) = pipe.read(bytes.as_mut()) {
        if count == 0 {
            break;
        }
        let Ok(mut kept) = joined.lock() else { break };
        let kept = &mut kept.0;
        let over = (kept.len() + count).saturating_sub(MOST_KEPT);
        for byte in kept.range_mut(..over) {
            byte.zeroize();
        }
        kept.drain(..over);
        kept.extend(&bytes[..count]);
    }
}

struct Tail(VecDeque<u8>);

impl Zeroize for Tail {
    fn zeroize(&mut self) {
        let (start, end) = self.0.as_mut_slices();
        start.zeroize();
        end.zeroize();
    }
}
