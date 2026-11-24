//! The native prompt's same-process send-once answers, separate from composer receipts.

use std::{collections::HashMap, time::Duration};

use http::{HeaderMap, Method, StatusCode};
use kuvo_server::{DispatchRequest, Reply};
use serde_json::Value;
use tokio::{
    sync::{mpsc, oneshot},
    time::Instant,
};

type Key = (String, String);

pub(crate) fn prompt(request: &DispatchRequest) -> Option<Key> {
    if request.method != Method::POST {
        return None;
    }
    let path = request
        .path
        .split('?')
        .next()?
        .strip_prefix("api/session/")?;
    let (session, "prompt") = path.split_once('/')? else {
        return None;
    };
    let session = percent_encoding::percent_decode_str(session)
        .decode_utf8()
        .ok()?;
    let body: Value = serde_json::from_slice(&request.body).ok()?;
    let id = body["id"]
        .as_str()
        .filter(|id| (1..=200).contains(&id.encode_utf16().count()))?;
    Some((session.into_owned(), id.to_owned()))
}

pub(crate) fn removed(request: &DispatchRequest) -> Option<Key> {
    if request.method != Method::DELETE {
        return None;
    }
    let path = request
        .path
        .split('?')
        .next()?
        .strip_prefix("api/session/")?;
    let (session, path) = path.split_once('/')?;
    let id = path
        .strip_prefix("inbox/")?
        .split('/')
        .next()
        .filter(|id| !id.is_empty())?;
    let decode = |value: &str| {
        percent_encoding::percent_decode_str(value)
            .decode_utf8()
            .ok()
            .map(|value| value.into_owned())
    };
    Some((decode(session)?, decode(id)?))
}

#[derive(Clone)]
pub(crate) struct Answer {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Answer {
    pub fn reply(self) -> Reply {
        let mut reply = Reply::bytes(self.status, "application/octet-stream", self.body);
        reply.headers = self.headers;
        reply
    }
}

enum Command {
    Read(Key, oneshot::Sender<Option<Answer>>),
    Keep(Key, Answer),
    Forget(Key),
}

#[derive(Clone)]
pub(crate) struct RawPrompts(mpsc::Sender<Command>);

impl RawPrompts {
    pub fn start() -> Self {
        let (commands, mut queue) = mpsc::channel(64);
        tokio::spawn(async move {
            let mut kept = HashMap::<Key, (Instant, Answer)>::new();
            while let Some(command) = queue.recv().await {
                match command {
                    Command::Read(key, answer) => {
                        kept.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(3600));
                        let _ = answer.send(kept.get(&key).map(|(_, reply)| reply.clone()));
                    }
                    Command::Keep(key, reply) => {
                        if kept.len() >= 1000
                            && let Some(oldest) = kept
                                .iter()
                                .min_by_key(|(_, (at, _))| *at)
                                .map(|(key, _)| key.clone())
                        {
                            kept.remove(&oldest);
                        }
                        kept.insert(key, (Instant::now(), reply));
                    }
                    Command::Forget(key) => {
                        kept.remove(&key);
                    }
                }
            }
        });
        Self(commands)
    }

    pub async fn read(&self, key: &Key) -> Option<Answer> {
        let (reply, answer) = oneshot::channel();
        self.0.send(Command::Read(key.clone(), reply)).await.ok()?;
        answer.await.ok().flatten()
    }

    pub async fn keep(&self, key: Key, answer: Answer) {
        let _ = self.0.send(Command::Keep(key, answer)).await;
    }

    pub async fn forget(&self, key: Key) {
        let _ = self.0.send(Command::Forget(key)).await;
    }
}
