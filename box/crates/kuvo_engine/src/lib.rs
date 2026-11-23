//! The resident's on-demand OpenCode and its API passthrough.

mod api;
mod context;
mod conversation;
mod disk;
mod launch;
mod lifetime;
mod naming;
mod parts;
mod proxy;
mod queue;
mod send;
mod session_list;
mod sessions;
mod start_session;

use http::StatusCode;
use kuvo_protocol::routes::{
    get_kuvo_sessions_page as page, post_kuvo_engine_restart as restart,
    post_kuvo_engine_start as start,
};
use kuvo_server::{Access, Refusal, Registry, RouteReply};

pub use launch::pinned_version;
pub use lifetime::{Engine, State, Timing};
pub use session_list::LocalSessions;

impl Engine {
    pub fn register(&self, routes: &mut Registry) -> LocalSessions {
        self.register_with_merged_pages(routes, |_, _, _| None)
    }

    pub fn register_with_merged_pages(
        &self,
        routes: &mut Registry,
        merged_page: impl Fn(&str, usize, u32) -> Option<page::Status200> + Send + Sync + 'static,
    ) -> LocalSessions {
        let list = LocalSessions::start(self.clone());
        session_list::register(list.clone(), routes, merged_page);
        parts::register(routes);
        queue::register(self, routes);
        send::register(self, list.clone(), routes);
        sessions::register(self, routes);
        start_session::register(self, list.clone(), routes);
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
        list
    }
}

fn control_refusal(message: &'static str) -> Refusal {
    Refusal::new(StatusCode::CONFLICT, message)
}

fn query(path: &str, key: &str) -> Option<String> {
    let (_, query) = path.split_once('?')?;
    let parsed = reqwest::Url::parse(&format!("http://localhost/?{query}")).ok()?;
    parsed
        .query_pairs()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.into_owned())
}
