//! Chrome's lifetime belongs to the resident. One launch, shared by viewers
//! and tools, and one deadline when the last browser use has finished.
mod access;
mod asks;
mod page;
mod pipe;
mod sites;
mod view;
pub use sites::Sites;

use super::install::Installer;
use access::Access;
pub use asks::BrowserAsks;
use kuvo_engine::Engine;
use kuvo_store::files;
use pipe::Pipe;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{Mutex as AsyncMutex, Notify, Semaphore, broadcast, mpsc, oneshot},
    task::JoinHandle,
    time::Instant,
};
use tokio_util::sync::CancellationToken;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Config {
    scripts: HashMap<String, String>,
    search_engines: Vec<SearchEngine>,
}
#[derive(Deserialize)]
struct SearchEngine {
    id: String,
    url: String,
}
impl Config {
    fn script(&self, name: &str) -> &str {
        self.scripts
            .get(name)
            .map(String::as_str)
            .unwrap_or_default()
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Target {
    target_id: String,
    title: String,
    url: String,
    #[serde(rename = "type")]
    kind: String,
    opener_id: Option<String>,
}
#[derive(Clone)]
struct Page {
    target: String,
    session: String,
}
#[derive(Clone)]
struct Claim {
    session: String,
    used: Instant,
}
#[derive(Default)]
struct Tabs {
    targets: Vec<Target>,
    closing: HashSet<String>,
    active: Option<String>,
    attached: HashMap<String, String>,
    claims: HashMap<String, Claim>,
    snapshots: HashMap<String, String>,
    held: HashMap<String, String>,
    seen: HashSet<String>,
    viewers: usize,
    hold_macs: HashMap<String, String>,
    loading: HashSet<String>,
    history: HashMap<String, (bool, bool)>,
    dialogs: HashMap<String, Value>,
    choosers: HashMap<String, u64>,
    working: HashMap<String, bool>,
    sizes: HashMap<String, view::Size>,
    viewer_sizes: HashMap<String, (String, view::Size, Instant)>,
    restores: HashMap<String, CancellationToken>,
    uploads: HashMap<String, Vec<tempfile::TempDir>>,
    frames: HashMap<String, String>,
    notes: HashMap<String, Vec<String>>,
    downloads: HashMap<String, (String, Option<String>)>,
    download_notes: HashMap<String, Vec<String>>,
}
struct Living {
    pipe: Pipe,
    home: PathBuf,
    config: Arc<Config>,
    engine: Engine,
    asks: BrowserAsks,
    tabs: AsyncMutex<Tabs>,
    last_pages: AsyncMutex<Option<Value>>,
    turn: Arc<Semaphore>,
    resize_gate: AsyncMutex<()>,
    changed: Notify,
    events: broadcast::Sender<Value>,
    file_rules: Mutex<HashMap<String, Access>>,
    download_folder: PathBuf,
    captures: Mutex<HashMap<view::ViewKey, std::sync::Weak<AsyncMutex<view::Capture>>>>,
    _profile_lock: kuvo_store::locks::OwnedLock,
    icons: Mutex<HashSet<String>>,
}
impl Living {
    async fn launch(
        home: PathBuf,
        config: Arc<Config>,
        engine: Engine,
        installer: &Installer,
        asks: BrowserAsks,
        events: broadcast::Sender<Value>,
    ) -> io::Result<Arc<Self>> {
        let build = installer.ensure().await?;
        let profile_lock = home.join("browser/.profile-editing");
        let profile_lock = tokio::task::spawn_blocking(move || {
            kuvo_store::locks::OwnedLock::acquire(&profile_lock, Duration::from_secs(10))
        })
        .await
        .map_err(io::Error::other)??;
        #[cfg(unix)]
        let sandbox = unsafe { libc::geteuid() } != 0;
        #[cfg(not(unix))]
        let sandbox = true;
        let pipe = match Pipe::launch(&home, &build, sandbox).await {
            Ok(pipe) => pipe,
            Err(first) if sandbox => {
                let log = files::text_of(&home.join("run/browser.log")).unwrap_or_default();
                let without = log.contains("No usable sandbox")
                    || log.contains("SUID sandbox")
                    || log.contains("user namespace");
                Pipe::launch(&home, &build, !without)
                    .await
                    .map_err(|_| first)?
            }
            Err(error) => return Err(error),
        };
        let download_folder = home.join("browser/downloads");
        files::create_private_dirs(&download_folder)?;
        // A machine browser has no previous Mac panel to take in.
        let taken = home.join("browser/app-data-read");
        if !taken.exists() {
            let _ = files::write_secret(
                &taken,
                json!({"at":jiff::Timestamp::now().as_millisecond(),"from":null})
                    .to_string()
                    .as_bytes(),
            );
        }
        let live = Arc::new(Self {
            pipe,
            home,
            config,
            engine,
            asks,
            tabs: AsyncMutex::default(),
            last_pages: AsyncMutex::default(),
            turn: Arc::new(Semaphore::new(1)),
            resize_gate: AsyncMutex::new(()),
            changed: Notify::new(),
            events,
            file_rules: Mutex::default(),
            download_folder,
            captures: Mutex::default(),
            _profile_lock: profile_lock,
            icons: Mutex::default(),
        });
        let mut events = live.pipe.events();
        let follow = Arc::downgrade(&live);
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(event) => {
                        if let Some(live) = follow.upgrade() {
                            live.event(event).await;
                        } else {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        if let Some(live) = follow.upgrade() {
                            let _ = live.refresh_targets().await;
                        } else {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        files::write_secret(
            &live.home.join("run/browser.pid"),
            live.pipe.pid().to_string().as_bytes(),
        )?;
        live.asks.clear();
        live.pipe
            .call("Target.setDiscoverTargets", json!({"discover":true}), None)
            .await?;
        live.pipe.call("Browser.setDownloadBehavior",json!({"behavior":"allowAndName","downloadPath":live.download_folder,"eventsEnabled":true}),None).await?;
        live.refresh_targets().await?;
        live.clear_pending().await;
        live.reopen().await;
        live.clear_old_downloads();
        Ok(live)
    }
    fn tell(&self, method: &str, params: Value) {
        let _ = self.events.send(json!({"method":method,"params":params}));
    }
    async fn refresh_targets(&self) -> io::Result<Vec<Target>> {
        let answer = self.pipe.call("Target.getTargets", json!({}), None).await?;
        let targets: Vec<Target> =
            serde_json::from_value(answer["targetInfos"].clone()).map_err(io::Error::other)?;
        let mut state = self.tabs.lock().await;
        let mut targets: Vec<_> = targets
            .into_iter()
            .filter(|t| t.kind == "page" && !state.closing.contains(&t.target_id))
            .collect();
        // The keeper's tab numbering is insertion order, not Chrome's unspecified enumeration order.
        let order: HashMap<_, _> = state
            .targets
            .iter()
            .enumerate()
            .map(|(i, t)| (t.target_id.as_str(), i))
            .collect();
        targets.sort_by_key(|t| {
            order
                .get(t.target_id.as_str())
                .copied()
                .unwrap_or(usize::MAX)
        });
        let open: HashSet<_> = targets.iter().map(|t| t.target_id.as_str()).collect();
        state.attached.retain(|key, _| open.contains(key.as_str()));
        state.claims.retain(|key, _| open.contains(key.as_str()));
        state.held.retain(|key, _| open.contains(key.as_str()));
        if state.active.as_deref().is_none_or(|id| !open.contains(id)) {
            state.active = targets.last().map(|t| t.target_id.clone());
        }
        state.targets = targets.clone();
        Ok(targets)
    }
    async fn attach(&self, target: &str) -> io::Result<Page> {
        let mut tabs = self.tabs.lock().await;
        let session = if let Some(id) = tabs.attached.get(target) {
            id.clone()
        } else {
            let attached = self
                .pipe
                .call(
                    "Target.attachToTarget",
                    json!({"targetId":target,"flatten":true}),
                    None,
                )
                .await?;
            let id = attached["sessionId"]
                .as_str()
                .ok_or_else(|| io::Error::other("Couldn't attach to the page."))?
                .to_owned();
            self.pipe.call("Page.enable", json!({}), Some(&id)).await?;
            self.pipe
                .call(
                    "Page.setInterceptFileChooserDialog",
                    json!({"enabled":true}),
                    Some(&id),
                )
                .await?;
            tabs.attached.insert(target.to_owned(), id.clone());
            id
        };
        Ok(Page {
            target: target.into(),
            session,
        })
    }
    async fn use_page(&self, target: &str, session: Option<&str>) -> io::Result<Page> {
        let p = self.attach(target).await?;
        let _ = self
            .pipe
            .call("Target.activateTarget", json!({"targetId":target}), None)
            .await;
        let mut tabs = self.tabs.lock().await;
        let changed = tabs.active.as_deref() != Some(target);
        tabs.active = Some(target.into());
        if let Some(session) = session {
            tabs.claims.insert(
                target.into(),
                Claim {
                    session: session.into(),
                    used: Instant::now(),
                },
            );
        }
        drop(tabs);
        self.publish().await;
        if changed {
            let mut params =
                json!({"targetId":target,"by":if session.is_some(){"agent"}else{"user"}});
            if let Some(session) = session {
                params["session"] = json!(session);
            }
            self.tell("Kuvo.activeChanged", params);
        }
        Ok(p)
    }
    async fn create(&self, session: Option<&str>) -> io::Result<Page> {
        let created = self
            .pipe
            .call(
                "Target.createTarget",
                json!({"url":"about:blank","newWindow":true}),
                None,
            )
            .await?;
        let id = created["targetId"]
            .as_str()
            .ok_or_else(|| io::Error::other("Couldn't open a tab."))?;
        self.refresh_targets().await?;
        self.use_page(id, session).await
    }
    async fn close_page(&self, id: &str) -> io::Result<()> {
        self.tabs.lock().await.closing.insert(id.to_owned());
        if let Err(error) = self
            .pipe
            .call("Target.closeTarget", json!({"targetId":id}), None)
            .await
        {
            self.tabs.lock().await.closing.remove(id);
            return Err(error);
        }
        let mut tabs = self.tabs.lock().await;
        tabs.targets.retain(|t| t.target_id != id);
        tabs.attached.remove(id);
        tabs.claims.remove(id);
        if tabs.active.as_deref() == Some(id) {
            tabs.active = tabs.targets.last().map(|t| t.target_id.clone());
        }
        drop(tabs);
        self.publish().await;
        Ok(())
    }
    async fn choose(
        &self,
        session: Option<&str>,
        access: &Access,
        permit: &mut Option<tokio::sync::OwnedSemaphorePermit>,
    ) -> io::Result<Page> {
        let targets = self.refresh_targets().await?;
        let state = self.tabs.lock().await;
        let own = session.and_then(|session| {
            state
                .claims
                .iter()
                .filter(|(_, c)| c.session == session)
                .max_by_key(|(_, c)| c.used)
                .map(|(id, _)| id.clone())
        });
        let chosen = own
            .or_else(|| state.active.clone())
            .or_else(|| targets.last().map(|t| t.target_id.clone()));
        let chosen = chosen.filter(|id| {
            targets
                .iter()
                .find(|t| t.target_id == *id)
                .is_some_and(|t| access.outside(&t.url).is_none())
                && session.is_none_or(|session| {
                    state.claims.get(id).is_none_or(|claim| {
                        claim.session == session || claim.used.elapsed() > Duration::from_secs(90)
                    })
                })
        });
        drop(state);
        if let Some(id) = chosen {
            let until = Instant::now() + Duration::from_secs(600);
            loop {
                let ready = self.changed.notified();
                if !self.tabs.lock().await.held.contains_key(&id) {
                    if permit.is_none() {
                        *permit = Some(
                            self.turn
                                .clone()
                                .acquire_owned()
                                .await
                                .map_err(io::Error::other)?,
                        );
                        continue;
                    }
                    break;
                }
                permit.take();
                if tokio::time::timeout_at(until, ready).await.is_err() {
                    return Err(io::Error::other(
                        "The user still has this tab. Carry on without it.",
                    ));
                }
            }
            return self.use_page(&id, session).await;
        }
        self.create(session).await
    }
    async fn page_facts(&self) -> Value {
        let state = self.tabs.lock().await;
        let pages:Vec<_>=state.targets.iter().map(|target|{
            let id=&target.target_id;
            let (back,forward)=state.history.get(id).copied().unwrap_or_default();
            let mut seen=json!({"targetId":id,"url":target.url,"title":target.title,"opener":target.opener_id,"session":state.claims.get(id).map(|c|&c.session),"loading":state.loading.contains(id),"canGoBack":back,"canGoForward":forward,"working":state.working.contains_key(id),"held":state.held.contains_key(id)});
            if let Some(dialog)=state.dialogs.get(id){seen["dialog"]=dialog.clone();}seen
        }).collect();
        json!({"active":state.active,"pages":pages})
    }
    async fn publish(&self) {
        let mut last = self.last_pages.lock().await;
        let pages = self.page_facts().await;
        if last.as_ref() != Some(&pages) {
            self.tell("Kuvo.pages", pages.clone());
            *last = Some(pages);
        }
    }
    async fn navigation_state(&self, p: &Page) {
        if let Ok(history) = self
            .pipe
            .call("Page.getNavigationHistory", json!({}), Some(&p.session))
            .await
        {
            let index = history["currentIndex"].as_u64().unwrap_or(0);
            let length = history["entries"].as_array().map_or(1, Vec::len) as u64;
            self.tabs
                .lock()
                .await
                .history
                .insert(p.target.clone(), (index > 0, index + 1 < length));
        }
        let _ = self.refresh_targets().await;
        self.publish().await;
    }
    async fn event(&self, event: pipe::Event) {
        let target = if let Some(session) = event.session_id.as_ref() {
            self.tabs
                .lock()
                .await
                .attached
                .iter()
                .find(|(_, id)| *id == session)
                .map(|(target, _)| target.clone())
        } else {
            None
        };
        match event.method.as_str() {
            "Target.targetCreated" | "Target.targetInfoChanged" => {
                if let Ok(target) =
                    serde_json::from_value::<Target>(event.params["targetInfo"].clone())
                    && target.kind == "page"
                {
                    let id = target.target_id.clone();
                    let mut tabs = self.tabs.lock().await;
                    if tabs.closing.contains(&id) {
                        return;
                    }
                    if let Some(old) = tabs
                        .targets
                        .iter_mut()
                        .find(|old| old.target_id == target.target_id)
                    {
                        *old = target;
                    } else {
                        if tabs.active.is_none() {
                            tabs.active = Some(target.target_id.clone());
                        }
                        tabs.targets.push(target);
                    }
                    drop(tabs);
                    if event.method == "Target.targetCreated" {
                        let _ = self.attach(&id).await;
                    }
                    self.publish().await;
                    self.changed.notify_waiters();
                }
            }
            "Target.targetDestroyed" => {
                if let Some(id) = event.params["targetId"].as_str() {
                    let mut tabs = self.tabs.lock().await;
                    tabs.targets.retain(|t| t.target_id != id);
                    tabs.closing.remove(id);
                    tabs.claims.remove(id);
                    tabs.seen.remove(id);
                    tabs.held.remove(id);
                    tabs.hold_macs.remove(id);
                    tabs.attached.remove(id);
                    tabs.working.remove(id);
                    tabs.uploads.remove(id);
                    tabs.choosers.remove(id);
                    tabs.sizes.remove(id);
                    tabs.viewer_sizes.retain(|_, (target, _, _)| target != id);
                    if let Some(restore) = tabs.restores.remove(id) {
                        restore.cancel();
                    }
                    tabs.frames.retain(|_, target| target != id);
                    tabs.notes.remove(id);
                    if tabs.active.as_deref() == Some(id) {
                        tabs.active = tabs.targets.last().map(|t| t.target_id.clone());
                    }
                    drop(tabs);
                    self.publish().await;
                    self.changed.notify_waiters();
                }
            }
            "Page.frameNavigated" => {
                if let Some(target) = target
                    && let Some(frame) = event.params["frame"]["id"].as_str()
                {
                    let mut tabs = self.tabs.lock().await;
                    tabs.frames.insert(frame.to_owned(), target.clone());
                    tabs.choosers.remove(&target);
                    drop(tabs);
                    self.dirty(&target).await;
                }
            }
            "Page.frameAttached" => {
                if let Some(target) = target
                    && let Some(frame) = event.params["frameId"].as_str()
                {
                    self.tabs
                        .lock()
                        .await
                        .frames
                        .insert(frame.to_owned(), target);
                }
            }
            "Page.frameDetached" => {
                if let Some(frame) = event.params["frameId"].as_str() {
                    self.tabs.lock().await.frames.remove(frame);
                }
            }
            "Page.frameStartedLoading" => {
                if let Some(target) = target {
                    self.tabs.lock().await.loading.insert(target);
                    self.publish().await;
                }
            }
            "Page.frameStoppedLoading" => {
                if let Some(target) = target {
                    self.tabs.lock().await.loading.remove(&target);
                    self.publish().await;
                }
            }
            "Page.frameRequestedNavigation" => {
                if let Some(target) = target
                    && let Some(url) = event.params["url"].as_str()
                    && let Ok(parsed) = url::Url::parse(url)
                    && !matches!(
                        parsed.scheme(),
                        "http" | "https" | "file" | "about" | "javascript" | "data" | "blob"
                    )
                {
                    self.tell("Kuvo.external", json!({"targetId":target,"url":url}));
                }
            }
            "Page.loadEventFired" => {
                if let Some(target) = target {
                    let mut tabs = self.tabs.lock().await;
                    tabs.loading.remove(&target);
                    let entry = tabs.targets.iter().find(|t| t.target_id == target).cloned();
                    drop(tabs);
                    if let Some(entry) = entry {
                        self.note_visit(&entry).await;
                    }
                    self.publish().await;
                }
            }
            "Fetch.requestPaused" => {
                let Some(session) = event.session_id else {
                    return;
                };
                let file = self
                    .file_rules
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(&session)
                    .and_then(|rules| {
                        event.params["request"]["url"]
                            .as_str()
                            .and_then(|url| rules.outside(url))
                    });
                let (method, params) = if let Some(file) = file {
                    if let Some(target) = target {
                        self.tabs.lock().await.notes.entry(target).or_default().push(format!("Not loaded: {file}, outside this conversation's folder; this session asks before reading files elsewhere."));
                    }
                    (
                        "Fetch.failRequest",
                        json!({"requestId":event.params["requestId"],"errorReason":"BlockedByClient"}),
                    )
                } else {
                    (
                        "Fetch.continueRequest",
                        json!({"requestId":event.params["requestId"]}),
                    )
                };
                let _ = self.pipe.call(method, params, Some(&session)).await;
            }
            "Page.javascriptDialogOpening" => {
                if let Some(target) = target {
                    let mut tabs = self.tabs.lock().await;
                    let agent = tabs
                        .working
                        .get(&target)
                        .copied()
                        .filter(|_| !tabs.held.contains_key(&target));
                    if let Some(accept) = agent {
                        let kind = event.params["type"].as_str().unwrap_or_default();
                        let accept = accept || matches!(kind, "alert" | "beforeunload");
                        tabs.notes.entry(target).or_default().push(format!(
                            "The page showed a dialog: “{}” ({}).",
                            event.params["message"].as_str().unwrap_or_default(),
                            if accept { "accepted" } else { "declined" }
                        ));
                        drop(tabs);
                        let _ = self
                            .pipe
                            .call(
                                "Page.handleJavaScriptDialog",
                                json!({"accept":accept,"promptText":event.params["defaultPrompt"]}),
                                event.session_id.as_deref(),
                            )
                            .await;
                    } else {
                        let mut notice = event.params.clone();
                        notice["targetId"] = json!(target);
                        tabs.dialogs.insert(target, event.params);
                        drop(tabs);
                        self.tell("Kuvo.dialog", notice);
                        self.publish().await;
                    }
                }
            }
            "Page.javascriptDialogClosed" => {
                if let Some(target) = target {
                    self.tabs.lock().await.dialogs.remove(&target);
                    self.publish().await;
                }
            }
            "Page.fileChooserOpened" => {
                if let Some(target) = target
                    && let Some(node) = event.params["backendNodeId"].as_u64()
                {
                    self.tabs.lock().await.choosers.insert(target.clone(), node);
                    self.tell(
                        "Kuvo.fileChooser",
                        json!({"targetId":target,"mode":event.params["mode"]}),
                    );
                }
            }
            "Browser.downloadWillBegin" => {
                if let (Some(guid), Some(name)) = (
                    event.params["guid"].as_str(),
                    event.params["suggestedFilename"].as_str(),
                ) {
                    let mut tabs = self.tabs.lock().await;
                    let target = event.params["frameId"]
                        .as_str()
                        .and_then(|frame| tabs.frames.get(frame).cloned());
                    let target =
                        target.or_else(|| event.params["frameId"].as_str().map(str::to_owned));
                    let whose = |target: &str| {
                        if tabs.held.contains_key(target) {
                            return None;
                        }
                        tabs.claims
                            .get(target)
                            .filter(|claim| {
                                tabs.working.contains_key(target)
                                    || claim.used.elapsed() < Duration::from_secs(120)
                            })
                            .map(|claim| claim.session.clone())
                    };
                    let session = target.as_deref().and_then(|target| {
                        whose(target).or_else(|| {
                            if tabs.held.contains_key(target) {
                                return None;
                            }
                            let opener = tabs
                                .targets
                                .iter()
                                .find(|t| t.target_id == target)?
                                .opener_id
                                .as_deref()?;
                            whose(
                                tabs.frames
                                    .get(opener)
                                    .map(String::as_str)
                                    .unwrap_or(opener),
                            )
                        })
                    });
                    tabs.downloads
                        .insert(guid.into(), (name.into(), session.clone()));
                    self.tell(
                        "Kuvo.downloading",
                        download_params(name, session.as_deref()),
                    );
                }
            }
            "Browser.downloadProgress" => {
                if event.params["state"] != "inProgress"
                    && let Some(guid) = event.params["guid"].as_str()
                {
                    let mut tabs = self.tabs.lock().await;
                    if let Some((name, session)) = tabs.downloads.remove(guid) {
                        let file = (event.params["state"] == "completed")
                            .then(|| finish_download(&self.download_folder, guid, &name))
                            .flatten();
                        if let Some(file) = file {
                            if let Some(session) = &session {
                                tabs.download_notes
                                    .entry(session.clone())
                                    .or_default()
                                    .push(format!("Downloaded: {}", file.display()));
                            }
                            let mut params = download_params(
                                file.file_name().and_then(|n| n.to_str()).unwrap_or(&name),
                                session.as_deref(),
                            );
                            params["path"] = json!(file);
                            params["size"] = event.params["totalBytes"].clone();
                            self.tell("Kuvo.downloaded", params);
                        } else {
                            self.tell(
                                "Kuvo.downloadFailed",
                                download_params(&name, session.as_deref()),
                            );
                        }
                    }
                }
            }
            _ => {}
        }
    }
    async fn note_visit(&self, target: &Target) {
        if !target.url.starts_with("https://") && !target.url.starts_with("http://") {
            return;
        }
        let target = self
            .pipe
            .call(
                "Target.getTargetInfo",
                json!({"targetId":target.target_id}),
                None,
            )
            .await
            .ok()
            .and_then(|v| serde_json::from_value::<Target>(v["targetInfo"].clone()).ok())
            .unwrap_or_else(|| target.clone());
        self.note_history(json!({"url":target.url,"title":target.title,"at":jiff::Timestamp::now().as_millisecond()}));
        let Some(origin) = url::Url::parse(&target.url)
            .ok()
            .map(|u| u.origin().ascii_serialization())
        else {
            return;
        };
        if !self
            .icons
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(origin)
        {
            return;
        }
        let p = self
            .tabs
            .lock()
            .await
            .attached
            .get(&target.target_id)
            .cloned()
            .map(|session| Page {
                target: target.target_id.clone(),
                session,
            });
        let Some(p) = p else {
            return;
        };
        let pipe = self.pipe.clone();
        let folder = self.home.join("browser");
        let address = target.url.clone();
        tokio::spawn(async move {
            if let Some(icon) = site_icon(&pipe, &p, &address).await {
                append_history(
                    &folder,
                    json!({"url":address,"icon":icon,"at":jiff::Timestamp::now().as_millisecond()}),
                );
            }
        });
    }
    fn note_history(&self, line: Value) {
        append_history(&self.home.join("browser"), line);
    }
    async fn clear_pending(&self) {
        let file = self.home.join("browser/clear-sites.json");
        let hosts: Vec<String> = files::text_of(&file)
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        if hosts.is_empty() {
            return;
        }
        let active = self.tabs.lock().await.active.clone();
        let Some(active) = active else {
            return;
        };
        let Ok(p) = self.attach(&active).await else {
            return;
        };
        let mut left = Vec::new();
        for host in hosts {
            if !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".:[-]".contains(&b))
            {
                continue;
            }
            let mut ok = true;
            for scheme in ["http", "https"] {
                if self
                    .pipe
                    .call(
                        "Storage.clearDataForOrigin",
                        json!({"origin":format!("{scheme}://{host}"),"storageTypes":"all"}),
                        Some(&p.session),
                    )
                    .await
                    .is_err()
                {
                    ok = false;
                }
            }
            if !ok {
                left.push(host);
            }
        }
        if left.is_empty() {
            let _ = files::remove_file_synced(&file);
        } else {
            let _ = files::write_secret(&file, json!(left).to_string().as_bytes());
        }
    }
    async fn reopen(&self) {
        let file = self.home.join("run/pages.json");
        let kept: Vec<Value> = files::text_of(&file)
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        let _ = files::remove_file_synced(&file);
        for (i, entry) in kept.into_iter().take(8).enumerate() {
            let Some(url) = entry
                .as_str()
                .or_else(|| entry["url"].as_str())
                .filter(|url| {
                    url.starts_with("http:")
                        || url.starts_with("https:")
                        || url.starts_with("file:")
                })
            else {
                continue;
            };
            let session = entry["session"].as_str();
            let opening = if i == 0 {
                let first = self.tabs.lock().await.active.clone();
                match first {
                    Some(target) => self.use_page(&target, session).await,
                    None => self.create(session).await,
                }
            } else {
                self.create(session).await
            };
            if let Ok(page) = opening {
                let _ = self
                    .pipe
                    .call("Page.navigate", json!({"url":url}), Some(&page.session))
                    .await;
            }
        }
    }
    async fn tidy_tabs(&self, age: Duration) {
        if self.home.join("browser/keeps-agent-tabs").exists() {
            return;
        }
        let tabs = self.tabs.lock().await;
        let eligible = |target: &str, claim: &Claim, tabs: &Tabs| {
            claim.used.elapsed() >= age
                && tabs.active.as_deref() != Some(target)
                && !tabs.seen.contains(target)
                && !tabs.held.contains_key(target)
                && !tabs.working.contains_key(target)
        };
        let candidates: Vec<_> = tabs
            .claims
            .iter()
            .filter(|(target, claim)| eligible(target, claim, &tabs))
            .map(|(id, claim)| (id.clone(), claim.clone()))
            .collect();
        drop(tabs);
        if candidates.is_empty() {
            return;
        }
        let Ok(busy) = self.engine.work_queue_active_sessions().await else {
            return;
        };
        let finder = self.engine.work_engine().await;
        let agent_busy: HashSet<_> = finder
            .agent_sessions()
            .await
            .into_iter()
            .filter(|(_, busy)| *busy)
            .filter_map(|(data, _)| data["id"].as_str().map(str::to_owned))
            .collect();
        for (target, claim) in candidates {
            if busy.contains(&claim.session) || agent_busy.contains(&claim.session) {
                continue;
            }
            let tabs = self.tabs.lock().await;
            let close = tabs.targets.len() > 1
                && tabs
                    .claims
                    .get(&target)
                    .is_some_and(|now| now.used == claim.used && eligible(&target, now, &tabs));
            drop(tabs);
            if close {
                let _ = self.close_page(&target).await;
            }
        }
    }
    async fn stop(&self) {
        let state = self.tabs.lock().await;
        let pages:Vec<_>=state.targets.iter().filter(|t|t.url.starts_with("http:")||t.url.starts_with("https:")||t.url.starts_with("file:")).take(8).map(|t|json!({"url":t.url,"session":state.claims.get(&t.target_id).map(|c|&c.session)})).collect();
        drop(state);
        let _ = files::write_secret(
            &self.home.join("run/pages.json"),
            json!(pages).to_string().as_bytes(),
        );
        self.asks.clear();
        let _ = tokio::time::timeout(
            Duration::from_secs(3),
            self.pipe.call("Browser.close", json!({}), None),
        )
        .await;
        self.pipe.stop_and_wait().await;
        let _ = files::remove_file_synced(&self.home.join("run/browser.pid"));
    }
    fn clear_old_downloads(&self) {
        let Ok(entries) = fs::read_dir(&self.download_folder) else {
            return;
        };
        for entry in entries.flatten() {
            if entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|at| at.elapsed().ok())
                .is_some_and(|age| age > Duration::from_secs(7 * 24 * 3600))
            {
                let _ = files::remove_file_synced(&entry.path());
            }
        }
    }
}
async fn site_icon(pipe: &Pipe, p: &Page, address: &str) -> Option<String> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let call = |method: &'static str, params: Value| {
        pipe.call_for(method, params, Some(&p.session), Duration::from_secs(5))
    };
    let tree = call("Page.getFrameTree", json!({})).await.ok()?;
    let frame = tree["frameTree"]["frame"]["id"].clone();
    let world = call(
        "Page.createIsolatedWorld",
        json!({"frameId":frame,"worldName":"kuvo-icons"}),
    )
    .await
    .ok()?;
    let expression = r#"(()=>{const links=[...document.querySelectorAll('link[rel][href]')].filter(l=>/\b(icon|apple-touch-icon)\b/i.test(l.rel)&&!l.type.includes('svg')&&!/\.svg([?#]|$)/i.test(l.href));const icons=links.filter(l=>/\bicon\b/i.test(l.rel)&&!l.rel.includes('apple-touch'));const score=l=>Math.abs((Math.max(0,...l.sizes.value.split(/\s+/).map(n=>parseInt(n)||0))||32)-64);return (icons.length?icons:links).sort((a,b)=>score(a)-score(b))[0]?.href??new URL('/favicon.ico',location.href).href;})()"#;
    let result=call("Runtime.evaluate",json!({"expression":expression,"contextId":world["executionContextId"],"returnByValue":true})).await.ok()?;
    let url = result["result"]["value"].as_str()?;
    if let Some((_, encoded)) = url
        .strip_prefix("data:")
        .and_then(|url| url.split_once(";base64,"))
    {
        return (encoded.len() <= 183500).then(|| encoded.to_owned());
    }
    let url = url::Url::parse(url)
        .ok()
        .or_else(|| url::Url::parse(address).ok()?.join(url).ok())?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let fetched=call("Network.loadNetworkResource",json!({"frameId":frame,"url":url.as_str(),"options":{"disableCache":false,"includeCredentials":false}})).await.ok()?;
    let handle = fetched["resource"]["stream"].as_str()?;
    let mut bytes = Vec::new();
    let icon = async {
        loop {
            let read = call("IO.read", json!({"handle":handle,"size":65536}))
                .await
                .ok()?;
            let data = read["data"].as_str().unwrap_or_default();
            if read["base64Encoded"] == true {
                bytes.extend(STANDARD.decode(data).ok()?);
            } else {
                bytes.extend_from_slice(data.as_bytes());
            }
            if bytes.len() > 131072 {
                return None;
            }
            if read["eof"] == true {
                return (!bytes.is_empty()).then(|| STANDARD.encode(&bytes));
            }
        }
    }
    .await;
    let _ = call("IO.close", json!({"handle":handle})).await;
    icon
}
fn append_history(folder: &Path, line: Value) {
    use std::io::Write;
    let path = folder.join("visits.jsonl");
    let prior = fs::metadata(&path).map_or(0, |m| m.len());
    if prior > 4 * 1024 * 1024 {
        return;
    }
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        if writeln!(file, "{line}").is_ok()
            && prior <= 256 * 1024
            && file.metadata().is_ok_and(|m| m.len() > 256 * 1024)
        {
            let folder = folder.to_owned();
            tokio::task::spawn_blocking(move || {
                let _ = super::history::fold_journal(&folder);
            });
        }
    }
}

