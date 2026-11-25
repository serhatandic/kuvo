//! Shared settings revisions, form safety and validation.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::config::sorted_json;

fn hash(value: &Value) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(sorted_json(value)))[..16].to_owned()
}

fn fields_of(value: &Value, names: &[&str]) -> Map<String, Value> {
    names
        .iter()
        .filter_map(|name| value.get(*name).map(|v| (name.to_string(), v.clone())))
        .collect()
}

fn governed_can(fields: &mut Map<String, Value>) {
    if let Some(can) = fields.get_mut("can") {
        *can = json!(*can == true);
    }
}

fn safe_field(field: &mut Value) {
    let Some(field) = field.as_object_mut() else {
        return;
    };
    let kind = field
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let value = field.shift_remove("value");
    let shows = field.shift_remove("shows");
    match kind.as_str() {
        "secret" => {
            if let Some(shows) = shows
                && shows["route"].as_str().is_some_and(|route| {
                    route
                        .strip_prefix("kuvo/vault/items/v_")
                        .and_then(|s| s.strip_suffix("/values"))
                        .is_some_and(|id| {
                            id.len() == 10
                                && id
                                    .bytes()
                                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                        })
                })
            {
                field.insert("shows".into(), shows);
            }
        }
        "pairs" if field.get("secret") == Some(&Value::Bool(true)) => {
            let pairs = value
                .as_ref()
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|pair| json!({"key":pair["key"].as_str().unwrap_or_default(), "value":""}))
                .collect::<Vec<_>>();
            field.insert("value".into(), json!(pairs));
        }
        "file" => {
            if let Some(name) = value
                .as_ref()
                .and_then(|v| v["name"].as_str())
                .filter(|s| !s.is_empty())
            {
                field.insert("value".into(), json!({"name":name}));
            }
        }
        _ => {
            if let Some(value) = value {
                field.insert("value".into(), value);
            }
        }
    }
}

fn governed_fields(fields: &[Value]) -> Value {
    json!(
        fields
            .iter()
            .map(|field| {
                let mut fields = fields_of(field, &["id", "kind", "value", "can"]);
                governed_can(&mut fields);
                fields
            })
            .collect::<Vec<_>>()
    )
}

fn action(action: &mut Value) -> Value {
    let mut governs = fields_of(action, &["id", "can", "opens", "confirm"]);
    governed_can(&mut governs);
    if let Some(fields) = action
        .get_mut("form")
        .and_then(|f| f.get_mut("fields"))
        .and_then(Value::as_array_mut)
    {
        for field in fields.iter_mut() {
            safe_field(field);
        }
        let fields = governed_fields(fields);
        action["form"]["revision"] = json!(hash(&fields));
        governs.insert("form".into(), fields);
    }
    Value::Object(governs)
}

/// A row's revision depends only on what governs its changes, not its display words.
pub fn row(mut row: Value) -> Value {
    let mut governs = fields_of(
        &row,
        &[
            "confirmBelow",
            "min",
            "max",
            "kind",
            "value",
            "can",
            "set",
            "section",
            "address",
            "name",
        ],
    );
    governed_can(&mut governs);
    if row["kind"] == "secret" {
        governs.shift_remove("value");
        row.as_object_mut().unwrap().shift_remove("value");
    }
    if let Some(choices) = row["choices"].as_array() {
        governs.insert(
            "choices".into(),
            json!(choices.iter().map(|v| &v["id"]).collect::<Vec<_>>()),
        );
    }
    if let Some(items) = row.get_mut("items").and_then(Value::as_array_mut) {
        for item in items.iter_mut() {
            *item = self::row(std::mem::take(item));
        }
        governs.insert(
            "items".into(),
            json!(items.iter().map(|v| &v["revision"]).collect::<Vec<_>>()),
        );
    }
    for key in ["actions", "start", "add"] {
        if let Some(at) = row.get_mut(key) {
            governs.insert(
                key.into(),
                if key == "actions" {
                    json!(
                        at.as_array_mut()
                            .into_iter()
                            .flatten()
                            .map(action)
                            .collect::<Vec<_>>()
                    )
                } else {
                    action(at)
                },
            );
        }
    }
    if let Some(flow) = row.get_mut("flow") {
        governs.insert(
            "flow".into(),
            Value::Object(fields_of(flow, &["id", "state"])),
        );
        if let Some(actions) = flow.get_mut("actions").and_then(Value::as_array_mut) {
            for each in actions {
                action(each);
            }
        }
    }
    row["revision"] = json!(hash(&Value::Object(governs)));
    row
}

pub fn page(id: &str, title: &str, groups: Vec<Value>) -> Value {
    json!({"id":id,"title":title,"groups":groups})
}

pub fn unread(id: &str, title: &str, words: impl Into<String>) -> Value {
    json!({"id":id,"title":title,"groups":[],"notes":[{"words":words.into(),"tone":"warning"}]})
}

pub fn group(id: &str, rows: Vec<Value>) -> Value {
    json!({"id":id,"rows":rows.into_iter().map(row).collect::<Vec<_>>()})
}

pub fn find_row<'a>(page: &'a Value, id: &str) -> Option<&'a Value> {
    page["groups"]
        .as_array()?
        .iter()
        .flat_map(|g| g["rows"].as_array().into_iter().flatten())
        .flat_map(|r| std::iter::once(r).chain(r["items"].as_array().into_iter().flatten()))
        .find(|r| r["id"] == id)
}

pub fn find_action<'a>(row: &'a Value, id: &str) -> Option<&'a Value> {
    row["actions"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(row.get("add"))
        .chain(row["flow"]["actions"].as_array().into_iter().flatten())
        .find(|a| a["id"] == id)
}

pub fn revision<'a>(row: &'a Value, action: Option<&'a Value>) -> &'a Value {
    action
        .and_then(|a| a["form"].get("revision"))
        .unwrap_or(&row["revision"])
}

pub fn why_not(value: &Value) -> Option<&str> {
    value["can"]["why"].as_str()
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Kind {
    Number,
    Folder,
    Text,
    Switch,
    Choice,
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct Choice {
    id: String,
}

#[derive(Deserialize)]
pub struct Rule {
    kind: Kind,
    min: Option<f64>,
    max: Option<f64>,
    #[serde(default)]
    choices: Vec<Choice>,
}

impl Rule {
    pub fn fits(&self, value: &Value) -> Result<(), &'static str> {
        let (fits, words) = match self.kind {
            Kind::Number => (
                value.as_f64().is_some_and(|n| {
                    self.min.is_none_or(|min| n >= min) && self.max.is_none_or(|max| n <= max)
                }),
                "Send a number within its limits.",
            ),
            Kind::Folder | Kind::Text => (value.is_string(), "Send text."),
            Kind::Switch => (value.is_boolean(), "Send true or false."),
            Kind::Choice => (
                self.choices.iter().any(|c| value == &c.id),
                "Pick one of its choices.",
            ),
            Kind::Other => (false, "Send true or false."),
        };
        if fits { Ok(()) } else { Err(words) }
    }
}
