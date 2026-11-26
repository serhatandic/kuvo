//! Versioned usage preferences. The main computer accepts shared changes;
//! an independent computer keeps its own settings at the existing disk path.
use http::StatusCode;
use kuvo_protocol::{
    JsonStatus, Route, RouteSpec,
    shape::{Primitive, Shape},
};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;

const PARTS: &[&str] = &[
    "layout",
    "providers",
    "totalSpend",
    "codexFallback",
    "notifications",
];
const LAYOUT: &[&str] = &[
    "placed",
    "providerOrder",
    "pinned",
    "expanded",
    "expandOnEnable",
    "seeded",
];

#[derive(Clone, Debug, Serialize)]
struct Stamp {
    v: u64,
    by: String,
    #[serde(skip_serializing_if = "is_false")]
    imported: bool,
}
fn is_false(value: &bool) -> bool {
    !*value
}
impl Stamp {
    fn version(&self) -> String {
        format!(
            "{}/{}{}",
            self.v,
            self.by,
            if self.imported { "/i" } else { "" }
        )
    }
}
#[derive(Clone, Debug, Serialize)]
struct Unit {
    value: Value,
    stamp: Stamp,
}
#[derive(Clone, Default, Debug, Serialize)]
struct Settings {
    units: BTreeMap<String, Unit>,
    imported: bool,
}

impl Settings {
    /// Read each independently versioned part. Older readers retain bounded
    /// future parts without interpreting them as settings they recognize.
    fn from_saved(value: &Value, future_limit: usize) -> Self {
        let mut settings = Self {
            imported: value["imported"] == true,
            ..Self::default()
        };
        let Some(parts) = value["units"].as_object() else {
            return settings;
        };
        let mut future = 0;
        for (name, kept) in parts {
            let stamp = &kept["stamp"];
            let Some(version) = stamp["v"]
                .as_f64()
                .filter(|v| (0.0..=9_007_199_254_740_991.0).contains(v) && v.fract() == 0.0)
            else {
                continue;
            };
            let Some(by) = stamp["by"]
                .as_str()
                .filter(|by| by.encode_utf16().count() <= 200)
            else {
                continue;
            };
            let Some(value) = kept.get("value") else {
                continue;
            };
            let value = if PARTS.contains(&name.as_str()) {
                let Some(value) = checked(name, value) else {
                    continue;
                };
                value
            } else {
                if future >= future_limit
                    || name.len() > 40
                    || !name.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
                    || !name.bytes().all(|b| b.is_ascii_alphanumeric())
                    || matches!(
                        name.as_str(),
                        "constructor"
                            | "toString"
                            | "valueOf"
                            | "hasOwnProperty"
                            | "isPrototypeOf"
                            | "propertyIsEnumerable"
                            | "toLocaleString"
                    )
                    || kuvo_wire::json::canonical(value).encode_utf16().count() > 64_000
                {
                    continue;
                }
                future += 1;
                value.clone()
            };
            settings.units.insert(
                name.clone(),
                Unit {
                    value,
                    stamp: Stamp {
                        v: version as u64,
                        by: by.to_owned(),
                        imported: stamp["imported"] == true,
                    },
                },
            );
        }
        settings
    }
}

