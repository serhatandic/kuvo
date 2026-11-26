//! Kuvo's own MCP server. The relay authenticates and attributes the call;
//! the tool names and page scripts are built from the shared definitions.
use http::Method;
use http_body_util::BodyExt;
use kuvo_agents::tool_relay::NativeTools;
use kuvo_server::{Caller, DispatchRequest, Dispatcher, LocalOrigin, Transport, WeakDispatcher};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{future::Future, io, path::PathBuf, pin::Pin, sync::OnceLock, time::Duration};
use tokio_util::sync::CancellationToken;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Definitions {
    tools: Vec<Value>,
    instructions: String,
    vault_instructions: String,
}

pub struct ToolServer {
    definitions: Definitions,
    dispatcher: OnceLock<WeakDispatcher>,
    home: PathBuf,
    mac: bool,
    version: String,
    client: reqwest::Client,
}
impl ToolServer {
    pub fn new(home: PathBuf, mac: bool) -> io::Result<Self> {
        let definitions =
            serde_json::from_str(include_str!("../../../../dist/browser-config.json"))
                .map_err(io::Error::other)?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(io::Error::other)?;
        let version = kuvo_store::files::text_of(&home.join("bin/box-version"))
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "dev".into());
        Ok(Self {
            definitions,
            dispatcher: OnceLock::new(),
            home,
            mac,
            version,
            client,
        })
    }
    pub fn attach(&self, dispatcher: &Dispatcher) {
        let _ = self.dispatcher.set(dispatcher.downgrade());
    }
    async fn route(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        cancel: CancellationToken,
    ) -> Option<(u16, Value)> {
        let dispatcher = self.dispatcher.get()?.upgrade()?;
        let request = DispatchRequest {
            method,
            path: path.to_owned(),
            headers: Default::default(),
            body: body.map(|v| v.to_string().into()).unwrap_or_default(),
            caller: Caller::Local(Transport::Http {
                origin: LocalOrigin::Direct,
                internal: true,
            }),
        };
        let answer = dispatcher.dispatch(request, cancel).await;
        let status = answer.status.as_u16();
        let body = answer.body.collect().await.ok()?.to_bytes();
        Some((status, serde_json::from_slice(&body).ok()?))
    }
    async fn list(&self, path: &str, cancel: CancellationToken) -> (Vec<Value>, String) {
        let Some((200, value)) = self.route(Method::GET, path, None, cancel).await else {
            return (vec![], String::new());
        };
        (
            value["tools"].as_array().cloned().unwrap_or_default(),
            value["instructions"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        )
    }
    async fn tool_list(&self, cancel: CancellationToken) -> (Vec<Value>, String) {
        let ((vault, _), (sandbox, _), (queue, queue_note)) = tokio::join!(
            self.list("kuvo/vault/tools", cancel.clone()),
            self.list("kuvo/sandbox/tools", cancel.clone()),
            self.list("kuvo/work-queue/tools", cancel)
        );
        let mut tools = self.definitions.tools.clone();
        let mut note = self.definitions.instructions.clone();
        if vault.is_empty() {
            tools.retain(|t| t["name"] != "browser_fill");
            if let Some(sign_in) = tools.iter_mut().find(|t| t["name"] == "browser_sign_in") {
                if let Some(fields) = sign_in["inputSchema"]["properties"].as_object_mut() {
                    fields.remove("item");
                }
            }
        } else {
            note.push_str(&self.definitions.vault_instructions);
        }
        note.push_str(&queue_note);
        tools.extend(vault);
        tools.extend(sandbox);
        tools.extend(queue);
        (tools, note)
    }
    async fn call(
        &self,
        session: Option<&str>,
        params: &Value,
        cancel: CancellationToken,
    ) -> Value {
        let name = params["name"].as_str().unwrap_or_default();
        let (route, fallback) = if name.starts_with("vault_") {
            (
                "kuvo/vault/tools",
                "This machine's vault can't be reached now. Try again in a moment.",
            )
        } else if name.starts_with("sandbox_") {
            (
                "kuvo/sandbox/tools",
                "Kuvo on this machine can't be reached now. Try again in a moment.",
            )
        } else if name.starts_with("queue_") {
            (
                "kuvo/work-queue/tools",
                "Kuvo on this machine can't be reached now. Try again in a moment.",
            )
        } else if name.starts_with("browser_") {
            return self.browser(session, params, cancel).await;
        } else {
            (
                "kuvo/agent-tools",
                "Kuvo Code on this machine didn't answer. Try again in a moment.",
            )
        };
        let operation = params["_meta"]["kuvo.operation"].as_str();
        let body = json!({"name":name,"arguments":params["arguments"].as_object().cloned().unwrap_or_default(),"session":session,"operation":operation});
        match self.route(Method::POST, route, Some(body), cancel).await {
            Some((200, answer)) if answer["text"].is_string() => {
                let mut words = answer["text"].as_str().unwrap_or_default().to_owned();
                if name == "send_message"
                    && let Some(delivery) = answer["delivery"].as_str()
                {
                    words.push_str(&format!("\nDelivery: {delivery}."));
                }
                text(words, answer["isError"] == true)
            }
            Some((_, answer)) => text(answer["message"].as_str().unwrap_or(fallback), true),
            None => text(fallback, true),
        }
    }
    async fn browser(
        &self,
        session: Option<&str>,
        params: &Value,
        cancel: CancellationToken,
    ) -> Value {
        if !self.mac {
            return text("The browser couldn't start.", true);
        }
        let Some(tools) = crate::mac_app::read(&self.home.join("run")).and_then(|r| r.tools) else {
            return text(
                "Kuvo Code on this Mac didn't take the browser's call. Ask the user to open Kuvo Code, then try again.",
                true,
            );
        };
        let id = fresh();
        let client_id = fresh();
        let send = |message: Value| {
            let mut request = self
                .client
                .post(&tools.url)
                .bearer_auth(&tools.token)
                .header("accept", "application/json, text/event-stream")
                .header("mcp-session-id", &client_id);
            if let Some(session) = session {
                request = request.header("x-kuvo-session", session);
            }
            request.json(&message)
        };
        let request =
            send(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":params})).send();
        let mut pending = AppCall(Some(send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":id}})).timeout(Duration::from_secs(5))));
        let result = tokio::select! {
            answer = async { let answer = request.await?; answer.json::<Value>().await } => answer,
            () = cancel.cancelled() => {
                return text("The call was stopped.",true);
            }
        };
        match result {
            Ok(answer) if answer["jsonrpc"] == "2.0" && answer["id"] == id => {
                pending.0.take();
                answer.get("result").cloned().unwrap_or_else(|| {
                    text(
                        format!(
                            "Kuvo Code's browser said: {}",
                            answer["error"]["message"]
                                .as_str()
                                .unwrap_or("The call didn't finish.")
                        ),
                        true,
                    )
                })
            }
            _ => text(
                "Kuvo Code on this Mac didn't take the browser's call. Ask the user to open Kuvo Code, then try again.",
                true,
            ),
        }
    }
}
struct AppCall(Option<reqwest::RequestBuilder>);
impl Drop for AppCall {
    fn drop(&mut self) {
        if let Some(request) = self.0.take() {
            tokio::spawn(async move {
                let _ = request.send().await;
            });
        }
    }
}
fn fresh() -> String {
    let mut bytes = [0; 16];
    let _ = getrandom::fill(&mut bytes);
    hex::encode(bytes)
}
fn text(words: impl Into<String>, error: bool) -> Value {
    json!({"content":[{"type":"text","text":words.into()}],"isError":error})
}
impl NativeTools for ToolServer {
    fn request<'a>(
        &'a self,
        session: Option<&'a str>,
        message: Value,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Value> + Send + 'a>> {
        Box::pin(async move {
            let id = message["id"].clone();
            let params = &message["params"];
            let result = match message["method"].as_str().unwrap_or_default() {
                "initialize" => {
                    let (_, note) = self.tool_list(cancel).await;
                    let wanted = params["protocolVersion"].as_str().unwrap_or_default();
                    let version = if ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"]
                        .contains(&wanted)
                    {
                        wanted
                    } else {
                        "2025-06-18"
                    };
                    json!({"protocolVersion":version,"capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"kuvo-browser","title":"Kuvo Code browser","version":self.version},"instructions":note})
                }
                "ping" => json!({}),
                "tools/list" => json!({"tools":self.tool_list(cancel).await.0}),
                "tools/call" => self.call(session, params, cancel).await,
                method => {
                    return json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":format!("Unknown method {method}")}});
                }
            };
            json!({"jsonrpc":"2.0","id":id,"result":result})
        })
    }
}
