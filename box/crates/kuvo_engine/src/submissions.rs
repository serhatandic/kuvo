//! Receipt lookup and admission for a draft pinned to its original owner.

use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::{StreamExt, stream};
use http::StatusCode;
use http_body_util::BodyExt;
use kuvo_protocol::routes::{
    get_kuvo_sessions_id_submissions as history,
    get_kuvo_sessions_id_submissions_submission as detail, post_kuvo_sessions_id_submit as submit,
};
use kuvo_server::{Access, Caller, Refusal, Registry, RouteCall, RouteReply};
use kuvo_wire::keys::{decode_exact, encode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    ComputerKey, Engine, LocalSessions, OwnedReply, OwnedRequest, OwnerAction, OwnerScope,
    SessionOwnership, Submitter, disk, sessions,
    typed::{self, Draft, DraftContext},
};

const FEATURE: &str = "typed.submission-recovery";
pub(crate) const PAGE: usize = 40;

#[derive(Clone)]
struct Handle {
    text: String,
    setup: String,
    origin: ComputerKey,
    message: String,
}

fn token(value: &str, size: std::ops::RangeInclusive<usize>) -> bool {
    size.contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

impl Handle {
    fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split('.');
        let setup = parts.next()?;
        let origin = parts.next()?;
        let message = parts.next()?;
        if parts.next().is_some()
            || !token(setup.strip_prefix("set_")?, 22..=22)
            || !token(origin, 43..=43)
            || !token(message, 1..=80)
        {
            return None;
        }
        Some(Self {
            text: text.into(),
            setup: setup.into(),
            origin: decode_exact(origin).filter(|key| encode(key) == origin)?,
            message: message.into(),
        })
    }

    fn key(&self, session: &str) -> String {
        fingerprint(json!(["draft-submission", self.setup, session, self.text]))
    }
}

pub(crate) fn fingerprint(mut value: Value) -> String {
    value.sort_all_objects();
    URL_SAFE_NO_PAD.encode(Sha256::digest(value.to_string()))
}

#[derive(Clone, Deserialize, Serialize)]
struct Claim {
    #[serde(rename = "formatVersion", default)]
    version: u32,
    digest: String,
    submission: Address,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    answer: Option<StoredAnswer>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    at: Option<u64>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Address {
    setup: String,
    session: String,
    id: String,
    origin: String,
    submitted_through: String,
    submitter: Submitter,
    #[serde(skip_serializing_if = "Option::is_none")]
    draft: Option<Value>,
}

#[derive(Clone, Deserialize, Serialize)]
struct StoredAnswer {
    status: u16,
    value: Value,
    #[serde(
        default,
        rename = "notAdmitted",
        skip_serializing_if = "std::ops::Not::not"
    )]
    not_admitted: bool,
}

impl Claim {
    fn belongs(&self, session: &str, handle: &Handle, viewer: &Submitter, through: &str) -> bool {
        self.version <= 1
            && self.submission.session == session
            && self.submission.setup == handle.setup
            && self.submission.id == handle.text
            && self.submission.origin == encode(&handle.origin)
            && self.submission.submitter.author_matches(viewer)
            && (*viewer != Submitter::Computer || self.submission.submitted_through == through)
    }

