//! Run a released-machine catch-up under the same reservation as Add.

use std::{
    collections::HashSet, io, path::PathBuf, process::ExitStatus, sync::atomic::AtomicBool,
    time::Duration,
};

use kuvo_store::home::Home;

use crate::storage::Files;

use super::{
    address::Address,
    connect::reservation::{BUSY, LOST, claim_script, locked_script, owns, release_script},
    saved_machines,
    transport::{self, Connection, Ran},
};

const ROOT: &str = ".kuvo-code";
const PROBE: Duration = Duration::from_secs(30);
const MAX_INPUT: usize = 102 * 1024 * 1024;
const WRONG_MACHINE: i32 = 78;

pub struct SavedMachine {
    files: Files,
    destination: String,
    machine: Machine,
    address: Address,
    connection: Connection,
    token: String,
    settled: bool,
}

pub struct SavedOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

#[derive(PartialEq, Eq)]
struct Machine {
    identity: String,
    addresses: Vec<Address>,
}

impl Machine {
    fn read(files: &Files, destination: &str) -> io::Result<Self> {
        let list = saved_machines(files).map_err(io::Error::other)?;
        let mut found = list
            .iter()
            .filter(|row| row["destination"].as_str() == Some(destination));
        let row = found.next().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "The SSH machine is not saved on this computer.",
            )
        })?;
        if found.next().is_some() {
            return Err(invalid("The saved SSH machine has more than one record."));
        }
        let identity = row["identity"]
            .as_str()
            .filter(|identity| {
                identity.len() == 32
                    && identity
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
            .ok_or_else(|| invalid("The saved SSH machine has no valid identity."))?;
        let primary = Address::read(destination).map_err(invalid)?;
        let mut seen = HashSet::new();
        let addresses = std::iter::once(primary)
            .chain(
                row["otherAddresses"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|value| Address::read(value.as_str()?).ok()),
            )
            .filter(|entry| {
                let text = entry.as_str();
                let host = text.rsplit_once('@').map_or(text, |(_, host)| host);
                seen.insert(host.to_ascii_lowercase())
            })
            .take(9)
            .collect();
        Ok(Self {
            addresses,
            identity: identity.to_owned(),
        })
    }

    fn prove(&self) -> String {
        format!(
            r#"K="$HOME/{ROOT}"
I="$K/machine-id"
[ ! -L "$K" ] && [ -f "$I" ] && [ ! -L "$I" ] || exit {WRONG_MACHINE}
size=$(wc -c < "$I") || exit {WRONG_MACHINE}
[ "$size" -eq 32 ] || [ "$size" -eq 33 ] || exit {WRONG_MACHINE}
[ "$(cat -- "$I")" = '{}' ] || exit {WRONG_MACHINE}"#,
            self.identity
        )
    }
}

/// Find this computer's saved SSH machine and keep its Unix account reserved across the callback.
/// Each command rechecks the machine's identity and uses strict host key checking.
pub async fn with_saved_machine<T>(
    home: &Home,
    destination: &str,
    work: impl AsyncFnOnce(&mut SavedMachine) -> io::Result<T>,
) -> io::Result<T> {
    let files = Files::new(home.root().to_owned());
    let machine = Machine::read(&files, destination)?;
    let token = super::connect::reservation::new_token()?;
    let mut selected = None;
    for address in &machine.addresses {
        let connection = open(home.root().to_owned(), address.clone()).await?;
        let probe = format!("{}\nexit 0", machine.prove());
        let checked = run(&connection, &probe, Vec::new(), PROBE).await;
        if checked.is_ok_and(|reply| reply.status.success()) {
            selected = Some((address.clone(), connection));
            break;
        }
    }
    let (address, connection) = selected.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "Could not reach the saved SSH machine with its known host key and identity.",
        )
    })?;
    let claiming = claim_script(ROOT, &token, None, &machine.prove());
    let claim = run(&connection, &claiming, Vec::new(), PROBE).await?;
    if !claim.status.success() {
        return Err(io::Error::other(
            "The saved SSH machine's account could not be reserved.",
        ));
    }
    let mut reserved = SavedMachine {
        files,
        destination: destination.to_owned(),
        machine,
        address,
        connection,
        token,
        settled: true,
    };
    let result = work(&mut reserved).await;
    if reserved.settled {
        let released = run(
            &reserved.connection,
            &release_script(ROOT, &reserved.token),
            Vec::new(),
            PROBE,
        )
        .await;
        if !released.is_ok_and(|reply| reply.status.success()) && result.is_ok() {
            return Err(io::Error::other(
                "The SSH operation finished, but the account reservation could not be released.",
            ));
        }
    }
    result
}

impl SavedMachine {
    pub fn identity(&self) -> &str {
        &self.machine.identity
    }

    pub fn address(&self) -> &str {
        self.address.as_str()
    }

