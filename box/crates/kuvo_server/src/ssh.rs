//! The SSH commands relay byte packets to the already selected resident. The resident
//! authenticates addressed connections and hands both verbs to the same dispatcher.

use std::{fs, io, path::Path, sync::Arc, time::Duration};

use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::AeadInPlace};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE_NO_PAD},
};
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header::CONTENT_TYPE};
use http_body_util::BodyExt;
use hyper::body::Bytes;
use kuvo_os::ipc::{self, Channel, Endpoint, LocalStream};
use kuvo_protocol::version::{ROUTE_VERSION, ROUTE_VERSION_HEADER, UnsupportedFormatVersion};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    time::{self, Instant},
};
use tokio_util::sync::CancellationToken;
use zeroize::{Zeroize, Zeroizing};

use crate::{Caller, DispatchRequest, Dispatcher, Pairing, Transport, resident::random};

const PACKET: usize = 64 * 1024;
const ACCEPTED: &[u8] = &[1];
const PROLOGUE: &[u8] = b"kuvo-ssh v1";
const NOISE: &str = "Noise_IK_25519_AESGCM_SHA256";
const STARTUP_WAIT: Duration = Duration::from_secs(80);
const RETRY: Duration = Duration::from_millis(200);

/// The setup port supplies a current, already verified view of this computer and its pairings.
/// The resident rechecks grants while addressed streams are open.
pub trait Pairings: Send + Sync {
    fn identity(&self, computer: &str) -> Result<[u8; 32], IdentityRefusal>;
    fn paired(&self, key: &[u8; 32]) -> Option<Grant>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentityRefusal {
    Updating,
    Disabled,
    Unavailable,
    WrongComputer,
}

impl IdentityRefusal {
    fn refusal(self) -> Denied {
        match self {
            Self::Updating => Denied(
                503,
                "Wait for the Kuvo Code update to finish before connecting over SSH.",
            ),
            Self::Disabled => Denied(403, "“Let my devices connect” is off on this Mac."),
            Self::Unavailable => Denied(503, "Kuvo couldn’t verify this Mac’s identity."),
            Self::WrongComputer => Denied(
                409,
                "SSH reached a different computer or one being removed.",
            ),
        }
    }
}

/// The standalone server before setup is wired up has no paired identity.
pub struct NoPairings;

impl Pairings for NoPairings {
    fn identity(&self, _computer: &str) -> Result<[u8; 32], IdentityRefusal> {
        Err(IdentityRefusal::WrongComputer)
    }
    fn paired(&self, _key: &[u8; 32]) -> Option<Grant> {
        None
    }
}

#[derive(Clone, Debug)]
pub enum Command {
    Route {
        internal: bool,
        internal_path: Option<String>,
    },
    RouteTo,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PipeHello {
    key: String,
    addressed: bool,
    internal: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    internal_path: Option<String>,
}

impl PipeHello {
    fn new(command: &Command, key: &[u8; 32]) -> Self {
        let (addressed, internal, internal_path) = match command {
            Command::Route {
                internal,
                internal_path,
            } => (false, *internal, internal_path.clone()),
            Command::RouteTo => (true, false, None),
        };
        Self {
            key: hex::encode(key),
            addressed,
            internal,
            internal_path,
        }
    }

    fn accept(self, key: &[u8; 32]) -> io::Result<Command> {
        if !bool::from(self.key.as_bytes().ct_eq(hex::encode(key).as_bytes()))
            || (self.addressed && (self.internal || self.internal_path.is_some()))
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Invalid SSH pipe credentials.",
            ));
        }
        Ok(if self.addressed {
            Command::RouteTo
        } else {
            Command::Route {
                internal: self.internal,
                internal_path: self.internal_path,
            }
        })
    }
}

pub(crate) struct Listener {
    ipc: ipc::Listener,
    key: [u8; 32],
}

