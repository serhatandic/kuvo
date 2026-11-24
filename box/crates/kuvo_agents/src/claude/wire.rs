//! Stream-json control messages travel on the same pipe as the conversation.

use std::io;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::hold::ControllerInput;

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum Frame {
    ControlResponse {
        response: Reply,
    },
    ControlRequest {
        request_id: String,
        request: Value,
    },
    ControlCancelRequest {
        request_id: String,
    },
    #[serde(other)]
    Message,
}

#[derive(Deserialize)]
pub(super) struct Reply {
    pub request_id: String,
    #[serde(flatten)]
    pub body: ReplyBody,
}

#[derive(Deserialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub(super) enum ReplyBody {
    Success {
        #[serde(default)]
        response: Value,
        pending_permission_requests: Option<Vec<Frame>>,
    },
    Error,
}

pub(super) fn control_request(id: &str, request: Value) -> Value {
    json!({"type":"control_request", "request_id":id, "request":request})
}

pub(super) async fn request(
    input: &mut ControllerInput,
    id: &str,
    request: Value,
) -> io::Result<()> {
    input.write(control_request(id, request).to_string()).await
}

pub(super) async fn reply(
    input: &mut ControllerInput,
    id: &str,
    body: Result<Value, &str>,
) -> io::Result<()> {
    let response = match body {
        Ok(value) => json!({"subtype":"success", "request_id":id, "response":value}),
        Err(error) => json!({"subtype":"error", "request_id":id, "error":error}),
    };
    input
        .write(json!({"type":"control_response", "response":response}).to_string())
        .await
}
