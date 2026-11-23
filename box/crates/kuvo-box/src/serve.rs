use std::{fs, io, path::Path, sync::Arc, time::UNIX_EPOCH};

use kuvo_engine::{Engine, State, Timing};
use kuvo_peers::{
    devices::Devices,
    lists::Local,
    relay::Relay,
    session::ComputerKind,
    setup_lists::{ResidentPeers, system_clock},
};
use kuvo_protocol::{
    gate::compatibility,
    json::{Null, Nullable},
    routes::{get_kuvo_info as info, types::GetKuvoInfoStatus200MacEngineState3 as EngineState},
};
use kuvo_server::{
    Access, LineTiming, LocalOrigin, Refusal, Registry, Resident, RouteReply, Transport,
};
use serde_json::json;

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub async fn run(
    home: &Path,
    mut vault_launch: kuvo_services::vault::Launch,
    helper_ended: Option<tokio::sync::oneshot::Receiver<()>>,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(resident) = Resident::bind_with_password(home, vault_launch.take_password()).await?
    else {
        return Ok(());
    };
    let generation = resident.record().generation;
    let port = resident.record().port;
    let engine = Engine::new(
        home.to_owned(),
        std::env::vars_os().collect(),
        Timing::default(),
    )?;
    let kind = match fs::read_to_string(home.join("box/kind")) {
        Ok(kind) if kind.trim() == "mac" => ComputerKind::Mac,
        Ok(_) => ComputerKind::Machine,
        Err(error) if error.kind() == io::ErrorKind::NotFound => ComputerKind::Machine,
        Err(error) => return Err(error.into()),
    };
    let name = hostname::get()?.to_string_lossy().into_owned();
    let relay = std::env::var("KUVO_TEST_RELAY").ok().or_else(|| {
        let account = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
        (home == Path::new(&account).join(".kuvo-code"))
            .then(|| "wss://kuvolink.porigam.com.tr".into())
    });
    let prepared =
        kuvo_setup::SetupRuntime::prepare(home, name.clone(), kind == ComputerKind::Mac, relay)
            .await?;
    #[cfg(target_os = "macos")]
    let authenticated_helper = prepared.has_mac_identity();
    let bootstrap = Devices::load(home, name, kind).await?;
    let accepted_roster = prepared.peer_roster();
    let managed = accepted_roster.is_some();
    let roster = accepted_roster.unwrap_or(bootstrap.roster);
    let relay = prepared
        .relay_config()?
        .or(bootstrap.relay)
        .map(Relay::start);
    prepared.record_build(VERSION)?;
    let setup = prepared.start(relay.as_ref().map(Relay::handle))?;
    let devices = if managed {
        bootstrap.devices.with_managed(Arc::new(setup.handle()))
    } else {
        bootstrap.devices
    };
    let info = info(home, &roster.name, kind, devices.permits_local_pairing())?;
    let vault_machine = (kind != ComputerKind::Mac).then(|| roster.name.clone());
    let mut peers = ResidentPeers::start(
        home,
        roster,
        relay,
        Some(devices.clone()),
        kind == ComputerKind::Mac,
        Arc::new(system_clock),
    )
    .await?;
    let authority = setup.handle();
    let ownership = kuvo_peers::ownership::Handle::new(
        peers.reader(),
        peers.relay_handle(),
        devices.clone(),
        move || authority.ownership_state(),
    );
    peers.attach_moves(engine.clone(), ownership).await?;
    let peers = Arc::new(peers);
    peers.follow_setup_roster(setup.handle().rosters()).await;
    let mut routes = Registry::new();
    if kind == ComputerKind::Machine {
        let setup = setup.handle();
        routes.observe_activity(move |caller| {
            let transport = caller.transport();
            let setup = setup.clone();
            async move {
                if !matches!(
                    transport,
                    Transport::Http {
                        origin: LocalOrigin::App,
                        ..
                    }
                ) {
                    setup.phone_activity().await;
                }
            }
        });
    }
    let pages = peers.reader();
    let sessions = engine.register_with_merged_pages(&mut routes, move |project, shown, step| {
        kuvo_peers::routes::merged_project_page(&pages, project, shown, step)
    });
    setup.handle().register(&mut routes);
    setup.handle().register_ssh(&mut routes, home);
    let account = kuvo_os::platform::Account::current()?.home_folder(
        kuvo_os::platform::Platform::current(),
        &std::env::vars_os().collect(),
    )?;
    kuvo_services::files::register(
        &mut routes,
        kuvo_services::files::Places {
            home: account.clone(),
            box_home: home.to_owned(),
            platform: kuvo_os::platform::Platform::current(),
        },
    );
    let (backup_mail, backup_receiver) = if let Some(relay) = peers.relay_handle()
        && let Some((mail, receiver)) = kuvo_setup::backup::relay::mail::start(
            home.to_owned(),
            account.clone(),
            setup.handle(),
            peers.reader(),
            relay,
        ) {
        peers.advertise_backup_receiver().await;
        (Some(mail), Some(receiver))
    } else {
        (None, None)
    };
    let vault = kuvo_services::vault::Resident::new(
        home,
        kind == ComputerKind::Mac,
        vault_launch,
        account.clone(),
        &kuvo_os::platform::Account::current()?.home,
        vault_machine,
    );
    vault.register(&mut routes);
    let daily_backups = kuvo_setup::backup::routes::register(
        &mut routes,
        kuvo_store::home::Home::new(home.to_owned())?,
        account,
        setup.handle(),
        backup_mail,
        Some(vault.clone()),
    );
    kuvo_peers::routes::register(&mut routes, peers.reader(), devices.clone());
    peers.register_moves(&mut routes, engine.clone());
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
    peers.dispatch_with(dispatcher.clone()).await;
    let vault_holder =
        match kuvo_services::vault::Holder::start(home, vault, dispatcher.clone()).await {
            Ok(holder) => Some(holder),
            Err(error) => {
                eprintln!("The vault's local socket couldn't listen: {error}");
                None
            }
        };
    println!("{}", json!({"url": format!("http://127.0.0.1:{port}")}));
    #[cfg(target_os = "macos")]
    if authenticated_helper {
        let home = home.to_owned();
        tokio::task::spawn_blocking(move || {
            let settlement = || -> Result<(), Box<dyn std::error::Error>> {
                let account = kuvo_os::platform::Account::current()?.home;
                let home = kuvo_store::home::Home::new(home)?;
                kuvo_move::mac::settle_started(&home, &account)?;
                Ok(())
            };
            if let Err(error) = settlement() {
                eprintln!("kuvo-box: settling the Mac app move: {error}");
            }
        });
    }
    let session_source = tokio::spawn(publish_local_sessions(home.join("run"), peers, sessions));
    let finishing = setup.handle();
    let result = resident
        .serve(dispatcher, Arc::new(devices), async {
            let helper = async {
                if let Some(ended) = helper_ended {
                    let _ = ended.await;
                } else {
                    std::future::pending::<()>().await;
                }
            };
            tokio::select! {
                () = shutdown => {},
                () = finishing.wait_for_local_wipe() => {},
                () = helper => {},
            }
        })
        .await;
    session_source.abort();
    if let Some(holder) = vault_holder {
        holder.shutdown().await;
    }
    let _ = session_source.await;
    drop(daily_backups);
    drop(backup_receiver);
    drop(setup);
    result?;
    Ok(())
}

