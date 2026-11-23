//! Attention leases expire on a monotonic clock.

use std::{collections::BTreeMap, time::Duration};

use kuvo_server::Caller;
use serde::Deserialize;
use tokio::{
    sync::{mpsc, oneshot},
    time::Instant,
};

#[derive(Deserialize)]
pub(crate) struct Attention {
    #[serde(rename = "for")]
    pub seconds: f64,
    #[serde(rename = "sessions", default, deserialize_with = "present")]
    _sessions: Option<Vec<String>>,
    #[serde(rename = "shown", default, deserialize_with = "present")]
    _shown: Option<Vec<Shown>>,
}

#[derive(Deserialize)]
struct Shown {
    #[serde(rename = "session")]
    _session: String,
    #[serde(rename = "ask", default, deserialize_with = "present")]
    _ask: Option<bool>,
    #[serde(rename = "request", default, deserialize_with = "present")]
    _request: Option<String>,
}

fn present<'a, T: Deserialize<'a>, D: serde::Deserializer<'a>>(
    value: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(value).map(Some)
}

#[derive(Clone)]
pub(crate) struct Leases(mpsc::Sender<Command>);

enum Command {
    Read(oneshot::Sender<bool>),
    Attend(Client, Duration, oneshot::Sender<()>),
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Client {
    Local,
    Paired([u8; 32], String),
}

impl Leases {
    pub fn start() -> Self {
        let (tx, mut rx) = mpsc::channel(32);
        tokio::spawn(async move {
            let mut active = BTreeMap::<Client, Instant>::new();
            while let Some(command) = rx.recv().await {
                let now = Instant::now();
                active.retain(|_, expires| now < *expires);
                match command {
                    Command::Read(reply) => {
                        let _ = reply.send(!active.is_empty());
                    }
                    Command::Attend(client, duration, reply) => {
                        if !duration.is_zero() {
                            active.insert(client, now + duration);
                        } else {
                            active.remove(&client);
                        }
                        let _ = reply.send(());
                    }
                }
            }
        });
        Self(tx)
    }

    pub async fn in_use(&self) -> bool {
        let (send, receive) = oneshot::channel();
        self.0.send(Command::Read(send)).await.is_ok() && receive.await.unwrap_or(false)
    }

    pub async fn attend(&self, caller: &Caller, given: Attention) -> Option<f64> {
        if !given.seconds.is_finite() || given.seconds < 0.0 {
            return None;
        }
        let seconds = given.seconds.min(60.0);
        let client = caller.pairing().map_or(Client::Local, |device| {
            Client::Paired(device.key, device.id.clone())
        });
        let (send, receive) = oneshot::channel();
        self.0
            .send(Command::Attend(
                client,
                Duration::from_secs_f64(seconds),
                send,
            ))
            .await
            .ok()?;
        receive.await.ok()?;
        Some(seconds)
    }
}
