//! This computer's bots as the Marketplace's Add Agent reaches them (`AgentBots`): the models it
//! offers, a bot as it's listed, and a bot made with an agent's job and its folder prepared.

use futures_util::{FutureExt, future::BoxFuture};
use kuvo_services::marketplace::bots::{AgentBots, AgentSetup};
use serde_json::{Value, json};

use super::{
    Bots,
    job::Job,
    kit::runs_with_agents,
    make::{Setup, bot_model},
};

impl AgentBots for Bots {
    fn offered<'a>(
        &'a self,
        wanted: Option<&'a Value>,
        start: bool,
    ) -> BoxFuture<'a, Option<Value>> {
        async move {
            self.inner
                .engine
                .offered_here(&self.inner.sessions, wanted, start)
                .await
        }
        .boxed()
    }

    fn listed(&self, id: &str) -> Option<Value> {
        self.listed_one(id)
    }

    fn runs_with_agents(&self, model: &Value) -> bool {
        runs_with_agents(bot_model(model).as_ref())
    }

    fn make(&self, given: Value, setup: AgentSetup) -> BoxFuture<'_, (u16, Value)> {
        let setup = Setup {
            job: Job {
                line: setup.line,
                details: setup.details,
            },
            prepare: setup.prepare,
        };
        async move {
            match self.make_with(given, Some(setup)).await {
                Ok(made) => (201, made),
                Err((status, message)) => (status, json!({ "message": message })),
            }
        }
        .boxed()
    }
}
