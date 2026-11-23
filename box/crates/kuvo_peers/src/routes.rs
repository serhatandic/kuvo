//! Register the resident's peer reads and desktop grant routes with the common dispatcher.

use std::io;

use futures_util::stream;
use http::StatusCode;
use kuvo_protocol::{
    JsonStatus,
    gate::{self, Compatibility},
    json::{BoolLiteral, Null, Nullable},
    routes::{types::*, *},
};
use kuvo_server::{Access, Caller, JsonLine, Refusal, Registry, RouteReply, Transport};
use kuvo_wire::keys::encode;
use serde_json::Number;

use crate::{
    attention, conversation,
    devices::{self, Devices},
    directory::RecordScope,
    lists::{self, Origin, Place, SessionAction},
    reach::{Millis, Standing},
    setup_lists::{Reader, View},
};

pub const FEATURES: &[&str] = &[
    "attention",
    "computers",
    "line.merged-notices",
    "line.merged-sessions",
    "line.merged-waiting",
    "line.setup-lists",
    "line.pending-computers",
    "notices",
    "phones",
    "sessions.location",
];

const NOT_READY: &str = "The session list isn’t ready.";
const ENGINE_NOT_READY: &str = "The engine isn’t running.";
const CONVERSATION_NOT_READY: &str = "The conversation’s engine isn’t answering.";
const CONVERSATION_GONE: &str = "That conversation isn’t there any more.";

pub fn register(registry: &mut Registry, peers: Reader, devices: Devices) {
    crate::moves::register(registry, &peers);
    follow::<get_kuvo_setup_sessions_events::Spec>(
        registry,
        &peers,
        |view, now| Some(view.sessions(now)),
        None,
    );
    follow::<get_kuvo_setup_sessions_list_events::Spec>(
        registry,
        &peers,
        View::sidebar,
        Some(ENGINE_NOT_READY),
    );
    follow::<get_kuvo_setup_waiting_events::Spec>(
        registry,
        &peers,
        |view, now| Some(view.waiting(now)),
        None,
    );
    follow::<get_kuvo_setup_queues_events::Spec>(
        registry,
        &peers,
        |view, now| Some(view.queues(now)),
        None,
    );
    follow::<get_kuvo_setup_runs_on_events::Spec>(
        registry,
        &peers,
        |view, now| Some(view.runs_on(now)),
        None,
    );
    follow::<get_kuvo_setup_waiting_counts_events::Spec>(
        registry,
        &peers,
        View::waiting_counts,
        None,
    );
    follow::<get_kuvo_setup_notices_events::Spec>(
        registry,
        &peers,
        |view, now| Some(view.merged_notices(now)),
        None,
    );
    follow::<get_kuvo_notices_events::Spec>(
        registry,
        &peers,
        |view, now| Some(view.notices(now)),
        None,
    );
    follow::<get_kuvo_computers_events::Spec>(
        registry,
        &peers,
        |view, now| Some(computers(view, now)),
        None,
    );
    read::<get_kuvo_notices::Spec>(registry, &peers, |view, now| Some(view.notices(now)));
    read::<get_kuvo_computers::Spec>(registry, &peers, |view, now| Some(computers(view, now)));
    read::<get_kuvo_setup_waiting_counts::Spec>(registry, &peers, View::waiting_counts);

    let state = peers.clone();
    registry.register::<get_kuvo_sessions_session_location::Spec, _, _>(Access::Any, move |call| {
        let id = call.param("session").unwrap_or_default();
        let with_tools = query(&call.request.path, "tools").as_deref() == Some("1");
        std::future::ready(
            location(&state.view(), id, with_tools, state.now()).map(RouteReply::json::<200>),
        )
    });
    let state = peers.clone();
    registry.register::<get_kuvo_peer_session_computer_session_messages::Spec, _, _>(
        Access::Any,
        move |call| {
            let state = state.clone();
            async move {
                if !matches!(
                    &call.request.caller,
                    Caller::Paired { .. }
                        | Caller::Local(Transport::Http {
                            origin: kuvo_server::LocalOrigin::App | kuvo_server::LocalOrigin::Mac,
                            ..
                        })
                ) {
                    return Err(Refusal::new(
                        StatusCode::FORBIDDEN,
                        "Open the conversation from Kuvo Code.",
                    ));
                }
                let view = state.view();
                let computer = call.param("computer").unwrap_or_default();
                let session = call.param("session").unwrap_or_default();
                let peer = view
                    .roster
                    .members
                    .iter()
                    .find(|peer| encode(&peer.key) == computer)
                    .filter(|_| conversation::valid_session(session))
                    .ok_or_else(|| {
                        Refusal::new(
                            StatusCode::NOT_FOUND,
                            "There’s no such computer in this setup.",
                        )
                    })?;
                let features = view.announced.get(computer);
                if features.is_none_or(|said| !said.supports("line.peer-conversation")) {
                    return Err(Refusal::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "Update Kuvo Code on that computer to follow it from here.",
                    ));
                }
                if query(&call.request.path, "cursor").is_some()
                    || (view.relay_connected && view.reach.away_since(computer).is_none())
                {
                    return Err(Refusal::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "The conversation’s computer isn’t answering.",
                    ));
                }
                let key = peer.key;
                let id = session.to_owned();
                let run = state.run.clone();
                let kept = tokio::task::spawn_blocking(move || conversation::load(&run, &key, &id))
                    .await
                    .map_err(|error| internal(io::Error::other(error)))?;
                let standing = view.standing(key, state.now());
                let seen = standing
                    .away()
                    .then(|| {
                        standing
                            .last_seen()
                            .unwrap_or_default()
                            .max(kept.as_ref().map_or(0, |page| page.at))
                    })
                    .filter(|seen| *seen != 0);
                let mut away = serde_json::json!({"computer":peer.computer()});
                if let Some(seen) = seen {
                    away["lastSeen"] = seen.into();
                }
                if standing.updating() == Some(true) {
                    away["updating"] = true.into();
                }
                Ok(RouteReply::json::<200>(serde_json::json!({
                    "data":kept.map_or_else(Vec::new, |page| page.data), "away":away,
                })))
            }
        },
    );
    register_devices(registry, devices);
    register_attention(registry);
}

