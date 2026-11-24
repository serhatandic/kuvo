//! A bot's work and what's asked before it (PROTOCOL.md, "Work"): where new projects go here
//! (`kuvo/work/settings`), and this Mac kept awake while agents work.

pub mod settings;

use http::StatusCode;
use kuvo_protocol::routes::{
    get_kuvo_work_settings as settings_get, patch_kuvo_work_settings as settings_patch,
};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use serde_json::Value;

pub use settings::WorkSettings;

fn typed<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, Refusal> {
    serde_json::from_value(value)
        .map_err(|error| Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
}

fn refused((status, message): (u16, String)) -> Refusal {
    Refusal::new(
        StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST),
        message,
    )
}

/// The work routes this part answers.
pub fn register(settings: &WorkSettings, routes: &mut Registry) {
    let here = settings.clone();
    routes.register::<settings_get::Spec, _, _>(Access::Any, move |_| {
        let settings = here.clone();
        async move { Ok(RouteReply::json::<200>(typed(settings.shown())?)) }
    });
    let here = settings.clone();
    routes.register::<settings_patch::Spec, _, _>(Access::Any, move |call| {
        let settings = here.clone();
        async move {
            let given = serde_json::from_slice(&call.request.body).unwrap_or(Value::Null);
            let changed = settings.change(&given).map_err(refused)?;
            Ok(RouteReply::json::<200>(typed(changed)?))
        }
    });
}