    pub async fn run(
        &mut self,
        script: &str,
        input: Vec<u8>,
        timeout: Duration,
    ) -> io::Result<SavedOutput> {
        if input.len() > MAX_INPUT {
            return Err(invalid(
                "The SSH update input exceeds the release size limit.",
            ));
        }
        if !self.settled {
            return Err(io::Error::other(
                "The previous SSH write did not report its outcome.",
            ));
        }
        if Machine::read(&self.files, &self.destination)? != self.machine {
            return Err(io::Error::other(
                "The saved SSH machine changed before the operation.",
            ));
        }
        let checked = format!(
            "{} || exit {LOST}\n{}\n(\n{script}\n)",
            owns(&self.token),
            self.machine.prove()
        );
        self.settled = false;
        let reply = run(
            &self.connection,
            &locked_script(ROOT, 5, &checked),
            input,
            timeout,
        )
        .await?;
        let status = reply
            .status
            .code()
            .filter(|code| *code != 255)
            .ok_or_else(|| io::Error::other("The SSH operation did not report its outcome."))?;
        self.settled = true;
        match status {
            WRONG_MACHINE => Err(io::Error::other(
                "The saved SSH machine's identity changed during the operation.",
            )),
            BUSY | LOST => Err(io::Error::other(
                "The saved SSH machine's account reservation changed.",
            )),
            _ => Ok(SavedOutput {
                status: reply.status,
                stdout: reply.stdout,
                stderr: reply.stderr,
            }),
        }
    }
}

async fn open(home: PathBuf, address: Address) -> io::Result<Connection> {
    tokio::task::spawn_blocking(move || Connection::open(home, address))
        .await
        .map_err(io::Error::other)?
}

async fn run(
    connection: &Connection,
    script: &str,
    input: Vec<u8>,
    timeout: Duration,
) -> io::Result<Ran> {
    let connection = connection.clone();
    let command = transport::sh(script);
    tokio::task::spawn_blocking(move || {
        connection.run(&command, input, timeout, &AtomicBool::new(false))
    })
    .await
    .map_err(io::Error::other)?
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(all(test, unix))]
mod tests {
    use std::{fs, os::unix::fs::symlink, path::Path, process::Command};

    use serde_json::json;

    use super::*;

    #[test]
    fn saved_machine_claim_checks_identity_and_retains_existing_writer() {
        let scratch = tempfile::tempdir().unwrap();
        let home = scratch.path();
        let local = home.join("local");
        fs::create_dir(&local).unwrap();
        let files = Files::new(local);
        let identity = "6a447d2973b4732897b2298f4b3b11a3";
        files
            .write(
                "machines.json",
                &json!([{
                    "destination": "alice@studio", "identity": identity,
                    "otherAddresses": ["ALICE@STUDIO", "-malformed", "workshop"]
                }]),
            )
            .unwrap();
        let machine = Machine::read(&files, "alice@studio").unwrap();
        assert_eq!(
            machine
                .addresses
                .iter()
                .map(Address::as_str)
                .collect::<Vec<_>>(),
            ["alice@studio", "workshop"]
        );
        let run = |script: &str| {
            Command::new("/bin/sh")
                .args(["-c", script])
                .env("HOME", home)
                .output()
                .unwrap()
                .status
        };
        let marker = home.join(".kuvo-code-adding");
        let root = home.join(ROOT);
        let path = root.join("machine-id");
        fs::create_dir(&root).unwrap();
        fs::write(&path, "not its machine id").unwrap();
        let token = super::super::connect::reservation::new_token().unwrap();
        let claim = claim_script(ROOT, &token, None, &machine.prove());
        assert_eq!(run(&claim).code(), Some(WRONG_MACHINE));
        assert!(marker.symlink_metadata().is_err());

        let previous = super::super::connect::reservation::new_token().unwrap();
        symlink(&previous, &marker).unwrap();
        fs::write(&path, format!("{identity}\n")).unwrap();
        assert!(!run(&claim).success());
        assert_eq!(fs::read_link(&marker).unwrap(), Path::new(&previous));
        fs::remove_file(&marker).unwrap();
        assert!(run(&claim).success());
        assert_eq!(fs::read_link(&marker).unwrap(), Path::new(&token));

        fs::remove_file(&path).unwrap();
        let outside = home.join("independent-id");
        fs::write(&outside, identity).unwrap();
        symlink(outside, &path).unwrap();
        let check = locked_script(
            ROOT,
            5,
            &format!(
                "{} || exit {LOST}\n{}\nexit 0",
                owns(&token),
                machine.prove()
            ),
        );
        assert_eq!(run(&check).code(), Some(WRONG_MACHINE));
        assert!(run(&release_script(ROOT, &token)).success());
        assert!(marker.symlink_metadata().is_err());
    }
}