fn id(value: &str, max: usize) -> bool {
    !value.is_empty() && value.encode_utf16().count() <= max && !value.chars().any(|c| c < ' ')
}
fn ids(value: &Value) -> Option<Value> {
    let list = value.as_array()?;
    if list.len() > 1000 {
        return None;
    }
    let mut result = vec![];
    for item in list {
        if !id(item.as_str()?, 200) {
            return None;
        }
        if !result.contains(item) {
            result.push(item.clone());
        }
    }
    Some(json!(result))
}
fn checked(name: &str, value: &Value) -> Option<Value> {
    if name == "codexFallback" {
        return (value.is_null() || value.as_str().is_some_and(|model| id(model, 200)))
            .then(|| value.clone());
    }
    let mut fields = value.as_object()?.clone();
    let empty = json!([]);
    match name {
        "layout" => {
            let order = value["metricOrder"].as_object()?;
            if order.len() > 200 {
                return None;
            }
            let mut clean = Map::new();
            for (name, list) in order {
                if !id(name, 200) {
                    return None;
                }
                clean.insert(name.clone(), ids(list)?);
            }
            fields.insert("metricOrder".into(), json!(clean));
            for key in LAYOUT {
                fields.insert((*key).into(), ids(value.get(key).unwrap_or(&empty))?);
            }
        }
        "providers" => {
            fields.insert("enabled".into(), ids(value.get("enabled")?)?);
            fields.insert("known".into(), ids(value.get("known").unwrap_or(&empty))?);
        }
        "notifications" => {
            for key in ["underTenPercent", "healthyToClose", "closeToRunningOut"] {
                value[key].as_bool()?;
            }
        }
        "totalSpend" => {
            value["show"].as_bool()?;
            for key in ["period", "metric"] {
                if !value[key].as_str().is_some_and(|s| id(s, 40)) {
                    fields.shift_remove(key);
                }
            }
        }
        _ => return None,
    }
    let unknown = fields
        .iter()
        .filter(|(key, _)| !known_fields(name, key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Map<_, _>>();
    if kuvo_wire::json::canonical(&Value::Object(unknown))
        .encode_utf16()
        .count()
        > 16_000
    {
        fields.retain(|key, _| known_fields(name, key));
    }
    Some(Value::Object(fields))
}

fn known_fields(name: &str, key: &str) -> bool {
    match name {
        "layout" => LAYOUT.contains(&key) || key == "metricOrder",
        "providers" => ["enabled", "known"].contains(&key),
        "notifications" => {
            ["underTenPercent", "healthyToClose", "closeToRunningOut"].contains(&key)
        }
        "totalSpend" => ["show", "period", "metric"].contains(&key),
        _ => false,
    }
}
fn put_back(list: &Value, previous: &Value, known: &[Value]) -> Value {
    let mut next = list.as_array().cloned().unwrap_or_default();
    if let Some(previous) = previous.as_array() {
        for (i, item) in previous.iter().enumerate() {
            if known.contains(item) || next.contains(item) {
                continue;
            }
            let index = previous[..i]
                .iter()
                .rev()
                .find_map(|p| next.iter().position(|v| v == p))
                .map_or(0, |i| i + 1);
            next.insert(index, item.clone());
        }
    }
    json!(next)
}
fn preserving(name: &str, mut value: Value, previous: &Value, knows: &Value) -> Value {
    let (Some(fields), Some(old)) = (value.as_object_mut(), previous.as_object()) else {
        return value;
    };
    for (key, item) in old {
        if !known_fields(name, key) && !fields.contains_key(key) {
            fields.insert(key.clone(), item.clone());
        }
    }
    if let (Some(cards), Some(meters)) = (knows["cards"].as_array(), knows["meters"].as_array()) {
        let keys: &[&str] = if name == "layout" {
            LAYOUT
        } else if name == "providers" {
            &["enabled", "known"]
        } else {
            &[]
        };
        for key in keys {
            let known = if name == "providers" || *key == "providerOrder" {
                cards
            } else {
                meters
            };
            fields.insert(
                (*key).into(),
                put_back(
                    fields.get(*key).unwrap_or(&Value::Null),
                    &previous[*key],
                    known,
                ),
            );
        }
        if name == "layout" {
            if let (Some(order), Some(was)) = (
                fields.get_mut("metricOrder").and_then(Value::as_object_mut),
                previous["metricOrder"].as_object(),
            ) {
                for (card, items) in was {
                    let next = if !cards.contains(&json!(card)) && !order.contains_key(card) {
                        items.clone()
                    } else {
                        put_back(order.get(card).unwrap_or(&Value::Null), items, meters)
                    };
                    order.insert(card.clone(), next);
                }
            }
        }
    }
    value
}

pub fn notification_availability() -> Value {
    json!([{"provider":"devin","available":false,"message":"Devin alerts are unavailable because Devin is not connected to Kuvo Code’s background service."}])
}

#[derive(Clone)]
pub struct Service {
    file: PathBuf,
    setup: kuvo_setup::SetupHandle,
    unnamed: String,
    gate: Arc<Mutex<()>>,
}
impl Service {
    pub fn new(home: PathBuf, setup: kuvo_setup::SetupHandle) -> Self {
        let mut bytes = [0; 8];
        let _ = getrandom::fill(&mut bytes);
        Self {
            file: home.join("agents/usage/settings.json"),
            setup,
            unnamed: format!("?{}", hex::encode(bytes)),
            gate: Arc::default(),
        }
    }
    pub fn codex_fallback(&self) -> (Option<String>, bool) {
        let shared = self.shared();
        self.read(shared.as_ref())
            .ok()
            .and_then(|settings| {
                settings
                    .units
                    .get("codexFallback")
                    .map(|unit| (unit.value.as_str().map(str::to_owned), true))
            })
            .unwrap_or_default()
    }
    pub async fn set_codex_fallback(&self, value: Value) -> Result<Value, Refusal> {
        self.route(Some(json!({"codexFallback":value})), false)
            .await
    }
    fn shared(&self) -> Option<kuvo_setup::UsageSetup> {
        self.setup.usage_setup()
    }
    fn disk(&self) -> Result<Option<Value>, Refusal> {
        let text = match std::fs::read_to_string(&self.file) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(internal(e)),
        };
        let value: Value = serde_json::from_str(&text).map_err(internal)?;
        if value["kuvo"].as_u64().is_some_and(|v| v > 1) {
            return Err(Refusal::new(
                StatusCode::CONFLICT,
                "Usage settings format is unsupported.",
            ));
        }
        Ok(Some(value))
    }
    fn read(&self, shared: Option<&kuvo_setup::UsageSetup>) -> Result<Settings, Refusal> {
        if let Some(shared) = shared.filter(|s| s.sequence != 0) {
            return Ok(Settings::from_saved(&shared.value, usize::MAX));
        }
        let Some(value) = self.disk()? else {
            return Ok(Settings::default());
        };
        if let Some(shared) = shared
            && let Some(previous) = value.get("setup").filter(|v| v.is_null() || v.is_string())
            && previous.as_str() != Some(shared.id.as_str())
        {
            return Ok(Settings::default());
        }
        let mut settings = Settings::from_saved(&value, 20);
        if !settings.units.contains_key("codexFallback")
            && let Some(fallback) = value["codex"].get("fallback")
        {
            let fallback = if fallback == "" {
                Value::Null
            } else {
                fallback.clone()
            };
            if let Some(value) = checked("codexFallback", &fallback) {
                let v = std::fs::metadata(&self.file)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_millis() as u64);
                settings.units.insert(
                    "codexFallback".into(),
                    Unit {
                        value,
                        stamp: Stamp {
                            v,
                            by: String::new(),
                            imported: true,
                        },
                    },
                );
            }
        }
        Ok(settings)
    }
    async fn save(
        &self,
        settings: &Settings,
        shared: Option<kuvo_setup::UsageSetup>,
    ) -> Result<Option<Value>, Refusal> {
        if let Some(shared) = shared {
            return self
                .setup
                .save_usage(shared.id, shared.epoch, shared.sequence, json!(settings))
                .await
                .map(Some);
        }
        let mut data = json!(settings);
        data["kuvo"] = json!(1);
        // A missing or not-yet-known setup is not a confirmed detachment.
        // Preserve the original file's marker; adopt an unmarked file only
        // when we know there is no setup, or through an accepted shared save.
        let marker = self
            .disk()?
            .and_then(|old| {
                old.get("setup")
                    .filter(|v| v.is_null() || v.is_string())
                    .cloned()
            })
            .or_else(|| self.setup.link_id().is_some().then_some(Value::Null));
        if let Some(marker) = marker {
            data["setup"] = marker;
        }
        if let Some(unit) = settings.units.get("codexFallback") {
            data["codex"] = json!({"fallback":unit.value});
        }
        kuvo_store::files::write_secret(&self.file, &serde_json::to_vec(&data).map_err(internal)?)
            .map_err(internal)?;
        Ok(None)
    }
    async fn route(&self, body: Option<Value>, import: bool) -> Result<Value, Refusal> {
        let _gate = self.gate.lock().await;
        let shared = self.shared();
        let mut settings = self.read(shared.as_ref())?;
        let mut extra = Map::new();
        if let Some(body) = body {
            if shared.as_ref().is_some_and(|s| !s.primary) {
                return Err(Refusal::new(
                    StatusCode::CONFLICT,
                    "Change your setup's usage settings on its main computer.",
                ));
            }
            let parts = body.as_object().ok_or_else(|| {
                bad(if import {
                    "Say what the settings were."
                } else {
                    "Say which settings change, each whole."
                })
            })?;
            let mut changes = vec![];
            if !import || !settings.imported {
                for (name, value) in parts {
                    if ["based", "knows"].contains(&name.as_str()) && !import {
                        continue;
                    }
                    if import && settings.units.contains_key(name) {
                        continue;
                    }
                    if !PARTS.contains(&name.as_str()) {
                        if import {
                            continue;
                        }
                        return Err(bad(&format!("There's no usage setting called {name}.")));
                    }
                    if name == "codexFallback"
                        && !value.is_null()
                        && !fallback_allowed(
                            value,
                            self.file.parent().map(|p| p.join("pricing")).as_deref(),
                        )
                    {
                        if import {
                            continue;
                        }
                        return Err(bad(
                            "This model's prices aren't available now. Choose another model or None.",
                        ));
                    }
                    let Some(value) = checked(name, value) else {
                        if import {
                            continue;
                        }
                        return Err(bad(&format!("That isn't a {name} setting.")));
                    };
                    changes.push((name, value));
                }
            }
            let mut stale = vec![];
            let mut changed = vec![];
            let now = jiff::Timestamp::now().as_millisecond().max(0) as u64;
            let mut highest = settings
                .units
                .values()
                .filter(|u| !u.stamp.imported)
                .map(|u| u.stamp.v)
                .max()
                .unwrap_or(0);
            let by = self.setup.link_id().unwrap_or_else(|| self.unnamed.clone());
            for (name, mut value) in changes {
                if let Some(previous) = settings.units.get(name) {
                    if let Some(based) = body["based"].get(name)
                        && *based != json!(previous.stamp.version())
                    {
                        stale.push(name);
                        continue;
                    }
                    value = preserving(name, value, &previous.value, &body["knows"]);
                    if !previous.stamp.imported && previous.value == value {
                        continue;
                    }
                } else if body["based"].get(name).is_some_and(|v| !v.is_null()) {
                    stale.push(name);
                    continue;
                }
                highest = now.max(highest.saturating_add(1));
                settings.units.insert(
                    name.clone(),
                    Unit {
                        value,
                        stamp: Stamp {
                            v: if import { now } else { highest },
                            by: by.clone(),
                            imported: import,
                        },
                    },
                );
                changed.push(name);
            }
            let save = !changed.is_empty() || import && !settings.imported;
            if import {
                settings.imported = true;
                extra.insert("took".into(), json!(changed));
            }
            if !stale.is_empty() {
                extra.insert("stale".into(), json!(stale));
            }
            if save && let Some(receipt) = self.save(&settings, shared).await? {
                extra.insert("save".into(), receipt);
            }
        }
        extra.insert(
            "settings".into(),
            json!(
                settings
                    .units
                    .iter()
                    .filter(|(n, _)| PARTS.contains(&n.as_str()))
                    .map(|(n, u)| (n, &u.value))
                    .collect::<BTreeMap<_, _>>()
            ),
        );
        extra.insert(
            "versions".into(),
            json!(
                settings
                    .units
                    .iter()
                    .filter(|(n, _)| PARTS.contains(&n.as_str()))
                    .map(|(n, u)| (n, u.stamp.version()))
                    .collect::<BTreeMap<_, _>>()
            ),
        );
        extra.insert("imported".into(), json!(settings.imported));
        extra.insert(
            "notificationAvailability".into(),
            notification_availability(),
        );
        Ok(json!({"data":extra}))
    }
}
fn fallback_allowed(value: &Value, folder: Option<&std::path::Path>) -> bool {
    let Some(model) = value.as_str() else {
        return false;
    };
    super::pricing::Pricing::load(folder)
        .ok()
        .is_some_and(|pricing| pricing.fallback_rates(model, "codex").is_some())
}

