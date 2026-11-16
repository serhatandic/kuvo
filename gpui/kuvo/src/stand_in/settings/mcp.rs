//! The stand-in's MCP Servers (`kuvo/settings/mcp`), as a box describes it (PROTOCOL.md, "A box's
//! settings", MCP Servers): its servers with how each is doing, Add Server's and Edit's forms
//! checked as they're typed, Remove, on and off, and a server's sign-in the box leads. Made up, in
//! the box's words, so the desktop app's tests and shots draw the flow kinds against it.

use serde_json::{Map, Value, json};

use super::Settings;
use super::pages::row;

/// One of its servers: its settings, and how the engine says it's doing.
#[derive(Clone, Debug)]
pub struct Server {
    pub name: String,
    pub entry: Value,
    pub status: String,
    pub error: Option<String>,
}

/// A server's sign-in under way, or ended.
#[derive(Clone, Debug, PartialEq)]
pub struct SignInFlow {
    pub server: String,
    pub state: &'static str,
    pub line: String,
}

#[derive(Clone, Debug)]
pub struct Mcp {
    /// Its section is listed (tests that don't look at it leave it out).
    pub on: bool,
    pub servers: Vec<Server>,
    pub flow: Option<SignInFlow>,
    /// What its forms were sent, in order (`values`).
    pub sent: Vec<Value>,
}

pub const SIGN_IN_PAGE: &str = "https://linear.example.com/authorize?state=demo";

impl Default for Mcp {
    fn default() -> Mcp {
        let server = |name: &str, entry: Value, status: &str, error: Option<&str>| Server { name: name.into(), entry, status: status.into(), error: error.map(str::to_owned) };
        Mcp {
            on: false,
            servers: vec![
                server("files", json!({ "type": "local", "command": ["npx", "-y", "@modelcontextprotocol/server-filesystem", "~/Documents"], "codemode": false }), "connected", None),
                server("linear", json!({ "type": "remote", "url": "https://mcp.linear.app/mcp" }), "needs_auth", None),
                server("postgres", json!({ "type": "local", "command": ["uvx", "postgres-mcp", "--readonly"], "environment": { "DATABASE_URL": "{env:DATABASE_URL}" } }), "failed", Some("Connection closed: could not connect to server at localhost:5432. Is the server running and accepting TCP/IP connections?")),
                server("sentry", json!({ "type": "remote", "url": "https://mcp.sentry.dev/mcp", "disabled": true }), "disabled", None),
            ],
            flow: None,
            sent: Vec::new(),
        }
    }
}

fn refused(status: u16, message: &str) -> (u16, Value) {
    (status, json!({ "message": message }))
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_owned()
}

fn command_line(entry: &Value) -> String {
    entry["command"].as_array().map(|words| words.iter().filter_map(Value::as_str).map(|word| if word.contains(' ') { format!("\"{word}\"") } else { word.to_owned() }).collect::<Vec<_>>().join(" ")).unwrap_or_default()
}

fn pairs(value: &Value) -> Value {
    Value::Array(value.as_object().map(|object| object.iter().map(|(key, value)| json!({ "key": key, "value": value })).collect()).unwrap_or_default())
}

impl Mcp {
    pub fn section() -> Value {
        json!({ "id": "mcp", "title": "MCP Servers", "group": "Agents & Tools", "icon": "sliders", "line": "Programs and web services that give agents more tools.", "keywords": ["mcp", "servers", "tools", "add server", "sign in", "oauth", "command", "url", "timeout"] })
    }

