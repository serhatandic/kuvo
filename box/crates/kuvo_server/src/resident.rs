use std::{
    fs,
    future::Future,
    io,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use kuvo_os::process::ProcessIdentity;
use kuvo_protocol::version::SERVE_RECORD_VERSION;
use serde::{Deserialize, Serialize};
use tokio::{net::TcpListener, task::JoinSet, time};
use tokio_util::sync::CancellationToken;

use crate::{Dispatcher, local, ssh};

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServeRecord {
    pub format_version: u32,
    pub pid: u32,
    pub port: u16,
    pub password: String,
    pub kept: bool,
    pub started_at: u64,
    pub generation: uuid::Uuid,
}

pub struct Resident {
    record: ServeRecord,
    http: TcpListener,
    ssh: ssh::Listener,
    run: PathBuf,
    _owner: Owner,
}

impl Resident {
    /// A resident claims the home before binding. Discovery is published only once both
    /// listeners can accept traffic. A paused or off home never publishes a new record.
    pub async fn bind(home: &Path) -> io::Result<Option<Self>> {
        let started_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_millis() as u64;
        let run = home.join("run");
        let owner_run = run.clone();
        let Some(owner) = tokio::task::spawn_blocking(move || Owner::claim(&owner_run))
            .await
            .map_err(io::Error::other)??
        else {
            return Ok(None);
        };
        let http = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let port = http.local_addr()?.port();
        let ssh = ssh::Listener::bind(&run).await?;
        if stopped(&run).await? {
            return Ok(None);
        }
        let record = ServeRecord {
            format_version: SERVE_RECORD_VERSION,
            pid: std::process::id(),
            port,
            password: hex::encode(random::<24>()?),
            kept: false,
            started_at,
            generation: uuid::Uuid::new_v4(),
        };
        let resident = Self {
            record,
            http,
            ssh,
            run,
            _owner: owner,
        };
        resident.publish()?;
        Ok(Some(resident))
    }

    fn publish(&self) -> io::Result<()> {
        use std::io::Write;
        let mut pid = tempfile::NamedTempFile::new_in(&self.run)?;
        write!(pid, "{}", self.record.pid)?;
        pid.persist(self.run.join("link.pid"))?;
        let mut draft = tempfile::NamedTempFile::new_in(&self.run)?;
        serde_json::to_writer(&mut draft, &self.record)?;
        draft.persist(self.run.join("serve.json"))?;
        Ok(())
    }

    pub fn record(&self) -> &ServeRecord {
        &self.record
    }

    pub async fn serve(
        mut self,
        dispatcher: Dispatcher,
        pairings: Arc<dyn ssh::Pairings>,
        shutdown: impl Future<Output = ()>,
    ) -> io::Result<()> {
        let authorization: Arc<str> = format!(
            "Basic {}",
            STANDARD.encode(format!("opencode:{}", self.record.password)),
        )
        .into();
        let mut tasks = JoinSet::new();
        let leaving = CancellationToken::new();
        tokio::pin!(shutdown);
        let result = loop {
            tokio::select! {
                () = &mut shutdown => break Ok(()),
                accepted = self.http.accept() => {
                    let (socket, _) = match accepted {
                        Ok(connection) => connection,
                        Err(error) => break Err(error),
                    };
                    let authorization = authorization.clone();
                    let dispatcher = dispatcher.clone();
                    let leaving = leaving.clone();
                    tasks.spawn(local::serve(socket, authorization, dispatcher, leaving));
                }
                accepted = self.ssh.accept() => {
                    let socket = match accepted {
                        Ok(socket) => socket,
                        Err(error) => break Err(error),
                    };
                    let dispatcher = dispatcher.clone();
                    let pairings = pairings.clone();
                    let key = *self.ssh.key();
                    tasks.spawn(async move { let _ = ssh::answer(socket, key, dispatcher, pairings).await; });
                }
                _ = tasks.join_next(), if !tasks.is_empty() => {},
            }
        };
        leaving.cancel();
        if time::timeout(Duration::from_millis(1500), async {
            while tasks.join_next().await.is_some() {}
        })
        .await
        .is_err()
        {
            tasks.shutdown().await;
        }
        result
    }
}

impl Drop for Resident {
    fn drop(&mut self) {
        let pid = self.run.join("link.pid");
        if fs::read_to_string(&pid).is_ok_and(|pid| pid == self.record.pid.to_string()) {
            let _ = fs::remove_file(pid);
        }
        let path = self.run.join("serve.json");
        if fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<ServeRecord>(&bytes).ok())
            .is_some_and(|record| {
                record.pid == self.record.pid && record.generation == self.record.generation
            })
        {
            let _ = fs::remove_file(path);
        }
    }
}

pub(crate) fn random<const N: usize>() -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    Ok(bytes)
}

async fn stopped(run: &Path) -> io::Result<bool> {
    Ok(tokio::fs::try_exists(run.join("off")).await?
        || tokio::fs::try_exists(run.join("paused")).await?)
}

struct Owner {
    path: PathBuf,
    marker: String,
}

impl Owner {
    fn claim(run: &Path) -> io::Result<Option<Self>> {
        private_dir(run)?;
        let path = run.join(".client-owner");
        let marker = format!(
            "owner-{}-{}",
            std::process::id(),
            hex::encode(random::<8>()?)
        );
        let draft = tempfile::tempdir_in(run)?;
        fs::File::create(draft.path().join(&marker))?;
        if fs::symlink_metadata(&path).is_ok() {
            Self::reclaim(&path)?;
        }
        fs::rename(draft.path(), &path).map_err(|error| {
            io::Error::new(error.kind(), format!("Claiming the resident home: {error}"))
        })?;
        let owner = Self { path, marker };
        Ok((!run.join("off").exists() && !run.join("paused").exists()).then_some(owner))
    }

    fn reclaim(path: &Path) -> io::Result<()> {
        let live = || {
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                "The resident box already runs.",
            )
        };
        if !fs::symlink_metadata(path)?.is_dir() {
            return Err(live());
        }
        let mut entries = fs::read_dir(path)?;
        let Some(entry) = entries.next().transpose()? else {
            return fs::remove_dir(path);
        };
        if entries.next().is_some() {
            return Err(live());
        }
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|name| {
            let (pid, token) = name.strip_prefix("owner-")?.split_once('-')?;
            (token.len() == 16 && token.bytes().all(|b| b.is_ascii_hexdigit()))
                .then(|| pid.parse().ok())
                .flatten()
        }) else {
            return Err(live());
        };
        if ProcessIdentity::read(pid)?.is_some() {
            return Err(live());
        }
        // Remove only the stale marker observed above. A newer nonempty owner directory wins.
        fs::remove_file(entry.path())?;
        fs::remove_dir(path)
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        if fs::remove_file(self.path.join(&self.marker)).is_ok() {
            let _ = fs::remove_dir(&self.path);
        }
    }
}

pub(crate) fn private_dir(path: &Path) -> io::Result<()> {
    let mut dirs = fs::DirBuilder::new();
    dirs.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        dirs.mode(0o700);
    }
    dirs.create(path)
}
