//! Models that can't answer now (`run/work/unanswering.json`): one whose session's turn failed at
//! its account (out of funds, at a plan's limit, signed out), which holds back every model of that
//! provider (those that cost something, for funds: a free one spends none; the free ones, for the
//! free usage it allows), or at its own limit (a rate limit), which holds back that model. Its
//! starter hears why; list_models leaves them out, and start_session on one says so and names one
//! that can. None is kept for good: each lasts until its reset (when the error says), else half an
//! hour (a rate limit a few minutes), or until a session on it answers, or that account's sign-in or
//! key changes (funds added, say).

use std::{path::PathBuf, sync::LazyLock, time::Duration};

use http::Method;
use kuvo_engine::{BotEngine, EngineEvent};
use kuvo_store::locks::{OnBusy, StaleLock};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const UNKNOWN_RESET: u64 = 30 * 60_000;
const BRIEF_RESET: u64 = 3 * 60_000;
const WAIT: Duration = Duration::from_secs(10);

// The engine's words for an account that can't (its funds, its plan's spend), or that's signed
// out; for a model at its own limit.
const FUNDS: &[&str] = &["provider.quota", "provider.budget-limit"];
const FREE_USAGE: &str = "provider.free-limit";
const MODEL_LIMITS: &[&str] = &[
    "provider.rate-limit",
    "provider.model-budget-limit",
    "limit",
];
// What the engine says when an account's sign-in or key changes.
const ACCOUNT_CHANGES: &[&str] = &[
    "credential.updated",
    "credential.switched",
    "credential.removed",
    "integration.updated",
    "provider.updated",
];

/// How widely a hold reaches: the whole account, or the model alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Provider,
    Model,
}

fn no(flag: &bool) -> bool {
    !flag
}

/// What's held back: a provider ("opencode") or one model ("opencode/gpt-6.1-sol"), only those of
/// it that cost something (`paid`: its funds ran out) or only its free ones (`free`: its free usage
/// did), why in its engine's words, since when and until when (ms).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Unanswering {
    pub key: String,
    pub scope: Scope,
    #[serde(default, skip_serializing_if = "no")]
    pub paid: bool,
    #[serde(default, skip_serializing_if = "no")]
    pub free: bool,
    pub why: String,
    pub at: u64,
    pub until: u64,
}

/// A model, as a session names it, and whether it's free (costs nothing), where that's known.
#[derive(Clone, Debug, Default)]
pub struct ModelRef {
    pub provider: String,
    pub id: String,
    pub free: Option<bool>,
}

impl ModelRef {
    pub fn new(provider: &str, id: &str) -> Self {
        let (provider, id) = (provider.to_owned(), id.to_owned());
        Self {
            provider,
            id,
            free: None,
        }
    }
}

/// An error a turn failed with, as its engine gives it, and when (if known); with the limit it
/// names, if it names one (how widely, and when it resets).
#[derive(Clone, Debug, PartialEq)]
pub struct TurnError {
    pub kind: String,
    pub message: String,
    pub at: Option<u64>,
    pub limit: Option<(Scope, Option<u64>)>,
}

/// An error as the engine gives it, if it's one.
pub fn error_of(value: &Value, at: Option<u64>) -> Option<TurnError> {
    if !value["type"].is_string() && !value["message"].is_string() {
        return None;
    }
    let text = |value: &Value| match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    let details = &value["limit"];
    let scope = serde_json::from_value::<Scope>(details["scope"].clone()).ok();
    let resets = details["resetsAt"].as_f64().filter(|at| at.is_finite());
    Some(TurnError {
        kind: text(&value["type"]),
        message: text(&value["message"]).trim().to_owned(),
        at: at.filter(|at| *at > 0),
        limit: scope.map(|scope| (scope, resets.map(|at| at as u64))),
    })
}

