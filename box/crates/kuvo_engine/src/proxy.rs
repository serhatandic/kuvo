use std::{collections::HashSet, io, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::StreamExt;
use http::{HeaderMap, HeaderValue, Method, StatusCode, header};
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

    fn request(&self, record: &Record, method: Method, path: &str) -> RequestBuilder {
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
                jobs.iter().any(|job| {
                    !job.is_object() || job.get("status").and_then(Value::as_str) == Some("running")
                })
            })
        };
        if list(&forms).is_none_or(|forms| !forms.is_empty())
            || list(&permissions).is_none_or(|permissions| !permissions.is_empty())
            || running(&terminals)
            || running(&commands)
        {
            return true;
        }
    }
    false
}

fn list(answer: &Value) -> Option<&Vec<Value>> {
    answer.as_array().or_else(|| answer.get("data")?.as_array())
}

pub(crate) async fn http(
    engine: Engine,
    request: DispatchRequest,
    cancel: CancellationToken,
) -> Reply {
    let path = request.path.split('?').next().unwrap_or_default();
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
        outgoing = outgoing.body(request.body);
    }
    let response = tokio::select! {
        result = outgoing.send() => result.ok(),
        () = cancel.cancelled() => None,
    };
    let Some(response) = response else {
        return down(&engine).await.reply();
    };
    let status = response.status();
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
    let stream = async_stream::try_stream! {
        let _lease = lease;
        let mut stream = response.bytes_stream();
        loop {
            tokio::select! {
                part = stream.next() => match part {
                    Some(piece) => yield piece.map_err(io::Error::other)?,
                    None => break,
                },
                () = cancel.cancelled() => break,
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
