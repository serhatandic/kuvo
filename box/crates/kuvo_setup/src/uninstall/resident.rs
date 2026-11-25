//! Revoke the local desktop's accepted grant before asking the resident to stop.

use std::{fs, io, path::Path, time::Duration};

use kuvo_os::process::{ProcessIdentity, ProcessState};
use kuvo_server::ServeRecord;
use kuvo_store::{files, home::Home, layout};
use serde_json::json;
use tokio::time::{self, Instant};

use super::{
    DESKTOP_NOT_REVOKED, Files, LINK, Link, Outcome, app_owned, desktop_granted, desktop_key,
    needs_handover, refusal, removal_in_progress, validate_disk,
};

const STOP: &str = "run/uninstall-stop";
const OWNER: &str = "run/.client-owner";
const ACTIVE: &str = "Kuvo Code is still running. Stop it before uninstalling.";

pub async fn stop_live(home: &Home) -> Option<Outcome> {
    stop_live_here(home)
        .await
        .err()
        .map(|error| refusal(&error.to_string()))
}

async fn stop_live_here(home: &Home) -> io::Result<()> {
    if app_owned(home.root())
        || !fs::symlink_metadata(home.root()).is_ok_and(|meta| meta.is_dir())
        || !fs::symlink_metadata(home.run()).is_ok_and(|meta| meta.is_dir())
        || layout::inspect(home).map_err(io::Error::other)? != layout::Layout::Current
    {
        return Ok(());
    }
    let kept = Files::new(home.root().canonicalize()?);
    let Some(record) = kept
        .read::<ServeRecord>("run/serve.json")
        .map_err(io::Error::other)?
    else {
        return Ok(());
    };
    let Some(process) = ProcessIdentity::read(record.pid)? else {
        return Ok(());
    };
    let owner_prefix = format!("owner-{}-", record.pid);
    if !kept
        .names(OWNER)
        .map_err(io::Error::other)?
        .iter()
        .any(|name| name.starts_with(&owner_prefix))
    {
        return Ok(());
    }
    validate_disk(&kept).map_err(io::Error::other)?;
    let original = kept.read::<Link>(LINK).map_err(io::Error::other)?;
    if original
        .as_ref()
        .map(needs_handover)
        .transpose()
        .map_err(io::Error::other)?
        == Some(true)
        || original.as_ref().is_some_and(removal_in_progress)
    {
        return Ok(());
    }
    if kept.path("bin/kuvo-start").exists()
        || kept.path("run/wsl-task").exists()
        || kept.path("run/host.json").exists()
    {
        return Ok(());
    }
    if crate::backup::pre_wipe_backup_enabled(kept.root()).map_err(io::Error::other)? {
        return Err(io::Error::other(
            "The verified pre-wipe backup could not be confirmed.",
        ));
    }

    let position = original
        .as_ref()
        .and_then(|link| link.setup().ok())
        .map(|setup| setup.position.clone());
    let key = desktop_key(&kept);
    if let Some(key) = &key
        && original
            .as_ref()
            .is_some_and(|link| desktop_granted(link, key))
    {
        revoke(&record, key).await?;
    }
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let current = kept.read::<Link>(LINK).map_err(io::Error::other)?;
        if current
            .as_ref()
            .and_then(|link| link.setup().ok())
            .map(|setup| &setup.position)
            != position.as_ref()
            || current
                .as_ref()
                .map(needs_handover)
                .transpose()
                .map_err(io::Error::other)?
                == Some(true)
            || current.as_ref().is_some_and(removal_in_progress)
            || desktop_key(&kept) != key
        {
            return Err(io::Error::other(
                "Kuvo Code couldn’t check who leads this setup right now. Try uninstalling again.",
            ));
        }
        if !key.as_ref().is_some_and(|key| {
            current
                .as_ref()
                .is_some_and(|link| desktop_granted(link, key))
        }) {
            break;
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(DESKTOP_NOT_REVOKED));
        }
        time::sleep(Duration::from_millis(100)).await;
    }
    let current = kept
        .read::<ServeRecord>("run/serve.json")
        .map_err(io::Error::other)?;
    if current
        .is_none_or(|current| current.generation != record.generation || current.pid != process.pid)
        || process.state() != ProcessState::Live
    {
        return Ok(());
    }
    kept.write_bytes("run/off", b"").map_err(io::Error::other)?;
    kept.write(STOP, &process).map_err(io::Error::other)?;
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        match process.state() {
            ProcessState::Stale => return files::remove_file_synced(&kept.path(STOP)),
            ProcessState::Live if Instant::now() < deadline => {
                time::sleep(Duration::from_millis(100)).await
            }
            _ => return Err(io::Error::other(ACTIVE)),
        }
    }
}

async fn revoke(record: &ServeRecord, key: &str) -> io::Result<()> {
    let error = || io::Error::other(DESKTOP_NOT_REVOKED);
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|_| error())?;
    let result = client
        .post(format!(
            "http://127.0.0.1:{}/kuvo/phones/remove",
            record.port
        ))
        .basic_auth("opencode", Some(&record.password))
        .header("x-kuvo-from", "app")
        .header("x-kuvo-route-version", "1")
        .json(&json!({"phones": [{"key": key}]}))
        .send()
        .await
        .map_err(|_| error())?;
    if result.status() != reqwest::StatusCode::OK {
        return Err(error());
    }
    Ok(())
}

pub async fn wait_for_stop(home: &Path) {
    let Ok(Some(own)) = ProcessIdentity::read(std::process::id()) else {
        return std::future::pending().await;
    };
    let kept = Files::new(home.to_owned());
    let mut clock = time::interval(Duration::from_millis(250));
    loop {
        clock.tick().await;
        if kept.read::<ProcessIdentity>(STOP).ok().flatten().as_ref() == Some(&own) {
            return;
        }
    }
}