fn bad(message: &str) -> Refusal {
    Refusal::new(StatusCode::BAD_REQUEST, message)
}
fn internal(error: impl std::fmt::Display) -> Refusal {
    Refusal::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("Usage settings: {error}"),
    )
}

macro_rules! spec {
    ($name:ident,$method:literal,$path:literal,$body:ty,$shape:expr) => {
        struct $name;
        impl RouteSpec for $name {
            type Body = $body;
            const ROUTE: &'static Route = &Route {
                method: $method,
                path: $path,
                feature: "usage.settings",
                answers: &[(200, &Shape::Primitive(Primitive::Any))],
                body: $shape,
            };
        }
        impl JsonStatus<200> for $name {
            type Body = Value;
        }
    };
}
spec!(Get, "GET", "kuvo/agents/api/kuvo/usage/settings", (), None);
spec!(
    Put,
    "PUT",
    "kuvo/agents/api/kuvo/usage/settings",
    Value,
    Some(&Shape::Primitive(Primitive::Any))
);
spec!(
    Import,
    "POST",
    "kuvo/agents/api/kuvo/usage/settings/import",
    Value,
    Some(&Shape::Primitive(Primitive::Any))
);
pub fn register(routes: &mut Registry, service: Service) {
    let read = service.clone();
    routes.register::<Get, _, _>(Access::Any, move |_| {
        let service = read.clone();
        async move {
            service
                .route(None, false)
                .await
                .map(RouteReply::json::<200>)
        }
    });
    let put = service.clone();
    routes.register::<Put, _, _>(Access::Any, move |call| {
        let service = put.clone();
        async move {
            service
                .route(Some(call.body), false)
                .await
                .map(RouteReply::json::<200>)
        }
    });
    routes.register::<Import, _, _>(Access::Any, move |call| {
        let service = service.clone();
        async move {
            service
                .route(Some(call.body), true)
                .await
                .map(RouteReply::json::<200>)
        }
    });
}