    fn form(&self, name: Option<&str>, entry: &Value, place: &str) -> Value {
        let remote = entry["type"] == json!("remote");
        let note = "To keep a key out of the settings file, write {env:NAME} to use one of your environment variables.";
        let mut fields = Vec::new();
        if name.is_none() {
            fields.push(json!({ "id": "name", "title": "Name", "kind": "text", "value": "", "placeholder": "files, or paste its setup from the README" }));
        }
        fields.extend([
            json!({ "id": "runs", "title": "Runs", "kind": "choice", "value": if remote { "remote" } else { "local" }, "choices": [{ "id": "local", "title": format!("On {place}") }, { "id": "remote", "title": "On the web" }] }),
            json!({ "id": "command", "title": "Command", "kind": "lines", "mono": true, "value": command_line(entry), "placeholder": "npx -y @modelcontextprotocol/server-filesystem ~/Documents", "shown": { "field": "runs", "is": ["local"] } }),
            json!({ "id": "cwd", "title": "Working folder", "kind": "folder", "value": text(&entry["cwd"]), "placeholder": "The project’s folder", "folder": { "choose": "Choose…", "clear": "The project’s folder", "prompt": "Choose", "message": "Choose the folder the server runs in." }, "shown": { "field": "runs", "is": ["local"] } }),
            json!({ "id": "environment", "title": "Environment Variables", "kind": "pairs", "value": pairs(&entry["environment"]), "pairs": { "key": "NAME", "value": "Value" }, "line": format!("Each by its name, like GITHUB_TOKEN. {note}"), "shown": { "field": "runs", "is": ["local"] } }),
            json!({ "id": "url", "title": "URL", "kind": "text", "value": text(&entry["url"]), "placeholder": "https://example.com/mcp", "shown": { "field": "runs", "is": ["remote"] } }),
            json!({ "id": "oauth", "title": "Sign in when the server asks", "kind": "switch", "value": entry["oauth"] != json!(false), "line": "Turn off for a server that takes an API key in a header instead.", "shown": { "field": "runs", "is": ["remote"] } }),
            json!({ "id": "headers", "title": "Headers", "kind": "pairs", "value": pairs(&entry["headers"]), "pairs": { "key": "Header, like Authorization", "value": "Value" }, "line": note, "shown": { "field": "runs", "is": ["remote"] } }),
            json!({ "id": "codemode", "title": "Let the agent call its tools from code", "kind": "switch", "value": name.is_some() && entry["codemode"] != json!(false), "line": "Code Mode: the agent looks up this server's tools and combines several in one step, which keeps each message small for a server with many tools. Smaller models may not find its tools this way." }),
        ]);
        match name {
            None => json!({ "title": "Add Server", "submit": "Add Server", "fields": fields, "note": format!("A server on {place} runs a command; one on the web is a URL.") }),
            Some(name) => json!({ "title": name, "submit": "Save", "fields": fields }),
        }
    }

    fn badge(server: &Server) -> Value {
        if server.entry["disabled"] == json!(true) {
            return json!({ "words": "Off", "tone": "quiet" });
        }
        match server.status.as_str() {
            "connected" => json!({ "words": "Connected", "tone": "good" }),
            "pending" => json!({ "words": "Connecting…", "tone": "busy" }),
            "needs_auth" => json!({ "words": "Needs sign-in", "tone": "warning" }),
            "failed" => json!({ "words": "Couldn’t connect", "tone": "failed" }),
            _ => json!({ "words": "Not started", "tone": "quiet" }),
        }
    }