pub fn merged_project_page(
    peers: &Reader,
    project: &str,
    shown: usize,
    step: u32,
) -> Option<get_kuvo_sessions_page::Status200> {
    let sidebar = peers.view().sidebar(peers.now())?;
    Some(lists::page(&sidebar.sessions, project, shown, step))
}

pub(crate) fn query(path: &str, key: &str) -> Option<String> {
    form_urlencoded::parse(path.split_once('?')?.1.as_bytes())
        .find_map(|(name, value)| (name == key).then(|| value.into_owned()))
}

fn read<R>(
    registry: &mut Registry,
    peers: &Reader,
    snapshot: fn(&View, Millis) -> Option<<R as JsonStatus<200>>::Body>,
) where
    R: JsonStatus<200> + kuvo_protocol::RouteSpec<Body = ()> + 'static,
{
    let peers = peers.clone();
    registry.register::<R, _, _>(Access::Any, move |_| {
        std::future::ready(
            snapshot(&peers.view(), peers.now())
                .map(RouteReply::json::<200>)
                .ok_or_else(|| Refusal::new(StatusCode::SERVICE_UNAVAILABLE, NOT_READY)),
        )
    });
}

pub(crate) fn follow<R>(
    registry: &mut Registry,
    peers: &Reader,
    snapshot: impl Fn(&View, Millis) -> Option<<R as JsonStatus<200>>::Body>
    + Clone
    + Send
    + Sync
    + 'static,
    refuse_initial: Option<&'static str>,
) where
    R: JsonStatus<200> + kuvo_protocol::RouteSpec<Body = ()> + 'static,
    <R as JsonStatus<200>>::Body: Clone + PartialEq + Send + 'static,
{
    let peers = peers.clone();
    registry.register::<R, _, _>(Access::Any, move |_| {
        let initial = snapshot(&peers.view(), peers.now());
        let answer = if let (None, Some(message)) = (&initial, refuse_initial) {
            Err(Refusal::new(StatusCode::SERVICE_UNAVAILABLE, message))
        } else {
            let snapshot = snapshot.clone();
            let states = stream::unfold(
                (peers.clone(), Some(initial), None),
                move |(mut peers, mut initial, mut before)| {
                    let snapshot = snapshot.clone();
                    async move {
                        loop {
                            let next = if let Some(first) = initial.take() {
                                first
                            } else {
                                if peers.changed().await.is_err() {
                                    return None;
                                }
                                snapshot(&peers.view(), peers.now())
                            };
                            let Some(next) = next else {
                                continue;
                            };
                            if before.as_ref() == Some(&next) {
                                continue;
                            }
                            before = Some(next.clone());
                            return Some((JsonLine::<R, 200>::new(next), (peers, initial, before)));
                        }
                    }
                },
            );
            Ok(RouteReply::ndjson::<200>(states))
        };
        std::future::ready(answer)
    });
}

