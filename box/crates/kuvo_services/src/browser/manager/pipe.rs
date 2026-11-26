//! DevTools' NUL-delimited pipe. The one reader owns response routing and the
//! browser process. No TCP debugging port, and no task while Chrome is stopped.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Event {
    pub method: String,
    #[serde(default)]
    pub params: Value,
    pub session_id: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Request {
    id: u64,
    method: String,
    params: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
}
struct Call {
    request: Request,
    reply: oneshot::Sender<io::Result<Value>>,
}
enum Command {
    Call(Call),
    Forget(u64),
}
#[derive(Clone)]
pub(super) struct Pipe {
    next: Arc<AtomicU64>,
    send: mpsc::Sender<Command>,
    events: broadcast::Sender<Event>,
    stop: CancellationToken,
    pid: u32,
    finished: watch::Receiver<bool>,
}
impl Pipe {
    pub fn pid(&self) -> u32 {
        self.pid
    }
    pub fn events(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }
    pub fn running(&self) -> bool {
        !self.stop.is_cancelled() && !self.send.is_closed()
    }
    pub fn stopping(&self) -> CancellationToken {
        self.stop.clone()
    }
    pub fn stop(&self) {
        self.stop.cancel();
    }
    pub async fn stop_and_wait(&self) {
        self.stop();
        let mut finished = self.finished.clone();
        while !*finished.borrow() {
            if finished.changed().await.is_err() {
                break;
            }
        }
    }
    pub async fn call(
        &self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> io::Result<Value> {
        self.call_for(method, params, session, Duration::from_secs(30))
            .await
    }
    pub async fn call_for(
        &self,
        method: &str,
        params: Value,
        session: Option<&str>,
        timeout: Duration,
    ) -> io::Result<Value> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (reply, answer) = oneshot::channel();
        let request = Request {
            id,
            method: method.into(),
            params,
            session_id: session.map(str::to_owned),
        };
        let gone = || io::Error::other("The browser closed.");
        self.send
            .send(Command::Call(Call { request, reply }))
            .await
            .map_err(|_| gone())?;
        let guard = Pending {
            send: self.send.clone(),
            id,
        };
        let result = tokio::time::timeout(timeout, answer)
            .await
            .map_err(|_| io::Error::other(format!("{method} did not answer in time.")))?
            .map_err(|_| gone())?;
        drop(guard);
        result
    }
    #[cfg(unix)]
    pub async fn launch(home: &Path, build: &Path, sandbox: bool) -> io::Result<Self> {
        platform::launch(home, build, sandbox).await
    }
    #[cfg(not(unix))]
    pub async fn launch(_home: &Path, _build: &Path, _sandbox: bool) -> io::Result<Self> {
        Err(io::Error::other(
            "The browser runs on Linux and macOS; this machine is win32.",
        ))
    }
}
struct Pending {
    send: mpsc::Sender<Command>,
    id: u64,
}
impl Drop for Pending {
    fn drop(&mut self) {
        let _ = self.send.try_send(Command::Forget(self.id));
    }
}

