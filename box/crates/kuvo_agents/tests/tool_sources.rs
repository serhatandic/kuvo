//! The computer's servers as its settings files have them, with your
//! sign-ins from OpenCode's database and a stand-in for OpenCode's API
//! (renewals only), and Kuvo Code's tools as an agent is given them. Each
//! case names the bridge's behaviour it ports (servers.ts, kuvotools.ts,
//! toolrelay.ts `agentToolsFor`).

use futures_util::{FutureExt, future::BoxFuture};
use http::Method;
use kuvo_agents::tool_relay::{
    AgentTools, EngineApi, ServerHost, ServerKind, SettingsServers, ToolPlaces, ToolRelay,
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// OpenCode's name for the `linear` server's sign-in (`mcp_` and the start of
/// the SHA-1 of its name, a NUL and its URL).
const LINEAR: &str = "mcp_0e309c5c6d08d88b";

#[derive(Default)]
struct Engine {
    running: Mutex<bool>,
    database: Mutex<Option<PathBuf>>,
    asked: Mutex<Vec<String>>,
}

impl EngineApi for Engine {
    fn call<'a>(&'a self, method: Method, path: &'a str, _: Option<Value>, _: Duration) -> BoxFuture<'a, Result<Value, String>> {
        async move {
            self.asked.lock().unwrap().push(format!("{method} {path}"));
            if !*self.running.lock().unwrap() {
                return Err("The engine isn't running.".into());
            }
            // Connecting it again renews the sign-in.
            if let Some(database) = &*self.database.lock().unwrap() {
                sign_in(database, "fresh", now_ms() + 3_600_000.0);
            }
            Ok(json!(true))
        }
        .boxed()
    }
}

fn now_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as f64
}

/// OpenCode's sign-in for `linear`, as it keeps it.
fn sign_in(database: &Path, access: &str, expires: f64) {
    let connection = rusqlite::Connection::open(database).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS credential (id text PRIMARY KEY, integration_id text, label text NOT NULL, value text NOT NULL, connector_id text, method_id text, active integer, time_created integer NOT NULL, time_updated integer NOT NULL); DELETE FROM credential;",
        )
        .unwrap();
    let value = json!({"type":"oauth","access":access,"expires":expires}).to_string();
    for (id, integration, active) in [("old", LINEAR, 0), ("other", "mcp_0000000000000000", 1), ("now", LINEAR, 1)] {
        connection
            .execute(
                "INSERT INTO credential VALUES (?1, ?2, 'linear', ?3, NULL, NULL, ?4, 1, 1)",
                rusqlite::params![id, integration, if active == 1 && id == "now" { value.clone() } else { json!({"type":"oauth","access":"not this"}).to_string() }, active],
            )
            .unwrap();
    }
}

/// A machine's home: this computer's settings as the box hands them to
/// OpenCode, a project with its own, and OpenCode's database.
fn machine() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("box")).unwrap();
    std::fs::write(
        home.path().join("box/opencode.json"),
        json!({"mcp":{"servers":{
            "files":{"type":"local","command":["npx","files"],"environment":{"KEY":"k1","N":1},"timeout":{"execution":5000}},
            "linear":{"type":"remote","url":"https://linear.example/mcp"},
            "kuvo-browser":{"type":"local","command":["kuvo"]},
            "off":{"type":"local","command":["x"],"enabled":false},
            "bad.name":{"type":"local","command":["x"]},
            "marketplace-left":{"type":"remote","url":"https://left.example"},
            "marketplace-kept":{"type":"remote","url":"https://kept.example","oauth":false},
        }}})
        .to_string(),
    )
    .unwrap();
    let folder = home.path().join("work/project");
    std::fs::create_dir_all(folder.join(".git")).unwrap();
    std::fs::write(
        folder.join("opencode.jsonc"),
        "{\n  // The folder's own.\n  \"mcp\": {\"servers\": {\"files\": {\"type\": \"local\", \"command\": [\"files-here\"], \"cwd\": \"relative\"},},},\n}",
    )
    .unwrap();
    std::fs::write(
        folder.join(".kuvo-marketplace.json"),
        json!({"request":"r","agent":"a","version":"1","apps":["kept"],"skills":[]}).to_string(),
    )
    .unwrap();
    let data = home.path().join("data/opencode");
    std::fs::create_dir_all(&data).unwrap();
    (home, folder, data.join("opencode.db"))
}

