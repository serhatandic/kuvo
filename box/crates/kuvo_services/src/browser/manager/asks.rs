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
use tokio::{
    sync::{broadcast, oneshot},
    time::Instant,
};
use tokio_util::sync::CancellationToken;

struct Pending {
    params: Value,
    reply: oneshot::Sender<Value>,
}
#[derive(Clone)]
pub struct BrowserAsks(Arc<Inner>);
struct Inner {
    run: PathBuf,
    state: Mutex<State>,
    events: broadcast::Sender<Value>,
}
#[derive(Default)]
struct State {
    waiting: HashMap<String, Pending>,
    sites: HashMap<String, Instant>,
}
impl BrowserAsks {
    pub(super) fn new(run: PathBuf, events: broadcast::Sender<Value>) -> Self {
        Self(Arc::new(Inner {
            run,
            state: Mutex::default(),
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
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .waiting
            .values()
            .map(|a| a.params.clone())
            .collect()
    }
    pub fn answer(&self, id: &str, given: Value) -> bool {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(ask) = state.waiting.remove(id) else {
            return false;
        };
        self.record(&state.waiting);
        let _ = self
            .0
            .events
            .send(json!({"method":"Kuvo.askClosed","params":{"id":id}}));
        ask.reply.send(given).is_ok()
    }
    pub(super) fn mark_passkey(&self, host: &str) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state
            .sites
            .retain(|_, at| at.elapsed() < Duration::from_secs(600));
        state.sites.insert(host.into(), Instant::now());
        for ask in state.waiting.values_mut() {
            if ask.params["host"] == host {
                ask.params["passkey"] = json!(true);
            }
        }
        self.record(&state.waiting);
    }
    pub(super) fn clear(&self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.waiting.clear();
        state.sites.clear();
        self.record(&state.waiting);
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
            let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
            if params["host"].as_str().is_some_and(|host| {
                state
                    .sites
                    .get(host)
                    .is_some_and(|at| at.elapsed() < Duration::from_secs(600))
            }) {
                params["passkey"] = json!(true);
            }
            state.waiting.insert(
                id.clone(),
                Pending {
                    params: params.clone(),
                    reply,
                },
            );
            self.record(&state.waiting);
            let _ = self
                .0
                .events
                .send(json!({"method":"Kuvo.asked","params":params}));
        }
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
        let mut state = self.asks.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.waiting.remove(&self.id).is_some() {
            self.asks.record(&state.waiting);
            let _ = self
                .asks
                .0
                .events
                .send(json!({"method":"Kuvo.askClosed","params":{"id":self.id}}));
        }
    }
}
