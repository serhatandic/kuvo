use std::{io, path::Path, time::Duration};

use futures_util::StreamExt;
use http::Method;
use jiff::{Timestamp, tz::TimeZone};
use regex::Regex;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::{LimitStop, drain, inbox};
use crate::{Engine, disk, lifetime::Demand, sessions};

#[derive(Clone)]
pub(super) struct Classifier {
    usage: Regex,
    reset: Regex,
    duration: Regex,
}

impl Classifier {
    pub(super) fn new() -> Self {
        Self {
            usage: Regex::new(r"(?i)\b429\b|rate.?limit|usage limit|limit (?:was |has been )?reached|reached (?:its|your|the) (?:usage )?limit|quota|too many requests|resets? in|try again in").unwrap(),
            reset: Regex::new(r"(?i)(?:resets?|try again|available again)\s+in\s+((?:\d+(?:\.\d+)?\s*(?:days?|d|hours?|hrs?|h|minutes?|mins?|m|seconds?|secs?|s)(?:[\s,]*(?:and\s+)?|$))+)(?:\b|$)").unwrap(),
            duration: Regex::new(r"(?i)(\d+(?:\.\d+)?)\s*([dhms])").unwrap(),
        }
    }

    pub(super) fn classify(&self, value: &Value, at: u64) -> Option<LimitStop> {
        let kind = value["type"].as_str().unwrap_or_default();
        let mut message = value["message"].as_str().unwrap_or_default().trim();
        if kind == "provider.auth" {
            return None;
        }
        let details = &value["limit"];
        let explicit = matches!(details["scope"].as_str(), Some("model" | "provider"));
        let limited = match kind {
            "provider.quota"
            | "provider.budget-limit"
            | "provider.free-limit"
            | "provider.rate-limit"
            | "provider.model-budget-limit"
            | "limit" => true,
            "" | "provider.unknown" => {
                let lower = message.to_ascii_lowercase();
                self.usage.is_match(message)
                    && !lower.contains("overloaded")
                    && !lower.contains("capacity")
            }
            _ => false,
        };
        if !explicit && !limited {
            return None;
        }
        let absolute = explicit.then(|| details["resetsAt"].as_u64()).flatten();
        if absolute.is_some()
            && message.ends_with('.')
            && let Some((reason, _)) = message.rsplit_once(" It resets at ")
        {
            message = reason;
        }
        Some(LimitStop {
            message: if message.is_empty() {
                "The model reached a usage limit.".to_owned()
            } else {
                message.to_owned()
            },
            resets_at: absolute.or_else(|| self.relative_reset(message, at)),
            releasing: false,
        })
    }

    fn relative_reset(&self, message: &str, at: u64) -> Option<u64> {
        let reset = self.reset.captures(message)?;
        let total = self
            .duration
            .captures_iter(&reset[1])
            .filter_map(|part| {
                let number = part[1].parse::<f64>().ok()?;
                let unit = match part[2].to_ascii_lowercase().as_str() {
                    "d" => 86_400_000.,
                    "h" => 3_600_000.,
                    "m" => 60_000.,
                    _ => 1_000.,
                };
                Some(number * unit)
            })
            .sum::<f64>();
        (total > 0.).then(|| at.saturating_add(total.min(31. * 86_400_000.) as u64))
    }
}

pub(super) fn line(home: &Path, stop: &LimitStop) -> String {
    match stop.resets_at.and_then(|at| reset_time(home, at)) {
        Some(time) => format!("{} It resets at {time}.", stop.message),
        None => format!("{} The provider didn't say when it resets.", stop.message),
    }
}

