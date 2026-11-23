//! Requests made by the resident to its own engine.

use std::{fmt, time::Duration};

use http::{Method, StatusCode};
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
        let mut request = self
            .upstream
            .request(&lease.record, method, path)
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