impl Listener {
    pub(crate) async fn bind(run: &Path) -> io::Result<Self> {
        let run = run.to_owned();
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let folder = run.join("route");
            kuvo_store::files::create_private_dirs(&folder)?;
            let key = ensure_key(&run)?;
            let _runtime = runtime.enter();
            let ipc = Endpoint::new(&folder.join("ssh"), &key, Channel::Ssh)?.bind()?;
            Ok(Self { ipc, key })
        })
        .await
        .map_err(io::Error::other)?
    }

    pub(crate) async fn accept(&mut self) -> io::Result<LocalStream> {
        self.ipc.accept().await
    }

    pub(crate) fn key(&self) -> &[u8; 32] {
        &self.key
    }
}

fn parse_key(raw: &str) -> io::Result<[u8; 32]> {
    let raw = raw.trim();
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "The box's socket key is unavailable.",
        )
    };
    if raw.len() != 64 || raw.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return Err(invalid());
    }
    let mut key = [0; 32];
    hex::decode_to_slice(raw, &mut key).map_err(|_| invalid())?;
    Ok(key)
}

fn ensure_key(run: &Path) -> io::Result<[u8; 32]> {
    use kuvo_store::files;

    let path = run.join("socket.key");
    let kept = || fs::read_to_string(&path).and_then(|value| parse_key(&value));
    if let Ok(key) = kept() {
        return Ok(key);
    }
    let key = random::<32>()?;
    let bytes = format!("{}\n", hex::encode(key));
    match files::write_new(&path, bytes.as_bytes()) {
        Ok(()) => Ok(key),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if let Ok(key) = kept() {
                return Ok(key);
            }
            // Listener::bind is called with the resident ownership guard held.
            files::write_atomic(&path, bytes.as_bytes())?;
            Ok(key)
        }
        Err(error) => Err(error),
    }
}

struct Input<R> {
    socket: R,
    pending: Bytes,
    eof: bool,
}

impl<R: AsyncRead + Unpin> Input<R> {
    fn new(socket: R) -> Self {
        Self {
            socket,
            pending: Bytes::new(),
            eof: false,
        }
    }

    async fn packet(&mut self) -> io::Result<Option<Bytes>> {
        if self.eof {
            return Ok(None);
        }
        let size = self.socket.read_u32().await? as usize;
        if size > PACKET {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid SSH pipe packet.",
            ));
        }
        if size == 0 {
            self.eof = true;
            return Ok(None);
        }
        let mut bytes = vec![0; size];
        self.socket.read_exact(&mut bytes).await?;
        Ok(Some(bytes.into()))
    }

    async fn take(&mut self) -> io::Result<Option<Bytes>> {
        if !self.pending.is_empty() {
            return Ok(Some(std::mem::take(&mut self.pending)));
        }
        self.packet().await
    }

    async fn line(&mut self, max: usize) -> io::Result<Value> {
        let mut line = Vec::new();
        while let Some(bytes) = self.take().await? {
            let newline = bytes.iter().position(|&byte| byte == b'\n');
            let count = newline.unwrap_or(bytes.len());
            if count > max.saturating_sub(line.len()) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "SSH request line is too large.",
                ));
            }
            line.extend_from_slice(&bytes[..count]);
            if newline.is_some() {
                self.pending = bytes.slice(count + 1..);
                return Ok(serde_json::from_slice(&line).unwrap_or(Value::Null));
            }
        }
        Ok(Value::Null)
    }

    async fn all(&mut self) -> io::Result<Value> {
        let mut all = Vec::new();
        while let Some(bytes) = self.take().await? {
            all.extend_from_slice(&bytes);
        }
        Ok(serde_json::from_slice(&all).unwrap_or(Value::Null))
    }

    async fn drain(&mut self) -> io::Result<()> {
        while self.take().await?.is_some() {}
        Ok(())
    }

    async fn left(&mut self, streaming: bool) -> io::Result<()> {
        self.drain().await?;
        if !streaming {
            // Framed stdin EOF does not close a finite request. A transport disconnect does.
            let _ = self.socket.read_u8().await;
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "SSH pipe disconnected.",
            ));
        }
        Ok(())
    }
}

