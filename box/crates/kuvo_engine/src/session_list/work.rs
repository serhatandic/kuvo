//! The work fields kept beside a session's row (`work`): what a bot's work or a subagent ran on and
//! how it went, and for a bot's own work its starter, depth, mode, tokens, cost and the asks it
//! answered. Read from the session as the list already has it, so a bot's work is read from the
//! list rather than asking each session again.

use serde_json::{Value, json};

use crate::sessions::Session;

fn text(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

/// A session's work fields; none for one that isn't a bot's work or a subagent.
pub(super) fn listed_work(session: &Session) -> Option<Value> {
    let note = |key: &str| session.notes().and_then(|notes| notes.get(key));
    let truthy = |value: &Value| {
        !matches!(value, Value::Null | Value::Bool(false)) && value != "" && value != 0
    };
    let parent = session
        .parent
        .as_deref()
        .is_some_and(|parent| !parent.is_empty());
    let starter = note("startedBy")
        .and_then(Value::as_str)
        .filter(|by| !by.is_empty());
    let by = note("by").filter(|by| by["name"].is_string() && !parent && starter.is_some());
    let bot = by.and_then(|by| by["bot"].as_str());
    let hidden = ["engineFor", "seat", "reviewPart"]
        .iter()
        .any(|mark| note(mark).is_some_and(truthy));
    if bot.is_none() && !(parent && !hidden) {
        return None;
    }
    let data = &session.extra;
    let model = &data.get("model").cloned().unwrap_or_default();
    let mut row = json!({ "idle": session.time.idle, "outcome": data.get("outcome") });
    if let (Some(id), Some(provider)) = (text(model.get("id")), text(model.get("providerID"))) {
        row["model"] = json!({"id": id, "providerID": provider});
        if let Some(variant) = text(model.get("variant")) {
            row["model"]["variant"] = json!(variant);
        }
    }
    for key in ["ranOn", "requestedModel"].iter().filter(|_| parent) {
        row[*key] = json!(text(note(key)));
    }
    if let (Some(bot), Some(starter)) = (bot, starter) {
        let tokens = data.get("tokens").cloned().unwrap_or_default();
        let count = |key: &str| tokens[key].as_f64().unwrap_or(0.0);
        let answered: Vec<&Value> = note("answers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|each| each["ask"].is_string() && each["decision"].is_string())
            .collect();
        row["bot"] = json!(bot);
        row["starter"] = json!(starter);
        row["depth"] = json!(
            note("depth")
                .and_then(Value::as_f64)
                .filter(|depth| *depth > 0.0)
                .unwrap_or(1.0) as u64
        );
        row["mode"] = data.get("agent").cloned().unwrap_or_default();
        row["tokens"] = json!((count("input") + count("output") + count("reasoning")) as u64);
        row["cost"] = json!(
            data.get("cost")
                .and_then(Value::as_f64)
                .filter(|cost| *cost > 0.0)
        );
        row["answered"] = if answered.is_empty() {
            Value::Null
        } else {
            json!(answered)
        };
    }
    // (Each field left out where there's none.)
    row.as_object_mut()?.retain(|_, value| !value.is_null());
    Some(row)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::listed_work;
    use crate::sessions::Session;

    fn session(value: Value) -> Session {
        serde_json::from_value(value).expect("a session")
    }

    #[test]
    fn a_bots_work_keeps_what_it_ran_on_and_how_it_went() {
        let work = listed_work(&session(json!({
            "id": "ses_w", "agent": "build", "outcome": "succeeded", "cost": 0.25,
            "time": {"created": 1, "idle": 9},
            "model": {"id": "big", "providerID": "opencode", "variant": "high"},
            "tokens": {"input": 10, "output": 5, "reasoning": 2},
            "metadata": {"kuvo": {
                "startedBy": "ses_ada", "by": {"bot": "bot_ada", "name": "Ada"}, "depth": 2,
                "answers": [{"ask": "a1", "decision": "allow"}, {"ask": 3}],
            }},
        })))
        .expect("work");
        assert_eq!(
            work,
            json!({
                "idle": 9, "outcome": "succeeded",
                "model": {"id": "big", "providerID": "opencode", "variant": "high"},
                "bot": "bot_ada", "starter": "ses_ada", "depth": 2, "mode": "build",
                "tokens": 17, "cost": 0.25, "answered": [{"ask": "a1", "decision": "allow"}],
            })
        );
    }

    #[test]
    fn a_subagent_keeps_where_it_ran_and_your_own_conversation_none() {
        let work = listed_work(&session(json!({
            "id": "ses_s", "parentID": "ses_p",
            "metadata": {"kuvo": {"ranOn": " codex ", "requestedModel": ""}},
        })));
        assert_eq!(work, Some(json!({"ranOn": "codex"})));
        assert_eq!(listed_work(&session(json!({"id": "ses_y"}))), None);
        let hidden = session(
            json!({"id": "ses_h", "parentID": "ses_p", "metadata": {"kuvo": {"seat": "x"}}}),
        );
        assert_eq!(listed_work(&hidden), None);
    }
}
