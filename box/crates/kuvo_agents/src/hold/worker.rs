use super::{
    HANDSHAKE, PROTOCOL, RUST_HOLDER, SAFE_INTEGER, endpoint, invalid,
    journal::Journal,
    protocol::{self, Exit, Hello, Identity, Input, Launch, Output, Ready, Record, Welcome},
};
use kuvo_os::{
    ipc::{Listener, LocalStream},
    platform::{Environment, which},
    process::{ChildGroup, GroupSignal, Spawn, Stdio},
};
use std::{
    collections::VecDeque,
    fs,
    io::{self, BufRead, Read, Write},
    path::{Path, PathBuf},
    process::ExitStatus,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use tokio::{
    io::{BufReader, ReadHalf, WriteHalf},
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::{Instant, timeout},
};

const HUNG: Duration = Duration::from_secs(60);
const WAIT_FOR_CONTROLLER: Duration = Duration::from_secs(15 * 60);

pub(super) async fn run(
    folder: &Path,
    id: &str,
    kind: protocol::HoldKind,
    boot: Launch,
) -> io::Result<()> {
    #[cfg(unix)]
    {
        match fs::symlink_metadata(folder) {
            Ok(metadata) if !metadata.is_dir() => {
                return Err(invalid("Holder folder must be a private directory"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        kuvo_store::files::private_folder(folder)?;
    }
    #[cfg(windows)]
    {
        let parent = folder
            .parent()
            .ok_or_else(|| invalid("Holder folder must have a parent"))?;
        fs::create_dir_all(parent)?;
        kuvo_os::windows::private_home(folder, true)?;
    }
    let listener = endpoint(folder, id)?.bind()?;
    let record_path = folder.join(format!("{id}.json"));
    let journal_path = folder.join(format!("{id}.journal"));
    let journal = Journal::open(
        journal_path.clone(),
        kind,
        boot.spec
            .journal_bytes
            .filter(|n| *n > 0)
            .unwrap_or(64_000_000),
    )?;
    let files = Files {
        record: record_path,
        journal: journal_path,
    };

    let mut env: Environment = std::env::vars_os().collect();
    env.retain(|name, _| !name.eq_ignore_ascii_case("BUN_BE_BUN"));
    if let Some(bun) = boot.spec.bun {
        env.insert("BUN_BE_BUN".into(), bun.into());
    }
    let mut command = boot.spec.command.into_iter();
    let program = command
        .next()
        .ok_or_else(|| invalid("Missing agent program"))?;
    let executable = which(program.as_os_str(), &env, &[]).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("Cannot find agent executable {}", program.display()),
        )
    });
    let mut spawn = Spawn::new(program.clone(), env);
    spawn.args = command.map(PathBuf::into_os_string).collect();
    spawn.cwd = boot.spec.cwd.clone();
    spawn.stdin = Stdio::Pipe;
    spawn.stdout = Stdio::Pipe;
    spawn.stderr = Stdio::Pipe;
    let mut child = match executable.and_then(|executable| {
        spawn.executable = executable;
        spawn.spawn()
    }) {
        Ok(child) => Agent::running(child, kind)?,
        Err(error) => Agent::failed(error),
    };
    let pid = std::process::id();
    let identity = Identity {
        process: kuvo_os::process::ProcessIdentity::read(pid)?
            .ok_or_else(|| io::Error::other("Cannot identify the holder process"))?,
    };
    let mut state = State {
        record: Record {
            proto: PROTOCOL.into(),
            holder: RUST_HOLDER.into(),
            id: id.into(),
            kind,
            pid,
            child: child.group.as_ref().map(|group| group.identity().pid),
            program,
            cwd: boot.spec.cwd,
            started: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            bridge: boot.spec.bridge,
            generation: 0,
            exited: None,
            meta: boot.spec.meta,
            key: boot.key,
            identity,
        },
        files,
        journal,
        controller: None,
        acked: 0,
        stderr: std::mem::take(&mut child.diagnostic),
        orphan: duration(boot.spec.orphan),
        linger: duration(boot.spec.linger),
        departure: None,
        stopping: None,
        connection_id: 0,
        portable_requests: boot.spec.portable_requests,
        lifecycle: folder.join("lifecycle.log"),
    };
    let stopping = termination()?;
    state.publish()?;
    state.report("holder listening", serde_json::json!({}));
    let mut ready = serde_json::to_vec(&Ready {
        ready: PROTOCOL.into(),
        pid,
        child: state.record.child,
    })?;
    ready.push(b'\n');
    if let Err(error) = io::stdout().write_all(&ready)
        && error.kind() != io::ErrorKind::BrokenPipe
    {
        return Err(error);
    }
    state.serve(listener, child, stopping).await
}

fn duration(ms: Option<u64>) -> Duration {
    ms.filter(|n| *n > 0 && *n <= SAFE_INTEGER)
        .map_or(WAIT_FOR_CONTROLLER, Duration::from_millis)
}

struct Files {
    record: PathBuf,
    journal: PathBuf,
}

impl Drop for Files {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.record);
        let _ = fs::remove_file(&self.journal);
    }
}