    pub fn page(&self, here: bool, name: &str) -> Value {
        let place = if here { "this Mac" } else { name };
        let items: Vec<Value> = self
            .servers
            .iter()
            .map(|server| {
                let on = server.entry["disabled"] != json!(true);
                let flow = self.flow.as_ref().filter(|flow| flow.server == server.name);
                let mut actions = Vec::new();
                if on && flow.is_none() && server.status == "needs_auth" {
                    actions.push(json!({ "id": "sign-in", "title": "Sign In…" }));
                }
                if on && server.status == "failed" {
                    actions.push(json!({ "id": "try-again", "title": "Try Again" }));
                }
                actions.push(json!({ "id": "edit", "title": "Edit…", "form": self.form(Some(&server.name), &server.entry, place) }));
                actions.push(json!({ "id": "remove", "title": "Remove…", "style": "destructive", "confirm": { "title": format!("Remove {}?", server.name), "body": "The agent loses its tools.", "button": "Remove" } }));
                let line = server.entry["url"].as_str().map(str::to_owned).unwrap_or_else(|| command_line(&server.entry));
                let mut item = json!({ "id": format!("server-{}", server.name), "title": server.name, "line": line, "kind": "switch", "value": on, "can": true, "actions": actions, "badge": Mcp::badge(server) });
                if let (true, Some(error)) = (on && server.status == "failed", &server.error) {
                    item["notice"] = json!({ "words": error, "tone": "warning" });
                }
                if let Some(flow) = flow {
                    item["flow"] = match flow.state {
                        "waiting" => {
                            let mut drawn = json!({ "id": "att_demo", "state": "waiting", "title": format!("Signing in to {}", server.name), "line": "Finish signing in on its page in your browser.", "opens": SIGN_IN_PAGE,
                                "actions": [{ "id": "open-sign-in", "title": "Open Sign-In Page", "opens": SIGN_IN_PAGE }, { "id": "cancel", "title": "Cancel" }] });
                            if !here {
                                drawn["actions"].as_array_mut().unwrap().insert(1, json!({ "id": "finish", "title": "Finish" }));
                                drawn["code"] = json!({ "title": "Or paste the address the page ended on", "placeholder": "http://127.0.0.1:19876/mcp/oauth/callback?code=…", "action": "finish" });
                            }
                            drawn
                        }
                        "done" => json!({ "id": "att_demo", "state": "done", "title": format!("Signing in to {}", server.name), "line": flow.line, "actions": [{ "id": "dismiss", "title": "OK" }] }),
                        _ => json!({ "id": "att_demo", "state": "failed", "title": format!("Signing in to {}", server.name), "line": flow.line, "actions": [{ "id": "sign-in", "title": "Try Again" }, { "id": "dismiss", "title": "OK" }] }),
                    };
                }
                row(item)
            })
            .collect();
        let list = row(json!({ "id": "servers", "title": "Servers", "kind": "list", "items": items, "empty": "None yet.", "add": { "id": "add", "title": "Add Server…", "form": self.form(None, &json!({}), place) } }));
        json!({ "id": "mcp", "title": "MCP Servers", "groups": [
            { "id": "servers", "title": "Servers", "note": format!("A server on {place} runs a command; one on the web is a URL."), "rows": [list] },
            { "id": "popular-open", "title": "No Account Needed", "note": "Popular servers, each from the company behind it, as listed in the official MCP Registry. One that needs an account asks you to sign in once it's added.", "rows": [
                row(json!({ "id": "popular-context7", "title": "Context7", "line": "Up-to-date docs for the libraries you use.", "kind": "actions", "actions": [{ "id": "add", "title": "Add" }] })),
                row(json!({ "id": "popular-deepwiki", "title": "DeepWiki", "line": "Ask about any public GitHub repository.", "kind": "status", "value": "Added" })),
            ] },
            { "id": "timeouts", "title": "Timeouts", "note": "For every server, unless one sets its own.", "rows": [
                row(json!({ "id": "timeout-startup", "title": "Time to start, in milliseconds", "kind": "number", "placeholder": "30000", "min": 1, "can": true })),
            ] },
        ] })
    }

