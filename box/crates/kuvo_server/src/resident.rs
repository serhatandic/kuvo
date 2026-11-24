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
use kuvo_protocol::version::SERVE_RECORD_VERSION;
use kuvo_store::{files, locks::OwnedLock};
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
    _owner: OwnedLock,
}

impl Resident {
    /// A resident claims the home before binding. Discovery is published only once both
    /// listeners can accept traffic. A paused or off home never publishes a new record.
    pub async fn bind(home: &Path) -> io::Result<Option<Self>> {
        Self::bind_with_password(home, None).await
    }

    pub async fn bind_with_password(
        home: &Path,
        password: Option<String>,
    ) -> io::Result<Option<Self>> {
        let started_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_millis() as u64;
        let run = home.join("run");
        let owner_run = run.clone();
        let owner = tokio::task::spawn_blocking(move || {
            OwnedLock::acquire(&owner_run.join(".client-owner"), Duration::ZERO).map_err(|error| {
                if error.kind() == io::ErrorKind::WouldBlock {
                    io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "The resident box already runs.",
                    )
                } else {
                    error
                }
            })
        })
        .await
        .map_err(io::Error::other)??;
        if stopped(&run).await? {
            return Ok(None);
        }
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
            password: match password {
                Some(password) => password,
                None => hex::encode(random::<24>()?),
            },
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
        // The publishing task owns the guard even if the async caller cancels.
        tokio::task::spawn_blocking(move || {
            resident.publish()?;
            Ok(Some(resident))
        })
        .await
        .map_err(io::Error::other)?
    }

    fn publish(&self) -> io::Result<()> {
        let record = serde_json::to_vec(&self.record)?;
        let pid = self.record.pid.to_string();
        files::write_atomic(&self.run.join("link.pid"), pid.as_bytes())?;
        files::write_atomic(&self.run.join("serve.json"), &record)
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
