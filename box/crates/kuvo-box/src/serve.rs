use std::{fs, io, path::Path, sync::Arc, time::UNIX_EPOCH};

use kuvo_engine::{Engine, State, Timing};
use kuvo_peers::relay::{Relay, SetupLink};
use kuvo_protocol::{
    gate::compatibility,
    json::{Null, Nullable},
    routes::{get_kuvo_info as info, types::GetKuvoInfoStatus200MacEngineState3 as EngineState},
};
use kuvo_server::{Access, LineTiming, Refusal, Registry, Resident, RouteReply};
use serde_json::json;

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub async fn run(home: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let Some(resident) = Resident::bind(home).await? else {
        return Ok(());
    };
    let generation = resident.record().generation;
    let port = resident.record().port;
    let engine = Engine::new(
        home.to_owned(),
        std::env::vars_os().collect(),
        Timing::default(),
    )?;
    let name = hostname::get()?.to_string_lossy().into_owned();
    let mac = is_mac(home)?;
    let relay = std::env::var("KUVO_TEST_RELAY").ok().or_else(|| {
        let account = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
        (home == Path::new(&account).join(".kuvo-code"))
            .then(|| "wss://kuvolink.porigam.com.tr".into())
    });
    let prepared = kuvo_setup::SetupRuntime::prepare(home, name.clone(), mac, relay).await?;
    let mut relay_feed = None;
    let network = prepared.relay_config()?.map(|config| {
        let mut relay = Relay::start(config);
        let (events, receive) = tokio::sync::mpsc::channel(64);
        let sender = relay.sender.clone();
        relay_feed = Some(tokio::spawn(async move {
            while let Some(event) = relay.events.recv().await {
                if let Some(event) = event.for_setup()
                    && events.send(event).await.is_err()
                {
                    return;
                }
            }
        }));
        SetupLink {
            sender,
            events: receive,
        }
    });
    let setup = prepared.start(network)?;
    let info = info(home, &name, mac)?;
    let mut routes = Registry::new();
    engine.register(&mut routes);
    setup.handle().register(&mut routes);
    routes.register::<info::Spec, _, _>(Access::Any, move |_| {
        let engine = engine.clone();
        let info = info.clone();
        async move {
            RouteReply::json_with::<200>(
                engine_info(info, engine.state().await),
                [
                    ("serverGeneration", json!(generation)),
                    ("serverBuild", json!(VERSION)),
                ],
            )
            .map_err(|error| {
                Refusal::new(http::StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
            })
        }
    });
    let dispatcher = routes.build(LineTiming::default());
    println!("{}", json!({"url": format!("http://127.0.0.1:{port}")}));
    let result = resident
        .serve(dispatcher, Arc::new(setup.handle()), super::shutdown())
        .await;
    if let Some(task) = relay_feed {
        task.abort();
    }
    result?;
    Ok(())
}

fn is_mac(home: &Path) -> io::Result<bool> {
    match fs::read_to_string(home.join("box/kind")) {
        Ok(kind) => Ok(kind.trim() == "mac"),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn info(
    home: &Path,
    name: &str,
    is_mac: bool,
) -> Result<info::Status200, Box<dyn std::error::Error>> {
    let chats = home.join("chats");
    fs::create_dir_all(&chats)?;
    // Advertise the implemented routes while the desktop remains behind its update gate.
    let mut features = vec!["info", "line", "engine.restart"];
    if !is_mac {
        features.push("engine.start");
    }
    let mut answer = json!({
        "kind": if is_mac { "mac" } else { "machine" },
        "name": name,
        "features": features,
        "compatibility": compatibility(&features),
        "agents": false,
        "started": null,
        "engineState": "idle",
        "chats": chats.to_string_lossy(),
    });
    if is_mac {
        answer["app"] = json!("");
        answer["engine"] = json!("");
        answer["integrations"] = json!({});
    } else {
        let resident = json!({"running": VERSION, "current": VERSION, "behind": false});
        answer["bridge"] = json!(VERSION);
        answer["engine"] = json!(null);
        answer["running"] = json!(false);
        answer["stopped"] = json!(false);
        answer["pieces"] = json!({
            "link": resident,
            "agents": {"running": null, "current": VERSION, "behind": false, "renewal": null, "waits": null},
            "engine": {"running": null, "current": kuvo_engine::pinned_version(), "behind": false},
        });
    }
    Ok(serde_json::from_value(answer)?)
}

fn engine_info(mut info: info::Status200, state: State) -> info::Status200 {
    let (state, version, started) = match state {
        State::Idle => (EngineState::Idle, None, None),
        State::Starting => (EngineState::Starting, None, None),
        State::Running { version, started } => (EngineState::Running, version, started),
        State::Stopped => (EngineState::Stopped, None, None),
    };
    let started = started
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|time| serde_json::Number::from_f64(time.as_secs_f64()))
        .map_or(Nullable::Null(Null), Nullable::Value);
    match &mut info {
        info::Status200::Mac(mac) => {
            mac.engine_state = Some(state);
            mac.engine = version.unwrap_or_default();
            mac.started = started;
        }
        info::Status200::Machine(machine) => {
            machine.engine_state = Some(state);
            machine.running = state == EngineState::Running;
            machine.stopped = state == EngineState::Stopped;
            machine.started = started;
            machine.engine = version.map_or(Nullable::Null(Null), Nullable::Value);
            if let Some(pieces) = &mut machine.pieces {
                pieces.engine.running = machine.engine.clone();
            }
        }
    }
    info
}
