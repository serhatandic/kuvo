//! Resident-owned Add attempts. The SSH job owns its subprocesses and the
//! setup writer issues enrollment only after the remote account was checked.

mod address;
mod connect;
mod transport;

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use http::StatusCode;
use kuvo_protocol::routes::*;
use kuvo_server::{Access, Refusal, Registry};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot, watch};

use self::{address::Address, connect::Destination, transport::HostKey};
use crate::{
    SetupHandle,
    actor::Command,
    edits::Submitter,
    preferences,
    routes::{submitter, typed},
    storage::Files,
};

#[derive(Clone, Copy, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum State {
    Working,
    Asking,
    Stopping,
    Failed,
    Done,
}

#[derive(Clone, Copy, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum StepState {
    Waiting,
    Now,
    Done,
    Failed,
}

#[derive(Clone, Deserialize, Serialize)]
struct Step {
    id: String,
    title: String,
    state: StepState,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
struct Failure {
    title: String,
    message: String,
    again: bool,
}

#[derive(Clone, Deserialize, Serialize)]
struct Computer {
    key: String,
    name: String,
}

enum Completed {
    New(Computer),
    Known(Computer),
}

#[derive(Clone, Deserialize, Serialize)]
struct Attempt {
    id: String,
    kind: String,
    destination: String,
    name: String,
    state: State,
    title: String,
    steps: Vec<Step>,
    #[serde(skip_serializing_if = "Option::is_none")]
    question: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure: Option<Failure>,
    #[serde(skip_serializing_if = "Option::is_none")]
    computer: Option<Computer>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    known: bool,
}

impl Attempt {
    fn new(id: String, address: &Address) -> Self {
        let name = preferences::truncate(address.as_str(), 100);
        let steps = [
            ("checking", "Checking it"),
            ("files", "Copying Kuvo Code"),
            ("engine", "Copying the engine"),
            ("starting", "Starting Kuvo Code there"),
            ("seeding", "Copying your keys and sign-ins"),
            ("joining", "Adding it to your computers"),
        ]
        .into_iter()
        .map(|(id, title)| Step {
            id: id.into(),
            title: title.into(),
            state: StepState::Waiting,
            detail: None,
        })
        .collect();
        Self {
            id,
            kind: "add".into(),
            destination: address.as_str().into(),
            title: format!("Adding {name}…"),
            name,
            steps,
            state: State::Working,
            question: None,
            failure: None,
            computer: None,
            known: false,
        }
    }