async fn packet(write: &mut (impl AsyncWrite + Unpin), bytes: &[u8]) -> io::Result<()> {
    if bytes.is_empty() {
        write.write_u32(0).await?;
    }
    for part in bytes.chunks(PACKET) {
        write.write_u32(part.len() as u32).await?;
        write.write_all(part).await?;
    }
    write.flush().await
}

fn json_line(value: &Value) -> Vec<u8> {
    let mut bytes = value.to_string().into_bytes();
    bytes.push(b'\n');
    bytes
}

async fn say(output: &mut (impl AsyncWrite + Unpin), value: Value) -> io::Result<()> {
    packet(output, &json_line(&value)).await
}

struct Denied(u16, &'static str);

impl Denied {
    fn json(&self) -> Value {
        json!({"status": self.0, "routeVersion": ROUTE_VERSION, "body": json!({"message": self.1}).to_string()})
    }
}

enum OpeningError {
    Refused(Denied),
    Broken(io::Error),
}

impl From<Denied> for OpeningError {
    fn from(value: Denied) -> Self {
        Self::Refused(value)
    }
}

impl From<io::Error> for OpeningError {
    fn from(value: io::Error) -> Self {
        Self::Broken(value)
    }
}

async fn addressed(
    input: &mut Input<impl AsyncRead + Unpin>,
    output: &mut (impl AsyncWrite + Unpin),
    pairings: Arc<dyn Pairings>,
) -> Result<(String, Value, Pairing), OpeningError> {
    let opening = input.line(PACKET).await?;
    let computer = opening["computer"].as_str().filter(|key| {
        key.len() == 43
            && key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    });
    let hello = opening["hello"]
        .as_str()
        .filter(|hello| hello.encode_utf16().count() <= 4000);
    let (Some(computer), Some(hello)) = (computer, hello) else {
        return Err(Denied(
            400,
            "Send the computer’s public key and a device handshake.",
        )
        .into());
    };
    let secret = Zeroizing::new(
        pairings
            .identity(computer)
            .map_err(IdentityRefusal::refusal)?,
    );
    let invalid = || Denied(403, "Kuvo couldn’t authenticate the SSH request.");
    let mut noise = snow::Builder::new(NOISE.parse().expect("static Noise pattern"))
        .prologue(PROLOGUE)
        .and_then(|builder| builder.local_private_key(secret.as_ref()))
        .and_then(snow::Builder::build_responder)
        .map_err(|_| invalid())?;
    let message = URL_SAFE_NO_PAD
        .decode(hello.trim_end_matches('='))
        .map_err(|_| invalid())?;
    // Node's X25519 rejects non-contributory keys. The pure-Rust Noise
    // resolver leaves that check to its caller.
    let static_secret = x25519_dalek::StaticSecret::from(*secret);
    let contributes = |key| {
        static_secret
            .diffie_hellman(&x25519_dalek::PublicKey::from(key))
            .was_contributory()
    };
    if !message
        .get(..32)
        .and_then(|bytes| bytes.try_into().ok())
        .is_some_and(contributes)
    {
        return Err(invalid().into());
    }
    let mut payload = vec![0; message.len()];
    let count = noise
        .read_message(&message, &mut payload)
        .map_err(|_| invalid())?;
    let hello: Value = serde_json::from_slice(&payload[..count]).map_err(|_| invalid())?;
    if hello["v"] != 1 {
        return Err(Denied(400, "The SSH handshake is not supported.").into());
    }
    let key: [u8; 32] = noise
        .get_remote_static()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(invalid)?;
    if !contributes(key) {
        return Err(invalid().into());
    }
    let grant = pairings.paired(&key);
    let mut reply = [0; 128];
    let len = noise
        .write_message(
            json!({"allowed": grant.is_some()}).to_string().as_bytes(),
            &mut reply,
        )
        .map_err(|_| invalid())?;
    say(
        output,
        json!({"hello": URL_SAFE_NO_PAD.encode(&reply[..len])}),
    )
    .await?;
    let Some(grant) = grant else {
        return Err(Denied(
            403,
            "Pair this device before reaching this computer over SSH.",
        )
        .into());
    };

    let message = input.line(usize::MAX).await?;
    let Some(sealed) = message["request"].as_str() else {
        return Err(Denied(400, "Send the authenticated SSH request.").into());
    };
    let mut sealed = URL_SAFE_NO_PAD
        .decode(sealed.trim_end_matches('='))
        .map_err(|_| invalid())?;
    // This is the first initiator transport message, with nonce zero. Kuvo v1
    // carries the complete request in it, including bodies larger than 64 KiB.
    let (receive, mut unused) = noise.dangerously_get_raw_split();
    let receive = Zeroizing::new(receive);
    unused.zeroize();
    Aes256Gcm::new((&*receive).into())
        .decrypt_in_place(&Nonce::from([0; 12]), &[], &mut sealed)
        .map_err(|_| invalid())?;
    let given: Value = serde_json::from_slice(&sealed).map_err(|_| invalid())?;
    if !given.is_object() {
        return Err(Denied(400, "Send a routed request.").into());
    }
    let current = pairings
        .paired(&key)
        .filter(|current| current.id == grant.id)
        .ok_or(Denied(403, "This device’s pairing changed. Connect again."))?;
    // The provider also rejects a local removal or a newly disabled device connection.
    let computer = computer.to_owned();
    let target = computer.clone();
    let id = current.id.clone();
    let pairing = Pairing::verified(
        key,
        current.id,
        current.name,
        hello["client"] == "desktop",
        move || {
            pairings.identity(&target).is_ok()
                && pairings.paired(&key).is_some_and(|grant| grant.id == id)
        },
    );
    Ok((computer, given, pairing))
}

pub(crate) async fn answer(
    socket: LocalStream,
    key: [u8; 32],
    dispatcher: Dispatcher,
    pairings: Arc<dyn Pairings>,
) -> io::Result<()> {
    let (read, mut output) = tokio::io::split(socket);
    let mut input = Input::new(read);
    let hello = time::timeout(Duration::from_secs(10), input.packet())
        .await??
        .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "SSH pipe has no opening."))?;
    let command = serde_json::from_slice::<PipeHello>(&hello)?.accept(&key)?;
    packet(&mut output, ACCEPTED).await?;
    let (computer, caller, given, internal_path) = match command {
        Command::RouteTo => match addressed(&mut input, &mut output, pairings).await {
            Ok((computer, given, pairing)) => (
                Some(computer),
                Caller::Paired {
                    transport: Transport::AddressedSsh,
                    pairing,
                },
                given,
                false,
            ),
            Err(OpeningError::Broken(error)) => return Err(error),
            Err(OpeningError::Refused(error)) => {
                say(&mut output, error.json()).await?;
                return packet(&mut output, &[]).await;
            }
        },
        Command::Route {
            internal,
            internal_path,
        } => {
            let path = internal_path.filter(|path| internal && !path.is_empty());
            let is_alias = path.is_some();
            let given = if let Some(path) = path {
                // The CLI sends a framed EOF even for an alias. Consume it
                // before closing so a fast reply doesn't race an unread write.
                input.drain().await?;
                json!({"path": path})
            } else {
                input.all().await?
            };
            (
                None,
                Caller::Local(Transport::Ssh { internal }),
                given,
                is_alias,
            )
        }
    };

    let streaming = computer.is_some() && given["stream"] == true;
    let cancel = CancellationToken::new();
    let _drop = cancel.clone().drop_guard();
    let pairing = caller.pairing().cloned();
    let request = request(&given, caller);
    let snapshot = request.as_ref().is_ok_and(|request| {
        let Some((path, query)) = request.path.split_once('?') else {
            return false;
        };
        path == "kuvo/line"
            && form_urlencoded::parse(query.as_bytes())
                .find(|(name, _)| name == "once")
                .is_some_and(|(_, value)| value == "1")
    });
    let mode = if streaming {
        Output::Stream
    } else if snapshot {
        Output::All
    } else {
        Output::FirstState
    };
    let answer = Answer {
        computer,
        mode,
        alias: internal_path,
    };
    tokio::select! {
        result = input.left(streaming) => result?,
        () = revoked(pairing) => {},
        result = answer.send(request, dispatcher, cancel, &mut output) => result?,
    }
    packet(&mut output, &[]).await
}

