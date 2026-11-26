//! The app's authenticated live browser view. Only the named view commands
//! are accepted here; the DevTools pipe isn't a client interface.
use super::{Browser, Living, Page};
use base64::{Engine, engine::general_purpose::STANDARD};
use kuvo_protocol::routes::get_kuvo_browser_view as route;
use kuvo_server::socket::{Message, Socket};
use kuvo_server::{Access, Refusal, Registry, RouteReply, Transport};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashSet, VecDeque},
    io,
    path::{Path, PathBuf},
    sync::{Arc, Weak},
    time::Duration,
};
use tokio::{
    sync::{Mutex, broadcast, mpsc},
    task::JoinSet,
    time::Instant,
};
use tokio_util::sync::CancellationToken;

const FRAME_POLICY: &str = include_str!("../../../../../src/browser/frame-policy.js");
const MAC_ONLY: &str = "Kuvo on a Mac has no browser of its own yet: its agents browse in Kuvo Code's panel on that Mac.";
fn bad(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}
fn event(method: &str, params: Value) -> Value {
    json!({"method":method,"params":params,"formatVersion":1})
}
fn encode(mut value: Value) -> Message {
    value["formatVersion"] = json!(1);
    Message::Text((value.to_string() + "\n").into())
}
async fn send(socket: &Socket, value: Value) -> io::Result<()> {
    socket
        .send
        .send(encode(value))
        .await
        .map_err(|_| bad("The browser view closed."))
}
fn frame_message(meta: Value, jpeg: &[u8]) -> Message {
    let header = event("Kuvo.frame", meta).to_string();
    let mut bytes = Vec::with_capacity(4 + header.len() + jpeg.len());
    bytes.extend_from_slice(&(header.len() as u32).to_be_bytes());
    bytes.extend_from_slice(header.as_bytes());
    bytes.extend_from_slice(jpeg);
    Message::Binary(bytes.into())
}
fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}
fn id() -> String {
    let mut bytes = [0; 16];
    let _ = getrandom::fill(&mut bytes);
    hex::encode(bytes)
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Size {
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    pub zoom: f64,
}
impl Default for Size {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 800,
            scale: 2.0,
            zoom: 1.0,
        }
    }
}
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(super) struct ViewKey {
    target: String,
    width: u32,
    height: u32,
    scale: u32,
    quality: u32,
}
#[derive(Clone)]
struct View {
    key: ViewKey,
    zoom: f64,
    fps: f64,
    class: Arc<Mutex<Capture>>,
    last: Option<String>,
    inflight: HashSet<u64>,
    last_sent: Option<Instant>,
}
pub(super) struct Capture {
    next: Instant,
    floor: Instant,
    last: Option<Arc<Picture>>,
    dirty: bool,
    policy_id: String,
}
struct Picture {
    jpeg: Vec<u8>,
    digest: String,
    width: u32,
    height: u32,
    x: f64,
    y: f64,
}
fn finite(value: &Value, default: f64, min: f64, max: f64) -> f64 {
    value.as_f64().unwrap_or(default).clamp(min, max)
}
impl Living {
    async fn resize(&self, p: &Page, size: Size, by: &str) -> io::Result<()> {
        let _gate = self.resize_gate.lock().await;
        let previous = self.tabs.lock().await.sizes.get(&p.target).copied();
        if previous == Some(size) {
            return Ok(());
        }
        if by == "viewer" && self.tabs.lock().await.working.contains_key(&p.target) {
            return Ok(());
        }
        self.pipe.call("Emulation.setDeviceMetricsOverride",json!({"width":size.width,"height":size.height,"deviceScaleFactor":size.scale,"mobile":false}),Some(&p.session)).await?;
        self.pipe
            .call(
                "Emulation.setPageScaleFactor",
                json!({"pageScaleFactor":size.zoom}),
                Some(&p.session),
            )
            .await?;
        self.tabs.lock().await.sizes.insert(p.target.clone(), size);
        self.tell("Kuvo.viewport",json!({"targetId":p.target,"width":size.width,"height":size.height,"scale":size.scale,"zoom":size.zoom,"by":by}));
        Ok(())
    }
    pub(super) async fn agent_size(&self, p: &Page) {
        let _ = self.resize(p, Size::default(), "agent").await;
    }
    pub(super) async fn restore_after_idle(self: &Arc<Self>, target: &str) {
        let mut tabs = self.tabs.lock().await;
        if let Some(old) = tabs.restores.remove(target) {
            old.cancel();
        }
        if !tabs
            .viewer_sizes
            .values()
            .any(|(page, _, _)| page == target)
        {
            return;
        }
        let token = self.pipe.stopping().child_token();
        tabs.restores.insert(target.to_owned(), token.clone());
        drop(tabs);
        let weak = Arc::downgrade(self);
        let target = target.to_owned();
        tokio::spawn(async move {
            tokio::select! {
                ()=token.cancelled()=>return,
                ()=tokio::time::sleep(Duration::from_secs(5))=>{},
            }
            let Some(live) = weak.upgrade() else {
                return;
            };
            if token.is_cancelled() {
                return;
            }
            let size = live
                .tabs
                .lock()
                .await
                .viewer_sizes
                .values()
                .filter(|(page, _, _)| page == &target)
                .max_by_key(|(_, _, at)| *at)
                .map(|(_, size, _)| *size);
            if let Some(size) = size
                && let Ok(page) = live.attach(&target).await
            {
                if !token.is_cancelled() {
                    let _ = live.resize(&page, size, "viewer").await;
                    live.dirty(&target).await;
                }
            }
        });
    }
    fn class(&self, key: &ViewKey) -> Arc<Mutex<Capture>> {
        let mut classes = self.captures.lock().unwrap_or_else(|e| e.into_inner());
        classes.retain(|_, class| class.strong_count() > 0);
        if let Some(class) = classes.get(key).and_then(Weak::upgrade) {
            return class;
        }
        let class = Arc::new(Mutex::new(Capture {
            next: Instant::now(),
            floor: Instant::now(),
            last: None,
            dirty: true,
            policy_id: id(),
        }));
        classes.insert(key.clone(), Arc::downgrade(&class));
        class
    }
    pub(super) async fn dirty(&self, target: &str) {
        let classes: Vec<_> = self
            .captures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(key, _)| key.target == target)
            .filter_map(|(_, class)| class.upgrade())
            .collect();
        for class in classes {
            class.lock().await.dirty = true;
        }
        self.changed.notify_waiters();
    }
    async fn capture(&self, view: &View) -> io::Result<(Option<Arc<Picture>>, Instant)> {
        let mut class = view.class.lock().await;
        let due = if class.dirty { class.floor } else { class.next };
        if due > Instant::now() {
            return Ok((class.last.clone(), due));
        }
        let target = &view.key.target;
        let p = self.attach(target).await?;
        let size = self
            .tabs
            .lock()
            .await
            .sizes
            .get(target)
            .copied()
            .unwrap_or_default();
        let scale = (view.key.width as f64 / size.width as f64)
            .min(view.key.height as f64 / size.height as f64)
            * (view.key.scale as f64 / 100.0);
        let scale = scale.min(size.scale);
        let metrics = self
            .pipe
            .call("Page.getLayoutMetrics", json!({}), Some(&p.session))
            .await?;
        let viewport = metrics
            .get("cssVisualViewport")
            .unwrap_or(&metrics["visualViewport"]);
        let x = viewport["pageX"].as_f64().unwrap_or(0.0);
        let y = viewport["pageY"].as_f64().unwrap_or(0.0);
        let result=self.pipe.call("Page.captureScreenshot",json!({"format":"jpeg","quality":view.key.quality,"captureBeyondViewport":false,"clip":{"x":x,"y":y,"width":size.width,"height":size.height,"scale":scale/size.scale}}),Some(&p.session)).await?;
        let data = STANDARD
            .decode(text(&result["data"]))
            .map_err(io::Error::other)?;
        let mut digest = Sha256::new();
        digest.update(&data);
        digest.update(x.to_le_bytes());
        digest.update(y.to_le_bytes());
        let digest = hex::encode(digest.finalize());
        let now = jiff::Timestamp::now().as_millisecond();
        // The shared frame policy itself runs in an isolated Chrome world;
        // ask its monotone due() clock for the next deadline, rather than polling.
        let key = json!(class.policy_id);
        let hash = json!(digest);
        let fps = view.fps;
        let source = FRAME_POLICY.replacen("export function", "function", 1);
        let expression = format!(
            "(function(){{ {source}\nconst all=globalThis.__kuvoPolicies??=Object.create(null);const policy=all[{key}]??=framePolicy();if ({dirty}) policy.dirty();const changed=policy.captured({hash},{now},{fps});let lo={now},hi={now}+2000;while(hi-lo>1){{const mid=Math.floor((lo+hi)/2);if(policy.due(mid))hi=mid;else lo=mid;}}return {{changed,next:hi}};}})()",
            dirty = class.dirty
        );
        let tree = self
            .pipe
            .call("Page.getFrameTree", json!({}), Some(&p.session))
            .await?;
        let frame = tree["frameTree"]["frame"]["id"].clone();
        let context = self
            .pipe
            .call(
                "Page.createIsolatedWorld",
                json!({"frameId":frame,"worldName":"kuvo-view"}),
                Some(&p.session),
            )
            .await?;
        let evaluated=self.pipe.call("Runtime.evaluate",json!({"expression":expression,"returnByValue":true,"contextId":context["executionContextId"]}),Some(&p.session)).await?;
        let deadline = evaluated["result"]["value"]["next"]
            .as_i64()
            .unwrap_or(now + 1000);
        class.floor = Instant::now() + Duration::from_secs_f64(1.0 / view.fps);
        class.next = Instant::now() + Duration::from_millis((deadline - now).clamp(1, 2000) as u64);
        class.dirty = false;
        if class.last.as_ref().is_none_or(|last| last.digest != digest) {
            class.last = Some(Arc::new(Picture {
                jpeg: data,
                digest,
                width: (size.width as f64 * scale).round() as u32,
                height: (size.height as f64 * scale).round() as u32,
                x,
                y,
            }));
        }
        Ok((class.last.clone(), class.next))
    }
}
impl Browser {
    pub fn register_view(&self, routes: &mut Registry, mac: bool) {
        routes.register::<route::Spec, _, _>(Access::Any, move |_| async move {
            if mac {
                return Err(Refusal::new(http::StatusCode::NOT_FOUND, MAC_ONLY));
            }
            Ok(RouteReply::json_as_kept::<426>(
                json!({"message":"Open this route as a WebSocket."}),
            ))
        });
        let browser = self.clone();
        routes.sockets_for("kuvo/browser/view", move |request, cancel| {
            let browser = browser.clone();
            async move {
                if mac {
                    return Err(Refusal::new(http::StatusCode::NOT_FOUND, MAC_ONLY));
                }
                let relay = matches!(request.caller.transport(), Transport::Relay);
                let (client, server) = Socket::pair();
                tokio::spawn(async move {
                    if let Err(error) = serve(browser, server, relay, cancel).await {
                        eprintln!("Browser view closed: {error}");
                    }
                });
                Ok(client)
            }
        });
    }
}
async fn serve(
    browser: Browser,
    mut socket: Socket,
    relay: bool,
    cancel: CancellationToken,
) -> io::Result<()> {
    let lease = match tokio::select! {result=browser.acquire()=>result,()=cancel.cancelled()=>return Ok(())}
    {
        Ok(lease) => lease,
        Err(error) => {
            send(
                &socket,
                event(
                    "Kuvo.error",
                    json!({"kind":"failed","message":error.to_string()}),
                ),
            )
            .await?;
            let _ = socket.send.send(Message::Close(None)).await;
            return Ok(());
        }
    };
    let live = &lease.live;
    live.refresh_targets().await?;
    let mut events = browser.events.subscribe();
    let connection = id();
    let mut tabs = live.tabs.lock().await;
    tabs.viewers += 1;
    let workers = tabs.working.keys().map(|target|json!({"targetId":target,"session":tabs.claims.get(target).map(|c|&c.session)})).collect::<Vec<_>>();
    let status = json!({"working":!workers.is_empty(),"pages":tabs.working.keys().collect::<Vec<_>>(),"workers":workers});
    let hello = event(
        "Kuvo.hello",
        json!({"targetId":tabs.active,"status":status,"version":"dev","keeper":51,"build":"native","idleStop":true,"asks":live.asks.pending(),"viewers":tabs.viewers,"held":tabs.held.keys().collect::<Vec<_>>(),"holdMacs":tabs.hold_macs,"passkeys":{}}),
    );
    let count = tabs.viewers;
    drop(tabs);
    live.tell("Kuvo.viewers", json!({"count":count}));
    let _viewing = Viewer {
        live: live.clone(),
        connection: connection.clone(),
    };
    send(&socket, hello).await?;
    send(&socket, event("Kuvo.pages", live.page_facts().await)).await?;
    let mut view: Option<View> = None;
    let mut next: Option<Instant> = None;
    let mut frame_id = 0u64;
    let mut revision = 0u64;
    let mut queue = VecDeque::<Value>::new();
    let mut actions = JoinSet::new();
    let mut captures = JoinSet::new();
    loop {
        if actions.is_empty()
            && let Some(message) = queue.pop_front()
        {
            let live = live.clone();
            let output = socket.send.clone();
            let connection = connection.clone();
            let mut working_view = view.clone();
            let replaces = message["method"] == "Kuvo.view";
            actions.spawn(async move {
                let result =
                    command(&live, &output, &connection, &message, &mut working_view).await;
                (message, replaces, working_view, result)
            });
        }
        tokio::select! {
            ()=cancel.cancelled()=>break Ok(()),
            ()=live.changed.notified()=>{if view.as_ref().is_some_and(|v|v.inflight.len()<2){next=Some(Instant::now());}},
            message=socket.receive.recv()=>{
                let Some(message)=message else{break Ok(());};
                let Message::Text(text)=message else {break Ok(());};
                for line in text.lines(){
                    let Ok(message)=serde_json::from_str::<Value>(line)else{return Err(bad("Unreadable view command."));};
                    if message.get("formatVersion").is_some_and(|v|v!=0&&v!=1){return Err(bad("Unsupported view format."));}
                    let method=text_of_method(&message);
                    // Credit and an answer to a page's dialog cannot wait behind the call
                    // that is waiting for the displayed picture or that same dialog.
                    let urgent=matches!(method,"Kuvo.frameAck"|"Kuvo.frameDropped"|"Kuvo.dialogAnswer"|"Kuvo.pages")
                        || method=="Kuvo.go"&&message["params"]["stop"]==true;
                    if urgent {
                        let result=command(live,&socket.send,&connection,&message,&mut view).await;
                        reply(&socket,&message,result).await?;
                        next=view.as_ref().filter(|v|v.inflight.len()<2).map(|_|Instant::now());
                    } else {
                        if queue.len()>=32 {return Err(bad("Too many pending view commands."));}
                        queue.push_back(message);
                    }
                }
            },
            completed=actions.join_next(),if !actions.is_empty()=>{
                if let Some(Ok((message,replaces,working_view,result)))=completed {
                    if replaces&&result.is_ok(){
                        revision+=1;
                        captures.abort_all();
                        view=working_view;
                    }
                    reply(&socket,&message,result).await?;
                    next=view.as_ref().filter(|v|v.inflight.len()<2).map(|_|Instant::now());
                }
            },
            said=events.recv()=>match said{Ok(value)=>{
                if value["method"]!="Kuvo.external" || view.as_ref().is_some_and(|v|value["params"]["targetId"]==v.key.target) {send(&socket,value).await?;}
            },Err(broadcast::error::RecvError::Lagged(_))=>send(&socket,event("Kuvo.pages",live.page_facts().await)).await?,Err(_)=>break Ok(())},
            ()=async{match next{Some(at)=>tokio::time::sleep_until(at).await,None=>std::future::pending().await}},if captures.is_empty()=>{
                let Some(viewing)=view.as_ref()else{next=None;continue;};
                if viewing.inflight.len()>=2{next=None;continue;}
                if let Some(last)=viewing.last_sent{let due=last+Duration::from_secs_f64(1.0/viewing.fps);if due>Instant::now(){next=Some(due);continue;}}
                let live=live.clone();
                let viewing=viewing.clone();
                captures.spawn(async move{(revision,live.capture(&viewing).await)});
                next=None;
            },
            captured=captures.join_next(),if !captures.is_empty()=>{
                let Some(viewing)=view.as_mut()else{continue;};
                match captured{
                    Some(Ok((generation,Ok((picture,deadline))))) if generation==revision=>{
                        next=Some(deadline);
                        if viewing.inflight.len()<2 && let Some(picture)=picture&&viewing.last.as_deref()!=Some(&picture.digest){
                            frame_id+=1;let frame=frame_message(json!({"targetId":viewing.key.target,"frameId":frame_id,"width":picture.width,"height":picture.height,"scrollX":picture.x,"scrollY":picture.y}),&picture.jpeg);
                            if relay&&picture.jpeg.len()>59000{if viewing.key.scale>100 {viewing.key.scale=(viewing.key.scale*80/100).max(100);}else{viewing.key.quality=viewing.key.quality.saturating_sub(10).max(20);}viewing.class=live.class(&viewing.key);viewing.last=None;next=Some(Instant::now()+Duration::from_millis(100));}
                            else if socket.send.try_send(frame).is_ok(){viewing.last=Some(picture.digest.clone());viewing.last_sent=Some(Instant::now());viewing.inflight.insert(frame_id);}
                        }
                    },
                    _=>{next=Some(Instant::now()+Duration::from_secs(1));}
                }
            }
        }
    }
}
fn text_of_method(message: &Value) -> &str {
    message["method"].as_str().unwrap_or_default()
}
async fn reply(socket: &Socket, message: &Value, result: io::Result<Value>) -> io::Result<()> {
    if let Some(id) = message.get("id") {
        let answer = match result {
            Ok(value) => json!({"id":id,"result":value}),
            Err(error) => json!({"id":id,"error":{"message":error.to_string()}}),
        };
        send(socket, answer).await?;
    }
    Ok(())
}
struct Viewer {
    live: Arc<Living>,
    connection: String,
}
impl Drop for Viewer {
    fn drop(&mut self) {
        let live = self.live.clone();
        let connection = self.connection.clone();
        tokio::spawn(async move {
            let mut tabs = live.tabs.lock().await;
            tabs.viewers = tabs.viewers.saturating_sub(1);
            let count = tabs.viewers;
            let released_view = tabs
                .viewer_sizes
                .remove(&connection)
                .map(|(target, _, _)| target);
            let held: Vec<_> = tabs
                .held
                .iter()
                .filter(|(_, who)| *who == &connection)
                .map(|(target, _)| target.clone())
                .collect();
            drop(tabs);
            live.tell("Kuvo.viewers", json!({"count":count}));
            if let Some(target) = released_view {
                live.restore_after_idle(&target).await;
            }
            // Retain the user's hold across a brief disconnect, as with the panel waking from sleep.
            if held.is_empty() {
                return;
            }
            let weak = Arc::downgrade(&live);
            let stopping = live.pipe.stopping();
            drop(live);
            tokio::select! {
                ()=stopping.cancelled()=>return,
                ()=tokio::time::sleep(Duration::from_secs(600))=>{},
            }
            let Some(live) = weak.upgrade() else {
                return;
            };
            if !live.pipe.running() {
                return;
            }
            let mut tabs = live.tabs.lock().await;
            for target in held {
                if tabs.held.get(&target) == Some(&connection) {
                    tabs.held.remove(&target);
                    tabs.hold_macs.remove(&target);
                }
            }
            let held = tabs.held.keys().cloned().collect::<Vec<_>>();
            drop(tabs);
            live.tell("Kuvo.held", json!({"pages":held}));
            live.changed.notify_waiters();
            live.publish().await;
        });
    }
}
fn host_port(input: &str) -> bool {
    let head = input.split(['/', '?', '#']).next().unwrap_or_default();
    head.rsplit_once(':').is_some_and(|(host, port)| {
        !host.is_empty()
            && !port.is_empty()
            && port.bytes().all(|b| b.is_ascii_digit())
            && host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b))
    })
}
async fn command(
    live: &Arc<Living>,
    socket: &mpsc::Sender<Message>,
    connection: &str,
    message: &Value,
    view: &mut Option<View>,
) -> io::Result<Value> {
    let name = text(&message["method"]);
    let params = &message["params"];
    let target = params["targetId"]
        .as_str()
        .or_else(|| view.as_ref().map(|v| v.key.target.as_str()));
    match name {
        "Kuvo.pages" => return Ok(live.page_facts().await),
        "Kuvo.frameAck" | "Kuvo.frameDropped" => {
            if let Some(view) = view.as_mut()
                && let Some(id) = params["frameId"].as_u64()
            {
                view.inflight.remove(&id);
                if name == "Kuvo.frameDropped" {
                    if text(&params["reason"]) == "oversize" {
                        if view.key.scale > 100 {
                            view.key.scale = (view.key.scale * 80 / 100).max(100);
                        } else {
                            view.key.quality = view.key.quality.saturating_sub(10).max(20);
                        }
                        view.class = live.class(&view.key);
                    }
                    view.last = None;
                }
            }
            return Ok(json!({}));
        }
        "Kuvo.view" => {
            let Some(target) = params["targetId"].as_str() else {
                *view = None;
                let old = live.tabs.lock().await.viewer_sizes.remove(connection);
                if let Some((target, _, _)) = old {
                    live.restore_after_idle(&target).await;
                }
                return Ok(json!({}));
            };
            let p = live.use_page(target, None).await?;
            live.tabs.lock().await.seen.insert(target.to_owned());
            let width = finite(&params["width"], 1280., 100., 3840.).round() as u32;
            let height = finite(&params["height"], 800., 100., 2160.).round() as u32;
            let mut scale = finite(&params["scale"], 1., 1., 2.);
            let mut quality = finite(&params["quality"], 75., 20., 90.).round() as u32;
            if width <= 600 {
                scale = 1.;
                quality = quality.min(50);
            }
            let zoom = finite(&params["zoom"], 1., 0.25, 5.);
            let size = Size {
                width,
                height,
                scale,
                zoom,
            };
            let old = live
                .tabs
                .lock()
                .await
                .viewer_sizes
                .insert(connection.into(), (target.into(), size, Instant::now()));
            if let Some((old, _, _)) = old
                && old != target
            {
                live.restore_after_idle(&old).await;
            }
            live.resize(&p, size, "viewer").await?;
            let key = ViewKey {
                target: target.into(),
                width,
                height,
                scale: (scale * 100.).round() as u32,
                quality,
            };
            let class = live.class(&key);
            *view = Some(View {
                key,
                zoom,
                fps: finite(&params["maxFps"], 15., 1., 15.),
                class,
                last: None,
                inflight: HashSet::new(),
                last_sent: None,
            });
            return Ok(json!({}));
        }
        _ => {}
    }
    let target = target
        .ok_or_else(|| bad("That page is closed."))?
        .to_owned();
    if !live
        .tabs
        .lock()
        .await
        .targets
        .iter()
        .any(|t| t.target_id == target)
    {
        return Err(bad("That page is closed."));
    }
    let p = live.attach(&target).await?;
    match name {
        "Kuvo.hold" => {
            let mut tabs = live.tabs.lock().await;
            if params["on"] == true {
                tabs.held.insert(target.clone(), connection.into());
                if let Some(mac) = params["mac"].as_str() {
                    tabs.hold_macs.insert(target.clone(), mac.into());
                } else {
                    tabs.hold_macs.remove(&target);
                }
            } else if tabs.held.get(&target).is_some_and(|who| who == connection)
                || params["mac"].as_str().is_some_and(|mac| {
                    tabs.hold_macs
                        .get(&target)
                        .is_some_and(|known| known == mac)
                })
            {
                tabs.held.remove(&target);
                tabs.hold_macs.remove(&target);
            }
            let held = tabs.held.keys().cloned().collect::<Vec<_>>();
            drop(tabs);
            live.tell("Kuvo.held", json!({"pages":held}));
            live.changed.notify_waiters();
            live.publish().await;
        }
        "Kuvo.claim" => {
            let Some(session) = params["session"].as_str() else {
                return Ok(json!({"ok":false}));
            };
            let mut tabs = live.tabs.lock().await;
            let ok = tabs
                .claims
                .get(&target)
                .is_none_or(|c| c.session == session || c.used.elapsed() > Duration::from_secs(90));
            if ok {
                tabs.claims.insert(
                    target.clone(),
                    super::Claim {
                        session: session.into(),
                        used: Instant::now(),
                    },
                );
            }
            drop(tabs);
            live.publish().await;
            return Ok(json!({"ok":ok}));
        }
        "Kuvo.dialogAnswer" => {
            if !params["accept"].is_boolean() {
                return Err(bad("Give a dialog answer and optional prompt text."));
            }
            let mut answer = json!({"accept":params["accept"]});
            if let Some(prompt) = params["promptText"].as_str() {
                answer["promptText"] = json!(prompt);
            }
            live.pipe
                .call("Page.handleJavaScriptDialog", answer, Some(&p.session))
                .await?;
        }
        "Kuvo.go" => {
            if let Some(input) = params["input"].as_str() {
                let input = input.trim();
                if ["javascript:", "data:", "blob:"]
                    .iter()
                    .any(|prefix| input.to_lowercase().starts_with(prefix))
                {
                    return Err(bad("That address cannot be opened here."));
                }
                if !host_port(input)
                    && let Ok(url) = url::Url::parse(input)
                    && !matches!(url.scheme(), "http" | "https" | "file" | "about")
                {
                    socket
                        .send(encode(event(
                            "Kuvo.external",
                            json!({"targetId":target,"url":input}),
                        )))
                        .await
                        .map_err(|_| bad("The browser view closed."))?;
                } else {
                    live.go(&p, input).await?;
                }
            } else if params["back"] == true || params["forward"] == true {
                let history = live
                    .pipe
                    .call("Page.getNavigationHistory", json!({}), Some(&p.session))
                    .await?;
                let at = history["currentIndex"].as_i64().unwrap_or(0)
                    + if params["back"] == true { -1 } else { 1 };
                if let Some(entry) = usize::try_from(at)
                    .ok()
                    .and_then(|at| history["entries"].as_array()?.get(at))
                {
                    live.pipe
                        .call(
                            "Page.navigateToHistoryEntry",
                            json!({"entryId":entry["id"]}),
                            Some(&p.session),
                        )
                        .await?;
                }
            } else {
                live.pipe
                    .call(
                        if params["stop"] == true {
                            "Page.stopLoading"
                        } else {
                            "Page.reload"
                        },
                        json!({}),
                        Some(&p.session),
                    )
                    .await?;
            }
            live.navigation_state(&p).await;
        }
        "Kuvo.input" => {
            if let Some(view) = view.as_ref().filter(|v| v.key.target == target) {
                if let Some((_, _, at)) = live.tabs.lock().await.viewer_sizes.get_mut(connection) {
                    *at = Instant::now();
                }
                let _ = live
                    .resize(
                        &p,
                        Size {
                            width: view.key.width,
                            height: view.key.height,
                            scale: view.key.scale as f64 / 100.0,
                            zoom: view.zoom,
                        },
                        "viewer",
                    )
                    .await;
            }
            let kind = text(&params["kind"]);
            let method = match kind {
                "mouse" | "wheel" => "Input.dispatchMouseEvent",
                "key" => "Input.dispatchKeyEvent",
                "text" => "Input.insertText",
                "compose" => "Input.imeSetComposition",
                _ => return Err(bad("Unknown input kind.")),
            };
            let mut args = params.as_object().cloned().unwrap_or_default();
            args.remove("kind");
            args.remove("targetId");
            if kind == "wheel" {
                args.insert("type".into(), json!("mouseWheel"));
            }
            if let Some(modifiers) = args.get("modifiers").and_then(Value::as_array) {
                let bits = modifiers.iter().fold(0, |bits, key| {
                    bits | match text(key) {
                        "alt" => 1,
                        "control" => 2,
                        "meta" => 4,
                        "shift" => 8,
                        _ => 0,
                    }
                });
                args.insert("modifiers".into(), json!(bits));
            }
            live.pipe
                .call(method, Value::Object(args), Some(&p.session))
                .await?;
        }
        "Kuvo.upload" => {
            let Some(files) = params["files"].as_array() else {
                return Err(bad("No file chooser."));
            };
            let node = live
                .tabs
                .lock()
                .await
                .choosers
                .get(&target)
                .copied()
                .ok_or_else(|| bad("No file chooser."))?;
            let directory = tempfile::Builder::new()
                .prefix("upload-")
                .tempdir_in(live.home.join("browser/tmp"))?;
            let mut paths = Vec::<PathBuf>::new();
            let mut count = 0usize;
            for file in files {
                let name = Path::new(text(&file["name"]))
                    .file_name()
                    .and_then(|n| n.to_str())
                    .filter(|n| !matches!(*n, "." | ".."))
                    .ok_or_else(|| bad("Files need a name and base64 text."))?;
                let data = text(&file["data"]);
                if data.len() > 70 * 1024 * 1024 {
                    return Err(bad("Files exceed 50 MB."));
                }
                let data = STANDARD.decode(data).map_err(io::Error::other)?;
                count += data.len();
                if count > 50 * 1024 * 1024 {
                    return Err(bad("Files exceed 50 MB."));
                }
                let path = directory.path().join(name);
                kuvo_store::files::write_secret(&path, &data)?;
                paths.push(path);
            }
            live.pipe
                .call(
                    "DOM.setFileInputFiles",
                    json!({"files":paths,"backendNodeId":node}),
                    Some(&p.session),
                )
                .await?;
            let mut tabs = live.tabs.lock().await;
            tabs.choosers.remove(&target);
            tabs.uploads
                .entry(target.clone())
                .or_default()
                .push(directory);
        }
        _ => return Err(bad("Unknown view command.")),
    }
    live.dirty(&target).await;
    Ok(json!({}))
}