async fn publish_local_sessions(
    run: std::path::PathBuf,
    peers: Arc<ResidentPeers>,
    mut sessions: kuvo_engine::LocalSessions,
) {
    loop {
        if let Some(record) = sessions.latest_record()
            && let Ok(list) = serde_json::from_value(record)
        {
            let run = run.clone();
            if let Ok(mut local) = tokio::task::spawn_blocking(move || Local::load(&run)).await {
                local.list = Some(list);
                peers.local(local).await;
            }
        }
        if !sessions.changed().await {
            break;
        }
    }
}

fn info(
    home: &Path,
    name: &str,
    kind: ComputerKind,
    permits_pairing: bool,
) -> Result<info::Status200, Box<dyn std::error::Error>> {
    let chats = home.join("chats");
    fs::create_dir_all(&chats)?;
    let is_mac = kind == ComputerKind::Mac;
    // Advertise the implemented routes while the desktop remains behind its update gate.
    let mut features = vec!["info", "line", "engine.restart", "box.backups.status"];
    if !is_mac {
        features.push("engine.start");
    }
    features.extend_from_slice(kuvo_peers::routes::FEATURES);
    features.extend_from_slice(kuvo_engine::FEATURES);
    features.extend_from_slice(kuvo_services::files::FEATURES);
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