fn location(
    view: &View,
    id: &str,
    with_tools: bool,
    now: Millis,
) -> Result<get_kuvo_sessions_session_location::Status200, Refusal> {
    use GetKuvoSessionsSessionLocationStatus200 as Answer;
    if !conversation::valid_session(id) {
        return Err(Refusal::new(StatusCode::NOT_FOUND, CONVERSATION_GONE));
    }
    let journal = view.moved(id, now);
    let owner = view.owner_of(RecordScope::Session, id, now);
    let local = view.local.list.as_ref();
    let entry = local.and_then(|list| list.sessions.get(id));
    let allowed_here = owner.as_ref().is_none_or(|owner| owner.local)
        && journal
            .as_ref()
            .is_none_or(|key| key == &view.roster.local().id);
    if allowed_here && !view.local.ownership.frozen.contains(id) {
        let here = match entry {
            Some(row) if row.facts.away != Some(true) => row.place.map(|place| {
                (
                    place,
                    row.facts.running == Some(true),
                    Some(row.facts.directory.as_str()),
                )
            }),
            None if local.is_none_or(|list| !list.gone.contains_key(id)) => view
                .local
                .created
                .get(id)
                .map(|(place, directory)| (*place, false, directory.as_deref())),
            _ => None,
        };
        if let Some((place, running, directory)) = here {
            return Ok(Answer::Object(
                GetKuvoSessionsSessionLocationStatus200Object {
                    place: match place {
                        Place::Engine => GetKuvoSetupReadyStatus200PendingItem::Engine,
                        Place::Bridge => GetKuvoSetupReadyStatus200PendingItem::Agents,
                    },
                    running,
                    tools: with_tools.then(|| tools(directory, &[])),
                },
            ));
        }
    }

    let destination = journal.clone().or_else(|| {
        owner
            .as_ref()
            .and_then(|owner| owner.key)
            .map(|key| encode(&key))
    });
    for (lists, origin) in view.list_origins(now) {
        if destination
            .as_ref()
            .is_some_and(|key| key != &origin.computer.id)
        {
            continue;
        }
        let Some(row) = lists.sessions.iter().find(|row| row.id == id) else {
            continue;
        };
        let features = if origin.standing == Standing::Live {
            view.announced
                .get(&origin.computer.id)
                .and_then(|peer| peer.features.as_deref())
                .unwrap_or_default()
        } else {
            &[]
        };
        return Ok(Answer::Object2(
            GetKuvoSessionsSessionLocationStatus200Object2 {
                on: origin.remote(&row.actions),
                tools: with_tools.then(|| tools(Some(&row.facts.directory), features)),
            },
        ));
    }
    if let Some(peer) = view
        .roster
        .members
        .iter()
        .find(|peer| destination.as_ref() == Some(&encode(&peer.key)))
    {
        let computer = peer.computer();
        let away = if view.lists.contains_key(&peer.key) {
            view.standing(peer.key, now).away()
        } else {
            !owner
                .as_ref()
                .is_some_and(|owner| owner.key == Some(peer.key) && owner.reachable)
        };
        let features = if away {
            &[]
        } else {
            view.announced
                .get(&computer.id)
                .and_then(|peer| peer.features.as_deref())
                .unwrap_or_default()
        };
        let standing = if away {
            Standing::Away { last_seen: None }
        } else {
            Standing::Live
        };
        return Ok(Answer::Object2(
            GetKuvoSessionsSessionLocationStatus200Object2 {
                tools: with_tools.then(|| tools(None, features)),
                on: Origin {
                    computer,
                    standing,
                    here: false,
                }
                .remote(&[SessionAction::Open]),
            },
        ));
    }
    if local.is_none_or(|list| {
        list.pending
            .as_ref()
            .is_some_and(|pending| !pending.is_empty())
    }) || entry.is_some()
        || owner.is_some()
        || journal.is_some()
    {
        Err(Refusal::new(
            StatusCode::SERVICE_UNAVAILABLE,
            CONVERSATION_NOT_READY,
        ))
    } else {
        Err(Refusal::new(StatusCode::NOT_FOUND, CONVERSATION_GONE))
    }
}

