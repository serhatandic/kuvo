use super::{
    HANDSHAKE, PROTOCOL, RUST_HOLDER, SAFE_INTEGER, endpoint, invalid,
    protocol::{self, Hello, Input, Launch, Metadata, Ready, Spec},
    valid_id,
};
use super::{HoldKind, Output, Record, Welcome};
use kuvo_os::{
    ipc::LocalStream,
    platform::Environment,
    process::{ProcessState, Spawn, Stdio},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs, io,
    io::{BufRead, Write},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    io::{BufReader, ReadHalf, WriteHalf},
    time::timeout,
};

#[derive(Clone)]
pub struct Directory {
    folder: PathBuf,
}

impl Directory {
    pub fn for_home(home: &kuvo_store::home::Home) -> Self {
        Self {
            folder: home.run().join("agents/hold"),
        }
    }

    /// Whether this path can support the holder transport. The caller can choose an
    /// unheld process at an old Unix home that exceeds the socket path limit.
    pub fn available(&self) -> bool {
        #[cfg(unix)]
        {
            let capacity = if cfg!(target_os = "macos") { 104 } else { 108 };
            self.folder.as_os_str().as_encoded_bytes().len() + 18 < capacity
        }
        #[cfg(windows)]
        true
    }

    pub async fn records(&self) -> io::Result<Vec<Record>> {
        let folder = self.folder.clone();
        tokio::task::spawn_blocking(move || records(&folder)).await?
    }

    /// Ask every agent held here to stop, as the box stops for good (no resident comes after it to
    /// take them up): each holder stops its agent as on SIGTERM, and goes once it has. Nothing waits.
    pub async fn stop_all(&self) -> io::Result<()> {
        for record in self.records().await? {
            if record.identity.process.state() != ProcessState::Live {
                continue;
            }
            // (Any system: a Windows holder has no SIGTERM to hear.)
            kuvo_store::files::write_atomic(&stop_request(&self.folder, &record.id), b"")?;
            #[cfg(unix)]
            if let Some(holder) = kuvo_os::process::AdoptedProcess::open(
                record.pid,
                Some(&record.identity.process.start_identity),
                |_| true,
                None,
            )? {
                holder.terminate()?;
            }
        }
        Ok(())
    }

    pub async fn start(&self, binary: &Path, options: Start) -> io::Result<Controller> {
        if !self.available() {
            return Err(invalid("The holder socket path is too long"));
        }

        let mut random = [0; 30];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        let id = hex::encode(&random[..6]);
        let key = hex::encode(&random[6..]);
        let Start {
            command,
            cwd,
            kind,
            env,
            meta,
            limits,
            portable_requests,
        } = options;
        let launch = Launch {
            key: key.clone(),
            spec: Spec {
                command,
                cwd,
                bridge: Some(std::process::id()),
                bun: env
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case("BUN_BE_BUN"))
                    .map(|(_, value)| value.to_string_lossy().into_owned()),
                meta,
                journal_bytes: limits.journal_bytes,
                linger: limits.linger.map(as_millis),
                orphan: limits.orphan.map(as_millis),
                portable_requests,
            },
        };
        let payload = serde_json::to_vec(&launch)?;
        let recovery_name = format!("holder-{id}");
        let mut spawn = Spawn::new(binary, env);
        spawn.args = vec![
            "hold".into(),
            self.folder.as_os_str().into(),
            id.clone().into(),
            kind.name().into(),
        ];
        spawn.stdin = Stdio::Pipe;
        spawn.stdout = Stdio::Pipe;
        let ready = timeout(
            HANDSHAKE,
            tokio::task::spawn_blocking(move || -> io::Result<Ready> {
                let mut worker = spawn.spawn_recoverable(&recovery_name)?;
                worker
                    .stdin
                    .take()
                    .expect("requested bootstrap pipe")
                    .write_all(&payload)?;
                let stdout = worker.stdout.take().expect("requested readiness pipe");
                let mut line = String::new();
                io::BufReader::new(stdout).read_line(&mut line)?;
                let ready: Ready = serde_json::from_str(&line)
                    .map_err(|_| invalid("Holder stopped before it was ready"))?;
                if ready.ready != PROTOCOL || ready.pid != worker.identity().pid {
                    return Err(invalid("Unexpected holder readiness"));
                }
                worker.leave_running_on_drop();
                // Reap the child even when its controller detaches. This OS thread
                // belongs to the process, so joining it never holds up resident exit.
                std::thread::Builder::new()
                    .name("reap-holder".into())
                    .spawn(move || {
                        let _ = worker.wait();
                    })?;
                Ok(ready)
            }),
        )
        .await???;

