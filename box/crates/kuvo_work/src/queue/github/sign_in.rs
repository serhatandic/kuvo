//! GitHub CLI's device-code attempt, owned by the client that starts it.

use super::command;
use crate::queue::routes::{internal, local_work_queue_app};
use http::{HeaderMap, StatusCode};
use kuvo_server::{Caller, Refusal};
use regex::Regex;
use serde_json::{Value, json};
use std::{path::PathBuf, process::Stdio, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    sync::{Mutex, oneshot, watch},
    time::{Instant, timeout},
};

const INTEGRATION: &str = "github-work-queue";
const PAGE: &str = "https://github.com/login/device";
const INSTRUCTIONS: &str = "Open GitHub, sign in, and enter the code shown here.";
const LIFETIME: Duration = Duration::from_secs(900);

#[derive(Clone)]
pub(in crate::queue) struct SignIn {
    temporary: PathBuf,
    directory: PathBuf,
    attempt: Arc<Mutex<Option<Attempt>>>,
    account: Arc<Mutex<Option<(Instant, bool)>>>,
    pattern: Arc<Regex>,
}

struct Attempt {
    id: String,
    owner: String,
    answer: Value,
    state: watch::Receiver<Progress>,
    _cancel: oneshot::Sender<()>,
}

#[derive(Clone)]
struct Progress {
    state: State,
    code: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Pending,
    Complete,
    Failed,
    Expired,
}

impl SignIn {
    pub fn register(&self, registry: &mut kuvo_server::Registry) {
        use crate::queue::routes::typed;
        use kuvo_protocol::routes::*;
        use kuvo_server::{Access, Refusal};

        let service = self.clone();
        registry.register::<get_api_integration_github_work_queue::Spec, _, _>(
            Access::Any,
            move |_| {
                let service = service.clone();
                async move { typed(service.located(service.integration().await)) }
            },
        );

        let service = self.clone();
        registry.register::<post_api_integration_github_work_queue_connect_oauth::Spec, _, _>(
            Access::Any,
            move |call| {
                let service = service.clone();
                async move {
                    let owner = asking(&call.request.caller, &call.request.headers)?;
                    let body: Value =
                        serde_json::from_slice(&call.request.body).unwrap_or_default();
                    if body["methodID"] != "device" {
                        return Err(Refusal::new(
                            StatusCode::BAD_REQUEST,
                            "GitHub sign-in didn’t answer. Try again.",
                        ));
                    }
                    let answer = service
                        .start(owner)
                        .await
                        .map_err(|error| wrap(error, "GitHub sign-in didn’t answer. Try again."))?;
                    typed(service.located(answer))
                }
            },
        );

        let service = self.clone();
        registry
            .register::<get_api_integration_github_work_queue_connect_oauth_attempt::Spec, _, _>(
                Access::Any,
                move |call| {
                    let service = service.clone();
                    async move {
                        let owner = asking(&call.request.caller, &call.request.headers)?;
                        let attempt = attempt_id(call.param("attempt"))?;
                        typed(service.located(service.status(&attempt, &owner).await))
                    }
                },
            );

        let service = self.clone();
        registry
            .register::<delete_api_integration_github_work_queue_connect_oauth_attempt::Spec, _, _>(
                Access::Any,
                move |call| {
                    let service = service.clone();
                    async move {
                        let owner = asking(&call.request.caller, &call.request.headers)?;
                        let attempt = attempt_id(call.param("attempt"))?;
                        service.cancel(&attempt, &owner).await;
                        typed(json!({"data":{}}))
                    }
                },
            );

        let service = self.clone();
        registry.register::<get_kuvo_sign_in_methods::Spec, _, _>(Access::Any, move |call| {
            let service = service.clone();
            async move {
                require_integration(&call.request.path)?;
                if service.integration().await.is_null() {
                    return Err(Refusal::new(StatusCode::NOT_FOUND, "Install GitHub CLI on this computer to sign in for Work Queue."));
                }
                typed(json!({"methods":[{"type":"oauth","id":"device","label":"Sign in with GitHub"}],"environment":[],"hint":"Sign in with GitHub"}))
            }
        });

        let service = self.clone();
        registry.register::<post_kuvo_sign_in_start::Spec, _, _>(Access::Any, move |call| {
            let service = service.clone();
            async move {
                require_integration(&call.request.path)?;
                let owner = asking(&call.request.caller, &call.request.headers)?;
                let body: Value = serde_json::from_slice(&call.request.body).unwrap_or_default();
                if body["methodID"] != "device" {
                    return Err(Refusal::new(
                        StatusCode::BAD_REQUEST,
                        "The sign-in couldn’t be started. Try again.",
                    ));
                }
                typed(
                    service.start(owner).await.map_err(|error| {
                        wrap(error, "The sign-in couldn’t be started. Try again.")
                    })?,
                )
            }
        });

        let service = self.clone();
        registry.register::<get_kuvo_sign_in_status::Spec, _, _>(Access::Any, move |call| {
            let service = service.clone();
            async move {
                require_integration(&call.request.path)?;
                let owner = asking(&call.request.caller, &call.request.headers)?;
                let attempt = attempt_id(query(&call.request.path, "attempt").as_deref())?;
                let wait = query(&call.request.path, "wait")
                    .and_then(|seconds| seconds.parse::<u64>().ok())
                    .unwrap_or(0)
                    .min(25);
                let mut progress = service.status(&attempt, &owner).await;
                let first = progress.clone();
                let expires = Instant::now() + Duration::from_secs(wait);
                while progress["status"] == "pending"
                    && progress == first
                    && Instant::now() < expires
                {
                    tokio::select! {
                        _ = call.cancel.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_secs(1)) => ()
                    }
                    progress = service.status(&attempt, &owner).await;
                }
                if progress["status"] == "expired" {
                    progress["message"] = "The sign-in timed out. Try again.".into();
                }
                if let Some(message) = progress.get("message").cloned() {
                    progress["failure"] = message;
                }
                typed(progress)
            }
        });

