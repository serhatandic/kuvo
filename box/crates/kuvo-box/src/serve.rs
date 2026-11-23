use std::{fs, io, path::Path, sync::Arc};

use kuvo_peers::{
    devices::Devices,
    relay::Relay,
    session::ComputerKind,
    setup_lists::{ResidentPeers, system_clock},
};
use kuvo_protocol::{gate::compatibility, routes::get_kuvo_info as info};
use kuvo_server::{Access, LineTiming, Refusal, Registry, Resident, RouteReply};
use serde_json::json;

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub async fn run(home: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let Some(resident) = Resident::bind(home).await? else {
        return Ok(());
    };
    let generation = resident.record().generation;
    let port = resident.record().port;
    let kind = if fs::read_to_string(home.join("box/kind")).is_ok_and(|kind| kind.trim() == "mac") {
        ComputerKind::Mac
    } else {
        ComputerKind::Machine
    };
    let bootstrap =
        Devices::load(home, hostname::get()?.to_string_lossy().into_owned(), kind).await?;
    let info = info(
        home,
        &bootstrap.roster.name,
        bootstrap.devices.permits_local_pairing(),
    )?;
    let peers = ResidentPeers::start(
        home,
        bootstrap.roster,
        bootstrap.relay.map(Relay::start),
        Some(bootstrap.devices.clone()),
        kind == ComputerKind::Mac,
        Arc::new(system_clock),
    )
    .await?;
    let mut routes = Registry::new();
    kuvo_peers::routes::register(&mut routes, peers.reader(), bootstrap.devices.clone());
    routes.register::<info::Spec, _, _>(Access::Any, move |_| {
        let answer = RouteReply::json_with::<200>(
            info.clone(),
            [
                ("serverGeneration", json!(generation)),
                ("serverBuild", json!(VERSION)),
            ],
        )
        .map_err(|error| Refusal::new(http::StatusCode::INTERNAL_SERVER_ERROR, error.to_string()));
        std::future::ready(answer)
    });
    let dispatcher = routes.build(LineTiming::default());
    peers.dispatch_with(dispatcher.clone()).await;
    println!("{}", json!({"url": format!("http://127.0.0.1:{port}")}));
    resident
        .serve(dispatcher, Arc::new(bootstrap.devices), super::shutdown())
        .await?;
    Ok(())
}

fn info(
    home: &Path,
    name: &str,
    permits_pairing: bool,
) -> Result<info::Status200, Box<dyn std::error::Error>> {
    let chats = home.join("chats");
    fs::create_dir_all(&chats)?;
    let is_mac = match fs::read_to_string(home.join("box/kind")) {
        Ok(kind) => kind.trim() == "mac",
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    // Advertise the implemented routes while the desktop remains behind its update gate.
    let mut features: Vec<_> = ["info", "line"]
        .into_iter()
        .chain(kuvo_peers::routes::FEATURES.iter().copied())
        .collect();
    if permits_pairing {
        features.push("devices.pair");
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
            "engine": {"running": null, "current": null, "behind": false},
        });
    }
    Ok(serde_json::from_value(answer)?)
}