    /// A form's values read, as the box reads them: each field's words, whether it can go.
    fn check(&self, name: Option<&str>, values: &Map<String, Value>, here: bool) -> Value {
        let place = if here { "this Mac" } else { "the computer" };
        let mut fields = Vec::new();
        let mut ready = true;
        if name.is_none() {
            let typed = text(&values.get("name").cloned().unwrap_or_default()).trim().to_owned();
            let id: String = typed.replace(' ', "-").chars().filter(|each| each.is_alphanumeric() || *each == '-' || *each == '_').collect();
            if id.is_empty() {
                ready = false;
                fields.push(json!({ "id": "name", "words": "A short name without spaces. The server's tools are named after it.", "tone": "quiet" }));
            } else if self.servers.iter().any(|server| server.name == id) {
                ready = false;
                fields.push(json!({ "id": "name", "words": format!("There's a server called {id} already."), "tone": "warning" }));
            } else if id != typed {
                fields.push(json!({ "id": "name", "words": format!("It'll be called {id}."), "tone": "quiet" }));
            }
        }
        if values.get("runs") == Some(&json!("remote")) {
            let url = text(values.get("url").unwrap_or(&Value::Null));
            if url.trim().is_empty() {
                ready = false;
                fields.push(json!({ "id": "url", "words": "Its address, like https://example.com/mcp.", "tone": "quiet" }));
            } else if url.contains(' ') || !url.contains('.') {
                ready = false;
                fields.push(json!({ "id": "url", "words": "That isn’t a web address. Give one like https://example.com/mcp.", "tone": "warning" }));
            }
        } else {
            let command = text(values.get("command").unwrap_or(&Value::Null));
            match command.split_whitespace().next() {
                None => {
                    ready = false;
                    fields.push(json!({ "id": "command", "words": "The command that starts it.", "tone": "quiet" }));
                }
                Some(program @ ("npx" | "uvx" | "docker" | "node")) => fields.push(json!({ "id": "command", "words": format!("{program} is on {place}."), "tone": "good" })),
                Some(program) => fields.push(json!({ "id": "command", "words": format!("{program} isn’t on {place}. Install it there first, or use a command it has."), "tone": "warning" })),
            }
        }
        json!({ "fields": fields, "ready": ready })
    }

    fn entry(values: &Map<String, Value>, before: &Value) -> Value {
        let object = |key: &str| -> Value {
            let list = values.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
            let kept: Map<String, Value> = list.iter().filter(|each| !text(&each["key"]).is_empty()).map(|each| (text(&each["key"]), each["value"].clone())).collect();
            Value::Object(kept)
        };
        let mut entry = before.as_object().cloned().unwrap_or_default();
        for key in ["command", "cwd", "environment", "url", "headers", "oauth"] {
            entry.remove(key);
        }
        if values.get("runs") == Some(&json!("remote")) {
            entry.insert("type".into(), json!("remote"));
            entry.insert("url".into(), values.get("url").cloned().unwrap_or_default());
            if values.get("oauth") == Some(&json!(false)) {
                entry.insert("oauth".into(), json!(false));
            }
            if object("headers").as_object().is_some_and(|headers| !headers.is_empty()) {
                entry.insert("headers".into(), object("headers"));
            }
        } else {
            entry.insert("type".into(), json!("local"));
            let words: Vec<Value> = text(values.get("command").unwrap_or(&Value::Null)).split_whitespace().map(|word| json!(word)).collect();
            entry.insert("command".into(), Value::Array(words));
            if object("environment").as_object().is_some_and(|environment| !environment.is_empty()) {
                entry.insert("environment".into(), object("environment"));
            }
        }
        entry.insert("codemode".into(), json!(values.get("codemode") == Some(&json!(true))));
        Value::Object(entry)
    }
}

/// Every row on the page, a list's items too.
fn find(page: &Value, id: &str) -> Option<Value> {
    page["groups"].as_array()?.iter().flat_map(|group| group["rows"].as_array().cloned().unwrap_or_default()).flat_map(|row| std::iter::once(row.clone()).chain(row["items"].as_array().cloned().unwrap_or_default())).find(|row| row["id"] == json!(id))
}