        let service = self.clone();
        registry.register::<post_kuvo_sign_in_action::Spec, _, _>(Access::Any, move |call| {
            let service = service.clone();
            async move {
                require_integration(&call.request.path)?;
                let body: Value = serde_json::from_slice(&call.request.body).unwrap_or_default();
                if body["action"] != "cancel" {
                    return Err(Refusal::new(
                        StatusCode::BAD_REQUEST,
                        "Name the sign-in action and what it needs.",
                    ));
                }
                let owner = asking(&call.request.caller, &call.request.headers)?;
                let attempt = attempt_id(body["attempt"].as_str())?;
                service.cancel(&attempt, &owner).await;
                typed(json!({"data":{}}))
            }
        });
    }

    pub fn new(temporary: PathBuf, directory: PathBuf) -> Self {
        Self {
            temporary,
            directory,
            attempt: Arc::default(),
            account: Arc::default(),
            pattern: Arc::new(
                Regex::new(r"(?i)One-time code\s*:\s*([A-Z0-9]{4}-[A-Z0-9]{4})")
                    .expect("device code"),
            ),
        }
    }

    pub fn located(&self, data: Value) -> Value {
        json!({"location":{"directory":self.directory},"data":data})
    }

    pub async fn integration(&self) -> Value {
        let Ok(mut gh) = command::gh(&["auth", "status", "--hostname", "github.com"]) else {
            return Value::Null;
        };
        gh.current_dir(&self.temporary)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut cache = self.account.lock().await;
        let now = Instant::now();
        let signed_in = if let Some((until, found)) = *cache
            && now < until
        {
            found
        } else {
            let connected = timeout(Duration::from_secs(5), gh.status())
                .await
                .ok()
                .and_then(Result::ok)
                .is_some_and(|result| result.success());
            *cache = Some((now + Duration::from_secs(30), connected));
            connected
        };
        json!({
            "id":INTEGRATION,"name":"GitHub for Work Queue","metadata":{"source":"work-queue"},
            "methods":[{"type":"oauth","id":"device","label":"Sign in with GitHub"}],
            "connections":if signed_in { json!([{"type":"external","method":"oauth","status":{"status":"connected","message":"Signed in to GitHub"}}]) } else { json!([]) },
        })
    }

    pub async fn start(&self, owner: String) -> Result<Value, Refusal> {
        let mut current = self.attempt.lock().await;
        if let Some(pending) = current
            .as_ref()
            .filter(|attempt| attempt.state.borrow().state == State::Pending)
        {
            if pending.owner != owner {
                return Err(Refusal::new(StatusCode::CONFLICT, BUSY));
            }
            return Ok(pending.answer.clone());
        }
        let mut gh = command::gh(&["auth","login","--hostname","github.com","--web"])
            .map_err(|_| unavailable("Install GitHub CLI on the computer that runs the Work Queue, then try again in Settings → Accounts."))?;
        for name in ["GH_TOKEN", "GITHUB_TOKEN"] {
            if std::env::var_os(name).is_some_and(|value| !value.is_empty()) {
                return Err(unavailable(
                    "The Work Queue uses a GitHub token set on this computer. Remove that setting before signing in here.",
                ));
            }
        }
        current.take();
        let temporary = self.temporary.clone();
        let folder = tokio::task::spawn_blocking(move || {
            kuvo_store::files::create_private_dirs(&temporary)?;
            tempfile::Builder::new()
                .prefix(&format!("kuvo-signin-{}-", std::process::id()))
                .tempdir_in(temporary)
        })
        .await
        .map_err(internal)?
        .map_err(internal)?;
        let no_browser = no_browser(folder.path()).map_err(internal)?;
        gh.env("BROWSER", &no_browser).stdin(Stdio::piped());
        let mut child = gh
            .spawn()
            .map_err(|_| unavailable("The sign-in program couldn’t start. Try again."))?;
        let input = child.stdin.take();
        let output = child.stdout.take().expect("GitHub output");
        let errors = child.stderr.take().expect("GitHub errors");
        let (progress, mut state) = watch::channel(Progress {
            state: State::Pending,
            code: None,
        });
        let (cancel, mut canceled) = oneshot::channel();
        let pattern = self.pattern.clone();
        let account = self.account.clone();
        tokio::spawn(async move {
            let _folder = folder;
            let _input = input;
            let (found, mut text) = tokio::sync::mpsc::channel(4);
            let a = tokio::spawn(read(output, found.clone()));
            let b = tokio::spawn(read(errors, found));
            let mut buffer = String::new();
            let end = Instant::now() + LIFETIME;
            let mut reading = true;
            loop {
                tokio::select! {
                    _ = &mut canceled => break,
                    _ = tokio::time::sleep_until(end) => {
                        progress.send_modify(|value| value.state = State::Expired);
                        break;
                    }
                    line = text.recv(), if reading => if let Some(line) = line {
                        if progress.borrow().code.is_none() {
                            buffer.push_str(&line);
                            if let Some(code) = pattern.captures(&buffer).and_then(|found| found.get(1)) {
                                progress.send_modify(|value| value.code = Some(code.as_str().into()));
                            }
                            if buffer.len() > 8192 {
                                buffer.clear();
                            }
                        }
                    } else { reading = false; },
                    result = child.wait() => {
                        let connected = result.is_ok_and(|status| status.success());
                        if connected { *account.lock().await = None; }
                        progress.send_modify(|value| value.state = if connected { State::Complete } else { State::Failed });
                        break;
                    }
                }
            }
            a.abort();
            b.abort();
        });
        let _ = timeout(
            Duration::from_secs(7),
            state.wait_for(|value| value.code.is_some() || value.state != State::Pending),
        )
        .await;
        let code = state.borrow().code.clone();
        let mut random = [0; 15];
        getrandom::fill(&mut random).map_err(internal)?;
        let nonce: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let id = format!("att_{nonce}");
        let created = jiff::Timestamp::now().as_millisecond();
        let mut answer = json!({"attemptID":id,"url":if code.is_some() { PAGE } else { "" },"instructions":INSTRUCTIONS,
            "mode":"auto","time":{"created":created,"expires":created + 900_000}});
        if let Some(code) = code {
            answer["userCode"] = code.into();
        }
        *current = Some(Attempt {
            id,
            owner,
            answer: answer.clone(),
            state,
            _cancel: cancel,
        });
        Ok(answer)
    }

    pub async fn status(&self, id: &str, owner: &str) -> Value {
        let current = self.attempt.lock().await;
        let Some(attempt) = current
            .as_ref()
            .filter(|attempt| attempt.id == id && attempt.owner == owner)
        else {
            return json!({"status":"expired"});
        };
        let progress = attempt.state.borrow();
        match progress.state {
            State::Pending => progress.code.as_ref().map_or_else(
                || json!({"status":"pending"}),
                |code| json!({"status":"pending","userCode":code,"url":PAGE}),
            ),
            State::Complete => json!({"status":"complete"}),
            State::Failed => {
                json!({"status":"failed","message":"GitHub didn’t finish signing in. Start again for a new code."})
            }
            State::Expired => json!({"status":"expired"}),
        }
    }

    pub async fn cancel(&self, id: &str, owner: &str) {
        let mut current = self.attempt.lock().await;
        if current
            .as_ref()
            .is_some_and(|attempt| attempt.id == id && attempt.owner == owner)
        {
            current.take();
        }
    }
}

