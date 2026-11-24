use super::invalid;
use kuvo_os::process::ProcessIdentity;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};
use std::{io, path::PathBuf, str::FromStr};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub type Metadata = Map<String, Value>;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HoldKind {
    JsonRpc,
    #[serde(rename = "claude")]
    ControlJson,
    Raw,
}

impl FromStr for HoldKind {
    type Err = io::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "jsonrpc" => Ok(Self::JsonRpc),
            "claude" => Ok(Self::ControlJson),
            "raw" => Ok(Self::Raw),
            _ => Err(invalid("Invalid holder kind")),
        }
    }
}

impl HoldKind {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::JsonRpc => "jsonrpc",
            Self::ControlJson => "claude",
            Self::Raw => "raw",
        }
    }
}

#[derive(Deserialize, Serialize)]
pub(super) struct Launch {
    pub key: String,
    pub spec: Spec,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Spec {
    pub command: Vec<PathBuf>,
    pub cwd: Option<PathBuf>,
    pub bridge: Option<u32>,
    pub bun: Option<String>,
    #[serde(default)]
    pub meta: Metadata,
    pub journal_bytes: Option<u64>,
    pub linger: Option<u64>,
    pub orphan: Option<u64>,
    pub portable_requests: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Exit {
    pub code: Option<i32>,
    pub signal: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Record {
    pub proto: String,
    pub holder: String,
    pub id: String,
    pub kind: HoldKind,
    pub pid: u32,
    pub child: Option<u32>,
    pub program: PathBuf,
    pub cwd: Option<PathBuf>,
    pub started: u64,
    pub bridge: Option<u32>,
    pub generation: u64,
    pub exited: Option<Exit>,
    pub meta: Metadata,
    pub(super) key: String,
    // Rust-only restart record. The wire frames themselves are hold/1.
    #[serde(flatten)]
    pub(super) identity: Identity,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Identity {
    pub process: ProcessIdentity,
}

#[derive(Deserialize, Serialize)]
pub(super) struct Ready {
    pub ready: String,
    pub pid: u32,
    pub child: Option<u32>,
}

#[derive(Deserialize, Serialize)]
pub(super) struct Hello {
    pub hello: String,
    pub key: String,
    pub generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<i64>,
    #[serde(default)]
    pub complete: bool,
}

#[derive(Deserialize, Serialize)]
pub struct Welcome {
    pub hello: String,
    pub pid: u32,
    pub child: Option<u32>,
    pub generation: u64,
    pub seq: u64,
    pub first: u64,
    pub gap: bool,
    pub exited: Option<Exit>,
    pub stderr: String,
    pub meta: Metadata,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
pub(super) struct Input {
    #[serde(rename = "i", skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ack: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<Metadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prepare: Option<i64>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub unpin: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub end: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub bye: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Output {
    Line {
        #[serde(rename = "s")]
        sequence: u64,
        #[serde(rename = "o")]
        line: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        again: bool,
    },
    Stderr {
        #[serde(rename = "e")]
        text: String,
    },
    Exited {
        #[serde(rename = "x")]
        exit: Exit,
    },
    Prepared {
        prepared: bool,
    },
    Fenced {
        fenced: bool,
    },
    Refused {
        refused: String,
    },
}

pub(super) const MAX_FRAME: u64 = 128_000_000;
pub(super) const MAX_HELLO: u64 = 64_000;

pub(super) async fn read<T: DeserializeOwned>(
    reader: &mut (impl AsyncBufRead + Unpin),
    limit: u64,
) -> io::Result<T> {
    let mut bytes = Vec::new();
    reader.take(limit).read_until(b'\n', &mut bytes).await?;
    if bytes.is_empty() {
        return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
    }
    if bytes.last() != Some(&b'\n') {
        return Err(invalid("Incomplete or oversized holder frame"));
    }
    serde_json::from_slice(&bytes).map_err(|_| invalid("Invalid holder frame"))
}

pub(super) async fn write<T: Serialize>(
    writer: &mut (impl AsyncWrite + Unpin),
    frame: &T,
) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(frame)?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await
}
