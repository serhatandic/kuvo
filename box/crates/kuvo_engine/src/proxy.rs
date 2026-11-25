use std::{collections::HashSet, io, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::StreamExt;
use http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use http_body_util::BodyExt;
use kuvo_server::{DispatchRequest, Refusal, Reply, socket::Socket};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest::{Client, RequestBuilder, redirect};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_tungstenite::{connect_async, tungstenite::client::IntoClientRequest};
use tokio_util::sync::CancellationToken;

use crate::{
    launch::Record,
    lifetime::{Demand, Engine},
};

const DIRECTORY: &str = "x-opencode-directory";
const REQUEST_HEADERS: &[&str] = &["content-type", "accept", DIRECTORY, "last-event-id"];
const RESPONSE_HEADERS: &[&str] = &["content-type", "cache-control"];
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const SESSION_WRITE_TURN: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub(crate) struct Upstream(Client);

impl Upstream {
    pub fn new() -> Result<Self, reqwest::Error> {
        let client = Client::builder()
            .no_proxy()
            .redirect(redirect::Policy::custom(|attempt| {
                // Keep all upstream requests on the authenticated engine address.
                if attempt.previous().len() < 10
                    && attempt
                        .previous()
                        .first()
                        .is_some_and(|origin| origin.origin() == attempt.url().origin())
                {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .connect_timeout(Duration::from_secs(3))
            .build()?;
        Ok(Self(client))
    }

    pub(crate) fn request(&self, record: &Record, method: Method, path: &str) -> RequestBuilder {
        self.0
            .request(method, record.url("http", path))
            .basic_auth("opencode", Some(&record.password))
    }

    async fn probe<T: serde::de::DeserializeOwned>(
        &self,
        record: &Record,
        path: &str,
        directory: Option<&str>,
        timeout: Duration,
    ) -> Result<T, reqwest::Error> {
        let mut request = self.request(record, Method::GET, path).timeout(timeout);
        if let Some(directory) = directory {
            request = request.header(
                DIRECTORY,
                utf8_percent_encode(directory, NON_ALPHANUMERIC).to_string(),
            );
        }
        request.send().await?.error_for_status()?.json().await
    }

    pub(crate) async fn get_data<T: serde::de::DeserializeOwned>(
        &self,
        record: &Record,
        path: &str,
        headers: &HeaderMap,
        timeout: Duration,
    ) -> Result<T, reqwest::Error> {
        #[derive(Deserialize)]
        struct Envelope<T> {
            data: T,
        }
        let mut request = self.request(record, Method::GET, path).timeout(timeout);
        if let Some(directory) = headers.get(DIRECTORY) {
            request = request.header(DIRECTORY, directory);
        }
        let answer: Envelope<T> = request.send().await?.error_for_status()?.json().await?;
        Ok(answer.data)
    }

    pub async fn healthy(&self, record: &Record) -> bool {
        #[derive(Deserialize)]
        struct Sessions {
            #[serde(rename = "data")]
            _sessions: Vec<serde::de::IgnoredAny>,
        }
        self.probe::<Sessions>(record, "api/session?limit=1", None, Duration::from_secs(3))
            .await
            .is_ok()
    }
}

pub(crate) async fn busy(upstream: &Upstream, record: &Record) -> bool {
    #[derive(Deserialize)]
    struct Active {
        data: ActiveSessions,
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ActiveSessions {
        List(Vec<serde::de::IgnoredAny>),
        Map(std::collections::BTreeMap<String, serde::de::IgnoredAny>),
    }

    let Ok(active) = upstream
        .probe::<Active>(record, "api/session/active", None, PROBE_TIMEOUT)
        .await
    else {
        return true;
    };
    if match active.data {
        ActiveSessions::List(sessions) => !sessions.is_empty(),
        ActiveSessions::Map(sessions) => !sessions.is_empty(),
    } {
        return true;
    }
    let Ok(folders) = upstream
        .probe::<Value>(record, "api/debug/location", None, PROBE_TIMEOUT)
        .await
    else {
        return true;
    };
    let Some(locations) = list(&folders) else {
        return true;
    };
    let mut checked = HashSet::new();
    for directory in locations
        .iter()
        .filter_map(|location| location.get("directory")?.as_str())
    {
        if directory.is_empty() || !checked.insert(directory) {
            continue;
        }
        if folder_busy(upstream, record, directory).await {
            return true;
        }
    }
    false
}

pub(crate) async fn folder_busy(upstream: &Upstream, record: &Record, directory: &str) -> bool {
    let get = |path| upstream.probe::<Value>(record, path, Some(directory), PROBE_TIMEOUT);
    let Ok((forms, permissions, terminals, commands)) = tokio::try_join!(
        get("api/form"),
        get("api/permission/request"),
        get("api/pty"),
        get("api/shell"),
    ) else {
        return true;
    };
    let running = |jobs: &Value| {
        list(jobs).is_none_or(|jobs| {
            jobs.iter()
                .any(|job| !job.is_object() || job["status"] == "running")
        })
    };
    list(&forms).is_none_or(|forms| !forms.is_empty())
        || list(&permissions).is_none_or(|permissions| !permissions.is_empty())
        || running(&terminals)
        || running(&commands)
}

fn list(answer: &Value) -> Option<&Vec<Value>> {
    answer.as_array().or_else(|| answer.get("data")?.as_array())
}

pub(crate) async fn http(
    engine: Engine,
    request: DispatchRequest,
    cancel: CancellationToken,
) -> Reply {
    if let Some(key) = crate::raw_prompts::prompt(&request) {
        let here = engine.clone();
        let send = tokio::spawn(async move {
            let turn_key = crate::disk::key(&key.0, &key.1);
            let _turn = here.draft_turns.acquire(&turn_key).await;
            if let Some(answer) = here.raw_prompts.read(&key).await {
                return Some(answer);
            }
            let answer = tokio::time::timeout(Duration::from_secs(120), async {
                let reply = forward(here.clone(), request, CancellationToken::new()).await;
                let body = reply.body.collect().await.ok()?.to_bytes().to_vec();
                Some(crate::raw_prompts::Answer {
                    status: reply.status,
                    headers: reply.headers,
                    body,
                })
            })
            .await
            .ok()
            .flatten()?;
            if answer.status.is_success() {
                here.raw_prompts.keep(key, answer.clone()).await;
            }
            Some(answer)
        });
        return match send.await.ok().flatten() {
            Some(answer) => answer.reply(),
            None => down(&engine).await.reply(),
        };
    }
    if let Some(key) = crate::raw_prompts::removed(&request) {
        engine.raw_prompts.forget(key).await;
    }
    forward(engine, request, cancel).await
}

async fn hold_prompt(
    engine: &Engine,
    session: &str,
    request: &DispatchRequest,
) -> Result<(bool, Option<Vec<u8>>), Refusal> {
    use crate::queue::{Delivery, Prompt};
    let body: Value = serde_json::from_slice(&request.body).unwrap_or_default();
    let id = match body.get("id") {
        Some(id) => id
            .as_str()
            .filter(|id| crate::send::valid_id(id))
            .map(str::to_owned)
            .ok_or_else(|| {
                Refusal::new(
                    StatusCode::BAD_REQUEST,
                    "This message couldn’t be queued. Send it again as a new message.",
                )
            })?,
        None => crate::send::new_id(engine)?,
    };
    if !body["text"].is_string() {
        return Err(crate::release::turn_refusal());
    }
    let _turn = engine.turn(session).await;
    if engine
        .queues
        .get(session)
        .await
        .map_err(|error| {
            eprintln!("Reading a waiting message: {error}");
            Refusal::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "This computer couldn’t save the message. Try sending it again.",
            )
        })?
        .contains(&id)
    {
        return Ok((true, None));
    }
    // Use the same raw attachment normalizer as an immediate prompt, after
    // checking its durable ID. If the hold ends, forward these bytes instead
    // of copying the attachment a second time.
    let converted = crate::attachments::raw(engine, request).await;
    let body = converted
        .as_deref()
        .and_then(|bytes| serde_json::from_slice(bytes).ok())
        .unwrap_or(body);
    let text = body["text"]
        .as_str()
        .ok_or_else(crate::release::turn_refusal)?;
    let prompt = Prompt {
        id,
        text: text.into(),
        delivery: if body["delivery"] == "queue" {
            Delivery::Queue
        } else {
            Delivery::Steer
        },
        files: body["files"].as_array().cloned().unwrap_or_default(),
        metadata: body
            .get("metadata")
            .filter(|value| value.is_object())
            .cloned(),
    };
    let waiting = engine
        .queues
        .keep(session, &prompt, None, None, None, true)
        .await
        .map_err(|error| {
            eprintln!("Saving a waiting message: {error}");
            Refusal::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "This computer couldn’t save the message. Try sending it again.",
            )
        })?;
    Ok((waiting, converted))
}

async fn forward(engine: Engine, request: DispatchRequest, cancel: CancellationToken) -> Reply {
    let path = request.path.split('?').next().unwrap_or_default();
    let mut prepared_body = None;
    let _admitted = if let Some(id) = crate::session_move::prompt_target(&request.method, path) {
        let prompt = path.ends_with("/prompt");
        let decision = path.contains("/permission/") || path.contains("/form/");
        let admitted = if prompt || decision {
            crate::session_move::admit(&engine, &id, None).await
        } else {
            crate::session_move::admit_turn(&engine, &id, None).await
        };
        match admitted {
            Ok(write) => {
                if prompt
                    && (crate::release::holding(&engine.home.join("run"))
                        || crate::release::releasing(&engine.home.join("run")))
                {
                    match hold_prompt(&engine, &id, &request).await {
                        Ok((true, _)) => {
                            return Reply::json(StatusCode::OK, json!({"waits": true}));
                        }
                        Ok((false, body)) => prepared_body = body,
                        Err(error) => return error.reply(),
                    }
                }
                Some(write)
            }
            Err(error) => return error.reply(),
        }
    } else {
        None
    };
    let demand = match path {
        "api/event" | "api/session/active" => Demand::Passive,
        _ => Demand::Start,
    };
    let lease = tokio::select! {
        lease = engine.acquire(demand) => lease,
        () = cancel.cancelled() => None,
    };
    let Some(lease) = lease else {
        if path == "api/session/active" {
            return Reply::json(StatusCode::OK, json!({"data": []}));
        }
        return down(&engine).await.reply();
    };
    if let Err(refused) = crate::consent::raw(&engine, &lease, &request, &cancel).await {
        return refused.reply();
    }
    let changing = (request.method == Method::PATCH)
        .then(|| path.strip_prefix("api/session/"))
        .flatten()
        .and_then(|id| percent_encoding::percent_decode_str(id).decode_utf8().ok())
        .filter(|id| crate::sessions::valid_id(id));
    let mut session_turn = None;
    let mut flip = None;
    if let Some(id) = &changing {
        session_turn = cancel.run_until_cancelled(engine.note_turn(id)).await;
        if session_turn.is_none() {
            return down(&engine).await.reply();
        }
        if let Some(archived) = serde_json::from_slice::<Value>(&request.body)
            .ok()
            .and_then(|body| body["metadata"]["kuvo.archived"].as_bool())
        {
            let before = engine
                .call::<Value>(&lease, Method::GET, path, None)
                .await
                .ok();
            if before.map(|state| state["metadata"]["kuvo.archived"] == true) != Some(archived) {
                flip = Some((id, archived));
            }
        }
    }
    let mut outgoing =
        engine
            .upstream
            .request(&lease.record, request.method.clone(), &request.path);
    for &name in REQUEST_HEADERS {
        if let Some(value) = request.headers.get(name).filter(|value| !value.is_empty()) {
            outgoing = outgoing.header(name, value);
        }
    }
    if !matches!(request.method, Method::GET | Method::HEAD) && !request.body.is_empty() {
        let body = match prepared_body {
            Some(body) => Some(body),
            None => crate::attachments::raw(&engine, &request).await,
        };
        outgoing = outgoing.body(body.map_or(request.body, Into::into));
    }
    let send = async {
        let request = outgoing.send();
        tokio::pin!(request);
        tokio::select! {
            answer = &mut request => answer.ok(),
            _ = tokio::time::sleep(SESSION_WRITE_TURN), if session_turn.is_some() => {
                drop(session_turn.take());
                request.await.ok()
            }
        }
    };
    let response = tokio::select! {
        result = send => result,
        () = cancel.cancelled() => None,
    };
    let Some(response) = response else {
        return down(&engine).await.reply();
    };
    let status = response.status();
    if status.is_success()
        && let Some((id, archived)) = flip
    {
        crate::sessions::after_archive(&engine, &lease, id, archived).await;
    }
    drop(session_turn);
    let headers: HeaderMap = RESPONSE_HEADERS
        .iter()
        .filter_map(|&name| {
            response
                .headers()
                .get(name)
                .cloned()
                .map(|value| (header::HeaderName::from_static(name), value))
        })
        .collect();
    let session_tail = path.strip_prefix("api/session/").and_then(|path| {
        let mut parts = path.split('/');
        let id = parts.next()?;
        (!id.is_empty()).then_some((id, parts.next(), parts.next(), parts.next()))
    });
    let session_read = matches!(session_tail, Some((id, None, None, None)) if id != "active");
    let message_read = matches!(session_tail, Some((_, Some("message"), _, None)));
    if status.is_success() && request.method == Method::GET && (session_read || message_read) {
        return tokio::select! {
            () = cancel.cancelled() => down(&engine).await.reply(),
            result = async {
                let mut page = response.json::<Value>().await.map_err(|_| {
                    Refusal::new(StatusCode::BAD_GATEWAY, "The engine's conversation could not be read.")
                })?;
                if session_read {
                    if let Some(session) = page.get_mut("data").and_then(Value::as_object_mut) {
                        crate::context::enrich(&engine, &lease.record, path, &request.headers, session).await;
                    }
                } else {
                    crate::conversation::compaction_page(&mut page);
                    engine.tool_words.fill_page(&engine, &mut page).await;
                }
                let mut reply = Reply::json(status, page);
                reply.headers = headers;
                Ok::<_, Refusal>(reply)
            } => result.unwrap_or_else(Refusal::reply),
        };
    }
    let events = status.is_success() && request.method == Method::GET && path == "api/event";
    let stream = async_stream::try_stream! {
        let _lease = lease;
        let mut stream = response.bytes_stream();
        let mut pending = Vec::new();
        loop {
            let next = tokio::select! {
                part = stream.next() => part,
                () = cancel.cancelled() => break,
            };
            let Some(piece) = next else {
                if !pending.is_empty() { yield pending.into(); }
                break;
            };
            let piece = piece.map_err(io::Error::other)?;
            if !events {
                yield piece;
                continue;
            }
            pending.extend_from_slice(&piece);
            let end = pending.iter().rposition(|byte| *byte == b'\n').map_or(0, |index| index + 1);
            if end > 0 {
                let rest = pending.split_off(end);
                let complete = std::mem::replace(&mut pending, rest);
                let mut output = Vec::with_capacity(complete.len());
                for line in complete.split_inclusive(|byte| *byte == b'\n') {
                    if let Some(event) = crate::conversation::compaction_event(line) {
                        output.extend_from_slice(&event);
                    } else {
                        output.extend_from_slice(line);
                    }
                }
                yield output.into();
            }
            if pending.len() > 4 * 1024 * 1024 {
                Err(io::Error::other("The engine event is too large."))?;
            }
        }
    };
    let mut reply = Reply::stream(status, "application/octet-stream", stream);
    reply.headers = headers;
    reply
}

pub(crate) async fn socket(
    engine: Engine,
    request: DispatchRequest,
    cancel: CancellationToken,
) -> Result<Socket, Refusal> {
    let mut parts = request
        .path
        .split('?')
        .next()
        .unwrap_or_default()
        .split('/');
    if !matches!((parts.next(), parts.next(), parts.next(), parts.next(), parts.next()),
        (Some("api"), Some("pty"), Some(id), Some("connect"), None) if !id.is_empty())
    {
        return Err(Refusal::new(
            StatusCode::FORBIDDEN,
            "That isn't available over the link.",
        ));
    }
    let Some(lease) = engine.acquire(Demand::Existing).await else {
        return Err(Refusal::new(
            StatusCode::BAD_GATEWAY,
            "The engine isn't running.",
        ));
    };
    let failed = || Refusal::new(StatusCode::BAD_GATEWAY, "The engine closed it.");
    let mut upgrade = lease
        .record
        .url("ws", &request.path)
        .into_client_request()
        .map_err(|_| failed())?;
    let auth = STANDARD.encode(format!("opencode:{}", lease.record.password));
    upgrade.headers_mut().insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Basic {auth}")).expect("base64 authorization"),
    );
    if let Some(directory) = request.headers.get(DIRECTORY) {
        upgrade.headers_mut().insert(DIRECTORY, directory.clone());
    }
    let (upstream, _) = tokio::select! {
        result = tokio::time::timeout(PROBE_TIMEOUT, connect_async(upgrade)) => {
            result.map_err(|_| failed())?.map_err(|_| failed())?
        }
        () = cancel.cancelled() => return Err(failed()),
    };
    let (client, server) = Socket::pair();
    tokio::spawn(async move {
        let _lease = lease;
        let _ = server.bridge(upstream, cancel).await;
    });
    Ok(client)
}

async fn down(engine: &Engine) -> Refusal {
    if engine.stopped_intentionally().await {
        return Refusal::new(StatusCode::SERVICE_UNAVAILABLE, "The engine isn't running.");
    }
    let name = if engine.is_mac_box().await {
        "This Mac's engine"
    } else {
        "This machine's engine"
    };
    let message = match engine.retry_minutes().await {
        Some(1) => format!("{name} keeps stopping. It's started again in about a minute."),
        Some(minutes) => {
            format!("{name} keeps stopping. It's started again in about {minutes} minutes.")
        }
        None => format!("{name} is starting. Try again in a moment."),
    };
    Refusal::new(StatusCode::SERVICE_UNAVAILABLE, message)
}
