//! The resident's app registration and its public version announcement.

use std::{
    io,
    sync::{Arc, Weak},
};

use http::StatusCode;
use kuvo_keeper::{fleet, update_status};
use kuvo_peers::setup_lists::{LocalVersions, ResidentPeers};
use kuvo_protocol::routes::{
    get_kuvo_box_app as get, post_kuvo_box_app as post, post_kuvo_box_app_open as open,
};
use kuvo_server::{Access, Caller, LocalOrigin, Refusal, Registry, RouteReply, Transport};
use kuvo_services::{config, mac_app};
use kuvo_store::{home::Home, release_version};
use serde_json::Value;
use tokio::sync::Mutex;

struct App {
    home: Home,
    is_mac: bool,
    own_mac: bool,
    peers: Weak<ResidentPeers>,
    writer: Mutex<()>,
    opening: Mutex<()>,
}

impl App {
    async fn publish(&self) -> io::Result<()> {
        let selected = update_status::version(&self.home).ok().flatten();
        let running = fleet::installed(&self.home)
            .ok()
            .and_then(|(version, _)| version)
            .filter(|version| release_version::order(version).is_some());
        let version = if self.is_mac {
            mac_app::legacy_version(self.home.root())
        } else {
            selected.clone()
        };
        let app_check = if self.is_mac {
            selected
                .as_deref()
                .and_then(|version| mac_app::app_check(&self.home.run(), version))
        } else {
            None
        };
        let peers = self.peers.upgrade().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotConnected,
                "The computer list is not running.",
            )
        })?;
        peers
            .announce_versions(LocalVersions {
                version,
                running,
                app_check,
                is_mac: self.is_mac,
            })
            .await
    }

    fn own_home(&self) -> Result<(), Refusal> {
        if self.own_mac {
            Ok(())
        } else {
            Err(Refusal::new(StatusCode::CONFLICT, mac_app::ONLY_MAC))
        }
    }
}

fn internal(error: impl std::fmt::Display) -> Refusal {
    Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

fn now() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

pub async fn register(
    routes: &mut Registry,
    home: Home,
    is_mac: bool,
    peers: Arc<ResidentPeers>,
) -> io::Result<()> {
    let app = Arc::new(App {
        own_mac: config::is_own_home_mac(home.root(), is_mac),
        home,
        is_mac,
        peers: Arc::downgrade(&peers),
        writer: Mutex::new(()),
        opening: Mutex::new(()),
    });
    app.publish().await?;
    let read = app.clone();
    routes.register::<get::Spec, _, _>(Access::Any, move |_| {
        let app = read.clone();
        async move {
            app.own_home()?;
            Ok(RouteReply::json::<200>(mac_app::status(
                &app.home.run(),
                now(),
            )))
        }
    });
    let registration = app.clone();
    routes.register::<post::Spec, _, _>(Access::Any, move |call| {
        let app = registration.clone();
        async move {
            app.own_home()?;
            let origin = match call.request.caller {
                Caller::Local(Transport::Http { origin, .. }) => origin,
                Caller::Local(Transport::Ssh { .. }) => LocalOrigin::Mac,
                _ => LocalOrigin::Direct,
            };
            let given: Value = serde_json::from_slice(&call.request.body).unwrap_or_default();
            // Once a registration begins, its durable record and its local
            // publication settle together even if the caller disconnects.
            tokio::spawn(async move {
                let _writer = app.writer.lock().await;
                let run = app.home.run();
                tokio::task::spawn_blocking(move || mac_app::register(&run, &given, origin, now()))
                    .await
                    .map_err(internal)??;
                app.publish().await.map_err(internal)?;
                Ok(RouteReply::json::<200>(mac_app::status(
                    &app.home.run(),
                    now(),
                )))
            })
            .await
            .map_err(internal)?
        }
    });
    routes.register::<open::Spec, _, _>(Access::Local, move |call| {
        let app = app.clone();
        async move {
            app.own_home()?;
            if !matches!(
                call.request.caller,
                Caller::Local(
                    Transport::Http {
                        origin: LocalOrigin::App | LocalOrigin::Mac,
                        ..
                    } | Transport::Http { internal: true, .. }
                        | Transport::Ssh { .. }
                )
            ) {
                return Err(Refusal::new(
                    StatusCode::FORBIDDEN,
                    "Only Kuvo Code on this Mac opens its app.",
                ));
            }
            let body: Value = serde_json::from_slice(&call.request.body).unwrap_or_default();
            if let Some(token) = body["gone"].as_str() {
                mac_app::forget_tools(&app.home.run(), token, now()).map_err(internal)?;
            }
            tokio::spawn(async move {
                let _opening = app.opening.lock().await;
                mac_app::open_browser(app.home.root())
                    .await
                    .map_err(|message| Refusal::new(StatusCode::SERVICE_UNAVAILABLE, message))?;
                Ok(RouteReply::json::<200>(mac_app::status(
                    &app.home.run(),
                    now(),
                )))
            })
            .await
            .map_err(internal)?
        }
    });
    Ok(())
}