fn tools(
    directory: Option<&str>,
    features: &[String],
) -> GetKuvoSessionsSessionLocationStatus200ObjectTools {
    let directory = directory.filter(|directory| directory.starts_with('/'));
    let supports = |feature| features.iter().any(|each| each == feature);
    GetKuvoSessionsSessionLocationStatus200ObjectTools {
        directory: directory.map_or(Nullable::Null(Null), |path| Nullable::Value(path.into())),
        ready: GetKuvoSessionsSessionLocationStatus200ObjectToolsReady {
            files: directory.is_some() && supports("folders.files"),
            changes: directory.is_some() && supports("git.status"),
            browser: supports("browser.view"),
            terminal: supports("pty.socket"),
        },
    }
}

fn computers(view: &View, now: Millis) -> get_kuvo_computers::Status200 {
    use GetKuvoComputersStatus200ComputersItemReach4 as Reach;
    use GetKuvoComputersStatus200ComputersItemVia3 as Via;
    let here = view.roster.local();
    let mut local = computer_row(
        here.id,
        here.name.clone(),
        Reach::Connected,
        Via::Here,
        true,
        &here.name,
    );
    if let Some(endpoint) = &view.endpoint {
        local.address = Some(encode(&endpoint.address));
        local.relay = Some(endpoint.url.clone());
        local.engine = endpoint.hello.engine.clone();
        if endpoint.hello.kind == crate::session::ComputerKind::Mac {
            local.kind = GetKuvoBotsStatus200ElsewhereItemKind3::Mac;
        }
    }
    local.system = view.system.as_deref().map(str::to_owned);
    let features = view.features();
    local.compatibility = computer_compatibility(&features);
    local.features = Some(features.into_iter().map(str::to_owned).collect());
    let mut active = view.at_work();
    local.working = Some(active.len().into());
    local.working_title = Some(working_title(active.len() as u64));
    active.truncate(100);
    local.at_work = Some(active);
    let mut rows = vec![local];
    let mut peers: Vec<_> = view.roster.members.iter().collect();
    peers.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.key.cmp(&b.key))
    });
    for peer in peers {
        let id = encode(&peer.key);
        let said = view.announced.get(&id);
        let heard = said.and_then(|said| said.heard);
        let reach = if view.reach.peer_is_away(&id, heard, now) {
            Reach::Away
        } else if heard.is_none() {
            Reach::Connecting
        } else {
            Reach::Connected
        };
        let mut row = computer_row(id, peer.name.clone(), reach, Via::Relay, false, &here.name);
        row.address = peer.address.as_ref().map(|address| encode(address));
        row.relay = view.endpoint.as_ref().map(|endpoint| endpoint.url.clone());
        if peer.kind == crate::session::ComputerKind::Mac {
            row.kind = GetKuvoBotsStatus200ElsewhereItemKind3::Mac;
        }
        let features = said
            .filter(|said| !said.features_cut)
            .and_then(|said| said.features.as_ref());
        if let Some(features) = features {
            let names: Vec<_> = features.iter().map(String::as_str).collect();
            row.compatibility = computer_compatibility(&names);
            row.features = Some(features.clone());
        }
        if let Some(facts) = said.and_then(|said| said.computer.as_ref()) {
            row.version = facts.version.clone();
            row.engine = facts.engine.clone();
            row.system = facts.system.clone();
            row.kuvo = facts.kuvo.clone();
            let gone = reach == Reach::Away
                && heard.is_none_or(|heard| {
                    now.saturating_sub(heard) > 30 * 60_000
                        || now.saturating_sub(view.reach.away_since(&row.key).unwrap_or(heard))
                            >= 120_000
                });
            if !gone {
                row.agents = facts.agents.clone();
            }
            if let Some(working) = facts.working {
                let working = if gone { 0 } else { working };
                row.working = Some(Number::from(working));
                row.working_title = Some(working_title(working));
                row.at_work = if gone {
                    Some(vec![])
                } else {
                    facts.at_work.clone()
                };
            }
        }
        rows.push(row);
    }
    get_kuvo_computers::Status200 {
        computers: rows,
        fleet_update: None,
        wipe_pending: None,
    }
}

