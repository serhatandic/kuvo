//! Coalesce reads of one engine folder, including simultaneous picker opens.

use std::{collections::HashMap, sync::Arc, time::Duration};

use futures_util::future::{BoxFuture, FutureExt, Shared};
use tokio::{
    sync::{oneshot, watch},
    time::Instant,
};

use super::Catalog;
use crate::{
    Engine,
    lifetime::{Demand, Lease},
};

const RETAIN_FOR: Duration = Duration::from_secs(10);
const PICKER_REOPEN: Duration = Duration::from_secs(2);
const MAX_FOLDERS: usize = 200;

type Lists = Option<Arc<Catalog>>;
type Reading = Shared<BoxFuture<'static, Lists>>;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Key {
    pid: u32,
    port: u16,
    directory: String,
}

#[derive(Clone)]
struct Entry {
    number: u64,
    until: Option<Instant>,
    reading: Reading,
}

#[derive(Default)]
struct Reads {
    number: u64,
    ordinary: HashMap<Key, Entry>,
    picker: HashMap<Key, Entry>,
}

impl Reads {
    fn entries(&mut self, refresh: bool) -> &mut HashMap<Key, Entry> {
        if refresh {
            &mut self.picker
        } else {
            &mut self.ordinary
        }
    }
}

#[derive(Clone)]
pub(crate) struct CatalogCache(watch::Sender<Reads>);

impl CatalogCache {
    pub(crate) fn new() -> Self {
        Self(watch::channel(Reads::default()).0)
    }

    pub(crate) fn invalidate(&self, keep_picker_open: bool) {
        self.0.send_modify(|reads| {
            reads.ordinary.clear();
            if !keep_picker_open {
                reads.picker.clear();
            }
        });
    }

    pub(crate) fn event(&self, kind: &str, data: &serde_json::Value) {
        if matches!(
            kind,
            "credential.updated"
                | "credential.switched"
                | "integration.updated"
                | "model.updated"
                | "provider.updated"
                | "config.updated"
                | "global.disposed"
                | "kuvo.auto.updated"
                | "kuvo.agent.account.changed"
        ) {
            self.invalidate(kind == "model.updated" && data["pickerRefresh"] == true);
        }
    }

    pub(super) async fn get(
        &self,
        engine: &Engine,
        lease: &Lease,
        directory: &str,
        refresh: bool,
    ) -> Lists {
        let key = Key {
            pid: lease.record.pid,
            port: lease.record.port,
            directory: directory.to_owned(),
        };
        let now = Instant::now();
        let mut selected = None;
        let mut start = None;
        self.0.send_modify(|reads| {
            if let Some(entry) = reads.entries(refresh).get(&key)
                && entry.until.is_none_or(|until| now < until)
            {
                selected = Some(entry.reading.clone());
                return;
            }
            let (send, answer) = oneshot::channel::<Lists>();
            let reading = async move { answer.await.ok().flatten() }.boxed().shared();
            reads.number = reads.number.wrapping_add(1);
            let number = reads.number;
            remember(
                reads.entries(refresh),
                key.clone(),
                Entry {
                    number,
                    until: (!refresh).then_some(now + RETAIN_FOR),
                    reading: reading.clone(),
                },
            );
            selected = Some(reading);
            start = Some((send, number));
        });
        if let Some((send, number)) = start {
            let engine = engine.clone();
            let cache = self.clone();
            // A dismissed picker must leave another client following the same read.
            tokio::spawn(async move {
                let found = load(&engine, &key, refresh).await.map(Arc::new);
                cache.finished(key, number, refresh, found.as_deref());
                let _ = send.send(found);
            });
        }
        selected.expect("each catalog request chooses a read").await
    }

    fn finished(&self, key: Key, number: u64, refresh: bool, found: Option<&Catalog>) {
        let now = Instant::now();
        self.0.send_modify(|reads| {
            let entries = reads.entries(refresh);
            let Some(entry) = entries.get_mut(&key).filter(|entry| entry.number == number) else {
                return;
            };
            if let Some(found) = found
                && (refresh || found.fallback.is_some())
            {
                if refresh {
                    entry.until = Some(now + PICKER_REOPEN);
                    let mut entry = entry.clone();
                    entry.until = Some(now + RETAIN_FOR);
                    remember(&mut reads.ordinary, key, entry);
                }
            } else {
                entries.remove(&key);
            }
        });
    }
}

fn remember(entries: &mut HashMap<Key, Entry>, key: Key, entry: Entry) {
    entries.insert(key, entry);
    if entries.len() > MAX_FOLDERS
        && let Some(oldest) = entries
            .iter()
            .min_by_key(|(_, entry)| entry.number)
            .map(|(key, _)| key.clone())
    {
        entries.remove(&oldest);
    }
}

async fn load(engine: &Engine, key: &Key, refresh: bool) -> Option<Catalog> {
    let lease = engine
        .acquire(Demand::Passive)
        .await
        .filter(|lease| lease.record.pid == key.pid && lease.record.port == key.port)?;
    if refresh {
        crate::model_refresh::revalidate_picker(engine, &lease, &key.directory).await;
    }
    Catalog::read(engine, &lease, &key.directory).await
}
