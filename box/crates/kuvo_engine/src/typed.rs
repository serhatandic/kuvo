//! Composer decisions and local draft admission using the same prompt journal as direct sends.

use std::{collections::BTreeMap, path::Path};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use http::{Method, StatusCode};
use http_body_util::BodyExt;
use kuvo_protocol::routes::{
    get_kuvo_sessions_id_typed as offers, post_kuvo_sessions_id_draft as draft,
    post_kuvo_sessions_id_typed as decide_route,
};
use kuvo_server::{
    Access, Caller, LocalOrigin, Refusal, Registry, RouteCall, RouteReply, Transport,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{Engine, LocalSessions, api, disk, models, send, sessions};

const KEEP_FOR: u64 = 60 * 60 * 1000;
const KEEP_MOST: usize = 1000;
const KEEP_BYTES: usize = 64 * 1024;
const UNAVAILABLE: &str = "The engine isn't answering. Try again in a moment.";
const DRAFT_FIELDS: &str = "Send a draft with its stable id, text and message fields.";
const UNCONFIRMED: &str =
    "The Box could not confirm the draft's outcome. Check the conversation before sending again.";

fn restore(status: StatusCode, message: &str, text: &str) -> Refusal {
    Refusal::json(
        status,
        json!({ "message": message, "restore": { "text": text } }),
    )
}

async fn decision(engine: &Engine, session: &str, text: &str) -> Result<Value, Refusal> {
    // A coding agent's conversation: its agent says whether it runs, when it takes the message.
    if engine
        .agent_session(session)
        .await
        .map_err(|_| restore(StatusCode::SERVICE_UNAVAILABLE, UNAVAILABLE, text))?
        .is_some()
    {
        return Ok(json!({ "send": { "text": text, "quoted": true } }));
    }
    let lease = api::start(engine).await.map_err(|_| {
        restore(
            StatusCode::SERVICE_UNAVAILABLE,
            "The engine isn't running.",
            text,
        )
    })?;
    let session = engine
        .call::<sessions::Session>(&lease, Method::GET, &sessions::path(session), None)
        .await
        .map_err(|error| {
            if error.missing() {
                restore(
                    StatusCode::NOT_FOUND,
                    "There's no such conversation here.",
                    text,
                )
            } else {
                restore(StatusCode::SERVICE_UNAVAILABLE, UNAVAILABLE, text)
            }
        })?;
    if let Some(model) = models::readiness(engine, &lease, &session).await
        && let Some(error) = model["refusal"].as_str()
    {
        let mut answer = json!({ "restore": { "text": text }, "error": error });
        if let Some(action) = model.get("action") {
            answer["action"] = action.clone();
        }
        return Ok(answer);
    }
    Ok(json!({ "send": { "text": text, "quoted": true } }))
}

#[derive(Default, Deserialize, Serialize)]
struct Decisions {
    #[serde(default, rename = "formatVersion")]
    version: u32,
    #[serde(flatten)]
    answers: BTreeMap<String, DecisionReceipt>,
}

#[derive(Deserialize, Serialize)]
struct DecisionReceipt {
    at: u64,
    status: u16,
    headers: BTreeMap<String, String>,
    body: String,
}

async fn read_decisions(path: &Path) -> Result<Decisions, Refusal> {
    let receipts = disk::read::<Decisions>(path)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    if receipts.version > 1 {
        return Err(Refusal::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "This computer can't read saved message decisions.",
        ));
    }
    Ok(receipts)
}

async fn typed(
    engine: &Engine,
    session: &str,
    text: &str,
    id: Option<String>,
) -> Result<Value, Refusal> {
    let Some(id) = id else {
        return decision(engine, session, text).await;
    };
    let key = format!("typed\n{session}\n{id}");
    let _turn = engine.draft_turns.acquire(&key).await;
    let file = engine.home.join("run/typed-sent.json");
    let receipts = read_decisions(&file).await?;
    let now = disk::now();
    if let Some(answer) = receipts.answers.get(&key)
        && answer.status == 200
        && now.saturating_sub(answer.at) <= KEEP_FOR
        && let Ok(bytes) = STANDARD.decode(&answer.body)
        && let Ok(value) = serde_json::from_slice(&bytes)
    {
        return Ok(value);
    }
    let answer = decision(engine, session, text).await?;
    let bytes = serde_json::to_vec(&answer).expect("serializable decision");
    if bytes.len() <= KEEP_BYTES {
        let _writer = engine
            .decision_writer
            .acquire()
            .await
            .expect("the decision writer stays open");
        let mut receipts = read_decisions(&file).await?;
        let at = disk::now();
        receipts.version = 1;
        receipts
            .answers
            .retain(|_, answer| at.saturating_sub(answer.at) <= KEEP_FOR);
        receipts.answers.insert(
            key,
            DecisionReceipt {
                at,
                status: 200,
                headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
                body: STANDARD.encode(bytes),
            },
        );
        let mut by_age = receipts.answers.into_iter().collect::<Vec<_>>();
        by_age.sort_by_key(|(_, answer)| answer.at);
        receipts.answers = by_age.into_iter().rev().take(KEEP_MOST).collect();
        let _ = disk::durable(file, receipts).await;
    }
    Ok(answer)
}

