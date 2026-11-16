//! The stand-in's Vault (`kuvo/settings/vault`), as a box describes it (PROTOCOL.md, "A box's
//! settings", Vault): fixed pages the real box answered (`vault.json`, made by
//! `box/test/make-vault-fixture.ts` from the box's own code, for a machine named lokum, as your other
//! devices read it), the checks its Add Item form answered, and what's sent kept for tests. Show's
//! values (`kuvo/vault/items/<id>/values`) are made up here, never in the fixture: each read is kept
//! (`shown`), as the vault notes one. No vault logic is copied: an action changes only what a test
//! looks for (an answered ask goes, a renamed item shows its name, a deleted one goes).

use serde_json::{Value, json};

use super::Settings;

/// What the real box answered: the section's line in `kuvo/settings`, its page, the page locked,
/// its form's checks, and its items' titles by row.
const FIXTURE: &str = include_str!("vault.json");

#[derive(Clone, Debug)]
pub struct Vault {
    /// Its section is listed (tests that don't look at it leave it out).
    pub on: bool,
    /// Its vault is locked, as after its computer restarted.
    pub locked: bool,
    page: Value,
    /// What was sent, in order: the row, the action, its values.
    pub sent: Vec<(String, String, Value)>,
    /// The values route read, in order: each item's id (Show).
    pub shown: Vec<String>,
    /// The values `…/check` was sent, in order.
    pub checked: Vec<Value>,
}

impl Default for Vault {
    fn default() -> Vault {
        Vault { on: false, locked: false, page: fixture()["page"].clone(), sent: Vec::new(), shown: Vec::new(), checked: Vec::new() }
    }
}

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).expect("the Vault fixture")
}

/// The made-up value an item's field shows here (never a real one, never in the fixture).
pub fn made_up(item: &str, field: &str) -> String {
    format!("stand-in-{field}-{}", item.trim_start_matches("v_"))
}

fn refused(status: u16, message: &str) -> (u16, Value) {
    (status, json!({ "message": message }))
}

fn rows_mut(page: &mut Value, change: &mut dyn FnMut(&mut Value)) {
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

fn changed(row: &mut Value) {
    let revision = row["revision"].as_str().unwrap_or_default().to_owned();
    row["revision"] = json!(format!("{revision}~"));
}

impl Vault {
    pub fn section() -> Value {
        fixture()["section"].clone()
    }

    pub fn page(&self) -> Value {
        if self.locked { fixture()["locked"].clone() } else { self.page.clone() }
    }

    /// Takes a row out wherever it is (an answered ask, a deleted item), and a group left empty.
    fn take_out(&mut self, id: &str) {
        for group in self.page.get_mut("groups").and_then(Value::as_array_mut).into_iter().flatten() {
            if let Some(rows) = group.get_mut("rows").and_then(Value::as_array_mut) {
                rows.retain(|row| row["id"] != json!(id));
                for row in rows.iter_mut() {
                    if let Some(items) = row.get_mut("items").and_then(Value::as_array_mut) {
                        let before = items.len();
                        items.retain(|item| item["id"] != json!(id));
                        if items.len() != before {
                            changed(row);
                        }
                    }
                }
            }
        }
        if let Some(groups) = self.page.get_mut("groups").and_then(Value::as_array_mut) {
            groups.retain(|group| group["rows"].as_array().is_some_and(|rows| !rows.is_empty()));
        }
    }
}

impl Settings {
    pub(super) fn vault_answer(&mut self, method: &str, segments: &[&str], body: &Value) -> Option<(u16, Value)> {
        if !self.vault.on {
            return None;
        }
        if let ("GET", ["kuvo", "vault", "items", id, "values"]) = (method, segments) {
            if self.vault.locked {
                return Some(refused(423, "This computer's vault is locked since it started again."));
            }
            let id = id.to_string();
            let fields = ["password", "value", "text", "issuer_id"];
            self.vault.shown.push(id.clone());
            return Some((200, json!({ "values": fields.iter().map(|field| (field.to_string(), json!(made_up(&id, field)))).collect::<serde_json::Map<_, _>>() })));
        }
        let ["kuvo", "settings", "vault", tail @ ..] = segments else { return None };
        let page = self.vault.page();
        Some(match (method, tail) {
            ("GET", []) | ("GET", ["events"]) => (200, page),
            ("PATCH", []) => {
                let Some(id) = body["row"].as_str() else { return Some(refused(400, "Name the row to change (row).")) };
                let Some(at) = find(&page, id) else { return Some(refused(404, "That isn’t on this page now.")) };
                if at["revision"] != body["was"] {
                    return Some((409, json!({ "message": "It changed since you read it.", "page": page })));
                }
                let Some(on) = body["value"].as_bool() else { return Some(refused(400, "Send true or false.")) };
                rows_mut(&mut self.vault.page, &mut |row| {
                    if row["id"] == json!(id) {
                        row["value"] = json!(on);
                        changed(row);
                    }
                });
                (200, self.vault.page())
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
                    self.vault.checked.push(body["values"].clone());
                    let checks = fixture()["checks"].clone();
                    let values = &body["values"];
                    // (The real box's words for what's typed: nothing yet, a value too short, or ready.)
                    let key = if values["name"].as_str().is_none_or(str::is_empty) { "items/add" } else if values["password"].as_str().is_some_and(|typed| typed.chars().count() < 4) { "items/add/short" } else { "items/add/ready" };
                    return Some((200, if *row == "items" { checks[key].clone() } else { json!({ "fields": [], "ready": true }) }));
                }
                self.vault.sent.push((row.to_string(), action.to_string(), body["values"].clone()));
                let row = row.to_string();
                match *action {
                    "delete" | "no" | "once" | "always" | "give" | "go-on" => self.vault.take_out(&row),
                    "edit" => {
                        let name = body["values"]["name"].clone();
                        rows_mut(&mut self.vault.page, &mut |each| {
                            if each["id"] == json!(row) && name.is_string() {
                                each["title"] = name.clone();
                                changed(each);
                            }
                        });
                    }
                    _ => {}
                }
                (200, self.vault.page())
            }
            _ => return None,
        })
    }
}
