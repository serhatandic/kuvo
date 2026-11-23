use super::Plan;
use crate::Error;
use kuvo_os::process::{Spawn, Stdio};
use serde::Deserialize;
use std::{
    ffi::OsString,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

const HELPER: &str = "bin/Kuvo Helper.app";
const HELPER_PROGRAM: &str = "Contents/MacOS/Kuvo Helper";
const COPY_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const SHORT_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Default, Deserialize)]
struct Reply {
    #[serde(default)]
    reason: String,
    #[serde(default)]
    unreadable: Vec<String>,
}

pub(super) fn copy(plan: &Plan, account: &Path, complete: impl FnOnce()) -> Result<(), Error> {
    let Some(keys) = &plan.keys else {
        return Ok(());
    };
    uncopy(plan, account)?;
    let bundle = plan.release.join(HELPER);
    let helper = bundle.join(HELPER_PROGRAM);
    let arguments = [
        "copy".into(),
        "--home".into(),
        plan.to.as_os_str().to_owned(),
        "--helper".into(),
        bundle.into_os_string(),
    ];
    let (mut copied, mut reply) = call(keys, &arguments, account, COPY_TIMEOUT)?;
    if !copied && matches!(reply.reason.as_str(), "leftovers" | "journal") {
        let action = if reply.reason == "leftovers" {
            "clear"
        } else {
            "uncopy"
        };
        if helper_command(&helper, action, plan, account)?.0 {
            (copied, reply) = call(keys, &arguments, account, COPY_TIMEOUT)?;
        }
    }
    if !copied {
        return Err(super::invalid(match reply.reason.as_str() {
            "locked" => {
                "Your Keychain is locked, so Kuvo can't take this Mac's keys along. Unlock it (log out and back in, or open Keychain Access), then try again."
            }
            "not-allowed" => {
                "Kuvo needs your OK to take this Mac's keys along. Try again: macOS asks once for each key whether kuvo-keys may use it. Each time, enter your Mac's password and click Always Allow."
            }
            _ => "Kuvo couldn't take this Mac's keys along, so nothing was moved. Try again.",
        }));
    }
    complete();
    let (checked, reply) = helper_command(&helper, "check", plan, account)?;
    if !checked {
        let message = match reply.unreadable.len() {
            0 => "Kuvo couldn't check this Mac's keys after copying them, so nothing was moved. Try again.".into(),
            1 => "Kuvo couldn't read one of this Mac's keys back after copying it. Try again.".into(),
            len => format!("Kuvo couldn't read {len} of this Mac's keys back after copying them. Try again."),
        };
        return Err(super::invalid(&message));
    }
    Ok(())
}

pub(super) fn uncopy(plan: &Plan, account: &Path) -> Result<(), Error> {
    if plan.keys.is_none() || !plan.to.join("updates/rehome/keychain.json").is_file() {
        return Ok(());
    }
    let helper = plan.release.join(HELPER).join(HELPER_PROGRAM);
    if !helper_command(&helper, "uncopy", plan, account)?.0 {
        return Err(super::invalid(
            "Kuvo couldn't take back the failed move's Keychain copies. Try again.",
        ));
    }
    Ok(())
}

pub(super) fn delete_originals(plan: &Plan, account: &Path) -> bool {
    let Some(keys) = &plan.keys else { return true };
    call(
        keys,
        &[
            "delete-originals".into(),
            "--home".into(),
            plan.to.as_os_str().to_owned(),
        ],
        account,
        SHORT_TIMEOUT,
    )
    .is_ok_and(|(success, _)| success)
}

fn helper_command(
    helper: &Path,
    action: &str,
    plan: &Plan,
    account: &Path,
) -> Result<(bool, Reply), Error> {
    call(
        helper,
        &[
            "keys".into(),
            action.into(),
            "--home".into(),
            plan.to.as_os_str().to_owned(),
        ],
        account,
        SHORT_TIMEOUT,
    )
}

/// Each signed tool is the immediate child of the process holding the
/// rehome lock. No secret bytes or tool stderr are written into move records.
fn call(
    executable: &Path,
    arguments: &[OsString],
    account: &Path,
    timeout: Duration,
) -> Result<(bool, Reply), Error> {
    let mut command = Spawn::new(
        PathBuf::from(executable),
        [
            ("HOME".into(), account.as_os_str().to_owned()),
            ("PATH".into(), "/usr/bin:/bin:/usr/sbin:/sbin".into()),
        ]
        .into_iter()
        .collect(),
    );
    command.args = arguments.into();
    command.stdout = Stdio::Pipe;
    let mut child = command.spawn()?;
    let output = child
        .stdout
        .take()
        .expect("short Keychain command has a stdout pipe");
    let reader = thread::spawn(move || {
        BufReader::new(output)
            .lines()
            .map_while(Result::ok)
            .filter_map(|line| serde_json::from_str::<Reply>(&line).ok())
            .last()
            .unwrap_or_default()
    });
    let start = Instant::now();
    while !child.has_exited()? {
        if start.elapsed() >= timeout {
            child.stop(Duration::from_secs(1), |_| {})?;
            let _ = reader.join();
            return Err(super::invalid("The Mac's Keychain tool did not finish."));
        }
        thread::sleep(Duration::from_millis(25));
    }
    let status = child.wait()?;
    let reply = reader.join().unwrap_or_default();
    Ok((status.success(), reply))
}
