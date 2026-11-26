use std::{
    ffi::OsString,
    fs, io,
    net::{Ipv4Addr, TcpListener},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use kuvo_os::{
    platform::{Environment, Platform, env_value, join_path},
    process::{AdoptedProcess, ChildGroup, ProcessIdentity, ProcessState, Spawn, Stdio},
};
use serde::{Deserialize, Serialize};

const ENGINE_EXE: &str = if cfg!(windows) {
    "opencode.exe"
} else {
    "opencode"
};

#[derive(Deserialize)]
struct Pin {
    version: String,
}

pub fn pinned_version() -> String {
    serde_json::from_str::<Pin>(include_str!("../../../src/shared/engine-pin.json"))
        .expect("checked-in engine pin")
        .version
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Record {
    pub pid: u32,
    pub port: u16,
    pub password: String,
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start_identity: Option<String>,
}

impl Record {
    fn recovery_name(&self) -> Option<String> {
        self.start_identity.as_ref()?;
        let token = self.password.strip_prefix("kuvo-")?;
        (token.len() == 48 && token.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| format!("kuvo-engine-{token}"))
    }

    pub fn url(&self, scheme: &str, path: &str) -> String {
        format!("{scheme}://127.0.0.1:{}/{path}", self.port)
    }
}

pub(crate) struct Worker {
    pub record: Record,
    pub started: Option<SystemTime>,
    process: Process,
}

enum Process {
    Spawned(ChildGroup),
    Adopted(AdoptedProcess),
    // A legacy record can describe an already delegated upstream. It can be
    // forwarded to, but only an identified local engine can be stopped.
    Borrowed(ProcessIdentity),
}

impl Worker {
    pub fn load(home: &Path) -> io::Result<Option<Self>> {
        let path = home.join("run/engine.json");
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let record: Record = serde_json::from_slice(&bytes)?;
        if record.port == 0 || record.password.is_empty() {
            return Err(io::Error::other("invalid engine address or password"));
        }
        let started = fs::metadata(&path).and_then(|meta| meta.modified()).ok();
        let root = fs::canonicalize(home.join("engine"));
        let worker = AdoptedProcess::open(
            record.pid,
            record.start_identity.as_deref(),
            |exe| {
                root.as_ref().is_ok_and(|root| {
                    fs::canonicalize(exe).ok().is_some_and(|exe| {
                        exe.strip_prefix(root).ok().is_some_and(|path| {
                            path.file_name().is_some_and(|name| name == ENGINE_EXE)
                                && path.components().count() == 2
                        })
                    })
                })
            },
            record.recovery_name().as_deref(),
        )?;
        let process = match worker {
            Some(worker) => Some(Process::Adopted(worker)),
            None if record.start_identity.is_none() => {
                ProcessIdentity::read(record.pid)?.map(Process::Borrowed)
            }
            None => None,
        };
        let Some(process) = process else {
            fs::remove_file(path)?;
            return Ok(None);
        };
        Ok(Some(Self {
            record,
            started,
            process,
        }))
    }

    pub fn launch(home: &Path, version: &str, env: Environment) -> io::Result<Self> {
        let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?
            .local_addr()?
            .port();
        let mut token = [0; 24];
        getrandom::fill(&mut token).map_err(io::Error::other)?;
        let token = hex::encode(token);
        let password = format!("kuvo-{token}");
        let mut spawn = Spawn::new(
            executable(home, version),
            environment(home, &password, env)?,
        );
        spawn.cwd = Some(home.join("workspace"));
        spawn.args = [
            "serve",
            "--port",
            &port.to_string(),
            "--hostname",
            "127.0.0.1",
        ]
        .map(OsString::from)
        .into();
        let log_path = home.join("run/engine.log");
        kuvo_store::files::write_atomic(&log_path, b"")?;
        let log = fs::OpenOptions::new().write(true).open(log_path)?;
        spawn.stdout = Stdio::File(log.try_clone()?);
        spawn.stderr = Stdio::File(log);
        let mut process = spawn.spawn_recoverable(&format!("kuvo-engine-{token}"))?;
        let ProcessIdentity {
            pid,
            start_identity,
        } = process.identity().clone();
        let record = Record {
            pid,
            port,
            password,
            version: Some(version.to_owned()),
            start_identity: Some(start_identity),
        };
        // Publish as soon as the process has an identity so even a restart during
        // warm-up can recover it. Clients get the address only after health passes.
        let path = home.join("run/engine.json");
        kuvo_store::files::write_atomic(&path, &serde_json::to_vec(&record)?)?;
        let started = fs::metadata(path).and_then(|meta| meta.modified()).ok();
        process.leave_running_on_drop();
        Ok(Self {
            record,
            started,
            process: Process::Spawned(process),
        })
    }

    pub fn identity(&self) -> &ProcessIdentity {
        match &self.process {
            Process::Spawned(child) => child.identity(),
            Process::Adopted(child) => child.identity(),
            Process::Borrowed(identity) => identity,
        }
    }

    pub fn alive(&self) -> io::Result<bool> {
        match &self.process {
            Process::Spawned(child) => child.has_exited().map(|exited| !exited),
            Process::Adopted(child) => identity_alive(child.identity()),
            Process::Borrowed(identity) => identity_alive(identity),
        }
    }

    pub fn owned(&self) -> bool {
        !matches!(self.process, Process::Borrowed(_))
    }

    pub fn stop(&mut self) -> io::Result<()> {
        let grace = if cfg!(windows) {
            Duration::ZERO
        } else {
            Duration::from_secs(5)
        };
        match &mut self.process {
            Process::Spawned(child) => child.stop(grace, |_| {}).map(|_| ()),
            Process::Adopted(child) => child.stop(grace),
            Process::Borrowed(identity) if identity_alive(identity)? => Err(io::Error::other(
                "the upstream engine belongs to another process",
            )),
            Process::Borrowed(_) => Ok(()),
        }
    }
}

fn identity_alive(identity: &ProcessIdentity) -> io::Result<bool> {
    match identity.state() {
        ProcessState::Live => Ok(true),
        ProcessState::Stale => Ok(false),
        ProcessState::Unknown => Err(io::Error::other("engine identity unavailable")),
    }
}

pub(crate) fn executable(home: &Path, version: &str) -> PathBuf {
    home.join("engine").join(version).join(ENGINE_EXE)
}

fn environment(home: &Path, password: &str, mut env: Environment) -> io::Result<Environment> {
    let platform = Platform::current();
    if cfg!(windows) {
        // Cmd includes per-drive working directories as entries such as "=C:".
        // OpenCode and its workspace already start from absolute paths.
        env.retain(|name, _| !name.as_encoded_bytes().starts_with(b"="));
    }
    let mut set = |key: &str, value: OsString| {
        if cfg!(windows) {
            env.retain(|name, _| !name.to_string_lossy().eq_ignore_ascii_case(key));
        }
        env.insert(key.into(), value);
    };
    for (key, sub) in [
        ("XDG_DATA_HOME", "data"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_CACHE_HOME", "cache"),
        ("TMPDIR", "tmp"),
        ("TEMP", "tmp"),
        ("TMP", "tmp"),
        ("npm_config_cache", "cache/npm"),
        ("BUN_INSTALL", "cache/bun"),
        ("KUVO_INTEGRATIONS", "integrations"),
        ("KUVO_BOTS", "bots"),
        ("KUVO_WAKES", "wakes"),
    ] {
        let destination = home.join(sub);
        fs::create_dir_all(&destination)?;
        set(key, destination.into_os_string());
    }
    fs::create_dir_all(home.join("workspace"))?;
    set("KUVO_HOME", home.as_os_str().to_owned());
    set("OPENCODE_CLIENT", "desktop".into());
    set("OPENCODE_PASSWORD", password.into());
    set("OPENCODE_SERVER_PASSWORD", password.into());
    set("OPENCODE_SERVER_USERNAME", "opencode".into());
    set("OPENCODE_USERNAME", "opencode".into());

    let mac = fs::read_to_string(home.join("box/kind")).is_ok_and(|kind| kind.trim() == "mac");
    if !mac {
        for candidate in ["box/opencode.json", "keys/settings.json"] {
            let settings = home.join(candidate);
            if settings.is_file() {
                set("OPENCODE_CONFIG", settings.into_os_string());
                break;
            }
        }
    }

    let bin = home.join("bin");
    let path = join_path(
        platform,
        std::iter::once(bin.as_os_str()).chain(env_value(platform, &env, "PATH")),
    );
    env.retain(|name, _| {
        ![
            "BUN_BE_BUN",
            "BUN_RUNTIME_TRANSPILER_CACHE_PATH",
            "KUVO_AGENTS_PASSWORD",
            "PATH",
        ]
        .iter()
        .any(|key| name.to_string_lossy().eq_ignore_ascii_case(key))
    });
    env.insert("PATH".into(), path);
    Ok(env)
}
