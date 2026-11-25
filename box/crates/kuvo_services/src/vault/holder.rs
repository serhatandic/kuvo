//! Compatibility endpoint used by existing one-off commands and the browser.
//! Only the resident owns unlock state. Each connection gets one request.
use std::{collections::BTreeMap, fs, io, path::Path, sync::Arc, time::Duration};

use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use http::{HeaderMap, HeaderName, HeaderValue, Method};
use http_body_util::BodyExt;
use kuvo_os::ipc::{Channel, Endpoint, LocalStream};
use kuvo_server::{Caller, DispatchRequest, Dispatcher, LocalOrigin, Transport};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    task::{JoinHandle, JoinSet},
    time,
};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use super::{Resident, Secret, crypto};

const MAX_LINE: usize = 8 * 1024 * 1024;
const VAULT_AUTH: &str = "x-kuvo-vault-auth";

pub struct Holder {
    task: Option<JoinHandle<()>>,
    #[cfg(unix)]
    _agents: Option<super::ssh_agent::Agents>,
}

impl Holder {
    pub async fn start(
        home: &Path,
        resident: Arc<Resident>,
        dispatcher: Dispatcher,
    ) -> io::Result<Self> {
        let run = home.join("run");
        let runtime = tokio::runtime::Handle::current();
        #[cfg(unix)]
        let agents = match super::ssh_agent::Agents::start(resident.clone()).await {
            Ok(agents) => agents,
            Err(error) => {
                eprintln!("The vault's SSH agents couldn't start: {error}");
                None
            }
        };
        #[cfg(windows)]
        kuvo_store::files::create_private_dirs(&home.join("run/ssh-for"))?;
        let bound = tokio::task::spawn_blocking(move || {
            let abandoned = run.join("vault-run");
            if let Err(error) = fs::remove_dir_all(abandoned)
                && error.kind() != io::ErrorKind::NotFound
            {
                return Err(error);
            }
            let key = Zeroizing::new(fs::read_to_string(run.join("socket.key"))?);
            let socket_key: [u8; 32] = hex::decode(key.trim())
                .ok()
                .and_then(|bytes| bytes.try_into().ok())
                .ok_or(io::ErrorKind::InvalidData)?;
            let endpoint = Endpoint::new(
                &run.join("vault.sock"),
                &socket_key,
                Channel::Holder("000000000000".into()),
            )?;
            let _runtime = runtime.enter();
            Ok::<_, io::Error>((Arc::new(Secret::new(key.trim().into())), endpoint.bind()?))
        })
        .await
        .map_err(io::Error::other)?;
        let (key, mut listener) = match bound {
            Ok(bound) => bound,
            Err(error) => {
                eprintln!("The vault's local socket couldn't listen: {error}");
                return Ok(Self {
                    task: None,
                    #[cfg(unix)]
                    _agents: agents,
                });
            }
        };
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    result = listener.accept() => {
                        let Ok(stream) = result else { break };
                        let key = key.clone();
                        let resident = resident.clone();
                        let dispatcher = dispatcher.clone();
                        connections.spawn(async move {
                            let _ = connection(stream, &key, &resident, dispatcher).await;
                        });
                    }
                    _ = connections.join_next(), if !connections.is_empty() => {}
                }
            }
        });
        Ok(Self {
            task: Some(task),
            #[cfg(unix)]
            _agents: agents,
        })
    }

    pub async fn shutdown(mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

#[derive(Deserialize)]
struct Hello {
    key: Secret,
    #[serde(default)]
    hello: String,
}

#[derive(Deserialize)]
struct Sent {
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    path: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    headers: BTreeMap<String, String>,
}

async fn connection(
    stream: LocalStream,
    key: &Secret,
    resident: &Arc<Resident>,
    dispatcher: Dispatcher,
) -> io::Result<()> {
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);
    let bytes = time::timeout(Duration::from_secs(10), line(&mut reader))
        .await
        .map_err(io::Error::other)??;
    let hello = decode::<Hello>(&bytes)?;
    if !bool::from(hello.key.expose().as_bytes().ct_eq(key.expose().as_bytes())) {
        return Ok(());
    }
    if !supported(&bytes) {
        return incompatible(&mut writer).await;
    }

    let nonce = URL_SAFE_NO_PAD.encode(&*crypto::random::<18>().map_err(io::Error::other)?);
    let mut answer = json!({ "formatVersion": 1, "nonce": nonce });
    if !hello.hello.is_empty() {
        let hello: String = hello.hello.chars().take(100).collect();
        let resident = resident.clone();
        if let Some(proof) = tokio::task::spawn_blocking(move || resident.holder_proof(&hello))
            .await
            .map_err(io::Error::other)?
        {
            answer["proof"] = Value::String(proof);
        }
    }
    write(&mut writer, answer).await?;

    let bytes = line(&mut reader).await?;
    if !supported(&bytes) {
        return incompatible(&mut writer).await;
    }
    let mut sent = decode::<Sent>(&bytes)?;
    let path = sent.path.split('?').next().unwrap_or_default();
    let labels_restore = path == "kuvo/work-queue/labels/restore";
    if !labels_restore && path != "kuvo/vault" && !path.starts_with("kuvo/vault/") {
        return reply(&mut writer, 404, "application/json", b"{}").await;
    }

    let method = sent
        .method
        .as_deref()
        .unwrap_or("GET")
        .to_ascii_uppercase()
        .parse::<Method>()
        .map_err(io::Error::other)?;
    let proved = if let Some(proof) = sent.headers.remove(VAULT_AUTH) {
        let hash = URL_SAFE_NO_PAD.encode(Sha256::digest(&sent.body));
        let covered = format!("{nonce}\0{method}\0{}\0{hash}", sent.path);
        let resident = resident.clone();
        tokio::task::spawn_blocking(move || resident.proved_holder_client(&covered, &proof))
            .await
            .map_err(io::Error::other)?
    } else {
        false
    };
    let headers: HeaderMap = sent
        .headers
        .into_iter()
        .filter_map(|(name, value)| {
            let name = HeaderName::from_bytes(name.as_bytes()).ok()?;
            let value = HeaderValue::from_str(&value).ok()?;
            Some((name, value))
        })
        .collect();
    let body = STANDARD
        .decode(Zeroizing::new(sent.body).as_bytes())
        .map_err(io::Error::other)?;
    let origin = if labels_restore || proved {
        LocalOrigin::VaultHolder
    } else {
        LocalOrigin::Mac
    };
    let cancel = CancellationToken::new();
    let request = DispatchRequest {
        method,
        path: sent.path,
        headers,
        body: body.into(),
        caller: Caller::Local(Transport::Http {
            origin,
            internal: false,
        }),
    };
    tokio::select! {
        result = async {
            let response = dispatcher.dispatch(request, cancel.clone()).await;
            let status = response.status.as_u16();
            let kind = response.headers.get(http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()).unwrap_or("").to_owned();
            let mut output = Zeroizing::new(Vec::new());
            let mut body = response.body;
            while let Some(part) = body.frame().await {
                let part = part?;
                if let Some(data) = part.data_ref() {
                    if output.len() + data.len() > MAX_LINE * 2 {
                        return Err(io::ErrorKind::InvalidData.into());
                    }
                    output.extend_from_slice(data);
                    if kind.starts_with("application/x-ndjson")
                        && let Some(end) = output.iter().position(|byte| *byte == b'\n')
                    {
                        output.truncate(end + 1);
                        break;
                    }
                }
            }
            reply(&mut writer, status, &kind, &output).await
        } => result,
        _ = reader.read_u8() => {
            cancel.cancel();
            Ok(())
        }
    }
}

