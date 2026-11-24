use std::time::Duration;

use serde_json::Value;
use tokio::time::{Instant, sleep_until};

use super::{
    AddError, Completed, Denied, Job, Progress, StepState, address::Address, refuse,
    saved_ssh_destination,
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
        let destination = saved_ssh_destination(files, saved, &link, peer)
            .map_err(|_| missing())?
            .ok_or_else(missing)?;
        if !setup.checkpointed {
            return Err(refuse(409, SETUP_CHANGED));
        }
        Ok(Self {
            destination: Address::read(&destination).map_err(|_| missing())?,
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
    let receipt = serde_json::from_value::<Receipt>(answer.value.clone()).ok();
    if answer.status != 200 {
        if receipt.is_some() {
            job.send(Progress::UninstallCommitted(receipt)).await?;
        }
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
    job.send(Progress::UninstallCommitted(receipt)).await?;
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

fn finished(message: impl Into<String>) -> AddError {
    AddError {
        again: false,
        ..AddError::from(message.into())
    }
}
