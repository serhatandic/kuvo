use std::sync::Arc;

use http::StatusCode;
use kuvo_protocol::routes::{
    get_kuvo_vault_asks as list, get_kuvo_vault_asks_events as events,
    post_kuvo_vault_asks_id as answer,
};
use kuvo_server::{Access, JsonLine, Refusal, Registry, RouteReply};
use serde_json::{Value, json};

use super::{
    Error,
    asks::{Answer, Decision},
    routes::{Resident, VerifiedVaultClient, body, typed},
    store::missing,
};

pub(super) fn register(resident: &Arc<Resident>, routes: &mut Registry) {
    let list_resident = resident.clone();
    routes.register::<list::Spec, _, _>(Access::Any, move |call| {
        let resident = list_resident.clone();
        async move {
            resident.block(|local| {
                local.client_as(&call.request, call.extension::<VerifiedVaultClient>())
            })?;
            typed::<_, 200>(json!({ "asks": resident.broker.asks.list() }))
        }
    });
    let events_resident = resident.clone();
    routes.register::<events::Spec, _, _>(Access::Any, move |call| {
        let resident = events_resident.clone();
        async move {
            resident.block(|local| {
                local.client_as(&call.request, call.extension::<VerifiedVaultClient>())
            })?;
            let mut stream = resident.broker.asks.events();
            let cancel = call.cancel;
            Ok(RouteReply::ndjson::<200>(async_stream::try_stream! {
                loop {
                    let asks = stream.borrow_and_update().clone();
                    let body = serde_json::from_value(json!({ "asks": asks }))?;
                    yield JsonLine::<events::Spec, 200>::new(body)?;
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        result = stream.changed() => if result.is_err() { break },
                    };
                }
            }))
        }
    });
    let answer_resident = resident.clone();
    routes.register::<answer::Spec, _, _>(Access::Any, move |call| {
        let resident = answer_resident.clone();
        async move {
            let by = resident.block(|local| {
                local.client_as(&call.request, call.extension::<VerifiedVaultClient>())
            })?;
            let given = body(&call.request);
            let decision = given
                .get("answer")
                .and_then(|value| serde_json::from_value::<Decision>(value.clone()).ok())
                .ok_or_else(|| Error::refused(400, "Answer “once”, “always” or “no”."))?;
            let item = given
                .get("item")
                .and_then(Value::as_str)
                .filter(|item| !item.is_empty())
                .map(str::to_owned);
            if let Some(item) = &item {
                resident.block(|local| local.vault.item(item)?.ok_or_else(missing))?;
            }
            let id = call
                .param("id")
                .and_then(|id| percent_encoding::percent_decode_str(id).decode_utf8().ok());
            if !id.is_some_and(|id| {
                resident.broker.asks.answer(
                    &id,
                    Answer {
                        decision,
                        item,
                        by: Some(by),
                    },
                )
            }) {
                return Err(Refusal::json(StatusCode::NOT_FOUND, json!({})));
            }
            typed::<_, 200>(json!({}))
        }
    });
}