    fn fail(&mut self, message: String) {
        self.state = State::Failed;
        self.question = None;
        self.title = format!("Couldn’t add {}", self.name);
        self.failure = Some(Failure {
            title: self.title.clone(),
            message,
            again: true,
        });
        for step in &mut self.steps {
            if step.state == StepState::Now {
                step.state = StepState::Failed;
            }
        }
    }
}

struct Active {
    seen: Attempt,
    stop: Arc<AtomicBool>,
    answer: Option<oneshot::Sender<()>>,
}

impl Drop for Active {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

#[derive(Clone)]
struct Adding {
    setup: SetupHandle,
    commands: mpsc::Sender<Message>,
    views: watch::Receiver<Vec<Attempt>>,
    files: Files,
}

enum Change {
    Start(Address, Submitter, WindowsMode),
    Answer(String, Value),
    Cancel(String),
}

#[derive(Clone, Copy, PartialEq)]
enum WindowsMode {
    Wsl,
    Native,
}

enum Progress {
    Step(&'static str, StepState, String),
    Name(String),
    HostKey(Vec<HostKey>, oneshot::Sender<()>),
    Done(Result<Completed, String>),
}

enum Message {
    Client(Change, oneshot::Sender<Result<Value, Refusal>>),
    Job(String, Progress),
}

impl SetupHandle {
    pub fn register_ssh(&self, routes: &mut Registry, home: &Path) {
        let adding = Adding::new(self.clone(), home.into());
        let route = adding.clone();
        routes.register::<get_kuvo_computers_add::Spec, _, _>(Access::Any, move |_| {
            let route = route.clone();
            async move { typed::<get_kuvo_computers_add::Spec, 200>(route.page().await) }
        });
        let route = adding.clone();
        routes.register::<get_kuvo_computers_add_events::Spec, _, _>(Access::Any, move |_| {
            let route = route.clone();
            async move {
                let mut attempts = route.views.clone();
                let mut setup = route.setup.views.clone();
                Ok(kuvo_server::RouteReply::ndjson::<200>(async_stream::stream! {
                    let mut before = Value::Null;
                    loop {
                        let next = route.page().await;
                        if next != before {
                            let line = serde_json::from_value(next.clone())
                                .map_err(std::io::Error::other)
                                .and_then(kuvo_server::JsonLine::<get_kuvo_computers_add_events::Spec, 200>::new);
                            before = next;
                            yield line;
                        }
                        tokio::select! {
                            result = attempts.changed() => if result.is_err() { break; },
                            result = setup.changed() => if result.is_err() { break; },
                        }
                    }
                }))
            }
        });
        let route = adding.clone();
        routes.register::<post_kuvo_computers_add::Spec, _, _>(Access::DesktopOrPaired, move |call| {
            let route = route.clone();
            async move {
                let by = submitter(&call.request.caller)?;
                route.allowed(by.clone()).await.map_err(Denied::refusal)?;
                let value = body(&call.request.body);
                if value.get("replace").is_some_and(|item| !item.as_str().is_some_and(|v| crate::model::safe_token(v, 43, 43))) {
                    return Err(refuse(400, "Choose the previous membership by its key."));
                }
                if value.get("discardLocalPairings").is_some_and(|item| !item.is_boolean()) {
                    return Err(refuse(400, "Choose whether to discard local pairings."));
                }
                let destination = Address::read(value["destination"].as_str().unwrap_or(""))
                    .map_err(|message| refuse(400, message))?;
                if value.get("credentials").is_some_and(|item| !item.as_array().is_some_and(|ids| {
                    ids.len() <= 300 && ids.iter().all(|id| id.as_str().is_some_and(|id| crate::model::safe_token(id, 43, 43)))
                })) {
                    return Err(refuse(400, "Choose sign-ins and keys by their checklist IDs."));
                }
                if value["credentials"].as_array().is_some_and(|items| !items.is_empty()) {
                    return Err(refuse(503, "Some selected sign-ins or keys are no longer available. Open the checklist again."));
                }
                let windows = match value.get("windows") {
                    None => WindowsMode::Wsl,
                    Some(value) if value == "native" => WindowsMode::Native,
                    Some(value) if value == "wsl" => WindowsMode::Wsl,
                    Some(_) => return Err(refuse(400, "Choose native Windows or WSL.")),
                };
                typed::<post_kuvo_computers_add::Spec, 202>(route.change(Change::Start(destination, by, windows)).await?)
            }
        });
        let route = adding.clone();
        routes.register::<post_kuvo_computers_add_id_answer::Spec, _, _>(
            Access::DesktopOrPaired,
            move |call| {
                let route = route.clone();
                async move {
                    route
                        .allowed(submitter(&call.request.caller)?)
                        .await
                        .map_err(Denied::refusal)?;
                    let id = call.param("id").unwrap_or_default().to_owned();
                    typed::<post_kuvo_computers_add_id_answer::Spec, 200>(
                        route
                            .change(Change::Answer(id, body(&call.request.body)))
                            .await?,
                    )
                }
            },
        );
        routes.register::<delete_kuvo_computers_add_id::Spec, _, _>(
            Access::DesktopOrPaired,
            move |call| {
                let route = adding.clone();
                async move {
                    route
                        .allowed(submitter(&call.request.caller)?)
                        .await
                        .map_err(Denied::refusal)?;
                    let id = call.param("id").unwrap_or_default().to_owned();
                    typed::<delete_kuvo_computers_add_id::Spec, 200>(
                        route.change(Change::Cancel(id)).await?,
                    )
                }
            },
        );
    }
}

impl Adding {
    fn new(setup: SetupHandle, home: PathBuf) -> Self {
        let files = Files::new(home);
        let (commands, incoming) = mpsc::channel(64);
        let (changed, views) = watch::channel(Vec::new());
        let kept = files
            .read::<Vec<Attempt>>("run/adding.json")
            .ok()
            .flatten()
            .unwrap_or_default();
        let mut owner = Owner {
            active: Vec::new(),
            finished: kept.into_iter().take(8).collect(),
            setup: setup.clone(),
            files: files.clone(),
            changed,
            asks: 0,
        };
        for attempt in &mut owner.finished {
            if matches!(
                attempt.state,
                State::Working | State::Asking | State::Stopping
            ) {
                attempt.fail("Kuvo Code on this computer started again before this finished. Try again: it goes on from what's done.".into());
            }
        }
        owner.changed.send_replace(owner.snapshot());
        tokio::spawn(owner.run(incoming, commands.downgrade()));
        Self {
            setup,
            commands,
            views,
            files,
        }
    }

