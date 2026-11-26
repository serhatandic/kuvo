//! Validate consent at the last boundary before forwarding it to OpenCode.

use std::time::Duration;

use http::{HeaderMap, Method, StatusCode};
use kuvo_server::{Caller, DispatchRequest, Refusal};
use percent_encoding::percent_decode_str;
use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::{Engine, disk, lifetime::Lease};

pub(crate) fn current(caller: &Caller, cancel: &CancellationToken) -> Result<(), Refusal> {
    if cancel.is_cancelled() {
        return Err(Refusal::new(
            StatusCode::NOT_FOUND,
            "It isn’t waiting any more.",
        ));
    }
    if caller.pairing().is_some_and(|pair| !pair.is_current()) {
        return Err(Refusal::new(
            StatusCode::FORBIDDEN,
            "This device no longer has access to act here.",
        ));
    }
    Ok(())
}

pub(crate) fn live(
    engine: &Engine,
    caller: &Caller,
    cancel: &CancellationToken,
    sessions: [&str; 2],
) -> Result<(), Refusal> {
    current(caller, cancel)?;
    if let Some(authority) = &engine.ownership {
        for session in sessions
            .into_iter()
            .filter(|session| crate::sessions::valid_id(session))
        {
            let scope = authority.scope(session, None)?;
            if scope.held || scope.owner.is_some_and(|owner| !owner.local) {
                return Err(Refusal::new(
                    StatusCode::CONFLICT,
                    "Open the current owner’s live conversation before acting.",
                ));
            }
        }
    }
    Ok(())
}

pub(crate) async fn raw(
    engine: &Engine,
    lease: &Lease,
    request: &DispatchRequest,
    cancel: &CancellationToken,
) -> Result<(), Refusal> {
    let route = request.path.split('?').next().unwrap_or_default();
    let mut parts = route.split('/');
    if !matches!(request.method, Method::POST | Method::DELETE)
        || parts.next() != Some("api")
        || parts.next() != Some("session")
    {
        return Ok(());
    }
    let (Some(session), Some(kind), Some(id)) = (parts.next(), parts.next(), parts.next()) else {
        return Ok(());
    };
    if !matches!(kind, "permission" | "form")
        || !matches!((parts.next(), parts.next()), (Some("reply") | None, None))
    {
        return Ok(());
    }
    let session = percent_decode_str(session)
        .decode_utf8()
        .map_err(|_| Refusal::new(StatusCode::NOT_FOUND, "It isn’t waiting any more."))?;
    live(engine, &request.caller, cancel, [&session, ""])?;
    if request.method == Method::POST
        && kind == "permission"
        && let Ok(body) = serde_json::from_slice::<Value>(&request.body)
        && let Some(decision) = body["decision"].as_str()
        && let Ok(id) = percent_decode_str(id).decode_utf8()
    {
        grant(engine, lease, &session, &id, decision, &request.headers).await?;
    }
    live(engine, &request.caller, cancel, [&session, ""])
}

#[derive(Deserialize)]
struct Rule {
    action: String,
    resource: String,
    effect: String,
}

pub(crate) async fn grant(
    engine: &Engine,
    lease: &Lease,
    session: &str,
    ask: &str,
    decision: &str,
    headers: &HeaderMap,
) -> Result<(), Refusal> {
    if !matches!(decision, "once" | "always") {
        return Ok(());
    }
    let route = format!("{}/permission?live=1", crate::sessions::path(session));
    let pending = engine
        .upstream
        .get_data::<Vec<Value>>(&lease.record, &route, headers, Duration::from_secs(5))
        .await
        .map_err(|_| {
            Refusal::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "This computer couldn’t verify the live permission.",
            )
        })?;
    allowed(engine, &pending, ask, decision).await
}

