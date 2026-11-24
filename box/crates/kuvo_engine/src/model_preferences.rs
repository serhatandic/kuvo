//! The model picker and coding agents share the local preference file.

use std::{future::Future, pin::Pin, sync::Arc};

use http::StatusCode;
use kuvo_protocol::routes::{
    get_kuvo_models_starred as read_stars, post_kuvo_models_speed as set_speed,
    post_kuvo_models_starred as set_star,
};
use kuvo_server::{Access, Caller, Refusal, Registry, RouteReply};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{Engine, disk, sessions::typed};

const MAX: usize = 200;
const FILE: &str = "box/models.json";

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ModelStar {
    #[serde(rename = "providerID")]
    pub provider: String,
    pub id: String,
}

impl ModelStar {
    fn valid(&self) -> bool {
        id(&self.provider) && id(&self.id)
    }
}

pub type StarSave<'a> =
    Pin<Box<dyn Future<Output = Result<Option<Vec<ModelStar>>, Refusal>> + Send + 'a>>;

pub trait ModelStarSource: Send + Sync {
    fn current(&self) -> Option<Vec<ModelStar>>;
    fn save<'a>(&'a self, caller: &'a Caller, model: ModelStar, starred: bool) -> StarSave<'a>;
}

impl Engine {
    pub fn with_model_stars(mut self, source: Arc<dyn ModelStarSource>) -> Self {
        self.model_stars = Some(source);
        self
    }
}

#[derive(Deserialize)]
struct StarChange {
    #[serde(flatten)]
    model: ModelStar,
    starred: bool,
}

fn id(given: &str) -> bool {
    (1..=300).contains(&given.encode_utf16().count())
}

pub(crate) fn speed_id(given: &str) -> bool {
    (1..=64).contains(&given.len())
        && given
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn valid_speed(given: &Value) -> bool {
    given.is_null() || given.as_str().is_some_and(speed_id)
}

fn stars(kept: Option<&Value>) -> Vec<ModelStar> {
    let mut stars = Vec::new();
    let rows = kept.and_then(Value::as_array).into_iter().flatten();
    for star in rows.filter_map(|row| serde_json::from_value::<ModelStar>(row.clone()).ok()) {
        if star.valid() && !stars.contains(&star) {
            stars.push(star);
        }
    }
    stars.drain(..stars.len().saturating_sub(MAX));
    stars
}

async fn settings(engine: &Engine) -> Map<String, Value> {
    disk::read(&engine.home.join(FILE))
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
}

pub(crate) async fn picked_stars(engine: &Engine) -> Vec<ModelStar> {
    if let Some(stars) = engine
        .model_stars
        .as_ref()
        .and_then(|source| source.current())
    {
        return stars;
    }
    stars(settings(engine).await.get("starred"))
}

pub(crate) async fn picked_speeds(engine: &Engine) -> Map<String, Value> {
    speeds(&settings(engine).await)
}

fn speeds(kept: &Map<String, Value>) -> Map<String, Value> {
    kept.get("speeds")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(provider, speed)| id(provider) && valid_speed(speed))
        .map(|(provider, speed)| (provider.clone(), speed.clone()))
        .collect()
}

fn io_refusal(_: impl std::fmt::Display) -> Refusal {
    Refusal::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "The model preference could not be saved.",
    )
}

async fn change_star(
    engine: Engine,
    caller: Caller,
    change: StarChange,
) -> Result<Vec<ModelStar>, Refusal> {
    tokio::spawn(async move {
        if let Some(source) = &engine.model_stars
            && let Some(saved) = source
                .save(&caller, change.model.clone(), change.starred)
                .await?
        {
            return Ok(saved);
        }
        let _turn = engine.turn("model-preferences").await;
        let mut kept = settings(&engine).await;
        let mut stars = stars(kept.get("starred"));
        let existing = stars.iter().position(|star| star == &change.model);
        match (existing, change.starred) {
            (Some(at), false) => {
                stars.remove(at);
            }
            (None, true) => {
                stars.push(change.model);
                stars.drain(..stars.len().saturating_sub(MAX));
            }
            _ => return Ok(stars),
        }
        kept.insert("starred".into(), json!(stars));
        disk::durable(engine.home.join(FILE), kept)
            .await
            .map_err(io_refusal)?;
        Ok(stars)
    })
    .await
    .map_err(io_refusal)?
}

async fn change_speed(engine: Engine, provider: String, speed: Value) -> Result<Value, Refusal> {
    tokio::spawn(async move {
        let _turn = engine.turn("model-preferences").await;
        let mut kept = settings(&engine).await;
        let was = speeds(&kept);
        if was.get(&provider) != Some(&speed) {
            let others = was
                .into_iter()
                .filter(|(id, _)| id != &provider)
                .collect::<Vec<_>>();
            let skip = others.len().saturating_sub(MAX - 1);
            let after = others
                .into_iter()
                .skip(skip)
                .chain([(provider.clone(), speed.clone())])
                .collect();
            kept.insert("speeds".into(), Value::Object(after));
            disk::durable(engine.home.join(FILE), kept)
                .await
                .map_err(io_refusal)?;
        }
        Ok(json!({ "providerID": provider, "speed": speed }))
    })
    .await
    .map_err(io_refusal)?
}

pub(crate) fn register(engine: &Engine, routes: &mut Registry) {
    let here = engine.clone();
    routes.register::<read_stars::Spec, _, _>(Access::Any, move |_| {
        let here = here.clone();
        async move {
            Ok(RouteReply::json::<200>(typed(
                json!({"starred": picked_stars(&here).await}),
            )?))
        }
    });
    let here = engine.clone();
    routes.register::<set_star::Spec, _, _>(Access::Any, move |call| {
        let here = here.clone();
        async move {
            let bad = || Refusal::new(StatusCode::BAD_REQUEST, "Say which model (providerID, id) and whether it's starred (starred, true or false).");
            let change: StarChange = serde_json::from_slice(&call.request.body).map_err(|_| bad())?;
            if !change.model.valid() {
                return Err(bad());
            }
            let stars = change_star(here, call.request.caller, change).await?;
            Ok(RouteReply::json::<200>(typed(json!({"starred": stars}))?))
        }
    });
    let here = engine.clone();
    routes.register::<set_speed::Spec, _, _>(Access::Any, move |call| {
        let here = here.clone();
        async move {
            let bad = || Refusal::new(StatusCode::BAD_REQUEST, "Say which provider (providerID) and its speed (speed: a speed's id, or null for Standard).");
            let given: Value = serde_json::from_slice(&call.request.body).map_err(|_| bad())?;
            let provider = given["providerID"].as_str().filter(|provider| id(provider)).ok_or_else(bad)?;
            let speed = given.get("speed").filter(|speed| valid_speed(speed)).ok_or_else(bad)?;
            Ok(RouteReply::json::<200>(typed(change_speed(here, provider.to_owned(), speed.clone()).await?)?))
        }
    });
}