    async fn allowed(&self, by: Submitter) -> Result<(), Denied> {
        let answer = self
            .setup
            .request_command(Command::SshAdmission(by))
            .await
            .map_err(|error| Denied::new(503, error.to_string()))?;
        if answer.status != 200 {
            return Err(Denied {
                status: answer.status,
                value: answer.value,
            });
        }
        let home = self.files.root();
        if !connect::has_release(home) {
            let view = self.setup.views.borrow();
            let name = view.setup["computers"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|peer| peer["key"] == view.setup["you"]["key"])
                .and_then(|peer| peer["name"].as_str())
                .unwrap_or("This computer");
            return Err(Denied::new(
                503,
                format!(
                    "Kuvo Code on {name} has no files of its own to give another computer yet. Update Kuvo Code here, then try again."
                ),
            ));
        }
        Ok(())
    }

    async fn page(&self) -> Value {
        let tries = self.views.borrow().clone();
        if let Err(reason) = self.allowed(Submitter::Computer).await {
            return json!({"can": false, "why": reason.value["message"], "hosts": [], "tries": tries});
        }
        let env = std::env::vars_os().collect();
        let account = kuvo_os::platform::Account::current()
            .and_then(|user| user.home_folder(kuvo_os::platform::Platform::current(), &env));
        let saved = self
            .files
            .read::<Vec<Value>>("machines.json")
            .ok()
            .flatten()
            .unwrap_or_default();
        let taken = saved
            .iter()
            .flat_map(|record| {
                record["destination"].as_str().into_iter().chain(
                    record["otherAddresses"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str),
                )
            })
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let hosts = account
            .map(|account| address::suggestions(&account, &taken))
            .unwrap_or_default();
        json!({"can": true, "hosts": hosts, "tries": tries})
    }

    async fn change(&self, change: Change) -> Result<Value, Refusal> {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Message::Client(change, reply))
            .await
            .map_err(|_| unavailable())?;
        result.await.map_err(|_| unavailable())?
    }
}

struct Owner {
    active: Vec<Active>,
    finished: VecDeque<Attempt>,
    files: Files,
    setup: SetupHandle,
    changed: watch::Sender<Vec<Attempt>>,
    asks: u64,
}

impl Owner {
    async fn run(mut self, mut incoming: mpsc::Receiver<Message>, out: mpsc::WeakSender<Message>) {
        while let Some(message) = incoming.recv().await {
            match message {
                Message::Client(change, reply) => {
                    let result = self.change(change, &out);
                    self.publish();
                    let _ = reply.send(result);
                }
                Message::Job(id, progress) => {
                    self.progress(&id, progress);
                    self.publish();
                }
            }
        }
    }

    fn snapshot(&self) -> Vec<Attempt> {
        self.active
            .iter()
            .rev()
            .map(|attempt| attempt.seen.clone())
            .chain(self.finished.iter().cloned())
            .collect()
    }

    fn publish(&self) {
        let next = self.snapshot();
        if let Err(error) = self.files.write("run/adding.json", &next) {
            eprintln!("saving SSH attempts: {error}");
        }
        self.changed.send_replace(next);
    }

