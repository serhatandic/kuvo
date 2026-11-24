//! Retire the app-owned Mac link identity at the phase chosen by the wipe journal.
use std::{
    env, fs, io,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use kuvo_os::{platform::Account, process::Spawn};

const ACCOUNTS: [&str; 3] = ["key", "secret", "sign"];
const REFUSED: &str = "The app-owned Box’s identity could not be removed safely.";

pub fn clear_mac_app_link_keys(app_folder: &Path) -> io::Result<()> {
    let support = Account::current()?.home.join("Library/Application Support");
    clear_in(app_folder, &support, None)
}

fn clear_in(app_folder: &Path, support: &Path, keychain: Option<&Path>) -> io::Result<()> {
    let invalid = || io::Error::new(io::ErrorKind::PermissionDenied, REFUSED);
    let id = app_folder
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(invalid)?;
    let valid_id = id.strip_prefix("run.kuvo.code").is_some_and(|suffix| {
        suffix.is_empty()
            || suffix.strip_prefix('.').is_some_and(|parts| {
                parts.split('.').all(|part| {
                    !part.is_empty()
                        && part
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                })
            })
    });
    if !valid_id
        || !app_folder.is_absolute()
        || app_folder.parent().ok_or_else(invalid)?.canonicalize()? != support.canonicalize()?
    {
        return Err(invalid());
    }
    match fs::symlink_metadata(app_folder) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err(invalid()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let service = format!("{id}.phone-link");
    for account in ACCOUNTS {
        let mut command = Spawn::new(PathBuf::from("/usr/bin/security"), env::vars_os().collect());
        command.args = ["delete-generic-password", "-s", &service, "-a", account]
            .into_iter()
            .map(Into::into)
            .collect();
        command
            .args
            .extend(keychain.map(|file| file.as_os_str().to_owned()));
        let mut child = command.spawn()?;
        let began = Instant::now();
        while !child.has_exited()? {
            if began.elapsed() >= Duration::from_secs(30) {
                let _ = child.stop(Duration::from_secs(1), |_| {});
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "The Keychain removal timed out.",
                ));
            }
            thread::sleep(Duration::from_millis(25));
        }
        let result = child.wait()?;
        // security reports errSecItemNotFound as 44. A previous attempt may
        // already have removed some or all of the three items.
        if !result.success() && result.code() != Some(44) {
            return Err(io::Error::other(format!(
                "The app-owned identity could not be removed from the Keychain ({result})."
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    use zeroize::Zeroizing;

    fn security() -> Command {
        let mut command = Command::new("/usr/bin/security");
        command.stdout(Stdio::null()).stderr(Stdio::null());
        command
    }

    #[test]
    fn app_wipe_clears_only_the_validated_identity_and_can_retry() {
        let folder = tempfile::tempdir().unwrap();
        let support = folder.path().join("Library/Application Support");
        fs::create_dir_all(&support).unwrap();
        let app = support.join("run.kuvo.code.wipe-fixture");
        let chain = folder.path().join("test.keychain-db");
        let status = security()
            .args(["create-keychain", "-p", ""])
            .arg(&chain)
            .status()
            .unwrap();
        assert!(status.success());
        struct RemoveKeychain(PathBuf);
        impl Drop for RemoveKeychain {
            fn drop(&mut self) {
                let _ = security().arg("delete-keychain").arg(&self.0).status();
            }
        }
        let _remove = RemoveKeychain(chain.clone());
        let service = "run.kuvo.code.wipe-fixture.phone-link";
        let member = "run.kuvo.code.member-fixture.phone-link";
        let neighbor = "run.kuvo.code.another-fixture.phone-link";
        let exists = |service: &str, account: &str| {
            security()
                .args(["find-generic-password", "-s", service, "-a", account])
                .arg(&chain)
                .status()
                .unwrap()
                .success()
        };
        for (name, account) in ACCOUNTS
            .into_iter()
            .map(|account| (service, account))
            .chain(ACCOUNTS.into_iter().map(|account| (member, account)))
            .chain([(neighbor, "key")])
        {
            let mut bytes = Zeroizing::new([0u8; 24]);
            getrandom::fill(bytes.as_mut()).unwrap();
            let value = Zeroizing::new(hex::encode(bytes.as_ref()));
            let line = Zeroizing::new(format!(
                "add-generic-password -U -s {name} -a {account} -X {} \"{}\"\n",
                *value,
                chain.display()
            ));
            let mut child = security().arg("-i").stdin(Stdio::piped()).spawn().unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(line.as_bytes())
                .unwrap();
            assert!(child.wait().unwrap().success() && exists(name, account));
        }

        let wrong_parent = folder.path().join("outside");
        fs::create_dir(&wrong_parent).unwrap();
        assert!(
            clear_in(
                &wrong_parent.join(app.file_name().unwrap()),
                &support,
                Some(&chain)
            )
            .is_err()
        );
        assert!(ACCOUNTS.into_iter().all(|account| exists(service, account)));

        // The last-computer path clears before detach, and member retries
        // may reach the Keychain again with the directory already absent.
        fs::create_dir(&app).unwrap();
        clear_in(&app, &support, Some(&chain)).unwrap();
        assert!(app.is_dir());
        let detached = support.join("run.kuvo.code.member-fixture");
        assert!(!detached.exists());
        clear_in(&detached, &support, Some(&chain)).unwrap();
        clear_in(&detached, &support, Some(&chain)).unwrap();
        assert!(
            ACCOUNTS
                .into_iter()
                .all(|account| !exists(service, account))
        );
        assert!(ACCOUNTS.into_iter().all(|account| !exists(member, account)));
        assert!(exists(neighbor, "key"));
    }
}
