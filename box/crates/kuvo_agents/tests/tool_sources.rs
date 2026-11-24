//! The computer's servers as the engine has them, through a stand-in for
//! the engine's API, and Kuvo Code's tools as an agent is given them. Each
//! case names the bridge's behaviour it ports (servers.ts, kuvotools.ts,
//! toolrelay.ts `agentToolsFor`).

use futures_util::{FutureExt, future::BoxFuture};
use http::Method;
use kuvo_agents::tool_relay::{
    AgentTools, EngineApi, EngineServers, ServerHost, ServerKind, ToolPlaces, ToolRelay,
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Default)]
struct Engine {
    running: Mutex<bool>,
    config: Mutex<Value>,
    credentials: Mutex<Value>,
    asked: Mutex<Vec<String>>,
}

impl EngineApi for Engine {
    fn call<'a>(&'a self, method: Method, path: &'a str, _: Option<Value>, _: Duration) -> BoxFuture<'a, Result<Value, String>> {
        async move {
            self.asked.lock().unwrap().push(format!("{method} {path}"));
            if !*self.running.lock().unwrap() {
                return Err("The engine isn't running.".into());
            }
            let route = path.split('?').next().unwrap_or_default();
            Ok(match (method.as_str(), route) {
                ("GET", "/api/config") => self.config.lock().unwrap().clone(),
                ("GET", "/api/mcp") => json!({"data":[{"name":"linear","integrationID":"int_linear"}]}),
                ("GET", "/api/credential") => self.credentials.lock().unwrap().clone(),
                ("POST", _) => {
                    // Connecting it again renews the sign-in.
                    let later = now_ms() + 3_600_000.0;
                    *self.credentials.lock().unwrap() = credential("fresh", later);
                    json!(true)
                }
                _ => Value::Null,
            })
        }
        .boxed()
    }
}

fn now_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as f64
}

fn credential(access: &str, expires: f64) -> Value {
    json!({"data":[{"integrationID":"int_linear","active":true,"value":{"type":"oauth","access":access,"expires":expires}}]})
}

fn settings() -> Value {
    json!({"data":[
        {"info":{"mcp":{"servers":{
            "files":{"type":"local","command":["npx","files"],"environment":{"KEY":"k1","N":1},"timeout":{"execution":5000}},
            "linear":{"type":"remote","url":"https://linear.example/mcp"},
            "kuvo-browser":{"type":"local","command":["kuvo"]},
            "off":{"type":"local","command":["x"],"enabled":false},
            "bad.name":{"type":"local","command":["x"]},
            "marketplace-left":{"type":"remote","url":"https://left.example"},
            "marketplace-kept":{"type":"remote","url":"https://kept.example","oauth":false},
        }}}},
        {"info":{"mcp":{"servers":{
            "files":{"type":"local","command":["files-here"],"cwd":"relative"},
        }}}},
    ]})
}

#[tokio::test]
async fn the_engines_servers_for_a_folder_with_your_sign_ins_renewed_as_they_run_out() {
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(
        folder.path().join(".kuvo-marketplace.json"),
        json!({"request":"r","agent":"a","version":"1","apps":["kept"],"skills":[]}).to_string(),
    )
    .unwrap();
    let engine = Arc::new(Engine::default());
    *engine.config.lock().unwrap() = settings();
    *engine.credentials.lock().unwrap() = credential("stale", now_ms() + 10_000.0);
    let host = EngineServers::new(engine.clone(), None);
    // The engine not running: none, and it isn't started for it.
    assert!(host.servers(folder.path(), false).await.servers.is_empty());
    *engine.running.lock().unwrap() = true;
    let found = host.servers(folder.path(), true).await;
    assert_eq!(found.key, "files linear marketplace-kept");
    let names = found.servers.iter().map(|server| server.name.as_str()).collect::<Vec<_>>();
    assert_eq!(names, ["files", "linear", "marketplace-kept"]);
    // The folder's settings in place of the computer's.
    let ServerKind::Local { command, cwd, env } = &found.servers[0].kind else { panic!() };
    assert_eq!(command, &["files-here"].map(std::ffi::OsString::from));
    assert_eq!((cwd, env.len()), (&None, 0));
    assert_eq!(found.servers[0].timeout, Duration::from_secs(300));
    // Its sign-in, renewed first as it was running out.
    let ServerKind::Remote { headers, expires, .. } = &found.servers[1].kind else { panic!() };
    assert_eq!(headers, &[("Authorization".to_owned(), "Bearer fresh".to_owned())]);
    assert!(expires.is_some_and(|at| at > SystemTime::now() + Duration::from_secs(3000)));
    assert!(engine.asked.lock().unwrap().iter().any(|asked| asked.starts_with("POST /api/experimental/mcp/linear/connect?location%5Bdirectory%5D=")));
    // Read again only every few seconds; the last known while the engine can't say.
    let asked = engine.asked.lock().unwrap().len();
    host.servers(folder.path(), false).await;
    assert_eq!(engine.asked.lock().unwrap().len(), asked);
    *engine.running.lock().unwrap() = false;
    assert_eq!(host.servers(folder.path(), true).await.key, "files linear marketplace-kept");
}