    fn report(&self, full: bool) -> Value {
        let address = &self.submission;
        let mut result = json!({
            "submission": address.id, "originalOwner": address.origin,
            "digest": self.digest, "submitter": address.submitter, "evidence": "owner",
        });
        let status = self.answer.as_ref().map_or("unknown", |answer| {
            let decision = &answer.value["answer"];
            if answer.value["ambiguous"] == true
                || decision["ambiguous"] == true
                || !decision["error"].is_null() && decision["restore"].is_null()
                || answer.status >= 500 && !answer.not_admitted && answer.value["restore"].is_null()
            {
                "unknown"
            } else if answer.not_admitted || answer.status >= 400 || !decision["restore"].is_null()
            {
                "notAdmitted"
            } else {
                "committed"
            }
        });
        result["status"] = json!(status);
        match (status, &self.answer) {
            ("committed", Some(answer)) => {
                result["status"] = json!("committed");
                result["acceptedAt"] = json!(self.at.unwrap_or_default());
                result["effect"] = if full {
                    answer.value.clone()
                } else {
                    let actions = answer.value["answer"]
                        .as_object()
                        .into_iter()
                        .flat_map(|answer| answer.keys())
                        .collect::<Vec<_>>();
                    let mut effect = json!({"actions": actions});
                    if let Some(message) = answer.value["message"].as_str() {
                        effect["message"] = json!(message);
                    }
                    effect
                };
            }
            (status, answer) => {
                if let Some(answer) = answer.as_ref().filter(|_| status == "notAdmitted") {
                    result["decision"] = answer.value.clone();
                    if answer.not_admitted {
                        result["retryable"] = json!(true);
                    }
                }
                if let Some(draft) = &address.draft {
                    result["draftAvailable"] = json!(true);
                    if full {
                        result["draft"] = draft.clone();
                    }
                }
            }
        }
        result
    }
}

fn directory(engine: &Engine) -> PathBuf {
    engine.home.join("run/owned-data/submissions")
}

pub(crate) async fn guard_legacy_draft(
    engine: &Engine,
    session: &str,
    id: &str,
    submission: Option<&str>,
) -> Result<(), Refusal> {
    let Some(authority) = &engine.ownership else {
        return Ok(());
    };
    let setup = authority.scope(session, None)?.setup.unwrap_or_default();
    let legacy = fingerprint(json!(["typed-draft", setup, session, id]));
    let path = engine
        .home
        .join("run/owned-data/mutations")
        .join(format!("{legacy}.json"));
    let existing = disk::read::<Value>(&path).await.map_err(store)?;
    if let Some(submission) = submission {
        let handle = Handle::parse(submission).ok_or_else(bad)?;
        let digest = format!("scoped:{}", handle.key(session));
        if handle.setup != setup || handle.message != id {
            return Err(wrong_scope());
        }
        match existing {
            Some(ref known) if known["digest"] == digest => {}
            Some(_) => return Err(uncertain()),
            None => disk::durable(
                path,
                json!({"digest":digest,
                    "draftReplayFence":{"setup":setup,"session":session,"id":id}
                }),
            )
            .await
            .map_err(store)?,
        }
    } else if existing.is_some() {
        return Err(uncertain());
    }
    Ok(())
}

fn uncertain() -> Refusal {
    Refusal::json(
        StatusCode::CONFLICT,
        json!({"ambiguous":true,
        "message":"The Box could not confirm the draft's outcome. Check the conversation before sending again."}),
    )
}

fn wrong_scope() -> Refusal {
    Refusal::json(
        StatusCode::CONFLICT,
        json!({
            "ambiguous":true,"message":"Look up that submission on its original computer."
        }),
    )
}

fn bad() -> Refusal {
    Refusal::new(
        StatusCode::BAD_REQUEST,
        "Use the original owner and stable submission ID.",
    )
}

fn store(_: impl std::fmt::Display) -> Refusal {
    Refusal::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "This computer couldn’t save the message receipt. Try again in a moment.",
    )
}

fn changed() -> Refusal {
    Refusal::new(
        StatusCode::CONFLICT,
        "The conversation's owner changed. Check the conversation before sending again.",
    )
}

fn draft_handle(body: &Value) -> Result<Handle, Refusal> {
    let handle = body["submission"]
        .as_str()
        .and_then(Handle::parse)
        .ok_or_else(bad)?;
    if body["id"] != handle.message {
        return Err(bad());
    }
    Ok(handle)
}

pub(crate) fn key(session: &str, setup: &str, owner: &str, body: &Value) -> Option<String> {
    let handle = draft_handle(body).ok()?;
    (handle.setup == setup && encode(&handle.origin) == owner).then(|| handle.key(session))
}

pub(crate) fn listing_id(submission: &str, setup: &str) -> Option<(String, String)> {
    let handle = Handle::parse(submission)?;
    (handle.setup == setup).then(|| (encode(&handle.origin), handle.message))
}

