//! A machine's box over SSH (PROTOCOL.md, "How a client connects"). `box route-to` verifies a
//! paired device and the expected computer via Noise IK, then sends the same request as the relay.
//! The legacy `box route` is used for standalone SSH aliases and polls for followed routes.
//!
//! It runs as the Mac app runs it: with the machine's own engine runtime (the newest it has), its
//! home `~/.kuvo-code`, and the script sent as base64 for `sh`, whatever the login shell is there.

use std::io::{self, BufRead, BufReader, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use base64::Engine as _;
use serde::Deserialize;

use crate::error::{Error, Refusal, Result};
use crate::link::{Identity, keys};
use crate::transport::{check_route_version, Closer, Request, Response, Stream, Transport, ROUTE_VERSION_HEADER};

/// One machine, by what `ssh` connects to (`me@devbox`, a `Host` in `~/.ssh/config`).
#[derive(Debug, Clone)]
pub struct SshBox {
    wsl_distro: Option<String>,
    destination: String,
    /// The `ssh` to run: `ssh` on the PATH unless a test names a stand-in.
    program: PathBuf,
    /// The box's home in the machine's home folder.
    root: String,
    computer: Option<(String, Identity)>,
    app: String,
    known_hosts: Option<PathBuf>,
    /// A folder of this user's alone where connections are shared (`ControlPath`), so each request
    /// costs a round trip rather than a login. The directory is private to this account.
    sharing: Option<PathBuf>,
}

impl SshBox {
    /// Connections to it are shared, as the Mac app shares them ([`app_sharing_folder`]).
    pub fn new(destination: impl Into<String>) -> SshBox {
        SshBox { wsl_distro: None, destination: destination.into(), program: PathBuf::from("ssh"), root: ".kuvo-code".into(), computer: None, app: option_env!("KUVO_APP_VERSION").unwrap_or("").into(), known_hosts: None, sharing: app_sharing_folder() }
    }

    /// Ask only the enrolled computer with this public key, even if its SSH address has changed.
    pub fn for_computer(mut self, key: impl Into<String>, device: Identity) -> SshBox {
        self.computer = Some((key.into(), device));
        self
    }

    pub(crate) fn wsl_distro(mut self, distro: Option<String>) -> SshBox {
        self.wsl_distro = distro;
        self
    }

    pub(crate) fn known_hosts(mut self, from_add: Option<PathBuf>) -> SshBox {
        self.known_hosts = from_add;
        self
    }

    pub(crate) fn app_version(mut self, app: &str) -> SshBox {
        self.app = app.to_owned();
        self
    }

    /// Runs `program` in place of `ssh` (a test's stand-in, or an `ssh` at a path of its own).
    pub fn program(mut self, program: impl Into<PathBuf>) -> SshBox {
        self.program = program.into();
        self
    }

    /// Shares one connection per machine through sockets in `folder` (none: each request logs in on
    /// its own). Taken only while it's this user's alone.
    pub fn sharing_connections_in(mut self, folder: Option<PathBuf>) -> SshBox {
        self.sharing = folder;
        self
    }

    pub fn destination(&self) -> &str {
        &self.destination
    }

    /// For every connection: never a password prompt (there's no terminal to type it in), a time
    /// to connect, and a check that notices a machine gone away. A command or a terminal of the
    /// machine's own in `~/.ssh/config` is left out: ssh refuses a command beside one.
    fn options(&self) -> Vec<String> {
        let timeout = if self.computer.is_some() { "ConnectTimeout=3" } else { "ConnectTimeout=15" };
        let mut options: Vec<String> = [
            "-o", "BatchMode=yes",
            "-o", "RemoteCommand=none",
            "-o", "RequestTTY=no",
            "-o", timeout,
            "-o", "ServerAliveInterval=15",
            "-o", "ServerAliveCountMax=3",
        ]
        .iter()
        .map(|option| option.to_string())
        .collect();
        if self.computer.is_some() {
            options.extend(["-o".into(), "StrictHostKeyChecking=yes".into()]);
            if let Some(file) = self.known_hosts.as_ref().and_then(|file| file.to_str()).filter(|file| !file.contains(['\n', '\r', '"'])) {
                options.extend(["-o".into(), format!("GlobalKnownHostsFile=\"{file}\" /etc/ssh/ssh_known_hosts /etc/ssh/ssh_known_hosts2")]);
            }
        }
        match self.sharing.as_deref().filter(|folder| is_own_private_folder(folder)) {
            Some(folder) => {
                options.extend(["-o".into(), "ControlMaster=auto".into(), "-o".into(), format!("ControlPath={}/%C", folder.display()), "-o".into(), "ControlPersist=120".into()]);
            }
            None => options.extend(["-o", "ControlMaster=no", "-o", "ControlPath=none"].iter().map(|option| option.to_string())),
        }
        options
    }

    /// The script that answers one request there: by its keeper (`kuvo-start route`/`route-to`),
    /// which runs it on the engine it picks itself; a keeper from before that verb, by the newest
    /// engine there, as Kuvo did before.
    fn script(&self) -> String {
        format!(
            r#"K="$HOME/{root}"
[ -f "$K/bin/kuvo-link.js" ] || {{ echo "Kuvo Code isn't set up on this machine." >&2; exit 3; }}
if grep -q '^route|route-to)' "$K/bin/kuvo-start" 2>/dev/null; then exec sh "$K/bin/kuvo-start" {verb}; fi
E=$(for d in "$K"/engine/*/; do [ -x "${{d}}opencode" ] && basename "$d"; done | sort -t. -k1,1n -k2,2n -k3,3n | tail -n 1)
[ -n "$E" ] || {{ echo "Kuvo Code's engine isn't on this machine." >&2; exit 3; }}
export XDG_DATA_HOME="$K/data" XDG_CONFIG_HOME="$K/config" XDG_STATE_HOME="$K/state" XDG_CACHE_HOME="$K/cache" KUVO_INTEGRATIONS="$K/integrations" TMPDIR="$K/tmp"
cd "$K/workspace" 2>/dev/null || cd "$K" || exit 1
export KUVO_HOME="$K" BUN_BE_BUN=1 BUN_RUNTIME_TRANSPILER_CACHE_PATH=0
exec "$K/engine/$E/opencode" --config=/dev/null --no-env-file "$K/bin/kuvo-link.js" box {verb}"#,
            root = self.root,
            verb = if self.computer.is_some() { "route-to" } else { "route" },
        )
    }

    /// The command line for the machine's login shell that runs `script` with `sh`: as base64, which
    /// no shell changes (fish reads backslashes in quotes its own way). Without a working `base64`
    /// there it fails (127) rather than running nothing and saying it's done.
    pub fn command_line(script: &str) -> String {
        let encoded = base64::engine::general_purpose::STANDARD.encode(script);
        format!("exec sh -c 'kuvo_script=$(printf %s {encoded} | base64 -d) || exit 127; eval \"$kuvo_script\"'")
    }

    fn entry(&self) -> Result<String> {
        let Some(distro) = &self.wsl_distro else { return Ok(Self::command_line(&self.script())) };
        if distro.len() > 80 || !distro.starts_with(|c: char| c.is_ascii_alphanumeric())
            || !distro.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
        {
            return Err(Error::BadAnswer(format!("Kuvo Code on {} didn't answer as it should.", self.destination)));
        }
        let verb = if self.computer.is_some() { "route-to" } else { "route" };
        Ok(format!("wsl.exe -d {distro} --cd ~ --exec /bin/sh .kuvo-code/bin/kuvo-start {verb}"))
    }

    fn start(&self, input: &[u8], keep_input: bool, hold: &dyn Fn(Closer) -> bool) -> Result<(Running, BufReader<ChildStdout>, Closer)> {
        let mut command = Command::new(&self.program);
        // (`--`: a machine's name is never read as an option.)
        command.args(self.options()).arg("--").arg(&self.destination).arg(self.entry()?);
        // Nothing of a Kuvo Code process's own goes along (ssh sends a machine only `LANG` and
        // `LC_*`, but a stand-in runs here).
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("KUVO_") {
                command.env_remove(name);
            }
        }
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| Error::Unreachable(format!("Couldn't start ssh to reach {} ({}).", self.destination, error.kind())))?;
        // What ends it from another thread: `ssh` itself, this one we started, by its process id,
        // only while it hasn't been waited for.
        let pid = Arc::new(Mutex::new(Some(child.id())));
        let ender = pid.clone();
        let closer = Closer::new(move || {
            if let Some(pid) = *ender.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) {
                end(pid);
            }
        });
        if !hold(closer.clone()) {
            closer.close();
        }
        // What it says, to its end (it closes them as it exits); then it's waited for and let go of
        // under the one lock, so the closer never signals an id that's been waited for.
        let mut stderr = child.stderr.take();
        let stderr = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(stderr) = stderr.as_mut() {
                let _ = stderr.read_to_end(&mut bytes);
            }
            bytes
        });
        let mut source = BufReader::new(child.stdout.take().expect("ssh stdout is piped"));
        let mut running = Running { child, pid, stderr: Some(stderr), destination: self.destination.clone() };
        if let Some((key, device)) = &self.computer {
            self.start_paired(key, device, input, &mut running, &mut source)?;
        } else {
            // The legacy command reads to EOF and reports an SSH failure with its exit status.
            let _ = running.write(input);
        }
        if !keep_input {
            drop(running.child.stdin.take());
        }
        Ok((running, source, closer))
    }

    fn start_paired(&self, key: &str, device: &Identity, input: &[u8], running: &mut Running, source: &mut BufReader<ChildStdout>) -> Result<()> {
        let Some(public) = keys::decode_exact(key) else { return Err(Error::BadAnswer("Invalid computer key.".into())) };
        let mut handshake = device.initiate(public, b"kuvo-ssh v1");
        let hello = serde_json::json!({"v": 1, "client": "desktop", "app": self.app});
        let hello = handshake.write_message_1(hello.to_string().as_bytes())
            .map_err(|_| Error::BadAnswer("Invalid computer key.".into()))?;
        let hello = keys::encode(&hello);
        running.write_line(serde_json::json!({"computer": key, "hello": hello}))?;
        let mut line = String::new();
        source.take(16_384).read_line(&mut line).map_err(|error| running.unreachable(error))?;
        let reply: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
        let opened = reply["hello"].as_str().and_then(keys::decode).and_then(|hello| handshake.read_message_2(&hello).ok());
        let Some((proof, mut secure)) = opened else {
            return Err(Error::Unreachable(format!("{} didn't verify the selected computer over SSH.", self.destination)));
        };
        let allowed: serde_json::Value = serde_json::from_slice(&proof).unwrap_or_default();
        if allowed["allowed"] != true {
            if allowed["reason"] == crate::link::client::words::CLIENT_UPDATE_REQUIRED {
                return Err(Error::Refused(Refusal::from_answer(426, &proof)));
            }
            return Err(crate::link::client::unpaired());
        }
        running.write_line(serde_json::json!({"request": keys::encode(&secure.seal(input))}))
    }

    fn run(&self, input: &[u8], hold: &dyn Fn(Closer) -> bool) -> Result<String> {
        let (mut running, mut stdout, _) = self.start(input, false, hold)?;
        let mut output = Vec::new();
        let read = stdout.read_to_end(&mut output);
        running.finish()?;
        read.map_err(|error| Error::Unreachable(format!("Couldn't read SSH's answer from {} ({error}).", self.destination)))?;
        Ok(String::from_utf8_lossy(&output).into_owned())
    }
}

struct Running {
    child: Child,
    pid: Arc<Mutex<Option<u32>>>,
    stderr: Option<JoinHandle<Vec<u8>>>,
    destination: String,
}

impl Running {
    fn unreachable(&self, error: io::Error) -> Error {
        Error::Unreachable(format!("Couldn't reach {} ({error}).", self.destination))
    }

    fn write(&mut self, input: &[u8]) -> Result<()> {
        let stdin = self.child.stdin.as_mut().expect("ssh stdin is piped");
        stdin.write_all(input).map_err(|error| self.unreachable(error))
    }

    fn write_line(&mut self, value: serde_json::Value) -> Result<()> {
        let mut line = value.to_string();
        line.push('\n');
        self.write(line.as_bytes())
    }

    fn wait(&mut self, stop: bool) -> io::Result<ExitStatus> {
        let mut pid = self.pid.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if stop && pid.is_some() {
            let _ = self.child.kill();
        }
        let status = self.child.wait();
        *pid = None;
        status
    }

    fn finish(&mut self) -> Result<()> {
        let status = self.wait(false).map_err(|error| Error::Unreachable(format!("Couldn't reach {} ({}).", self.destination, error.kind())))?;
        let stderr = self.stderr.take().and_then(|reading| reading.join().ok()).unwrap_or_default();
        if !status.success() {
            let said = String::from_utf8_lossy(&stderr);
            let said = said.lines().map(str::trim).rfind(|line| !line.is_empty()).unwrap_or("").chars().take(300).collect::<String>();
            return Err(match status.code() {
                Some(255) | None => Error::Unreachable(format!("Couldn't reach {}.{}", self.destination, if said.is_empty() { String::new() } else { format!(" {said}") })),
                Some(_) if !said.is_empty() => Error::Unreachable(said),
                Some(code) => Error::Unreachable(format!("Kuvo Code on {} didn't answer ({code}).", self.destination)),
            });
        }
        Ok(())
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.wait(true);
    }
}

/// What `box route` prints: its status, and its body as text or (bytes) base64.
#[derive(Deserialize)]
struct Routed {
    status: u16,
    computer: Option<String>,
    #[serde(default, rename = "routeVersion", deserialize_with = "present_version")]
    route_version: Option<serde_json::Value>,
    body: Option<String>,
    #[serde(rename = "bodyBase64")]
    body_base64: Option<String>,
    #[serde(rename = "type")]
    content_type: Option<String>,
}

fn present_version<'de, D: serde::Deserializer<'de>>(decoder: D) -> std::result::Result<Option<serde_json::Value>, D::Error> {
    serde_json::Value::deserialize(decoder).map(Some)
}

impl SshBox {
    fn request(&self, request: &Request, stream: bool) -> Vec<u8> {
        let mut asked = serde_json::json!({ "method": request.method.to_ascii_uppercase(), "path": request.path.trim_start_matches('/') });
        if stream {
            asked["stream"] = true.into();
        }
        if let Some(body) = &request.body {
            asked["body"] = body.clone();
        }
        // Addressed requests carry the same headers as relay requests. Keep the legacy command's
        // smaller header set for standalone aliases.
        let mut headers: serde_json::Map<String, serde_json::Value> = request
            .wire_headers()
            .filter(|(name, _)| self.computer.is_some() || matches!(*name, "x-opencode-directory" | "x-kuvo-sign-in-client" | "x-kuvo-client-id" | ROUTE_VERSION_HEADER))
            .map(|(name, value)| (name.to_owned(), serde_json::Value::String(value.to_owned())))
            .collect();
        if !self.app.is_empty() {
            headers.insert("x-kuvo-client-version".into(), self.app.clone().into());
        }
        if !headers.is_empty() {
            asked["headers"] = serde_json::Value::Object(headers);
        }
        let mut bytes = serde_json::to_vec(&asked).unwrap_or_default();
        bytes.push(b'\n');
        bytes
    }

    fn answer(&self, line: &str) -> Result<Routed> {
        let routed: Routed = serde_json::from_str(line)
            .map_err(|_| Error::BadAnswer(format!("Kuvo Code on {} didn't answer as it should.", self.destination)))?;
        check_route_version(routed.route_version.as_ref().map(|value| value.as_str().unwrap_or("")))?;
        if let Some((key, _)) = &self.computer {
            if routed.computer.as_ref() != Some(key) {
                return Err(Error::Unreachable(format!("{} didn't verify the selected computer over SSH.", self.destination)));
            }
        }
        Ok(routed)
    }

    fn probe_ssh(&self, hold: &dyn Fn(Closer) -> bool) -> Result<()> {
        let (finished, timeout) = mpsc::channel();
        let timer = Mutex::new(Some(timeout));
        let reply = self.route(&Request::get("kuvo/info"), &|closer| {
            if !hold(closer.clone()) {
                return false;
            }
            if let Some(timeout) = timer.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take() {
                std::thread::spawn(move || {
                    if timeout.recv_timeout(Duration::from_secs(5)) == Err(mpsc::RecvTimeoutError::Timeout) {
                        closer.close();
                    }
                });
            }
            true
        });
        let _ = finished.send(());
        reply?.verified()
    }

    fn route(&self, request: &Request, hold: &dyn Fn(Closer) -> bool) -> Result<Response> {
        let output = self.run(&self.request(request, false), hold)?;
        let last = output.lines().map(str::trim).rfind(|line| !line.is_empty()).unwrap_or("");
        let routed = self.answer(last)?;
        let body = match (routed.body_base64, routed.body) {
            (Some(encoded), _) => base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|_| Error::BadAnswer(format!("Kuvo Code on {} sent bytes that couldn't be read.", self.destination)))?,
            (None, Some(text)) => text.into_bytes(),
            (None, None) => Vec::new(),
        };
        Ok(Response { status: routed.status, content_type: routed.content_type.filter(|kind| !kind.is_empty()).or_else(|| Some("application/json".into())), body })
    }

    fn follow(&self, request: &Request, hold: &dyn Fn(Closer) -> bool) -> Result<Stream> {
        let (mut running, mut source, closer) = self.start(&self.request(request, true), true, hold)?;
        let mut line = String::new();
        (&mut source).take(16_384).read_line(&mut line)
            .map_err(|error| Error::Unreachable(format!("Couldn't read SSH's answer from {} ({error}).", self.destination)))?;
        if line.is_empty() {
            running.finish()?;
        }
        let routed = self.answer(&line)?;
        let reader = Box::new(Streaming { source, running });
        Ok(Stream { status: routed.status, content_type: routed.content_type.filter(|kind| !kind.is_empty()), reader, closer })
    }
}

struct Streaming {
    source: BufReader<ChildStdout>,
    running: Running,
}

impl Read for Streaming {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let count = self.source.read(bytes)?;
        if count == 0 && !bytes.is_empty() {
            self.running.finish().map_err(io::Error::other)?;
        }
        Ok(count)
    }
}

impl BufRead for Streaming {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        let bytes = self.source.fill_buf()?;
        if bytes.is_empty() {
            self.running.finish().map_err(io::Error::other)?;
        }
        Ok(bytes)
    }

    fn consume(&mut self, count: usize) {
        self.source.consume(count);
    }
}

impl Transport for SshBox {
    fn send(&self, request: &Request) -> Result<Response> {
        self.route(request, &|_| true)
    }

    fn probe(&self, hold: &dyn Fn(Closer) -> bool) -> Result<()> {
        self.probe_ssh(hold)
    }

    fn open(&self, request: &Request, hold: &dyn Fn(Closer) -> bool) -> Result<Stream> {
        if self.computer.is_some() {
            return self.follow(request, hold);
        }
        let response = self.route(request, hold)?;
        Ok(Stream { status: response.status, content_type: response.content_type, reader: Box::new(Cursor::new(response.body)), closer: Closer::nothing() })
    }

    fn streams(&self) -> bool {
        self.computer.is_some()
    }

    /// `box route` answers one request at a time: it can't carry a socket.
    fn socket(&self, _request: &Request) -> Result<crate::transport::Socket> {
        Err(crate::transport::sockets_not_here(format!("Terminals on {} aren’t available over SSH yet.", self.destination)))
    }

    fn describe(&self) -> String {
        self.destination.clone()
    }
}

/// Whether `folder` is a folder of this user's that no one else can reach: one another account
/// made first could hold a socket of its own, which ssh would hand what it sends.
#[cfg(unix)]
fn is_own_private_folder(folder: &Path) -> bool {
    use std::os::unix::{ffi::OsStrExt, fs::{DirBuilderExt, MetadataExt}};
    // OpenSSH appends a temporary suffix to the 40-character %C expansion.
    if folder.as_os_str().as_bytes().len() > 45 { return false; }
    // Made private in one step (a umask only takes bits away).
    let _ = std::fs::DirBuilder::new().mode(0o700).create(folder);
    let Ok(metadata) = std::fs::symlink_metadata(folder) else { return false };
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    metadata.is_dir() && metadata.uid() == uid && metadata.mode() & 0o077 == 0
}

#[cfg(not(unix))]
fn is_own_private_folder(_folder: &Path) -> bool {
    false
}

/// The desktop app's private SSH connection directory.
#[cfg(unix)]
pub fn app_sharing_folder() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).filter(|home| home.is_absolute()).map(|home| home.join(".kuvo-ssh-ctl"))
}

#[cfg(not(unix))]
pub fn app_sharing_folder() -> Option<PathBuf> {
    None
}

/// Asks process `pid` (an `ssh` this client started, still running) to end.
#[cfg(unix)]
fn end(pid: u32) {
    if let Ok(pid) = libc::pid_t::try_from(pid) {
        // SAFETY: a signal to a child of ours not yet waited for.
        unsafe { libc::kill(pid, libc::SIGTERM) };
    }
}

#[cfg(not(unix))]
fn end(_pid: u32) {}