/// Whether `error` says the model can't answer for now, and how widely. An error the engine didn't
/// type counts by its words, if they say a limit (not that it's busy, overloaded or at capacity).
pub fn scope_of(error: &TurnError) -> Option<Scope> {
    let kind = error.kind.as_str();
    if let Some((scope, _)) = error.limit {
        return Some(scope);
    }
    if FUNDS.contains(&kind) || kind == FREE_USAGE || kind == "provider.auth" {
        return Some(Scope::Provider);
    }
    if MODEL_LIMITS.contains(&kind) {
        return Some(Scope::Model);
    }
    let untyped = kind.is_empty() || kind == "provider.unknown";
    (untyped && kuvo_engine::limit_reached(&error.message)).then_some(Scope::Model)
}

/// When the limit `error` names resets, if it says.
pub fn reset_of(error: &TurnError, at: u64) -> Option<u64> {
    let named = error.limit.and_then(|(_, resets)| resets);
    named.or_else(|| kuvo_engine::resets_in(&error.message, at))
}

/// Whether `model` costs nothing, by its engine's prices (an OpenCode Zen model's name may say so).
pub fn is_free(model: &Value) -> bool {
    let cost = match &model["cost"] {
        Value::Array(cost) => cost.clone(),
        cost @ Value::Object(_) => vec![cost.clone()],
        _ => Vec::new(),
    };
    let none = |each: &Value| each["input"] == 0 && each["output"] == 0;
    if !cost.is_empty() && cost.iter().all(none) {
        return true;
    }
    let name = model["name"].as_str().unwrap_or_default();
    model["providerID"] == "opencode" && FREE_NAME.is_match(name)
}

// Whether a provider's hold is one on `model`: its paid ones', or its free ones', or all of them.
fn covers(held: &Unanswering, free: bool) -> bool {
    !(held.paid && free) && !(held.free && !free)
}

/// Why `model` can't answer now, if it can't: its provider's hold (one for funds, if it costs
/// something), or its own, kept under any name it's known by (a session on Claude Code's
/// `default`).
pub fn hold_of<'a>(
    model: &ModelRef,
    aliases: &[String],
    held: &'a [Unanswering],
) -> Option<&'a Unanswering> {
    let free = model.free.unwrap_or(false);
    let ids = std::iter::once(&model.id).chain(aliases);
    let keys: Vec<String> = ids.map(|id| format!("{}/{id}", model.provider)).collect();
    let provider = held
        .iter()
        .find(|each| each.key == model.provider && covers(each, free));
    provider.or_else(|| held.iter().find(|each| keys.contains(&each.key)))
}

/// What a hold is, in a line for an agent: "No opencode model can answer now: …".
pub fn hold_words(held: &Unanswering, home: &std::path::Path) -> String {
    let key = &held.key;
    let what = match (held.scope, held.paid, held.free) {
        (Scope::Model, ..) => format!("{key} can't answer now"),
        (_, true, _) => format!("No paid {key} model can answer now"),
        (_, _, true) => format!("No free {key} model can answer now"),
        _ => format!("No {key} model can answer now"),
    };
    let why = held.why.trim_end_matches('.');
    let why = match why.chars().count() {
        count if count > 200 => why.chars().take(199).chain(['…']).collect(),
        _ => why.to_owned(),
    };
    let time = |at| kuvo_engine::reset_time(home, at).unwrap_or_default();
    let (since, until) = (time(held.at), time(held.until));
    format!(
        "{what}: {why} (since {since}; offered again after {until}, or once a session on it answers)"
    )
}

/// The holds kept here.
#[derive(Clone)]
pub struct Unanswerings {
    file: PathBuf,
}

fn now() -> u64 {
    jiff::Timestamp::now().as_millisecond().max(0) as u64
}

impl Unanswerings {
    pub fn new(run: &std::path::Path) -> Self {
        let file = run.join("work/unanswering.json");
        Self { file }
    }

    /// What can't answer now.
    pub fn now(&self) -> Vec<Unanswering> {
        self.read(now())
    }

