//! Verify that a staged SSH command still holds the account writer lock.

use crate::Error;

pub(crate) struct LocalReservation {
    pub(crate) marker: std::path::PathBuf,
    pub(crate) lock: std::path::PathBuf,
    token: String,
}

impl LocalReservation {
    pub(crate) fn owns_layout_file(&self, home: &std::path::Path, path: &std::path::Path) -> bool {
        path == self.marker
            || path == self.lock
            || path == home.join(super::fresh::FILE) && super::fresh::Fresh::saved(home).is_some()
    }

    pub(crate) fn check_seed(&self, home: &std::path::Path) -> Result<(), Error> {
        self.check()?;
        if !super::fresh::Fresh::saved(home)
            .is_some_and(|fresh| fresh.phase == super::fresh::Phase::Copying)
        {
            return Err(Error::Invalid("SSH credential copy reservation"));
        }
        Ok(())
    }

    pub(crate) fn copying_for(&self, home: &std::path::Path, setup: &str, primary: &str) -> bool {
        use super::{
            Selection,
            fresh::{Fresh, Phase},
        };

        let scope = Fresh::new(setup, primary, &Selection::default());
        Fresh::saved(home)
            .is_some_and(|fresh| fresh.phase == Phase::Copying && fresh.same_scope(&scope))
    }

    #[cfg(unix)]
    pub(crate) fn inherited(root: &std::path::Path) -> Result<Option<Self>, Error> {
        use std::os::unix::fs::MetadataExt as _;

        let Some(token) = std::env::var_os("KUVO_SSH_ACCOUNT_TOKEN") else {
            return Ok(None);
        };
        let token = token
            .into_string()
            .ok()
            .filter(|value| crate::ssh::is_reservation_token(value))
            .ok_or(Error::Invalid("SSH account reservation"))?;
        let (marker, lock) = if root.file_name().is_some_and(|name| name == ".kuvo-code") {
            (
                root.with_file_name(".kuvo-code-adding"),
                root.with_file_name(".kuvo-code-adding.lock"),
            )
        } else {
            (
                root.join("run/ssh-adding"),
                root.join("run/ssh-adding.lock"),
            )
        };
        let meta = std::fs::symlink_metadata(&lock).map_err(|source| Error::File {
            action: "checking the SSH account lock at",
            path: lock.clone(),
            source,
        })?;
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // The reservation shell passes FD 9. Verify its inode before marking
        // it close-on-exec so a detached resident can't keep the writer lock.
        let matches_lock = meta.is_file()
            && unsafe {
                if libc::fstat(9, stat.as_mut_ptr()) == 0 {
                    let stat = stat.assume_init();
                    stat.st_dev as u64 == meta.dev() && stat.st_ino as u64 == meta.ino()
                } else {
                    false
                }
            };
        if !matches_lock {
            return Err(Error::Invalid("SSH account reservation lock"));
        }
        if unsafe { libc::fcntl(9, libc::F_SETFD, libc::FD_CLOEXEC) } != 0 {
            return Err(Error::File {
                action: "keeping the SSH account lock private at",
                path: marker,
                source: std::io::Error::last_os_error(),
            });
        }
        let account = Self {
            marker,
            lock,
            token,
        };
        account.check()?;
        Ok(Some(account))
    }

    #[cfg(not(unix))]
    pub(crate) fn inherited(_: &std::path::Path) -> Result<Option<Self>, Error> {
        Ok(None)
    }

    pub(crate) fn check(&self) -> Result<(), Error> {
        if std::fs::read_link(&self.marker)
            .is_ok_and(|token| token.as_os_str() == self.token.as_str())
        {
            Ok(())
        } else {
            Err(Error::Installer(
                "The SSH account was cleared for another attempt. Try again.".into(),
            ))
        }
    }
}