/// Whether this computer's saved permissions still allow `decision` on `ask`, one of the
/// requests waiting now (`pending`, as the live permission list has them).
pub(crate) async fn allowed(
    engine: &Engine,
    pending: &[Value],
    ask: &str,
    decision: &str,
) -> Result<(), Refusal> {
    if !matches!(decision, "once" | "always") {
        return Ok(());
    }
    let request = pending
        .iter()
        .find(|row| row["id"] == ask)
        .ok_or_else(|| Refusal::new(StatusCode::NOT_FOUND, "It isn’t waiting any more."))?;
    let relative = if engine.is_mac_box().await {
        "config/opencode/opencode.json"
    } else {
        "box/opencode.json"
    };
    let saved = disk::read::<Value>(&engine.home.join(relative))
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    let Ok(rules) = serde_json::from_value::<Vec<Rule>>(saved["permissions"].clone()) else {
        return Ok(());
    };
    if rules.iter().any(|rule| rule.effect.is_empty()) {
        return Ok(());
    }
    let action = request["action"].as_str().unwrap_or_default();
    let edits = ["edit", "write", "patch", "apply_patch", "multiedit"];
    let shell = matches!(action, "bash" | "shell");
    let family = if shell {
        &["shell", "bash"][..]
    } else if edits.contains(&action) {
        &edits
    } else {
        std::slice::from_ref(&action)
    };
    let mut resources = request["resources"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    if shell {
        resources.extend(resources.clone().into_iter().flat_map(shell_parts));
    }
    if resources.is_empty() {
        resources.push("*");
    }
    let mut overlaps = false;
    for rule in rules.iter().filter(|rule| {
        rule.effect != "allow"
            && rule.effect != "ask"
            && family.iter().any(|action| matches(&rule.action, action))
    }) {
        if resources
            .iter()
            .any(|resource| matches(&rule.resource, resource))
        {
            return Err(Refusal::new(
                StatusCode::FORBIDDEN,
                "This computer’s permissions no longer allow that action.",
            ));
        }
        overlaps = true;
    }
    if decision == "always" && overlaps {
        return Err(Refusal::new(
            StatusCode::FORBIDDEN,
            "An Always answer could override a local restriction for this tool. Use Allow Once for this request.",
        ));
    }
    Ok(())
}

fn matches(pattern: &str, value: &str) -> bool {
    if pattern == "*" || pattern == value || pattern.strip_suffix(" *") == Some(value) {
        return true;
    }
    // JS's matcher consumes UTF-16 units. Backtrack only to the last star.
    let pattern = pattern.encode_utf16().collect::<Vec<_>>();
    let value = value.encode_utf16().collect::<Vec<_>>();
    let (mut at, mut matched, mut star) = (0, 0, None);
    while matched < value.len() {
        match pattern.get(at).copied() {
            Some(42) => {
                star = Some((at, matched));
                at += 1;
            }
            Some(ch) if ch == 63 || ch == value[matched] => {
                at += 1;
                matched += 1;
            }
            _ => {
                let Some((wild, end)) = star.as_mut() else {
                    return false;
                };
                *end += 1;
                at = *wild + 1;
                matched = *end;
            }
        }
    }
    pattern[at..].iter().all(|ch| *ch == 42)
}

fn shell_parts(line: &str) -> Vec<&str> {
    let mut chars = line.char_indices().peekable();
    let mut quote = None;
    let mut start = 0;
    let mut word = false;
    let mut parts = Vec::new();
    while let Some((at, ch)) = chars.next() {
        match (quote, ch) {
            (Some(kind), end) if kind == end => {
                quote = None;
            }
            (Some('\''), _) => {}
            (Some('"'), '$' | '`') => return vec![line],
            (_, '\\') if chars.peek().is_some() => {
                let (_, escaped) = chars.next().expect("peeked character");
                word |= escaped != '\n';
            }
            (Some(_), _) => {}
            (None, '\'' | '"') => {
                quote = Some(ch);
                word = true;
            }
            (None, '$' | '`' | '(' | ')' | '{' | '}' | '<' | '>') => return vec![line],
            (None, '#' | '~') if !word => {
                let next = chars.peek().map(|(_, ch)| *ch);
                if ch == '#' || !matches!(next, None | Some('/' | ' ' | '\t' | '\n')) {
                    return vec![line];
                }
                word = true;
            }
            (None, '\n' | ';' | '|' | '&') => {
                let next = chars.peek().map(|(_, ch)| *ch);
                if (ch == '&' && next != Some('&')) || (ch == '|' && next == Some('&')) {
                    return vec![line];
                }
                let part = line[start..at].trim();
                if !part.is_empty() {
                    parts.push(part);
                }
                start = at + 1;
                if matches!(ch, '|' | '&') && next == Some(ch) {
                    chars.next();
                    start += 1;
                }
                word = false;
            }
            (None, ch) => word = !ch.is_whitespace(),
        }
    }
    if quote.is_some() {
        return vec![line];
    }
    let part = line[start..].trim();
    if !part.is_empty() {
        parts.push(part);
    }
    parts
}