pub(crate) fn intent(body: &Value, viewer: &Submitter, through: &str) -> String {
    let mut value = json!({"given": body, "submitter": viewer});
    if *viewer == Submitter::Computer {
        value["through"] = json!(through);
    }
    fingerprint(value)
}

struct IncomingDraft {
    session: String,
    draft: Draft,
    body: Value,
    viewer: Submitter,
    through: ComputerKey,
    caller: Caller,
    handle: Handle,
}

async fn claim(
    engine: &Engine,
    list: &LocalSessions,
    authority: &dyn SessionOwnership,
    input: IncomingDraft,
) -> Result<Value, Refusal> {
    let IncomingDraft {
        session,
        draft,
        body,
        viewer,
        through,
        caller,
        handle,
    } = input;
    let through = encode(&through);
    let digest = intent(&body, &viewer, &through);
    let key = handle.key(&session);
    let _turn = engine.submission_turns.acquire(&key).await;
    let path = directory(engine).join(format!("{key}.json"));
    let previous = disk::read::<Claim>(&path).await.map_err(store)?;
    let mut record = match previous {
        Some(prior) => {
            if prior.digest != digest || !prior.belongs(&session, &handle, &viewer, &through) {
                return Err(Refusal::new(
                    StatusCode::CONFLICT,
                    "That operation id is already in use.",
                ));
            }
            if let Some(answer) = prior.answer {
                return reply_of(answer, prior.at);
            }
            prior
        }
        None => Claim {
            version: 1,
            digest,
            submission: Address {
                setup: handle.setup.clone(),
                session: session.clone(),
                id: handle.text.clone(),
                origin: encode(&handle.origin),
                submitted_through: through.clone(),
                submitter: viewer.clone(),
                draft: Some(body),
            },
            answer: None,
            at: None,
        },
    };
    let scope = authority.scope(&session, Some(handle.origin))?;
    if scope.held
        || scope.setup.as_deref() != Some(handle.setup.as_str())
        || scope.here != Some(handle.origin)
        || scope.owner_key() != scope.here
    {
        return Err(changed());
    }
    viewer.authorize(authority)?;
    disk::durable(path.clone(), record.clone())
        .await
        .map_err(store)?;
    let author = typed::author(&caller)?;
    let result = typed::submit(
        engine,
        list,
        &session,
        draft,
        DraftContext {
            caller: &caller,
            author: &author,
            delivery_mode: None,
            submission: Some(&handle.text),
            request: typed::DraftRequest::Durable,
        },
    )
    .await;
    let answer = match result {
        Ok(value) => {
            if value["answer"]["error"].is_null() {
                record.submission.draft = None;
            }
            StoredAnswer {
                status: 200,
                value,
                not_admitted: false,
            }
        }
        Err(error) => {
            let response = error.reply();
            if response.status.is_server_error() {
                return Err(Refusal::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "The engine isn't answering. Try again in a moment.",
                ));
            }
            let status = response.status.as_u16();
            let bytes = response.body.collect().await.map_err(store)?.to_bytes();
            StoredAnswer {
                status,
                value: serde_json::from_slice(&bytes).map_err(store)?,
                not_admitted: false,
            }
        }
    };
    let at = answer.value["acceptedAt"]
        .as_u64()
        .unwrap_or_else(disk::now);
    record.at = Some(at);
    record.answer = Some(answer.clone());
    disk::durable(path, record).await.map_err(store)?;
    reply_of(answer, Some(at))
}

fn reply_of(mut answer: StoredAnswer, at: Option<u64>) -> Result<Value, Refusal> {
    let status = StatusCode::from_u16(answer.status).map_err(store)?;
    if status.is_success() {
        if answer.value["message"].is_string()
            && let Some(at) = at
        {
            answer.value["acceptedAt"] = json!(at);
        }
        Ok(answer.value)
    } else {
        Err(Refusal::json(status, answer.value))
    }
}

struct ReceiptViewer<'a> {
    author: &'a Submitter,
    through: ComputerKey,
    include_listing: bool,
}

