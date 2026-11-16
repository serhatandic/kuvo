//! A fixed Custom Auto page from the real service's test, for generic rendering and shots.
use serde_json::{Value, json};

pub fn section() -> Value {
    json!({"id":"custom-auto","title":"Custom Auto","group":"Models","icon":"sliders","line":"Your lists of models to go through in order when one reaches a usage limit.","keywords":["auto","models","fallback"]})
}

pub fn page() -> Value {
    serde_json::from_str(include_str!("custom-auto.json")).expect("Custom Auto fixture")
}
