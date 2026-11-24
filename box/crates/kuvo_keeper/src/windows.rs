//! Task Scheduler owns the native keeper's lifetime, independently of sshd.

use crate::{Release, SELECTOR, readiness};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use kuvo_os::{
    platform::{Environment, windows_conventional_path},
    process::{ProcessIdentity, ProcessState},
    windows::{private_home, system_directory, user_sid},
};
use kuvo_store::{files, home::Home, locks};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read as _},
    os::windows::{ffi::OsStrExt as _, process::CommandExt as _},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

pub const STARTER: &str = "Kuvo.exe";

/// A task belongs to one account and one installation. This handle names it
/// without making its lifetime depend on the installer process.
pub struct StartupTask {
    name: String,
    sid: String,
    program: PathBuf,
}

impl StartupTask {
    pub fn for_home(home: &Path) -> io::Result<Self> {
        private_home(home, false)?;
        let sid = user_sid()?;
        let canonical = home.canonicalize()?;
        let home = windows_conventional_path(&canonical)
            .ok_or_else(|| io::Error::other("Kuvo needs a local Windows folder"))?;
        let mut digest = Sha256::new();
        for unit in home.as_os_str().encode_wide() {
            digest.update(unit.to_le_bytes());
        }
        let digest = digest.finalize();
        let prefix = if home
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case(".kuvo-code"))
        {
            "Kuvo"
        } else {
            "Kuvo test"
        };
        Ok(Self {
            name: format!("{prefix} {sid} {}", hex::encode(&digest[..12])),
            sid,
            program: home.join("bin").join(STARTER),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn register(&self) -> io::Result<bool> {
        if !self.program.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "The Kuvo starter is missing",
            ));
        }
        Ok(self.scheduler("register")?.trim() == "registered")
    }

    pub fn remove(&self) -> io::Result<()> {
        self.scheduler("remove").map(drop)
    }

    fn scheduler(&self, operation: &str) -> io::Result<String> {
        let script = STANDARD.encode(
            include_str!("windows/task.ps1")
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
        );
        let output =
            Command::new(system_directory()?.join("WindowsPowerShell/v1.0/powershell.exe"))
                .creation_flags(CREATE_NO_WINDOW)
                .args([
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-EncodedCommand",
                    &script,
                ])
                .env("KUVO_TASK_ACTION", operation)
                .env("KUVO_TASK_NAME", &self.name)
                .env("KUVO_TASK_SID", &self.sid)
                .env("KUVO_TASK_PROGRAM", &self.program)
                .output()
                .map_err(|error| {
                    io::Error::other(format!("Open Windows Task Scheduler: {error}"))
                })?;
        let reply = String::from_utf8_lossy(&output.stdout);
        if !output.status.success() {
            // The script reports failures as UTF-8 on stdout. Windows PowerShell
            // can serialize its own diagnostics and progress on stderr as CLIXML.
            let detail = reply.trim();
            let detail = if detail.is_empty() {
                format!(
                    "Windows Task Scheduler could not be reached ({})",
                    output.status
                )
            } else {
                detail.to_owned()
            };
            return Err(io::Error::other(format!(
                "Kuvo couldn't {operation} its Windows sign-in task: {detail}"
            )));
        }
        Ok(reply.into_owned())
    }

    /// Wait for the real resident's authenticated reply, even over an SSH pipe.
    pub async fn start_and_wait(&self, home: &Path) -> io::Result<()> {
        if self.scheduler("start")?.trim() == "sign-in" {
            // The logon trigger remains registered while this account is signed out.
            return Ok(());
        }
        let client = readiness::probe_client()?;
        let run = home.join("run");
        let result = tokio::time::timeout(Duration::from_secs(25), async {
            loop {
                let current: Option<kuvo_server::ServeRecord> =
                    crate::read_record(&run.join("serve.json"))?;
                if let Some(record) = current
                    && ProcessIdentity::read(record.pid)?.is_some()
                    && readiness::probe(&client, &run, record.pid).await?.is_some()
                {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await;
        result.unwrap_or_else(|_| {
            Err(io::Error::new(io::ErrorKind::TimedOut,
                "Kuvo did not start answering. Sign in to Windows with the account that installed it."))
        })
    }

    pub async fn disable(&self, home: &Home) -> io::Result<()> {
        crate::execute(
            home,
            Environment::new(),
            crate::Command::Off,
            std::future::pending(),
        )
        .await?;
        let home = home.root();
        let owner = home.join("run/keeper-owner");
        #[derive(serde::Deserialize)]
        struct Resident {
            identity: ProcessIdentity,
        }
        let resident = home.join("run/keeper-resident.json");
        let keeper: Option<ProcessIdentity> = crate::read_record(&owner)?;
        let worker: Option<Resident> = crate::read_record(&resident)?;
        let process = worker.map(|worker| worker.identity);
        // Give the keeper its ordinary shutdown before removing the task.
        tokio::time::timeout(Duration::from_secs(10), async {
            while keeper
                .iter()
                .chain(&process)
                .any(|identity| identity.state() != ProcessState::Stale)
            {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Kuvo is still stopping"))?;
        self.remove()
    }
}

/// Install a local release from the directory containing the installer.
pub fn stage_local_release(home: &Path, executable: &Path) -> io::Result<()> {
    let bin = home.join("bin");
    let starter = bin.join(STARTER);
    if !starter.try_exists()? {
        let file = executable.with_file_name("kuvo-starter.exe");
        let source = if file.try_exists()? {
            file
        } else {
            executable.with_file_name(STARTER)
        };
        stage_file(&source, &starter)?;
    }
    let unwind = executable.with_file_name("libunwind.dll");
    let dll = bin.join("libunwind.dll");
    if unwind.try_exists()? && !dll.try_exists()? {
        stage_file(&unwind, &dll)?;
    }
    let target = bin.join("kuvo-box.exe");
    if !target.try_exists()? {
        stage_file(executable, &target)?;
    }
    Ok(())
}

fn stage_file(from: &Path, to: &Path) -> io::Result<()> {
    let mut reader = fs::File::open(from).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("Can't open {}: {error}", from.display()),
        )
    })?;
    files::write_generated_new(to, |writer| io::copy(&mut reader, writer).map(drop))
}

/// Put the SSH-installed bytes into the keeper's normal selector layout.
/// Published release packs and subsequent switches use the same layout.
pub fn prepare_first_release(home: &Path, version: &str) -> io::Result<()> {
    let run = home.join("run");
    files::create_private_dirs(&run)?;
    let _lock = locks::try_file_lock(&run.join(".keeper-selection.lock"))?;
    let selector = home.join(SELECTOR);
    if Release::read(&selector)?.is_some() {
        return Ok(());
    }
    let stable = home.join("bin");
    let program = stable.join("kuvo-box.exe");
    let mut reader = io::BufReader::new(fs::File::open(&program)?);
    let mut digest = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    let release: Release = format!(
        "versions/{version}-{}/bin",
        hex::encode(&digest.finalize()[..6])
    )
    .parse()?;
    let folder = release.folder(home);
    let bin = folder.join("bin");
    files::create_private_dirs(&bin)?;
    for name in ["kuvo-box.exe", "libunwind.dll"] {
        let source = stable.join(name);
        if name == "kuvo-box.exe" || source.try_exists()? {
            let target = bin.join(name);
            files::write_generated(&target, |writer| {
                io::copy(&mut fs::File::open(source)?, writer).map(drop)
            })?;
        }
    }
    let manifest = serde_json::json!({"kuvo": "box/1", "version": version, "launch": "rust"});
    files::write_secret(&folder.join("box.json"), &serde_json::to_vec(&manifest)?)?;
    release.validate(home)?;
    release.write(&selector)
}