#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Draft {
    pub id: String,
    pub text: String,
    pub quoted_text: Option<String>,
    pub message: Map<String, Value>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DraftReceipt {
    format_version: u32,
    author: String,
    draft: Draft,
    decision: Value,
    outcome: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    submission: Option<String>,
}

pub(crate) struct DraftContext<'a> {
    pub caller: &'a Caller,
    pub author: &'a str,
    pub delivery_mode: Option<crate::delivery::Mode>,
    pub submission: Option<&'a str>,
    pub request: DraftRequest,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum DraftRequest {
    Direct,
    Durable,
}

pub(crate) fn author(caller: &Caller) -> Result<String, Refusal> {
    match caller {
        Caller::Paired { pairing, .. } => {
            Ok(format!("{}:{}", hex::encode(pairing.key), pairing.id))
        }
        Caller::Local(
            Transport::Http {
                origin: LocalOrigin::App | LocalOrigin::Mac,
                ..
            }
            | Transport::Ssh { .. },
        ) => Ok("computer".into()),
        Caller::Local(_) => Err(Refusal::new(
            StatusCode::FORBIDDEN,
            "Open the conversation as a paired Kuvo Code client.",
        )),
    }
}

fn store_error(_: std::io::Error) -> Refusal {
    Refusal::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "This computer couldn’t save the message receipt. Try again in a moment.",
    )
}

pub(crate) async fn submit(
    engine: &Engine,
    list: &LocalSessions,
    session: &str,
    draft: Draft,
    context: DraftContext<'_>,
) -> Result<Value, Refusal> {
    let DraftContext {
        caller,
        author,
        delivery_mode,
        submission,
        request,
    } = context;
    let key = disk::key(session, &draft.id);
    let _turn = engine.draft_turns.acquire(&format!("draft:{key}")).await;
    let path = engine
        .home
        .join("run/draft-receipts")
        .join(format!("{key}.json"));
    let saved = disk::read::<DraftReceipt>(&path).await.map_err(store_error)?;
    let recovering = saved.is_some();
    let mut receipt = match saved {
        Some(receipt) => {
            if receipt.format_version != 1 {
                return Err(Refusal::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "This computer can't read the saved draft.",
                ));
            }
            if receipt.author != author
                || receipt.draft != draft
                || receipt.submission.as_deref() != submission
            {
                return Err(Refusal::new(
                    StatusCode::CONFLICT,
                    "That operation id is already in use.",
                ));
            }
            if let Some(outcome) = receipt.outcome {
                return Ok(outcome);
            }
            receipt
        }
        None => {
            crate::submissions::guard_legacy_draft(engine, session, &draft.id, submission).await?;
            let decision = decision(engine, session, &draft.text).await?;
            DraftReceipt {
                format_version: 1,
                author: author.to_owned(),
                draft,
                decision,
                outcome: None,
                submission: submission.map(str::to_owned),
            }
        }
    };
    let answer = if let Some(text) = receipt
        .decision
        .pointer("/send/text")
        .and_then(Value::as_str)
    {
        crate::consent::current(caller, &tokio_util::sync::CancellationToken::new())?;
        let mut message = receipt.draft.message.clone();
        message.insert("id".into(), json!(receipt.draft.id));
        let quoted = receipt.decision.pointer("/send/quoted") == Some(&Value::Bool(true));
        let words = if quoted {
            receipt.draft.quoted_text.as_deref().unwrap_or(text)
        } else {
            text
        };
        message.insert("words".into(), json!(words));
        if let Some(mode) = delivery_mode {
            message.insert("delivery".into(), json!(mode.as_str()));
        }
        // Keep only fields the composer passes to the send route.
        message.retain(|key, _| {
            matches!(
                key.as_str(),
                "id" | "words" | "files" | "notes" | "delivery" | "comments" | "named" | "widget"
            )
        });
        // Record the model decision before the effect. A retry can reconcile an accepted
        // message without making a new choice based on a changed model catalog.
        disk::durable(path.clone(), receipt.clone())
            .await
            .map_err(store_error)?;
        match send::send_draft(engine, list, session, message, caller).await {
            Ok(_) => {
                json!({ "answer": receipt.decision, "message": receipt.draft.id, "acceptedAt": disk::now() })
            }
            Err(error) => {
                let reply = error.reply();
                let unconfirmed = reply.status.is_server_error();
                if unconfirmed && request == DraftRequest::Durable {
                    return Err(Refusal::new(StatusCode::SERVICE_UNAVAILABLE, UNAVAILABLE));
                }
                if unconfirmed && recovering {
                    return Err(Refusal::json(
                        StatusCode::CONFLICT,
                        json!({ "ambiguous": true, "message": UNCONFIRMED }),
                    ));
                }
                let value: Value = reply
                    .body
                    .collect()
                    .await
                    .ok()
                    .and_then(|body| serde_json::from_slice(&body.to_bytes()).ok())
                    .unwrap_or_default();
                if crate::session_move::refused(reply.status, &value) {
                    return Err(Refusal::json(reply.status, value));
                }
                let mut refused = if unconfirmed {
                    json!({ "ambiguous": true })
                } else {
                    json!({ "restore": { "text": text } })
                };
                refused["error"] = json!(value["message"].as_str()
                    .unwrap_or("Check the conversation to see whether the draft went before sending again."));
                json!({ "answer": refused })
            }
        }
    } else {
        json!({ "answer": receipt.decision })
    };
    receipt.outcome = Some(answer.clone());
    disk::durable(path, receipt).await.map_err(store_error)?;
    Ok(answer)
}

