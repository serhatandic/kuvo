mod agents;

use std::{fs, path::Path, sync::Arc, time::UNIX_EPOCH};

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
    let kind = if kuvo_setup::is_mac_home(home)? {
        ComputerKind::Mac
    } else {
        ComputerKind::Machine
    };
    #[cfg(target_os = "macos")]
    let helper_watch =
        if kind == ComputerKind::Mac && helper_ended.is_some() && !vault_launch.helper_renews() {
            kuvo_keeper::mac_helper::watch_handover(kuvo_store::home::Home::new(home.to_owned())?)
        } else {
            None
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
    let managed = accepted_roster.is_some() || prepared.is_set_aside();
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
    let factories = agents::factories(home);
    let resident_agents = if factories.is_empty() {
        None
    } else {
        let agent_home = kuvo_store::home::Home::new(home.to_owned())?;
        let runtime = kuvo_agents::runtime::Runtime::for_home(&agent_home);
        match kuvo_agents::resident::ResidentAgents::start(runtime.clone(), factories).await {
            Ok((agents, failures)) => {
                for failure in failures {
                    eprintln!(
                        "Kuvo could not take up agent holder {}: {}",
                        failure.holder, failure.error
                    );
                }
                Some((runtime, agents))
            }
            Err(error) => {
                eprintln!("Kuvo could not start its coding agents: {error}");
                None
            }
        }
    };
    let info = info(
        home,
        &roster.name,
        kind,
        devices.permits_local_pairing(),
        resident_agents.is_some(),
    )?;
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
    let owners = kuvo_peers::ownership::Handle::new(
        peers.reader(),
        peers.relay_handle(),
        devices.clone(),
        move || authority.ownership_state(),
    );
    peers.attach_moves(engine.clone(), owners.clone()).await?;
    let peers = Arc::new(peers);
    peers.follow_setup_roster(setup.handle().rosters()).await;
    let identity = kuvo_os::platform::Account::current()?;
    let account = identity.home_folder(
        kuvo_os::platform::Platform::current(),
        &std::env::vars_os().collect(),
    )?;
    let vault = kuvo_services::vault::Resident::new(
        home,
        kind == ComputerKind::Mac,
        vault_launch,
        account.clone(),
        &identity.home,
        vault_machine,
    );
    let engine = engine
        .with_session_ownership(Arc::new(super::session_ownership::OwnerPort(
            owners.clone(),
        )))
        .with_model_stars(Arc::new(super::model_stars::SetupStars(setup.handle())))
        .with_external_asks(Arc::new(super::vault_asks::VaultAsks::new(vault.clone())));
    let agent_shutdown = resident_agents.as_ref().map(|(_, agents)| agents.clone());
    let mut routes = Registry::new();
    let engine = if let Some((runtime, accounts)) = resident_agents {
        agents::register(accounts.clone(), &mut routes);
        engine.with_agent_model_source(Arc::new(agents::Models {
            runtime,
            agents: accounts,
        }))
    } else {
        engine
    };
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
    let owner_requests = if let Ok(requests) = owners.incoming() {
        peers.advertise_engine_submissions().await?;
        Some(tokio::spawn(super::session_ownership::serve(
            owners,
            requests,
            engine.clone(),
            sessions.clone(),
        )))
    } else {
        None
    };
    let accepted = setup.handle();
    let memory =
        kuvo_peers::memory_read::Handle::new(peers.reader(), peers.relay_handle(), move || {
            accepted.ownership_state().id
        });
    let search_engine = engine.clone();
    let memory_reads =
        memory.serve(move |_, read| super::setup_search::answer(search_engine.clone(), read));
    engine.register_setup_search(
        Arc::new(super::setup_search::SetupSearch {
            peers: peers.reader(),
            reads: memory,
        }),
        &mut routes,
    );
    if memory_reads.is_some() {
        peers.advertise_conversation_search().await?;
    }
    setup.handle().register(&mut routes);
    setup.handle().register_ssh(&mut routes, home);
    kuvo_services::files::register(
        &mut routes,
        kuvo_services::files::Places {
            home: account.clone(),
            box_home: home.to_owned(),
            platform: kuvo_os::platform::Platform::current(),
        },
    );
    kuvo_services::git::register(&mut routes, home.to_owned());
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
    let update_home = kuvo_store::home::Home::new(home.to_owned())?;
    let update_profile = account.clone();
    routes.register::<kuvo_protocol::routes::get_kuvo_update::Spec, _, _>(Access::Any, move |_| {
        let answer = kuvo_keeper::update_status::read(&update_home, &update_profile);
        async move {
            let result = answer.and_then(|body| Ok(serde_json::from_value(body)?));
            result.map(RouteReply::json::<200>).map_err(|error| {
                Refusal::new(http::StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
            })
        }
    });
    crate::updates::register_helper_stop(
        &mut routes,
        kuvo_store::home::Home::new(home.to_owned())?,
        kind == ComputerKind::Mac,
    );
    let updates = crate::updates::start(
        kuvo_store::home::Home::new(home.to_owned())?,
        &account,
        peers.clone(),
        setup.handle(),
    );
    crate::updates::register_check(
        &mut routes,
        kuvo_store::home::Home::new(home.to_owned())?,
        account.clone(),
        updates.as_ref(),
    );
    vault.register(&mut routes);
    #[cfg(target_os = "macos")]
    if kind == ComputerKind::Mac {
        super::sign_ins::register(&mut routes, kuvo_store::home::Home::new(home.to_owned())?);
    }
    let daily_backups = kuvo_setup::backup::routes::register(
        &mut routes,
        kuvo_store::home::Home::new(home.to_owned())?,
        account,
        setup.handle(),
        backup_mail,
        Some(vault.clone()),
    );
    let removal = setup.handle();
    kuvo_peers::routes::register(&mut routes, peers.reader(), devices.clone(), move || {
        removal.pending_wipes()
    });
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
    if let Some(agents) = agent_shutdown {
        match tokio::time::timeout(
            std::time::Duration::from_secs(2),
            agents.prepare_replacement(),
        )
        .await
        {
            Ok(failures) => {
                for (provider, error) in failures {
                    eprintln!("Kuvo could not checkpoint {provider}: {error}");
                }
            }
            Err(_) => eprintln!("Kuvo timed out checkpointing the coding agents."),
        }
    }
    if let Some(updates) = updates {
        updates.stop().await;
    }
    #[cfg(target_os = "macos")]
    if let Some(watch) = helper_watch {
        watch.abort();
        let _ = watch.await;
    }
    session_source.abort();
    if let Some(owner_requests) = owner_requests {
        owner_requests.abort();
        let _ = owner_requests.await;
    }
    if let Some(memory_reads) = memory_reads {
        memory_reads.abort();
        let _ = memory_reads.await;
    }
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
    agent_accounts: bool,
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
    features.push(kuvo_peers::memory_read::FEATURE);
    features.extend_from_slice(kuvo_services::files::FEATURES);
    if permits_pairing {
        features.push("devices.pair");
    }
    if agent_accounts {
        features.push("agents.accounts");
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
