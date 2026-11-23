//! A durable claim precedes the prompt. A retry reads the claim, then checks the
//! queue, inbox and transcript before it can offer the original prompt again.

use std::{io, path::PathBuf, sync::atomic::Ordering, time::Duration};

use http::{Method, StatusCode};
use kuvo_protocol::routes::{
    post_kuvo_sessions_id_queue_add as add, post_kuvo_sessions_id_send as send,
};
use kuvo_server::{
    Access, Caller, LocalOrigin, Refusal, Registry, RouteCall, RouteReply, Transport,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    Engine, api, disk,
    lifetime::Lease,
    parts,
    queue::{self, Delivery, Prompt},
    sessions,
};

const BAD_ID: &str = "This message couldn’t be queued. Send it again as a new message.";

pub(crate) fn valid_id(id: &str) -> bool {
    id.strip_prefix("msg_").is_some_and(|suffix| {
        (1..=80).contains(&suffix.len()) && suffix.bytes().all(|ch| ch.is_ascii_alphanumeric())
    })
}

pub(crate) fn new_id(engine: &Engine) -> Result<String, Refusal> {
    let now = disk::now() << 12;
    let tick = engine
        .message_clock
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |earlier| {
            Some(now.max(earlier + 1))
        })
        .expect("the engine's message clock always advances");
    let mut random = [0u8; 14];
    getrandom::fill(&mut random).map_err(|_| {
        Refusal::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "The computer couldn’t make a message ID.",
        )
    })?;
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let random = random
        .into_iter()
        .map(|byte| char::from(ALPHABET[usize::from(byte) % ALPHABET.len()]))
        .collect::<String>();
    Ok(format!(
        "msg_{:012x}{random}",
        now.max(tick + 1) & 0xffff_ffff_ffff
    ))
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Receipt {
    format_version: u32,
    request: Send,
    /// A persisted answer is returned even if the engine has since parked.
    #[serde(skip_serializing_if = "Option::is_none")]
    answer: Option<Answer>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Send {
    prompt: Prompt,
    words: Option<String>,
    from: Option<String>,
    before: Option<String>,
    immediate: bool,
    form: Form,
}

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Form {
    Send,
    Add,
}

#[derive(Clone, Serialize, Deserialize)]
struct Answer {
    value: Value,
    delivered: bool,
}

fn receipt_path(engine: &Engine, session: &str, id: &str) -> PathBuf {
    engine
        .home
        .join("run/prompt-receipts")
        .join(format!("{}.json", disk::key(session, id)))
}

fn disk_failure(error: io::Error) -> Refusal {
    Refusal::new(
        StatusCode::SERVICE_UNAVAILABLE,
        format!("This computer couldn’t save the message receipt: {error}"),
    )
}

fn engine_busy() -> Refusal {
    Refusal::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "The engine isn't answering. Try again in a moment.",
    )
}

async fn read(engine: &Engine, session: &str, id: &str) -> Result<Option<Receipt>, Refusal> {
    let saved: Option<Receipt> = disk::read(&receipt_path(engine, session, id))
        .await
        .map_err(disk_failure)?;
    if saved
        .as_ref()
        .is_some_and(|saved| saved.format_version != 1)
    {
        return Err(Refusal::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Unsupported prompt receipt format.",
        ));
    }
    Ok(saved)
}

pub(crate) async fn was_sent(engine: &Engine, session: &str, id: &str) -> Result<bool, Refusal> {
    Ok(read(engine, session, id)
        .await?
        .and_then(|receipt| receipt.answer)
        .is_some_and(|answer| answer.delivered))
}

pub(crate) async fn forget(engine: &Engine, session: &str, id: &str) -> Result<(), Refusal> {
    match tokio::fs::remove_file(receipt_path(engine, session, id)).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(disk_failure(error)),
    }
}

fn decode(engine: &Engine, call: &RouteCall<()>, form: Form) -> Result<Send, Refusal> {
    let given = sessions::body(call)
        .map_err(|_| Refusal::new(StatusCode::BAD_REQUEST, "Send what was written (words)."))?;
    let id = match given.get("id") {
        Some(Value::String(id)) if valid_id(id) => id.clone(),
        None => new_id(engine)?,
        _ => return Err(Refusal::new(StatusCode::BAD_REQUEST, BAD_ID)),
    };
    let files = given
        .get("files")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let phone = !match &call.request.caller {
        Caller::Paired { pairing, .. } => pairing.desktop,
        Caller::Local(
            Transport::Http {
                origin: LocalOrigin::Direct,
                ..
            }
            | Transport::Relay,
        ) => false,
        Caller::Local(_) => true,
    };
    let legacy_queue = form == Form::Add;
    let composed = parts::compose(&given, !files.is_empty(), phone, legacy_queue)
        .map_err(|message| Refusal::new(StatusCode::BAD_REQUEST, message))?;
    if given
        .get("named")
        .and_then(Value::as_array)
        .is_some_and(|named| !named.is_empty())
        && given
            .get("words")
            .and_then(Value::as_str)
            .is_some_and(|words| words.contains('&'))
    {
        return Err(Refusal::new(
            StatusCode::CONFLICT,
            "An agent you named with & isn't known to this computer yet. Try again in a moment.",
        ));
    }
    let mut metadata = composed
        .parts
        .map(|parts| json!({ "kuvo": { "parts": parts } }));
    if legacy_queue
        && let Some(summary) = given
            .get("metadata")
            .and_then(|meta| meta.pointer("/kuvo/wake/summary"))
            .and_then(Value::as_str)
    {
        let summary: String = summary
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(300)
            .collect();
        if !summary.is_empty() {
            metadata.get_or_insert_with(|| json!({ "kuvo": {} }))["kuvo"]["wake"] =
                json!({ "summary": summary });
        }
    }
    let choice = given.get("delivery").and_then(Value::as_str);
    let delivery = if choice == Some("queue") || (legacy_queue && choice != Some("steer")) {
        Delivery::Queue
    } else {
        Delivery::Steer
    };
    let from = legacy_queue
        .then(|| given.get("from").and_then(Value::as_str))
        .flatten()
        .map(str::trim)
        .filter(|from| !from.is_empty())
        .map(|from| from.chars().take(80).collect());
    let before = legacy_queue
        .then(|| given.get("before").and_then(Value::as_str))
        .flatten()
        .filter(|before| {
            !before.is_empty()
                && before.len() <= 100
                && before
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_')
        })
        .map(str::to_owned);
    Ok(Send {
        form,
        prompt: Prompt {
            id,
            text: composed.text,
            delivery,
            files,
            metadata,
        },
        words: composed.words,
        from,
        before,
        immediate: choice == Some("now") && !legacy_queue,
    })
}

