use std::{
    env,
    fs::{self, OpenOptions},
    io::{self, Read},
    os::unix::fs::OpenOptionsExt as _,
    os::unix::process::CommandExt as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use kuvo_os::{
    platform::{Account, Platform},
    process::{ProcessIdentity, ProcessState},
};
use kuvo_store::files;
use serde::Deserialize;
use serde_json::Value;
use tokio::time::{self, Instant};

use super::member::{Completion, MemberWipe, Report};
use super::{
    AppWipeKeychain, AppWipeKind, Files, Home, Journal, LINK, Layout, LocalWipe, OwnedLock, Phase,
    UNSUPPORTED, app_boundary, erase_approved, identity, layout, parent, private, read_journal,
    uninstall, validate_disk,
};
use crate::model::Link;

const FAILED: &str = "Kuvo Code’s app-owned background work could not be stopped.";
const STOP_WITHIN: Duration = Duration::from_secs(20);
const ROOT_SETTINGS: &[&str] = &[
    "KUVO_BOTS",
    "KUVO_WAKES",
    "KUVO_ENGINE_DATA",
    "KUVO_ENGINE_DB_FOLDER",
    "KUVO_ENGINE_CONFIG",
    "KUVO_AGENTS_HOME",
    "KUVO_GROUPS",
    "KUVO_ARTIFACTS",
    "KUVO_MAC_ATTACHMENTS",
    "KUVO_CHATS",
    "KUVO_KEYS",
    "KUVO_INTEGRATIONS",
];
const SETTINGS: &str = "run/app-wipe-roots.json";

pub(super) struct Target {
    root: PathBuf,
    account: PathBuf,
    real_account: bool,
}

impl Target {
    pub(super) fn for_box(home: &Path) -> io::Result<Option<Self>> {
        if env::var_os("KUVO_BOX")
            .as_deref()
            .is_none_or(|kind| kind != "mac")
            || home.file_name().is_none_or(|name| name != "box")
        {
            return Ok(None);
        }
        let account = Account::current()?;
        let configured = account
            .home_folder(Platform::Mac, &env::vars_os().collect())?
            .canonicalize()?;
        Self::for_folder(parent(home)?, &configured)
    }

    pub(super) fn for_folder(folder: &Path, account: &Path) -> io::Result<Option<Self>> {
        let account = account.canonicalize()?;
        let Some(root) = app_boundary(folder, &account)? else {
            return Ok(None);
        };
        let real_account = account == Account::current()?.home.canonicalize()?;
        Ok(Some(Self {
            root,
            account,
            real_account,
        }))
    }

    fn receipts(&self) -> io::Result<PathBuf> {
        let id = self
            .root
            .file_name()
            .and_then(|id| id.to_str())
            .ok_or_else(|| io::Error::other(UNSUPPORTED))?;
        Ok(parent(&self.root)?.join(format!(".{id}-removal-receipts")))
    }

    fn marker(&self, kind: AppWipeKind) -> io::Result<PathBuf> {
        Ok(self.receipts()?.join(kind.marker()))
    }

    pub(super) fn pending(&self) -> io::Result<bool> {
        for kind in [AppWipeKind::LastComputer, AppWipeKind::Member] {
            if read_journal(&self.marker(kind)?)?.is_some_and(|journal| journal.phase.is_some()) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(super) fn launch_worker(&self) -> io::Result<()> {
        let mut worker = Command::new(env::current_exe()?);
        worker
            .arg("wipe-member")
            .current_dir(parent(&self.root)?)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // The old app's keeper can be stopped with its process group. The wipe
        // continues in a separate session, after the resident releases its home.
        unsafe {
            worker.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        worker.spawn()?;
        Ok(())
    }

    pub(super) async fn install_state(
        &self,
        keychain: &dyn AppWipeKeychain,
    ) -> io::Result<Option<super::InstallState>> {
        use super::{InstallState, ReinstallReceipt, pending_hint};
        let kept = self.box_files();
        if kept.read::<Value>(LINK).ok().flatten().is_some_and(|link| {
            link["setup"]["role"] == "member"
                && link["setup"]["id"].is_string()
                && link["setup"]["id"] == link["removalSource"]["setup"]
                && link["removalSource"]["review"].is_string()
        }) {
            return Ok(Some(InstallState::MemberReview));
        }
        if let Err(error) = self.finish(keychain, Report::Installer).await {
            eprintln!("finishing the earlier app wipe: {error}");
        }
        if self.pending()? || pending_hint(&kept) {
            return Ok(Some(InstallState::Pending));
        }
        if self.root.exists() {
            return Ok(None);
        }
        Ok(
            read_journal(&self.marker(AppWipeKind::LastComputer)?)?.map(|journal| {
                InstallState::Erased(ReinstallReceipt {
                    setup: journal.setup,
                    operation: journal.operation,
                })
            }),
        )
    }

    fn lock(&self) -> io::Result<OwnedLock> {
        let mut lock = self.root.as_os_str().to_owned();
        lock.push("-wipe.lock");
        OwnedLock::acquire(Path::new(&lock), Duration::ZERO)
    }

    pub(super) fn box_files(&self) -> Files {
        Files::new(self.root.join("box"))
    }

    pub(super) fn root(&self) -> &Path {
        &self.root
    }

    fn save(&self, kind: AppWipeKind, journal: &Journal) -> io::Result<()> {
        let receipts = self.receipts()?;
        files::create_private_dirs(&receipts)?;
        private(&receipts, true)?;
        files::write_atomic_through(
            &self.marker(kind)?,
            &serde_json::to_vec(journal)?,
            parent(&self.root)?,
        )
    }

    pub(super) fn check_layout(
        &self,
        settings: &std::collections::BTreeMap<String, PathBuf>,
    ) -> io::Result<()> {
        let kept = self.box_files();
        let home = Home::new(kept.root().to_owned())?;
        if identity(&self.root)?.is_none()
            || !fs::symlink_metadata(home.root())?.is_dir()
            || !fs::symlink_metadata(home.run())?.is_dir()
            || layout::inspect(&home).map_err(io::Error::other)? != Layout::Current
        {
            return Err(io::Error::other(UNSUPPORTED));
        }
        for path in settings.values() {
            let path = std::path::absolute(path)?;
            if !resolved_data(&path)?.starts_with(&self.root) {
                return Err(io::Error::other(UNSUPPORTED));
            }
        }
        for (setting, fallback) in [
            ("KUVO_BOTS", home.bots()),
            ("KUVO_WAKES", home.wakes()),
            ("KUVO_ENGINE_DATA", home.engine_data()),
            ("KUVO_AGENTS_HOME", self.root.join("agents")),
            ("KUVO_GROUPS", self.root.join("groups")),
            ("KUVO_ARTIFACTS", home.root().join("artifacts")),
            ("KUVO_CHATS", self.root.join("Chats")),
            ("", home.attachments()),
        ] {
            if !settings.contains_key(setting) && !resolved_data(&fallback)?.starts_with(&self.root)
            {
                return Err(io::Error::other(UNSUPPORTED));
            }
        }
        Ok(())
    }

    pub(super) fn settings() -> io::Result<std::collections::BTreeMap<String, PathBuf>> {
        ROOT_SETTINGS
            .iter()
            .filter_map(|name| {
                env::var_os(name)
                    .filter(|value| !value.is_empty())
                    .map(|value| (*name, value))
            })
            .map(|(name, value)| Ok((name.into(), std::path::absolute(PathBuf::from(value))?)))
            .collect()
    }

    pub(super) fn prepare(
        &self,
        kind: AppWipeKind,
        decision: LocalWipe,
        member: Option<&MemberWipe>,
        accepted: &Link,
        settings: &std::collections::BTreeMap<String, PathBuf>,
    ) -> io::Result<()> {
        let kept = self.box_files();
        if helper_keeps_keys(&kept)
            && accepted
                .extra
                .get("localWipe")
                .and_then(|value| value["hostKeysCleared"].as_bool())
                != Some(true)
        {
            return Err(io::Error::other(
                "The host still holds Kuvo integration keys.",
            ));
        }
        let _guard = self.lock()?;
        self.check_layout(settings)?;
        let source = identity(&self.root)?
            .ok_or_else(|| io::Error::other("The folder to erase is missing."))?;
        let journal = Journal {
            setup: decision.setup,
            operation: decision.operation,
            source: Some(source),
            phase: Some(Phase::Stopping),
            format_version: 1,
        };
        if read_journal(&self.marker(kind)?)?.is_some_and(|kept| {
            kept.phase.is_some()
                && (kept.phase != journal.phase
                    || kept.setup != journal.setup
                    || kept.operation != journal.operation
                    || kept.source != journal.source)
        }) {
            return Err(io::Error::other(
                "A different directory erase is still pending.",
            ));
        }
        verify_approval(&kept, kind, &journal)?;
        kept.write(SETTINGS, settings).map_err(io::Error::other)?;
        if let Some(member) = member {
            Completion::prepare(&self.receipts()?, accepted, member)?;
        }
        self.save(kind, &journal)
    }

    pub(super) fn retained(&self, kind: AppWipeKind, decision: &LocalWipe) -> io::Result<bool> {
        let Some(journal) = read_journal(&self.marker(kind)?)? else {
            return Ok(false);
        };
        Ok(journal.phase == Some(Phase::Stopping)
            && journal.setup == decision.setup
            && journal.operation == decision.operation
            && journal.source == identity(&self.root)?)
    }

    pub(super) async fn finish(
        &self,
        keychain: &dyn AppWipeKeychain,
        report: Report,
    ) -> io::Result<bool> {
        for kind in [AppWipeKind::LastComputer, AppWipeKind::Member] {
            if self
                .finish_kind(kind, keychain, report)
                .await
                .inspect_err(|error| uninstall::log_cleanup(&self.box_files(), error))?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn finish_kind(
        &self,
        kind: AppWipeKind,
        keychain: &dyn AppWipeKeychain,
        report: Report,
    ) -> io::Result<bool> {
        let Some(journal) = read_journal(&self.marker(kind)?)? else {
            return Ok(false);
        };
        if matches!(journal.phase, Some(Phase::Erasing | Phase::Detached)) {
            if !super::resume_app_wipe(&self.root, &self.account, kind, keychain)? {
                return Ok(false);
            }
            self.report_completion(kind, &journal.setup, &journal.operation, report)
                .await?;
            return Ok(true);
        }
        if journal.phase != Some(Phase::Stopping) {
            if identity(&self.root)?.is_none() {
                self.report_completion(kind, &journal.setup, &journal.operation, report)
                    .await?;
                return Ok(true);
            }
            return Ok(false);
        }
        let guard = self.lock()?;
        let Some(mut journal) = read_journal(&self.marker(kind)?)? else {
            return Ok(false);
        };
        if journal.phase != Some(Phase::Stopping) {
            return Ok(false);
        }
        let kept = self.box_files();
        let settings = kept
            .read(SETTINGS)
            .map_err(io::Error::other)?
            .ok_or_else(|| io::Error::other(UNSUPPORTED))?;
        let verify = |journal: &Journal| {
            if identity(&self.root)? != journal.source {
                return Err(io::Error::other("The folder to erase has changed."));
            }
            self.check_layout(&settings)?;
            verify_approval(&kept, kind, journal)
        };
        let accepted = verify(&journal)?;
        crate::backup::with_verified_configured_disk(
            kept.root(),
            &self.root,
            super::saved_backup(&accepted)?.as_ref(),
            || Ok(()),
        )?;
        let receipts = self.receipts()?;
        if kind == AppWipeKind::Member
            && Completion::read(&receipts, &journal.setup, &journal.operation)?.is_none()
        {
            return Err(io::Error::other(
                "The wipe’s completion receipt is unavailable.",
            ));
        }
        stop_keeper(&self.root, self.real_account).await?;
        for name in ["run/serve.needed", "run/link.needed"] {
            files::remove_file_synced(&kept.path(name))?;
        }
        kept.write_bytes("run/off", b"").map_err(io::Error::other)?;
        let deadline = Instant::now() + STOP_WITHIN;
        while !uninstall::live_processes(&kept)
            .map_err(io::Error::other)?
            .is_empty()
        {
            if Instant::now() >= deadline {
                return Err(io::Error::other(
                    "Kuvo Code is still running. Stop it before wiping this computer.",
                ));
            }
            time::sleep(Duration::from_millis(100)).await;
        }
        let owner = OwnedLock::acquire(&kept.path("run/.client-owner"), Duration::ZERO)?;
        let link = OwnedLock::acquire(&kept.path("run/.link.lock"), Duration::ZERO)?;
        let candidate = verify(&journal)?;
        let backup = super::saved_backup(&candidate)?;
        if kind == AppWipeKind::LastComputer {
            keychain.clear_app_link_keys(&self.root)?;
        }
        let setup = journal.setup.clone();
        let operation = journal.operation.clone();
        crate::backup::with_verified_configured_disk(
            kept.root(),
            &self.root,
            backup.as_ref(),
            || {
                verify(&journal)?;
                journal.phase = Some(Phase::Erasing);
                self.save(kind, &journal)?;
                erase_approved(
                    &self.root,
                    &receipts,
                    journal,
                    |next| self.save(kind, next),
                    || {
                        drop((owner, link));
                        if kind == AppWipeKind::Member {
                            super::clear_detached_app_identity(&self.root, keychain)?;
                        }
                        Ok(())
                    },
                )
            },
        )?;
        drop(guard);
        self.report_completion(kind, &setup, &operation, Report::Wipe)
            .await?;
        Ok(true)
    }

    async fn report_completion(
        &self,
        kind: AppWipeKind,
        setup: &str,
        operation: &str,
        report: Report,
    ) -> io::Result<()> {
        if kind == AppWipeKind::Member {
            let receipts = self.receipts()?;
            if let Some(completion) = Completion::read(&receipts, setup, operation)? {
                completion.publish(&self.root, &receipts, report).await?;
            }
        }
        Ok(())
    }
}

pub(super) fn helper_keeps_keys(files: &Files) -> bool {
    env::var_os("KUVO_KEYS_ON_HOST").is_some_and(|setting| setting == "1")
        || env::var_os("KUVO_HOST_URL").is_some()
        || files.path("run/host.json").is_file()
}

fn resolved_data(path: &Path) -> io::Result<PathBuf> {
    match path.canonicalize() {
        Ok(path) => Ok(path),
        Err(error)
            if error.kind() == io::ErrorKind::NotFound
                && path
                    .symlink_metadata()
                    .is_err_and(|error| error.kind() == io::ErrorKind::NotFound) =>
        {
            Ok(resolved_data(parent(path)?)?.join(path.file_name().ok_or(error)?))
        }
        Err(error) => Err(error),
    }
}

fn verify_approval(kept: &Files, kind: AppWipeKind, journal: &Journal) -> io::Result<Link> {
    validate_disk(kept).map_err(io::Error::other)?;
    let link = kept
        .read::<Link>(LINK)
        .map_err(io::Error::other)?
        .ok_or_else(|| io::Error::other("The wipe has no accepted removal."))?;
    // The authenticated resident verified the app's Helper identity before it wrote the
    // private stopping receipt. Recheck the same accepted local operation without keys.
    let setup = link.setup().map_err(io::Error::other)?;
    let verified = match kind {
        AppWipeKind::LastComputer => {
            LocalWipe::accepted(&link, &setup.position.primary, &setup.sign).is_some_and(
                |decision| {
                    decision.setup == journal.setup && decision.operation == journal.operation
                },
            )
        }
        AppWipeKind::Member => link
            .extra
            .get("keyNow")
            .and_then(Value::as_str)
            .or_else(|| {
                link.extra
                    .get("localWipe")
                    .and_then(|wipe| wipe["source"].as_str())
            })
            .and_then(|key| MemberWipe::from(&link, key))
            .is_some_and(|decision| {
                decision.setup == journal.setup && decision.operation == journal.operation
            }),
    };
    if !verified {
        return Err(io::Error::other("The wipe has no accepted removal."));
    }
    Ok(link)
}

#[derive(Deserialize)]
struct Keeper {
    pid: u32,
    url: String,
    password: String,
}

impl Keeper {
    fn read(path: &Path) -> io::Result<Option<Self>> {
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        use std::os::unix::fs::MetadataExt as _;
        let meta = file.metadata()?;
        if !meta.is_file() || meta.mode() & 0o077 != 0 || meta.uid() != unsafe { libc::geteuid() } {
            return Err(io::Error::other(FAILED));
        }
        let mut bytes = Vec::new();
        file.take(16_385).read_to_end(&mut bytes)?;
        if bytes.len() > 16_384 {
            return Err(io::Error::other(FAILED));
        }
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| io::Error::other(FAILED))
    }

    fn process(&self) -> io::Result<Option<ProcessIdentity>> {
        if self.pid <= 1 {
            return Ok(None);
        }
        let Some(process) = ProcessIdentity::read(self.pid)? else {
            return Ok(None);
        };
        let output = Command::new("/bin/ps")
            .args(["-ww", "-o", "command=", "-p", &self.pid.to_string()])
            .output()?;
        let keeper = output.status.success()
            && String::from_utf8_lossy(&output.stdout)
                .split_whitespace()
                .any(|word| word == "--box");
        Ok((keeper && process.state() == ProcessState::Live).then_some(process))
    }

    fn stop_url(&self) -> io::Result<reqwest::Url> {
        let port = self
            .url
            .strip_prefix("http://127.0.0.1:")
            .map(|port| port.strip_suffix('/').unwrap_or(port))
            .filter(|port| {
                !port.is_empty() && port.len() <= 5 && port.bytes().all(|b| b.is_ascii_digit())
            })
            .and_then(|port| port.parse::<u16>().ok())
            .filter(|port| *port != 0 && !self.password.is_empty())
            .ok_or_else(|| io::Error::other(FAILED))?;
        reqwest::Url::parse(&format!("http://127.0.0.1:{port}/stop"))
            .map_err(|_| io::Error::other(FAILED))
    }

    async fn stop(self, process: ProcessIdentity) -> io::Result<()> {
        let url = self.stop_url()?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(STOP_WITHIN)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| io::Error::other(FAILED))?;
        let response = client
            .post(url)
            .bearer_auth(self.password)
            .send()
            .await
            .map_err(|_| io::Error::other(FAILED))?;
        if !response.status().is_success() {
            return Err(io::Error::other(FAILED));
        }
        let deadline = Instant::now() + STOP_WITHIN;
        loop {
            match process.state() {
                ProcessState::Stale => return Ok(()),
                ProcessState::Live if Instant::now() < deadline => {
                    time::sleep(Duration::from_millis(100)).await
                }
                _ => return Err(io::Error::other(FAILED)),
            }
        }
    }
}

/// Called only after the durable stopping receipt describes the current app folder.
pub(super) async fn stop_keeper(root: &Path, real_account: bool) -> io::Result<()> {
    let root = root.to_path_buf();
    let keeper = tokio::task::spawn_blocking(move || -> io::Result<_> {
        let keeper = Keeper::read(&root.join("box/run/keeper.json"))?;
        if real_account {
            disable_service(&root)?;
        }
        let Some(keeper) = keeper else {
            return Ok(None);
        };
        Ok(keeper.process()?.map(|process| (keeper, process)))
    })
    .await
    .map_err(io::Error::other)??;
    if let Some((keeper, process)) = keeper {
        keeper.stop(process).await?;
    }
    Ok(())
}

fn disable_service(root: &Path) -> io::Result<()> {
    let id = root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::other(FAILED))?;
    let uid = unsafe { libc::getuid() };
    let service = format!("gui/{uid}/{id}.box");
    let run = |verb: &str| {
        Command::new("/bin/launchctl")
            .args([verb, service.as_str()])
            .output()
    };
    if !run("disable")?.status.success() {
        return Err(io::Error::other(FAILED));
    }
    let _ = run("bootout");
    let loaded = run("print")?;
    if loaded.status.code() != Some(113)
        && !String::from_utf8_lossy(&loaded.stderr)
            .to_ascii_lowercase()
            .contains("could not find service")
    {
        return Err(io::Error::other(FAILED));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        process::Child,
        thread,
    };

    use kuvo_wire::keys::encode;
    use serde_json::json;

    use super::*;
    use crate::{
        actor,
        model::{ComputerKind, Identity, SharedKind},
    };

    const OPERATION: &str = "bbbbbbbbbbbbbbbbbbbbbb";

    struct KeeperChild(Child);

    impl Drop for KeeperChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    struct Keychain<'a>(&'a Target);

    impl AppWipeKeychain for Keychain<'_> {
        fn clear_app_link_keys(&self, folder: &Path) -> io::Result<()> {
            assert_eq!(folder, self.0.root);
            let journal = read_journal(&self.0.marker(AppWipeKind::LastComputer)?)?.unwrap();
            assert!(journal.phase == Some(Phase::Stopping));
            assert_eq!(journal.operation, OPERATION);
            assert_eq!(fs::read(folder.join("box/run/work"))?, b"held data");
            assert!(folder.join("box/run/off").exists());
            Ok(())
        }
    }

    #[tokio::test]
    async fn an_app_wipe_stops_its_keeper_before_removing_the_last_computers_identity() {
        let account = tempfile::tempdir().unwrap();
        let folder = account
            .path()
            .join("Library/Application Support/run.kuvo.code.fixture");
        fs::create_dir_all(folder.join("box/run")).unwrap();
        let target = Target::for_folder(&folder, account.path())
            .unwrap()
            .unwrap();
        let kept = target.box_files();
        kept.write("folder-version.json", &json!({"version": layout::VERSION}))
            .unwrap();
        kept.write_bytes("run/work", b"held data").unwrap();
        let mut state = Link {
            key: Some(encode(&[41; 32])),
            sign: Some(encode(&[42; 32])),
            secret: Some(encode(&[43; 32])),
            ..Link::default()
        };
        let own = Identity::read(&state).unwrap();
        actor::bootstrap(&kept, &mut state, &own, ComputerKind::Mac).unwrap();
        let mut roster = state
            .value(SharedKind::MembershipAndAccess)
            .unwrap()
            .clone();
        roster["removals"] =
            json!({ &own.public: { "local": true, "wipe": true, "operation": OPERATION } });
        state
            .put(
                &own,
                SharedKind::MembershipAndAccess,
                roster,
                None,
                Some(OPERATION.into()),
            )
            .unwrap();
        state.extra.insert("localWipe".into(), json!({
            "setup": state.setup().unwrap().position.id, "operation": OPERATION, "prepared": true,
        }));
        actor::write_link(&kept, &state, ComputerKind::Mac).unwrap();
        let kind = AppWipeKind::LastComputer;
        target
            .prepare(
                kind,
                LocalWipe::from(&state).unwrap(),
                None,
                &state,
                &Default::default(),
            )
            .unwrap();
        assert!(
            target
                .retained(kind, &LocalWipe::from(&state).unwrap())
                .unwrap()
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let child = KeeperChild(
            Command::new("/bin/sleep")
                .arg0("--box")
                .arg("20")
                .spawn()
                .unwrap(),
        );
        let url = format!("http://{}", listener.local_addr().unwrap());
        kept.write(
            "run/keeper.json",
            &json!({"pid": child.0.id(), "url": url, "password": "fixture-keeper"}),
        )
        .unwrap();
        let root = target.root.clone();
        let journal = target.marker(kind).unwrap();
        let stop = thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error)
                        if error.kind() == io::ErrorKind::WouldBlock
                            && std::time::Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(10))
                    }
                    Err(error) => panic!("waiting for keeper stop: {error}"),
                }
            };
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let request = BufReader::new(&socket)
                .lines()
                .map(Result::unwrap)
                .take_while(|line| !line.is_empty())
                .collect::<Vec<_>>();
            assert_eq!(request[0], "POST /stop HTTP/1.1");
            assert!(
                request
                    .iter()
                    .any(|line| line == "authorization: Bearer fixture-keeper")
            );
            assert!(root.exists());
            assert!(read_journal(&journal).unwrap().unwrap().phase == Some(Phase::Stopping));
            socket
                .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                .unwrap();
            drop(child);
        });
        let finished = target.finish(&Keychain(&target), Report::Wipe).await;
        stop.join().unwrap();
        assert!(finished.unwrap());
        assert!(!target.root.exists());
        assert!(
            read_journal(&target.marker(kind).unwrap())
                .unwrap()
                .unwrap()
                .phase
                .is_none()
        );
    }
}