async fn line(reader: &mut (impl AsyncBufRead + Unpin)) -> io::Result<Zeroizing<Vec<u8>>> {
    let mut bytes = Zeroizing::new(Vec::new());
    reader
        .take(MAX_LINE as u64)
        .read_until(b'\n', &mut bytes)
        .await?;
    if bytes.last() != Some(&b'\n') {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    Ok(bytes)
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> io::Result<T> {
    serde_json::from_slice(bytes).map_err(|_| io::ErrorKind::InvalidData.into())
}

fn supported(bytes: &[u8]) -> bool {
    decode::<Value>(bytes).ok().and_then(|value| {
        value.as_object().map(|object| {
            object
                .get("formatVersion")
                .is_none_or(|marker| matches!(marker.as_u64(), Some(0..=1)))
        })
    }) == Some(true)
}

async fn incompatible(writer: &mut (impl AsyncWrite + Unpin)) -> io::Result<()> {
    reply(writer, 409, "application/json", br#"{"code":"unsupportedFormatVersion","message":"This Vault holder uses an unsupported socket format."}"#).await
}

async fn reply(
    writer: &mut (impl AsyncWrite + Unpin),
    status: u16,
    kind: &str,
    bytes: &[u8],
) -> io::Result<()> {
    write(writer, json!({ "formatVersion": 1, "status": status, "type": kind, "body": STANDARD.encode(bytes) })).await
}

async fn write(writer: &mut (impl AsyncWrite + Unpin), value: Value) -> io::Result<()> {
    let mut bytes = Zeroizing::new(serde_json::to_vec(&value)?);
    bytes.push(b'\n');
    writer.write_all(&bytes).await
}
