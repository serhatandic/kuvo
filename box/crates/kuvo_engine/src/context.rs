//! The occupancy of the last measured step and the model window cached for offline reads.

use std::{collections::BTreeMap, path::Path, time::Duration};

use http::HeaderMap;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{Engine, launch::Record};

const DEADLINE: Duration = Duration::from_millis(1500);
const MAX_TOKEN_COUNT: u64 = (1 << 53) - 1;

#[derive(Deserialize)]
struct ModelRef {
    #[serde(rename = "providerID")]
    provider_id: String,
    id: String,
}

#[derive(Deserialize)]
struct Model {
    #[serde(flatten)]
    reference: ModelRef,
    #[serde(default)]
    aliases: Vec<String>,
    limit: Option<Limit>,
}

#[derive(Deserialize)]
struct Limit {
    context: Option<u64>,
}

pub(crate) async fn enrich(
    engine: &Engine,
    record: &Record,
    path: &str,
    headers: &HeaderMap,
    session: &mut Map<String, Value>,
) {
    if session
        .get("id")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return;
    }
    let client = &engine.upstream;
    let messages_path = format!("{path}/message?limit=8&order=desc");
    let read_messages = async {
        if session.get("revert").is_some_and(|value| !value.is_null()) {
            return Vec::new();
        }
        client
            .get_data::<Vec<Value>>(record, &messages_path, headers, DEADLINE)
            .await
            .unwrap_or_default()
    };
    // Decode each catalog entry independently so an incomplete entry cannot discard the rest.
    let (messages, models) = tokio::join!(
        read_messages,
        client.get_data::<Vec<Value>>(record, "api/model", headers, DEADLINE),
    );
    let step = messages
        .iter()
        .take_while(|message| message["type"] != "compaction" || message["status"] == "failed")
        .filter(|message| message["type"] == "assistant" && message["error"].is_null())
        .find_map(|message| occupancy(&message["tokens"]).map(|used| (used, message.get("model"))));

    let reference = step
        .and_then(|(_, model)| model.filter(|model| !model.is_null()))
        .or_else(|| session.get("model"))
        .and_then(|model| ModelRef::deserialize(model).ok());

    let mut window = None;
    if let Some(reference) = reference {
        let live = models
            .unwrap_or_default()
            .into_iter()
            .filter_map(|value| serde_json::from_value::<Model>(value).ok())
            .find(|model| {
                model.reference.provider_id == reference.provider_id
                    && (model.reference.id == reference.id || model.aliases.contains(&reference.id))
            })
            .and_then(|model| model.limit?.context)
            .filter(|&count| valid_capacity(count));

        let key = json!([reference.provider_id, reference.id]).to_string();
        window = engine.context_windows.lookup(key, live).await;
    }

    if let Some(window) = window {
        session.insert("contextWindow".into(), json!(window));
    }
    if let Some((used, _)) = step {
        let mut context = json!({ "used": used });
        if let Some(window) = window {
            context["window"] = json!(window);
        }
        session.insert("context".into(), context);
    }
}

fn occupancy(tokens: &Value) -> Option<u64> {
    let count = |value: &Value| value.as_u64().filter(|&count| count <= MAX_TOKEN_COUNT);
    let optional = |value: &Value| {
        if value.is_null() {
            Some(0)
        } else {
            count(value)
        }
    };
    let used = count(&tokens["input"])?
        + optional(&tokens["output"])?
        + optional(&tokens["reasoning"])?
        + optional(&tokens["cache"]["read"])?
        + optional(&tokens["cache"]["write"])?;
    (used > 0).then_some(used)
}

fn valid_capacity(count: u64) -> bool {
    count > 0 && count <= MAX_TOKEN_COUNT
}

type Lookup = (
    String,
    Option<u64>,
    tokio::sync::oneshot::Sender<Option<u64>>,
);

#[derive(Clone)]
pub(crate) struct Windows(tokio::sync::mpsc::Sender<Lookup>);

impl Windows {
    pub(crate) fn open(home: &Path) -> Self {
        let cache = home.join("cache/engine-context-windows.json");
        let (send, mut inbox) = tokio::sync::mpsc::channel::<Lookup>(32);
        let (saved, mut writes) = tokio::sync::watch::channel(None);
        let file = cache.clone();
        tokio::spawn(async move {
            while writes.changed().await.is_ok() {
                let snapshot: Option<std::sync::Arc<BTreeMap<String, Value>>> =
                    writes.borrow_and_update().clone();
                if let Some(snapshot) = snapshot {
                    let file = file.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        crate::disk::write(&file, snapshot.as_ref())
                    })
                    .await;
                }
            }
        });
        tokio::spawn(async move {
            let mut windows: BTreeMap<String, Value> = tokio::fs::read(&cache)
                .await
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_default();
            while let Some((key, live, answer)) = inbox.recv().await {
                let previous = windows
                    .get(&key)
                    .and_then(Value::as_u64)
                    .filter(|&count| valid_capacity(count));
                if let Some(live) = live.filter(|live| Some(*live) != previous) {
                    windows.insert(key, json!(live));
                    saved.send_replace(Some(std::sync::Arc::new(windows.clone())));
                }
                let _ = answer.send(live.or(previous));
            }
        });
        Self(send)
    }

    async fn lookup(&self, key: String, live: Option<u64>) -> Option<u64> {
        let (answer, result) = tokio::sync::oneshot::channel();
        if self.0.send((key, live, answer)).await.is_err() {
            return live;
        }
        result.await.unwrap_or(live)
    }
}