async fn revoked(pairing: Option<Pairing>) {
    if let Some(pairing) = pairing {
        loop {
            time::sleep(Duration::from_secs(2)).await;
            if !pairing.is_current() {
                return;
            }
        }
    }
    std::future::pending().await
}

fn request(given: &Value, caller: Caller) -> Result<DispatchRequest, Value> {
    let addressed = matches!(caller.transport(), Transport::AddressedSsh);
    let mut headers = headers(&given["headers"])?;
    if !addressed {
        const FORWARDED: &[&str] = &[
            "x-opencode-directory",
            "x-kuvo-sign-in-client",
            "x-kuvo-vault-key",
            "x-kuvo-vault-proof",
            "x-kuvo-client-version",
            "x-kuvo-client-id",
            ROUTE_VERSION_HEADER,
        ];
        let removed: Vec<_> = headers
            .keys()
            .filter(|name| !FORWARDED.contains(&name.as_str()))
            .cloned()
            .collect();
        for name in removed {
            headers.remove(name);
        }
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    }
    let raw = given["path"].as_str().unwrap_or_default();
    let route = raw.to_owned();
    let raw_method = given["method"]
        .as_str()
        .unwrap_or("GET")
        .to_ascii_uppercase();
    let method = Method::from_bytes(raw_method.as_bytes())
        .map_err(|_| json!({"message": "Send a valid request method."}))?;
    let body = if let Some(encoded) = given["bodyBase64"].as_str().filter(|_| addressed) {
        STANDARD
            .decode(encoded)
            .or_else(|_| STANDARD_NO_PAD.decode(encoded))
            .or_else(|_| URL_SAFE_NO_PAD.decode(encoded))
            .unwrap_or_default()
    } else if let Some(text) = given["bodyText"].as_str() {
        text.as_bytes().to_owned()
    } else {
        given
            .get("body")
            .map_or_else(Vec::new, |body| body.to_string().into_bytes())
    };
    Ok(DispatchRequest {
        method,
        path: route,
        headers,
        body: body.into(),
        caller,
    })
}

