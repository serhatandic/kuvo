mod agents;
mod found;
mod library_artifacts;
mod marketplace;
mod usage;
mod usage_history_peers;
mod usage_peers;

use kuvo_os::timer_fires::{self, Timer};
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
    let (tools, browser_tools) =
        agents::tools::relay(home, engine.clone(), kind == ComputerKind::Mac).await?;
    let feed = kuvo_agents::feed::Feed::default();
    let factories = agents::factories(home, &tools, &feed).await;
    let resident_agents = if factories.is_empty() {
        None
    } else {
        let agent_home = kuvo_store::home::Home::new(home.to_owned())?;
        let runtime = kuvo_agents::runtime::Runtime::for_home(&agent_home);
        match kuvo_agents::resident::ResidentAgents::start(runtime.clone(), factories).await {
            Ok((agents, failures)) => {
                let agents = agents.with_feed(feed);
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
    if let Some((runtime, _)) = &resident_agents {
        agents::tools::take_up(&tools, runtime).await;
    }
    let info = info(
        home,
        &roster.name,
        kind,
        devices.permits_local_pairing(),
        resident_agents.is_some(),
    )?;
    let vault_machine = (kind != ComputerKind::Mac).then(|| roster.name.clone());
    let bot_name = roster.name.clone();
    let identity = kuvo_os::platform::Account::current()?;
    let account = identity.home_folder(
        kuvo_os::platform::Platform::current(),
        &std::env::vars_os().collect(),
    )?;
    let update_home = kuvo_store::home::Home::new(home.to_owned())?;
    let bridge = crate::updates::installed_bridge(&update_home, &account);
    let mut peers = ResidentPeers::start_with_bridge(
        home,
        roster,
        relay,
        Some(devices.clone()),
        kind == ComputerKind::Mac,
        Arc::new(system_clock),
        bridge,
    )
    .await?;
    let authority = setup.handle();
    let ownership_events = authority.clone();
    let owners = kuvo_peers::ownership::Handle::new(
        peers.reader(),
        peers.relay_handle(),
        devices.clone(),
        move || authority.ownership_state(),
    ).with_setup_changes(move || {
        let mut setup = ownership_events.clone();
        Box::pin(async_stream::stream! {
            while setup.changed().await { yield (); }
        })
    });
    peers
        .attach_moves_with_queue(
            engine.clone(),
            owners.clone(),
            Some(kuvo_work::queue::move_transfer(home, setup.handle())),
        )
        .await?;
    // Work for leads on your other computers: taken once this box's work is there (below).
    let elsewhere = kuvo_work::work::elsewhere::Elsewhere::new();
    let work_calls = match peers.attach_work(Arc::new(elsewhere.clone())).await {
        Ok(calls) => Some(calls),
        Err(error) if error.kind() == std::io::ErrorKind::NotConnected => None,
        Err(error) => return Err(error.into()),
    };
    let peers = Arc::new(peers);
    peers.follow_setup_roster(setup.handle().rosters()).await;
    let vault = kuvo_services::vault::Resident::new(
        home,
        kind == ComputerKind::Mac,
        vault_launch,
        account.clone(),
        &identity.home,
        vault_machine,
    );
    let (peer_waiting, waiting_source) = tokio::sync::watch::channel(Arc::new(
        peers.reader().view().peer_waiting(peers.reader().now()),
    ));
    let peer_waiting_source = tokio::spawn(publish_peer_waiting(peers.reader(), peer_waiting));
    let engine = engine
        .with_session_ownership(Arc::new(super::session_ownership::OwnerPort(
            owners.clone(),
        )))
        .with_model_stars(Arc::new(super::model_stars::SetupStars(setup.handle())))
        .with_peer_waiting(waiting_source)
        .with_external_asks(Arc::new(super::vault_asks::VaultAsks::new(vault.clone())));
    let agent_shutdown = resident_agents.as_ref().map(|(_, agents)| agents.clone());
    let menu_limits = kuvo_services::limits_menu::MenuLimits::default();
    let usage_settings =
        kuvo_services::usage::settings::Service::new(home.to_owned(), setup.handle());
    let limits = kuvo_services::usage::limits::Service::new(
        home.to_owned(),
        usage_settings.clone(),
        menu_limits.clone(),
        peers.reader().view().roster.local().name,
        peers
            .reader()
            .view()
            .roster
            .here
            .as_ref()
            .map(|key| kuvo_wire::keys::encode(key))
            .unwrap_or_default(),
    );
    let standalone_plans = kuvo_services::usage::standalone::Readers::for_home(home, &account);
    if let Some(readers) = &standalone_plans {
        readers.confirm(&limits).await;
    }
    let agent_settings_feed = resident_agents
        .as_ref()
        .map(|(_, accounts)| accounts.feed().clone());
    let mut agent_handover = None;
    let mut routes = Registry::new();
    crate::box_app::register(
        &mut routes,
        update_home.clone(),
        kind == ComputerKind::Mac,
        peers.clone(),
    )
    .await?;
    let sessions = resident_agents.as_ref().map_or_else(
        || kuvo_agents::session_store::SessionStore::new(home.join("agents/sessions")),
        |(runtime, _)| runtime.sessions().clone(),
    );
    // OpenCode's conversations from before it was a coding agent here, read from its database.
    let opencode_history = kuvo_agents::acp::opencode_history::OpenCodeHistory::new(
        home,
        kuvo_engine::agent_program(home, std::env::vars_os().collect())
            .ok()
            .map(|(executable, env)| kuvo_agents::acp::OpenCodeProgram { executable, env }),
        sessions.clone(),
    );
    agents::sessions::register(sessions.clone(), Some(opencode_history.clone()), &mut routes);
    let agent_background = sessions.background().clone();
    let shell_commands = agent_background.clone();
    let engine = if let Some((runtime, accounts)) = resident_agents {
        // What its coding agents keep going, for the work routes and those that read the file.
        let _published = sessions
            .background()
            .publish(home.join("run/work/background.json"));
        agents::register(accounts.clone(), &mut routes);
        let location = if kind == ComputerKind::Mac {
            kuvo_agents::permissions::ConfigLocation::Desktop
        } else {
            kuvo_agents::permissions::ConfigLocation::Machine
        };
        let handover = kuvo_agents::session_runs::Handover::take(&agents::handover_file(home))
            .unwrap_or_else(|error| {
                eprintln!("Kuvo could not read the coding agents' handover: {error}");
                None
            });
        usage::confirm(&limits, &accounts).await;
        tokio::spawn(usage::Turns::follow(
            limits.clone(),
            accounts.clone(),
            standalone_plans.clone(),
        ));
        let runs = kuvo_agents::session_runs::SessionRuns::start_with_limits(
            accounts.clone(),
            sessions.clone(),
            home.to_owned(),
            location,
            kuvo_agents::session_runs::Update {
                adopted: accounts.adopted_turns().to_vec(),
                handover,
            },
            usage::Turns::start(limits.clone(), accounts.clone()),
        );
        agent_handover = Some((runs.clone(), runtime.clone()));
        agents::asks::register(runs.clone(), sessions.clone(), &mut routes);
        agents::asks::follow(runs.clone(), sessions.clone(), accounts.feed().clone());
        let inventory = agents::sessions::Inventory {
            store: sessions.clone(),
            running: Some(runs.clone()),
            home: home.to_owned(),
            agents: Some(accounts.clone()),
            opencode: Some(opencode_history.clone()),
        };
        let rows = kuvo_agents::account_rows::AccountRows::new(
            home.to_owned(),
            accounts.clone(),
        );
        found::register(Some(rows.clone()), home, &mut routes);
        agents::rows::register(rows, Some(runs.clone()), sessions.clone(), &mut routes);
        agents::sessions::register_reads(inventory.clone(), &mut routes);
        agents::turns::register(runs, accounts.feed().clone(), &mut routes);
        engine
            .with_agent_model_source(Arc::new(agents::Models {
                runtime,
                agents: accounts,
            }))
            .with_agent_sessions(Arc::new(inventory))
    } else {
        found::register(None, home, &mut routes);
        engine.with_agent_sessions(Arc::new(agents::sessions::Inventory {
            store: sessions,
            running: None,
            home: home.to_owned(),
            agents: None,
            opencode: Some(opencode_history),
        }))
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
    agents::background::register(
        home,
        agent_background,
        agents::background::Listed {
            sessions: sessions.clone(),
            peers: peers.reader(),
            bots: home.join("bots"),
        },
        &mut routes,
    );
    let owner_requests = if let Ok(requests) = owners.incoming() {
        peers.advertise_engine_submissions().await?;
        Some(tokio::spawn(super::session_ownership::serve(
            owners.clone(),
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
    let work_queue = kuvo_work::queue::register(
        &mut routes,
        home,
        sessions.clone(),
        engine.clone(),
        setup.handle(),
        kuvo_work::queue::Network {
            peers: peers.reader(),
            relay: peers.relay_handle(),
            memory: memory.clone(),
            owners: owners.clone(),
        },
    )
    .await?;
    let queue_reads = work_queue.peer_reads();
    let search_engine = engine.clone();
    let read_owner = kuvo_peers::owner_read::Service::new(
        peers.reader(),
        owners.clone(),
        memory.clone(),
        engine.clone(),
    );
    read_owner.register(&mut routes);
    let library_artifacts = Arc::new(
        library_artifacts::ResidentLibraryArtifacts::start(
            home,
            peers.reader().view().roster.local().name,
            peers.reader(),
            peers.relay_handle(),
            memory.clone(),
            engine.clone(),
            &mut routes,
        )
        .await,
    );
    let library_reader = library_artifacts.clone();
    let memory_reads = memory.serve(move |_, read| {
        let owner = read_owner.clone();
        let queues = queue_reads.clone();
        let engine = search_engine.clone();
        let library_reader = library_reader.clone();
        async move {
            match read.area.as_deref() {
                Some("ownership") if queues.serves_ownership(&read) => queues.answer(read).await,
                Some("ownership") => owner.answer(read).await,
                Some("search") => super::setup_search::answer(engine, read).await,
                Some("library") => library_reader.answer_library_read(read).await,
                _ => queues.answer(read).await,
            }
        }
    });
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
    let setup_routes = setup.handle();
    // Outside a setup, a released beta.1 Mac that enrolled this machine can be let go here.
    let setup_routes = if managed { setup_routes } else { setup_routes.with_legacy(devices.clone()) };
    setup_routes.register_with_record_edits(&mut routes, Some(work_queue.record_edits()));
    setup.handle().register_ssh(&mut routes, home);
    let archives = kuvo_services::settings::register(
        &mut routes,
        home.to_owned(),
        account.clone(),
        setup.handle(),
        kind == ComputerKind::Mac,
        vault.clone(),
        peers.reader(),
        engine.clone(),
        sessions.clone(),
        agent_settings_feed,
        work_queue.archive_forward(),
    );
    kuvo_services::files::register(
        &mut routes,
        kuvo_services::files::Places {
            home: account.clone(),
            box_home: home.to_owned(),
            platform: kuvo_os::platform::Platform::current(),
        },
    );
    kuvo_services::git::register(&mut routes, home.to_owned(), kind == ComputerKind::Mac);
    kuvo_services::terminals::register(&mut routes, home);
    kuvo_services::programs::register(&mut routes, account.clone());
    kuvo_services::browser::register(&mut routes, home);
    let browser_install = kuvo_services::browser::install::InstallTask::start(home.to_owned(), kind == ComputerKind::Mac)?;
    browser_install.register(&mut routes);
    let browser_task = browser_install.browser_task(engine.clone())?;
    let browser = browser_task.browser();
    let browser_downloads = {
        let mut notes = browser.events();
        let library = library_artifacts.clone();
        tokio::spawn(async move {
            loop {
                match notes.recv().await {
                    Ok(note) if note["method"] == "Kuvo.downloaded" => {
                        if let (Some(session),Some(path))=(note["params"]["session"].as_str(),note["params"]["path"].as_str()) {
                            library.note_download(session,path).await;
                        }
                    }
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {},
                    Err(_) => break,
                }
            }
        })
    };
    browser.register_view(&mut routes, kind == ComputerKind::Mac);
    let browser_sites = kuvo_services::browser::manager::Sites::new(home.to_owned(), browser.clone());
    browser_sites.register(&mut routes);
    if let Some(tools) = browser_tools.as_ref() { tools.set_browser(browser.clone()); }
    let integration_keys = kuvo_services::integrations::register(
        &mut routes,
        home,
        kind == ComputerKind::Mac,
        setup.handle(),
        browser_sites,
    );
    let skills = kuvo_services::skills::ResidentSkills::start(&mut routes, home, &account);
    let resident_marketplace = kuvo_services::marketplace::ResidentMarketplace::start(
        &mut routes,
        home,
        &account,
        kind == ComputerKind::Mac,
        peers.reader().view().roster.local().name,
        Some(setup.handle()),
        vault.clone(),
        skills.store.clone(),
        skills.library.clone(),
        Arc::new(marketplace::PeerBridge::new(peers.reader())),
    )
    .await;
    let wakes = kuvo_work::wakes::Wakes::new(home, engine.clone());
    wakes.register(&mut routes);
    // (A machine names itself on its asks; a Mac's own aren't named.)
    let asks_machine = (kind != ComputerKind::Mac).then(|| bot_name.clone());
    kuvo_work::asks::Asks::new(home, asks_machine, engine.clone()).with_browser(browser.asks()).register(&mut routes);
    agents::shells::register(shell_commands, engine.clone(), &mut routes);
    let bots = kuvo_work::bots::Bots::new(
        kuvo_work::bots::Setting {
            home: home.to_owned(),
            account: account.clone(),
            name: bot_name,
            is_mac: kind == ComputerKind::Mac,
        },
        wakes.clone(),
        peers.reader(),
        sessions.clone(),
        engine.clone(),
        Arc::new(super::bot_setup::BotSetup(setup.handle())),
    );
    bots.register(&mut routes);
    // (Add Agent in the Marketplace makes its bots here.)
    resident_marketplace
        .service
        .give_agent_bots(Arc::new(bots.clone()));
    let bots_follow_engine = bots.follow_engine();
    // Edits held for bots on your other computers, sent; and theirs for bots here, taken.
    let bots_held = peers
        .relay_handle()
        .map(|relay| bots.follow_held(owners.clone(), relay));
    // Bots' old keys of their own taken away, with no client there.
    let _own_keys = kuvo_work::bots::own_keys::keep_bots_own_keys_gone(
        home.to_owned(),
        kind == ComputerKind::Mac,
        home.join("bots"),
        integration_keys.clone(),
        kuvo_work::bots::own_keys::reconnecting(engine.clone()),
    );
    let approvals = kuvo_work::approval::ApprovalService::new(
        kuvo_work::approval::Approvals::new(home, kind == ComputerKind::Mac),
        engine.clone(),
        home.join("bots"),
        sessions.clone(),
    );
    approvals.register(&mut routes);
    let approvals_kept = approvals.keep();
    let work_settings =
        kuvo_work::work::WorkSettings::new(home, &account, kind == ComputerKind::Mac);
    let kept_awake = work_settings.keep_awake(sessions.clone());
    let engine_reader = usage::engine_reader(home, &account);
    let mut resets = kuvo_services::usage::resets::Resets::new(
        home,
        limits.clone(),
        Arc::new(usage::ResetPort {
            agents: agent_shutdown.clone(),
            limits: limits.clone(),
            engine: engine_reader
                .clone()
                .map(|reader| (engine.clone(), reader, home.to_owned())),
        }),
    );
    if let Some(relay) = peers.relay_handle() {
        resets = resets.with_peers(Arc::new(usage_peers::ResetTransport {
            directory: peers.reader(),
            relay,
        }));
    }
    resets.follow_sign_ins();
    kuvo_services::usage::resets::register(&mut routes, resets.clone());
    let work = kuvo_work::work::Work::new(
        work_settings,
        home,
        sessions.clone(),
        peers.reader(),
        Arc::new(super::bot_setup::BotSetup(setup.handle())),
        bots.clone(),
    )
    .with_limits(limits.clone())
    .with_resets(resets.clone());
    work.attach_start_link(work_queue.start_link());
    // (What it tells, read again for the clients that follow it.)
    let (told_peers, told_run) = (peers.clone(), home.join("run"));
    work.notices.attach_told(Arc::new(move || {
        let (peers, run) = (told_peers.clone(), told_run.clone());
        let telling = kind == ComputerKind::Mac;
        tokio::spawn(async move {
            peers
                .notices(kuvo_peers::notices::load(&run, telling))
                .await
        });
    }));
    usage::notices(&limits, work.notices.clone());
    work.register(&mut routes);
    let work_watch = work.watch();
    let work_rows = work_calls.map(|calls| elsewhere.serve(work.clone(), calls));
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
    kuvo_services::usage::settings::register(&mut routes, usage_settings.clone());
    usage::verify_sign_ins(
        &limits,
        agent_shutdown.as_ref(),
        standalone_plans.as_ref(),
        &engine,
        home,
    )
    .await;
    let catch_up = limits.clone();
    let _ = tokio::task::spawn_blocking(move || {
        catch_up.replay_notices(kuvo_services::usage::limits::now());
    })
    .await;
    let (verify_service, verify_agents, verify_readers, verify_engine, verify_home) = (
        limits.clone(),
        agent_shutdown.clone(),
        standalone_plans.clone(),
        engine.clone(),
        home.to_owned(),
    );
    kuvo_services::usage::limits::register(&mut routes, limits.clone(), move || {
        let (service, agents, readers, engine, home) = (
            verify_service.clone(),
            verify_agents.clone(),
            verify_readers.clone(),
            verify_engine.clone(),
            verify_home.clone(),
        );
        async move {
            usage::verify_sign_ins(&service, agents.as_ref(), readers.as_ref(), &engine, &home)
                .await;
        }
    });
    let insights = kuvo_services::usage::insights::Service::new(
        home.to_owned(),
        peers.reader().view().roster.local().name,
        peers
            .reader()
            .view()
            .roster
            .here
            .as_ref()
            .map(|key| kuvo_wire::keys::encode(key))
            .unwrap_or_default(),
        &hostname::get()?.to_string_lossy(),
    )
    .with_history(usage_settings, limits.clone())
    .with_directory(peers.reader())
    .with_cursor(standalone_plans.clone());
    let cursor_watch = insights.follow_cursor(routes.client_activity());
    let price_watch = insights.follow_prices(&account);
    let usage_watch = kuvo_os::watch::WatchHub::default();
    let usage_sign_ins = standalone_plans
        .as_ref()
        .map(|reader| reader.follow_sign_ins(usage_watch.clone(), limits.clone()));
    let insights_watch = insights.follow(usage_watch.clone());
    let engine_usage_watch = insights.follow_engine(usage_watch.clone());
    let native_usage_watch = insights.follow_native(usage_watch, limits.clone());
    kuvo_services::usage::insights::register(&mut routes, insights.clone());
    let refresh_limits = limits.clone();
    let refresh_agents = agent_shutdown.clone();
    let refresh_engine = engine.clone();
    let refresh_home = home.to_owned();
    kuvo_services::usage::limits::register_refresh(
        &mut routes,
        limits.clone(),
        move |selected, explicit| {
            let (limits, agents, engine, reader, standalone, home) = (
                refresh_limits.clone(),
                refresh_agents.clone(),
                refresh_engine.clone(),
                engine_reader.clone(),
                standalone_plans.clone(),
                refresh_home.clone(),
            );
            async move {
                let agents_failed = if let Some(agents) = agents {
                    usage::confirm(&limits, &agents).await;
                    usage::refresh(limits.clone(), agents, selected.clone(), explicit).await
                } else {
                    false
                };
                let standalone_failed = if let Some(standalone) = standalone {
                    standalone
                        .refresh(limits.clone(), selected.as_deref(), explicit)
                        .await
                } else {
                    false
                };
                let engine_failed = if let Some(reader) = reader {
                    usage::refresh_engine(limits, engine, reader, home, selected, explicit).await
                } else {
                    false
                };
                agents_failed || engine_failed || standalone_failed
            }
        },
    );
    let limits_peers = peers.relay_handle().map(|relay| {
        tokio::spawn(usage_peers::follow_limits(
            limits.clone(),
            peers.reader(),
            relay,
            setup.handle(),
            resets.clone(),
        ))
    });
    let history_peers = peers.relay_handle().map(|relay| {
        tokio::spawn(usage_history_peers::follow(
            insights,
            limits.clone(),
            peers.reader(),
            relay,
            setup.handle(),
        ))
    });
    let _ = limits.answer(kuvo_services::usage::limits::now());
    crate::box_at_work::register(
        &mut routes,
        home,
        kind == ComputerKind::Mac,
        engine.clone(),
        sessions.clone(),
        peers.reader(),
        menu_limits,
    );
    crate::updates::register_helper_stop(
        &mut routes,
        kuvo_store::home::Home::new(home.to_owned())?,
        crate::box_at_work::applies(home, kind == ComputerKind::Mac),
    );
    let update_home = kuvo_store::home::Home::new(home.to_owned())?;
    let backup_sweep = crate::updates::sweep_backups(
        update_home.clone(),
        kuvo_services::config::is_own_home_mac(home, kind == ComputerKind::Mac),
    );
    let updates = crate::updates::start(
        update_home,
        &account,
        peers.clone(),
        setup.handle(),
        engine.clone(),
    );
    crate::updates::register_check(
        &mut routes,
        kuvo_store::home::Home::new(home.to_owned())?,
        account.clone(),
        updates.as_ref(),
        engine.clone(),
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
        Some(work_queue.queue_review()),
    );
    let removal = setup.handle();
    let follows = kuvo_peers::session_follow::Follows::new(peers.reader(), peers.relay_handle(), peers.reach_again());
    kuvo_peers::routes::register(&mut routes, peers.reader(), devices.clone(), follows, move || {
        removal.computers_listing()
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
    if let Some(browser_tools) = browser_tools {
        browser_tools.attach(&dispatcher);
    }
    peers.dispatch_with(dispatcher.clone()).await;
    // What other computers follow of this one's conversations, answered while they keep it.
    let _conversation_follows = peers.relay_handle().and_then(|relay| {
        kuvo_peers::session_follow::serve(peers.reader(), relay, dispatcher.downgrade(), owners.clone())
    });
    work_queue.serve_archives(archives.clone(), dispatcher.clone());
    let archive_source = tokio::spawn({
        let peers = peers.clone();
        let dispatcher = dispatcher.clone();
        let mut sessions = sessions.clone();
        let mut setup = setup.handle();
        let mut moves = kuvo_os::watch::FileSet::new(kuvo_os::watch::WatchHub::default());
        moves.set_targets(Vec::new(), vec![(home.join("run/moves"), false)]);
        async move {
            let mut previous = None;
            let mut published_to = Vec::new();
            let mut peer_changes = peers.reader();
            let mut zone = None;
            let mut first = true;
            let mut held = None;
            loop {
                let mut forced = first;
                if !first {
                    tokio::select! {
                        changed = sessions.changed() => if !changed { break },
                        changed = peer_changes.changed() => if changed.is_err() { break },
                        changed = setup.changed() => if !changed { break },
                        _ = moves.changed() => forced = true,
                    }
                }
                first = false;
                let members = peer_changes
                    .view()
                    .roster
                    .members
                    .iter()
                    .map(|member| member.key)
                    .collect::<Vec<_>>();
                if members.is_empty() {
                    published_to.clear();
                    continue;
                }
                let revised = sessions.archive_revision();
                let next_zone = setup.time_zone();
                let next_held = peer_changes.view().local.ownership.held.clone();
                if !forced
                    && revised == previous
                    && members == published_to
                    && zone == next_zone
                    && held.as_ref() == Some(&next_held)
                {
                    continue;
                }
                if let Some(copy) = archives.snapshot(dispatcher.clone()).await {
                    previous = revised;
                    published_to = members;
                    zone = next_zone;
                    held = Some(next_held);
                    peers.archives(copy).await;
                }
            }
        }
    });
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
    let session_source = tokio::spawn(publish_local_sessions(
        home.join("run"),
        peers,
        sessions,
        bots,
        wakes.changes(),
    ));
    let firing = wakes.start();
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
    if let Some((runs, runtime)) = agent_handover {
        // Making way for the next resident, its coding agents go on for it to take up; stopping
        // for good (turned off, paused, or with no keeper to start another), they stop with it.
        if kuvo_keeper::next_resident_coming(home) {
            agents::hand_over(home, &runs, &runtime).await;
        } else if let Err(error) = runtime.holders().stop_all().await {
            eprintln!("Kuvo could not stop the coding agents: {error}");
        }
    }
    tools.shutdown().await;
    browser_task.stop().await;
    browser_downloads.abort();
    let _ = browser_downloads.await;
    browser_install.stop().await;
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
    firing.stop().await;
    bots_follow_engine.abort();
    bots_held.inspect(tokio::task::JoinHandle::abort);
    approvals_kept.abort();
    insights_watch.abort();
    native_usage_watch.abort();
    if let Some(watch) = usage_sign_ins {
        watch.abort();
    }
    engine_usage_watch.abort();
    if let Some(task) = cursor_watch {
        task.abort();
    }
    if let Some(task) = price_watch {
        task.abort();
    }
    if let Some(task) = limits_peers {
        task.abort();
    }
    if let Some(task) = history_peers {
        task.abort();
    }
    work_watch.abort();
    work_rows.inspect(tokio::task::JoinHandle::abort);
    if let Some(kept_awake) = kept_awake {
        kept_awake.abort();
    }
    backup_sweep.abort();
    if let Some(updates) = updates {
        updates.stop().await;
    }
    #[cfg(target_os = "macos")]
    if let Some(watch) = helper_watch {
        watch.abort();
        let _ = watch.await;
    }
    peer_waiting_source.abort();
    let _ = peer_waiting_source.await;
    session_source.abort();
    archive_source.abort();
    let _ = archive_source.await;
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
    if let Ok(library_artifacts) = Arc::try_unwrap(library_artifacts) {
        library_artifacts.stop().await;
    }
    skills.stop();
    resident_marketplace.stop().await;
    let _ = session_source.await;
    drop(daily_backups);
    drop(backup_receiver);
    drop(setup);
    result?;
    Ok(())
}

async fn publish_peer_waiting(
    mut peers: kuvo_peers::setup_lists::Reader,
    waiting: tokio::sync::watch::Sender<Arc<Vec<kuvo_engine::PeerWaiting>>>,
) {
    loop {
        let now = peers.now();
        let view = peers.view();
        let current = view.peer_waiting(now);
        let expiry = view.next_peer_waiting_change(now);
        waiting.send_if_modified(|last| {
            if **last == current {
                return false;
            }
            *last = Arc::new(current);
            true
        });
        tokio::select! {
            changed = peers.changed() => if changed.is_err() { break; },
            _ = async {
                match expiry {
                    Some(at) => timer_fires::sleep(Timer::PeerWaiting, std::time::Duration::from_millis(at.saturating_sub(now).max(1) as u64)).await,
                    None => std::future::pending().await,
                }
            } => {},
            _ = waiting.closed() => break,
        }
    }
}

async fn publish_local_sessions(
    run: std::path::PathBuf,
    peers: Arc<ResidentPeers>,
    mut sessions: kuvo_engine::LocalSessions,
    bots: kuvo_work::bots::Bots,
    mut wakes: tokio::sync::watch::Receiver<u64>,
) {
    loop {
        if let Some(record) = sessions.latest_record()
            && let Ok(list) = serde_json::from_value(record)
        {
            let (run, bots) = (run.clone(), bots.clone());
            if let Ok(mut local) = tokio::task::spawn_blocking(move || {
                let mut local = Local::load(&run);
                local.bots = serde_json::to_value(bots.peer_activity())
                    .and_then(serde_json::from_value)
                    .ok();
                local
            })
            .await
            {
                local.list = Some(list);
                peers.local(local).await;
            }
        }
        tokio::select! {
            changed = sessions.changed() => if !changed { break },
            _ = wakes.changed() => {}
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
    let mut features = vec![
        "info",
        "line",
        "engine.restart",
        "box.backups.status",
        "box.atWork",
        found::FEATURE,
    ];
    if !is_mac {
        features.push("browser");
        if !cfg!(windows) { features.push("browser.view"); }
        features.push("engine.start");
    }
    features.extend_from_slice(kuvo_peers::routes::FEATURES);
    features.extend_from_slice(kuvo_engine::FEATURES);
    features.push(kuvo_peers::memory_read::FEATURE);
    features.push(kuvo_peers::owner_read::FEATURE);
    features.extend_from_slice(&["browser.data", "browser.data.never"]);
    features.extend_from_slice(kuvo_services::files::FEATURES);
    features.extend_from_slice(kuvo_services::git::FEATURES);
    features.extend_from_slice(kuvo_services::artifacts::FEATURES);
    features.extend_from_slice(kuvo_services::library::FEATURES);
    features.extend_from_slice(kuvo_services::programs::FEATURES);
    features.extend_from_slice(kuvo_services::integrations::FEATURES);
    features.extend_from_slice(kuvo_services::skills::FEATURES);
    features.extend_from_slice(kuvo_services::settings::FEATURES);
    features.extend_from_slice(kuvo_services::marketplace::FEATURES);
    features.extend_from_slice(kuvo_work::wakes::FEATURES);
    features.extend_from_slice(kuvo_work::asks::FEATURES);
    features.push("shell.stop");
    features.push("usage.settings");
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