/// When a limit resets (ms), in your time zone: "4:30 PM", or "Oct 21, 9:00 AM" when it isn't
/// today.
pub fn reset_time(home: &Path, at: u64) -> Option<String> {
    let at = Timestamp::from_millisecond(i64::try_from(at).ok()?).ok()?;
    // Setup writes its accepted zone here, for both standalone and member computers.
    let accepted = std::fs::read_to_string(home.join("wakes/timezone")).ok();
    let local = std::fs::read(home.join("box/time-zone.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value["zone"].as_str().map(str::to_owned));
    let given = std::env::var("KUVO_TZ").ok();
    let zone = [accepted, local, given]
        .into_iter()
        .flatten()
        .find_map(|name| TimeZone::get(name.trim()).ok())
        .unwrap_or_else(TimeZone::system);
    let now = Timestamp::now().to_zoned(zone.clone());
    let at = at.to_zoned(zone);
    let format = if now.date() == at.date() {
        "%-I:%M %p"
    } else {
        "%b %-d, %-I:%M %p"
    };
    Some(at.strftime(format).to_string())
}

static WORDS: std::sync::LazyLock<Classifier> = std::sync::LazyLock::new(Classifier::new);

/// Whether an error's words say a model's limit was reached (a plan's window, a rate limit, a 429),
/// which resets, rather than that it's busy, overloaded or at capacity.
pub fn limit_reached(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    WORDS.usage.is_match(message) && !lower.contains("overloaded") && !lower.contains("capacity")
}

/// When a limit an error's words name resets (ms), as they say it ("try again in 5 minutes").
pub fn resets_in(message: &str, at: u64) -> Option<u64> {
    WORDS.relative_reset(message, at)
}

pub(super) fn watch(engine: Engine, stop: CancellationToken) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        stop.run_until_cancelled(async {
            let mut changes = engine.queues.changes.subscribe();
            loop {
                changes.borrow_and_update();
                let mut next = Duration::from_secs(60);
                let mut due = Vec::new();
                let now = disk::now();
                if let Ok(index) = engine.queues.index().await {
                    for (session, queue) in index {
                        let Some(reset) = queue.limit_stop.and_then(|stop| stop.resets_at) else {
                            continue;
                        };
                        if reset <= now {
                            due.push((session, reset));
                        } else {
                            next = next.min(Duration::from_millis(reset - now));
                        }
                    }
                }
                if due.is_empty() {
                    tokio::select! {
                        _ = changes.changed() => {}
                        _ = tokio::time::sleep(next) => {}
                    }
                } else {
                    futures_util::stream::iter(due)
                        .for_each_concurrent(8, |(session, reset)| {
                            let engine = &engine;
                            async move {
                                let _ = release(engine, &session, reset).await;
                            }
                        })
                        .await;
                    // A refused prompt or unfinished removal stays persisted for the next try.
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        })
        .await;
    })
}

async fn release(engine: &Engine, session: &str, reset: u64) -> io::Result<()> {
    let _turn = engine.turn(session).await;
    let saved = engine.queues.get(session).await?;
    let is_ours = |stop: &LimitStop| stop.resets_at == Some(reset);
    if !saved.limit_stop.as_ref().is_some_and(is_ours) || reset > disk::now() {
        return Ok(());
    }
    let Ok(_admitted) = crate::session_move::admit(engine, session, None).await else {
        return Ok(());
    };
    engine.queues.verify_waiting(&saved).await?;
    let Some(lease) = engine.acquire(Demand::Passive).await else {
        return Ok(());
    };
    let info = engine
        .call::<Value>(&lease, Method::GET, &sessions::path(session), None)
        .await
        .map_err(io::Error::other)?;
    if info["id"] != session {
        return Ok(());
    }
    let saved = engine
        .queues
        .put(session, |state| {
            if let Some(stop) = state.limit_stop.as_mut().filter(|stop| is_ours(stop)) {
                state.paused = false;
                state.resumed = Some(disk::now());
                stop.releasing = true;
            }
        })
        .await?;
    if !saved.limit_stop.as_ref().is_some_and(is_ours) {
        return Ok(());
    }
    let rows = inbox(engine, super::Target::Engine(&lease), session)
        .await
        .map_err(io::Error::other)?;
    if drain(
        engine,
        super::Target::Engine(&lease),
        session,
        &saved,
        &rows,
    )
    .await?
    {
        engine
            .queues
            .put(session, |state| {
                if state
                    .limit_stop
                    .as_ref()
                    .is_some_and(|stop| is_ours(stop) && stop.releasing)
                {
                    state.limit_stop = None;
                }
            })
            .await?;
    }
    Ok(())
}