fn headers(given: &Value) -> Result<HeaderMap, Value> {
    let mut headers = HeaderMap::new();
    for (name, value) in given.as_object().into_iter().flatten() {
        let name = name.to_ascii_lowercase();
        if name == ROUTE_VERSION_HEADER
            && let Some(refusal) =
                UnsupportedFormatVersion::for_route(Some(value.as_str().unwrap_or("")))
        {
            return Err(json!(refusal));
        }
        let invalid = || json!({"code": "invalidHeader", "message": format!("Invalid request header: {name}.")});
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| invalid())?;
        let values: Vec<_> = match value {
            Value::String(value) => vec![value.as_str()],
            Value::Array(values) => values
                .iter()
                .map(Value::as_str)
                .collect::<Option<_>>()
                .ok_or_else(invalid)?,
            _ => return Err(invalid()),
        };
        if headers.contains_key(&name) {
            if !headers
                .get_all(&name)
                .iter()
                .map(HeaderValue::as_bytes)
                .eq(values.iter().map(|value| value.as_bytes()))
            {
                return Err(
                    json!({"code": "conflictingHeader", "message": format!("Conflicting request header: {name}.")}),
                );
            }
        } else {
            for value in values {
                headers.append(&name, HeaderValue::from_str(value).map_err(|_| invalid())?);
            }
        }
    }
    Ok(headers)
}

enum Output {
    Stream,
    All,
    FirstState,
}

struct Answer {
    computer: Option<String>,
    mode: Output,
    alias: bool,
}

