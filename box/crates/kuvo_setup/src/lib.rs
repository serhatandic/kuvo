//! Setup authority, signed checkpoints and the shared values accepted by the main computer.

mod actor;
pub mod backup;
mod checkpoint;
mod cleanup;
mod discovery;
mod document;
mod edits;
pub mod installer;
mod preferences;
mod routes;
pub mod seed;
mod ssh;

mod lease;
mod mac_host;
mod model;
mod storage;

use std::{io, path::PathBuf};

use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("This computer couldn't read its setup.")]
    SetupUnavailable,
    #[error("{0}")]
    Installer(String),
    #[error(transparent)]
    InstallerLayout(#[from] kuvo_store::layout::Error),
    #[error("{action} {path}: {source}")]
    File {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("reading {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("unsupported {0} format")]
    Version(&'static str),
    #[error("Personality library format version {0} is unsupported (this build writes 1).")]
    PersonalityVersion(String),
    #[error("invalid {0}")]
    Invalid(&'static str),
    #[error("This device no longer has access to change setup devices.")]
    DevicePermission,
    #[error("This app was removed from your devices.")]
    DeviceRemoved,
    #[error("setup authority changed")]
    Conflict,
    #[error("Kuvo's relay can't be reached. Setup changes will resume when it reconnects.")]
    Fenced,
    #[error("setup preferences exceed their saved capacity")]
    Capacity,
    #[error("relay lease request: {0}")]
    Relay(#[from] reqwest::Error),
    #[error("signed setup document: {0}")]
    Document(#[from] kuvo_wire::documents::Error),
}

fn object(value: Value) -> serde_json::Map<String, Value> {
    match value {
        Value::Object(fields) => fields,
        _ => unreachable!("the setup writer constructs an object"),
    }
}

fn random<const N: usize>() -> Result<[u8; N], Error> {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).map_err(|_| Error::Invalid("system randomness"))?;
    Ok(bytes)
}

pub use actor::{
    ManagedDeviceChange, ManagedDeviceGrant, ManagedDeviceSave, ModelStar, PreparedSetup,
    SetupHandle, SetupRuntime,
};
pub use discovery::locate;
pub use model::{Receipt, SharedKind};

pub use preferences::{project_app_personalities, project_kit_personalities};

pub mod uninstall;
pub mod wipe;
