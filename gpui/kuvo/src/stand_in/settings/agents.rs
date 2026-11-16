//! The stand-in's Agents (`kuvo/settings/agents`), as a box describes it (PROTOCOL.md, "A box's
//! settings", Agents): a fixed page the real box answered (`agents.json`, made by its own code for a
//! Mac's box beside the app, with sample agents and Claude Code's), the checks its forms answered,
//! and what's sent kept for tests. No agent logic is copied here: a switch flips its row, Delete takes
//! its row out, Import marks it imported, and a form's values are kept as they're sent.

use serde_json::{Value, json};

use super::Settings;

/// What the real box answered: the section's line in `kuvo/settings`, its page, and its forms' checks.
const FIXTURE: &str = include_str!("agents.json");

#[derive(Clone, Debug)]
pub struct Agents {
    /// Its section is listed (tests that don't look at it leave it out).
    pub on: bool,
    page: Value,
    /// What was sent, in order: the row, the action, its values.
    pub sent: Vec<(String, String, Value)>,
}

impl Default for Agents {
    fn default() -> Agents {
        let fixture: Value = serde_json::from_str(FIXTURE).expect("the Agents fixture");
        Agents { on: false, page: fixture["page"].clone(), sent: Vec::new() }
    }
}

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).expect("the Agents fixture")
}

fn refused(status: u16, message: &str) -> (u16, Value) {
    (status, json!({ "message": message }))
}

/// Each row on the page, a list's items too, changed in place by `change`.
fn each_row(page: &mut Value, change: &mut dyn FnMut(&mut Value)) {
    // (`get_mut`, never `[…]`, which would add the key to a row without it.)
    for group in page.get_mut("groups").and_then(Value::as_array_mut).into_iter().flatten() {
        for row in group.get_mut("rows").and_then(Value::as_array_mut).into_iter().flatten() {
            for item in row.get_mut("items").and_then(Value::as_array_mut).into_iter().flatten() {
                change(item);
            }
            change(row);
        }
    }
}

fn find(page: &Value, id: &str) -> Option<Value> {
    page["groups"].as_array()?.iter().flat_map(|group| group["rows"].as_array().into_iter().flatten()).flat_map(|row| std::iter::once(row).chain(row["items"].as_array().into_iter().flatten())).find(|row| row["id"] == json!(id)).cloned()
}

/// A row's revision once it changed.
fn changed(row: &mut Value) {
    let revision = row["revision"].as_str().unwrap_or_default().to_owned();
    row["revision"] = json!(format!("{revision}~"));
}

impl Agents {
    pub fn section() -> Value {
        fixture()["section"].clone()
    }

    pub fn page(&self) -> Value {
        self.page.clone()
    }
}

impl Settings {
    pub(super) fn agents_answer(&mut self, method: &str, rest: &[&str], body: &Value) -> Option<(u16, Value)> {
        let ["agents", tail @ ..] = rest else { return None };
        if !self.agent_settings.on {
            return None;
        }
        let page = self.agent_settings.page();
        Some(match (method, tail) {
            ("GET", []) | ("GET", ["events"]) => (200, page),
            ("PATCH", []) => {
                let Some(id) = body["row"].as_str() else { return Some(refused(400, "Name the row to change (row).")) };
                let Some(at) = find(&page, id) else { return Some(refused(404, "That isn’t on this page now.")) };
                if at["revision"] != body["was"] {
                    return Some((409, json!({ "message": "It changed since you read it.", "page": page })));
                }
                let Some(on) = body["value"].as_bool() else { return Some(refused(400, "Send true or false.")) };
                each_row(&mut self.agent_settings.page, &mut |row| {
                    if row["id"] == json!(id) {
                        row["value"] = json!(on);
                        changed(row);
                    }
                });
                (200, self.agent_settings.page())
            }
            ("POST", [row, action, rest @ ..]) => {
                let Some(at) = find(&page, row) else { return Some(refused(404, "That isn’t on this page now.")) };
                let offered = at["actions"].as_array().into_iter().flatten().chain(at.get("add")).find(|each| each["id"] == json!(action)).cloned();
                let Some(offered) = offered else { return Some(refused(404, "That isn't something it does.")) };
                let was = offered["form"]["revision"].as_str().map(|form| json!(form)).unwrap_or_else(|| at["revision"].clone());
                if was != body["was"] {
                    return Some((409, json!({ "message": "It changed since you read it.", "page": page })));
                }
                if rest == ["check"] {
                    let checked = fixture()["checks"][format!("{row}/{action}")].clone();
                    return Some((200, if checked.is_null() { json!({ "fields": [], "ready": true }) } else { checked }));
                }
                self.agent_settings.sent.push((row.to_string(), action.to_string(), body["values"].clone()));
                match *action {
                    "delete" => {
                        each_row(&mut self.agent_settings.page, &mut |list| {
                            if let Some(items) = list.get_mut("items").and_then(Value::as_array_mut) {
                                items.retain(|item| item["id"] != json!(row));
                                changed(list);
                            }
                        });
                    }
                    "import" => {
                        each_row(&mut self.agent_settings.page, &mut |each| {
                            if each["id"] == json!(row) {
                                each["kind"] = json!("status");
                                each["value"] = json!("Imported");
                                if let Some(fields) = each.as_object_mut() {
                                    fields.remove("actions");
                                }
                                changed(each);
                            }
                        });
                    }
                    _ => {}
                }
                (200, self.agent_settings.page())
            }
            _ => return None,
        })
    }
}
