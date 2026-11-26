//! Browser asks are presented by the existing asks routes. Their answers are
//! delivered to just the waiting tool, not to its model or the other tools.
use kuvo_store::files;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{broadcast, oneshot};
use tokio_util::sync::CancellationToken;

struct Pending {
    params: Value,
    reply: oneshot::Sender<Value>,
}
#[derive(Clone)]
pub struct BrowserAsks(Arc<Inner>);
struct Inner {
    run: PathBuf,
    waiting: Mutex<HashMap<String, Pending>>,
    events: broadcast::Sender<Value>,
}
impl BrowserAsks {
    pub(super) fn new(run: PathBuf, events: broadcast::Sender<Value>) -> Self {
        Self(Arc::new(Inner {
            run,
            waiting: Mutex::default(),
            events,
        }))
    }
    fn record(&self, waiting: &HashMap<String, Pending>) {
        let file = self.0.run.join("browser-asks.json");
        if waiting.is_empty() {
            let _ = files::remove_file_synced(&file);
            return;
        }
        let value = json!({"asks":waiting.values().map(|ask|&ask.params).collect::<Vec<_>>()});
        let _ = files::write_secret(&file, value.to_string().as_bytes());
    }
    pub(super) fn pending(&self) -> Vec<Value> {
        self.0
            .waiting
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(|a| a.params.clone())
            .collect()
    }
    pub fn answer(&self, id: &str, given: Value) -> bool {
        let mut waiting = self.0.waiting.lock().unwrap_or_else(|e| e.into_inner());
        let Some(ask) = waiting.remove(id) else {
            return false;
        };
        self.record(&waiting);
        let _ = self
            .0
            .events
            .send(json!({"method":"Kuvo.askClosed","params":{"id":id}}));
        ask.reply.send(given).is_ok()
    }
    pub(super) fn clear(&self) {
        let mut waiting = self.0.waiting.lock().unwrap_or_else(|e| e.into_inner());
        waiting.clear();
        self.record(&waiting);
    }
    pub(super) async fn ask(&self, mut params: Value, cancel: &CancellationToken) -> Value {
        if cancel.is_cancelled() {
            return json!({"type":"cancelled"});
        }
        let mut bytes = [0; 16];
        let _ = getrandom::fill(&mut bytes);
        let id = hex::encode(bytes);
        params["id"] = json!(id);
        let (reply, answer) = oneshot::channel();
        {
            let mut waiting = self.0.waiting.lock().unwrap_or_else(|e| e.into_inner());
            waiting.insert(
                id.clone(),
                Pending {
                    params: params.clone(),
                    reply,
                },
            );
            self.record(&waiting);
        }
        let _ = self
            .0
            .events
            .send(json!({"method":"Kuvo.asked","params":params}));
        let _guard = Guard {
            asks: self.clone(),
            id,
        };
        tokio::select! {
            reply=answer=>reply.unwrap_or_else(|_|json!({"type":"lost"})),
            ()=cancel.cancelled()=>json!({"type":"cancelled"}),
            ()=tokio::time::sleep(Duration::from_secs(900))=>json!({"type":"timedOut"}),
        }
    }
}
struct Guard {
    asks: BrowserAsks,
    id: String,
}
impl Drop for Guard {
    fn drop(&mut self) {
        let mut waiting = self
            .asks
            .0
            .waiting
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if waiting.remove(&self.id).is_some() {
            self.asks.record(&waiting);
            let _ = self
                .asks
                .0
                .events
                .send(json!({"method":"Kuvo.askClosed","params":{"id":self.id}}));
        }
    }
}
