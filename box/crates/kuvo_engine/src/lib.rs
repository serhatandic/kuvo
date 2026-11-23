//! The resident's on-demand OpenCode and its API passthrough.

mod launch;
mod lifetime;
mod proxy;

use http::StatusCode;
use kuvo_protocol::routes::{post_kuvo_engine_restart as restart, post_kuvo_engine_start as start};
use kuvo_server::{Access, Refusal, Registry, RouteReply};

pub use launch::pinned_version;
pub use lifetime::{Engine, State, Timing};

impl Engine {
    pub fn register(&self, routes: &mut Registry) {
        let engine = self.clone();
        routes.engine(move |request, cancel| proxy::http(engine.clone(), request, cancel));
        let engine = self.clone();
        routes.sockets(move |request, cancel| proxy::socket(engine.clone(), request, cancel));

        let engine = self.clone();
        routes.register::<start::Spec, _, _>(Access::Any, move |_| {
            let engine = engine.clone();
            async move {
                if engine.is_mac_box().await {
                    return Err(Refusal::json(StatusCode::NOT_FOUND, serde_json::json!({})));
                }
                engine
                    .control(lifetime::Control::Start)
                    .await
                    .map_err(control_refusal)?;
                Ok(RouteReply::json::<202>(start::Status202 {}))
            }
        });

        let engine = self.clone();
        routes.register::<restart::Spec, _, _>(Access::Any, move |_| {
            let engine = engine.clone();
            async move {
                engine
                    .control(lifetime::Control::Restart)
                    .await
                    .map_err(control_refusal)?;
                Ok(RouteReply::json::<202>(restart::Status202 {}))
            }
        });
    }
}

fn control_refusal(message: &'static str) -> Refusal {
    Refusal::new(StatusCode::CONFLICT, message)
}