pub(crate) fn read_draft(call: &RouteCall<()>) -> Result<Draft, Refusal> {
    let bad = || Refusal::new(StatusCode::BAD_REQUEST, DRAFT_FIELDS);
    let draft: Draft = serde_json::from_slice(&call.request.body).map_err(|_| bad())?;
    if draft.id.is_empty()
        || draft.id.len() > 80
        || !draft
            .id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        || call
            .request
            .headers
            .get("idempotency-key")
            .is_some_and(|key| key.to_str().ok() != Some(draft.id.as_str()))
    {
        return Err(bad());
    }
    Ok(draft)
}

pub(crate) fn register(engine: &Engine, list: LocalSessions, routes: &mut Registry) {
    routes.register::<offers::Spec, _, _>(Access::Any, |call| async move {
        sessions::id(&call)?;
        Ok(RouteReply::json::<200>(sessions::typed(
            json!({ "keywords": [], "commands": [] }),
        )?))
    });
    let here = engine.clone();
    routes.register::<decide_route::Spec, _, _>(Access::Any, move |call| {
        let engine = here.clone();
        async move {
            let session = sessions::id(&call)?;
            let given = sessions::body(&call).unwrap_or_default();
            let text = given
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    Refusal::new(StatusCode::BAD_REQUEST, "Send what was typed (text).")
                })?
                .to_owned();
            let id = given
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .or_else(|| {
                    call.request
                        .headers
                        .get("idempotency-key")
                        .and_then(|id| id.to_str().ok())
                        .map(str::trim)
                })
                .filter(|id| !id.is_empty())
                .map(|id| id.chars().take(200).collect());
            let answer = tokio::spawn(async move { typed(&engine, &session, &text, id).await })
                .await
                .map_err(|_| Refusal::new(StatusCode::SERVICE_UNAVAILABLE, UNAVAILABLE))??;
            Ok(RouteReply::json::<200>(sessions::typed(answer)?))
        }
    });
    let here = engine.clone();
    routes.register::<draft::Spec, _, _>(Access::Any, move |call| {
        let engine = here.clone();
        let list = list.clone();
        async move {
            let session = sessions::id(&call)?;
            let draft = read_draft(&call)?;
            let caller = call.request.caller;
            let author = author(&caller)?;
            let answer = tokio::spawn(async move {
                submit(
                    &engine,
                    &list,
                    &session,
                    draft,
                    DraftContext {
                        caller: &caller,
                        author: &author,
                        delivery_mode: None,
                        submission: None,
                        request: DraftRequest::Direct,
                    },
                )
                .await
            })
            .await
            .map_err(|_| Refusal::new(StatusCode::SERVICE_UNAVAILABLE, UNAVAILABLE))??;
            Ok(RouteReply::json::<200>(sessions::typed(answer)?))
        }
    });
}
