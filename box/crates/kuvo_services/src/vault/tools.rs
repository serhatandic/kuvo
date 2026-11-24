use std::sync::Arc;

use kuvo_protocol::routes::{get_kuvo_vault_tools as list, post_kuvo_vault_tools as call};
use kuvo_server::{Access, Registry};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use super::{
    Error, How, Item, Kind, Result,
    activity::Action,
    asks::{AskKind, Decision},
    broker::{LEFT_UNDONE, Where},
    fill::{AGENTS_LATER, NOT_CALLING, text},
    helper::millis,
    identity::Identity,
    items::one_line,
    routes::{Resident, body, typed},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Answer {
    pub text: String,
    pub is_error: bool,
}

impl Answer {
    pub fn spoken(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
        }
    }

    pub fn failure(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
        }
    }
}

pub(super) fn register(resident: &Arc<Resident>, routes: &mut Registry) {
    let listing = resident.clone();
    routes.register::<list::Spec, _, _>(Access::Any, move |_| {
        let enabled = listing.broker.gate.agents;
        async move {
            typed::<_, 200>(json!({"tools": if enabled { definitions() } else { Vec::new() }}))
        }
    });
    let resident = resident.clone();
    routes.register::<call::Spec, _, _>(Access::Any, move |call| {
        let resident = resident.clone();
        async move {
            let given = body(&call.request);
            let name = text(&given, "name").unwrap_or_default();
            let answer = if name != "vault_items" {
                if let Some(operation) = operation(&call.request, &given)? {
                    let asked_at = given.clone();
                    let method = name.to_owned();
                    let cancel = if name == "vault_run" {
                        CancellationToken::new()
                    } else {
                        call.cancel.clone()
                    };
                    let runner = resident.clone();
                    resident
                        .broker
                        .ledger
                        .answer(&call.request.path, &given, operation, async move {
                            runner
                                .call_tool(&method, &asked_at, &cancel)
                                .await
                                .unwrap_or_else(|error| Answer::failure(error.message))
                        })
                        .await?
                } else {
                    run(&resident, name, &given, &call.cancel).await
                }
            } else {
                run(&resident, name, &given, &call.cancel).await
            };
            typed::<_, 200>(answer)
        }
    });
}

async fn run(
    resident: &Arc<Resident>,
    name: &str,
    given: &Map<String, Value>,
    cancel: &CancellationToken,
) -> Answer {
    resident
        .call_tool(name, given, cancel)
        .await
        .unwrap_or_else(|error| Answer::failure(error.message))
}

fn operation<'a>(
    request: &'a kuvo_server::DispatchRequest,
    given: &'a Map<String, Value>,
) -> Result<Option<&'a str>> {
    let header = request
        .headers
        .get("x-kuvo-operation")
        .map(|header| header.to_str().unwrap_or_default());
    let field = given
        .get("operation")
        .map(|field| field.as_str().unwrap_or_default());
    if header
        .zip(field)
        .is_some_and(|(header, field)| header != field)
    {
        return Err(Error::refused(400, "Invalid tool operation id."));
    }
    let id = header.or(field);
    if id.is_some_and(|id| !valid_operation(id)) {
        return Err(Error::refused(400, "Invalid tool operation id."));
    }
    Ok(id)
}