    fn change(
        &mut self,
        change: Change,
        out: &mpsc::WeakSender<Message>,
    ) -> Result<Value, Refusal> {
        match change {
            Change::Start(destination, by, windows) => {
                if let Some(now) = self
                    .active
                    .iter()
                    .find(|now| now.seen.destination == destination.as_str())
                {
                    return Ok(json!({"try": now.seen}));
                }
                let id = format!(
                    "add_{}",
                    hex::encode(crate::random::<12>().map_err(|_| unavailable())?)
                );
                let seen = Attempt::new(id.clone(), &destination);
                let stop = Arc::new(AtomicBool::new(false));
                let active = Active {
                    seen: seen.clone(),
                    stop: stop.clone(),
                    answer: None,
                };
                self.active.push(active);
                let job = Job {
                    id,
                    messages: out.clone(),
                    stop,
                    files: self.files.clone(),
                    setup: self.setup.clone(),
                    by,
                    windows,
                };
                tokio::spawn(async move {
                    let result = job.add(destination).await;
                    let _ = job.send(Progress::Done(result)).await;
                });
                Ok(json!({"try": seen}))
            }
            Change::Answer(id, input) => {
                let now = self
                    .active
                    .iter_mut()
                    .find(|now| now.seen.id == id)
                    .filter(|now| {
                        now.seen.state == State::Asking
                            && now.seen.question.as_ref().is_some_and(|question| {
                                question["at"].as_u64().is_some()
                                    && question["at"].as_u64() == input["at"].as_u64()
                            })
                            && matches!(input["answer"].as_str(), Some("trust" | "cancel"))
                    })
                    .ok_or_else(|| refuse(409, "It isn’t asking that now."))?;
                now.seen.question = None;
                if input["answer"] == "trust" {
                    if let Some(answer) = now.answer.take() {
                        let _ = answer.send(());
                    }
                    now.seen.state = State::Working;
                } else {
                    now.stop.store(true, Ordering::Release);
                    now.answer = None;
                    now.seen.state = State::Stopping;
                }
                Ok(json!({"try": now.seen}))
            }
            Change::Cancel(id) => {
                if let Some(now) = self.active.iter_mut().find(|now| now.seen.id == id) {
                    now.stop.store(true, Ordering::Release);
                    now.answer = None;
                    now.seen.question = None;
                    now.seen.state = State::Stopping;
                } else if let Some(index) = self.finished.iter().position(|seen| seen.id == id) {
                    self.finished.remove(index);
                } else {
                    return Err(refuse(404, "There’s no such try."));
                }
                Ok(json!({}))
            }
        }
    }

    fn progress(&mut self, id: &str, progress: Progress) {
        let Some(index) = self.active.iter().position(|now| now.seen.id == id) else {
            return;
        };
        let now = &mut self.active[index];
        if now.stop.load(Ordering::Acquire) && !matches!(progress, Progress::Done(_)) {
            return;
        }
        match progress {
            Progress::Name(name) => {
                now.seen.title = format!("Adding {name}…");
                now.seen.name = name;
            }
            Progress::Step(id, state, detail) => {
                for step in &mut now.seen.steps {
                    if step.id == id {
                        step.state = state;
                        step.detail = (!detail.is_empty()).then(|| detail.clone());
                    } else if step.state == StepState::Now {
                        step.state = StepState::Done;
                    }
                }
            }
            Progress::HostKey(keys, answer) => {
                self.asks += 1;
                let here = self.setup.views.borrow().setup["you"]["name"]
                    .as_str()
                    .unwrap_or("This computer")
                    .to_owned();
                let name = &now.seen.name;
                let mut facts = vec![json!({"label": "Address", "value": now.seen.destination})];
                facts.extend(
                    keys.iter()
                        .find(|key| key.kind == "ED25519")
                        .or_else(|| keys.first())
                        .map(|key| {
                            json!({"label": format!("Fingerprint ({})", key.kind),
                    "value": key.fingerprint, "code": true})
                        }),
                );
                now.seen.question = Some(json!({
                    "id": "hostKey", "title": format!("Is this {name}?"),
                    "message": format!("{here} hasn’t connected to {name} before. Check that the fingerprint is its own (on that computer: ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub), then trust it."),
                    "at": self.asks, "facts": facts,
                    "answers": [{"id":"cancel","label":"Cancel","kind":"plain"},{"id":"trust","label":"Trust","kind":"primary"}],
                }));
                now.seen.state = State::Asking;
                now.answer = Some(answer);
            }
            Progress::Done(result) => {
                let mut now = self.active.remove(index);
                if now.stop.load(Ordering::Acquire) && result.is_err() {
                    return;
                }
                match result {
                    Ok(completed) => {
                        now.seen.state = State::Done;
                        now.seen.question = None;
                        let computer = match completed {
                            Completed::Known(computer) => {
                                now.seen.known = true;
                                now.seen.name.clone_from(&computer.name);
                                now.seen.title = format!(
                                    "{} is {}, already one of your computers",
                                    now.seen.destination, computer.name
                                );
                                computer
                            }
                            Completed::New(computer) => {
                                now.seen.title =
                                    format!("{} is one of your computers", now.seen.name);
                                computer
                            }
                        };
                        now.seen.computer = Some(computer);
                        for step in &mut now.seen.steps {
                            step.state = StepState::Done;
                        }
                    }
                    Err(message) => now.seen.fail(message),
                }
                self.finished.retain(|other| {
                    other.destination != now.seen.destination || other.kind != now.seen.kind
                });
                self.finished.push_front(now.seen.clone());
                self.finished.truncate(8);
            }
        }
    }
}

struct Job {
    id: String,
    messages: mpsc::WeakSender<Message>,
    stop: Arc<AtomicBool>,
    setup: SetupHandle,
    files: Files,
    by: Submitter,
    windows: WindowsMode,
}

impl Job {
    async fn send(&self, progress: Progress) -> Result<(), String> {
        let queue = self
            .messages
            .upgrade()
            .ok_or_else(|| "The Add was stopped.".to_owned())?;
        queue
            .send(Message::Job(self.id.clone(), progress))
            .await
            .map_err(|_| "The Add was stopped.".to_owned())
    }