    fn read(&self, now: u64) -> Vec<Unanswering> {
        let kept = kuvo_store::files::text_of(&self.file);
        let kept: Vec<Value> = kept
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        let held = kept
            .into_iter()
            .filter_map(|each| serde_json::from_value::<Unanswering>(each).ok());
        held.filter(|each| each.until > now).collect()
    }

    fn change(&self, now: u64, edit: impl FnOnce(Vec<Unanswering>) -> Vec<Unanswering>) {
        let lock = self.file.with_extension("json.lock");
        let wait = Duration::from_secs(3);
        let _lock = StaleLock::acquire(&lock, Duration::from_secs(10), wait, OnBusy::Proceed);
        let text = serde_json::to_string(&edit(self.read(now))).expect("JSON");
        let _ = kuvo_store::files::write_secret(&self.file, text.as_bytes());
    }

    /// What can't answer now, as a client reads it (`GET kuvo/work/unanswering`): with the models
    /// a hold on an account's paid or free ones covers, out of `listed` (its engine's, when it
    /// could say).
    pub fn list(held: &[Unanswering], listed: Option<&[Value]>) -> Value {
        let shown = held.iter().map(|each| {
            let (provider, model) = each.key.split_once('/').unwrap_or((&each.key, ""));
            let mut shown = json!({ "provider": provider });
            if each.scope == Scope::Model {
                shown["model"] = json!(model);
            }
            if each.paid || each.free {
                shown["only"] = json!(if each.paid { "paid" } else { "free" });
            }
            let only = each.scope == Scope::Provider && (each.paid || each.free);
            if let Some(listed) = listed.filter(|_| only) {
                let ours = |one: &&Value| one["providerID"] == provider && one["id"].is_string();
                let covered = listed
                    .iter()
                    .filter(ours)
                    .filter(|one| covers(each, is_free(one)));
                shown["models"] = json!(covered.map(|one| one["id"].clone()).collect::<Vec<_>>());
            }
            shown["why"] = json!(each.why);
            shown["since"] = json!(each.at);
            shown["until"] = json!(each.until);
            shown
        });
        json!({ "held": shown.collect::<Vec<_>>() })
    }

    /// `model`'s turn failed with `error`: held back, when that says it can't answer for now. A free
    /// model that fails for want of funds is held alone. Counted from when it failed: one that's
    /// over by now (a failure caught up on later) isn't kept.
    pub fn note_failed(
        &self,
        model: &ModelRef,
        error: &TurnError,
        now: u64,
    ) -> Option<Unanswering> {
        let mut scope = scope_of(error)?;
        let kind = error.kind.as_str();
        let (funds, free_usage, free) = (
            FUNDS.contains(&kind),
            kind == FREE_USAGE,
            model.free.unwrap_or(false),
        );
        if scope == Scope::Provider && ((funds && free) || (free_usage && !free)) {
            scope = Scope::Model;
        }
        let at = error.at.unwrap_or(now).min(now);
        let reset = reset_of(error, at);
        let limited = RATE_LIMIT.is_match(&error.message);
        let brief = reset.is_none()
            && (kind == "provider.rate-limit" || (scope == Scope::Model && limited));
        let key = match scope {
            Scope::Provider => model.provider.clone(),
            Scope::Model => format!("{}/{}", model.provider, model.id),
        };
        let account = scope == Scope::Provider;
        let why = if error.message.is_empty() {
            "It's at a usage limit."
        } else {
            &error.message
        };
        let until = reset.unwrap_or(at + if brief { BRIEF_RESET } else { UNKNOWN_RESET });
        let (paid, free) = (account && funds, account && !funds && free_usage);
        let held = Unanswering {
            key,
            scope,
            paid,
            free,
            why: why.to_owned(),
            at,
            until,
        };
        if held.until <= now {
            return None;
        }
        // (In place of the same one: an account's free models may be held apart from its paid ones.)
        let same = |each: &Unanswering| {
            each.key == held.key && each.paid == held.paid && each.free == held.free
        };
        self.change(now, |all| {
            all.into_iter()
                .filter(|each| !same(each))
                .chain([held.clone()])
                .collect()
        });
        Some(held)
    }

