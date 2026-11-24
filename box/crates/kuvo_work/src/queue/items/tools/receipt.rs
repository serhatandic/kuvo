//! Durable operation claims shared with the TypeScript tool adapter.

use super::*;

#[derive(Clone)]
pub(super) struct Operation {
    pub id: String,
    digest: String,
}

impl Operation {
    pub fn for_call(headers: &HeaderMap, given: &Value) -> Result<Option<Self>, Refusal> {
        if given.get("operation").is_none() && !headers.contains_key("x-kuvo-operation") {
            return Ok(None);
        }
        let nonce = nonce(headers, given).map_err(|_| bad("Invalid tool operation id."))?;
        let scope = json!(["tool", "kuvo/work-queue/tools", given["session"]]).to_string();
        Ok(Some(Self {
            id: key(json!([scope, nonce])),
            digest: key(given.clone()),
        }))
    }

    fn path(&self, store: &Store) -> PathBuf {
        data(store)
            .join("mutations")
            .join(format!("{}.json", self.id))
    }

    pub fn claim(&self, store: &Store) -> Result<Option<Value>, Refusal> {
        let _lock = lock(store).map_err(storage_refusal)?;
        let path = self.path(store);
        if let Some(previous) = store::read(&path, RECEIPT).map_err(storage_refusal)? {
            if previous.get("digest").and_then(Value::as_str) != Some(&self.digest) {
                return Err(Refusal::new(
                    StatusCode::CONFLICT,
                    "That operation id is already in use.",
                ));
            }
            if let Some(answer) = previous.get("answer") {
                if answer["status"] != 200 {
                    let status = answer["status"]
                        .as_u64()
                        .and_then(|status| u16::try_from(status).ok())
                        .and_then(|status| StatusCode::from_u16(status).ok())
                        .unwrap_or(StatusCode::CONFLICT);
                    return Err(Refusal::json(status, answer["value"].clone()));
                }
                return Ok(Some(answer["value"].clone()));
            }
            return Ok(kept_answer(
                &store.items().map_err(storage_refusal)?,
                &self.id,
            ));
        }
        let fields = Fields::from_iter([("digest".into(), self.digest.clone().into())]);
        store::write(&path, RECEIPT, fields).map_err(|error| storage_refusal(error.into()))?;
        Ok(None)
    }

    pub fn finish(&self, store: &Store, answer: Value) -> Result<Value, Refusal> {
        let _lock = lock(store).map_err(storage_refusal)?;
        let path = self.path(store);
        let mut fields = store::read(&path, RECEIPT)
            .map_err(storage_refusal)?
            .unwrap_or_default();
        if fields.get("digest").and_then(Value::as_str) != Some(&self.digest) {
            return Err(Refusal::new(
                StatusCode::CONFLICT,
                "That operation id is already in use.",
            ));
        }
        if let Some(previous) = fields.get("answer") {
            return Ok(previous["value"].clone());
        }
        let now = Timestamp::now().as_millisecond();
        fields.insert("answer".into(), json!({"status":200,"value":answer}));
        fields.insert("at".into(), now.into());
        store::write(&path, RECEIPT, fields).map_err(|error| storage_refusal(error.into()))?;
        Ok(answer)
    }
}

pub(in crate::queue::items) fn kept_answer(items: &[Item], operation: &str) -> Option<Value> {
    items
        .iter()
        .find_map(|item| item.saved.get("toolReplies")?.get(operation).cloned())
}

fn data(store: &Store) -> PathBuf {
    store
        .root
        .parent()
        .expect("queue under run")
        .join("owned-data")
}

fn lock(store: &Store) -> Result<Option<StaleLock>, Error> {
    StaleLock::acquire(
        &data(store).join(".mutations.lock"),
        Duration::from_secs(10),
        Duration::from_secs(3),
        OnBusy::Refuse,
    )
    .map_err(Into::into)
}
