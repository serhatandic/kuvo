use std::time::Duration;

use serde_json::Value;
use tokio::time::{Instant, sleep_until};

use super::{
    AddError, Completed, Denied, Job, PHONE_LINK, Progress, StepState, address::Address,
    kept_hosts_files, refuse,
};
use crate::{
    Receipt,
    actor::{Command, LINK, WipeProgress, wipe_progress},
    model::{Link, Position, safe_token},
    storage::Files,
};

const NO_ADDRESS: &str = "That isn’t a computer this one set up over SSH.";
const SETUP_CHANGED: &str = "The setup changed. Refresh it and try again.";

pub(super) struct Target {
    pub(super) destination: Address,
    pub(super) name: String,
    key: String,
    position: Position,
}

impl Target {
    pub(super) fn read(files: &Files, saved: &[Value], key: &str) -> Result<Self, Denied> {
        let missing = || refuse(404, NO_ADDRESS);
        if !safe_token(key, 43, 43) {
            return Err(missing());
        }
        let link = files
            .read::<Link>(LINK)
            .ok()
            .flatten()
            .ok_or_else(missing)?;
        let setup = link.setup().map_err(|_| missing())?;
        let roster = link.member_list().map_err(|_| missing())?;
        let peer = roster
            .boxes
            .iter()
            .find(|peer| peer.key == key && key != setup.position.primary)
            .ok_or_else(missing)?;
        let kept = kept_hosts_files(files)
            .read::<Value>(PHONE_LINK)
            .map_err(|_| missing())?;
        let hosts = kept
            .as_ref()
            .and_then(|whole| whole.get("machines"))
            .and_then(Value::as_object);
        let destination = saved
            .iter()
            .find_map(|record| {
                let destination = record["destination"].as_str()?;
                let bound = hosts
                    .and_then(|hosts| hosts.get(destination))
                    .and_then(|host| host["key"].as_str());
                if let Some(bound) = bound {
                    return (bound == key).then_some(destination);
                }
                // Earlier Rust enrollments saved the address in the roster. The
                // local issuer's retained claim ties that address to this peer.
                (peer.extra.get("id").and_then(Value::as_str) == Some(destination)
                    && link.installer_invites.values().any(|invite| {
                        invite
                            .claim
                            .as_ref()
                            .is_some_and(|claim| claim.matches(peer))
                    }))
                .then_some(destination)
            })
            .ok_or_else(missing)?;
        if !setup.checkpointed {
            return Err(refuse(409, SETUP_CHANGED));
        }
        Ok(Self {
            destination: Address::read(destination).map_err(|_| missing())?,
            name: peer.name.clone(),
            key: key.into(),
            position: setup.position.clone(),
        })
    }
}

pub(super) async fn run(job: &Job, target: Target) -> Result<Completed, AddError> {
    job.admit().await?;
    job.step("removing", StepState::Now, "").await?;
    let answer = job
        .setup
        .request_command(Command::SshUninstall {
            key: target.key.clone(),
            position: target.position.clone(),
            by: job.by.clone(),
        })
        .await
        .map_err(|error| error.to_string())?;
    if answer.status != 200 {
        return Err(answer.value["message"]
            .as_str()
            .unwrap_or("The removal couldn’t be saved.")
            .into());
    }
    let operation = answer.value["removal"]["operation"]
        .as_str()
        .filter(|op| safe_token(op, 22, 43))
        .ok_or("The removal couldn’t be saved.")?
        .to_owned();
    let receipt = serde_json::from_value::<Receipt>(answer.value).ok();
    job.send(Progress::UninstallCommitted(receipt)).await?;
    if let Err(error) = forget_address(&job.files, &target) {
        eprintln!("Removal machines-list cleanup failed: {error}");
        return Err(
            "The computer’s access was removed, but the saved machines list couldn’t be updated."
                .into(),
        );
    }
    job.send(Progress::Step(
        "removing",
        StepState::Now,
        "Access was removed. Waiting for confirmation that the data was erased.".into(),
    ))
    .await?;

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let seen = job
            .files
            .read::<Link>(LINK)
            .ok()
            .flatten()
            .map_or(WipeProgress::Pending, |link| {
                wipe_progress(&link, &target.key, &operation)
            });
        match seen {
            WipeProgress::Delivered => break,
            WipeProgress::Closed => {
                return Err(finished(
                    "The removal was closed without confirmation that the data was erased.",
                ));
            }
            WipeProgress::Review(error) => return Err(finished(error)),
            WipeProgress::Pending => {}
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(finished(
                "Access was removed. This computer’s wipe is still pending in Computers.",
            ));
        }
        sleep_until((now + Duration::from_millis(250)).min(deadline)).await;
    }
    job.send(Progress::Step("forgetting", StepState::Now, String::new()))
        .await?;
    Ok(Completed::Uninstalled)
}

fn forget_address(files: &Files, target: &Target) -> Result<(), crate::Error> {
    let destination = target.destination.as_str();
    let mut saved = super::saved_machines(files)?;
    if let Some(kept) = kept_hosts_files(files).read::<Value>(PHONE_LINK)?
        && kept["machines"][destination]["key"]
            .as_str()
            .is_some_and(|key| key != target.key)
    {
        return Ok(());
    }
    let before = saved.len();
    saved.retain(|row| row["destination"] != destination);
    if saved.len() != before {
        files.write("machines.json", &saved)?;
    }
    Ok(())
}

fn finished(message: impl Into<String>) -> AddError {
    AddError {
        again: false,
        ..AddError::from(message.into())
    }
}
