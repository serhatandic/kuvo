//! The owner-only labels command uses the resident's private holder connection.
use std::{
    ffi::OsString,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use kuvo_os::{
    ipc::{Channel, Endpoint},
    platform::{Account, Platform, box_home},
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const ROUTE: &str = "kuvo/work-queue/labels/restore";
const MAX_LIBRARY: u64 = 64 * 1024;

pub fn run(args: &[OsString]) -> Result<ExitCode, Box<dyn std::error::Error>> {
    let invalid = || {
        eprintln!("Usage: kuvo-box box labels restore <file> [--dry-run] [--force]");
        Ok(ExitCode::from(2))
    };
    let mut dry_run = false;
    let mut force = false;
    let mut file = None;
    if args.first().and_then(|arg| arg.to_str()) != Some("restore") {
        return invalid();
    }
    for option in &args[1..] {
        match option.to_str() {
            Some("--dry-run") => dry_run = true,
            Some("--force") => force = true,
            Some(word) if word.starts_with('-') => return invalid(),
            _ if file.is_none() => file = Some(PathBuf::from(option)),
            _ => return invalid(),
        }
    }
    let Some(file) = file else { return invalid() };
    let library = match read_library(&file) {
        Ok(library) => library,
        Err(error) => {
            eprintln!("Couldn't read {} as JSON: {error}", file.display());
            return Ok(ExitCode::FAILURE);
        }
    };
    let account = Account::current()?;
    let platform = Platform::current();
    let home = box_home(&account, platform, &std::env::vars_os().collect())?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let reply = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(30), ask(&home, library, force, dry_run)).await
    });
    let Ok(Ok(reply)) = reply else {
        eprintln!("The local Kuvo box didn't answer. Check that it's running and try again.");
        return Ok(ExitCode::FAILURE);
    };
    let status = reply["status"].as_u64().unwrap_or_default();
    let body = STANDARD.decode(reply["body"].as_str().unwrap_or_default())?;
    let value: Value = serde_json::from_slice(&body).unwrap_or_default();
    if status != 200
        || !value["restored"].is_number()
        || !value["alreadyPresent"].is_number()
        || !value["details"].is_array()
    {
        if let Some(message) = value["message"].as_str() {
            eprintln!("{message}");
        } else {
            eprintln!("Label restore failed ({status}).");
        }
        return Ok(ExitCode::FAILURE);
    }
    if dry_run && value["provisional"] == true {
        println!(
            "The shared label library is syncing. This preview uses the main computer’s current labels."
        );
    }
    let verb = if dry_run {
        "Dry run: would restore"
    } else {
        "Restored"
    };
    println!(
        "{verb} {}, already present {}, conflicts {}.",
        value["restored"], value["alreadyPresent"], value["conflicts"]
    );
    if let Some(details) = value["details"].as_array() {
        for detail in details {
            println!(
                "{}: {}",
                detail["id"].as_str().unwrap_or_default(),
                detail["reason"].as_str().unwrap_or_default()
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn read_library(path: &Path) -> io::Result<Value> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("Use a saved JSON file."));
    }
    let mut bytes = Vec::new();
    file.take(MAX_LIBRARY + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_LIBRARY {
        return Err(io::Error::other("The saved list exceeds 64 KiB."));
    }
    let value: Value = serde_json::from_slice(&bytes)?;
    Ok(if value.is_object() {
        value["labels"].clone()
    } else {
        value
    })
}

async fn ask(home: &Path, library: Value, force: bool, dry_run: bool) -> io::Result<Value> {
    let run = home.join("run");
    let key = fs::read_to_string(run.join("socket.key"))?;
    let socket_key = parse_socket_key(key.trim()).ok_or(io::ErrorKind::InvalidData)?;
    let endpoint = Endpoint::new(
        &run.join("vault.sock"),
        &socket_key,
        Channel::Holder("000000000000".into()),
    )?;
    let mut stream = BufReader::new(endpoint.connect().await?);
    send(&mut stream, json!({"formatVersion":1,"key":key.trim()})).await?;
    let hello = receive(&mut stream).await?;
    if !hello["nonce"].is_string() {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let body = serde_json::to_vec(&json!({"library":library,"force":force}))?;
    send(
        &mut stream,
        json!({
            "formatVersion":1, "method":if dry_run { "GET" } else { "POST" },
            "path":ROUTE,"body":STANDARD.encode(body),"headers":{},
        }),
    )
    .await?;
    receive(&mut stream).await
}

async fn send(stream: &mut BufReader<kuvo_os::ipc::LocalStream>, value: Value) -> io::Result<()> {
    let mut line = serde_json::to_vec(&value)?;
    line.push(b'\n');
    stream.write_all(&line).await
}

async fn receive(stream: &mut BufReader<kuvo_os::ipc::LocalStream>) -> io::Result<Value> {
    use tokio::io::AsyncReadExt;
    let mut bytes = Vec::new();
    stream
        .take(MAX_LIBRARY * 2)
        .read_until(b'\n', &mut bytes)
        .await?;
    serde_json::from_slice(&bytes).map_err(Into::into)
}

fn parse_socket_key(text: &str) -> Option<[u8; 32]> {
    let digits = text.as_bytes();
    if digits.len() != 64 {
        return None;
    }
    let mut key = [0; 32];
    for (index, pair) in digits.chunks_exact(2).enumerate() {
        let high = (pair[0] as char).to_digit(16)? as u8;
        let low = (pair[1] as char).to_digit(16)? as u8;
        key[index] = high * 16 + low;
    }
    Some(key)
}
