//! One independent process per agent, and a generation-fenced controller.

mod client;
mod journal;
mod protocol;
mod worker;

pub use client::{
    Controller, ControllerInput, ControllerOutput, Directory, Limits, Replay, RequestId, Start,
    TurnNote,
};
pub use protocol::{Exit, HoldKind, Metadata, Output, Record, Welcome};

use kuvo_os::ipc::{Channel, Endpoint};
use std::{
    ffi::OsString,
    io::{self, Read},
    path::{Path, PathBuf},
    time::Duration,
};

const PROTOCOL: &str = "hold/1";
const RUST_HOLDER: &str = "kuvo-box/1";
const HANDSHAKE: Duration = Duration::from_secs(10);
const SAFE_INTEGER: u64 = (1 << 53) - 1;

pub fn run(args: &[OsString]) -> io::Result<()> {
    let [folder, id, kind] = args else {
        return Err(invalid("Expected hold <folder> <id> <kind>"));
    };
    let folder = PathBuf::from(folder);
    let id = id
        .to_str()
        .filter(|id| valid_id(id))
        .ok_or_else(|| invalid("Invalid holder ID"))?;
    let kind = kind
        .to_str()
        .ok_or_else(|| invalid("Invalid holder kind"))?
        .parse()?;
    let mut bytes = Vec::new();
    io::stdin().take(2_000_001).read_to_end(&mut bytes)?;
    if bytes.len() > 2_000_000 {
        return Err(invalid("Holder launch details are too large"));
    }
    let boot: protocol::Launch =
        serde_json::from_slice(&bytes).map_err(|_| invalid("Invalid holder launch details"))?;
    if boot.key.len() < 16 || boot.spec.command.is_empty() || !folder.is_absolute() {
        return Err(invalid(
            "A holder requires its private key, a command and an absolute folder",
        ));
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(worker::run(&folder, id, kind, boot))
}

fn valid_id(id: &str) -> bool {
    id.len() == 12
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn endpoint(folder: &Path, id: &str) -> io::Result<Endpoint> {
    #[cfg(unix)]
    let key = [0; 32];
    #[cfg(windows)]
    let key = {
        let run = folder
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| invalid("A holder requires its computer's run folder"))?;
        let text = std::fs::read_to_string(run.join("socket.key"))?;
        let mut key = [0; 32];
        hex::decode_to_slice(text.trim(), &mut key)
            .map_err(|_| invalid("Invalid local socket key"))?;
        key
    };
    Endpoint::new(
        &folder.join(format!("{id}.sock")),
        &key,
        Channel::Holder(id.into()),
    )
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
