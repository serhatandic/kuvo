//! Integrations (`kuvo/integrations*`), integration keys (`kuvo/keys*`), popular MCP server
//! catalog (`kuvo/mcp/catalog*`), MCP setup snippet parsing, and startup integration upgrades.

pub mod host;
pub mod keys;
pub mod mcp_catalog;
pub mod mcp_setup;
pub mod routes;
pub mod upgrade;

use std::{path::Path, sync::Arc};

use kuvo_server::Registry;

use crate::config::ConfigPaths;

/// All features advertised by the integrations, keys, and MCP catalog routes.
pub const FEATURES: &[&str] = &["integrations", "keys", "mcp.catalog"];

/// Start and register integrations, keys, MCP catalog, and config routes for one box home.
pub fn register(routes: &mut Registry, home: &Path, is_mac: bool, setup: kuvo_setup::SetupHandle, browser: crate::browser::manager::Sites) -> Arc<keys::KeyStore> {
    let config = ConfigPaths::for_home(home, is_mac);
    let catalog_file = upgrade::integrations_file_for(home, is_mac);
    let popular_file = mcp_catalog::catalog_file_for(home);
    upgrade::upgrade_before_engine(&config, &catalog_file, &popular_file);
    let key_store = Arc::new(keys::KeyStore::for_home(home, is_mac));
    let integrations = Arc::new(routes::IntegrationsService::new(
        home,
        config.clone(),
        key_store.clone(),
        browser,
    ));
    crate::config::register(routes, config.clone(), setup.clone());
    crate::git_preferences::register(routes, config.clone());
    keys::register(routes, key_store.clone());
    mcp_catalog::register(routes, config.clone(), setup);
    routes::register(routes, integrations);
    let upgrade_keys = key_store.clone();
    tokio::spawn(async move {
        upgrade::upgrade_integrations(&config, &upgrade_keys, &catalog_file, &popular_file).await;
    });
    key_store
}
