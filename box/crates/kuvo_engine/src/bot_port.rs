//! The engine's side of bots: a bot's conversation made, told what it is, renamed, put on another
//! model and read, as the box's bot routes reach the engine. The handle keeps the engine up for as
//! long as it's held.

use std::time::Duration;

use http::Method;
use kuvo_server::Refusal;
use serde_json::Value;

use crate::{
    Engine, NoteEdits,
    lifetime::{Demand, Lease},
    sessions,
    wake_port::Unreachable,
};

/// The engine, held up for a bot's change.
pub struct BotEngine {
    engine: Engine,
    lease: Lease,
}

impl Engine {
    /// The engine for a bot's change, started where it isn't running; none where it can't be.
    pub async fn bot_engine(&self) -> Option<BotEngine> {
        self.bot_engine_as(Demand::Start).await
    }

    /// The engine for bots, only while it's running already.
    pub async fn running_bot_engine(&self) -> Option<BotEngine> {
        self.bot_engine_as(Demand::Existing).await
    }

    async fn bot_engine_as(&self, demand: Demand) -> Option<BotEngine> {
        let lease = self.acquire(demand).await?;
        Some(BotEngine {
            engine: self.clone(),
            lease,
        })
    }
}

impl BotEngine {
    /// One of the engine's routes, its answer's `data` where it has one; why not where it failed.
    pub async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        wait: Duration,
    ) -> Result<Value, Unreachable> {
        let mut value = self
            .engine
            .call_raw(&self.lease, method, path, body, wait)
            .await?;
        if let Some(data) = value.get_mut("data").filter(|data| !data.is_null()) {
            value = data.take();
        }
        Ok(value)
    }

    /// One of the engine's routes about `folder` (its commands, its tools), its answer's `data`.
    pub async fn call_in(
        &self,
        method: Method,
        path: &str,
        folder: &str,
        wait: Duration,
    ) -> Result<Value, Unreachable> {
        let named = http::HeaderValue::from_str(&crate::api::segment(folder));
        let named =
            named.map_err(|_| Unreachable("That folder can't be named to the engine.".into()))?;
        let headers = http::HeaderMap::from_iter([(
            http::HeaderName::from_static("x-opencode-directory"),
            named,
        )]);
        let mut value = self
            .engine
            .call_with_headers(&self.lease, method, path, None, wait, headers)
            .await?;
        if let Some(data) = value.get_mut("data").filter(|data| !data.is_null()) {
            value = data.take();
        }
        Ok(value)
    }

    /// One of the engine's routes, its answer whole (a page of sessions with its `cursor`).
    pub async fn call_whole(
        &self,
        method: Method,
        path: &str,
        wait: Duration,
    ) -> Result<Value, Unreachable> {
        Ok(self
            .engine
            .call_raw(&self.lease, method, path, None, wait)
            .await?)
    }

    /// Whether the engine has the conversation.
    pub async fn has_session(&self, session: &str) -> bool {
        sessions::valid_id(session)
            && self
                .call(Method::GET, &sessions::path(session), None, WAIT)
                .await
                .is_ok()
    }

    /// The conversation deleted; whether it's gone (or was).
    pub async fn delete_session(&self, session: &str) -> bool {
        if !sessions::valid_id(session) {
            return true;
        }
        match self
            .engine
            .call_raw(
                &self.lease,
                Method::DELETE,
                &sessions::path(session),
                None,
                WAIT * 3,
            )
            .await
        {
            Ok(_) => true,
            Err(error) => error.missing(),
        }
    }

    /// The model a new conversation starts on here, by the box's rule (`models.rs`): the default,
    /// else the first that runs. `None` while the engine can't say; `Some(None)` for none.
    pub async fn starting_model(&self, directory: &str) -> Option<Option<Value>> {
        crate::models::starting_here(&self.engine, &self.lease, directory).await
    }

    /// The conversation's rules and notes changed in its turn, as `change` makes them from the
    /// conversation as it is then (nothing for no change). Refused where it isn't there.
    pub async fn rewrite(
        &self,
        session: &str,
        change: impl FnOnce(&Value) -> Option<NoteEdits>,
    ) -> Result<(), Refusal> {
        let mut writer = self.engine.note_writer(session).await?;
        let now = writer.current().await?;
        if let Some(edits) = change(&now) {
            writer.edit(edits).await?;
        }
        Ok(())
    }
}

const WAIT: Duration = Duration::from_secs(10);