fn computer_compatibility(
    features: &[&str],
) -> Option<GetKuvoComputersStatus200ComputersItemCompatibility> {
    use GetKuvoComputersStatus200ComputersItemCompatibility as Answer;
    use GetKuvoComputersStatus200ComputersItemCompatibilityObjectState2 as State;
    let (state, missing) = match gate::compatibility(features) {
        Compatibility::Open => {
            return Some(Answer::Open(
                GetKuvoComputersStatus200ComputersItemCompatibilityOpen {
                    state: GetKuvoComputersStatus200ComputersItemCompatibilityOpenState::Open,
                },
            ));
        }
        Compatibility::Behind { missing } => (State::Behind, missing),
        Compatibility::Held { missing } => (State::Held, missing),
        Compatibility::Unknown => return None,
    };
    Some(Answer::Object(
        GetKuvoComputersStatus200ComputersItemCompatibilityObject { missing, state },
    ))
}

fn working_title(count: u64) -> String {
    let noun = if count == 1 { "session" } else { "sessions" };
    format!("{count} {noun} at work")
}

fn computer_row(
    key: String,
    name: String,
    reach: GetKuvoComputersStatus200ComputersItemReach4,
    via: GetKuvoComputersStatus200ComputersItemVia3,
    here: bool,
    local_name: &str,
) -> GetKuvoComputersStatus200ComputersItem {
    use GetKuvoComputersStatus200ComputersItemReach4 as Reach;
    let (title, notice, settings_notice) = match reach {
        Reach::Connected => ("Connected", None, None),
        Reach::Connecting => (
            "Connecting",
            Some((
                format!("Connecting to {name}…"),
                format!("{local_name} hasn’t heard from it yet."),
            )),
            Some((
                format!("Connecting to {name}…"),
                format!("Its settings show here once {local_name} hears from it."),
            )),
        ),
        Reach::Away => (
            "Can’t be reached",
            Some((
                format!("{name} can’t be reached"),
                format!(
                    "{local_name} hasn’t heard from it for a while: it may be asleep, off, or offline. It shows here again by itself once it’s back, with what it runs."
                ),
            )),
            Some((
                format!("{name} can’t be reached"),
                format!(
                    "Its settings are kept on {name}, so they can’t be read or changed until {local_name} hears from it again. They show here by themselves once it’s back."
                ),
            )),
        ),
    };
    let note = |(title, detail)| GetKuvoComputersStatus200ComputersItemNotice { title, detail };
    GetKuvoComputersStatus200ComputersItem {
        name,
        key,
        reach,
        via,
        title: title.into(),
        here: here.then_some(BoolLiteral),
        kind: GetKuvoBotsStatus200ElsewhereItemKind3::Machine,
        actions: vec![],
        limit_services: vec![],
        limits: vec![],
        reach_titles: [
            ("connecting", "Connecting…"),
            ("held", "Needs an update"),
            ("away", "Can’t be reached"),
            ("notPaired", "Hasn’t taken this app yet"),
            ("unreached", "Can’t be reached from this app"),
        ]
        .into_iter()
        .map(|(key, title)| (key.into(), title.into()))
        .collect(),
        detail: notice.as_ref().map(|(_, detail)| detail.clone()),
        notice: notice.map(note),
        settings_notice: settings_notice.map(note),
        address: None,
        agents: None,
        app_check: None,
        at_work: None,
        compatibility: None,
        engine: None,
        features: None,
        fleet_update: None,
        id: None,
        kuvo: None,
        relay: None,
        ssh: None,
        ssh_distro: None,
        system: None,
        update: None,
        version: None,
        working: None,
        working_title: None,
        wsl: None,
    }
}