struct Agent {
    group: Option<ChildGroup>,
    stdin: Option<mpsc::Sender<String>>,
    stdout: mpsc::Receiver<String>,
    stderr: mpsc::Receiver<String>,
    status: Option<Exit>,
    diagnostic: String,
}

impl Agent {
    fn running(mut group: ChildGroup, kind: protocol::HoldKind) -> io::Result<Self> {
        let mut input = group.stdin.take().expect("requested agent stdin pipe");
        let (stdin, mut send) = mpsc::channel::<String>(32);
        std::thread::Builder::new()
            .name("agent-input".into())
            .spawn(move || {
                while let Some(line) = send.blocking_recv() {
                    if writeln!(input, "{line}").is_err() {
                        break;
                    }
                }
            })?;
        let output = group.stdout.take().expect("requested agent stdout pipe");
        let (lines, stdout) = mpsc::channel(1);
        std::thread::Builder::new()
            .name("agent-output".into())
            .spawn(move || {
                let mut output = io::BufReader::new(output);
                let mut bytes = Vec::new();
                while output.read_until(b'\n', &mut bytes).is_ok_and(|n| n != 0) {
                    if bytes.last() == Some(&b'\n') {
                        bytes.pop();
                        if kind != protocol::HoldKind::Raw && bytes.last() == Some(&b'\r') {
                            bytes.pop();
                        }
                    }
                    let line = String::from_utf8_lossy(&bytes);
                    if (kind == protocol::HoldKind::Raw || !line.trim().is_empty())
                        && lines.blocking_send(line.into_owned()).is_err()
                    {
                        break;
                    }
                    bytes.clear();
                }
            })?;
        let diagnostic = group.stderr.take().expect("requested agent stderr pipe");
        let (chunks, stderr) = mpsc::channel(1);
        std::thread::Builder::new()
            .name("agent-stderr".into())
            .spawn(move || read_stderr(diagnostic, chunks))?;
        Ok(Self {
            group: Some(group),
            stdin: Some(stdin),
            stdout,
            stderr,
            status: None,
            diagnostic: String::new(),
        })
    }

    fn failed(error: io::Error) -> Self {
        Self {
            group: None,
            stdin: None,
            stdout: mpsc::channel(1).1,
            stderr: mpsc::channel(1).1,
            status: Some(Exit {
                code: Some(127),
                signal: None,
            }),
            diagnostic: error.to_string(),
        }
    }

    fn poll_exit(&mut self) -> io::Result<()> {
        if let Some(group) = &mut self.group
            && group.has_exited()?
        {
            let status: ExitStatus = group.wait()?;
            self.status = Some(Exit {
                code: status.code(),
                signal: GroupSignal::name_from_exit(status),
            });
            self.group.take();
            self.stdin.take();
        }
        Ok(())
    }

    fn signal(&self, signal: GroupSignal) {
        if let Some(group) = &self.group {
            let _ = group.signal(signal);
        }
    }
}