pub(crate) async fn post(
    engine: &Engine,
    lease: &Lease,
    session: &str,
    prompt: &Prompt,
) -> Result<Value, Refusal> {
    let path = format!("{}/prompt", sessions::path(session));
    let response = engine
        .upstream
        .request(&lease.record, Method::POST, &path)
        .timeout(Duration::from_secs(120))
        .json(prompt)
        .send()
        .await
        .map_err(|_| engine_busy())?;
    let status = response.status();
    let bytes = response.bytes().await.map_err(|_| engine_busy())?;
    let value = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
    if status.is_success() {
        Ok(value)
    } else {
        Err(Refusal::json(status, value))
    }
}

async fn deliver(engine: &Engine, session: &str, given: Send) -> Result<Value, Refusal> {
    let _turn = engine.turn(session).await;
    let path = receipt_path(engine, session, &given.prompt.id);
    let saved = read(engine, session, &given.prompt.id).await?;
    if let Some(answer) = saved.as_ref().and_then(|receipt| receipt.answer.as_ref()) {
        return Ok(answer.value.clone());
    }
    let interrupted = saved.is_some();
    let mut receipt = saved.unwrap_or(Receipt {
        format_version: 1,
        request: given,
        answer: None,
    });
    let request = &receipt.request;
    let id = &request.prompt.id;
    let queue = engine.queues.get(session).await.map_err(disk_failure)?;
    let mut admitted = queue.contains(id);
    let mut delivered = false;
    let mut answer = json!({});
    if !admitted {
        let lease = api::start(engine).await?;
        let route = sessions::path(session);
        match engine
            .call::<Value>(&lease, Method::GET, &route, None)
            .await
        {
            Ok(_) => {}
            Err(error) if error.missing() => {
                return Err(Refusal::new(
                    StatusCode::NOT_FOUND,
                    "There's no such conversation here.",
                ));
            }
            Err(_) => return Err(engine_busy()),
        }
        if interrupted {
            delivered = queue::reached(engine, &lease, session, id)
                .await
                .map_err(|_| engine_busy())?;
        } else {
            disk::durable(path.clone(), receipt.clone())
                .await
                .map_err(disk_failure)?;
        }
        if !delivered {
            admitted = engine
                .queues
                .keep(
                    session,
                    &request.prompt,
                    request.words.as_deref(),
                    request.from.clone(),
                    request.before.as_deref(),
                )
                .await
                .map_err(disk_failure)?;
            if !admitted {
                answer = post(engine, &lease, session, &request.prompt).await?;
                delivered = true;
                engine.queues.notify();
                if request.immediate {
                    let _ = queue::advance(engine, &lease, session, id, true).await;
                }
            }
        }
    }

    if request.form == Form::Add {
        answer = engine
            .queues
            .get(session)
            .await
            .map_err(disk_failure)?
            .shown();
        answer["waits"] = json!(admitted);
    } else if admitted {
        answer = json!({ "waits": true });
    }
    receipt.answer = Some(Answer {
        value: answer.clone(),
        delivered,
    });
    disk::durable(path, receipt).await.map_err(disk_failure)?;
    Ok(answer)
}

async fn apply(engine: Engine, call: RouteCall<()>, form: Form) -> Result<Value, Refusal> {
    let session = sessions::id(&call)?;
    let given = decode(&engine, &call, form)?;
    // The handoff owns its claim even after the caller disconnects.
    tokio::spawn(async move { deliver(&engine, &session, given).await })
        .await
        .map_err(|_| engine_busy())?
}

pub(crate) fn register(engine: &Engine, routes: &mut Registry) {
    let local = engine.clone();
    routes.register::<send::Spec, _, _>(Access::Any, move |call| {
        let engine = local.clone();
        async move {
            Ok(RouteReply::json::<200>(
                apply(engine, call, Form::Send).await?,
            ))
        }
    });
    let local = engine.clone();
    routes.register::<add::Spec, _, _>(Access::Any, move |call| {
        let engine = local.clone();
        async move {
            Ok(RouteReply::json::<200>(sessions::typed(
                apply(engine, call, Form::Add).await?,
            )?))
        }
    });
}
