//! Stream-json control messages travel on the same pipe as the conversation.

use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum Frame {
    ControlResponse {
        response: Reply,
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
    },
    Error,
}

pub(super) fn control_request(id: &str, request: Value) -> Value {
    json!({"type":"control_request", "request_id":id, "request":request})
}
