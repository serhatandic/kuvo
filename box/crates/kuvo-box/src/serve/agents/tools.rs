//! Kuvo Code's tools and the computer's MCP servers for the coding agents,
//! through the box's tools relay: the servers as the engine has them for a
//! conversation's folder, Kuvo Code's own as the engine's settings start
//! them. What an agent is given of them is its adapter's to ask
//! (`ToolRelay::agent_tools` and `relayed_servers`).

use http::Method;
use kuvo_agents::{
    runtime::Runtime,
    tool_relay::{EngineApi, EngineServers, ToolPlaces, ToolRelay},
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

/// The resident's relay, its address and keys kept in its run folder. On a
/// computer, Kuvo Code's tools are the program the engine's settings name,
/// and agents reach them through the box's forwarder, which waits for the
/// relay through an update. (A command's server runs as it is until the
/// sandbox has a launcher for it.)
pub(in crate::serve) fn relay(home: &Path, machine: bool, engine: Engine) -> io::Result<ToolRelay> {
    let places = Arc::new(ToolPlaces::default());
    if machine {
        places.set_program_settings(std::env::var("OPENCODE_CONFIG_CONTENT").ok().as_deref());
    }
    let host = Arc::new(EngineServers::new(Arc::new(EnginePort(engine)), None));
    let relay = ToolRelay::with_host(Some(home.join("run/agents/relay.json")), places, host)?;
    if machine {
        relay.set_forwarder(std::env::current_exe().ok());
    }
    Ok(relay)
}

/// On a computer, as the resident starts: the relay at once where it was,
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
