//! Resident-owned Add attempts. The SSH job owns its subprocesses and the
//! setup writer issues enrollment only after the remote account was checked.

mod address;
mod connect;
mod transport;
mod uninstall;

#[cfg(unix)]
pub(crate) use connect::is_reservation_token;

use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use kuvo_protocol::routes::*;
use kuvo_server::{Access, Refusal, Registry};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot, watch};

use self::{address::Address, connect::Destination, transport::HostKey};
use crate::{
    SetupHandle,
    actor::{Command, Response, computer_requests::ComputerRequest},
    edits::Submitter,
    preferences,
    routes::{response_refusal, submitter, typed},
    storage::Files,
};

const PHONE_LINK: &str = "phone-link.json";

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

struct AddError {
    title: Option<String>,
    message: String,
    outcome: FailureOutcome,
    again: bool,
}

enum FailureOutcome {
    Operation,
    ReservationCleanup(Option<Completed>),
}

impl AddError {
    fn titled(title: String, message: String) -> Self {
        Self {
            title: Some(title),
            ..Self::from(message)
        }
    }

    fn occupied(name: &str) -> Self {
        Self {
            again: false,
            ..Self::from(format!(
                "Kuvo Code on {name} already belongs to another setup. To add this computer, connect over SSH to your own OS account."
            ))
        }
    }
}

impl From<String> for AddError {
    fn from(message: String) -> Self {
        Self {
            title: None,
            message,
            outcome: FailureOutcome::Operation,
            again: true,
        }
    }
}

impl From<&str> for AddError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

#[derive(Clone, Deserialize, Serialize)]
struct Computer {
    key: String,
    name: String,
}

enum Completed {
    New(Computer),
    Known(Computer),
    Uninstalled,
}

impl Completed {
    fn computer(self) -> Option<Computer> {
        match self {
            Self::New(computer) | Self::Known(computer) => Some(computer),
            Self::Uninstalled => None,
        }
    }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    save: Option<crate::Receipt>,
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
            save: None,
        }
    }

    fn uninstall(id: String, target: &uninstall::Target) -> Self {
        Self {
            id,
            kind: "uninstall".into(),
            destination: target.destination.as_str().into(),
            name: target.name.clone(),
            state: State::Working,
            title: format!("Uninstalling Kuvo Code from {}…", target.name),
            steps: [
                ("checking", "Reaching it"),
                ("removing", "Stopping and removing Kuvo Code"),
                ("forgetting", "Taking it off your computers"),
            ]
            .into_iter()
            .map(|(id, title)| Step {
                id: id.into(),
                title: title.into(),
                state: StepState::Waiting,
                detail: None,
            })
            .collect(),
            question: None,
            failure: None,
            computer: None,
            known: false,
            save: None,
        }
    }

    fn fail(&mut self, error: AddError) {
        self.state = State::Failed;
        self.question = None;
        self.title = if self.kind == "uninstall" {
            format!("Couldn’t uninstall from {}", self.name)
        } else {
            format!("Couldn’t add {}", self.name)
        };
        self.failure = Some(Failure {
            title: error.title.unwrap_or_else(|| self.title.clone()),
            message: error.message,
            again: error.again,
        });
        if let FailureOutcome::ReservationCleanup(Some(completed)) = error.outcome {
            self.known = matches!(completed, Completed::Known(_));
            self.computer = completed.computer();
        }
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
    done: watch::Receiver<()>,
    committed: bool,
}

impl Active {
    fn cancel(&mut self) {
        if self.committed || self.stop.swap(true, Ordering::AcqRel) {
            return;
        }
        self.answer = None;
        self.seen.question = None;
        self.seen.state = State::Stopping;
        self.seen.title = "Stopping…".into();
    }
}

impl Drop for Active {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

#[derive(Clone)]
pub(crate) struct Adding {
    setup: SetupHandle,
    commands: mpsc::Sender<Message>,
    views: watch::Receiver<Vec<Attempt>>,
    files: Files,
}

pub(crate) enum Action {
    Page,
    Start(Value),
    Answer { id: String, input: Value },
    Cancel(String),
}

impl Action {
    pub(crate) fn wire(&self) -> (&'static str, String, Option<Value>) {
        match self {
            Self::Page => ("GET", "add".into(), None),
            Self::Start(input) => ("POST", "add".into(), Some(input.clone())),
            Self::Answer { id, input } => ("POST", format!("answer/{id}"), Some(input.clone())),
            Self::Cancel(id) => ("DELETE", id.clone(), None),
        }
    }

