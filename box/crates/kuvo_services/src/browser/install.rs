//! The pinned Chrome for Testing build, downloaded in the background. Only
//! setup runs at startup; Chrome itself starts on a browsing request.
mod archive;
mod download;

use download::{Expected, download};
use futures_util::{StreamExt, stream};
use kuvo_protocol::routes::{get_kuvo_browser as get, post_kuvo_browser as post};
use kuvo_server::{Access, Registry, RouteReply};
use kuvo_store::{files, locks::OwnedLock};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
    time::{Instant, sleep_until},
};
use tokio_util::sync::CancellationToken;

const MAC_PIN: &str = include_str!("../../../../src/link/browser/mac.json");
const LINUX_PIN: &str = include_str!("../../../../src/link/browser/linux.json");
const MAC_LAUNCHER: &str = "#!/bin/sh\nexec \"$(dirname \"$0\")/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing\" \"$@\"\n";

#[derive(Clone, Deserialize)]
struct Chrome {
    url: String,
    #[serde(flatten)]
    expected: Expected,
}
#[derive(Clone, Deserialize)]
struct Package {
    name: String,
    version: String,
    repo: String,
    path: String,
    #[serde(flatten)]
    expected: Expected,
}
struct Pin {
    version: String,
    id: String,
    arch: String,
    mac: bool,
    chrome: Chrome,
    packages: Vec<Package>,
}
impl Pin {
    fn read(real: bool) -> Result<Self, String> {
        let system = if !real {
            std::env::var("KUVO_BROWSER_SYSTEM")
                .ok()
                .filter(|s| matches!(s.as_str(), "darwin" | "linux"))
        } else {
            None
        };
        let system = system.as_deref().unwrap_or(if cfg!(target_os = "macos") {
            "darwin"
        } else if cfg!(windows) {
            "win32"
        } else {
            "linux"
        });
        let mac = match system {
            "darwin" => true,
            "linux" => false,
            other => {
                return Err(format!(
                    "The browser runs on Linux and macOS; this machine is {other}."
                ));
            }
        };
        let arch = std::env::consts::ARCH;
        if !matches!(arch, "aarch64" | "x86_64") {
            return Err(format!(
                "The browser runs on machines with x86-64 or ARM64 processors; this one is {arch}."
            ));
        }
        let fixture = (!real)
            .then(|| std::env::var_os("KUVO_BROWSER_PINS"))
            .flatten()
            .and_then(|p| fs::read(p).ok())
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| v.get(system).cloned());
        let pin = fixture.unwrap_or_else(|| {
            serde_json::from_str(if mac { MAC_PIN } else { LINUX_PIN })
                .expect("built-in browser pins")
        });
        let version = pin["chromium"]["version"]
            .as_str()
            .ok_or("No pinned browser version.")?
            .to_owned();
        if version.is_empty() || !version.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
            return Err("No pinned browser version.".into());
        }
        let hash = hex::encode(Sha256::digest(pin.to_string().as_bytes()));
        let id = format!("{version}-{}{}", if mac { "mac-" } else { "" }, &hash[..8]);
        let chrome =
            serde_json::from_value(pin["chromium"][arch].clone()).map_err(|e| e.to_string())?;
        let packages = if mac {
            vec![]
        } else {
            serde_json::from_value(pin["packages"][arch].clone()).map_err(|e| e.to_string())?
        };
        Ok(Self {
            version,
            id,
            arch: arch.into(),
            mac,
            chrome,
            packages,
        })
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Step {
    Missing,
    Checking,
    Downloading,
    Installing,
    Ready,
    Failed,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<Step>,
    #[serde(skip_serializing_if = "Option::is_none")]
    progress: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failures: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    repaired_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    repaired_id: Option<String>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    state: Step,
    chromium: Option<String>,
    installed: Option<String>,
    progress: Option<f64>,
    message: Option<String>,
    retry_at: Option<i64>,
}
fn now() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}
fn at_least(a: &str, b: &str) -> bool {
    let a: Vec<_> = a
        .split('.')
        .map(|n| n.parse::<u64>().unwrap_or_default())
        .collect();
    let b: Vec<_> = b
        .split('.')
        .map(|n| n.parse::<u64>().unwrap_or_default())
        .collect();
    (0..a.len().max(b.len()))
        .map(|i| a.get(i).unwrap_or(&0).cmp(b.get(i).unwrap_or(&0)))
        .find(|c| !c.is_eq())
        .is_none_or(|c| c.is_gt())
}
fn has_browser(folder: &Path) -> bool {
    ["chrome", "chrome-headless-shell"].iter().any(|name| {
        fs::metadata(folder.join(name)).is_ok_and(|m| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                m.is_file() && m.permissions().mode() & 0o111 != 0
            }
            #[cfg(not(unix))]
            {
                m.is_file()
            }
        })
    })
}
fn in_use(folder: &Path) -> Option<(PathBuf, String)> {
    let build = folder.join("current").canonicalize().ok()?;
    if !has_browser(&build) {
        return None;
    }
    let version = files::text_of(&build.join("chromium-version"))
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "0".into());
    Some((build, version))
}
fn real_home(home: &Path) -> bool {
    let actual = kuvo_os::platform::Account::current()
        .ok()
        .map(|account| account.home.join(".kuvo-code"));
    let resolved = home.canonicalize().unwrap_or_else(|_| home.to_owned());
    actual.is_some_and(|p| p.canonicalize().unwrap_or(p) == resolved)
        || home.parent().and_then(Path::file_name).is_some_and(|name| {
            name.to_string_lossy()
                .to_lowercase()
                .starts_with("run.kuvo.code")
        })
}
struct Setup {
    home: PathBuf,
    folder: PathBuf,
    pin: Result<Pin, String>,
    mac: bool,
    downloads: bool,
    source: Option<String>,
    client: reqwest::Client,
}
impl Setup {
    fn new(home: PathBuf, mac: bool) -> io::Result<Self> {
        let real = real_home(&home);
        let source = (!real)
            .then(|| std::env::var("KUVO_BROWSER_SOURCE").ok())
            .flatten()
            .filter(|s| !s.is_empty());
        let downloads = !mac && (real || source.is_some());
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .read_timeout(Duration::from_secs(60))
            .build()
            .map_err(io::Error::other)?;
        Ok(Self {
            folder: home.join("browser"),
            home,
            pin: Pin::read(real),
            mac,
            downloads,
            source,
            client,
        })
    }
    fn record(&self) -> Record {
        files::read(&self.home.join("run/browser-install.json"))
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
    fn status(&self, record: &Record, active: bool) -> Status {
        let mut status = Status {
            state: Step::Missing,
            chromium: self.pin.as_ref().ok().map(|p| p.version.clone()),
            installed: in_use(&self.folder).map(|(_, v)| v),
            progress: None,
            message: None,
            retry_at: None,
        };
        if self.mac {
            status.chromium = None;
            status.installed = None;
            status.message = Some("This Mac's agents browse in its app's panel.".into());
            return status;
        }
        let pin = match &self.pin {
            Ok(pin) => pin,
            Err(why) => {
                status.state = Step::Failed;
                status.message = Some(why.clone());
                return status;
            }
        };
        if let Some(step @ (Step::Checking | Step::Downloading | Step::Installing)) = record.state {
            if active {
                status.state = step;
                if step == Step::Downloading {
                    status.progress = Some(record.progress.unwrap_or(0.0));
                }
                return status;
            }
            status.state = Step::Failed;
            status.message = Some(
                "Setting up the browser stopped before it finished. It starts again in a minute."
                    .into(),
            );
            return status;
        }
        if status
            .installed
            .as_ref()
            .is_some_and(|v| at_least(v, &pin.version))
        {
            status.state = Step::Ready;
            return status;
        }
        if record.state == Some(Step::Failed) && record.id.as_ref().is_none_or(|id| id == &pin.id) {
            status.state = Step::Failed;
            status.message = Some(
                record
                    .message
                    .clone()
                    .unwrap_or_else(|| "The browser couldn't be set up.".into()),
            );
            status.retry_at = record.retry_at;
            return status;
        }
        if status.installed.is_some() {
            status.state = Step::Ready;
        } else if !self.downloads {
            status.message =
                Some("Only a machine's own Kuvo Code folder downloads the browser.".into());
        }
        status
    }
    fn note(
        &self,
        record: &mut Record,
        step: Step,
        progress: Option<f64>,
        message: Option<String>,
        changed: &watch::Sender<Status>,
    ) {
        record.state = Some(step);
        record.progress = progress;
        record.message = message;
        record.at = Some(now());
        let mut value = serde_json::to_value(&record).unwrap_or_default();
        if matches!(step, Step::Ready | Step::Failed) && record.retry_at.is_none() {
            value["retryAt"] = Value::Null;
        }
        let _ = files::write_secret(
            &self.home.join("run/browser-install.json"),
            value.to_string().as_bytes(),
        );
        let next = if step == Step::Failed {
            record
                .retry_at
                .map_or(9_999_999_999, |ms| (ms + 999) / 1000)
        } else {
            0
        };
        let _ = files::write_secret(
            &self.home.join("run/browser.next"),
            format!("{next}\n").as_bytes(),
        );
        changed.send_replace(self.status(record, true));
    }
    fn repair(&self, record: &Record, asked: Ask) -> Option<bool> {
        if !self.downloads {
            return None;
        }
        let pin = self.pin.as_ref().ok()?;
        let current = in_use(&self.folder);
        let waits = !asked.now
            && record.failures.unwrap_or_default() > 0
            && record.id.as_ref() == Some(&pin.id)
            && record.retry_at.is_none_or(|at| at > now());
        let repair = asked.repair
            && current.is_some()
            && !waits
            && (asked.now
                || record.repaired_id.as_ref() != Some(&pin.id)
                || now() - record.repaired_at.unwrap_or_default() >= 6 * 3_600_000);
        if !repair && current.is_some_and(|(_, v)| at_least(&v, &pin.version)) || waits {
            return None;
        }
        Some(repair)
    }
}
#[derive(Clone, Copy, Default)]
struct Ask {
    now: bool,
    repair: bool,
}
struct Request {
    ask: Ask,
    reply: oneshot::Sender<Status>,
}
#[derive(Clone)]
pub(super) struct Installer {
    home: PathBuf,
    requests: mpsc::Sender<Request>,
    status: watch::Receiver<Status>,
}
pub struct InstallTask {
    handle: Installer,
    stop: CancellationToken,
    task: Option<JoinHandle<()>>,
}
impl InstallTask {
    pub fn start(home: PathBuf, mac: bool) -> io::Result<Self> {
        let setup = Arc::new(Setup::new(home.clone(), mac)?);
        let record = setup.record();
        let (updates, status) = watch::channel(setup.status(&record, false));
        let (requests, inbox) = mpsc::channel(8);
        let stop = CancellationToken::new();
        let task = tokio::spawn(actor(setup, record, updates, inbox, stop.clone()));
        Ok(Self {
            handle: Installer {
                home,
                requests,
                status,
            },
            stop,
            task: Some(task),
        })
    }
    pub fn browser_task(
        &self,
        engine: kuvo_engine::Engine,
    ) -> io::Result<super::manager::BrowserTask> {
        super::manager::BrowserTask::start(self.handle.home.clone(), engine, self.handle.clone())
    }
    pub fn register(&self, routes: &mut Registry) {
        let read = self.handle.clone();
        routes.register::<get::Spec, _, _>(Access::Any, move |_| {
            let status = read.status.borrow().clone();
            async move { Ok(RouteReply::json_as_kept::<200>(json!(status))) }
        });
        let start = self.handle.clone();
        routes.register::<post::Spec, _, _>(Access::Any, move |call| {
            let start = start.clone();
            let body: Value = serde_json::from_slice(&call.request.body).unwrap_or_default();
            async move {
                let status = start
                    .ask(Ask {
                        now: body["now"] == true,
                        repair: body["repair"] == true,
                    })
                    .await;
                Ok(RouteReply::json_as_kept::<202>(json!(status)))
            }
        });
    }
    pub async fn stop(mut self) {
        self.stop.cancel();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}
impl Drop for InstallTask {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
impl Installer {
    pub(super) async fn ensure(&self) -> io::Result<PathBuf> {
        let folder = self.home.join("browser");
        if let Some((build, _)) = in_use(&folder) {
            return Ok(build);
        }
        let first = self.ask(Ask::default()).await;
        let mut status = self.status.clone();
        if !matches!(
            first.state,
            Step::Checking | Step::Downloading | Step::Installing | Step::Ready
        ) {
            return Err(io::Error::other(
                first
                    .message
                    .unwrap_or_else(|| "The browser couldn't start.".into()),
            ));
        }
        tokio::time::timeout(Duration::from_secs(900), async {
            loop {
                if let Some((build, _)) = in_use(&folder) {
                    return Ok(build);
                }
                let current = status.borrow().clone();
                if current.state == Step::Failed {
                    return Err(io::Error::other(
                        current
                            .message
                            .unwrap_or_else(|| "The browser couldn't start.".into()),
                    ));
                }
                if status.changed().await.is_err() {
                    return Err(io::Error::other("The browser's setup stopped."));
                }
            }
        })
        .await
        .map_err(|_| io::Error::other("The browser didn't start in time."))?
    }
    async fn ask(&self, ask: Ask) -> Status {
        let (reply, answer) = oneshot::channel();
        if self.requests.send(Request { ask, reply }).await.is_ok() {
            if let Ok(status) = answer.await {
                return status;
            }
        }
        self.status.borrow().clone()
    }
}
async fn actor(
    setup: Arc<Setup>,
    mut record: Record,
    updates: watch::Sender<Status>,
    mut inbox: mpsc::Receiver<Request>,
    stop: CancellationToken,
) {
    let mut next = Some(Instant::now());
    loop {
        let (ask, reply) = tokio::select! {
            ()=stop.cancelled()=>return,
            request=inbox.recv()=>match request{Some(request)=>(request.ask,Some(request.reply)),None=>return},
            ()=async {match next {Some(at)=>sleep_until(at).await,None=>std::future::pending().await}}=>(Ask::default(),None),
        };
        next = None;
        let Some(repair) = setup.repair(&record, ask) else {
            if setup.downloads
                && setup.pin.as_ref().is_ok_and(|pin| {
                    in_use(&setup.folder).is_some_and(|(_, v)| at_least(&v, &pin.version))
                })
                && record.state != Some(Step::Ready)
            {
                setup.note(&mut record, Step::Ready, None, None, &updates);
            }
            let status = setup.status(&record, false);
            if setup.downloads && status.state == Step::Failed {
                next = record
                    .retry_at
                    .filter(|at| *at > now())
                    .map(|at| Instant::now() + Duration::from_millis((at - now()).max(1) as u64));
            }
            updates.send_replace(status.clone());
            if let Some(reply) = reply {
                let _ = reply.send(status);
            }
            continue;
        };
        let before = record.clone();
        setup.note(&mut record, Step::Checking, None, None, &updates);
        if let Some(reply) = reply {
            let _ = reply.send(updates.borrow().clone());
        }
        let running = Arc::new(Mutex::new(record.clone()));
        let job = install(&setup, running.clone(), updates.clone(), repair);
        tokio::pin!(job);
        let result = loop {
            tokio::select! {
                ()=stop.cancelled()=>{
                    let unpacking=updates.borrow().state==Step::Installing;
                    if unpacking {let _=(&mut job).await;}
                    return;
                },
                result=&mut job=>break result,
                request=inbox.recv()=>match request{Some(request)=>{let _=request.reply.send(updates.borrow().clone());},None=>return},
            }
        };
        record = running.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let pin = setup.pin.as_ref().expect("a pinned install");
        match result {
            Ok(()) => {
                record.failures = Some(0);
                record.retry_at = None;
                if repair {
                    record.repaired_id = Some(pin.id.clone());
                    record.repaired_at = Some(now());
                }
                setup.note(&mut record, Step::Ready, None, None, &updates);
            }
            Err(error) => {
                let count = if before.id.as_ref() == Some(&pin.id) {
                    before.failures.unwrap_or(0) + 1
                } else {
                    1
                };
                record.id = Some(pin.id.clone());
                record.failures = Some(count);
                let retry = error.kind() != io::ErrorKind::NotFound
                    && error.kind() != io::ErrorKind::InvalidData;
                let delay = [60_000, 300_000, 900_000]
                    .get(count.saturating_sub(1) as usize)
                    .copied()
                    .unwrap_or(3_600_000);
                record.retry_at = retry.then(|| now() + delay);
                let mut message = error.to_string().chars().take(400).collect::<String>();
                if retry {
                    let words = match delay {
                        60_000 => "in a minute".into(),
                        3_600_000 => "in an hour".into(),
                        n => format!("in {} minutes", n / 60_000),
                    };
                    message.push_str(&format!(" It tries again {words}."));
                    next = Some(Instant::now() + Duration::from_millis(delay as u64));
                }
                setup.note(&mut record, Step::Failed, None, Some(message), &updates);
            }
        }
    }
}
async fn install(
    setup: &Setup,
    record: Arc<Mutex<Record>>,
    changed: watch::Sender<Status>,
    repair: bool,
) -> io::Result<()> {
    let pin = setup
        .pin
        .as_ref()
        .map_err(|why| io::Error::other(why.clone()))?;
    let _lock = OwnedLock::acquire(
        &setup.home.join("run/.browser-installing"),
        Duration::from_secs(1),
    )?;
    files::create_private_dirs(&setup.folder)?;
    let total =
        pin.chrome.expected.size + pin.packages.iter().map(|p| p.expected.size).sum::<u64>();
    let needed = total.saturating_mul(4).saturating_add(200_000_000);
    if kuvo_os::platform::available_space(&setup.folder).is_ok_and(|free| free < needed) {
        return Err(io::Error::other(format!(
            "There isn't room for the browser on this machine: it needs about {} GB free where Kuvo Code keeps its files.",
            (needed as f64 / 1e8).ceil() / 10.0
        )));
    }
    if !pin.mac && pin.packages.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "This build of Kuvo Code has no browser for this machine.",
        ));
    }
    let cache = setup.home.join("run/browser-downloads").join(&pin.id);
    files::create_private_dirs(&cache)?;
    let progress = Mutex::new((
        std::collections::HashMap::<String, u64>::new(),
        -1.0,
        Instant::now(),
    ));
    let received = |name: &str, count: u64| {
        let mut progress = progress.lock().unwrap_or_else(|e| e.into_inner());
        progress.0.insert(name.into(), count);
        let ratio = (progress.0.values().sum::<u64>() as f64 / total as f64).min(1.0);
        if ratio - progress.1 >= 0.01
            || ratio == 1.0
            || progress.2.elapsed() > Duration::from_secs(2)
        {
            progress.1 = ratio;
            progress.2 = Instant::now();
            setup.note(
                &mut record.lock().unwrap_or_else(|e| e.into_inner()),
                Step::Downloading,
                Some((ratio * 1000.0).floor() / 1000.0),
                None,
                &changed,
            );
        }
    };
    let chrome = cache.join("chromium.zip");
    if !download(
        &setup.client,
        &pin.chrome.url,
        setup.source.as_deref(),
        &pin.chrome.expected,
        &chrome,
        |count| received("chromium", count),
    )
    .await?
    {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "storage.googleapis.com doesn't have the browser this Kuvo Code pins (Chrome for Testing {}): update Kuvo Code.",
                pin.version
            ),
        ));
    }
    let mut packages=stream::iter(pin.packages.clone().into_iter().enumerate()).map(|(i,package)|{
        let file=cache.join(format!("{i}.rpm"));let received=&received;
        async move{
            for base in ["https://dl.rockylinux.org/pub/rocky/8","https://dl.rockylinux.org/vault/rocky/8.10"] {
                let address=format!("{base}/{}/{}/os/{}",package.repo,pin.arch,package.path);
                if download(&setup.client,&address,setup.source.as_deref(),&package.expected,&file,|count|received(&package.name,count)).await? {return Ok((i,file));}
            }
            Err(io::Error::new(io::ErrorKind::NotFound,format!("Rocky Linux no longer has {} {}, which the browser needs: update Kuvo Code.",package.name,package.version)))
        }
    }).buffer_unordered(4);
    let mut downloaded = Vec::new();
    while let Some(result) = packages.next().await {
        downloaded.push(result?);
    }
    downloaded.sort_by_key(|(i, _)| *i);
    setup.note(
        &mut record.lock().unwrap_or_else(|e| e.into_inner()),
        Step::Installing,
        None,
        None,
        &changed,
    );
    let (folder, id, version, mac) = (
        setup.folder.clone(),
        pin.id.clone(),
        pin.version.clone(),
        pin.mac,
    );
    tokio::task::spawn_blocking(move || {
        put_build(&folder, &id, &version, mac, &chrome, downloaded, repair)
    })
    .await
    .map_err(io::Error::other)??;
    files::remove_tree_synced(&cache)?;
    Ok(())
}
fn put_build(
    folder: &Path,
    id: &str,
    version: &str,
    mac: bool,
    chrome: &Path,
    packages: Vec<(usize, PathBuf)>,
    _repair: bool,
) -> io::Result<()> {
    let stage = tempfile::Builder::new()
        .prefix(".box-staging-")
        .tempdir_in(folder)?;
    let bundle = stage.path().join("bundle");
    let unpacked = stage.path().join("chromium");
    for part in [
        bundle.join("lib"),
        bundle.join("fonts"),
        bundle.join("licenses"),
        unpacked.clone(),
    ] {
        fs::create_dir_all(part)?;
    }
    for (_, package) in packages {
        archive::package(&package, &bundle)?;
    }
    archive::unzip(chrome, &unpacked)?;
    let top = fs::read_dir(&unpacked)?
        .filter_map(Result::ok)
        .find(|entry| {
            entry.file_type().is_ok_and(|ty| ty.is_dir())
                && entry.file_name().to_string_lossy().starts_with("chrome-")
        })
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "The browser's download has no browser in it.",
            )
        })?;
    for entry in fs::read_dir(top.path())? {
        let entry = entry?;
        let to = bundle.join(entry.file_name());
        if fs::symlink_metadata(&to).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "The browser's download has {} twice.",
                    entry.file_name().to_string_lossy()
                ),
            ));
        }
        fs::rename(entry.path(), to)?;
    }
    if mac {
        fs::write(bundle.join("chrome"), MAC_LAUNCHER)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(bundle.join("chrome"), fs::Permissions::from_mode(0o755))?;
        }
    }
    fs::write(bundle.join("chromium-version"), format!("{version}\n"))?;
    fs::write(
        bundle.join("kuvo-version"),
        format!("{}\n", env!("CARGO_PKG_VERSION")),
    )?;
    if !has_browser(&bundle)
        || (mac
            && !bundle
                .join("Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing")
                .is_file())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "The browser's download has no browser in it.",
        ));
    }
    archive::links_inside(&bundle)?;
    let target = folder.join(id);
    if !(target.is_dir() && has_browser(&target) && running(&target)) {
        files::remove_tree_synced(&target)?;
        fs::rename(&bundle, &target)?;
    }
    let link = folder.join(format!(".current-{}", std::process::id()));
    files::remove_file_synced(&link)?;
    archive::link(Path::new(id), &link)?;
    let current = folder.join("current");
    if fs::symlink_metadata(&current).is_ok_and(|m| m.is_dir())
        && has_browser(&current)
        && !running(&current)
    {
        files::remove_tree_synced(&current)?;
    }
    fs::rename(&link, &current)?;
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        if entry.file_name() != id
            && entry.file_type()?.is_dir()
            && has_browser(&entry.path())
            && !running(&entry.path())
        {
            files::remove_tree_synced(&entry.path())?;
        }
    }
    Ok(())
}
fn running(build: &Path) -> bool {
    let prefix = format!(
        "{}/",
        build
            .canonicalize()
            .unwrap_or_else(|_| build.to_owned())
            .display()
    );
    #[cfg(target_os = "linux")]
    {
        fs::read_dir("/proc").map_or(true, |entries| {
            entries
                .filter_map(Result::ok)
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .bytes()
                        .all(|b| b.is_ascii_digit())
                })
                .any(|e| {
                    fs::read(e.path().join("cmdline"))
                        .is_ok_and(|b| String::from_utf8_lossy(&b).contains(&prefix))
                })
        })
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/bin/ps")
            .args(["-axww", "-o", "command="])
            .output()
            .map_or(true, |o| {
                !o.status.success() || String::from_utf8_lossy(&o.stdout).contains(&prefix)
            })
    }
    #[cfg(not(unix))]
    {
        let _ = prefix;
        true
    }
}
