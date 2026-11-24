use std::sync::Arc;

use kuvo_protocol::routes::post_kuvo_vault_fill as route;
use kuvo_server::{Access, Caller, LocalOrigin, Registry, Transport};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;
use url::Url;

use super::{
    Error, How, Item, Kind, LOCKED, Result, Secret, Values,
    activity::Action,
    broker::Where,
    helper::millis,
    identity::Identity,
    items::one_line,
    routes::{Resident, body, typed},
};

pub(super) const AGENTS_LATER: &str = "The user's vault isn't open to agents on this computer yet: its items, keys, files and notes can't be used here. To sign in, call browser_sign_in without an item: a sign-in the user lets you use on this site fills on its own, or else they sign in. For anything else, ask the user in the chat.";
pub(super) const NOT_CALLING: &str =
    "Only a conversation's own tool call can use the vault, and this one isn't running in it.";

pub(super) fn register(resident: &Arc<Resident>, routes: &mut Registry) {
    let resident = resident.clone();
    routes.register::<route::Spec, _, _>(Access::Any, move |call| {
        let resident = resident.clone();
        async move {
            let local_app = matches!(
                call.request.caller,
                Caller::Local(Transport::Http {
                    origin: LocalOrigin::App,
                    ..
                })
            );
            typed::<_, 200>(
                resident
                    .fill(body(&call.request), local_app, &call.cancel)
                    .await?,
            )
        }
    });
}

impl Resident {
    async fn fill(
        self: &Arc<Self>,
        given: Map<String, Value>,
        local_app: bool,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        let open = self.broker.gate.agents;
        let field = text(&given, "purpose") == Some("field");
        let named = given
            .get("item")
            .filter(|item| !item.is_null() && item.as_str() != Some(""));
        self.block(|local| {
            if local.vault.on_host() && !local_app {
                return Err(Error::refused(403, super::routes::CLIENTS_ONLY));
            }
            if !open && (field || named.is_some()) {
                return Err(Error::refused(404, AGENTS_LATER));
            }
            if local.vault.locked() {
                return Err(Error::refused(423, LOCKED));
            }
            Ok(())
        })?;
        let session = text(&given, "session").unwrap_or_default();
        let tools: &[&str] = if open {
            &["browser_sign_in", "browser_fill"]
        } else {
            &["browser_sign_in"]
        };
        let call = self
            .broker
            .reader
            .calling(session, tools, None)
            .await
            .filter(|id| self.broker.first_fill(id.clone()))
            .ok_or_else(|| Error::refused(403, NOT_CALLING))?;
        drop(call);
        let caller = self.broker.reader.caller(session).await;
        let site = text(&given, "origin")
            .and_then(page_site)
            .ok_or_else(|| Error::refused(400, "Kuvo Code fills only on a secure page (https)."))?;
        let place = Where {
            site: Some(site.clone()),
            why: text(&given, "why").map(|why| one_line(why, 300)),
            program: None,
        };
        let item = self.fill_item(named, field, &caller, &site)?;
        if field && item.sign_in_only == Some(true) {
            return Err(Error::refused(
                403,
                format!(
                    "“{}” is only for signing in: use browser_sign_in.",
                    item.name
                ),
            ));
        }
        let allowed = self
            .authorize(&item, &caller, &place, true, cancel)
            .await??;
        self.block(|local| {
            let values = local.vault.values(&item.id)?;
            let values = filled(&item, values, field, text(&given, "field"))?;
            local.vault.note_used(&item.id, millis())?;
            local.note_agent(Action::Used, Some(&item), &caller, &place, Some(allowed.as_str()), None)?;
            let mut answer = json!({
                "item": { "id": item.id, "name": item.name },
                "values": values.iter().map(|(field, value)| (field.clone(), json!(value.expose()))).collect::<Map<String, Value>>(),
            });
            if let Some(username) = item.username.as_ref().filter(|username| !username.is_empty()) {
                answer["username"] = json!(username);
            }
            Ok(answer)
        })
    }

    fn fill_item(
        &self,
        named: Option<&Value>,
        field: bool,
        caller: &Identity,
        site: &str,
    ) -> Result<Item> {
        self.block(|local| {
            if let Some(named) = named {
                let name = named.as_str().unwrap_or_default();
                let item = local.vault.named(name)?
                    .ok_or_else(|| Error::refused(404, format!("There's no item “{}” in the vault.", one_line(name, 80))))?;
                if !item.usable_on(site) {
                    let why = format!("It tried to use “{}” on {site}, which isn't one of its sites.", item.name);
                    let who = caller.bot.as_ref().map(|bot| bot.id.as_str()).unwrap_or(&caller.root);
                    if !who.is_empty() { local.vault.set_paused(true, Some(who), &why)?; }
                    let place = Where { site: Some(site.to_owned()), ..Where::default() };
                    local.note_agent(Action::Refused, Some(&item), caller, &place, Some("wrong site"), None)?;
                    let say = if who.is_empty() { String::new() } else {
                        format!(" The user has been told, and this {}'s use of the vault is paused until they look.",
                            if caller.bot.is_some() { "bot" } else { "conversation" })
                    };
                    return Err(Error::refused(403, format!("{why} Kuvo Code only fills it on {}.{say}", item.sites.join(", "))));
                }
                return Ok(item);
            }
            if field { return Err(Error::refused(400, "Name the item to fill from.")); }
            let grants = local.vault.grants()?;
            let mut items = local.vault.items()?.into_iter().filter(|item| {
                item.kind == Kind::Login && item.usable_on(site)
                    && (caller.grant(item, &grants, true) == Some(How::Own) || self.broker.has_once(caller, &item.id, false))
            });
            let item = items.next().ok_or_else(|| Error::refused(404, "No sign-in in the vault is yours to use on this site."))?;
            if items.next().is_some() { return Err(Error::refused(404, "More than one sign-in fits this site.")); }
            Ok(item)
        })
    }
}

fn page_site(text: &str) -> Option<String> {
    let page = Url::parse(text.trim()).ok()?;
    let host = page.host_str()?;
    if page.scheme() != "https" && (page.scheme() != "http" || host != "localhost") {
        return None;
    }
    super::site_of(host)
}

fn filled(item: &Item, mut values: Values, field: bool, named: Option<&str>) -> Result<Values> {
    if !field {
        if item.kind != Kind::Login {
            return Err(Error::refused(
                400,
                format!(
                    "“{}” isn't a sign-in: fill one of its fields with browser_fill.",
                    item.name
                ),
            ));
        }
        values.retain(|field, _| field == "password");
        return Ok(values);
    }
    let field = match named.filter(|name| !name.is_empty()) {
        Some(name) => name.trim().to_lowercase(),
        None if item.kind == Kind::Login => "password".into(),
        None if item.fields.len() == 1 => item.fields[0].clone(),
        None => String::new(),
    };
    if field == "username"
        && let Some(username) = item.username.as_ref().filter(|name| !name.is_empty())
    {
        return Ok([(field, Secret::new(username.clone()))].into());
    }
    if !field.is_empty()
        && let Some(value) = values.remove(&field)
    {
        return Ok([(field, value)].into());
    }
    let fields: Vec<_> = std::iter::once("username")
        .chain(item.fields.iter().map(String::as_str))
        .filter(|field| *field != "username" || item.username.is_some())
        .collect();
    Err(Error::refused(
        400,
        format!(
            "Say which of “{}”'s fields to fill: {}.",
            item.name,
            fields.join(", ")
        ),
    ))
}

pub(super) fn text<'a>(given: &'a Map<String, Value>, field: &str) -> Option<&'a str> {
    given.get(field).and_then(Value::as_str)
}