async fn local_reports(
    engine: &Engine,
    authority: &dyn SessionOwnership,
    session: &str,
    scope: &OwnerScope,
    query: &Value,
    reader: ReceiptViewer<'_>,
) -> Result<Value, Refusal> {
    let requested = query["submission"].as_str().and_then(Handle::parse);
    let after = query["after"].as_str().unwrap_or_default();
    let want_digest = query["digest"].as_str();
    let through = encode(&reader.through);
    let folder = directory(engine);
    let keys = if let Some(one) = &requested {
        let key = one.key(session);
        crate::receipt_index::Keys {
            owner: vec![key.clone()],
            listing: vec![key],
            complete: true,
        }
    } else {
        let mut issuers = authority
            .receipt_issuers()
            .into_iter()
            .filter_map(|(origin, issuer)| (Some(issuer) == scope.here).then_some(origin))
            .collect::<std::collections::BTreeSet<_>>();
        issuers.extend(scope.here);
        match engine
            .receipt_index
            .keys(crate::receipt_index::PageScope {
                setup: scope.setup.clone().unwrap_or_default(),
                session: session.into(),
                after: after.into(),
                viewer: reader.author.clone(),
                through: reader.through,
                issuers,
                include_listing: reader.include_listing,
            })
            .await
        {
            Ok(keys) => keys,
            Err(_) => crate::receipt_index::Keys {
                owner: Vec::new(),
                listing: Vec::new(),
                complete: false,
            },
        }
    };
    let mut complete = keys.complete;
    let mut outcomes = Vec::new();
    for key in keys.owner {
        let claim = match disk::read::<Claim>(&folder.join(format!("{key}.json"))).await {
            Ok(Some(claim)) if claim.version <= 1 => claim,
            Ok(None) if requested.is_some() => continue,
            _ => {
                complete = false;
                continue;
            }
        };
        let Some(handle) = Handle::parse(&claim.submission.id) else {
            complete = false;
            continue;
        };
        if handle.key(&claim.submission.session) != key {
            complete = false;
            continue;
        }
        if scope.setup.as_deref() != Some(handle.setup.as_str())
            || handle.text.as_str() <= after
            || want_digest.is_some_and(|digest| claim.digest != digest)
            || !claim.belongs(session, &handle, reader.author, &through)
        {
            complete &= requested.is_some();
            continue;
        }
        let current = authority.scope(session, Some(handle.origin))?;
        if current.here.is_some() && current.here == current.receipt_issuer {
            outcomes.push(claim.report(requested.is_some()));
        } else {
            complete &= requested.is_some();
        }
    }
    if reader.include_listing
        && let Some(setup) = &scope.setup
    {
        let expected = keys.listing.len();
        match crate::delivery::listing(engine, session, setup, reader.author, query, keys.listing)
            .await
        {
            Ok(listing) => {
                complete &= requested.is_some() || listing.len() == expected;
                outcomes.extend(listing);
            }
            Err(_) => complete = false,
        }
    }
    Ok(page(scope, outcomes, complete, false, requested.is_some()))
}

fn priority(row: &Value) -> u8 {
    if row["status"] == "committed" {
        4
    } else if row["evidence"] == "owner" && row["retryable"] != true {
        3
    } else if row["status"] == "pending" {
        2
    } else if row["status"] == "unknown" {
        1
    } else {
        0
    }
}

fn page(
    scope: &OwnerScope,
    outcomes: Vec<Value>,
    complete: bool,
    more: bool,
    detail: bool,
) -> Value {
    let mut unique = BTreeMap::<String, Value>::new();
    for mut report in outcomes {
        let Some(id) = report["submission"].as_str().map(str::to_owned) else {
            continue;
        };
        if let Some(prior) = unique.get_mut(&id) {
            let conflict = prior["digest"] != report["digest"]
                || prior["conflict"] == true
                || report["conflict"] == true;
            if priority(&report) < priority(prior)
                || priority(&report) == priority(prior) && report["evidence"] != "owner"
            {
                std::mem::swap(prior, &mut report);
            }
            if conflict {
                report["conflict"] = json!(true);
            }
        }
        unique.insert(id, report);
    }
    let can_page = complete && (more || unique.len() > PAGE);
    let shown = unique.into_values().take(PAGE).collect::<Vec<_>>();
    let next = if can_page && shown.len() == PAGE {
        shown.last().and_then(|last| last["submission"].as_str())
    } else {
        None
    };
    let mut result = json!({"outcomes": shown, "complete": complete,
        "currentOwner": scope.owner_key().map(|key| encode(&key))});
    if detail && shown.is_empty() {
        result["unknown"] = json!(true);
    }
    if let Some(next) = next {
        result["next"] = json!(next);
    }
    result
}

