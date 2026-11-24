//! Recorded 0.7.2 Linux workers, whose launcher predates the resident keeper.

use std::{fs, io, os::unix::ffi::OsStrExt, path::Path, time::Duration};

use kuvo_os::process::{AdoptedProcess, ProcessIdentity, ProcessState};
use kuvo_store::home::Home;

use super::{process_pid, running};

pub(super) struct Workers {
    supervisor: Option<AdoptedProcess>,
    bridge: Option<AdoptedProcess>,
    link: Option<AdoptedProcess>,
    engine: Option<AdoptedProcess>,
}

impl Workers {
    pub(super) fn inspect(home: &Home) -> io::Result<Self> {
        for file in ["box.pid", "serve.json"] {
            if let Some(pid) = process_pid(home, file)?
                && ProcessIdentity::read(pid)?.is_some()
            {
                return Err(running());
            }
        }
        let engine = home.root().join("engine/2.0.32/opencode");
        let engine_pid = process_pid(home, "engine.json")?;
        let engine_worker = recorded(home, "engine.json", &engine, |args| {
            args.first() == Some(&engine.as_os_str().as_bytes())
                && args.get(1) == Some(&b"serve".as_slice())
        })?;
        let link = recorded(home, "link.pid", &engine, |args| {
            args.get(1) == Some(&home.bin().join("kuvo-link.js").as_os_str().as_bytes())
                && args.get(2) == Some(&b"run".as_slice())
        })?;
        let bridge = Self::bridge(home, &engine)?;
        let supervisor = recorded(home, "agents.pid", Path::new("/bin/sh"), |args| {
            let pid = engine_pid.map(|pid| pid.to_string());
            args.get(1) == Some(&b"-c".as_slice())
                && args.get(3) == Some(&b"kuvo-agents".as_slice())
                && pid
                    .as_ref()
                    .is_some_and(|pid| args.get(4) == Some(&pid.as_bytes()))
                && args.get(5) == Some(&engine.as_os_str().as_bytes())
                && args.get(6) == Some(&home.bin().join("kuvo-agents.js").as_os_str().as_bytes())
                && args.get(7) == Some(&home.run().join("agents.log").as_os_str().as_bytes())
        })?;
        // A supervisor can be starting its child. Wait for the child's own
        // record instead of stopping a shell while an unrecorded writer starts.
        if supervisor.is_some() && bridge.is_none() {
            return Err(running());
        }
        Ok(Self {
            supervisor,
            bridge,
            link,
            engine: engine_worker,
        })
    }

    fn bridge(home: &Home, engine: &Path) -> io::Result<Option<AdoptedProcess>> {
        recorded(home, "bridge.json", engine, |args| {
            args.get(1) == Some(&home.bin().join("kuvo-agents.js").as_os_str().as_bytes())
        })
    }

    pub(super) fn stop(self, home: &Home) -> io::Result<()> {
        let engine_pid = process_pid(home, "engine.json")?;
        let stop = |worker: Option<AdoptedProcess>| -> io::Result<()> {
            if let Some(mut worker) = worker {
                worker.stop(Duration::from_secs(3))?;
            }
            Ok(())
        };
        stop(self.supervisor)?;
        stop(self.bridge)?;
        // Once the supervisor cannot restart it, check the actual child
        // record again in case its predecessor exited during inspection.
        stop(Self::bridge(
            home,
            &home.root().join("engine/2.0.32/opencode"),
        )?)?;
        stop(self.link)?;
        stop(self.engine)?;
        // The earlier watchdog checks this record after its sleep. Withdraw
        // the stopped engine's address before moving the live folders.
        if let Some(pid) = engine_pid
            && process_pid(home, "engine.json")? == Some(pid)
            && ProcessIdentity::read(pid)?.is_none()
        {
            kuvo_store::files::remove_file_synced(&home.run().join("engine.json"))?;
        }
        Ok(())
    }
}

fn recorded(
    home: &Home,
    file: &str,
    executable: &Path,
    matches_args: impl FnOnce(&[&[u8]]) -> bool,
) -> io::Result<Option<AdoptedProcess>> {
    let Some(pid) = process_pid(home, file)? else {
        return Ok(None);
    };
    let Some(identity) = ProcessIdentity::read(pid)? else {
        return Ok(None);
    };
    let args = match fs::read(format!("/proc/{pid}/cmdline")) {
        Ok(args) => args,
        Err(_) if identity.state() == ProcessState::Stale => return Ok(None),
        Err(error) => return Err(error),
    };
    let args: Vec<_> = args.split(|byte| *byte == 0).collect();
    if !matches_args(&args) {
        return Err(running());
    }
    let executable = executable.canonicalize().map_err(|_| running())?;
    let worker = AdoptedProcess::open(
        pid,
        Some(&identity.start_identity),
        |path| path == executable,
        None,
    )?;
    if worker.is_none() && identity.state() != ProcessState::Stale {
        return Err(running());
    }
    Ok(worker)
}