fn valid_operation(id: &str) -> bool {
    (1..=80).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

impl Resident {
    async fn call_tool(
        self: &Arc<Self>,
        name: &str,
        given: &Map<String, Value>,
        cancel: &CancellationToken,
    ) -> Result<Answer> {
        if !self.broker.gate.agents {
            return Ok(Answer::failure(AGENTS_LATER));
        }
        let args = given
            .get("arguments")
            .filter(|value| value.is_object())
            .cloned()
            .unwrap_or_else(|| json!({}));
        let session = text(given, "session").unwrap_or_default();
        if name.is_empty()
            || self
                .broker
                .reader
                .calling(session, &[name], Some(&args))
                .await
                .is_none()
        {
            return Ok(Answer::failure(NOT_CALLING));
        }
        let caller = self.broker.reader.caller(session).await;
        let Value::Object(args) = args else {
            unreachable!("tool args are an object")
        };
        match name {
            "vault_items" => self.list_tool(&caller),
            "vault_ask" => self.ask_for_item(&caller, &args, cancel).await,
            "vault_run" => self.run_tool(&caller, &args, cancel).await,
            _ => Ok(Answer::failure(format!(
                "There's no vault tool “{}”.",
                one_line(name, 40)
            ))),
        }
    }

    fn list_tool(&self, caller: &Identity) -> Result<Answer> {
        self.block(|local| {
            let items = local.vault.items()?;
            let grants = local.vault.grants()?;
            let mut usable = 0;
            let mut lines = Vec::new();
            for item in &items {
                let Some(how) = caller.grant(item, &grants, false) else { continue };
                usable += 1;
                let site = if item.sites.is_empty() { String::new() } else { format!(" on {}", item.sites.join(", ")) };
                let fields = if item.kind == Kind::Key && item.fields.len() > 1 { format!(" (fields: {})", item.fields.join(", ")) } else { String::new() };
                let username = if item.kind == Kind::Login {
                    item.username.as_ref().filter(|name| !name.is_empty()).map_or_else(String::new, |name| format!(", username {name}"))
                } else { String::new() };
                let only = if item.sign_in_only == Some(true) { ", only for signing in with browser_sign_in" } else { "" };
                let grant = if how == How::Own { "yours to use" } else { "the user is asked each time" };
                lines.push(format!("- “{}”: a {}{site}{username}{fields}{only}; {grant}.", item.name, kind(item.kind)));
                if item.kind == Kind::Note && item.readable == Some(true) && how == How::Own
                    && let Ok(values) = local.vault.values(&item.id)
                    && let Some(text) = values.get("text").filter(|text| !text.expose().is_empty())
                {
                    lines.push(format!("  It reads: {}", cut(text.expose(), 4000)));
                }
            }
            let more = match items.len() - usable {
                0 => String::new(),
                1 => "The vault has one more item you may not use. If you need one, ask with vault_ask.".into(),
                count => format!("The vault has {count} more items you may not use. If you need one, ask with vault_ask."),
            };
            if lines.is_empty() {
                let rest = if more.is_empty() { "If you need a sign-in or a key, ask with vault_ask." } else { &more };
                return Ok(Answer::spoken(format!("You may use nothing in the vault yet. {rest}")));
            }
            let more = if more.is_empty() { more } else { format!("\n\n{more}") };
            Ok(Answer::spoken(format!(
                "What you may use, by name:\n{}{more}\n\nSign in with browser_sign_in (it fills a sign-in on its own site), fill one field with browser_fill, or hand values to a command with vault_run.",
                lines.join("\n")
            )))
        })
    }

    async fn ask_for_item(
        self: &Arc<Self>,
        caller: &Identity,
        args: &Map<String, Value>,
        cancel: &CancellationToken,
    ) -> Result<Answer> {
        let need = one_line(text(args, "need").unwrap_or_default(), 200);
        let why = one_line(text(args, "why").unwrap_or_default(), 300);
        let (need, why) = (need.trim(), why.trim());
        if need.is_empty() || why.is_empty() {
            return Ok(Answer::failure("Say what you need and why."));
        }
        let site = text(args, "site").and_then(super::site_of);
        let place = Where {
            site: site.clone(),
            program: None,
            why: Some(format!("{need}: {why}")),
        };
        if !caller.autopilot {
            self.block(|local| local.note_agent(Action::Asked, None, caller, &place, None, None))?;
        }
        let mut question = self.broker.question(AskKind::Item, caller);
        question.need = Some(need.into());
        question.why = Some(why.into());
        question.site = site;
        let given = self.broker.wait(caller, question, cancel).await?;
        self.block(|local| {
            let Some(given) = given else {
                local.note_agent(Action::Refused, None, caller, &place, Some("autopilot"), None)?;
                return Ok(Answer::spoken(LEFT_UNDONE));
            };
            let item = given.item.as_deref().map(|id| local.vault.item(id)).transpose()?.flatten();
            let Some(item) = item.filter(|_| given.decision != Decision::No) else {
                return Ok(Answer::spoken("The user didn't give you anything for this. Go on without it, or say what's missing in your report."));
            };
            let who = (given.decision == Decision::Always).then(|| caller.always_for(&self.broker.account)).flatten();
            let once = who.is_none();
            if let Some(who) = who {
                local.vault.set_grant(&item.id, who, Some(How::Own), millis())?;
            } else {
                self.broker.allow_once(caller, &item.id);
            }
            local.note_agent(
                if once { Action::Allowed } else { Action::Granted }, Some(&item), caller,
                &Where::default(), Some(if once { "once" } else { "own" }), given.by.as_deref(),
            )?;
            Ok(Answer::spoken(gave_item(&item, once)))
        })
    }
}

fn gave_item(item: &Item, once: bool) -> String {
    let site = if item.sites.is_empty() {
        String::new()
    } else {
        format!(" for {}", item.sites.join(", "))
    };
    let once = if once { ", for this once" } else { "" };
    let tool = if item.kind == Kind::Login {
        "browser_sign_in"
    } else {
        "vault_run or browser_fill"
    };
    format!(
        "The user gave you “{}” (a {}{site}){once}. Use it by that name with {tool}.",
        item.name,
        kind(item.kind)
    )
}

fn kind(kind: Kind) -> &'static str {
    match kind {
        Kind::Login => "sign-in",
        Kind::Key => "key",
        Kind::File => "file",
        Kind::Note => "note",
    }
}