fn valid_report(
    issuers: &BTreeMap<ComputerKey, ComputerKey>,
    setup: &str,
    peer: ComputerKey,
    viewer: &Submitter,
    query: &Value,
    report: &Value,
) -> bool {
    let Some(id) = report["submission"].as_str().and_then(Handle::parse) else {
        return false;
    };
    id.setup == setup
        && report["originalOwner"] == encode(&id.origin)
        && query["submission"]
            .as_str()
            .is_none_or(|wanted| wanted == id.text)
        && query["digest"]
            .as_str()
            .is_none_or(|digest| report["digest"] == digest)
        && id.text.as_str() > query["after"].as_str().unwrap_or_default()
        && serde_json::from_value::<Submitter>(report["submitter"].clone())
            .is_ok_and(|author| author.author_matches(viewer))
        && (report["evidence"] == "listing" && *viewer != Submitter::Computer
            || report["evidence"] == "owner"
                && *issuers.get(&id.origin).unwrap_or(&id.origin) == peer)
}

async fn query_report(
    engine: &Engine,
    call: &RouteCall<()>,
    handle: Option<Handle>,
) -> Result<history::Status200, Refusal> {
    let viewer = Submitter::from_request(&call.request.caller)?;
    let session = sessions::id(call)?;
    let scope_query = crate::query(&call.request.path, "scope").is_some();
    let Some(authority) = &engine.ownership else {
        return sessions::typed(
            json!({"currentOwner":null,"outcomes":[],"complete":scope_query,"canSubmit":false}),
        );
    };
    viewer.authorize(authority.as_ref())?;
    let scope = authority.scope(&session, handle.as_ref().map(|handle| handle.origin))?;
    let here = scope.here;
    if scope_query || scope.setup.is_none() || here.is_none() {
        let can = scope.setup.is_some()
            && !scope.held
            && scope.owner.as_ref().is_none_or(|owner| {
                owner.local || owner.features.iter().any(|feature| feature == FEATURE)
            });
        let mut result = json!({"currentOwner":scope.owner_key().map(|key| encode(&key)),
            "outcomes":[],"complete":scope_query,"canSubmit":can && here.is_some()});
        if let Some(setup) = scope.setup {
            result["setup"] = json!(setup);
        }
        return sessions::typed(result);
    }
    let through = here.expect("checked local key");
    let mut query = json!({"after":crate::query(&call.request.path, "after")});
    if let Some(handle) = &handle {
        if scope.setup.as_deref() != Some(handle.setup.as_str()) {
            return sessions::typed(
                json!({"currentOwner":scope.owner_key().map(|key| encode(&key)),
                "outcomes":[],"complete":false,"unknown":true}),
            );
        }
        query =
            json!({"submission":handle.text,"digest":crate::query(&call.request.path, "digest")});
    }
    let issuer_view = authority.receipt_issuers();
    let mut result = local_reports(
        engine,
        authority.as_ref(),
        &session,
        &scope,
        &query,
        ReceiptViewer {
            author: &viewer,
            through,
            include_listing: true,
        },
    )
    .await?;
    if query["digest"].is_string()
        && result["outcomes"][0]["evidence"] == "owner"
        && result["outcomes"][0]["status"] == "committed"
        && authority.receipt_issuers() == issuer_view
        && authority
            .scope(&session, handle.as_ref().map(|handle| handle.origin))?
            .setup
            == scope.setup
    {
        viewer.authorize(authority.as_ref())?;
        return sessions::typed(result);
    }
    let setup = scope.setup.as_deref().expect("checked setup");
    let roster = authority.members();
    let before = roster
        .iter()
        .filter_map(|member| member.key)
        .collect::<std::collections::BTreeSet<_>>();
    let replies = stream::iter(
        roster
            .into_iter()
            .filter_map(|member| member.key.map(|key| (key, member.features))),
    )
    .map(|(peer, features)| {
        let request = OwnedRequest {
            setup: setup.to_owned(),
            session: session.clone(),
            operation: fingerprint(query.clone()),
            submitter: viewer.clone(),
            action: OwnerAction::Outcome {
                issuer: peer,
                query: query.clone(),
            },
        };
        async move {
            let reply = if features.iter().any(|feature| feature == FEATURE) {
                authority
                    .request(peer, request, Duration::from_secs(3))
                    .await
                    .ok()
            } else {
                None
            };
            (peer, reply)
        }
    })
    .buffer_unordered(16)
    .collect::<Vec<_>>()
    .await;
    let current = authority.scope(&session, handle.as_ref().map(|handle| handle.origin))?;
    if current.setup.as_deref() != Some(setup) {
        return Err(Refusal::new(
            StatusCode::FORBIDDEN,
            "The submitting device no longer has access.",
        ));
    }
    viewer.authorize(authority.as_ref())?;
    let issuers = authority.receipt_issuers();
    let after = authority
        .members()
        .into_iter()
        .filter_map(|member| member.key)
        .collect::<std::collections::BTreeSet<_>>();
    let mut complete = result["complete"] == true
        && before == after
        && issuer_view == issuers
        && current.receipt_issuer == scope.receipt_issuer
        && current
            .receipt_issuer
            .is_none_or(|issuer| issuer == through || after.contains(&issuer));
    let mut more = result["next"].is_string();
    let mut outcomes = result["outcomes"]
        .as_array_mut()
        .map(std::mem::take)
        .unwrap_or_default();
    outcomes.retain(|row| {
        let still_issued_here = row["evidence"] == "listing"
            || valid_report(&issuers, setup, through, &viewer, &query, row);
        complete &= still_issued_here;
        still_issued_here
    });
    for (peer, reply) in replies {
        let Some(reply) = reply.filter(|reply| reply.status == 200 && after.contains(&peer)) else {
            complete = false;
            continue;
        };
        let Some(rows) = reply.body["outcomes"]
            .as_array()
            .filter(|rows| rows.len() <= PAGE + 1)
        else {
            complete = false;
            continue;
        };
        let legacy = reply.body.get("complete").is_none();
        complete &= legacy || reply.body["complete"] == true;
        more |= reply.body["next"].is_string() || legacy && rows.len() == PAGE;
        for row in rows {
            if valid_report(&issuers, setup, peer, &viewer, &query, row) {
                outcomes.push(row.clone());
            } else {
                complete = false;
            }
        }
    }
    let result = page(&current, outcomes, complete, more, handle.is_some());
    if handle.is_some() {
        crate::delivery::settle_listing(engine, &session, setup, &result["outcomes"][0])
            .await
            .map_err(store)?;
    }
    sessions::typed(result)
}