fn download_params(name: &str, session: Option<&str>) -> Value {
    let mut params = json!({"name":name});
    if let Some(session) = session {
        params["session"] = json!(session);
    }
    params
}
fn finish_download(folder: &Path, guid: &str, name: &str) -> Option<PathBuf> {
    if !guid.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') || guid.is_empty() {
        return None;
    }
    let name = Path::new(name)
        .file_name()?
        .to_str()?
        .trim_start_matches('.');
    let name = if name.is_empty() { "download" } else { name };
    let path = Path::new(name);
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    let (stem, extension) = if extension.len() > 20 {
        (name, String::new())
    } else {
        (path.file_stem()?.to_str()?, extension)
    };
    let mut stem = stem.chars().take(200).collect::<String>();
    while stem.len() + extension.len() > 200 {
        stem.pop();
    }
    let safe = format!("{stem}{extension}");
    for n in 1..10000 {
        let target = folder.join(if n == 1 {
            safe.clone()
        } else {
            format!("{stem} {n}{extension}")
        });
        if !target.exists() {
            fs::rename(folder.join(guid), &target).ok()?;
            return Some(target);
        }
    }
    None
}
struct Lease {
    live: Arc<Living>,
    release: mpsc::UnboundedSender<Command>,
    generation: u64,
}
impl Drop for Lease {
    fn drop(&mut self) {
        let _ = self.release.send(Command::Release(self.generation));
    }
}
enum Command {
    Acquire(oneshot::Sender<io::Result<Lease>>),
    Existing(oneshot::Sender<Option<Lease>>),
    Release(u64),
}
#[derive(Clone)]
pub struct Browser {
    commands: mpsc::UnboundedSender<Command>,
    asks: BrowserAsks,
    events: broadcast::Sender<Value>,
}
pub struct BrowserTask {
    browser: Browser,
    stop: CancellationToken,
    task: Option<JoinHandle<()>>,
}
impl BrowserTask {
    pub(super) fn start(home: PathBuf, engine: Engine, installer: Installer) -> io::Result<Self> {
        let config: Config =
            serde_json::from_str(include_str!("../../../../dist/browser-config.json"))
                .map_err(io::Error::other)?;
        let (commands, inbox) = mpsc::unbounded_channel();
        let (events, _) = broadcast::channel(128);
        let asks = BrowserAsks::new(home.join("run"), events.clone());
        let stop = CancellationToken::new();
        let browser = Browser {
            commands,
            asks,
            events,
        };
        let task = tokio::spawn(manager(
            home,
            Arc::new(config),
            engine,
            installer,
            browser.clone(),
            inbox,
            stop.clone(),
        ));
        Ok(Self {
            browser,
            stop,
            task: Some(task),
        })
    }
    pub fn browser(&self) -> Browser {
        self.browser.clone()
    }
    pub async fn stop(mut self) {
        self.stop.cancel();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}
impl Drop for BrowserTask {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
impl Browser {
    pub fn events(&self) -> broadcast::Receiver<Value> {
        self.events.subscribe()
    }
    pub fn asks(&self) -> BrowserAsks {
        self.asks.clone()
    }
    async fn existing(&self) -> Option<Lease> {
        let (reply, answer) = oneshot::channel();
        self.commands.send(Command::Existing(reply)).ok()?;
        answer.await.ok().flatten()
    }
    async fn acquire(&self) -> io::Result<Lease> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(Command::Acquire(reply))
            .map_err(|_| io::Error::other("The browser is closed."))?;
        answer
            .await
            .map_err(|_| io::Error::other("The browser is closed."))?
    }
}
async fn manager(
    home: PathBuf,
    config: Arc<Config>,
    engine: Engine,
    installer: Installer,
    browser: Browser,
    mut inbox: mpsc::UnboundedReceiver<Command>,
    stop: CancellationToken,
) {
    let idle = std::env::var("KUVO_BROWSER_IDLE")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(600));
    let tab_idle = std::env::var("KUVO_BROWSER_TAB_IDLE")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(3600))
        .max(Duration::from_secs(1));
    let tab_interval = (tab_idle / 2).min(Duration::from_secs(300));
    let mut tidy_at = Instant::now() + tab_interval;
    let mut current: Option<Arc<Living>> = None;
    let mut generation = 0u64;
    let mut users = 0usize;
    let mut deadline: Option<Instant> = None;
    loop {
        tokio::select! {
            ()=stop.cancelled()=>break,
            command=inbox.recv()=>match command {
                Some(Command::Acquire(reply))=>{
                    if reply.is_closed(){continue;}
                    if current.as_ref().is_some_and(|live|!live.pipe.running()){if let Some(old)=current.take(){old.stop().await;}}
                    if current.is_none(){
                        let launched=tokio::select!{()=stop.cancelled()=>break,result=Living::launch(home.clone(),config.clone(),engine.clone(),&installer,browser.asks.clone(),browser.events.clone())=>result};
                        match launched{Ok(live)=>{current=Some(live);generation+=1;users=0;tidy_at=Instant::now()+tab_interval;},Err(error)=>{let _=reply.send(Err(error));continue;}}
                    }
                    users+=1;deadline=None;
                    let lease=Lease{live:current.as_ref().expect("launched browser").clone(),release:browser.commands.clone(),generation};let _=reply.send(Ok(lease));
                },
                Some(Command::Existing(reply))=>{
                    let lease=current.as_ref().filter(|live|live.pipe.running()).map(|live|{users+=1;deadline=None;Lease{live:live.clone(),release:browser.commands.clone(),generation}});let _=reply.send(lease);
                },
                Some(Command::Release(of)) if of==generation=>{users=users.saturating_sub(1);if users==0{deadline=Some(Instant::now()+idle);}},
                Some(Command::Release(_))=>{},None=>break,
            },
            ()=tokio::time::sleep_until(tidy_at),if current.is_some()=>{
                if let Some(live)=current.as_ref(){live.tidy_tabs(tab_idle).await;}
                tidy_at=Instant::now()+tab_interval;
            },
            ()=async{match deadline{Some(at)=>tokio::time::sleep_until(at).await,None=>std::future::pending().await}}=>{if let Some(live)=current.take(){live.stop().await;}deadline=None;},
        }
    }
    if let Some(live) = current {
        live.stop().await;
    }
}
