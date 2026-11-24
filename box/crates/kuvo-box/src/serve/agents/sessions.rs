//! Read saved agent conversations through the resident.

use http::StatusCode;
use kuvo_agents::session_store::{MessageOrder, SessionStore};
use kuvo_protocol::routes::{
    get_kuvo_agents_api_session_id_message as transcript,
    types::GetKuvoAgentsApiSessionIdMessageStatus200Cursor as Cursor,
};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use percent_encoding::percent_decode_str;

pub(crate) fn register(sessions: SessionStore, routes: &mut Registry) {
    routes.register::<transcript::Spec, _, _>(Access::Any, move |call| {
        let sessions = sessions.clone();
        async move {
            let given = call.param("id").unwrap_or_default();
            let id = percent_decode_str(given)
                .decode_utf8()
                .map_err(|_| missing(given))?;
            let query = TranscriptQuery::parse(&call.request.path);
            let page = sessions
                .message_page(&id, query.after, query.limit, query.order)
                .await
                .map_err(|error| {
                    eprintln!("Kuvo could not read the coding-agent transcript {id}: {error}");
                    Refusal::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "The conversation couldn't be read. Try again in a moment.",
                    )
                })?
                .ok_or_else(|| missing(&id))?;
            let next = page
                .messages
                .last()
                .and_then(|message| message["id"].as_str())
                .filter(|_| page.more)
                .map(|last| {
                    let order = &query.cursor_order;
                    match query.filter.filter(|filter| !filter.is_empty()) {
                        Some(filter) => format!("{order}:{last}:{filter}"),
                        None => format!("{order}:{last}"),
                    }
                });
            Ok(RouteReply::json::<200>(transcript::Status200 {
                data: page.messages,
                cursor: Cursor { next },
            }))
        }
    });
}

pub(super) fn missing(id: &str) -> Refusal {
    Refusal::new(StatusCode::NOT_FOUND, format!("No session {id}."))
}

struct TranscriptQuery {
    after: Option<String>,
    limit: u32,
    order: MessageOrder,
    cursor_order: String,
    filter: Option<String>,
}

impl TranscriptQuery {
    fn parse(path: &str) -> Self {
        let query = path.split_once('?').map_or("", |(_, query)| query);
        let get = |key| {
            query
                .split('&')
                .filter_map(|part| part.split_once('='))
                .find(|(name, _)| *name == key)
                .and_then(|(_, value)| {
                    percent_decode_str(&value.replace('+', " "))
                        .decode_utf8()
                        .ok()
                        .map(|value| value.into_owned())
                })
        };
        let cursor = get("cursor").filter(|value| !value.is_empty());
        let mut parts = cursor
            .as_deref()
            .into_iter()
            .flat_map(|text| text.split(':'));
        let from_cursor = parts.next();
        let after = cursor.as_ref().map(|_| parts.next().unwrap_or("").into());
        let given_order = get("order");
        let oldest = given_order.as_deref() == Some("asc")
            || from_cursor.is_some_and(|text| text.starts_with("asc"));
        let limit = get("limit")
            .and_then(|limit| limit.trim().parse::<u32>().ok())
            .filter(|limit| *limit != 0)
            .unwrap_or(100);
        Self {
            after,
            limit,
            order: if oldest {
                MessageOrder::OldestFirst
            } else {
                MessageOrder::NewestFirst
            },
            cursor_order: from_cursor
                .map(str::to_owned)
                .or(given_order)
                .unwrap_or_else(|| "desc".into()),
            filter: get("parentID").or_else(|| parts.next().map(str::to_owned)),
        }
    }
}
