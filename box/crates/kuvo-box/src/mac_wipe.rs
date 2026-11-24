use std::{
    env, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use kuvo_os::platform::{Account, Platform};
use kuvo_setup::wipe::{self, AppWipeKeychain, InstallState};

enum Keychain {
    Account,
    Fixture(Option<PathBuf>),
}

impl Keychain {
    fn for_account(account: &Path) -> io::Result<Self> {
        let account = account.canonicalize()?;
        if account == Account::current()?.home.canonicalize()? {
            return Ok(Self::Account);
        }
        let fixture = env::var_os("KUVO_TEST_UNINSTALL_TOOLS")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .map(|path| path.canonicalize())
            .transpose()?
            .filter(|path| path.starts_with(account))
            .map(|path| path.join("security"));
        Ok(Self::Fixture(fixture))
    }
}

impl AppWipeKeychain for Keychain {
    fn clear_app_link_keys(&self, folder: &Path) -> io::Result<()> {
        let Self::Fixture(fixture) = self else {
            return kuvo_services::vault::clear_mac_app_link_keys(folder);
        };
        let Some(security) = fixture else {
            return Ok(());
        };
        let id = folder
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(io::ErrorKind::InvalidInput)?;
        let service = format!("{id}.phone-link");
        for account in ["key", "secret", "sign"] {
            let status = Command::new(security)
                .args(["delete-generic-password", "-s", &service, "-a", account])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?;
            if !status.success() && status.code() != Some(44) {
                return Err(io::Error::other(
                    "The app-owned Box’s identity could not be removed safely.",
                ));
            }
        }
        Ok(())
    }
}

pub fn finish(home: &Path) -> io::Result<bool> {
    let account = Account::current()?.home_folder(Platform::Mac, &env::vars_os().collect())?;
    let keychain = Keychain::for_account(&account)?;
    run(wipe::finish_prepared_app_wipe(home, &keychain))
}

pub fn install_state(folder: &Path, account: &Path) -> io::Result<Option<InstallState>> {
    let keychain = Keychain::for_account(account)?;
    run(wipe::app_install_state(folder, account, &keychain))
}

fn run<T>(work: impl std::future::Future<Output = io::Result<T>>) -> io::Result<T> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(work)
}
