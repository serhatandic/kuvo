//! The browser tools use the shared scripts in an isolated DevTools world.
//! The model names numbered elements, never arbitrary script or DevTools calls.
use super::{
    Browser, Living, Page, Target,
    access::{self, Access},
};
use crate::browser::tools::{ToolServer, text};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::OwnedSemaphorePermit,
    time::{Instant, sleep},
};
use tokio_util::sync::CancellationToken;
use url::Url;

fn bad(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}
fn words(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}
fn integer(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_f64().filter(|n| n.is_finite()).map(|n| n as i64))
        .or_else(|| value.as_str()?.parse().ok())
}
fn empty(url: &str) -> bool {
    url.is_empty() || url == "about:blank"
}
pub(super) fn host_of(address: &str) -> Option<String> {
    let url = Url::parse(address).ok()?;
    if url.scheme() == "file" {
        return Some("a file on this machine".into());
    }
    let host = url.host_str()?;
    Some(if url.scheme() == "http" {
        format!("http://{host}")
    } else {
        host.into()
    })
}
fn site_of(host: &str) -> &str {
    if host == "a file on this machine" {
        "a file on this Mac"
    } else {
        host
    }
}
fn fresh() -> String {
    let mut bytes = [0; 16];
    let _ = getrandom::fill(&mut bytes);
    hex::encode(bytes)
}
fn trimmed(text: &str, limit: usize, tail: &str) -> String {
    let mut chars = text.chars();
    let mut out = chars.by_ref().take(limit).collect::<String>();
    if chars.next().is_some() {
        out.push_str(tail);
    }
    out
}
fn home() -> PathBuf {
    kuvo_os::platform::Account::current()
        .and_then(|a| {
            a.home_folder(
                kuvo_os::platform::Platform::current(),
                &std::env::vars_os().collect(),
            )
        })
        .unwrap_or_default()
}
fn expand(input: &str) -> PathBuf {
    if let Some(path) = input.strip_prefix("~/") {
        home().join(path)
    } else {
        PathBuf::from(input)
    }
}
fn outside_note(file: &str) -> String {
    format!(
        "Not opened: {file} is outside this conversation's folder, and this session asks before reading files elsewhere. Ask the user first; if they agree, copy it into the folder (a command, which asks them) and open it there."
    )
}
impl Living {
    pub(super) fn destination(&self, input: &str) -> Option<String> {
        let input = input.trim();
        if input.is_empty() {
            return None;
        }
        if let Ok(url) = Url::parse(input)
            && matches!(url.scheme(), "http" | "https" | "file" | "about")
        {
            return Some(url.to_string());
        }
        if input.starts_with('/') || input.starts_with("~/") {
            return Url::from_file_path(expand(input))
                .ok()
                .map(|url| url.to_string());
        }
        if !input.contains(' ')
            && (input.contains('.') || input.starts_with("localhost") || input.contains(':'))
        {
            return Some(format!(
                "{}://{input}",
                if input.starts_with("localhost") || input.starts_with("127.") {
                    "http"
                } else {
                    "https"
                }
            ));
        }
        let settings: Value = kuvo_store::files::text_of(&self.home.join("browser/settings.json"))
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let chosen = self
            .config
            .search_engines
            .iter()
            .find(|e| settings["searchEngine"] == e.id)
            .map(|e| e.url.clone())
            .or_else(|| std::env::var("KUVO_SEARCH").ok())
            .unwrap_or_else(|| "https://www.google.com/search?q=%s".into());
        let encoded =
            percent_encoding::utf8_percent_encode(input, percent_encoding::NON_ALPHANUMERIC)
                .to_string();
        Some(chosen.replace("%s", &encoded))
    }
    pub(super) async fn address(&self, p: &Page) -> String {
        self.pipe
            .call("Target.getTargetInfo", json!({"targetId":p.target}), None)
            .await
            .ok()
            .and_then(|v| v["targetInfo"]["url"].as_str().map(str::to_owned))
            .unwrap_or_default()
    }
    pub(super) async fn evaluate(
        &self,
        p: &Page,
        expr: &str,
        world: bool,
        by_value: bool,
    ) -> io::Result<Value> {
        let mut params = json!({"expression":expr,"awaitPromise":true,"returnByValue":by_value,"userGesture":true});
        if world {
            let tree = self
                .pipe
                .call("Page.getFrameTree", json!({}), Some(&p.session))
                .await?;
            let frame = tree["frameTree"]["frame"]["id"]
                .as_str()
                .ok_or_else(|| bad("The page has no document."))?;
            let context = self
                .pipe
                .call(
                    "Page.createIsolatedWorld",
                    json!({"frameId":frame,"worldName":"kuvo-agent"}),
                    Some(&p.session),
                )
                .await?;
            params["contextId"] = context["executionContextId"].clone();
        }
        let result = self
            .pipe
            .call("Runtime.evaluate", params, Some(&p.session))
            .await?;
        if let Some(error) = result.get("exceptionDetails") {
            return Err(bad(error["exception"]["description"]
                .as_str()
                .or_else(|| error["text"].as_str())
                .unwrap_or("The page refused that.")));
        }
        Ok(if by_value {
            result["result"]["value"].clone()
        } else {
            result["result"].clone()
        })
    }
    pub(super) async fn settle(&self, p: &Page, navigated: bool) {
        if navigated {
            sleep(Duration::from_millis(250)).await;
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut readable = None;
        while Instant::now() < deadline {
            let state = self
                .evaluate(p, "document.readyState", false, true)
                .await
                .unwrap_or_default();
            if state == "complete" {
                break;
            }
            if state == "interactive" {
                if readable.get_or_insert_with(Instant::now).elapsed() > Duration::from_secs(2) {
                    break;
                }
            } else {
                readable = None;
            }
            sleep(Duration::from_millis(150)).await;
        }
        sleep(Duration::from_millis(if navigated { 500 } else { 350 })).await;
        self.navigation_state(p).await;
    }
    pub(super) async fn go(&self, p: &Page, input: &str) -> io::Result<Value> {
        let target = self
            .destination(input)
            .ok_or_else(|| bad(format!("Not an address or search: {input}")))?;
        self.pipe
            .call("Page.navigate", json!({"url":target}), Some(&p.session))
            .await
    }
}
struct Guard {
    live: Arc<Living>,
    session: String,
    enabled: bool,
}
impl Guard {
    fn new(live: Arc<Living>, p: &Page, rules: Access) -> Self {
        let enabled = !rules.anywhere();
        if enabled {
            live.file_rules
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(p.session.clone(), rules);
        }
        Self {
            live,
            session: p.session.clone(),
            enabled,
        }
    }
    async fn enable(&self) -> io::Result<()> {
        if self.enabled {
            self.live
                .pipe
                .call(
                    "Fetch.enable",
                    json!({"patterns":[{"urlPattern":"file://*"}]}),
                    Some(&self.session),
                )
                .await?;
        }
        Ok(())
    }
    async fn close(&mut self) {
        if self.enabled {
            let _ = self
                .live
                .pipe
                .call("Fetch.disable", json!({}), Some(&self.session))
                .await;
            self.live
                .file_rules
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&self.session);
            self.enabled = false;
        }
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        if self.enabled {
            let live = self.live.clone();
            let session = self.session.clone();
            tokio::spawn(async move {
                let _ = live
                    .pipe
                    .call("Fetch.disable", json!({}), Some(&session))
                    .await;
                live.file_rules
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&session);
            });
        }
    }
}
struct Context<'a> {
    live: &'a Arc<Living>,
    p: Page,
    caller: Option<&'a str>,
    rights: Access,
    permit: Option<OwnedSemaphorePermit>,
    guard: Option<Guard>,
    accept_dialog: bool,
    cancel: &'a CancellationToken,
}
impl Drop for Context<'_> {
    fn drop(&mut self) {
        if let Some(mut guard) = self.guard.take() {
            let permit = self.permit.take();
            let live = self.live.clone();
            let target = self.p.target.clone();
            tokio::spawn(async move {
                guard.close().await;
                live.tabs.lock().await.working.remove(&target);
                live.tell(
                    "Kuvo.status",
                    json!({"working":false,"pages":[],"workers":[]}),
                );
                live.publish().await;
                live.restore_after_idle(&target).await;
                drop(permit);
            });
        }
    }
}
impl<'a> Context<'a> {
    async fn start(
        live: &'a Arc<Living>,
        p: Page,
        caller: Option<&'a str>,
        rights: Access,
        permit: OwnedSemaphorePermit,
        accept_dialog: bool,
        cancel: &'a CancellationToken,
    ) -> io::Result<Self> {
        let mut ctx = Self {
            live,
            p,
            caller,
            rights,
            permit: Some(permit),
            guard: None,
            accept_dialog,
            cancel,
        };
        ctx.engage().await?;
        Ok(ctx)
    }
    async fn engage(&mut self) -> io::Result<()> {
        self.guard = Some(Guard::new(self.live.clone(), &self.p, self.rights.clone()));
        self.guard.as_ref().expect("file guard").enable().await?;
        let mut tabs = self.live.tabs.lock().await;
        if let Some(restore) = tabs.restores.remove(&self.p.target) {
            restore.cancel();
        }
        tabs.working
            .insert(self.p.target.clone(), self.accept_dialog);
        drop(tabs);
        self.live.agent_size(&self.p).await;
        self.live.tell("Kuvo.status",json!({"working":true,"pages":[self.p.target],"workers":[{"targetId":self.p.target,"session":self.caller}]}));
        self.live.publish().await;
        Ok(())
    }
    async fn disengage(&mut self) {
        if let Some(guard) = self.guard.as_mut() {
            guard.close().await;
        }
        self.guard.take();
        self.live.tabs.lock().await.working.remove(&self.p.target);
        self.live.tell(
            "Kuvo.status",
            json!({"working":false,"pages":[],"workers":[]}),
        );
        self.live.publish().await;
        self.live.dirty(&self.p.target).await;
        self.live.restore_after_idle(&self.p.target).await;
    }
    async fn pause(&mut self) {
        self.disengage().await;
        self.permit.take();
    }
    async fn resume(&mut self) -> io::Result<()> {
        self.permit = Some(
            self.live
                .turn
                .clone()
                .acquire_owned()
                .await
                .map_err(io::Error::other)?,
        );
        self.live.refresh_targets().await?;
        let tabs = self.live.tabs.lock().await;
        if !tabs.targets.iter().any(|t| t.target_id == self.p.target) {
            return Err(bad("The page was closed while the user answered."));
        }
        if tabs.held.contains_key(&self.p.target) {
            return Err(bad("The user took the page over and still has it."));
        }
        drop(tabs);
        self.p = self.live.use_page(&self.p.target, self.caller).await?;
        self.engage().await
    }
    async fn script_args(&self, mut args: Value) -> Value {
        if let Some(caller) = self.caller {
            let id = self
                .live
                .tabs
                .lock()
                .await
                .snapshots
                .get(caller)
                .cloned()
                .unwrap_or_else(|| "none".into());
            args["kuvoSnapshot"] = json!(id);
        }
        args
    }
    async fn script_value(&self, body: &str, args: Value, value: bool) -> io::Result<Value> {
        let args = self.script_args(args).await;
        let names = args
            .as_object()
            .map(|o| o.keys().map(String::as_str).collect::<Vec<_>>().join(","))
            .unwrap_or_default();
        self.live
            .evaluate(
                &self.p,
                &format!("(async function({{{names}}}) {{\n{body}\n}})({args})"),
                true,
                value,
            )
            .await
    }
    async fn run(&self, body: &str, args: Value) -> io::Result<String> {
        let value = self.script_value(body, args, true).await?;
        Ok(value.as_str().unwrap_or("ok").into())
    }
    async fn action(&self, name: &str, args: Value) -> io::Result<String> {
        let body = format!(
            "{}\n{}",
            self.live.config.script("find"),
            self.live.config.script(name)
        );
        self.run(&body, args).await
    }
    async fn snapshot(&self) -> String {
        let address = self.live.address(&self.p).await;
        if empty(&address) {
            return "The browser is empty. Use browser_navigate to open a page.".into();
        }
        if let Some(file) = self.rights.outside(&address) {
            return outside_note(&file);
        }
        let handlers = self
            .live
            .evaluate(
                &self.p,
                &format!(
                    "(function(){{\n{}\n}})()",
                    self.live.config.script("handlers")
                ),
                false,
                true,
            )
            .await
            .unwrap_or_default();
        let id = fresh();
        let args = json!({"kuvoSnapshot":id,"kuvoHandlers":if handlers.is_array(){handlers}else{json!([])}});
        let expression = format!(
            "(async function({{kuvoSnapshot,kuvoHandlers}}){{\n{}\n}})({args})",
            self.live.config.script("snapshot")
        );
        let Ok(result) = self.live.evaluate(&self.p, &expression, true, true).await else {
            return format!("Couldn't read the page at {address}.");
        };
        if let Some(caller) = self.caller {
            self.live
                .tabs
                .lock()
                .await
                .snapshots
                .insert(caller.into(), id);
        }
        let below = result["below"].as_i64().unwrap_or(0);
        let above = result["above"].as_i64().unwrap_or(0);
        let body = words(&result["text"]);
        let body = if body.is_empty() {
            "(no visible content)".into()
        } else {
            trimmed(
                body,
                18_000,
                "\n… (cut; scroll or use browser_text for more)",
            )
        };
        let passkey = self
            .live
            .tabs
            .lock()
            .await
            .passkeys
            .get(&self.p.target)
            .is_some_and(|host| Some(host) == host_of(words(&result["url"])).as_ref());
        format!(
            "Page: {}\nURL: {}\nScroll: {above} px above, {below} px below{}\n{}Interactive elements are numbered like [3]; use the number with browser_click, browser_type and browser_select.\n\n{body}",
            words(&result["title"]),
            words(&result["url"]),
            if below > 1200 {
                " (browser_text reads all of it at once)"
            } else {
                ""
            },
            if passkey {
                format!("{}\n", super::passkeys::NOTE)
            } else {
                String::new()
            }
        )
    }
    async fn point(&self, index: i64, action: &str) -> Option<Value> {
        let value = self.action("rect", json!({"index":index})).await.ok()?;
        let rect = serde_json::from_str::<Value>(&value).ok()?;
        self.live.tell(
            "Kuvo.pointer",
            json!({"targetId":self.p.target,"action":action,"rect":rect}),
        );
        sleep(Duration::from_millis(150)).await;
        Some(rect)
    }
    async fn checked(&self, index: i64, wanted: &str) -> io::Result<Option<String>> {
        let kind = self.action("fieldKind", json!({"index":index})).await?;
        Ok(if kind == "missing" {
            Some(format!(
                "There's no element [{index}] on the page now. Take a fresh browser_snapshot."
            ))
        } else if wanted == "password" && kind != "password" {
            Some(format!(
                "[{index}] isn't a password field: a password only goes into one."
            ))
        } else if wanted == "text" && kind != "text" {
            Some(format!("[{index}] isn't a field for a username or email."))
        } else if wanted == "code" && !matches!(kind.as_str(), "text" | "password") {
            Some(format!("[{index}] isn't a field to type a code into."))
        } else {
            None
        })
    }
    async fn fill(&self, index: i64, value: &str, wanted: &str, host: &str) -> io::Result<String> {
        self.action(
            "fill",
            json!({"index":index,"value":value,"wanted":wanted,"site":site_of(host)}),
        )
        .await
    }
    async fn same_page(&self, host: &str, fields: &[(i64, &str)]) -> io::Result<Option<String>> {
        if host_of(&self.live.address(&self.p).await).as_deref() != Some(host) {
            return Ok(Some(format!(
                "The page left {host} while the user answered. Nothing was filled: take a fresh browser_snapshot, and try again if it's still needed."
            )));
        }
        for (index, wanted) in fields {
            if let Some(problem) = self.checked(*index, wanted).await? {
                return Ok(Some(format!(
                    "The page changed while the user answered: {problem} Nothing was filled: take a fresh browser_snapshot, and try again if it's still needed."
                )));
            }
        }
        Ok(None)
    }
    async fn ask_user(&mut self, mut params: Value) -> io::Result<Value> {
        let finder = self.live.engine.work_engine().await;
        if let Some(caller) = self.caller
            && let Ok(data) = finder.session_data(caller).await
            && data["metadata"]["kuvo"]["autopilot"] == true
        {
            return Ok(json!({"type":"autopilot"}));
        }
        params["targetId"] = json!(self.p.target);
        if let Some(caller) = self.caller {
            params["session"] = json!(caller);
        }
        self.pause().await;
        let answer = self.live.asks.ask(params, self.cancel).await;
        self.resume().await?;
        Ok(answer)
    }
    async fn vault(
        &mut self,
        server: &ToolServer,
        mut body: Value,
    ) -> io::Result<Option<(u16, Value)>> {
        body["session"] = json!(self.caller);
        self.pause().await;
        let answer = server
            .route(
                http::Method::POST,
                "kuvo/vault/fill",
                Some(body),
                self.cancel.clone(),
            )
            .await;
        self.resume().await?;
        Ok(answer)
    }
    async fn notes(&self) -> String {
        let mut tabs = self.live.tabs.lock().await;
        let mut notes = tabs.notes.remove(&self.p.target).unwrap_or_default();
        if let Some(caller) = self.caller {
            notes.extend(tabs.download_notes.remove(caller).unwrap_or_default());
        }
        if notes.is_empty() {
            String::new()
        } else {
            format!("{}\n\n", notes.join("\n"))
        }
    }
}
fn no_answer(answer: &Value, what: &str) -> String {
    match words(&answer["type"]){"autopilot"=>"This is on Autopilot: nothing was put to the user, and nothing was chosen for you. If only the user can do it, don't do it or ask again: leave that step undone, carry on with everything else, and list what's left for them in your final reply.".into(),"nobody"=>"No one has Kuvo Code open to answer now. Carry on with what you can without it, and say in your reply what you need; if you work on your own (a bot), schedule a wake-up to try again later.".into(),"timedOut"=>"The user didn't answer in time. Carry on with what you can without it, and say in your reply what you need; if you work on your own (a bot), schedule a wake-up to try again later.".into(),"lost"=>"The connection to the browser closed while the user was answering. Try again.".into(),_=>format!("The user didn't {what}. Ask them in the chat how to go on.")}
}
fn describe(target: &Target, rights: &Access) -> String {
    if empty(&target.url) {
        "New tab — empty".into()
    } else if rights.outside(&target.url).is_some() {
        "A file outside this conversation's folder".into()
    } else {
        format!(
            "{} — {}",
            if target.title.is_empty() {
                "(untitled)"
            } else {
                &target.title
            },
            target.url
        )
    }
}
async fn tab_list(live: &Living, rights: &Access, caller: Option<&str>) -> io::Result<String> {
    let targets = live.refresh_targets().await?;
    let state = live.tabs.lock().await;
    let active = caller
        .and_then(|caller| {
            state
                .claims
                .iter()
                .filter(|(_, c)| c.session == caller)
                .max_by_key(|(_, c)| c.used)
                .map(|(id, _)| id)
        })
        .or(state.active.as_ref());
    if targets.is_empty() {
        return Ok("No tabs are open.".into());
    }
    Ok(format!(
        "Tabs:\n{}",
        targets
            .iter()
            .enumerate()
            .map(|(i, t)| format!(
                "[{}] {}{}",
                i + 1,
                describe(t, rights),
                if active == Some(&t.target_id) {
                    " (in use)"
                } else {
                    ""
                }
            ))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}
impl Browser {
    pub(crate) async fn call(
        &self,
        server: &ToolServer,
        session: Option<&str>,
        params: &Value,
        cancel: CancellationToken,
    ) -> Value {
        let lease = match self.acquire().await {
            Ok(lease) => lease,
            Err(error) => return text(format!("The browser couldn't start: {error}"), true),
        };
        let result = perform(&lease.live, server, session, params, &cancel).await;
        match result {
            Ok(value) => value,
            Err(error) => text(format!("The browser ran into a problem: {error}"), true),
        }
    }
}
async fn perform(
    live: &Arc<Living>,
    server: &ToolServer,
    caller: Option<&str>,
    params: &Value,
    cancel: &CancellationToken,
) -> io::Result<Value> {
    let name = words(&params["name"]);
    let args = &params["arguments"];
    let rights = Access::for_session(&live.engine, caller).await;
    if matches!(name, "browser_navigate" | "browser_new_tab")
        && let Some(target) = live.destination(words(&args["url"]))
        && let Some(file) = rights.outside(&target)
    {
        return Ok(text(outside_note(&file), true));
    }
    let mut permit = Some(
        live.turn
            .clone()
            .acquire_owned()
            .await
            .map_err(io::Error::other)?,
    );
    let mut p = if name == "browser_tabs" {
        let action = args["action"].as_str().unwrap_or("list");
        if action == "list" {
            return Ok(text(tab_list(live, &rights, caller).await?, false));
        }
        let number = integer(&args["tab"]).unwrap_or(0);
        let targets = live.refresh_targets().await?;
        let Some(chosen) = number
            .checked_sub(1)
            .and_then(|n| usize::try_from(n).ok())
            .and_then(|i| targets.get(i))
        else {
            return Ok(text(
                format!(
                    "There's no tab [{number}]. {}",
                    tab_list(live, &rights, caller).await?
                ),
                false,
            ));
        };
        if rights.outside(&chosen.url).is_some() {
            return Ok(text(
                format!(
                    "Tab [{number}] shows a file outside this conversation's folder, which this session asks about before reading. Pick another, or open one with browser_new_tab."
                ),
                false,
            ));
        }
        let state = live.tabs.lock().await;
        if state.held.contains_key(&chosen.target_id) {
            return Ok(text(
                format!(
                    "The user took tab [{number}] over and has it now. Pick another, or carry on without it."
                ),
                false,
            ));
        }
        if let Some(caller) = caller
            && state
                .claims
                .get(&chosen.target_id)
                .is_some_and(|c| c.session != caller && c.used.elapsed() < Duration::from_secs(90))
        {
            return Ok(text(
                format!(
                    "Another conversation's agent is using tab [{number}]. Pick another, or open one with browser_new_tab."
                ),
                false,
            ));
        }
        drop(state);
        if action == "close" {
            live.close_page(&chosen.target_id).await?;
            return Ok(text(
                format!(
                    "Closed tab [{number}]. {}",
                    tab_list(live, &rights, caller).await?
                ),
                false,
            ));
        }
        live.use_page(&chosen.target_id, caller).await?
    } else if name == "browser_new_tab" {
        live.create(caller).await?
    } else {
        live.choose(caller, &rights, &mut permit).await?
    };
    if !rights.anywhere() {
        let tree = live
            .pipe
            .call("Page.getResourceTree", json!({}), Some(&p.session))
            .await?;
        if access::outside_in_tree(&tree["frameTree"], &rights).is_some() {
            p = live.create(caller).await?;
        }
    }
    let mut context = Context::start(
        live,
        p,
        caller,
        rights,
        permit.expect("browser turn"),
        args["accept_dialog"] == true,
        cancel,
    )
    .await?;
    let result = run_tool(&mut context, server, name, args).await;
    // Recheck a file page's resources before any model-visible result is returned.
    let held_back = if !context.rights.anywhere() {
        let tree = context
            .live
            .pipe
            .call("Page.getResourceTree", json!({}), Some(&context.p.session))
            .await;
        match tree {
            Ok(tree) => access::outside_in_tree(&tree["frameTree"], &context.rights).is_some(),
            Err(_) => true,
        }
    } else {
        false
    };
    context.disengage().await;
    if held_back {
        return Ok(text(
            "The page had files from outside this conversation's folder in it, which this session asks about before reading, so this step's result was held back. Take the step again.",
            true,
        ));
    }
    let mut value = result?;
    let notes = context.notes().await;
    if !notes.is_empty()
        && let Some(content) = value["content"].as_array_mut()
    {
        if let Some(first) = content.first_mut().filter(|first| first["type"] == "text") {
            first["text"] = json!(format!("{notes}{}", words(&first["text"])));
        } else {
            content.insert(0, json!({"type":"text","text":notes}));
        }
    }
    Ok(value)
}

async fn run_tool(
    ctx: &mut Context<'_>,
    server: &ToolServer,
    name: &str,
    args: &Value,
) -> io::Result<Value> {
    let index = integer(&args["index"]);
    let named = |key: &str| args[key].as_str();
    let say = |message: String| Ok(text(message, false));
    match name {
        "browser_navigate" | "browser_new_tab" => {
            let input = named("url").unwrap_or_default();
            if input.is_empty() {
                return if name == "browser_new_tab" {
                    say(ctx.snapshot().await)
                } else {
                    Ok(text("Missing url.", true))
                };
            }
            let outcome = ctx.live.go(&ctx.p, input).await?;
            if let Some(error) = outcome["errorText"]
                .as_str()
                .filter(|e| *e != "net::ERR_ABORTED")
            {
                ctx.live.settle(&ctx.p, false).await;
                return Ok(text(
                    format!(
                        "Couldn't open {}: {error}\n\n{}",
                        ctx.live.destination(input).unwrap_or_default(),
                        ctx.snapshot().await
                    ),
                    true,
                ));
            }
            ctx.live.settle(&ctx.p, true).await;
            say(ctx.snapshot().await)
        }
        "browser_snapshot" | "browser_tabs" => say(ctx.snapshot().await),
        "browser_text" => {
            let value = ctx
                .live
                .evaluate(
                    &ctx.p,
                    "document.body ? document.body.innerText : ''",
                    false,
                    true,
                )
                .await
                .unwrap_or_default();
            let value = words(&value);
            say(if value.is_empty() {
                "(no text)".into()
            } else {
                trimmed(value, 30_000, "\n… (cut)")
            })
        }
        "browser_click" => {
            let Some(index) = index else {
                return Ok(text("Missing index.", true));
            };
            ctx.point(index, "click").await;
            let before = ctx.live.address(&ctx.p).await;
            let old: HashSet<_> = ctx
                .live
                .refresh_targets()
                .await?
                .into_iter()
                .map(|t| t.target_id)
                .collect();
            let outcome = ctx.action("click", json!({"index":index})).await?;
            if outcome != "ok" {
                return say(outcome);
            }
            let deadline = Instant::now() + Duration::from_secs(1);
            while Instant::now() < deadline
                && ctx.live.address(&ctx.p).await == before
                && ctx
                    .live
                    .evaluate(&ctx.p, "document.readyState", false, true)
                    .await
                    .is_ok_and(|v| v != "loading")
            {
                sleep(Duration::from_millis(100)).await;
            }
            let opened = ctx.live.refresh_targets().await?.into_iter().find(|t| {
                !old.contains(&t.target_id) && t.opener_id.as_deref() == Some(&ctx.p.target)
            });
            if let Some(opened) = opened {
                ctx.disengage().await;
                ctx.p = ctx.live.use_page(&opened.target_id, ctx.caller).await?;
                ctx.engage().await?;
                ctx.live.settle(&ctx.p, true).await;
            } else {
                let changed = ctx.live.address(&ctx.p).await != before;
                ctx.live.settle(&ctx.p, changed).await;
            }
            say(format!("Clicked [{index}].\n\n{}", ctx.snapshot().await))
        }
        "browser_type" => {
            let (Some(index), Some(value)) = (index, named("text")) else {
                return Ok(text("Missing index or text.", true));
            };
            let submit = args["submit"] == true;
            ctx.point(index, "type").await;
            let outcome=ctx.action("type",json!({"index":index,"text":value,"submit":submit,"clear":args["clear"]!=false})).await?;
            if outcome != "ok" {
                return say(outcome);
            }
            ctx.live.settle(&ctx.p, submit).await;
            say(format!(
                "Typed into [{index}]{}.\n\n{}",
                if submit { " and submitted" } else { "" },
                ctx.snapshot().await
            ))
        }
        "browser_select" => {
            let (Some(index), Some(option)) = (index, named("option")) else {
                return Ok(text("Missing index or option.", true));
            };
            ctx.point(index, "select").await;
            let outcome = ctx
                .action("select", json!({"index":index,"option":option}))
                .await?;
            if outcome != "ok" {
                return say(outcome);
            }
            ctx.live.settle(&ctx.p, false).await;
            say(format!(
                "Selected “{option}” in [{index}].\n\n{}",
                ctx.snapshot().await
            ))
        }
        "browser_scroll" => {
            let pages = args["pages"].as_f64().filter(|n| *n != 0.0).unwrap_or(1.0);
            ctx.run("window.scrollBy({top: (down ? 1 : -1) * window.innerHeight * 0.85 * pages, behavior: 'instant'}); return 'ok';",json!({"down":args["direction"]!="up","pages":pages})).await?;
            sleep(Duration::from_millis(300)).await;
            say(ctx.snapshot().await)
        }
        "browser_press" => {
            let Some(key) = named("key").filter(|s| !s.is_empty()) else {
                return Ok(text("Missing key.", true));
            };
            let outcome = ctx
                .run(ctx.live.config.script("press"), json!({"key":key}))
                .await?;
            if outcome != "ok" {
                return say(outcome);
            }
            ctx.live.settle(&ctx.p, key == "Enter").await;
            say(format!("Pressed {key}.\n\n{}", ctx.snapshot().await))
        }
        "browser_back" | "browser_forward" => {
            let step = if name == "browser_back" { -1 } else { 1 };
            let history = ctx
                .live
                .pipe
                .call("Page.getNavigationHistory", json!({}), Some(&ctx.p.session))
                .await?;
            let at = history["currentIndex"].as_i64().unwrap_or(0) + step;
            let Some(entry) = usize::try_from(at)
                .ok()
                .and_then(|i| history["entries"].as_array()?.get(i))
            else {
                return Ok(text(
                    if step < 0 {
                        "There's no page to go back to."
                    } else {
                        "There's no page to go forward to."
                    },
                    false,
                ));
            };
            if let Some(file) = entry["url"]
                .as_str()
                .and_then(|url| ctx.rights.outside(url))
            {
                return Ok(text(outside_note(&file), true));
            }
            ctx.live
                .pipe
                .call(
                    "Page.navigateToHistoryEntry",
                    json!({"entryId":entry["id"]}),
                    Some(&ctx.p.session),
                )
                .await?;
            ctx.live.settle(&ctx.p, true).await;
            say(ctx.snapshot().await)
        }
        "browser_wait" => {
            if let Some(wanted) = named("text").filter(|s| !s.is_empty()) {
                let deadline = Instant::now() + Duration::from_secs(30);
                let needle = json!(wanted.to_lowercase());
                let expression = format!(
                    "(document.body ? document.body.innerText : '').toLowerCase().includes({needle})"
                );
                while Instant::now() < deadline {
                    if ctx
                        .live
                        .evaluate(&ctx.p, &expression, false, true)
                        .await
                        .is_ok_and(|v| v == true)
                    {
                        return say(format!(
                            "“{wanted}” is on the page.\n\n{}",
                            ctx.snapshot().await
                        ));
                    }
                    sleep(Duration::from_millis(500)).await;
                }
                return say(format!(
                    "“{wanted}” didn't show up in 30 seconds.\n\n{}",
                    ctx.snapshot().await
                ));
            }
            let seconds = args["seconds"]
                .as_f64()
                .filter(|s| *s != 0.0)
                .unwrap_or(2.0)
                .clamp(0.0, 10.0);
            sleep(Duration::from_secs_f64(seconds)).await;
            say(ctx.snapshot().await)
        }
        "browser_hover" => {
            let Some(index) = index else {
                return Ok(text("Missing index.", true));
            };
            let Some(rect) = ctx.point(index, "hover").await else {
                return say(format!(
                    "There's no element [{index}] on the page now. Take a fresh browser_snapshot."
                ));
            };
            let n = |key: &str| rect[key].as_f64().unwrap_or(0.0);
            ctx.live.pipe.call("Input.dispatchMouseEvent",json!({"type":"mouseMoved","x":n("x")+n("width")/2.0,"y":n("y")+n("height")/2.0}),Some(&ctx.p.session)).await?;
            sleep(Duration::from_millis(400)).await;
            say(format!(
                "Hovered over [{index}].\n\n{}",
                ctx.snapshot().await
            ))
        }
        "browser_screenshot" => {
            let image = ctx.capture().await;
            match image {
                Ok(data) => {
                    Ok(json!({"content":[{"type":"image","data":data,"mimeType":"image/png"}]}))
                }
                Err(_) => Ok(text("Couldn't capture the page.", true)),
            }
        }
        "browser_save_pdf" => {
            let Some(given) = named("path") else {
                return Ok(text("Give a full path ending in .pdf.", true));
            };
            let file = expand(given);
            if !file.is_absolute() || !given.to_lowercase().ends_with(".pdf") {
                return Ok(text("Give a full path ending in .pdf.", true));
            }
            if !ctx.rights.allows(&file) {
                return Ok(text(
                    format!(
                        "Not saved: {} is outside this conversation's folder, and this session asks before writing files elsewhere. Save it in the folder; if the user wants it elsewhere, ask them first, then move it there (a command, which asks them).",
                        file.display()
                    ),
                    true,
                ));
            }
            if empty(&ctx.live.address(&ctx.p).await) {
                return Ok(text(
                    "The browser is empty. Open the page first: for a document, write it as an HTML file and open it (file:///…).",
                    true,
                ));
            }
            match ctx.pdf(&file).await {
                Ok(pages) => say(format!(
                    "Saved the page as a {}PDF at {}.",
                    if pages > 0 {
                        format!("{pages}-page ")
                    } else {
                        String::new()
                    },
                    file.display()
                )),
                Err(error) => Ok(text(
                    format!("Couldn't save the page as a PDF: {error}"),
                    true,
                )),
            }
        }
        "browser_upload" => ctx.upload(index, args).await,
        "browser_sign_in" => sign_in(ctx, server, args).await,
        "browser_fill" => fill_vault(ctx, server, args).await,
        "browser_ask_code" => {
            let Some(index) = index else {
                return Ok(text("Missing index.", true));
            };
            let Some(host) = host_of(&ctx.live.address(&ctx.p).await) else {
                return Ok(text("Open the page that asks for the code first.", false));
            };
            if let Some(problem) = ctx.checked(index, "code").await? {
                return say(problem);
            }
            let answer = ctx
                .ask_user(
                    json!({"kind":"code","host":host,"prompt":named("prompt").unwrap_or_default()}),
                )
                .await?;
            if answer["type"] != "code" || words(&answer["code"]).is_empty() {
                return say(no_answer(&answer, "give a code"));
            }
            if let Some(problem) = ctx.same_page(&host, &[(index, "code")]).await? {
                return say(problem);
            }
            let result = ctx
                .fill(index, words(&answer["code"]), "code", &host)
                .await?;
            if result != "ok" {
                return say(result);
            }
            sleep(Duration::from_millis(300)).await;
            say(format!(
                "Filled the code (you can't see it). Submit it if the page doesn't on its own.\n\n{}",
                ctx.snapshot().await
            ))
        }
        "browser_ask_user" => {
            let Some(instruction) = named("instruction").filter(|s| !s.is_empty()) else {
                return Ok(text("Missing instruction.", true));
            };
            let host =
                host_of(&ctx.live.address(&ctx.p).await).unwrap_or_else(|| "the page".into());
            let answer = ctx
                .ask_user(json!({"kind":"action","host":host,"instruction":instruction}))
                .await?;
            if answer["type"] != "done" {
                return say(no_answer(&answer, "do it"));
            }
            ctx.live.settle(&ctx.p, false).await;
            say(format!(
                "The user says it's done.\n\n{}",
                ctx.snapshot().await
            ))
        }
        _ => Ok(text(format!("Unknown tool {name}."), true)),
    }
}
impl Context<'_> {
    async fn capture(&self) -> io::Result<String> {
        if self.run(self.live.config.script("mask"), json!({})).await? != "ok" {
            return Err(bad("Couldn't hide the filled fields."));
        }
        let capture=async {
            let metrics=self.live.pipe.call("Page.getLayoutMetrics",json!({}),Some(&self.p.session)).await?;
            let view=&metrics["cssVisualViewport"];
            let ratio=self.live.evaluate(&self.p,"window.devicePixelRatio",false,true).await.ok().and_then(|v|v.as_f64()).unwrap_or(1.0);
            let width=view["clientWidth"].as_f64().unwrap_or(1280.0).max(1.0);let scale=(1280.0/width).min(1.0);
            let captured=self.live.pipe.call("Page.captureScreenshot",json!({"format":"png","clip":{"x":view["pageX"],"y":view["pageY"],"width":width,"height":view["clientHeight"],"scale":scale/ratio}}),Some(&self.p.session)).await?;
            captured["data"].as_str().map(str::to_owned).ok_or_else(||bad("No picture."))
        }.await;
        let _ = self.run(self.live.config.script("unmask"), json!({})).await;
        capture
    }
    async fn pdf(&self, file: &Path) -> io::Result<usize> {
        if self
            .run(self.live.config.script("printMask"), json!({}))
            .await?
            != "ok"
        {
            return Err(bad("Couldn't hide the filled fields."));
        }
        let (width, height) = if std::env::var("KUVO_PAPER").as_deref() == Ok("letter") {
            (8.5, 11.0)
        } else {
            (8.27, 11.69)
        };
        let data=self.live.pipe.call_for("Page.printToPDF",json!({"printBackground":true,"preferCSSPageSize":true,"marginTop":0.5,"marginBottom":0.5,"marginLeft":0.5,"marginRight":0.5,"paperWidth":width,"paperHeight":height}),Some(&self.p.session),Duration::from_secs(60)).await;
        let _ = self
            .run(self.live.config.script("printUnmask"), json!({}))
            .await;
        let data = STANDARD
            .decode(words(&data?["data"]))
            .map_err(io::Error::other)?;
        let count = regex::bytes::Regex::new(r"/Type\s*/Page[^s]")
            .map_err(io::Error::other)?
            .find_iter(&data)
            .count();
        kuvo_store::files::write_secret(file, &data)?;
        Ok(count)
    }
    async fn upload(&self, index: Option<i64>, args: &Value) -> io::Result<Value> {
        let paths: Vec<_> = args["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(expand)
            .map(|p| {
                if p.is_absolute() {
                    p
                } else {
                    std::env::current_dir().unwrap_or_default().join(p)
                }
            })
            .collect();
        let Some(index) = index.filter(|_| !paths.is_empty()) else {
            return Ok(text("Missing index or files.", true));
        };
        if let Some(file) = paths.iter().find(|p| !p.is_file()) {
            return Ok(text(
                format!(
                    "Can't read {} on this machine. Give its full path.",
                    file.display()
                ),
                false,
            ));
        }
        if let Some(file) = paths.iter().find(|p| !self.rights.allows(p)) {
            return Ok(text(
                format!(
                    "Not attached: {} is outside this conversation's folder, and this session asks before reading files elsewhere. Ask the user first; if they agree, copy it into the folder (a command, which asks them) and attach it from there.",
                    file.display()
                ),
                true,
            ));
        }
        let body = format!(
            "{}\n{}",
            self.live.config.script("find"),
            self.live.config.script("fileInput")
        );
        let found = self
            .script_value(&body, json!({"index":index}), false)
            .await?;
        if found["type"] == "string" && found["value"] == "missing" {
            return Ok(text(
                format!(
                    "There's no element [{index}] on the page now. Take a fresh browser_snapshot."
                ),
                false,
            ));
        }
        if let Some(object) = found["objectId"].as_str() {
            self.live
                .pipe
                .call(
                    "DOM.setFileInputFiles",
                    json!({"files":paths,"objectId":object}),
                    Some(&self.p.session),
                )
                .await?;
        } else {
            self.live
                .pipe
                .call(
                    "Page.setInterceptFileChooserDialog",
                    json!({"enabled":true}),
                    Some(&self.p.session),
                )
                .await?;
            let mut events = self.live.pipe.events();
            let attached = async {
                self.action("click", json!({"index":index})).await?;
                let chooser = tokio::time::timeout(Duration::from_secs(4), async {
                    loop {
                        let event = events.recv().await.ok()?;
                        if event.method == "Page.fileChooserOpened"
                            && event.session_id.as_deref() == Some(&self.p.session)
                        {
                            return event.params["backendNodeId"].as_u64();
                        }
                    }
                })
                .await
                .ok()
                .flatten();
                if let Some(node) = chooser {
                    self.live
                        .pipe
                        .call(
                            "DOM.setFileInputFiles",
                            json!({"files":paths,"backendNodeId":node}),
                            Some(&self.p.session),
                        )
                        .await?;
                    Ok::<_, io::Error>(true)
                } else {
                    Ok(false)
                }
            }
            .await;
            if !attached? {
                return Ok(text(
                    format!(
                        "[{index}] didn't open a file picker. Give the number of the upload button or its file input."
                    ),
                    false,
                ));
            }
        }
        sleep(Duration::from_millis(400)).await;
        Ok(text(
            format!(
                "Attached {}.\n\n{}",
                if paths.len() == 1 {
                    "the file".into()
                } else {
                    format!("{} files", paths.len())
                },
                self.snapshot().await
            ),
            false,
        ))
    }
}
async fn sign_in(ctx: &mut Context<'_>, server: &ToolServer, args: &Value) -> io::Result<Value> {
    let username = integer(&args["username"]);
    let password = integer(&args["password"]);
    if username.is_none() && password.is_none() {
        return Ok(text(
            "Name the username field, the password field, or both, by number.",
            true,
        ));
    }
    let address = ctx.live.address(&ctx.p).await;
    let Some(host) = host_of(&address) else {
        return Ok(text("Open the sign-in page first.", false));
    };
    let fields: Vec<_> = [
        (password, "password", "the password"),
        (username, "text", "the username"),
    ]
    .into_iter()
    .filter_map(|(index, wanted, what)| Some((index?, wanted, what)))
    .collect();
    for (index, wanted, _) in &fields {
        if let Some(problem) = ctx.checked(*index, wanted).await? {
            return Ok(text(problem, false));
        }
    }
    let mut request = json!({"origin":address,"purpose":"sign-in"});
    if let Some(item) = args["item"].as_str() {
        request["item"] = json!(item);
    }
    let vault = ctx.vault(server, request).await?;
    let mut kept_name = None;
    let answer=match vault {
        Some((200,vault))=>{kept_name=vault["item"]["name"].as_str().map(str::to_owned);json!({"type":"signIn","username":vault["username"],"password":vault["values"]["password"]})},
        Some((status,error)) if status==403||(args["item"].is_string()&&status!=423)=>return Ok(text(error["message"].as_str().unwrap_or("The vault didn't fill that."),status!=403)),
        _=>ctx.ask_user(json!({"kind":"signIn","host":host,"username":username.is_some(),"password":password.is_some()})).await?,
    };
    if answer["type"] != "signIn" {
        return Ok(text(no_answer(&answer, "sign in"), false));
    }
    let offered = [
        (username, "text", "the username", words(&answer["username"])),
        (
            password,
            "password",
            "the password",
            words(&answer["password"]),
        ),
    ];
    let offered: Vec<_> = offered
        .into_iter()
        .filter_map(|(index, wanted, what, value)| Some((index?, wanted, what, value)))
        .filter(|(_, _, _, value)| !value.is_empty())
        .collect();
    if offered.is_empty() {
        return Ok(text(
            kept_name.map_or_else(
                || "The user didn't give a sign-in for that.".into(),
                |name| format!("“{name}” has nothing for those fields."),
            ),
            false,
        ));
    }
    let check = offered
        .iter()
        .map(|(index, wanted, _, _)| (*index, *wanted))
        .collect::<Vec<_>>();
    if let Some(problem) = ctx.same_page(&host, &check).await? {
        return Ok(text(problem, false));
    }
    let mut filled = Vec::new();
    for (index, wanted, what, value) in offered {
        let result = ctx.fill(index, value, wanted, &host).await?;
        if result != "ok" {
            return Ok(text(result, false));
        }
        filled.push(what);
    }
    sleep(Duration::from_millis(300)).await;
    let from = kept_name.map_or_else(String::new, |name| format!(" from the vault's “{name}”"));
    Ok(text(
        format!(
            "Filled {} for {host}{from} (you can't see them). Submit the form to sign in.\n\n{}",
            filled.join(" and "),
            ctx.snapshot().await
        ),
        false,
    ))
}
async fn fill_vault(ctx: &mut Context<'_>, server: &ToolServer, args: &Value) -> io::Result<Value> {
    let (Some(index), Some(item)) = (
        integer(&args["index"]),
        args["item"].as_str().filter(|s| !s.is_empty()),
    ) else {
        return Ok(text(
            "Name the field by its number and the vault item to fill it from.",
            true,
        ));
    };
    let address = ctx.live.address(&ctx.p).await;
    let Some(host) = host_of(&address) else {
        return Ok(text("Open the page first.", false));
    };
    let kind = ctx.action("fieldKind", json!({"index":index})).await?;
    if kind == "missing" {
        return Ok(text(
            format!("There's no element [{index}] on the page now. Take a fresh browser_snapshot."),
            false,
        ));
    }
    if !matches!(kind.as_str(), "text" | "password") {
        return Ok(text(format!("[{index}] isn't a field to fill."), false));
    }
    let mut request = json!({"origin":address,"purpose":"field","item":item});
    for name in ["field", "why"] {
        if let Some(value) = args[name].as_str() {
            request[name] = json!(value);
        }
    }
    let Some((status, answer)) = ctx.vault(server, request).await? else {
        return Ok(text(
            "This machine's vault can't be reached now. Try again in a moment.",
            true,
        ));
    };
    if status != 200 {
        return Ok(text(
            answer["message"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("The vault didn't fill that ({status}).")),
            status != 403,
        ));
    }
    let name = words(&answer["item"]["name"]);
    let Some((field, value)) = answer["values"].as_object().and_then(|m| m.iter().next()) else {
        return Ok(text(format!("“{name}” has nothing to fill there."), false));
    };
    let wanted = if field == "password" {
        "password"
    } else {
        "code"
    };
    if let Some(problem) = ctx.same_page(&host, &[(index, wanted)]).await? {
        return Ok(text(problem, false));
    }
    let result = ctx.fill(index, words(value), wanted, &host).await?;
    if result != "ok" {
        return Ok(text(result, false));
    }
    sleep(Duration::from_millis(300)).await;
    Ok(text(
        format!(
            "Filled [{index}] with “{name}”'s {field} (you can't see it).\n\n{}",
            ctx.snapshot().await
        ),
        false,
    ))
}
