//! Requests made by the resident to its own engine.

use std::{fmt, time::Duration};

use http::{HeaderMap, Method, StatusCode};
use kuvo_server::Refusal;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{
    Engine,
    lifetime::{Demand, Lease},
};

pub(crate) const WAIT: Duration = Duration::from_secs(10);

pub(crate) fn segment(id: &str) -> String {
    const COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'!')
        .remove(b'~')
        .remove(b'*')
        .remove(b'\'')
        .remove(b'(')
        .remove(b')');
    utf8_percent_encode(id, COMPONENT).to_string()
}

pub(crate) fn folder_header(folder: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if !folder.is_empty() {
        headers.insert(
            "x-opencode-directory",
            segment(folder).parse().expect("encoded folder"),
        );
    }
    headers
}

pub(crate) fn active_ids(data: &Value) -> Option<Vec<&str>> {
    match data {
        Value::Array(ids) => Some(ids.iter().filter_map(Value::as_str).collect()),
        Value::Object(ids) => Some(ids.keys().map(String::as_str).collect()),
        _ => None,
    }
}

pub(crate) async fn start(engine: &Engine) -> Result<Lease, Refusal> {
    engine
        .acquire(Demand::Start)
        .await
        .ok_or_else(|| Refusal::new(StatusCode::SERVICE_UNAVAILABLE, "The engine isn't running."))
}

#[derive(Debug)]
pub(crate) enum Error {
    Transport(reqwest::Error),
    Status(StatusCode, String),
    Invalid(serde_json::Error),
}

impl Error {
    pub(crate) fn missing(&self) -> bool {
        matches!(self, Self::Status(StatusCode::NOT_FOUND, _))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => write!(out, "Could not reach the engine: {error}"),
            Self::Status(_, message) => out.write_str(message),
            Self::Invalid(error) => write!(out, "Could not read the engine's answer: {error}"),
        }
    }
}
impl std::error::Error for Error {}

impl Engine {
    pub(crate) async fn call<T: DeserializeOwned>(
        &self,
        lease: &Lease,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<T, Error> {
        let mut value = self.call_raw(lease, method, path, body, WAIT).await?;
        if let Some(data) = value.get_mut("data").filter(|data| !data.is_null()) {
            value = data.take();
        }
        serde_json::from_value(value).map_err(Error::Invalid)
    }

    /// Some engine routes put cursor metadata next to `data`.
    pub(crate) async fn call_raw(
        &self,
        lease: &Lease,
        method: Method,
        path: &str,
        body: Option<&Value>,
        wait: Duration,
    ) -> Result<Value, Error> {
        self.call_with_headers(lease, method, path, body, wait, HeaderMap::new())
            .await
    }

    pub(crate) async fn call_with_headers(
        &self,
        lease: &Lease,
        method: Method,
        path: &str,
        body: Option<&Value>,
        wait: Duration,
        headers: HeaderMap,
    ) -> Result<Value, Error> {
        let mut request = self
            .upstream
            .request(&lease.record, method, path)
            .headers(headers)
            .timeout(wait);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.map_err(Error::Transport)?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(Error::Transport)?;
        let value: Value = if bytes.is_empty() {
            Value::Null
        } else if status.is_success() {
            serde_json::from_slice(&bytes).map_err(Error::Invalid)?
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        if !status.is_success() {
            let message = value["message"]
                .as_str()
                .filter(|message| !message.is_empty())
                .map_or_else(
                    || format!("It answered {}.", status.as_u16()),
                    str::to_owned,
                );
            return Err(Error::Status(status, message));
        }
        Ok(value)
    }
}

#[derive(serde::Deserialize)]
struct Page {
    data: Vec<Value>,
    cursor: Option<PageCursor>,
}

#[derive(serde::Deserialize)]
struct PageCursor {
    next: Option<String>,
    previous: Option<String>,
}

pub(crate) enum Scan {
    Sessions,
    Messages,
}

impl Scan {
    pub async fn read(
        self,
        engine: &Engine,
        lease: &Lease,
        route: &str,
    ) -> Result<Vec<Value>, Box<dyn std::error::Error + Send + Sync>> {
        let limit = match self {
            Self::Sessions => 100,
            Self::Messages => 200,
        };
        let mut path = format!("{route}?limit={limit}&order=desc");
        let mut seen = std::collections::BTreeSet::new();
        let mut rows = Vec::new();
        for _ in 0..1000 {
            let page = engine
                .upstream
                .request(&lease.record, Method::GET, &path)
                .timeout(WAIT)
                .send()
                .await?
                .error_for_status()?
                .json::<Page>()
                .await?;
            let count = page.data.len();
            rows.extend(page.data);
            if count == 0
                || matches!(self, Self::Messages) && (count < limit || rows.len() >= 20_000)
            {
                break;
            }
            let next = page.cursor.and_then(|cursor| match self {
                Self::Sessions => cursor.next,
                Self::Messages => cursor.next.or(cursor.previous),
            });
            let Some(next) = next else { break };
            if !seen.insert(next.clone()) {
                return Err(std::io::Error::other("The engine's page cursor repeated.").into());
            }
            path = format!("{route}?limit={limit}&cursor={}", segment(&next));
        }
        if seen.len() == 1000 {
            return Err(std::io::Error::other("The engine's inventory did not finish.").into());
        }
        Ok(rows)
    }
}