    pub(crate) fn read(method: &str, item: &str, input: Option<Value>) -> Option<Self> {
        fn valid(id: &str) -> bool {
            id.strip_prefix("add_").is_some_and(|suffix| {
                (1..=40).contains(&suffix.len())
                    && suffix
                        .bytes()
                        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
            })
        }
        match (method, item) {
            ("GET", "add") => Some(Self::Page),
            ("POST", "add") => Some(Self::Start(input.unwrap_or_default())),
            ("POST", item) => {
                let id = item.strip_prefix("answer/").filter(|id| valid(id))?;
                Some(Self::Answer {
                    id: id.into(),
                    input: input.unwrap_or_default(),
                })
            }
            ("DELETE", item) if valid(item) => Some(Self::Cancel(item.into())),
            _ => None,
        }
    }

    pub(crate) fn changes(&self) -> bool {
        !matches!(self, Self::Page)
    }
}

enum Change {
    Start(Address, Submitter, WindowsMode),
    Uninstall(uninstall::Target, Submitter),
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
    RecoverAccount(String, oneshot::Sender<()>),
    UninstallCommitted(Option<crate::Receipt>),
    Done(Result<Completed, AddError>),
}

enum Work {
    Add(Address, RemovalRevisions),
    Uninstall(uninstall::Target, Option<watch::Receiver<()>>),
}

pub(crate) type RemovalRevisions = BTreeMap<String, u64>;

enum Message {
    Client(Change, oneshot::Sender<Result<Value, Denied>>),
    Job(String, Progress),
}

impl SetupHandle {
    pub fn register_ssh(&self, routes: &mut Registry, home: &Path) {
        let adding = Arc::new(Adding::new(self.clone(), home.into()));
        self.ssh_routes.send_replace(Some(Arc::downgrade(&adding)));
        let route = adding.clone();
        routes.register::<get_kuvo_computers_add::Spec, _, _>(Access::Any, move |_| {
            let route = route.clone();
            async move { typed::<get_kuvo_computers_add::Spec, 200>(route.followed_page().await) }
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
                        let next = route.followed_page().await;
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
                            () = tokio::time::sleep(std::time::Duration::from_millis(1500)), if route.member() => {},
                        }
                    }
                }))
            }
        });
        let route = adding.clone();
        routes.register::<post_kuvo_computers_add::Spec, _, _>(
            Access::DesktopOrPaired,
            move |call| {
                let route = route.clone();
                async move {
                    let response = route
                        .follow(
                            Action::Start(body(&call.request.body)),
                            submitter(&call.request.caller)?,
                        )
                        .await?;
                    if response.status == 202 {
                        typed::<post_kuvo_computers_add::Spec, 202>(response.value)
                    } else {
                        Err(response_refusal(response))
                    }
                }
            },
        );
        let route = adding.clone();
        routes.register::<post_kuvo_computers_key_uninstall::Spec, _, _>(
            Access::DesktopOrPaired,
            move |call| {
                let route = route.clone();
                async move {
                    let key = call.param("key").unwrap_or_default();
                    let by = submitter(&call.request.caller)?;
                    let result = route.begin_uninstall(key, by).await;
                    match result {
                        Ok(value) => typed::<post_kuvo_computers_key_uninstall::Spec, 202>(value),
                        Err(error) => Err(response_refusal(Response {
                            status: error.status,
                            value: error.value,
                        })),
                    }
                }
            },
        );
        let route = adding.clone();
        routes.register::<post_kuvo_computers_add_id_answer::Spec, _, _>(
            Access::DesktopOrPaired,
            move |call| {
                let route = route.clone();
                async move {
                    let id = call.param("id").unwrap_or_default().to_owned();
                    let action = Action::Answer {
                        id,
                        input: body(&call.request.body),
                    };
                    let response = route
                        .follow(action, submitter(&call.request.caller)?)
                        .await?;
                    if response.status == 200 {
                        typed::<post_kuvo_computers_add_id_answer::Spec, 200>(response.value)
                    } else {
                        Err(response_refusal(response))
                    }
                }
            },
        );
        routes.register::<delete_kuvo_computers_add_id::Spec, _, _>(
            Access::DesktopOrPaired,
            move |call| {
                let route = adding.clone();
                async move {
                    let id = call.param("id").unwrap_or_default().to_owned();
                    let response = route
                        .follow(Action::Cancel(id), submitter(&call.request.caller)?)
                        .await?;
                    if response.status == 200 {
                        typed::<delete_kuvo_computers_add_id::Spec, 200>(response.value)
                    } else {
                        Err(response_refusal(response))
                    }
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
            reservations: Arc::default(),
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

    async fn allowed(&self, by: Submitter) -> Result<Vec<Value>, Denied> {
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
        let name = local_name(&self.setup);
        if !connect::has_release(self.files.root()) {
            return Err(Denied::new(
                503,
                format!(
                    "Kuvo Code on {name} has no files of its own to give another computer yet. Update Kuvo Code here, then try again."
                ),
            ));
        }
        saved_machines(&self.files).map_err(|_| {
            refuse(
                503,
                format!("{name}’s list of machines can’t be read right now."),
            )
        })
    }

    async fn page(&self) -> Value {
        let tries = self.views.borrow().clone();
        let saved = match self.allowed(Submitter::Computer).await {
            Ok(saved) => saved,
            Err(reason) => {
                return json!({"can": false, "why": reason.value["message"], "hosts": [], "tries": tries});
            }
        };
        let env = std::env::vars_os().collect();
        let account = kuvo_os::platform::Account::current()
            .and_then(|user| user.home_folder(kuvo_os::platform::Platform::current(), &env));
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

    fn member(&self) -> bool {
        self.setup.views.borrow().setup["you"]["role"] == "member"
    }

    async fn follow(&self, action: Action, by: Submitter) -> Result<Response, Refusal> {
        if self.member() {
            self.setup
                .request(Command::ComputerViaMain(ComputerRequest::Ssh(action), by))
                .await
        } else {
            Ok(self.execute(action, by).await)
        }
    }

    async fn followed_page(&self) -> Value {
        match self.follow(Action::Page, Submitter::Computer).await {
            Ok(answer) if answer.status == 200 => answer.value,
            Ok(answer) => {
                json!({"can": false, "why": answer.value["message"], "hosts": [], "tries": []})
            }
            Err(_) => {
                json!({"can": false, "why": "This computer can't reach your main computer.", "hosts": [], "tries": []})
            }
        }
    }

    pub(crate) async fn execute(&self, action: Action, by: Submitter) -> Response {
        if matches!(action, Action::Page) {
            return Response::ok(self.page().await);
        }
        let result = async {
            self.allowed(by.clone()).await?;
            let (status, change) = match action {
                Action::Start(value) => (202, start(value, by)?),
                Action::Answer { id, input } => (200, Change::Answer(id, input)),
                Action::Cancel(id) => (200, Change::Cancel(id)),
                Action::Page => unreachable!("the page is read separately"),
            };
            Ok(Response {
                status,
                value: self.change(change).await?,
            })
        }
        .await;
        result.unwrap_or_else(|error: Denied| Response {
            status: error.status,
            value: error.value,
        })
    }

    async fn change(&self, change: Change) -> Result<Value, Denied> {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Message::Client(change, reply))
            .await
            .map_err(|_| unavailable())?;
        result.await.map_err(|_| unavailable())?
    }

    async fn begin_uninstall(&self, key: &str, by: Submitter) -> Result<Value, Denied> {
        let saved = self.allowed(by.clone()).await?;
        let target = uninstall::Target::read(&self.files, &saved, key)?;
        self.change(Change::Uninstall(target, by)).await
    }
}

struct Owner {
    active: Vec<Active>,
    finished: VecDeque<Attempt>,
    files: Files,
    setup: SetupHandle,
    changed: watch::Sender<Vec<Attempt>>,
    asks: u64,
    reservations: Arc<Mutex<HashSet<String>>>,
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

    fn spawn(
        &mut self,
        seen: Attempt,
        by: Submitter,
        windows: WindowsMode,
        out: &mpsc::WeakSender<Message>,
        work: Work,
    ) {
        let stop = Arc::new(AtomicBool::new(false));
        let (ended, done) = watch::channel(());
        self.active.push(Active {
            seen: seen.clone(),
            stop: stop.clone(),
            answer: None,
            done,
            committed: false,
        });
        let job = Job {
            id: seen.id,
            messages: out.clone(),
            stop,
            files: self.files.clone(),
            setup: self.setup.clone(),
            by,
            windows,
            reservations: self.reservations.clone(),
        };
        tokio::spawn(async move {
            let result = match work {
                Work::Add(destination, revisions) => job.add(destination, revisions).await,
                Work::Uninstall(target, after) => {
                    if let Some(mut after) = after {
                        let _ = after.changed().await;
                    }
                    uninstall::run(&job, target).await
                }
            };
            let _ = job.send(Progress::Done(result)).await;
            drop(ended);
        });
    }

    fn change(&mut self, change: Change, out: &mpsc::WeakSender<Message>) -> Result<Value, Denied> {
        match change {
            Change::Start(destination, by, windows) => {
                if let Some(now) = self
                    .active
                    .iter()
                    .rev()
                    .find(|now| now.seen.destination == destination.as_str())
                {
                    if now.seen.kind == "uninstall" {
                        return Err(refuse(
                            409,
                            format!(
                                "Kuvo Code is being uninstalled from {} now. Add it again once that’s done.",
                                now.seen.name
                            ),
                        ));
                    }
                    return Ok(json!({"try": now.seen}));
                }
                let id = format!(
                    "add_{}",
                    hex::encode(crate::random::<12>().map_err(|_| unavailable())?)
                );
                let seen = Attempt::new(id, &destination);
                let revisions = self
                    .files
                    .read::<crate::model::Link>(crate::actor::LINK)
                    .ok()
                    .flatten()
                    .and_then(|link| link.member_list().ok())
                    .and_then(|mut roster| roster.extra.remove("removalRevisions"))
                    .and_then(|revisions| serde_json::from_value(revisions).ok())
                    .unwrap_or_default();
                self.spawn(
                    seen.clone(),
                    by,
                    windows,
                    out,
                    Work::Add(destination, revisions),
                );
                Ok(json!({"try": seen}))
            }
            Change::Uninstall(target, by) => {
                let mut after = None;
                if let Some(now) = self
                    .active
                    .iter_mut()
                    .rev()
                    .find(|now| now.seen.destination == target.destination.as_str())
                {
                    if now.seen.kind == "uninstall" {
                        return Ok(json!({"try": now.seen}));
                    }
                    now.cancel();
                    after = Some(now.done.clone());
                }
                let id = format!(
                    "add_{}",
                    hex::encode(crate::random::<12>().map_err(|_| unavailable())?)
                );
                let seen = Attempt::uninstall(id, &target);
                self.spawn(
                    seen.clone(),
                    by,
                    WindowsMode::Wsl,
                    out,
                    Work::Uninstall(target, after),
                );
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
                                    && input["answer"].as_str().is_some_and(|id| {
                                        question["answers"].as_array().is_some_and(|answers| {
                                            answers.iter().any(|answer| answer["id"] == id)
                                        })
                                    })
                            })
                    })
                    .ok_or_else(|| refuse(409, "It isn’t asking that now."))?;
                now.seen.question = None;
                if input["answer"] != "cancel" {
                    if let Some(answer) = now.answer.take() {
                        let _ = answer.send(());
                    }
                    now.seen.state = State::Working;
                } else {
                    now.cancel();
                }
                Ok(json!({"try": now.seen}))
            }
            Change::Cancel(id) => {
                if let Some(now) = self.active.iter_mut().find(|now| now.seen.id == id) {
                    now.cancel();
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
        if now.stop.load(Ordering::Acquire)
            && !matches!(
                progress,
                Progress::Done(_) | Progress::UninstallCommitted(_)
            )
        {
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
                let here = local_name(&self.setup);
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
            Progress::RecoverAccount(name, answer) => {
                self.asks += 1;
                now.seen.question = Some(json!({
                    "id": "recoverAccount", "title": format!("Continue adding {name}?"),
                    "message": format!("An earlier attempt to add {name} didn't finish, and it's no longer running. Kuvo can clear it and start again."),
                    "at": self.asks, "facts": [],
                    "answers": [{"id":"cancel","label":"Cancel","kind":"plain"},{"id":"recover","label":"Clear and Continue","kind":"primary"}],
                }));
                now.seen.state = State::Asking;
                now.answer = Some(answer);
            }
            Progress::UninstallCommitted(save) => {
                now.committed = true;
                now.stop.store(false, Ordering::Release);
                now.seen.save = save;
                now.seen.state = State::Working;
                now.seen.title = format!("Uninstalling Kuvo Code from {}…", now.seen.name);
            }
            Progress::Done(result) => {
                let mut now = self.active.remove(index);
                if now.stop.load(Ordering::Acquire)
                    && result
                        .as_ref()
                        .is_err_and(|error| matches!(error.outcome, FailureOutcome::Operation))
                {
                    return;
                }
                match result {
                    Ok(completed) => {
                        now.seen.state = State::Done;
                        now.seen.question = None;
                        now.seen.computer = match completed {
                            Completed::Known(computer) => {
                                now.seen.known = true;
                                now.seen.name.clone_from(&computer.name);
                                now.seen.title = format!(
                                    "{} is {}, already one of your computers",
                                    now.seen.destination, computer.name
                                );
                                Some(computer)
                            }
                            Completed::New(computer) => {
                                now.seen.title =
                                    format!("{} is one of your computers", now.seen.name);
                                Some(computer)
                            }
                            Completed::Uninstalled => {
                                now.seen.title =
                                    format!("{} no longer has Kuvo Code running", now.seen.name);
                                None
                            }
                        };
                        for step in &mut now.seen.steps {
                            if step.state == StepState::Now || now.seen.kind == "add" {
                                step.state = StepState::Done;
                            }
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
    reservations: Arc<Mutex<HashSet<String>>>,
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

    async fn admit(&self) -> Result<(), String> {
        let reply = self
            .setup
            .request_command(Command::SshAdmission(self.by.clone()))
            .await
            .map_err(|error| error.to_string())?;
        if self.stop.load(Ordering::Acquire) {
            return Err("The Add was stopped.".into());
        }
        match reply.status {
            200 => Ok(()),
            403 => Err(
                "The device that started this add is no longer paired. Start a new add from one of your devices."
                    .into(),
            ),
            _ => Err(reply.value["message"]
                .as_str()
                .unwrap_or("The Add was stopped.")
                .into()),
        }
    }

    async fn add(
        &self,
        destination: Address,
        revisions: RemovalRevisions,
    ) -> Result<Completed, AddError> {
        let name = preferences::truncate(destination.as_str(), 100);
        self.step("checking", StepState::Now, &format!("Connecting to {name}"))
            .await?;
        let remote = Destination::inspect(self, destination.clone(), &name).await?;
        self.admit().await?;
        let mut account = remote.reserve(self).await?;
        let result = self
            .add_at(&remote, &destination, account.as_mut(), revisions)
            .await;
        if let Some(account) = account
            && let Err(message) = account.release(result.as_ref().err()).await
        {
            return Err(AddError {
                title: None,
                message,
                outcome: FailureOutcome::ReservationCleanup(result.ok()),
                again: true,
            });
        }
        result
    }

    async fn add_at(
        &self,
        remote: &Destination,
        destination: &Address,
        mut account: Option<&mut connect::Reservation<'_>>,
        revisions: RemovalRevisions,
    ) -> Result<Completed, AddError> {
        let name = remote.name();
        if let Some(existing) = remote.inspect_existing(account.as_deref_mut()).await? {
            if let Some(identity) = &existing.identity {
                let result = self
                    .setup
                    .request_command(Command::SshAlias {
                        identity: identity.clone(),
                        key: existing.key.clone(),
                        destination: destination.as_str().into(),
                        name: name.into(),
                        revisions,
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
                        .into());
                }
            }
            return Err(AddError::occupied(name));
        }
        if name != preferences::truncate(destination.as_str(), 100) {
            self.send(Progress::Name(name.to_owned())).await?;
        }
        self.admit().await?;
        self.step("files", StepState::Now, "").await?;
        remote.install(self, account.as_deref_mut()).await?;
        self.admit().await?;
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
        self.admit().await?;
        let (computer, identity) = remote.enroll(self, account, name, code).await?;
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

fn local_name(setup: &SetupHandle) -> String {
    let view = setup.views.borrow();
    view.setup["computers"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|peer| peer["key"] == view.setup["you"]["key"])
        .and_then(|peer| peer["name"].as_str())
        .unwrap_or("This computer")
        .to_owned()
}

pub(crate) fn saved_machines(files: &Files) -> Result<Vec<Value>, crate::Error> {
    let unreadable = || crate::Error::Invalid("saved list of machines");
    if let Some(records) = files.read::<Vec<Value>>("machines.json")? {
        if records.iter().all(|row| row["destination"].is_string()) {
            return Ok(records);
        }
        return Err(unreadable());
    }
    if let Some(links) =
        kept_hosts_files(files).read::<serde_json::Map<String, Value>>(PHONE_LINK)?
        && let Some(machines) = links.get("machines").filter(|value| !value.is_null())
        && !machines.as_object().is_some_and(|items| items.is_empty())
    {
        return Err(unreadable());
    }
    Ok(Vec::new())
}

fn kept_hosts_files(files: &Files) -> Files {
    if cfg!(target_os = "macos")
        && std::env::var_os("KUVO_BOX").is_some_and(|kind| kind == "mac")
        && let Some(parent) = files.root().parent()
    {
        Files::new(parent.to_owned())
    } else {
        files.clone()
    }
}

pub(crate) fn keep_ssh_host(
    files: &Files,
    destination: &str,
    peer: &crate::model::Peer,
) -> Result<(), crate::Error> {
    let files = kept_hosts_files(files);
    let path = files.path("phone-link.json.lock");
    let _lock = kuvo_store::locks::OwnedLock::acquire(&path, std::time::Duration::from_secs(2))
        .map_err(|source| crate::Error::File {
            action: "locking the saved SSH hosts at",
            path,
            source,
        })?;
    let mut whole = files
        .read::<serde_json::Map<String, Value>>(PHONE_LINK)?
        .unwrap_or_default();
    let mut machines = whole
        .remove("machines")
        .and_then(|value| match value {
            Value::Object(machines) => Some(machines),
            _ => None,
        })
        .unwrap_or_default();
    machines.insert(
        destination.into(),
        json!({
            "name": peer.name, "address": peer.address, "key": peer.key,
            "sign": peer.sign, "anotherMac": false, "peers": true,
        }),
    );
    whole.insert("machines".into(), Value::Object(machines));
    files.write(PHONE_LINK, &whole)
}

pub(crate) fn saved_ssh_destination(
    files: &Files,
    saved: &[Value],
    link: &crate::model::Link,
    peer: &crate::model::Peer,
) -> Result<Option<String>, crate::Error> {
    let kept = kept_hosts_files(files)
        .read::<Value>(PHONE_LINK)?
        .unwrap_or_default();
    Ok(saved.iter().find_map(|row| {
        let destination = row["destination"].as_str()?;
        if let Some(bound) = kept["machines"][destination]["key"].as_str() {
            return (bound == peer.key).then(|| destination.to_owned());
        }
        // Earlier Rust enrollments kept their address on the roster. The local
        // issuer's retained claim ties that record to the same peer.
        (peer.extra.get("id").and_then(Value::as_str) == Some(destination)
            && link.installer_invites.values().any(|invite| {
                invite
                    .claim
                    .as_ref()
                    .is_some_and(|claim| claim.matches(peer))
            }))
        .then(|| destination.to_owned())
    }))
}

pub(crate) fn forget_ssh_destination(
    files: &Files,
    link: &crate::model::Link,
    destination: &str,
) -> Result<(), crate::Error> {
    let mut saved = saved_machines(files)?;
    if let Some(kept) = kept_hosts_files(files).read::<Value>(PHONE_LINK)?
        && let Some(key) = kept["machines"][destination]["key"].as_str()
        && link.member_list()?.boxes.iter().any(|peer| peer.key == key)
    {
        return Ok(());
    }
    let before = saved.len();
    saved.retain(|row| row["destination"] != destination);
    if saved.len() != before {
        files.write("machines.json", &saved)?;
    }
    Ok(())
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
}

fn body(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or(Value::Null)
}

fn refuse(status: u16, message: impl Into<String>) -> Denied {
    Denied::new(status, message.into())
}

fn unavailable() -> Denied {
    refuse(
        503,
        "Kuvo’s local server did not answer. Check the add attempts before retrying.",
    )
}

fn start(value: Value, by: Submitter) -> Result<Change, Denied> {
    if value.get("replace").is_some_and(|item| {
        !item
            .as_str()
            .is_some_and(|v| crate::model::safe_token(v, 43, 43))
    }) {
        return Err(refuse(400, "Choose the previous membership by its key."));
    }
    if value
        .get("discardLocalPairings")
        .is_some_and(|item| !item.is_boolean())
    {
        return Err(refuse(400, "Choose whether to discard local pairings."));
    }
    let destination = Address::read(value["destination"].as_str().unwrap_or(""))
        .map_err(|message| refuse(400, message))?;
    let selection = crate::seed::source::Selection::read(value.get("credentials").cloned())
        .ok_or_else(|| refuse(400, "Choose sign-ins and keys by their checklist IDs."))?;
    if !selection.empty() {
        return Err(refuse(
            503,
            "Some selected sign-ins or keys are no longer available. Open the checklist again.",
        ));
    }
    let windows = match value.get("windows") {
        None => WindowsMode::Wsl,
        Some(value) if value == "native" => WindowsMode::Native,
        Some(value) if value == "wsl" => WindowsMode::Wsl,
        Some(_) => return Err(refuse(400, "Choose native Windows or WSL.")),
    };
    Ok(Change::Start(destination, by, windows))
}