#[tokio::test]
async fn the_computers_servers_for_a_folder_reach_agents_while_opencode_isnt_running() {
    let (home, folder, database) = machine();
    sign_in(&database, "kept", now_ms() + 3_600_000.0);
    let engine = Arc::new(Engine::default());
    let host = SettingsServers::new(home.path().to_owned(), engine.clone(), None);
    let found = host.servers(&folder, false).await;
    assert_eq!(found.key, "files linear marketplace-kept");
    let names = found.servers.iter().map(|server| server.name.as_str()).collect::<Vec<_>>();
    assert_eq!(names, ["files", "linear", "marketplace-kept"]);
    // The folder's settings in place of the computer's.
    let ServerKind::Local { command, cwd, env } = &found.servers[0].kind else { panic!() };
    assert_eq!(command, &["files-here"].map(std::ffi::OsString::from));
    assert_eq!((cwd, env.len()), (&None, 0));
    assert_eq!(found.servers[0].timeout, Duration::from_secs(300));
    // Your sign-in, as OpenCode keeps it; a server that signs in no way has none.
    let ServerKind::Remote { headers, expires, .. } = &found.servers[1].kind else { panic!() };
    assert_eq!(headers, &[("Authorization".to_owned(), "Bearer kept".to_owned())]);
    assert!(expires.is_some_and(|at| at > SystemTime::now() + Duration::from_secs(3000)));
    let ServerKind::Remote { headers, .. } = &found.servers[2].kind else { panic!() };
    assert!(headers.is_empty());
    // OpenCode isn't asked, nor started.
    assert!(engine.asked.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_sign_in_running_out_is_renewed_while_opencode_runs() {
    let (home, folder, database) = machine();
    sign_in(&database, "stale", now_ms() + 10_000.0);
    let engine = Arc::new(Engine::default());
    *engine.database.lock().unwrap() = Some(database.clone());
    *engine.running.lock().unwrap() = true;
    let host = SettingsServers::new(home.path().to_owned(), engine.clone(), None);
    let found = host.servers(&folder, true).await;
    let ServerKind::Remote { headers, expires, .. } = &found.servers[1].kind else { panic!() };
    assert_eq!(headers, &[("Authorization".to_owned(), "Bearer fresh".to_owned())]);
    assert!(expires.is_some_and(|at| at > SystemTime::now() + Duration::from_secs(3000)));
    assert!(engine.asked.lock().unwrap().iter().any(|asked| asked.starts_with("POST /api/experimental/mcp/linear/connect?location%5Bdirectory%5D=")));
    // Read again only every few seconds; the last known while a file can't be read.
    let asked = engine.asked.lock().unwrap().len();
    host.servers(&folder, false).await;
    assert_eq!(engine.asked.lock().unwrap().len(), asked);
    std::fs::write(folder.join("opencode.jsonc"), "{ not json").unwrap();
    assert_eq!(host.servers(&folder, true).await.key, "files linear marketplace-kept");
}

#[tokio::test]
async fn opencodes_variables_in_the_settings_are_filled_in() {
    let (home, folder, _) = machine();
    std::fs::write(folder.join("token.txt"), " from a file\n").unwrap();
    std::fs::write(
        folder.join("opencode.json"),
        "{\"mcp\":{\"servers\":{\"docs\":{\"type\":\"remote\",\"url\":\"https://docs.example/mcp\",\"oauth\":false,\"headers\":{\"Authorization\":\"Bearer {file:token.txt}\",\"X-Unset\":\"[{env:KUVO_TEST_SURELY_UNSET}]\"}}}}}",
    )
    .unwrap();
    let host = SettingsServers::new(home.path().to_owned(), Arc::new(Engine::default()), None);
    let found = host.servers(&folder, false).await;
    let docs = found.servers.iter().find(|server| server.name == "docs").expect("the folder's server");
    let ServerKind::Remote { headers, .. } = &docs.kind else { panic!() };
    assert_eq!(
        headers,
        &[("Authorization".to_owned(), "Bearer from a file".to_owned()), ("X-Unset".to_owned(), "[]".to_owned())]
    );
}

#[tokio::test]
async fn a_server_that_turns_down_its_sign_in_is_connected_again_for_the_folder_and_the_engine() {
    let engine = Arc::new(Engine::default());
    *engine.running.lock().unwrap() = true;
    let host = SettingsServers::new(PathBuf::from("/nowhere"), engine.clone(), None);
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
