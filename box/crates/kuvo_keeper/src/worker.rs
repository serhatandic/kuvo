use std::{fs, io, path::Path, time::Duration};

use kuvo_os::{
    platform::Environment,
    process::{AdoptedProcess, ChildGroup, ProcessIdentity, ProcessState, Spawn, Stdio},
};
use kuvo_store::files;
use serde::{Deserialize, Serialize};

use crate::Release;

#[derive(Deserialize, Serialize)]
struct Record {
    release: String,
    identity: ProcessIdentity,
    job: String,
}

enum Process {
    Child(ChildGroup),
    Adopted(AdoptedProcess),
}

pub(crate) struct Worker {
    pub release: Release,
    process: Process,
}

impl Worker {
    pub fn adopt(home: &Path, record: &Path) -> io::Result<Option<Self>> {
        let Some(bytes) = files::read(record)? else {
            return Ok(None);
        };
        let Record {
            release,
            identity,
            job,
        } = serde_json::from_slice(&bytes)?;
        let release: Release = release.parse()?;
        match identity.state() {
            ProcessState::Stale => return Ok(None),
            ProcessState::Unknown => {
                return Err(io::Error::other("Couldn't verify the recorded resident"));
            }
            ProcessState::Live => {}
        }
        let program = release.executable(home).canonicalize()?;
        let adopted = AdoptedProcess::open_resident(
            &identity,
            |actual| actual.canonicalize().is_ok_and(|actual| actual == program),
            &job,
        )?;
        Ok(adopted.map(|process| Self {
            release,
            process: Process::Adopted(process),
        }))
    }

    pub fn start(
        home: &Path,
        record: &Path,
        release: Release,
        mut env: Environment,
    ) -> io::Result<Self> {
        release.validate(home)?;
        env.retain(|name, _| {
            // Windows shells may supply per-drive working directories as =C:.
            // The resident has an explicit working directory.
            !name.is_empty()
                && !name.as_encoded_bytes().contains(&b'=')
                && ![
                    "KUVO_HOME",
                    "KUVO_SETTINGS",
                    "BUN_BE_BUN",
                    "BUN_RUNTIME_TRANSPILER_CACHE_PATH",
                ]
                .iter()
                .any(|key| name.to_string_lossy().eq_ignore_ascii_case(key))
        });
        env.insert("KUVO_HOME".into(), home.as_os_str().to_owned());
        let mut launch = Spawn::new(release.executable(home), env);
        launch.args.push("serve".into());
        if cfg!(windows) {
            launch.args.push("--supervised".into());
        }
        let workspace = home.join("workspace");
        launch.cwd = Some(if workspace.is_dir() {
            workspace
        } else {
            home.to_owned()
        });
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(home.join("run/serve.log"))?;
        launch.stdout = Stdio::File(log.try_clone()?);
        launch.stderr = Stdio::File(log);
        let job = format!("kuvo-resident-{}", uuid::Uuid::new_v4());
        let mut child = launch.spawn_resident(&job)?;
        let bytes = serde_json::to_vec(&Record {
            release: release.to_string(),
            identity: child.identity().clone(),
            job,
        })?;
        files::write_secret(record, &bytes)?;
        if cfg!(unix) {
            child.leave_running_on_drop();
        }
        Ok(Self {
            release,
            process: Process::Child(child),
        })
    }

    pub fn identity(&self) -> &ProcessIdentity {
        match &self.process {
            Process::Child(child) => child.identity(),
            Process::Adopted(process) => process.identity(),
        }
    }

    /// Reap an exited child. The group/job can still own stragglers after the resident exits.
    pub fn finished(&mut self) -> io::Result<bool> {
        match &mut self.process {
            Process::Child(child) if child.has_exited()? => {
                child.wait()?;
                Ok(true)
            }
            Process::Child(_) => Ok(false),
            Process::Adopted(process) => Ok(process.identity().state() == ProcessState::Stale),
        }
    }

    pub async fn stop(mut self, run: &Path) -> io::Result<()> {
        const GRACE: Duration = Duration::from_secs(5);
        let request = run.join("keeper-stop");
        #[cfg(windows)]
        files::write_secret(&request, &serde_json::to_vec(self.identity())?)?;
        let result = tokio::task::spawn_blocking(move || match &mut self.process {
            Process::Child(child) => child.stop(GRACE, |_| {}).map(|_| ()),
            Process::Adopted(process) => {
                #[cfg(windows)]
                {
                    let until = std::time::Instant::now() + GRACE;
                    while process.identity().state() == ProcessState::Live
                        && std::time::Instant::now() < until
                    {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
                process.stop(GRACE)
            }
        })
        .await
        .map_err(io::Error::other)?;
        result.and_then(|()| files::remove_file_synced(&request))
    }
}

pub async fn wait_for_stop(home: &Path) {
    let Ok(Some(identity)) = ProcessIdentity::read(std::process::id()) else {
        return std::future::pending().await;
    };
    let path = home.join("run/keeper-stop");
    let mut clock = tokio::time::interval(Duration::from_millis(250));
    loop {
        clock.tick().await;
        if files::read(&path)
            .ok()
            .flatten()
            .and_then(|bytes| serde_json::from_slice::<ProcessIdentity>(&bytes).ok())
            .as_ref()
            == Some(&identity)
        {
            return;
        }
    }
}