        connect(&self.folder, &id, &key, ready.pid, 1, None, false).await
    }

    /// Use the next generation and demand complete replay for an active turn.
    /// Adapter-specific compatibility is decided by the adapter before it claims the record.
    pub async fn adopt(&self, record: &Record, replay: Replay) -> io::Result<Controller> {
        if record.proto != PROTOCOL
            || record.holder != RUST_HOLDER
            || record.identity.process.pid != record.pid
            || record.identity.process.state() != ProcessState::Live
        {
            return Err(invalid("The recorded Rust holder is unavailable"));
        }
        let (from, complete) = match replay {
            Replay::Idle => (Some(SAFE_INTEGER as i64), false),
            Replay::Acknowledged => (None, false),
            Replay::Complete(from) => (Some(from.min(SAFE_INTEGER) as i64), true),
        };
        let generation = record
            .generation
            .checked_add(1)
            .ok_or_else(|| invalid("Invalid recorded holder generation"))?;
        connect(
            &self.folder,
            &record.id,
            &record.key,
            record.pid,
            generation,
            from,
            complete,
        )
        .await
    }
}

fn as_millis(duration: Duration) -> u64 {
    duration.as_millis().min(SAFE_INTEGER as u128) as u64
}

/// Where a holder is asked to stop: beside its record.
pub(super) fn stop_request(folder: &Path, id: &str) -> PathBuf {
    folder.join(format!("{id}.stop"))
}

fn records(folder: &Path) -> io::Result<Vec<Record>> {
    let entries = match fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut records = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(id) = name
            .to_str()
            .and_then(|name| name.strip_suffix(".json"))
            .filter(|id| valid_id(id))
        else {
            continue;
        };
        if !entry.file_type()?.is_file() {
            continue;
        }
        let Ok(bytes) = fs::read(entry.path()) else {
            continue;
        };
        let Ok(record) = serde_json::from_slice::<Record>(&bytes) else {
            continue;
        };
        if record.proto != PROTOCOL
            || record.holder != RUST_HOLDER
            || record.id != id
            || record.identity.process.pid != record.pid
        {
            continue;
        }
        match record.identity.process.state() {
            ProcessState::Live => records.push(record),
            ProcessState::Unknown => {}
            ProcessState::Stale =>
            {
                #[cfg(unix)]
                for suffix in [".json", ".journal", ".sock"] {
                    let _ = fs::remove_file(folder.join(format!("{id}{suffix}")));
                }
            }
        }
    }
    records.sort_unstable_by(|a, b| a.id.cmp(&b.id));
    Ok(records)
}

pub struct Start {
    pub command: Vec<PathBuf>,
    pub cwd: Option<PathBuf>,
    pub env: Environment,
    pub kind: HoldKind,
    pub meta: Metadata,
    pub limits: Limits,
    /// When set, transfer waits until all other agent requests are settled.
    pub portable_requests: Option<Vec<String>>,
}

#[derive(Default)]
pub struct Limits {
    pub journal_bytes: Option<u64>,
    pub linger: Option<Duration>,
    pub orphan: Option<Duration>,
}