fn read_stderr(mut output: impl Read, sender: mpsc::Sender<String>) {
    let mut bytes = [0; 8196];
    let mut partial = 0;
    while let Ok(n) = output.read(&mut bytes[partial..]) {
        if n == 0 {
            if partial > 0 {
                let _ =
                    sender.blocking_send(String::from_utf8_lossy(&bytes[..partial]).into_owned());
            }
            break;
        }
        let total = partial + n;
        // UTF-8 sequences split across reads must still arrive intact.
        let mut offset = 0;
        let mut valid = total;
        while let Err(error) = std::str::from_utf8(&bytes[offset..total]) {
            offset += error.valid_up_to();
            let Some(bad) = error.error_len() else {
                valid = offset;
                break;
            };
            offset += bad;
        }
        if valid > 0
            && sender
                .blocking_send(String::from_utf8_lossy(&bytes[..valid]).into_owned())
                .is_err()
        {
            break;
        }
        bytes.copy_within(valid..total, 0);
        partial = total - valid;
    }
}

struct State {
    record: Record,
    files: Files,
    journal: Journal,
    controller: Option<Controller>,
    acked: u64,
    stderr: String,
    orphan: Duration,
    linger: Duration,
    departure: Option<Instant>,
    stopping: Option<(Instant, bool)>,
    connection_id: u64,
    portable_requests: Option<Vec<String>>,
    lifecycle: PathBuf,
}

struct Connection {
    hello: Hello,
    reader: BufReader<ReadHalf<LocalStream>>,
    writer: WriteHalf<LocalStream>,
}

enum Control {
    Input(u64, Input),
    Closed(u64),
}

enum Frame {
    Welcome(Welcome),
    Output(Output),
}

struct Controller {
    writer: mpsc::Sender<Frame>,
    reader_task: JoinHandle<()>,
    fence: Option<oneshot::Sender<()>>,
    sent: u64,
    live_from: u64,
    asks: VecDeque<u64>,
    replay_through: Option<u64>,
    release_replay: bool,
    heard_end: bool,
    prepared: Option<bool>,
}

impl Drop for Controller {
    fn drop(&mut self) {
        self.reader_task.abort();
    }
}

impl Controller {
    fn fence(mut self) {
        if let Some(fence) = self.fence.take() {
            let _ = fence.send(());
        }
    }
}

impl State {
    fn publish(&self) -> io::Result<()> {
        kuvo_store::files::write_atomic(&self.files.record, &serde_json::to_vec(&self.record)?)
    }