impl Answer {
    async fn send(
        self,
        request: Result<DispatchRequest, Value>,
        dispatcher: Dispatcher,
        cancel: CancellationToken,
        output: &mut (impl AsyncWrite + Unpin),
    ) -> io::Result<()> {
        let reply = match request {
            Ok(request) => dispatcher.dispatch(request, cancel).await,
            Err(error) => crate::Reply::json(StatusCode::BAD_REQUEST, error),
        };
        let ty = reply
            .headers
            .get(CONTENT_TYPE)
            .and_then(|ty| ty.to_str().ok())
            .unwrap_or_default();
        let mut head = json!({"status": reply.status.as_u16(), "routeVersion": ROUTE_VERSION});
        if let Some(computer) = self.computer {
            head["computer"] = json!(computer);
            head["type"] = json!(ty);
        }
        let mut body = reply.body.into_data_stream();
        use futures_util::StreamExt;
        if matches!(self.mode, Output::Stream) {
            say(output, head).await?;
            while let Some(chunk) = body.next().await {
                let bytes = chunk?;
                if !bytes.is_empty() {
                    packet(output, &bytes).await?;
                }
            }
        } else {
            let mut reply = Vec::new();
            while let Some(chunk) = body.next().await {
                let chunk = chunk?;
                if matches!(self.mode, Output::FirstState)
                    && ty.starts_with("application/x-ndjson")
                    && let Some(end) = chunk.iter().position(|&byte| byte == b'\n')
                {
                    reply.extend_from_slice(&chunk[..=end]);
                    head["body"] = json!(String::from_utf8_lossy(&reply));
                    return say(output, head).await;
                }
                reply.extend_from_slice(&chunk);
            }
            if self.alias {
                head["internal"] = true.into();
            }
            if let Ok(text) = std::str::from_utf8(&reply)
                && (ty.is_empty() || ty.starts_with("text/") || ty.contains("json"))
            {
                head["body"] = json!(text);
            } else {
                head["bodyBase64"] = json!(STANDARD.encode(&reply));
                head["type"] = json!(ty);
            }
            say(output, head).await?;
        }
        Ok(())
    }
}

/// Stdio entry for both SSH verbs. An unadmitted request waits through startup or restart.
pub async fn pipe(
    home: &Path,
    command: Command,
    mut stdin: impl AsyncRead + Unpin,
    mut stdout: impl AsyncWrite + Unpin,
) -> io::Result<()> {
    let buffered = match &command {
        Command::Route {
            internal_path: Some(_),
            internal: true,
        } => Some((Vec::new(), false)),
        Command::Route { .. } => {
            let mut bytes = Vec::new();
            stdin.read_to_end(&mut bytes).await?;
            Some(tool_call(bytes))
        }
        Command::RouteTo => None,
    };
    let replay = buffered.as_ref().is_some_and(|(_, replay)| *replay);
    let mut deadline = Instant::now() + STARTUP_WAIT;
    let run = home.join("run");
    let mut retrying = false;
    let mut pending = None;
    loop {
        let Some(socket) = open_pipe(&run, &command, deadline).await? else {
            let answer = pending.unwrap_or_else(|| {
                json_line(
                    &Denied(
                        503,
                        "Kuvo on this computer is starting. Connect again in a moment.",
                    )
                    .json(),
                )
            });
            stdout.write_all(&answer).await?;
            return stdout.flush().await;
        };
        let (read, mut write) = tokio::io::split(socket);
        let mut input = Input::new(read);
        let received = {
            let upload = async {
                if let Some((bytes, _)) = &buffered {
                    if !bytes.is_empty() {
                        packet(&mut write, bytes).await?;
                    }
                } else {
                    let mut buffer = vec![0; PACKET];
                    loop {
                        let size = stdin.read(&mut buffer).await?;
                        if size == 0 {
                            break;
                        }
                        packet(&mut write, &buffer[..size]).await?;
                    }
                }
                packet(&mut write, &[]).await
            };
            let download = async {
                let mut complete = Vec::new();
                while let Some(bytes) = input.packet().await? {
                    if replay {
                        complete.extend_from_slice(&bytes);
                    } else {
                        stdout.write_all(&bytes).await?;
                        stdout.flush().await?;
                    }
                }
                Ok::<_, io::Error>(complete)
            };
            tokio::pin!(upload, download);
            tokio::select! {
                result = &mut download => result,
                result = &mut upload => match result {
                    Ok(()) => download.await,
                    Err(error) => download.await.or(Err(error)),
                },
            }
        };
        match received {
            Ok(complete) if replay && retryable(&complete) => pending = Some(complete),
            Ok(complete) => {
                if replay {
                    stdout.write_all(&complete).await?;
                }
                return stdout.flush().await;
            }
            Err(error) if !replay => return Err(error),
            Err(_) => {}
        }
        if !retrying {
            deadline = Instant::now() + STARTUP_WAIT;
            retrying = true;
        }
        time::sleep_until((Instant::now() + RETRY).min(deadline)).await;
    }
}