    /// A session on `model` answered: it, and its provider, can again (a free one says nothing of
    /// the provider's funds, nor a paid one of its free usage).
    pub fn note_answered(&self, model: &ModelRef, now: u64) {
        let key = format!("{}/{}", model.provider, model.id);
        let free = model.free.unwrap_or(false);
        let clears = |each: &Unanswering| {
            each.key == key || (each.key == model.provider && covers(each, free))
        };
        if self.read(now).iter().any(clears) {
            self.change(now, |all| {
                all.into_iter().filter(|each| !clears(each)).collect()
            });
        }
    }

    /// Whether `model` is free, by its engine.
    async fn free_there(engine: &BotEngine, model: &ModelRef) -> bool {
        let listed = engine
            .call(Method::GET, "api/model", None, WAIT)
            .await
            .unwrap_or_default();
        let this = |each: &&Value| {
            each["providerID"] == model.provider.as_str() && each["id"] == model.id.as_str()
        };
        listed
            .as_array()
            .and_then(|listed| listed.iter().find(this))
            .is_some_and(is_free)
    }

    /// A session on `model` answered: as `note_answered`, knowing whether it's free where that
    /// matters (a hold for its provider's funds).
    pub async fn answered_on(&self, engine: &BotEngine, model: &ModelRef) {
        let (held, at) = (self.read(now()), now());
        let key = format!("{}/{}", model.provider, model.id);
        if !held
            .iter()
            .any(|each| each.key == model.provider || each.key == key)
        {
            return;
        }
        let split = held
            .iter()
            .any(|each| each.key == model.provider && (each.paid || each.free));
        let free = split && Self::free_there(engine, model).await;
        self.note_answered(
            &ModelRef {
                free: Some(free),
                ..model.clone()
            },
            at,
        );
    }

    /// `model` failed with `error`: as `note_failed`, knowing whether it's free where that matters
    /// (a failure for want of funds).
    pub async fn failed_on(
        &self,
        engine: &BotEngine,
        model: &ModelRef,
        error: &TurnError,
    ) -> Option<Unanswering> {
        let spent = FUNDS.contains(&error.kind.as_str()) || error.kind == FREE_USAGE;
        let free = match model.free {
            Some(free) => free,
            None => spent && Self::free_there(engine, model).await,
        };
        self.note_failed(
            &ModelRef {
                free: Some(free),
                ..model.clone()
            },
            error,
            now(),
        )
    }

    /// What can't answer, as the engine's events change it: an account's models once it changes
    /// (its key or sign-in: tried again; all of them, when the event doesn't name one, except the
    /// bare `credential.updated` that comes with every change and with a sign-in's refresh), a
    /// model and its provider once a turn on it ends well.
    pub async fn follow(&self, event: &EngineEvent, engine: Option<&BotEngine>) {
        let now = now();
        if self.read(now).is_empty() {
            return;
        }
        let data = &event.data;
        if ACCOUNT_CHANGES.contains(&event.kind.as_str()) {
            let named = ["integrationID", "providerID", "id"]
                .iter()
                .find_map(|key| data[key].as_str().filter(|id| !id.is_empty()));
            match named {
                Some(named) => {
                    let within = format!("{named}/");
                    let gone =
                        |each: &Unanswering| each.key == named || each.key.starts_with(&within);
                    self.change(now, |all| {
                        all.into_iter().filter(|each| !gone(each)).collect()
                    });
                }
                None if event.kind != "credential.updated" => self.change(now, |_| Vec::new()),
                None => {}
            }
            return;
        }
        let (Some(session), Some(engine)) = (data["sessionID"].as_str(), engine) else {
            return;
        };
        if event.kind != "session.execution.succeeded" {
            return;
        }
        let session =
            percent_encoding::utf8_percent_encode(session, percent_encoding::NON_ALPHANUMERIC);
        let read = engine
            .call(Method::GET, &format!("api/session/{session}"), None, WAIT)
            .await;
        let model = &read.unwrap_or_default()["model"];
        if let (Some(provider), Some(id)) = (model["providerID"].as_str(), model["id"].as_str()) {
            self.answered_on(engine, &ModelRef::new(provider, id)).await;
        }
    }
}

