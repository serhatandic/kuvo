//! The built-in browser's records and routes, owned by this computer.
mod history;
pub mod install;
mod settings;
pub mod tools;

pub fn register(routes: &mut kuvo_server::Registry, home: &std::path::Path) {
    settings::register(routes, home);
    history::register(routes, home);
}