impl Service {
    /// None means the metric is not visible. Some(empty) means it is visible
    /// but its alerts are off; that must not consume an unannounced crossing.
    pub fn notification_milestones(
        &self,
        service: &str,
        notice_card: Option<&str>,
        window: &super::readings::Window,
    ) -> Option<std::collections::BTreeSet<super::readings::Milestone>> {
        use super::readings::Milestone::*;
        let Ok(settings) = self.read(self.shared().as_ref()) else {
            return Default::default();
        };
        let metric = metric_name(service, window);
        let exact = notice_card.filter(|candidate| {
            let in_list = |value: &Value| {
                value
                    .as_array()
                    .is_some_and(|items| items.iter().any(|item| item.as_str() == Some(*candidate)))
            };
            settings
                .units
                .get("providers")
                .is_some_and(|p| in_list(&p.value["known"]) || in_list(&p.value["enabled"]))
                || settings.units.get("layout").is_some_and(|layout| {
                    let v = &layout.value;
                    in_list(&v["providerOrder"])
                        || v["metricOrder"]
                            .as_object()
                            .is_some_and(|m| m.contains_key(*candidate))
                        || ["placed", "seeded"].into_iter().any(|key| {
                            v[key]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(Value::as_str)
                                .any(|id| {
                                    id.split_once('.')
                                        .is_some_and(|(card, _)| card == *candidate)
                                })
                        })
                })
        });
        let enabled = |card: &str| {
            settings.units.get("providers").is_none_or(|p| {
                !p.value["known"]
                    .as_array()
                    .is_some_and(|l| l.contains(&json!(card)))
                    || p.value["enabled"]
                        .as_array()
                        .is_some_and(|l| l.contains(&json!(card)))
            })
        };
        let visible = if let Some(layout) = settings.units.get("layout") {
            layout.value["placed"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .any(|id| {
                    id.split_once('.').is_some_and(|(card, m)| {
                        exact.map_or_else(
                            || card.split('@').next() == Some(service),
                            |wanted| card == wanted,
                        ) && m == metric
                            && enabled(card)
                    })
                })
        } else {
            let defaults = [
                "antigravity.geminiPro",
                "antigravity.geminiWeekly",
                "antigravity.argonWeekly",
                "antigravity.claude",
                "antigravity.claudeWeekly",
                "claude.session",
                "claude.weekly",
                "claude.fable",
                "claude.extra",
                "codex.session",
                "codex.weekly",
                "codex.spark",
                "codex.sparkWeekly",
                "cursor.usage",
                "cursor.auto",
                "cursor.api",
                "cursor.grokBot",
                "cursor.onDemand",
                "copilot.premium",
                "copilot.chat",
                "copilot.completions",
                "grok.weekly",
                "ollama.session",
                "ollama.weekly",
                "ollama.monthly",
                "opencode.session",
                "opencode.weekly",
                "opencode.monthly",
                "openrouter.credits",
                "openrouter.keyLimit",
                "zai.session",
                "zai.weekly",
                "zai.webSearches",
            ];
            enabled(exact.unwrap_or(service))
                && defaults.contains(&format!("{service}.{metric}").as_str())
        };
        if !visible {
            return Default::default();
        }
        let unit = settings.units.get("notifications");
        Some(
            [
                ("underTenPercent", UnderTenPercent),
                ("healthyToClose", HealthyToClose),
                ("closeToRunningOut", CloseToRunningOut),
            ]
            .into_iter()
            .filter_map(|(key, m)| {
                unit.is_some_and(|unit| unit.value[key] == true)
                    .then_some(m)
            })
            .collect(),
        )
    }
}

fn metric_name<'a>(service: &str, window: &'a super::readings::Window) -> &'a str {
    let spark = service == "codex"
        && window
            .model
            .as_deref()
            .unwrap_or(&window.id)
            .to_ascii_lowercase()
            .contains("spark");
    match (service, window.id.as_str()) {
        ("codex", id) if spark => {
            if id.ends_with("secondary") {
                "sparkWeekly"
            } else {
                "spark"
            }
        }
        ("claude", "five_hour") | ("codex", "primary") | ("opencode", "rolling") => "session",
        ("claude", "seven_day") | ("codex", "secondary") => "weekly",
        ("claude", "seven_day_fable") => "fable",
        ("claude", "seven_day_opus") => "opus",
        ("claude", "seven_day_sonnet") => "sonnet",
        ("antigravity", "gemini-5h") => "geminiPro",
        ("antigravity", "gemini-weekly") => "geminiWeekly",
        ("antigravity", "argon-weekly") => "argonWeekly",
        ("antigravity", "3p-5h") => "claude",
        ("antigravity", "3p-weekly") => "claudeWeekly",
        ("cursor", "total" | "included") => "usage",
        ("cursor", "on_demand") => "onDemand",
        ("cursor", "grok_bot") => "grokBot",
        ("copilot", "premium_interactions") => "premium",
        ("openrouter", "key") => "keyLimit",
        ("zai", "web_search") => "webSearches",
        ("zai", "window_10080") => "weekly",
        _ => &window.id,
    }
}

#[cfg(test)]
mod notice_card_tests {
    use super::*;
    use crate::usage::readings::{Source, Window};

    #[test]
    fn window_aliases_name_the_configured_meter() {
        for (service, id, model, expected) in [
            ("codex", "primary", Some("GPT-5.3-Codex-Spark"), "spark"),
            (
                "codex",
                "secondary",
                Some("GPT-5.3-Codex-Spark"),
                "sparkWeekly",
            ),
            ("antigravity", "gemini-5h", None, "geminiPro"),
            ("antigravity", "gemini-weekly", None, "geminiWeekly"),
            ("antigravity", "argon-weekly", None, "argonWeekly"),
            ("antigravity", "3p-5h", None, "claude"),
            ("antigravity", "3p-weekly", None, "claudeWeekly"),
        ] {
            let window = Window {
                id: id.into(),
                model: model.map(str::to_owned),
                name: None,
                used: 0.5,
                minutes: None,
                resets_at: None,
                amount: None,
                read: 1,
                source: Source::Usage,
            };
            assert_eq!(metric_name(service, &window), expected);
        }
    }
}