    async fn serve(
        &mut self,
        mut listener: Listener,
        mut agent: Agent,
        mut termination: impl std::future::Future<Output = ()> + Unpin,
    ) -> io::Result<()> {
        let (waiting, mut handshakes) = mpsc::channel(8);
        let handshake_slots = std::sync::Arc::new(tokio::sync::Semaphore::new(8));
        let (commands, mut input) = mpsc::channel(16);
        let mut tick = tokio::time::interval(Duration::from_millis(25));
        let mut pending_input = None;
        let mut stdout_closed = false;
        let mut stderr_closed = false;
        self.departure = Some(Instant::now() + self.orphan);
        loop {
            self.pump()?;
            // A full controller stream holds back agent output. Keep accepting
            // successors even when the current controller has stopped reading.
            let has_room = self.controller.as_ref().is_none_or(|controller| {
                controller.sent == self.journal.seq && controller.writer.capacity() > 0
            });
            if self.record.exited.is_none()
                && stdout_closed
                && stderr_closed
                && let Some(exit) = agent.status.take()
            {
                self.report(
                    "child exited",
                    serde_json::json!({
                        "code": exit.code,
                        "signal": exit.signal,
                        "stopping": self.stopping.is_some(),
                        "stderrBytes": self.stderr.len(),
                    }),
                );
                self.record.exited = Some(exit);
                self.publish()?;
                if self.stopping.is_some() && self.controller.is_none() {
                    return Ok(());
                }
                self.departure = Some(Instant::now() + self.linger);
                continue;
            }
            let writer = self
                .controller
                .as_ref()
                .map(|controller| controller.writer.clone());
            let waiting_to_pump = !has_room && writer.is_some();
            let stdin = agent.stdin.clone();
            if stdin.is_none() {
                pending_input = None;
            }
            tokio::select! {
                stream = listener.accept() => {
                    let stream = stream?;
                    if let Ok(slot) = handshake_slots.clone().try_acquire_owned() {
                        let waiting = waiting.clone();
                        tokio::spawn(async move {
                            let _slot = slot;
                            let (reader, writer) = tokio::io::split(stream);
                            let mut reader = BufReader::new(reader);
                            if let Ok(Ok(hello)) = timeout(HANDSHAKE, protocol::read(&mut reader, protocol::MAX_HELLO)).await {
                                let _ = waiting.try_send(Connection { hello, reader, writer });
                            }
                        });
                    }
                }
                Some(connection) = handshakes.recv() => self.welcome(connection, &commands)?,
                Some(control) = input.recv(), if pending_input.is_none()
                    && self.controller.as_ref().is_none_or(|controller| controller.prepared.is_none()) => {
                    let finished = match control {
                        Control::Input(connection, frame) if self.current(connection) => {
                            if let Some(line) = frame.line {
                                pending_input = Some(line);
                                false
                            } else {
                                self.handle(frame, &mut agent)?
                            }
                        }
                        Control::Closed(connection) if self.current(connection) => self.detach(),
                        _ => false,
                    };
                    if finished { return Ok(()); }
                }
                line = agent.stdout.recv(), if !stdout_closed && has_room && !self.journal.paused() => match line {
                    Some(line) => self.journal.append(&line)?,
                    None => stdout_closed = true,
                },
                chunk = agent.stderr.recv(), if !stderr_closed && has_room => match chunk {
                    Some(text) => self.stderr(text),
                    None => stderr_closed = true,
                },
                permit = async { writer.as_ref().expect("writer available").reserve().await }, if waiting_to_pump => {
                    if permit.is_err() && self.detach() { return Ok(()); }
                }
                permit = async { stdin.as_ref().expect("stdin available").reserve().await }, if pending_input.is_some() => {
                    let line = pending_input.take().expect("pending input");
                    if let Ok(permit) = permit {
                        self.journal.acknowledge_answer(&line);
                        permit.send(line);
                    } else {
                        agent.stdin.take();
                    }
                }
                _ = &mut termination, if self.stopping.is_none() => self.stop(&mut agent, "SIGTERM")?,
                _ = tick.tick() => {
                    agent.poll_exit()?;
                    if let Some((start, sent_term)) = &mut self.stopping
                        && agent.group.is_some()
                    {
                        if start.elapsed() >= Duration::from_secs(15) {
                            agent.signal(GroupSignal::Kill);
                        } else if !*sent_term && start.elapsed() >= Duration::from_secs(5) {
                            agent.signal(GroupSignal::Terminate);
                            *sent_term = true;
                        }
                    }
                    if self.departure.is_some_and(|deadline| Instant::now() >= deadline) {
                        if self.record.exited.is_some() { return Ok(()); }
                        self.stop(&mut agent, "orphan timeout")?;
                    }
                }
            }
        }
    }

    fn stop(&mut self, agent: &mut Agent, reason: &str) -> io::Result<()> {
        if self.stopping.is_none() {
            self.report(
                "holder stopping child",
                serde_json::json!({ "reason": reason }),
            );
            self.stopping = Some((Instant::now(), false));
        }
        self.departure = None;
        agent.stdin.take();
        if self.controller.is_none() {
            // No successor came to claim the pin. Drain the remaining pipe data
            // so exit can be recorded even if the abandoned journal was full.
            self.journal.unpin()?;
        }
        Ok(())
    }

    fn report(&self, event: &str, details: serde_json::Value) {
        let record = &self.record;
        let mut line = serde_json::json!({
            "at": jiff::Timestamp::now().to_string(),
            "event": event,
            "id": record.id,
            "holder": record.pid,
            "child": record.child,
            "bridge": record.bridge,
            "provider": record.meta.get("provider"),
            "generation": record.generation,
        });
        if let Some(fields) = details.as_object() {
            line.as_object_mut()
                .expect("lifecycle event is an object")
                .extend(fields.clone());
        }
        if !self.lifecycle.exists() {
            let _ = kuvo_store::files::write_new(&self.lifecycle, b"");
        }
        if let Ok(mut file) = fs::OpenOptions::new().append(true).open(&self.lifecycle) {
            let _ = file.write_all(format!("{line}\n").as_bytes());
        }
    }

