//! The resident's on-demand OpenCode and its API passthrough.

mod agent_models;
mod api;
mod asks;
mod attachment_retention;
mod attachments;
mod consent;
mod context;
mod conversation;
mod delivery;
mod disk;
mod external_asks;
mod launch;
mod lifetime;
mod model_preferences;
mod model_refresh;
mod models;
mod naming;
mod notes;
mod ownership;
mod parts;
mod proxy;
mod queue;
mod raw_prompts;
mod receipt_index;
mod search;
mod send;
mod session_list;
mod session_move;
mod sessions;
mod start_session;
mod submissions;
mod transcript;
mod turns;
mod typed;
mod waiting;

use http::StatusCode;
use kuvo_protocol::routes::{
    get_kuvo_sessions_page as page, post_kuvo_engine_restart as restart,
    post_kuvo_engine_start as start,
};
use kuvo_server::{Access, Refusal, Registry, RouteReply};

pub const FEATURES: &[&str] = &[
    "models.speed",
    "models.starred",
    "search",
    "search.setup",
    "sessions.models",
    "typed",
    "typed.draft",
    "typed.delivery",
    "typed.submission-recovery",
];

pub use agent_models::{
    AgentCatalog, AgentModel, AgentModelSource, AutoModel, CatalogRead, ModelAccount,
};
pub use attachment_retention::{AttachmentConversation, AttachmentRead, AttachmentSource};
pub use external_asks::{
    AskResult, ExternalAsk, ExternalAskKind, ExternalAsks, ExternalCard, ExternalDecision,
    ExternalReply, ExternalWaiting,
};
pub use launch::pinned_version;
pub use lifetime::{Engine, State, Timing};
pub use model_preferences::{ModelStar, ModelStarSource, StarSave};
pub use models::ModelSpeed;
pub use ownership::{
    ComputerKey, DeviceAccess, OwnedReply, OwnedRequest, OwnerAction, OwnerScope, PeerReply,
    SessionOwner, SessionOwnership, Submitter,
};
pub use search::{
    PeerSearch, SearchAvailability, SearchComputer, SearchPeer, SearchPeers, SearchResult,
};
pub use session_list::{Activity, ActivityState, LocalSessions};
pub use session_move::{MoveEngineError, SessionMove};
pub use transcript::{TranscriptError, TranscriptScope};
pub use waiting::PeerWaiting;

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
        asks::register(self, list.clone(), routes);
        waiting::register(self, list.clone(), routes);
        parts::register(routes);
        model_preferences::register(self, routes);
        model_refresh::register(self, routes);
        models::register(self, list.clone(), routes);
        search::register(self, routes);
        queue::register(self, routes);
        send::register(self, list.clone(), routes);
        sessions::register(self, routes);
        start_session::register(self, list.clone(), routes);
        typed::register(self, list.clone(), routes);
        let delivery = delivery::register(self, list.clone(), routes);
        submissions::register(self, list.clone(), delivery, routes);
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