pub(crate) fn register(
    engine: &Engine,
    list: LocalSessions,
    delivery: crate::delivery::Delivery,
    routes: &mut Registry,
) {
    let here = engine.clone();
    routes.register::<submit::Spec, _, _>(Access::Any, move |call| {
        let engine = here.clone();
        let list = list.clone();
        let delivery = delivery.clone();
        async move {
            let session = sessions::id(&call)?;
            let draft = typed::read_draft(&call)?;
            let body: Value = serde_json::from_slice(&call.request.body).map_err(|_| bad())?;
            let handle = draft_handle(&body)?;
            let authority = engine.ownership.clone().ok_or_else(bad)?;
            let scope = authority.scope(&session, Some(handle.origin))?;
            let through = scope.here.ok_or_else(bad)?;
            if scope.setup.is_none() {
                return Err(bad());
            }
            if scope.setup.as_deref() != Some(handle.setup.as_str()) {
                return Err(wrong_scope());
            }
            let caller = call.request.caller;
            let viewer = Submitter::from_request(&caller)?;
            viewer.authorize(authority.as_ref())?;
            if call.cancel.is_cancelled() {
                return Err(Refusal::new(
                    StatusCode::CONFLICT,
                    "The live draft request closed before admission.",
                ));
            }
            if through != handle.origin {
                return delivery
                    .scoped(&engine, &session, body, &scope, viewer)
                    .await;
            }
            let input = IncomingDraft {
                session,
                draft,
                body,
                handle,
                caller,
                viewer,
                through,
            };
            let answer =
                tokio::spawn(async move { claim(&engine, &list, authority.as_ref(), input).await })
                    .await
                    .map_err(store)??;
            Ok(RouteReply::json::<200>(sessions::typed(answer)?))
        }
    });
    let here = engine.clone();
    routes.register::<history::Spec, _, _>(Access::Any, move |call| {
        let engine = here.clone();
        async move {
            Ok(RouteReply::json::<200>(
                query_report(&engine, &call, None).await?,
            ))
        }
    });
    let here = engine.clone();
    routes.register::<detail::Spec, _, _>(Access::Any, move |call| {
        let engine = here.clone();
        async move {
            let handle = call
                .param("submission")
                .and_then(Handle::parse)
                .ok_or_else(|| {
                    Refusal::new(StatusCode::BAD_REQUEST, "Use the original submission id.")
                })?;
            Ok(RouteReply::json::<200>(
                query_report(&engine, &call, Some(handle)).await?,
            ))
        }
    });
}

