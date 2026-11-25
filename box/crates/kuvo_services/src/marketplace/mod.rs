//! Marketplace: catalog, installations, skill sources, app connections & loopback MCP relay,
//! bot scopes, and `kuvo/marketplace*` routes.

pub mod bots;
pub mod catalog;
pub mod connections;
pub mod installations;
pub mod routes;
pub mod skill_sources;

use std::{path::Path, sync::Arc, time::Duration};

use kuvo_server::Registry;
use kuvo_setup::SetupHandle;

use crate::{
    config::ConfigPaths,
    skills::{library::SkillLibrary, store::SkillStore},
    vault::Resident as VaultResident,
};

pub const FEATURES: &[&str] = &["marketplace", "marketplace.skills.remove"];

pub struct ResidentMarketplace {
    pub service: Arc<routes::MarketplaceService>,
    relay: Option<connections::RelayServer>,
    projection: tokio::task::JoinHandle<()>,
}

impl ResidentMarketplace {
    #[allow(clippy::too_many_arguments)]
    pub async fn start(
        routes: &mut Registry,
        home: &Path,
        account_home: &Path,
        is_mac: bool,
        computer_name: String,
        setup: Option<SetupHandle>,
        vault: Arc<VaultResident>,
        skills: Arc<SkillStore>,
        skill_library: Arc<SkillLibrary>,
        peers: Arc<dyn routes::MarketplacePeers>,
    ) -> Self {
        let config = ConfigPaths::for_home(home, is_mac);
        let catalog = Arc::new(catalog::CatalogStore::new(home, account_home));
        let installations = Arc::new(installations::InstallationsStore::new(
            home,
            account_home,
            setup,
            skills.clone(),
            config.clone(),
        ));
        let connections = Arc::new(connections::Connections::new(
            installations.clone(),
            vault,
            config.clone(),
        ));
        let relay = match connections.start_relay().await {
            Ok(server) => Some(server),
            Err(err) => {
                eprintln!("Marketplace MCP relay couldn't listen: {err}");
                None
            }
        };
        let service = Arc::new(routes::MarketplaceService::new(
            home,
            account_home,
            is_mac,
            computer_name,
            catalog,
            installations,
            connections,
            skills,
            skill_library,
            config,
            peers,
        ));
        routes::register(routes, service.clone());
        let bg = service.clone();
        // Reading an unlocked Mac vault can call its Helper synchronously.
        // Initial reconciliation runs on the same blocking lane as later rounds.
        let initial = bg.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let _ = initial.reconcile(None, &[]);
            initial.connections.clean_removed_connections();
        })
        .await;
        let projection = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            interval.tick().await;
            loop {
                interval.tick().await;
                let svc = bg.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    let _ = svc.reconcile(None, &[]);
                    svc.connections.clean_removed_connections();
                })
                .await;
            }
        });
        Self { service, relay, projection }
    }

    pub async fn stop(self) {
        self.projection.abort();
        if let Some(relay) = self.relay {
            relay.stop().await;
        }
    }
}
