//! Read GitHub using the configured GitHub CLI account.

use kuvo_os::platform::{self, Environment};
use kuvo_store::format::Fields;
use serde_json::Value;
use std::{collections::HashSet, ffi::OsStr, io, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    time::timeout,
};

const MAX_BYTES: usize = 8 * 1024 * 1024;
const TIME: Duration = Duration::from_secs(30);

pub(super) struct Answered {
    pub data: Fields,
    pub held: HashSet<String>,
    pub said: Option<String>,
}

#[derive(Clone, Copy)]
pub(super) enum Unread {
    Missing,
    SignedOut,
    Failed,
}

impl Unread {
    pub fn words(self, computer: &str) -> String {
        match self {
            Self::Missing => format!(
                "Install GitHub CLI on {computer}, then open Settings → Accounts → GitHub for Work Queue."
            ),
            Self::SignedOut => {
                format!("On {computer}, sign in under Settings → Accounts → GitHub for Work Queue.")
            }
            Self::Failed => "gh couldn't reach GitHub just now: try again in a moment.".into(),
        }
    }
}

async fn collect(pipe: impl AsyncRead + Unpin) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    pipe.take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() > MAX_BYTES {
        return Err(io::Error::other("GitHub CLI output is too large"));
    }
    Ok(bytes)
}

pub(super) fn gh(args: &[&str]) -> Result<Command, Unread> {
    let env: Environment = std::env::vars_os().collect();
    let gh = platform::which(OsStr::new("gh"), &env, &[]).ok_or(Unread::Missing)?;
    let mut process = Command::new(gh.path());
    process.args(args);
    process.current_dir(std::env::temp_dir());
    process.envs([
        ("GH_PROMPT_DISABLED", "1"),
        ("GH_NO_UPDATE_NOTIFIER", "1"),
        ("NO_COLOR", "1"),
    ]);
    process
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    process.kill_on_drop(true);
    Ok(process)
}

pub(super) async fn ask(host: &str, query: String) -> Result<Answered, Unread> {
    let rendered = format!("query={query}");
    let mut process = gh(&["api", "graphql", "--hostname", host, "-f", &rendered])?;
    let mut child = process.spawn().map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            Unread::Missing
        } else {
            Unread::Failed
        }
    })?;
    let out = child.stdout.take().expect("GitHub output pipe");
    let err = child.stderr.take().expect("GitHub errors pipe");
    let (out, err) = timeout(TIME, async {
        let (out, err, _) = tokio::try_join!(collect(out), collect(err), child.wait())?;
        Ok::<_, io::Error>((out, err))
    })
    .await
    .map_err(|_| Unread::Failed)?
    .map_err(|_| Unread::Failed)?;
    if let Ok(json) = serde_json::from_slice::<Value>(&out)
        && let Some(data) = json["data"].as_object()
    {
        return Ok(answer_of(data.clone(), &json["errors"]));
    }
    let message = String::from_utf8_lossy(&err).to_ascii_lowercase();
    if ["auth login", "not logged in", "authentication", "http 401"]
        .iter()
        .any(|clue| message.contains(clue))
    {
        Err(Unread::SignedOut)
    } else {
        Err(Unread::Failed)
    }
}

fn answer_of(data: Fields, errors: &Value) -> Answered {
    let mut held = HashSet::new();
    let mut missing = HashSet::new();
    let mut all = false;
    let mut said = None;
    for error in errors
        .as_array()
        .into_iter()
        .flatten()
        .filter(|error| error.is_object())
    {
        let alias = error["path"]
            .as_array()
            .and_then(|path| path.first())
            .and_then(Value::as_str);
        if error["type"] == "NOT_FOUND" {
            if let Some(alias) = alias {
                missing.insert(alias);
            }
            continue;
        }
        if said.is_none() {
            said = error["message"]
                .as_str()
                .map(|line| super::super::store::limit(line, 300));
        }
        if let Some(alias) = alias {
            held.insert(alias.into());
        } else {
            all = true;
        }
    }
    for (alias, value) in &data {
        if all || (value.is_null() && !missing.contains(alias.as_str())) {
            held.insert(alias.clone());
        }
    }
    Answered { data, held, said }
}
