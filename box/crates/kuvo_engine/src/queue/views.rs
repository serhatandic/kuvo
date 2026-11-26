//! Keep the last complete inbox for immediate reads while OpenCode updates it.

use std::{collections::HashMap, sync::Arc};

use futures_util::future::{BoxFuture, FutureExt, Shared};
use serde_json::Value;
use tokio::sync::{oneshot, watch};

use crate::Engine;

type Reply = Option<Arc<Value>>;
type Reading = Shared<BoxFuture<'static, Reply>>;

#[derive(Clone)]
pub(super) struct Views(watch::Sender<Book>);

#[derive(Default)]
struct Book {
    clock: u64,
    sessions: HashMap<String, Remembered>,
}

#[derive(Default)]
struct Remembered {
    engine: (u32, u16),
    used: u64,
    read: u64,
    items: Reply,
    pending: Option<Reading>,
}

#[derive(Default)]
pub(super) struct Sample {
    pub items: Reply,
    pending: Option<Reading>,
}

impl Sample {
    pub(super) fn requested(&self) -> bool { self.pending.is_some() }

    pub async fn complete(self) -> Reply {
        self.pending?.await
    }
}

impl Views {
    pub fn new() -> Self {
        Self(watch::channel(Book::default()).0)
    }

    pub async fn refresh(&self, engine: &Engine, session: &str) -> Sample {
        let Some(place) = super::Place::passive(engine, session).await else {
            self.0.send_modify(|book| {
                book.sessions.remove(session);
            });
            return Sample::default();
        };
        let identity = match &place {
            super::Place::Engine(lease) => (lease.record.pid, lease.record.port),
            super::Place::Agents(_) => (std::process::id(), 0),
        };
        let mut sample = Sample::default();
        let mut start = None;
        self.0.send_modify(|book| {
            book.clock = book.clock.wrapping_add(1);
            let age = book.clock;
            let entry = book.sessions.entry(session.to_owned()).or_default();
            if entry.engine != identity {
                *entry = Remembered {
                    engine: identity,
                    ..Remembered::default()
                };
            }
            entry.used = age;
            sample.items.clone_from(&entry.items);
            if entry.pending.is_none() {
                let (send, receive) = oneshot::channel::<Reply>();
                entry.read = age;
                entry.pending = Some(async move { receive.await.ok().flatten() }.boxed().shared());
                start = Some((send, age));
            }
            sample.pending.clone_from(&entry.pending);
            if book.sessions.len() > 1000
                && let Some(oldest) = book
                    .sessions
                    .iter()
                    .min_by_key(|(_, item)| item.used)
                    .map(|(id, _)| id.clone())
            {
                book.sessions.remove(&oldest);
            }
        });
        if let Some((send, read)) = start {
            let home = engine.clone();
            let views = self.clone();
            let session = session.to_owned();
            tokio::spawn(async move {
                let result = async {
                    let queue = home.queues.get(&session).await.ok()?;
                    let inbox = super::inbox(&home, place.target(), &session).await.ok()?;
                    super::with_items(&home, place.target(), &session, queue, &inbox)
                        .await
                        .ok()
                        .map(Arc::new)
                }
                .await;
                views.0.send_modify(|book| {
                    if let Some(entry) = book
                        .sessions
                        .get_mut(&session)
                        .filter(|entry| entry.read == read)
                    {
                        if let Some(view) = &result {
                            entry.items = Some(Arc::new(view["items"].clone()));
                        }
                        entry.pending = None;
                    }
                });
                let _ = send.send(result);
            });
        }
        sample
    }
}