    fn detach(&mut self) -> bool {
        let heard_end = self
            .controller
            .take()
            .is_some_and(|controller| controller.heard_end);
        if self.record.exited.is_some() {
            self.stopping.is_some() || (heard_end && !self.journal.pinned)
        } else {
            self.report(
                "controller disconnected",
                serde_json::json!({
                    "seq": self.journal.seq, "acked": self.acked,
                }),
            );
            self.departure = Some(Instant::now() + self.orphan);
            false
        }
    }

    fn current(&self, connection_id: u64) -> bool {
        self.controller.is_some() && self.connection_id == connection_id
    }

    fn stderr(&mut self, text: String) {
        self.stderr.push_str(&text);
        let mut trim = self.stderr.len().saturating_sub(8000);
        while !self.stderr.is_char_boundary(trim) {
            trim += 1;
        }
        self.stderr.drain(..trim);
        self.send(Output::Stderr { text });
    }

    fn send(&mut self, output: Output) {
        if let Some(controller) = &self.controller {
            let _ = controller.writer.try_send(Frame::Output(output));
        }
    }

    fn welcome(
        &mut self,
        connection: Connection,
        commands: &mpsc::Sender<Control>,
    ) -> io::Result<()> {
        let Connection {
            hello,
            mut reader,
            mut writer,
        } = connection;
        let asked_from = hello
            .from
            .filter(|from| from.unsigned_abs() <= SAFE_INTEGER);
        let from = asked_from
            .map_or(self.acked, |n| n.max(0) as u64)
            .min(self.journal.seq);
        let refused = if hello.hello != PROTOCOL
            || !bool::from(hello.key.as_bytes().ct_eq(self.record.key.as_bytes()))
        {
            Some("Not this holder's key.".into())
        } else if hello.generation > SAFE_INTEGER
            || hello.generation < self.record.generation
            || (hello.generation == self.record.generation && self.controller.is_some())
        {
            Some(format!(
                "A bridge of generation {} has this agent.",
                self.record.generation
            ))
        } else if hello.complete && !self.journal.range_available(from) {
            Some("The turn is no longer in the journal.".into())
        } else {
            None
        };
        if let Some(refused) = refused {
            tokio::spawn(async move {
                let _ = timeout(
                    HANDSHAKE,
                    protocol::write(&mut writer, &Output::Refused { refused }),
                )
                .await;
            });
            return Ok(());
        }

        if let Some(previous) = self.controller.take() {
            previous.fence();
        }
        if self.record.exited.is_none() {
            self.departure = None;
        }
        self.record.generation = hello.generation;
        self.publish()?;
        self.report("controller attached", serde_json::json!({}));
        let record = &self.record;
        self.connection_id += 1;
        let connection_id = self.connection_id;
        let welcome = Welcome {
            hello: PROTOCOL.into(),
            pid: record.pid,
            child: record.child,
            generation: record.generation,
            seq: self.journal.seq,
            first: self.journal.first(),
            gap: !self.journal.range_available(from),
            exited: record.exited.clone(),
            stderr: self.stderr.clone(),
            meta: record.meta.clone(),
        };
        let (output, frames) = mpsc::channel(8);
        let (fence, fenced) = oneshot::channel();
        let writer_commands = commands.clone();
        tokio::spawn(async move {
            write_frames(writer, frames, fenced).await;
            let _ = writer_commands.send(Control::Closed(connection_id)).await;
        });
        let commands = commands.clone();
        let reader_task = tokio::spawn(async move {
            while let Ok(frame) = protocol::read(&mut reader, protocol::MAX_FRAME).await {
                if commands
                    .send(Control::Input(connection_id, frame))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            let _ = commands.send(Control::Closed(connection_id)).await;
        });
        let sent = from.max(self.journal.first() - 1);
        self.controller = Some(Controller {
            writer: output.clone(),
            reader_task,
            fence: Some(fence),
            sent,
            live_from: self.journal.seq,
            asks: self.journal.open_asks(from).collect(),
            replay_through: self.journal.pinned.then_some(self.journal.seq),
            release_replay: !hello.complete,
            heard_end: false,
            prepared: None,
        });
        let _ = output.try_send(Frame::Welcome(welcome));
        Ok(())
    }

    fn pump(&mut self) -> io::Result<()> {
        let Some(controller) = &mut self.controller else {
            return Ok(());
        };
        while let Ok(permit) = controller.writer.try_reserve() {
            let output = if let Some(seq) = controller.asks.pop_front() {
                self.journal.ask(seq)
            } else if controller.sent < self.journal.seq {
                controller.sent += 1;
                self.journal.replay(controller.sent, controller.live_from)?
            } else if let Some(prepared) = controller.prepared.take() {
                Some(Output::Prepared { prepared })
            } else {
                break;
            };
            if let Some(frame) = output {
                permit.send(Frame::Output(frame));
            }
        }
        if controller.release_replay
            && controller
                .replay_through
                .is_some_and(|through| through <= controller.sent)
        {
            controller.replay_through = None;
            self.journal.unpin()?;
        }
        if let Some(exit) = &self.record.exited
            && controller.sent == self.journal.seq
            && controller.asks.is_empty()
            && !controller.heard_end
            && let Ok(permit) = controller.writer.try_reserve()
        {
            permit.send(Frame::Output(Output::Exited { exit: exit.clone() }));
            controller.heard_end = true;
        }
        Ok(())
    }

    fn handle(&mut self, frame: Input, agent: &mut Agent) -> io::Result<bool> {
        if let Some(ack) = frame.ack.filter(|ack| *ack <= SAFE_INTEGER) {
            self.acked = self.acked.max(ack.min(self.journal.seq));
        } else if let Some(meta) = frame.meta {
            self.record.meta.extend(meta);
            self.publish()?;
        } else if let Some(from) = frame.prepare {
            let portable = self
                .portable_requests
                .as_ref()
                .is_none_or(|methods| self.journal.asks_are_portable(methods));
            let ready = portable
                && self.record.exited.is_none()
                && u64::try_from(from).is_ok_and(|from| {
                    from <= self.journal.seq && self.journal.range_available(from)
                });
            if ready {
                self.journal.pinned = true;
            }
            self.controller
                .as_mut()
                .expect("current controller")
                .prepared = Some(ready);
        } else if frame.unpin {
            let controller = self.controller.as_mut().expect("current controller");
            controller.release_replay = true;
            if controller.replay_through.is_none() {
                self.journal.unpin()?;
            }
            return Ok(controller.heard_end && !self.journal.pinned);
        } else if frame.end {
            self.report("controller closed stdin", serde_json::json!({}));
            agent.stdin.take();
        } else if let Some(name) = frame.signal
            && let Some(signal) = GroupSignal::from_name(&name)
        {
            self.report(
                "controller signalled child",
                serde_json::json!({ "signal": name }),
            );
            agent.signal(signal);
        } else if frame.bye {
            return Ok(self.record.exited.is_some() && !self.journal.pinned);
        }
        Ok(false)
    }
}

async fn write_frames(
    mut writer: WriteHalf<LocalStream>,
    mut frames: mpsc::Receiver<Frame>,
    mut fenced: oneshot::Receiver<()>,
) {
    loop {
        let result = tokio::select! {
            biased;
            result = &mut fenced => {
                if result.is_ok() {
                    let _ = timeout(Duration::from_secs(1), async {
                        while let Ok(frame) = frames.try_recv() {
                            write_frame(&mut writer, frame).await?;
                        }
                        protocol::write(&mut writer, &Output::Fenced { fenced: true }).await
                    }).await;
                }
                return;
            }
            item = frames.recv() => match item {
                Some(frame) => timeout(HUNG, write_frame(&mut writer, frame)).await,
                None => return,
            }
        };
        if !matches!(result, Ok(Ok(()))) {
            return;
        }
    }
}

async fn write_frame(writer: &mut WriteHalf<LocalStream>, frame: Frame) -> io::Result<()> {
    match frame {
        Frame::Welcome(frame) => protocol::write(writer, &frame).await,
        Frame::Output(frame) => protocol::write(writer, &frame).await,
    }
}

fn termination() -> io::Result<std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate = signal(SignalKind::terminate())?;
        let hangup = signal(SignalKind::hangup())?;
        let interrupt = signal(SignalKind::interrupt())?;
        Ok(Box::pin(async move {
            let _ignored = (hangup, interrupt);
            terminate.recv().await;
        }))
    }
    #[cfg(windows)]
    Ok(Box::pin(std::future::pending()))
}