impl Engine {
    pub async fn answer_owner_request(
        &self,
        list: &LocalSessions,
        peer: ComputerKey,
        request: OwnedRequest,
    ) -> OwnedReply {
        match self.handle_owner_request(list, peer, request).await {
            Ok(body) => OwnedReply { status: 200, body },
            Err(error) => {
                let reply = error.reply();
                let status = reply.status.as_u16();
                let body = reply
                    .body
                    .collect()
                    .await
                    .ok()
                    .and_then(|bytes| serde_json::from_slice(&bytes.to_bytes()).ok())
                    .unwrap_or_default();
                OwnedReply { status, body }
            }
        }
    }

    async fn handle_owner_request(
        &self,
        list: &LocalSessions,
        peer: ComputerKey,
        request: OwnedRequest,
    ) -> Result<Value, Refusal> {
        let authority: &Arc<dyn SessionOwnership> = self.ownership.as_ref().ok_or_else(bad)?;
        request.submitter.authorize(authority.as_ref())?;
        match request.action.clone() {
            OwnerAction::Submit { draft: body } => {
                let handle = draft_handle(&body)?;
                if request.setup != handle.setup
                    || request.operation != handle.key(&request.session)
                {
                    return Err(bad());
                }
                let draft = serde_json::from_value::<Draft>(body.clone()).map_err(|_| bad())?;
                let caller = request.submitter.caller(authority.clone(), true);
                claim(
                    self,
                    list,
                    authority.as_ref(),
                    IncomingDraft {
                        session: request.session,
                        draft,
                        body,
                        handle,
                        caller,
                        viewer: request.submitter,
                        through: peer,
                    },
                )
                .await
            }
            OwnerAction::Outcome { issuer, query } => {
                let scope = authority.scope(&request.session, Some(issuer))?;
                if scope.setup.as_deref() != Some(request.setup.as_str())
                    || scope.here.is_none()
                    || scope.here != scope.receipt_issuer
                {
                    return Err(wrong_scope());
                }
                local_reports(
                    self,
                    authority.as_ref(),
                    &request.session,
                    &scope,
                    &query,
                    ReceiptViewer {
                        author: &request.submitter,
                        through: peer,
                        include_listing: request.submitter != Submitter::Computer,
                    },
                )
                .await
            }
            OwnerAction::Deliver { message } => {
                crate::delivery::receive(self, list, request, message).await
            }
        }
    }
}