fn words(pattern: &'static str) -> regex::Regex {
    regex::Regex::new(pattern).expect("a pattern")
}

static RATE_LIMIT: LazyLock<regex::Regex> =
    LazyLock::new(|| words(r"(?i)\b429\b|rate.?limit|too many requests"));
static FREE_NAME: LazyLock<regex::Regex> = LazyLock::new(|| words(r"(?i)\sfree$"));

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn funds() -> TurnError {
        let value = json!({ "type": "provider.quota", "message": "Insufficient account funds" });
        error_of(&value, Some(1_000)).unwrap()
    }

    #[test]
    fn funds_hold_an_accounts_paid_models_for_half_an_hour_and_a_free_one_alone() {
        let folder = tempfile::tempdir().unwrap();
        let kept = Unanswerings::new(folder.path());
        let paid = ModelRef {
            free: Some(false),
            ..ModelRef::new("opencode", "gpt")
        };
        let held = kept.note_failed(&paid, &funds(), 2_000).unwrap();
        assert_eq!(
            (held.key.as_str(), held.scope, held.paid, held.until),
            ("opencode", Scope::Provider, true, 1_000 + 30 * 60_000)
        );
        let free = ModelRef {
            free: Some(true),
            ..ModelRef::new("opencode", "flash-free")
        };
        assert_eq!(
            hold_of(&free, &[], &kept.read(2_000)),
            None,
            "a free one spends no funds"
        );
        let alone = kept.note_failed(&free, &funds(), 2_000).unwrap();
        assert_eq!(
            (alone.key.as_str(), alone.scope),
            ("opencode/flash-free", Scope::Model)
        );
        kept.note_answered(&free, 3_000);
        assert_eq!(
            kept.read(3_000).len(),
            1,
            "a free one answering says nothing of the funds"
        );
        kept.note_answered(&paid, 3_000);
        assert!(kept.read(3_000).is_empty());
    }

    #[test]
    fn only_a_limit_holds_a_model_back_and_a_rate_limit_briefly() {
        let busy = error_of(
            &json!({ "type": "provider.unknown", "message": "Overloaded" }),
            None,
        )
        .unwrap();
        assert_eq!(scope_of(&busy), None);
        let rate = error_of(&json!({ "message": "429 Too Many Requests" }), Some(5)).unwrap();
        let folder = tempfile::tempdir().unwrap();
        let held = Unanswerings::new(folder.path())
            .note_failed(&ModelRef::new("x", "y"), &rate, 10)
            .unwrap();
        assert_eq!((held.key.as_str(), held.until), ("x/y", 5 + 3 * 60_000));
        let named = error_of(&json!({ "type": "x", "message": "m", "limit": { "scope": "model", "resetsAt": 99_999 } }), Some(5)).unwrap();
        assert_eq!(reset_of(&named, 5), Some(99_999));
    }

    #[test]
    fn a_hold_kept_under_another_name_marks_the_model_it_stands_for() {
        let held = [Unanswering {
            key: "claude-code/default".into(),
            scope: Scope::Model,
            paid: false,
            free: false,
            why: "At its limit".into(),
            at: 1,
            until: u64::MAX,
        }];
        let opus = ModelRef::new("claude-code", "opus");
        assert_eq!(
            hold_of(&opus, &["default".into()], &held).map(|held| held.why.as_str()),
            Some("At its limit")
        );
        assert_eq!(
            hold_of(&ModelRef::new("claude-code", "haiku"), &[], &held),
            None
        );
    }
}
