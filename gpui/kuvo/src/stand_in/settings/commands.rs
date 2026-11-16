//! Fixed Commands descriptions for generic Settings tests and shots.
use serde_json::{Value, json};

pub fn section() -> Value {
    json!({"id":"commands","title":"Commands","group":"Agents & Tools","icon":"terminal","line":"Prompts you use often, run by typing / and their name.","keywords":["commands","prompt","slash"]})
}

pub fn page() -> Value {
    serde_json::from_str(include_str!("commands.json")).expect("Commands fixture")
}