#[cfg(unix)]
mod platform {
    use super::*;
    use kuvo_os::process::{ChildGroup, Spawn, Stdio};
    use kuvo_store::files;
    use std::{fs, path::PathBuf};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    const MAX_FRAME: usize = 80 << 20;
    pub async fn launch(home: &Path, build: &Path, sandbox: bool) -> io::Result<Pipe> {
        let browser = home.join("browser");
        let profile = browser.join("profile");
        let own_home = browser.join("home");
        let temporary = browser.join("tmp");
        for folder in [&profile, &own_home, &temporary] {
            files::create_private_dirs(folder)?;
        }
        let binary = ["chrome", "chrome-headless-shell"]
            .into_iter()
            .map(|name| build.join(name))
            .find(|file| file.is_file())
            .ok_or_else(|| io::Error::other("The browser on this machine isn't whole."))?;
        let mut spawn = Spawn::new(&binary, std::env::vars_os().collect());
        spawn.cwd = Some(own_home.clone());
        let mut args = vec![
            "--headless".into(),
            "--enable-unsafe-swiftshader".into(),
            "--password-store=basic".into(),
            "--disable-background-timer-throttling".into(),
            "--disable-renderer-backgrounding".into(),
            "--disable-backgrounding-occluded-windows".into(),
            "--remote-debugging-pipe".into(),
            format!("--user-data-dir={}", profile.display()).into(),
            "--window-size=1280,800".into(),
            "--force-device-scale-factor=2".into(),
            "--no-first-run".into(),
            "--no-default-browser-check".into(),
            "--disable-dev-shm-usage".into(),
            "--hide-scrollbars".into(),
            "--mute-audio".into(),
            "--disable-smooth-scrolling".into(),
        ];
        if cfg!(target_os = "macos") {
            args.extend([
                "--use-mock-keychain".into(),
                "--disable-features=MediaRouter,DialMediaRouteProvider".into(),
            ]);
        }
        if !sandbox {
            args.push("--no-sandbox".into());
        }
        args.push("about:blank".into());
        spawn.args = args;
        for name in [
            "HOME",
            "CFFIXED_USER_HOME",
            "XDG_CONFIG_HOME",
            "XDG_CACHE_HOME",
            "XDG_DATA_HOME",
            "XDG_STATE_HOME",
        ] {
            let path = match name {
                "XDG_CONFIG_HOME" => own_home.join(".config"),
                "XDG_CACHE_HOME" => own_home.join(".cache"),
                "XDG_DATA_HOME" => own_home.join(".local/share"),
                "XDG_STATE_HOME" => own_home.join(".local/state"),
                _ => own_home.clone(),
            };
            spawn.env.insert(name.into(), path.into_os_string());
        }
        if temporary.as_os_str().len() + "/org.chromium.Chromium.XXXXXX/SingletonSocket".len()
            <= if cfg!(target_os = "macos") { 103 } else { 107 }
        {
            spawn
                .env
                .insert("TMPDIR".into(), temporary.into_os_string());
        }
        let lib = build.join("lib");
        let lib_env = std::env::join_paths(
            std::iter::once(lib.clone()).chain(
                std::env::var_os("LD_LIBRARY_PATH")
                    .iter()
                    .flat_map(std::env::split_paths),
            ),
        )
        .map_err(io::Error::other)?;
        spawn.env.insert("LD_LIBRARY_PATH".into(), lib_env);
        spawn.env.insert(
            "GIO_MODULE_DIR".into(),
            lib.join("gio-modules").into_os_string(),
        );
        spawn
            .env
            .insert("GSETTINGS_BACKEND".into(), "memory".into());
        let font_config = own_home.join("fonts.conf");
        let xml = |p: PathBuf| {
            p.to_string_lossy()
                .replace('&', "&amp;")
                .replace('<', "&lt;")
        };
        files::write_secret(&font_config,format!("<?xml version=\"1.0\"?><!DOCTYPE fontconfig SYSTEM \"urn:fontconfig:fonts.dtd\"><fontconfig><dir>{}</dir><include ignore_missing=\"yes\">/etc/fonts/fonts.conf</include><cachedir>{}</cachedir></fontconfig>",xml(build.join("fonts")),xml(own_home.join(".cache/fontconfig"))).as_bytes())?;
        spawn
            .env
            .insert("FONTCONFIG_FILE".into(), font_config.into_os_string());
        spawn.env.remove(std::ffi::OsStr::new("BUN_BE_BUN"));
        files::write_secret(&home.join("run/browser.log"), b"")?;
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(home.join("run/browser.log"))?;
        spawn.stdout = Stdio::File(log.try_clone()?);
        spawn.stderr = Stdio::File(log);
        let (child, input, output) = spawn.spawn_debug_pipe()?;
        let pid = child.identity().pid;
        let (send, commands) = mpsc::channel(32);
        let (events, _) = broadcast::channel(512);
        let stop = CancellationToken::new();
        let (finished, done) = watch::channel(false);
        let pipe = Pipe {
            next: Arc::new(AtomicU64::new(1)),
            send,
            events: events.clone(),
            stop: stop.clone(),
            pid,
            finished: done,
        };
        tokio::spawn(run(child, input, output, commands, events, stop, finished));
        if let Err(error) = pipe
            .call_for(
                "Browser.getVersion",
                json!({}),
                None,
                Duration::from_secs(30),
            )
            .await
        {
            pipe.stop_and_wait().await;
            return Err(error);
        }
        Ok(pipe)
    }
    async fn run(
        mut child: ChildGroup,
        mut input: tokio::net::unix::pipe::Sender,
        output: tokio::net::unix::pipe::Receiver,
        mut commands: mpsc::Receiver<Command>,
        events: broadcast::Sender<Event>,
        stop: CancellationToken,
        finished: watch::Sender<bool>,
    ) {
        let mut reader = BufReader::new(output);
        let mut pending = HashMap::<u64, oneshot::Sender<io::Result<Value>>>::new();
        let mut bytes = Vec::new();
        let exited = child.identity().watch_exit();
        let mut exited = exited.ok();
        loop {
            tokio::select! {
                ()=stop.cancelled()=>break,
                _=async{if let Some(exit)=&mut exited{let _=exit.await;}else{std::future::pending().await}}=>break,
                command=commands.recv()=>match command {
                    Some(Command::Call(Call{request,reply}))=>{
                        if reply.is_closed(){continue;}
                        let id=request.id;
                        let Ok(mut bytes)=serde_json::to_vec(&request) else{let _=reply.send(Err(io::Error::other("Couldn't encode the browser call.")));continue;};
                        bytes.push(0);
                        let sent=tokio::select!{sent=input.write_all(&bytes)=>sent,()=stop.cancelled()=>break};
                        if sent.is_err(){break;}
                        pending.insert(id,reply);
                    },
                    Some(Command::Forget(id))=>{pending.remove(&id);},
                    None=>break,
                },
                read=reader.read_until(0,&mut bytes)=>{
                    let Ok(count)=read else{break;};
                    if count==0||bytes.len()>MAX_FRAME{break;}
                    if bytes.last()!=Some(&0){continue;}
                    bytes.pop();
                    if let Ok(message)=serde_json::from_slice::<Value>(&bytes){
                        if let Some(id)=message["id"].as_u64(){
                            if let Some(reply)=pending.remove(&id){
                                let result=if let Some(error)=message.get("error"){Err(io::Error::other(error["message"].as_str().unwrap_or("The browser refused that call.")))}else{Ok(message.get("result").cloned().unwrap_or_else(||json!({})))};
                                let _=reply.send(result);
                            }
                        }else if let Ok(event)=serde_json::from_value(message){let _=events.send(event);}
                    }
                    bytes.clear();
                }
            }
        }
        stop.cancel();
        drop(pending);
        drop(input);
        let _ =
            tokio::task::spawn_blocking(move || child.stop(Duration::from_secs(2), |_| {})).await;
        finished.send_replace(true);
    }
}
