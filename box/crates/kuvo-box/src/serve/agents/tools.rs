//! Kuvo Code's tools and the computer's MCP servers for the coding agents,
//! through the box's tools relay: the servers in the settings for a
//! conversation's folder (OpenCode only renews their sign-ins while it runs),
//! Kuvo Code's own as the engine's settings start them. What an agent is given of them is its adapter's to ask
//! (`ToolRelay::agent_tools` and `relayed_servers`).

use http::Method;
use kuvo_agents::{
    runtime::Runtime,
    tool_relay::{EngineApi, SettingsServers, ToolPlaces, ToolRelay, ToolSource},
};
use kuvo_engine::Engine;
use serde_json::Value;
use std::{future::Future, io, path::Path, pin::Pin, sync::Arc, time::Duration};

struct EnginePort(Engine);

impl EngineApi for EnginePort {
    fn call<'a>(
        &'a self,
        method: Method,
        path: &'a str,
        body: Option<Value>,
        wait: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>> {
        Box::pin(async move { self.0.call_running(method, path, body.as_ref(), wait).await })
    }
}

/// The resident's relay, its address and keys kept in its run folder. Every
/// resident runs in a box home of its own, a Mac's too: Kuvo Code's tools
/// are the program the engine's settings name, and agents reach them through
/// the box's forwarder, which waits for the relay through an update. (A
/// command's server runs as it is until the sandbox has a launcher for it.)
pub(in crate::serve) async fn relay(
    home: &Path,
    engine: Engine,
    mac: bool,
) -> io::Result<(
    ToolRelay,
    Option<Arc<kuvo_services::browser::tools::ToolServer>>,
)> {
    let places = Arc::new(ToolPlaces::default());
    places.set_program_settings(std::env::var("OPENCODE_CONFIG_CONTENT").ok().as_deref());
    // An explicitly named different program remains supported (including the
    // agent runtime fixtures). The released browser program is replaced here.
    let native = places.tools("engine").is_none_or(|program| {
        std::iter::once(program.command.as_os_str())
            .chain(program.args.iter().map(|arg| arg.as_os_str()))
            .any(|part| {
                Path::new(part)
                    .file_name()
                    .is_some_and(|name| name == "kuvo-browser.js")
            })
    });
    let host = Arc::new(SettingsServers::new(
        home.to_owned(),
        Arc::new(EnginePort(engine.clone())),
        None,
    ));
    let relay = ToolRelay::with_host(Some(home.join("run/agents/relay.json")), places, host)?;
    relay.set_forwarder(std::env::current_exe().ok());
    let native = if native {
        let server = Arc::new(kuvo_services::browser::tools::ToolServer::new(
            home.to_owned(),
            mac,
        )?);
        relay.set_native(server.clone());
        if let Some(connection) = relay.engine_tools().await? {
            let program = std::env::current_exe()?;
            engine.set_own_tool_server(serde_json::json!({
                "type":"local", "command":[program,"mcp"], "codemode":false,
                "environment":{
                    "KUVO_TOOL_RELAY":connection.url,
                    "KUVO_TOOL_RELAY_KEY":connection.authorization,
                    "KUVO_TOOL_TIMEOUT":connection.timeout.as_millis().to_string(),
                }, "timeout":{"execution":connection.timeout.as_millis()}
            }));
        }
        Some(server)
    } else {
        None
    };
    Ok((relay, native))
}

/// As the resident starts: the relay at once where it was,
/// with only the keys of the conversations whose agents it took up, so
/// they reach their tools as before.
pub(in crate::serve) async fn take_up(relay: &ToolRelay, runtime: &Runtime) {
    let held = match runtime.held_sessions().await {
        Ok(held) => held,
        Err(error) => return eprintln!("Kuvo could not read which agents it holds: {error}"),
    };
    match relay.port().await {
        Ok(_) => relay.keep_keys(&held),
        Err(error) => eprintln!("Kuvo's tools relay could not listen: {error}"),
    }
}
