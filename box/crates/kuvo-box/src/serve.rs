use std::{fs, io, path::Path, sync::Arc};

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
    let name = hostname::get()?.to_string_lossy().into_owned();
    let mac = is_mac(home)?;
    let relay = std::env::var("KUVO_TEST_RELAY").ok().or_else(|| {
        let account = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
        (home == Path::new(&account).join(".kuvo-code"))
            .then(|| "wss://kuvolink.porigam.com.tr".into())
    });
    let setup = kuvo_setup::SetupRuntime::start(home, name.clone(), mac, relay).await?;
    let info = info(home, &name, mac)?;
    let mut routes = Registry::new();
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
    setup.handle().register(&mut routes);
    let dispatcher = routes.build(LineTiming::default());
    println!("{}", json!({"url": format!("http://127.0.0.1:{port}")}));
    resident
        .serve(dispatcher, Arc::new(setup.handle()), super::shutdown())
        .await?;
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
    let features = ["info", "line"];
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