#[derive(Clone, Copy)]
pub enum Replay {
    Idle,
    Acknowledged,
    Complete(u64),
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnNote {
    pub session: String,
    pub from: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<RequestId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<Value>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum RequestId {
    Number(i64),
    Text(String),
}

impl Record {
    pub fn turns(&self) -> serde_json::Result<BTreeMap<String, TurnNote>> {
        self.meta
            .get("turns")
            .cloned()
            .map_or_else(|| Ok(BTreeMap::new()), serde_json::from_value)
    }
}

/// Dropping the connection leaves the agent running for the next resident.
pub struct Controller {
    pub welcome: Welcome,
    pub input: ControllerInput,
    pub output: ControllerOutput,
}

pub struct ControllerInput(WriteHalf<LocalStream>);
pub struct ControllerOutput(BufReader<ReadHalf<LocalStream>>);

impl ControllerInput {
    pub async fn write(&mut self, line: impl Into<String>) -> io::Result<()> {
        self.send(Input {
            line: Some(line.into()),
            ..Input::default()
        })
        .await
    }

    pub async fn acknowledge(&mut self, sequence: u64) -> io::Result<()> {
        self.send(Input {
            ack: Some(sequence),
            ..Input::default()
        })
        .await
    }

    pub async fn note(&mut self, meta: Metadata) -> io::Result<()> {
        self.send(Input {
            meta: Some(meta),
            ..Input::default()
        })
        .await
    }

    /// Reserve the replay range for the next resident. Read the Prepared output
    /// before allowing the old resident to exit.
    pub async fn prepare(&mut self, from: u64) -> io::Result<()> {
        self.send(Input {
            prepare: Some(from.min(SAFE_INTEGER) as i64),
            ..Input::default()
        })
        .await
    }

    pub async fn unpin(&mut self) -> io::Result<()> {
        self.send(Input {
            unpin: true,
            ..Input::default()
        })
        .await
    }

    pub async fn end(&mut self) -> io::Result<()> {
        self.send(Input {
            end: true,
            ..Input::default()
        })
        .await
    }

    pub async fn signal(&mut self, signal: &str) -> io::Result<()> {
        self.send(Input {
            signal: Some(signal.into()),
            ..Input::default()
        })
        .await
    }

    pub async fn bye(&mut self) -> io::Result<()> {
        self.send(Input {
            bye: true,
            ..Input::default()
        })
        .await
    }

    async fn send(&mut self, input: Input) -> io::Result<()> {
        protocol::write(&mut self.0, &input).await
    }
}

impl ControllerOutput {
    pub async fn next(&mut self) -> io::Result<Output> {
        protocol::read(&mut self.0, protocol::MAX_FRAME).await
    }
}

async fn connect(
    folder: &Path,
    id: &str,
    key: &str,
    pid: u32,
    generation: u64,
    from: Option<i64>,
    complete: bool,
) -> io::Result<Controller> {
    timeout(HANDSHAKE, async {
        let stream = endpoint(folder, id)?.connect().await?;
        let (reader, mut writer) = tokio::io::split(stream);
        let mut reader = BufReader::new(reader);
        protocol::write(
            &mut writer,
            &Hello {
                hello: PROTOCOL.into(),
                key: key.into(),
                generation,
                from,
                complete,
            },
        )
        .await?;
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Answer {
            Welcome(Welcome),
            Refused { refused: String },
        }
        let welcome = match protocol::read(&mut reader, protocol::MAX_FRAME).await? {
            Answer::Welcome(welcome) => welcome,
            Answer::Refused { refused } => {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, refused));
            }
        };
        if welcome.hello != PROTOCOL
            || welcome.pid != pid
            || welcome.generation != generation
            || (complete && welcome.gap)
        {
            return Err(invalid("Unexpected holder response"));
        }
        Ok(Controller {
            welcome,
            input: ControllerInput(writer),
            output: ControllerOutput(reader),
        })
    })
    .await?
}
