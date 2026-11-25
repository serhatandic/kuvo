//! Marketplace: catalog, installations, skill sources, app connections & loopback MCP relay,
//! bot scopes, and `kuvo/marketplace*` routes.

pub mod bots;
pub mod catalog;
pub mod connections;
pub mod installations;
pub mod routes;
pub mod skill_sources;

use kuvo_os::watch::{FileSet, WatchHub};
use std::{path::Path, sync::Arc};

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
        let config_path = config.config_file.clone();
        let mut setup_changes = setup.clone();
        let mut vault_changes = vault.changes();
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
        let mut watches = FileSet::new(WatchHub::default());
        watches.set_files(vec![
            home.join("box/marketplace/standalone-v1.json"),
            config_path,
            home.join("box/vault/items.json"),
            home.join("box/vault/key.json"),
        ]);
        // Initial installed projections must be available before the first route answer, as before.
        let initial = bg.clone();
        let initial_state = tokio::task::spawn_blocking(move || recheck(&initial, None, true))
            .await
            .ok()
            .flatten();
        let projection = tokio::spawn(async move {
            let mut previous = initial_state;
            loop {
                let force = tokio::select! {
                    _ = watches.changed() => true,
                    result = vault_changes.changed() => if result.is_err() { break } else { true },
                    result = async {
                        match &mut setup_changes { Some(setup) => setup.changed().await, None => std::future::pending().await }
                    } => if !result { setup_changes = None; false } else { false },
                };
                let svc = bg.clone();
                previous = tokio::task::spawn_blocking(move || recheck(&svc, previous, force))
                    .await
                    .ok()
                    .flatten();
            }
        });
        Self {
            service,
            relay,
            projection,
        }
    }

    pub async fn stop(self) {
        self.projection.abort();
        if let Some(relay) = self.relay {
            relay.stop().await;
        }
    }
}

/// An empty installation with no prior projection needs no vault read, skill scan or lock.
/// A previous installation may have left projection files or credentials to clean away, including
/// credentials removed from the setup while this computer's vault was locked.
fn recheck(
    svc: &routes::MarketplaceService,
    previous: Option<installations::Installations>,
    force: bool,
) -> Option<installations::Installations> {
    let state = svc.installations.read().ok()?;
    let nonempty = |state: &installations::Installations| {
        !state.apps.is_empty()
            || !state.skills.is_empty()
            || state
                .removed_skills
                .as_ref()
                .is_some_and(|items| !items.is_empty())
    };
    let changed = previous.as_ref() != Some(&state);
    let prior = svc
        .installations
        .marketplace_dir
        .join("mcp-projection-v1.json")
        .exists()
        || svc
            .installations
            .marketplace_dir
            .join("standalone-v1.json")
            .exists()
        || previous.as_ref().is_some_and(nonempty);
    if (force || changed) && (nonempty(&state) || prior) {
        let _ = svc.reconcile(Some(&state), &[]);
        svc.connections.clean_removed_connections();
    }
    Some(state)
}