async fn open_pipe(
    run: &Path,
    command: &Command,
    until: Instant,
) -> io::Result<Option<LocalStream>> {
    let folder = run.join("route");
    while Instant::now() < until {
        let opening = async {
            let key = tokio::fs::read_to_string(run.join("socket.key"))
                .await
                .and_then(|key| parse_key(&key))?;
            let endpoint = Endpoint::new(&folder.join("ssh"), &key, Channel::Ssh)?;
            let mut socket = endpoint.connect().await?;
            packet(
                &mut socket,
                &serde_json::to_vec(&PipeHello::new(command, &key))?,
            )
            .await?;
            if Input::new(&mut socket).packet().await?.as_deref() != Some(ACCEPTED) {
                return Err(io::Error::other("SSH pipe did not accept the command."));
            }
            Ok(socket)
        };
        match time::timeout_at(until.min(Instant::now() + Duration::from_secs(5)), opening).await {
            Ok(Ok(socket)) => return Ok(Some(socket)),
            Ok(Err(error)) if error.kind() == io::ErrorKind::PermissionDenied => return Ok(None),
            _ => time::sleep_until((Instant::now() + RETRY).min(until)).await,
        }
    }
    Ok(None)
}

fn retryable(reply: &[u8]) -> bool {
    let Ok(answer) = serde_json::from_slice::<Value>(reply) else {
        return false;
    };
    answer["status"] == 503
        && answer["body"]
            .as_str()
            .and_then(|body| serde_json::from_str::<Value>(body).ok())
            .is_some_and(|body| body["retryToolCall"] == true)
}

fn tool_call(bytes: Vec<u8>) -> (Vec<u8>, bool) {
    let Ok(mut request) = serde_json::from_slice::<Value>(&bytes) else {
        return (bytes, false);
    };
    if !matches!(
        request["path"].as_str(),
        Some(
            "kuvo/agent-tools"
                | "kuvo/work-queue/tools"
                | "kuvo/vault/tools"
                | "kuvo/sandbox/tools"
        )
    ) {
        return (bytes, false);
    }
    if request["method"] == "GET" {
        return (bytes, true);
    }
    if request["method"] != "POST" {
        return (bytes, false);
    }
    let Some(body) = request["body"].as_object_mut() else {
        return (bytes, false);
    };
    if read_tool(
        body.get("name").and_then(Value::as_str),
        body.get("arguments"),
    ) {
        return (bytes, true);
    }
    if let Some(operation) = body.get("operation") {
        let safe = operation.as_str().is_some_and(|id| {
            (1..=80).contains(&id.len())
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
        });
        return (bytes, safe);
    }
    body.insert("operation".into(), json!(uuid::Uuid::new_v4()));
    (request.to_string().into_bytes(), true)
}

fn read_tool(name: Option<&str>, args: Option<&Value>) -> bool {
    match name {
        Some(
            "list_agents" | "read_conversation" | "list_sessions" | "list_models" | "list_projects"
            | "plan_limits" | "list_wake_ups" | "show_in_panel" | "show_file" | "show_widget"
            | "mark_final" | "queue_list" | "queue_show" | "vault_items",
        ) => true,
        Some("queue_labels") => !["add", "rename", "recolor", "remove"].iter().any(|key| {
            args.and_then(|args| args[*key].as_str())
                .is_some_and(|arg| !arg.trim().is_empty())
        }),
        _ => false,
    }
}
