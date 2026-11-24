//! What the bot routes ask the engine: the accounts signed in here, the models that run, what a
//! bot's conversation is told, and telling it.

use std::time::Duration;

use http::Method;
use kuvo_engine::BotEngine;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Value, json};

/// How long a step of making, changing or deleting a bot waits on the engine, once under way.
pub const BOT_STEP: Duration = Duration::from_secs(30);

/// A path segment, as `encodeURIComponent` writes it.
pub fn segment(text: &str) -> String {
    const KEEP: &percent_encoding::AsciiSet = &NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'!')
        .remove(b'~')
        .remove(b'*')
        .remove(b'\'')
        .remove(b'(')
        .remove(b')');
    utf8_percent_encode(text, KEEP).to_string()
}

/// A model that runs here: its provider and id, its name and efforts, and other ids it's known by.
#[derive(Clone, Debug)]
pub struct Usable {
    pub provider: String,
    pub id: String,
    pub name: String,
    pub efforts: Vec<String>,
    pub aliases: Vec<String>,
    pub retiring: bool,
}

/// The accounts the engine says are ready here, by provider (empty where it doesn't say).
async fn providers(engine: &BotEngine, wait: Duration) -> Vec<String> {
    let ready = engine
        .call(Method::GET, "api/provider", None, wait)
        .await
        .unwrap_or_default();
    ready
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|each| each.get("id")?.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Why a model of the engine's can't run here: its account isn't signed in here; none where it can
/// (or the engine doesn't say).
pub async fn sign_in_refusal(engine: &BotEngine, model: &Value, here: &str) -> Option<String> {
    let ids = providers(engine, BOT_STEP).await;
    let provider = model["providerID"].as_str().unwrap_or_default();
    (!ids.is_empty() && !ids.iter().any(|id| id == provider)).then(|| {
        format!(
            "{} needs a sign-in on {here}: sign in to it in Settings › Accounts, or choose another model.",
            model["id"].as_str().unwrap_or_default()
        )
    })
}

/// Every model there's an account for here, by the providers the engine says are ready.
pub async fn usable_models(engine: &BotEngine) -> Vec<Usable> {
    let (ready, listed) = tokio::join!(
        providers(engine, BOT_STEP),
        engine.call(Method::GET, "api/model", None, BOT_STEP)
    );
    let listed = listed.unwrap_or_default();
    let strings = |value: &Value, of: fn(&Value) -> Option<&str>| -> Vec<String> {
        value
            .as_array()
            .map(|list| list.iter().filter_map(of).map(str::to_owned).collect())
            .unwrap_or_default()
    };
    listed
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|each| {
            let id = each.get("id")?.as_str()?;
            let provider = each.get("providerID")?.as_str()?;
            if each.get("enabled") == Some(&Value::Bool(false))
                || !ready.iter().any(|ready| ready == provider)
            {
                return None;
            }
            let name = each["name"].as_str().filter(|name| !name.is_empty()).unwrap_or(id);
            Some(Usable {
                provider: provider.to_owned(),
                id: id.to_owned(),
                name: name.to_owned(),
                efforts: strings(&each["variants"], |variant| variant.get("id")?.as_str()),
                aliases: strings(&each["aliases"], Value::as_str),
                retiring: each["status"] == "deprecated",
            })
        })
        .collect()
}

/// The model named `model` among `models`: by its own id, else one it's an alias of.
pub fn model_named<'a>(model: &Value, models: &'a [Usable]) -> Option<&'a Usable> {
    let provider = model["providerID"].as_str()?;
    let id = model["id"].as_str()?;
    let of_provider: Vec<_> = models.iter().filter(|each| each.provider == provider).collect();
    of_provider
        .iter()
        .find(|each| each.id == id)
        .or_else(|| of_provider.iter().find(|each| each.aliases.iter().any(|alias| alias == id)))
        .copied()
}

/// The model a new bot starts on here, with none named: by the box's rule, else (while that
/// can't say) this computer's default where it runs, else the first that runs; none for none.
pub async fn starting_model(engine: &BotEngine) -> Option<Value> {
    match engine.starting_model("").await {
        Some(model) => return model,
        None => {}
    }
    let models: Vec<Usable> = usable_models(engine)
        .await
        .into_iter()
        .filter(|model| !model.retiring)
        .collect();
    let first = models.first()?;
    let fallback = engine
        .call(Method::GET, "api/model/default", None, BOT_STEP)
        .await
        .ok()
        .filter(|fallback| {
            models.iter().any(|each| {
                fallback["id"].as_str() == Some(&each.id)
                    && fallback["providerID"].as_str() == Some(&each.provider)
            })
        });
    Some(match fallback {
        Some(fallback) => json!({"id": fallback["id"], "providerID": fallback["providerID"]}),
        None => json!({"id": first.id, "providerID": first.provider}),
    })
}

fn instructions(session: &str) -> String {
    format!(
        "api/experimental/session/{}/instructions/entries",
        segment(session)
    )
}

/// What a conversation is told as a bot (its `kuvo.bot` entry): its words, `Some(None)` for none,
/// `None` where that can't be read.
pub async fn told_there(engine: &BotEngine, session: &str) -> Option<Option<String>> {
    let entries = engine
        .call(Method::GET, &instructions(session), None, BOT_STEP)
        .await
        .ok()?;
    let entries = entries.as_array()?;
    Some(
        entries
            .iter()
            .find(|each| each.get("key").and_then(Value::as_str) == Some("kuvo.bot"))
            .and_then(|entry| entry.get("value")?.as_str().map(str::to_owned)),
    )
}

/// The conversation told `text` as a bot; whether it got there.
pub async fn tell(engine: &BotEngine, session: &str, text: &str) -> bool {
    engine
        .call(
            Method::PUT,
            &format!("{}/kuvo.bot", instructions(session)),
            Some(&json!({ "value": text })),
            BOT_STEP,
        )
        .await
        .is_ok()
}