#[tokio::test]
async fn a_server_that_turns_down_its_sign_in_is_connected_again_for_the_folder_and_the_engine() {
    let engine = Arc::new(Engine::default());
    *engine.running.lock().unwrap() = true;
    let host = EngineServers::new(engine.clone(), None);
    host.reconnect(&PathBuf::from("/work/a b"), "linear").await;
    assert_eq!(
        *engine.asked.lock().unwrap(),
        [
            "POST /api/experimental/mcp/linear/connect?location%5Bdirectory%5D=%2Fwork%2Fa%20b",
            "POST /api/experimental/mcp/linear/connect",
        ]
    );
}

#[tokio::test]
async fn kuvo_codes_tools_as_an_agent_is_given_them() {
    let places = Arc::new(ToolPlaces::default());
    let relay = ToolRelay::new(None, places.clone()).unwrap();
    assert!(relay.agent_tools("ses_one", 1, false).await.unwrap().is_none());
    // On a computer: the program its engine's settings name.
    places.set_program_settings(Some(
        &json!({"mcp":{"servers":{"kuvo-browser":{"command":["/opt/kuvo","mcp"],"environment":{"A":"1"},"timeout":{"execution":5000}}}}}).to_string(),
    ));
    let Some(AgentTools::Stdio { command, args, env, timeout, .. }) = relay.agent_tools("ses_one", 1, false).await.unwrap() else {
        panic!("the program");
    };
    assert_eq!((command, args), (PathBuf::from("/opt/kuvo"), vec!["mcp".into()]));
    assert_eq!(env.get(std::ffi::OsStr::new("KUVO_SESSION")).unwrap(), "ses_one");
    assert_eq!(env.get(std::ffi::OsStr::new("A")).unwrap(), "1");
    assert_eq!(timeout, Duration::from_secs(5));
    // In the sandbox: the relay, over HTTP, with the conversation's key.
    let Some(AgentTools::Http { url, headers, .. }) = relay.agent_tools("ses_one", 1, true).await.unwrap() else {
        panic!("the relay");
    };
    assert!(url.starts_with("http://127.0.0.1:") && url.ends_with("/mcp"), "{url}");
    assert!(headers[0].1.starts_with("Bearer "));
    // Where the box keeps agents through its updates: the forwarder, which waits for the relay.
    relay.set_forwarder(Some(PathBuf::from("/opt/kuvo-box")));
    let Some(AgentTools::Stdio { command, args, env, timeout, .. }) = relay.agent_tools("ses_one", 1, true).await.unwrap() else {
        panic!("the forwarder");
    };
    assert_eq!((command, args), (PathBuf::from("/opt/kuvo-box"), vec!["mcp".into()]));
    assert_eq!(env.get(std::ffi::OsStr::new("KUVO_TOOL_RELAY")).unwrap().to_str().unwrap(), url);
    assert_eq!(env.get(std::ffi::OsStr::new("KUVO_TOOL_TIMEOUT")).unwrap(), "5000");
    assert_eq!(timeout, Duration::from_secs(85));
    relay.shutdown().await;
}