fn unavailable(message: &str) -> Refusal {
    Refusal::new(StatusCode::BAD_GATEWAY, message)
}

fn owner(caller: &Caller, headers: &HeaderMap) -> Option<String> {
    if let Some(pairing) = caller.pairing() {
        let key: String = pairing
            .key
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        return Some(format!("key:{key}"));
    }
    if let Some(client) = headers
        .get("x-kuvo-sign-in-client")
        .and_then(|value| value.to_str().ok())
        .filter(|id| {
            id.len() == 32
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
    {
        return Some(format!("client:{client}"));
    }
    local_work_queue_app(caller).then(|| "local".to_owned())
}

async fn read(mut pipe: impl AsyncRead + Unpin, lines: tokio::sync::mpsc::Sender<String>) {
    let mut buffer = [0u8; 4096];
    while let Ok(count) = pipe.read(&mut buffer).await {
        if count == 0
            || lines
                .send(String::from_utf8_lossy(&buffer[..count]).into())
                .await
                .is_err()
        {
            break;
        }
    }
}

pub(in crate::queue) fn prepare(temporary: &std::path::Path) -> std::io::Result<()> {
    use kuvo_os::process::ProcessIdentity;
    kuvo_store::files::create_private_dirs(temporary)?;
    for folder in kuvo_store::files::list_of(temporary) {
        let pid = folder
            .to_str()
            .and_then(|name| name.strip_prefix("kuvo-signin-"))
            .and_then(|name| name.split_once('-'))
            .and_then(|(pid, _)| pid.parse::<u32>().ok());
        if let Some(pid) = pid.filter(|pid| *pid != 0 && *pid != std::process::id())
            && matches!(ProcessIdentity::read(pid), Ok(None))
        {
            let _ = std::fs::remove_dir_all(temporary.join(folder));
        }
    }
    Ok(())
}

fn no_browser(temporary: &std::path::Path) -> std::io::Result<PathBuf> {
    let name = if cfg!(windows) {
        "browser.cmd"
    } else {
        "browser"
    };
    let path = temporary.join(name);
    if cfg!(windows) {
        std::fs::write(&path, "@exit /b 0\r\n")?;
    } else {
        std::fs::write(&path, "#!/bin/sh\nexit 0\n")?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(path)
}
const BUSY: &str =
    "GitHub sign-in is already open on another device. Finish it there or wait for it to expire.";

fn wrap(error: Refusal, message: &str) -> Refusal {
    let status = error.reply().status;
    Refusal::new(
        status,
        if status == StatusCode::CONFLICT {
            BUSY
        } else {
            message
        },
    )
}

fn asking(caller: &Caller, headers: &HeaderMap) -> Result<String, Refusal> {
    owner(caller, headers).ok_or_else(|| {
        Refusal::new(
            StatusCode::BAD_REQUEST,
            "Start GitHub sign-in from Accounts.",
        )
    })
}

fn query(url: &str, field: &str) -> Option<String> {
    let (_, input) = url.split_once('?')?;
    url::form_urlencoded::parse(input.as_bytes())
        .find_map(|(key, value)| (key == field).then(|| value.into_owned()))
}

fn require_integration(path: &str) -> Result<(), Refusal> {
    match query(path, "integration").as_deref() {
        Some(INTEGRATION) => Ok(()),
        Some(_) => Err(Refusal::json(StatusCode::NOT_FOUND, json!({}))),
        None => Err(Refusal::new(
            StatusCode::BAD_REQUEST,
            "Say which account (integration).",
        )),
    }
}

fn attempt_id(id: Option<&str>) -> Result<String, Refusal> {
    id.filter(|id| {
        id.strip_prefix("att_").is_some_and(|value| {
            !value.is_empty() && value.bytes().all(|ch| ch.is_ascii_alphanumeric())
        })
    })
    .map(str::to_owned)
    .ok_or_else(|| {
        Refusal::new(
            StatusCode::BAD_REQUEST,
            "Say which sign-in (integration, attempt).",
        )
    })
}