fn cut(text: &str, maximum: usize) -> String {
    if text.encode_utf16().count() <= maximum {
        return text.into();
    }
    let mut units = 0;
    let mut cut: String = text
        .chars()
        .take_while(|char| {
            units += char.len_utf16();
            units < maximum
        })
        .collect();
    cut.push('…');
    cut
}

fn definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "vault_items", "title": "List vault items",
            "description": "List the vault items you may use (sign-ins, keys, files and notes) by name, with where each may be used and whether the user is asked each time. You never see a value: you name an item, and Kuvo Code fills it in (browser_sign_in, browser_fill) or hands it to a command (vault_run).",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        }),
        json!({
            "name": "vault_ask", "title": "Ask for a vault item",
            "description": "Ask the user for something you need from their vault that vault_items doesn't list, like a site's sign-in or an API key. They pick an item or say no; you get its name, never its value. Waits for their answer.",
            "inputSchema": { "type": "object", "properties": {
              "need": { "type": "string", "description": "What you need, in a few words, like “a sign-in for play.google.com”." },
              "why": { "type": "string", "description": "Why you need it, in one sentence the user reads." },
              "site": { "type": "string", "description": "The site or host it's for, if any." }
            }, "required": ["need", "why"], "additionalProperties": false }
        }),
        json!({
            "name": "vault_run", "title": "Run with vault values",
            "description": "Run a shell command in your project's folder with values from the vault in its environment, never on its command line: each variable gets an item's value (\"GITHUB_TOKEN\": \"GitHub token\"), and a file item's variable gets the path of a temporary copy, removed when the command ends. Vault values are hidden in its output. Use the variables in the command (`curl -H \"Authorization: Bearer $GITHUB_TOKEN\" …`); never print them.",
            "inputSchema": { "type": "object", "properties": {
              "command": { "type": "string", "description": "Run with the user's shell." },
              "variables": { "type": "object", "additionalProperties": { "type": "string" }, "description": "Each variable, and the item (or `Item.field`) whose value it gets." },
              "why": { "type": "string", "description": "Why you need these, in one sentence the user reads." },
              "timeout": { "type": "number", "description": "In seconds: 300 by default, 600 at most." }
            }, "required": ["command", "variables", "why"], "additionalProperties": false }
        }),
    ]
}