    async fn step(&self, id: &'static str, state: StepState, detail: &str) -> Result<(), String> {
        if self.stop.load(Ordering::Acquire) {
            return Err("The Add was stopped.".into());
        }
        self.send(Progress::Step(id, state, detail.into())).await
    }

    async fn add(&self, destination: Address) -> Result<Completed, String> {
        let name = preferences::truncate(destination.as_str(), 100);
        self.step("checking", StepState::Now, &format!("Connecting to {name}"))
            .await?;
        let remote = Destination::inspect(self, destination.clone(), &name).await?;
        let name = remote.name();
        if let Some(existing) = remote.existing() {
            if let Some(identity) = &existing.identity {
                let result = self
                    .setup
                    .request_command(Command::SshAlias {
                        identity: identity.clone(),
                        key: existing.key.clone(),
                        destination: destination.as_str().into(),
                        by: self.by.clone(),
                    })
                    .await
                    .map_err(|error| error.to_string())?;
                if result.status == 200 {
                    let computer: Computer = serde_json::from_value(result.value)
                        .map_err(|_| "Couldn’t keep the SSH address.")?;
                    let detail = format!("It’s {}, already one of your computers", computer.name);
                    for id in ["files", "engine", "starting", "seeding", "joining"] {
                        self.step(id, StepState::Done, &detail).await?;
                    }
                    return Ok(Completed::Known(computer));
                }
                if result.status != 404 {
                    return Err(result.value["message"]
                        .as_str()
                        .unwrap_or("Couldn’t keep the SSH address.")
                        .to_owned());
                }
            }
            return Err(format!(
                "Kuvo Code on {name} already belongs to another setup. To add this computer, connect over SSH to your own OS account."
            ));
        }
        if name != preferences::truncate(destination.as_str(), 100) {
            self.send(Progress::Name(name.to_owned())).await?;
        }
        self.step("files", StepState::Now, "").await?;
        remote.install(self).await?;
        self.step("engine", StepState::Done, "").await?;
        self.step("starting", StepState::Done, "").await?;
        self.step("seeding", StepState::Done, "Nothing to copy.")
            .await?;
        self.step("joining", StepState::Now, "").await?;
        let invite = self
            .setup
            .request_command(Command::InstallerCode(None, self.by.clone()))
            .await
            .map_err(|error| error.to_string())?;
        let code = invite.value["code"].as_str().ok_or_else(|| {
            invite.value["message"]
                .as_str()
                .unwrap_or("Your main computer could not issue an installer code.")
                .to_owned()
        })?;
        let invitation = crate::installer::Code::read(code)
            .map(|(code, _)| crate::installer::hash(&code.token))
            .ok_or("Your main computer could not issue an installer code.")?;
        let (computer, identity) = remote.enroll(self, name, code).await?;
        let done = self
            .setup
            .request_command(Command::SshAddress {
                key: computer.key.clone(),
                invite: invitation,
                destination: destination.as_str().into(),
                identity,
                distro: remote.distro(),
                by: self.by.clone(),
            })
            .await
            .map_err(|error| error.to_string())?;
        if done.status != 200 {
            return Err(done.value["message"]
                .as_str()
                .unwrap_or("Couldn’t keep the SSH address.")
                .into());
        }
        Ok(Completed::New(computer))
    }
}

struct Denied {
    status: u16,
    value: Value,
}

impl Denied {
    fn new(status: u16, message: String) -> Self {
        Self {
            status,
            value: json!({"message": message}),
        }
    }

    fn refusal(self) -> Refusal {
        Refusal::json(
            StatusCode::from_u16(self.status).expect("known status"),
            self.value,
        )
    }
}

fn body(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or(Value::Null)
}

fn refuse(status: u16, message: impl Into<String>) -> Refusal {
    Refusal::new(StatusCode::from_u16(status).expect("known status"), message)
}

fn unavailable() -> Refusal {
    refuse(
        503,
        "Kuvo’s local server did not answer. Check the add attempts before retrying.",
    )
}