fn register_devices(registry: &mut Registry, devices: Devices) {
    let state = devices.clone();
    registry.register::<get_kuvo_phones::Spec, _, _>(Access::Any, move |call| {
        let asking = call.request.caller.pairing().map(|pairing| pairing.key);
        std::future::ready(Ok(RouteReply::json::<200>(state.list(asking))))
    });
    let state = devices.clone();
    registry.register::<post_kuvo_devices_pair::Spec, _, _>(Access::Any, move |call| {
        let state = state.clone();
        async move {
            use kuvo_server::LocalOrigin;
            if !matches!(
                call.request.caller,
                Caller::Local(Transport::Http {
                    origin: LocalOrigin::App | LocalOrigin::Mac,
                    ..
                })
            ) {
                return Ok(RouteReply::json::<403>(post_kuvo_devices_pair::Status403 {
                    message: "Only Kuvo Code on this computer adds itself to your devices.".into(),
                }));
            }
            let request = serde_json::from_slice(&call.request.body)
                .map_err(|_| devices::Error::InvalidDevice);
            let result = match request {
                Ok(request) => {
                    state
                        .pair(
                            request,
                            crate::setup_lists::system_clock(),
                            &call.request.caller,
                        )
                        .await
                }
                Err(error) => Err(error),
            };
            Ok(match result {
                Ok(pairing) => RouteReply::json::<200>(post_kuvo_devices_pair::Status200 {
                    pairing,
                    paired: BoolLiteral,
                }),
                Err(devices::Error::InvalidDevice) => {
                    RouteReply::json::<400>(post_kuvo_devices_pair::Status400 {
                        message: devices::Error::InvalidDevice.to_string(),
                    })
                }
                Err(devices::Error::WaitingForMain(message)) => {
                    RouteReply::json::<503>(post_kuvo_devices_pair::Status503 { message })
                }
                Err(error) => {
                    let reason = matches!(error, devices::Error::Removed)
                        .then_some(PostKuvoDevicesPairStatus409Reason2::Removed);
                    RouteReply::json::<409>(post_kuvo_devices_pair::Status409 {
                        message: error.to_string(),
                        reason,
                    })
                }
            })
        }
    });
    registry.register::<post_kuvo_phones_remove::Spec, _, _>(Access::Any, move |call| {
        let devices = devices.clone();
        async move {
            use kuvo_server::LocalOrigin;
            let actor = if matches!(
                call.request.caller,
                Caller::Local(Transport::Http {
                    origin: LocalOrigin::App | LocalOrigin::Mac,
                    ..
                })
            ) {
                None
            } else {
                let pairing = call.request.caller.pairing().ok_or_else(|| {
                    Refusal::new(
                        StatusCode::BAD_REQUEST,
                        "Only a device paired with your setup can remove devices.",
                    )
                })?;
                Some(devices::Actor {
                    key: pairing.key,
                    pairing: pairing.id.clone(),
                })
            };
            let request = serde_json::from_slice(&call.request.body).map_err(|_| {
                Refusal::new(
                    StatusCode::BAD_REQUEST,
                    devices::Error::InvalidRemoval.to_string(),
                )
            })?;
            devices
                .remove(request, actor, &call.request.caller)
                .await
                .map_err(|error| {
                    let status = if matches!(error, devices::Error::InvalidRemoval) {
                        StatusCode::BAD_REQUEST
                    } else {
                        StatusCode::CONFLICT
                    };
                    Refusal::new(status, error.to_string())
                })?;
            let answer: post_kuvo_phones_remove::Status200 =
                serde_json::from_str("{}").map_err(internal)?;
            Ok(RouteReply::json::<200>(answer))
        }
    });
}

fn register_attention(registry: &mut Registry) {
    let leases = attention::Leases::start();
    let read = leases.clone();
    registry.register::<get_kuvo_attention::Spec, _, _>(Access::Any, move |_| {
        let read = read.clone();
        async move {
            Ok(RouteReply::json::<200>(get_kuvo_attention::Status200 {
                in_use: read.in_use().await,
                held: 0.into(),
            }))
        }
    });
    registry.register::<post_kuvo_attention::Spec, _, _>(Access::Any, move |call| {
        let leases = leases.clone();
        async move {
            let invalid = || Refusal::new(StatusCode::BAD_REQUEST,
                "Say for how many seconds you're in use (for: 0 for not), and which conversations are on your screen (sessions).");
            let given = serde_json::from_slice(&call.request.body).map_err(|_| invalid())?;
            let seconds = leases.attend(&call.request.caller, given).await.ok_or_else(invalid)?;
            let number = if seconds.fract() == 0.0 { Number::from(seconds as u64) }
                else { Number::from_f64(seconds).expect("checked attention duration") };
            let value = serde_json::from_value(serde_json::json!({"for":number})).map_err(internal)?;
            Ok(RouteReply::json::<200>(value))
        }
    });
}

fn internal(error: impl Into<io::Error>) -> Refusal {
    Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.into().to_string())
}