impl Settings {
    /// Its answer to `kuvo/settings/mcp…`.
    pub(super) fn mcp_answer(&mut self, method: &str, rest: &[&str], body: &Value, name: &str, here: bool) -> Option<(u16, Value)> {
        let ["mcp", tail @ ..] = rest else { return None };
        if !self.mcp.on {
            return None;
        }
        let page = self.mcp.page(here, name);
        Some(match (method, tail) {
            ("GET", []) | ("GET", ["events"]) => (200, page),
            ("PATCH", []) => {
                let Some(at) = body["row"].as_str().and_then(|id| find(&page, id)) else { return Some(refused(404, "That isn’t on this page now.")) };
                if at["revision"] != body["was"] {
                    return Some((409, json!({ "message": "It changed since you read it.", "page": page })));
                }
                let Some(server) = at["id"].as_str().and_then(|id| id.strip_prefix("server-")).and_then(|name| self.mcp.servers.iter_mut().find(|server| server.name == name)) else { return Some(refused(400, "That isn't something to change.")) };
                let Some(on) = body["value"].as_bool() else { return Some(refused(400, "Send true or false.")) };
                if let Some(entry) = server.entry.as_object_mut() {
                    if on { entry.remove("disabled"); } else { entry.insert("disabled".into(), json!(true)); }
                }
                (200, self.mcp.page(here, name))
            }
            ("POST", [row, action, rest @ ..]) => {
                let Some(at) = find(&page, row) else { return Some(refused(404, "That isn’t on this page now.")) };
                if at["revision"] != body["was"] {
                    return Some((409, json!({ "message": "It changed since you read it.", "page": page })));
                }
                let server = row.strip_prefix("server-").map(str::to_owned);
                let values = body["values"].as_object().cloned().unwrap_or_default();
                if rest == ["check"] {
                    return Some((200, self.mcp.check(if *action == "add" { None } else { server.as_deref() }, &values, here)));
                }
                match (*row, *action, server) {
                    ("servers", "add", _) => {
                        let checked = self.mcp.check(None, &values, here);
                        if checked["ready"] != json!(true) {
                            let first = checked["fields"].as_array().and_then(|fields| fields.iter().find(|each| each["tone"] == json!("warning"))).map(|each| text(&each["words"])).unwrap_or_else(|| "Fill in what it needs first.".into());
                            return Some((400, json!({ "message": first, "fields": checked["fields"] })));
                        }
                        self.mcp.sent.push(Value::Object(values.clone()));
                        let id: String = text(&values["name"]).trim().replace(' ', "-");
                        self.mcp.servers.push(Server { name: id, entry: Mcp::entry(&values, &json!({})), status: "pending".into(), error: None });
                        self.mcp.servers.sort_by(|a, b| a.name.cmp(&b.name));
                    }
                    (_, "edit", Some(name)) => {
                        self.mcp.sent.push(Value::Object(values.clone()));
                        if let Some(server) = self.mcp.servers.iter_mut().find(|server| server.name == name) {
                            server.entry = Mcp::entry(&values, &server.entry);
                        }
                    }
                    (_, "remove", Some(name)) => self.mcp.servers.retain(|server| server.name != name),
                    (_, "try-again", Some(name)) => {
                        if let Some(server) = self.mcp.servers.iter_mut().find(|server| server.name == name) {
                            server.status = "pending".into();
                        }
                    }
                    (_, "sign-in", Some(name)) => self.mcp.flow = Some(SignInFlow { server: name, state: "waiting", line: String::new() }),
                    (_, "finish", Some(name)) => {
                        if text(values.get("code").unwrap_or(&Value::Null)).is_empty() {
                            return Some(refused(400, "Paste the address the sign-in page ended on."));
                        }
                        self.mcp_signed_in(&name);
                    }
                    (_, "cancel" | "dismiss", Some(_)) => self.mcp.flow = None,
                    ("popular-context7", "add", _) => self.mcp.servers.push(Server { name: "context7".into(), entry: json!({ "type": "remote", "url": "https://mcp.context7.com/mcp", "codemode": false }), status: "pending".into(), error: None }),
                    (_, "open-sign-in", _) => return Some(refused(400, "That opens a page: open it in the app.")),
                    _ => return Some(refused(404, "That isn't something it does.")),
                }
                (200, self.mcp.page(here, name))
            }
            _ => refused(405, "Read it with GET, or change a row with PATCH."),
        })
    }

    /// A server's sign-in went through: done, and it's connected.
    pub fn mcp_signed_in(&mut self, name: &str) {
        self.mcp.flow = Some(SignInFlow { server: name.into(), state: "done", line: "Signed in. The agent can use its tools now.".into() });
        if let Some(server) = self.mcp.servers.iter_mut().find(|server| server.name == name) {
            server.status = "connected".into();
        }
    }

    /// A server's sign-in turned down on its page.
    pub fn mcp_sign_in_failed(&mut self, name: &str) {
        self.mcp.flow = Some(SignInFlow { server: name.into(), state: "failed", line: "The sign-in was turned down on its page. Try again to sign in.".into() });
    }
}
