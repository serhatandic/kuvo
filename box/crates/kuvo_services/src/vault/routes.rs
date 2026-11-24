use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use http::StatusCode;
use kuvo_protocol::{JsonStatus, RouteSpec, routes::*};
use kuvo_server::{Access, DispatchRequest, Refusal, Registry, RouteReply};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, json};
use subtle::ConstantTimeEq;

use super::{
    Error, Grant, How, Item, LOCKED, Launch, Result, State, Vault, Who,
    activity::{Action, Activity, Entry, Filter},
    client::Clients,
    helper::millis,
    store::missing,
};

pub(super) const CLIENTS_ONLY: &str = "Only Kuvo Code on your Mac or iPhone can do this.";

pub struct Resident {
    local: Mutex<Local>,
    pub(super) broker: super::broker::Broker,
}

pub(super) struct Local {
    pub(super) vault: Vault,
    pub(super) activity: Activity,
    pub(super) clients: Clients,
    bots: PathBuf,
    gate: super::broker::Gate,
}

impl Resident {
    pub fn new(
        home: &Path,
        mac: bool,
        launch: Launch,
        account: PathBuf,
        os_account: &Path,
        machine: Option<String>,
    ) -> Arc<Self> {
        let bots = std::env::var_os("KUVO_BOTS")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("bots"));
        let broker = super::broker::Broker::new(home, &bots, account, os_account, machine);
        Arc::new(Self {
            local: Mutex::new(Local {
                vault: Vault::open(home, mac, launch),
                activity: Activity::new(home),
                clients: Clients::new(home),
                bots,
                gate: broker.gate,
            }),
            broker,
        })
    }

    pub(super) fn with<T>(&self, work: impl FnOnce(&mut Local) -> Result<T>) -> Result<T> {
        let mut local = self
            .local
            .lock()
            .map_err(|_| Error::refused(503, "The vault's resident stopped."))?;
        local.vault.prime();
        work(&mut local)
    }

    pub(super) fn holder_proof(&self, nonce: &str) -> Option<String> {
        self.local.lock().ok()?.vault.unlock_proof("holder", nonce)
    }

    pub(super) fn proved_holder_client(&self, covered: &str, proof: &str) -> bool {
        self.local
            .lock()
            .is_ok_and(|local| local.vault.proves_unlock("client", covered, proof))
    }

    pub fn register(self: &Arc<Self>, routes: &mut Registry) {
        super::ask_routes::register(self, routes);
        super::fill::register(self, routes);
        super::tools::register(self, routes);
        self.route::<get_kuvo_vault::Spec>(routes, |local, request, _| {
            local.client(request)?;
            let items = if local.vault.locked() {
                Vec::new()
            } else {
                local.vault.items()?
            };
            let grants = local.grants()?;
            let state = local.state()?;
            Ok(json!({ "state": state, "items": items, "grants": grants }))
        });
        self.route::<post_kuvo_vault_unlock::Spec>(routes, |local, request, _| {
            if local.vault.made() && !local.vault.on_host() {
                let body = body(request);
                if !body
                    .get("key")
                    .and_then(Value::as_str)
                    .is_some_and(|key| local.vault.unlock(key))
                {
                    return Err(Error::refused(403, "That isn't this vault's key."));
                }
            }
            Ok(json!(local.state()?))
        });
        self.route::<post_kuvo_vault_key::Spec>(routes, |local, request, _| {
            if local.vault.on_host() {
                return Err(Error::refused(
                    409,
                    "This Mac's vault is its Keychain: it has no key to give.",
                ));
            }
            if local.client(request).is_err() && !local.clients.key_request(body(request)) {
                return Err(Error::refused(403, CLIENTS_ONLY));
            }
            let key = if local.vault.made() {
                local
                    .vault
                    .unlock_key()
                    .ok_or_else(|| Error::refused(423, LOCKED))?
            } else {
                local.vault.make()?
            };
            Ok(json!({ "key": key.expose() }))
        });
        self.route::<post_kuvo_vault_items::Spec>(routes, |local, request, _| {
            let by = local.client(request)?;
            let key = (!local.vault.made())
                .then(|| local.vault.make())
                .transpose()?;
            local.require_unlocked()?;
            let item = local.vault.add(body(request), millis())?;
            local.note(Action::Added, Some(&item), None, None, by)?;
            let mut answer = json!({ "item": item });
            if let Some(key) = key {
                answer["key"] = json!(key.expose());
            }
            Ok(answer)
        });
        self.route::<patch_kuvo_vault_items_id::Spec>(routes, |local, request, id| {
            let by = local.client(request)?;
            local.require_unlocked()?;
            let item = local.vault.change(id, body(request), millis())?;
            local.note(Action::Changed, Some(&item), None, None, by)?;
            Ok(json!({ "item": item }))
        });
        self.route::<delete_kuvo_vault_items_id::Spec>(routes, |local, request, id| {
            let by = local.client(request)?;
            local.require_unlocked()?;
            let item = local.vault.remove(id)?;
            local.note(Action::Removed, Some(&item), None, None, by)?;
            Ok(json!({}))
        });
        self.route::<get_kuvo_vault_items_id_values::Spec>(routes, |local, request, id| {
            let by = local.client(request)?;
            local.require_unlocked()?;
            let item = local.vault.item(id)?.ok_or_else(missing)?;
            let values = local.vault.values(id)?;
            local.note(Action::Shown, Some(&item), None, None, by)?;
            Ok(json!({ "values": values.iter().map(|(field, value)| (field.clone(), json!(value.expose()))).collect::<Map<String, Value>>() }))
        });
        self.route::<post_kuvo_vault_items_id_same::Spec>(routes, |local, request, id| {
            local.client(request)?;
            local.require_unlocked()?;
            local.vault.item(id)?.ok_or_else(missing)?;
            let given = body(request);
            let asked = given
                .get("values")
                .and_then(Value::as_object)
                .filter(|asked| !asked.is_empty() && asked.values().all(Value::is_string))
                .ok_or_else(|| Error::refused(400, "Send {\"values\": {…}}."))?;
            let values = local.vault.values(id)?;
            let same = asked.iter().all(|(field, asked)| {
                values.get(field).is_some_and(|value| {
                    bool::from(
                        value
                            .expose()
                            .as_bytes()
                            .ct_eq(asked.as_str().unwrap_or_default().as_bytes()),
                    )
                })
            });
            Ok(json!({ "same": same }))
        });
        self.route::<get_kuvo_vault_grants::Spec>(routes, |local, request, _| {
            local.client(request)?;
            Ok(json!({ "grants": local.grants()? }))
        });
        self.route::<post_kuvo_vault_grants::Spec>(routes, |local, request, _| {
            let by = local.client(request)?;
            let given = body(request);
            let (id, who) = given.get("item").and_then(Value::as_str).zip(given.get("who").and_then(who))
                .ok_or_else(|| Error::refused(400, "Send {\"item\", \"who\": {\"bot\"} or {\"project\"} or {\"everyone\": true}, \"how\": \"own\" | \"ask\" | null}."))?;
            super::check_id(id)?;
            let item = local.vault.item(id)?.ok_or_else(missing)?;
            let how = given.get("how").filter(|value| !value.is_null())
                .map(|value| serde_json::from_value::<How>(value.clone()))
                .transpose().map_err(|_| Error::refused(400, "A grant is “own” (on its own) or “ask” (asks each time), or null to take it away."))?;
            let bot = if let Who::Bot { bot } = &who { Some(bot.clone()) } else { None };
            local.vault.set_grant(id, who, how, millis())?;
            local.note(if how.is_some() { Action::Granted } else { Action::Ungranted }, Some(&item), bot, given.get("how").and_then(Value::as_str).map(str::to_owned), by)?;
            Ok(json!({ "grants": local.grants()? }))
        });
        self.route::<post_kuvo_vault_pause::Spec>(routes, |local, request, _| {
            let by = local.client(request)?;
            let given = body(request);
            let paused = given.get("paused").and_then(Value::as_bool) == Some(true);
            let bot = given
                .get("bot")
                .and_then(Value::as_str)
                .filter(|bot| {
                    (1..=100).contains(&bot.len())
                        && bot
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                })
                .map(str::to_owned);
            local
                .vault
                .set_paused(paused, bot.as_deref(), "Paused by you.")?;
            local.note(
                if paused {
                    Action::Paused
                } else {
                    Action::Resumed
                },
                None,
                bot,
                None,
                by,
            )?;
            Ok(json!(local.state()?))
        });
        self.route::<get_kuvo_vault_activity::Spec>(routes, |local, request, _| {
            let by = local.client(request)?;
            let query = request.path.split_once('?').map_or("", |(_, query)| query);
            let mut filter = Filter::default();
            for (key, value) in form_urlencoded::parse(query.as_bytes()) {
                match key.as_ref() {
                    "item" if filter.item.is_none() => filter.item = Some(value.into_owned()),
                    "bot" if filter.bot.is_none() => filter.bot = Some(value.into_owned()),
                    "session" if filter.session.is_none() => {
                        filter.session = Some(value.into_owned())
                    }
                    "before" if filter.before.is_none() => {
                        filter.before = value.parse().ok().filter(|n: &f64| !n.is_nan())
                    }
                    "limit" if filter.limit.is_none() => {
                        filter.limit = value.parse().ok().filter(|n: &f64| !n.is_nan())
                    }
                    _ => (),
                }
            }
            Ok(json!({ "entries": local.activity.list(filter, &by, millis())? }))
        });
    }

    fn route<R>(
        self: &Arc<Self>,
        routes: &mut Registry,
        handle: fn(&mut Local, &DispatchRequest, &str) -> Result<Value>,
    ) where
        R: RouteSpec + JsonStatus<200>,
        <R as JsonStatus<200>>::Body: DeserializeOwned,
    {
        let resident = self.clone();
        routes.register::<R, _, _>(Access::Any, move |call| {
            let resident = resident.clone();
            let id = call
                .param("id")
                .and_then(|id| percent_encoding::percent_decode_str(id).decode_utf8().ok())
                .map_or_else(String::new, std::borrow::Cow::into_owned);
            async move {
                tokio::task::spawn_blocking(move || {
                    let value = resident.with(|local| handle(local, &call.request, &id))?;
                    typed::<R, 200>(value)
                })
                .await
                .map_err(|_| refusal(Error::refused(503, "The vault's resident stopped.")))?
            }
        });
    }
}

pub(super) fn typed<R, const CODE: u16>(
    value: impl Serialize,
) -> std::result::Result<RouteReply<R>, Refusal>
where
    R: RouteSpec + JsonStatus<CODE>,
    <R as JsonStatus<CODE>>::Body: DeserializeOwned,
{
    serde_json::to_value(value)
        .and_then(serde_json::from_value)
        .map(RouteReply::json::<CODE>)
        .map_err(|_| {
            Refusal::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Couldn't encode the vault's answer.",
            )
        })
}

fn refusal(error: Error) -> Refusal {
    Refusal::new(
        StatusCode::from_u16(error.status).expect("vault refusal status"),
        error.to_string(),
    )
}

impl From<Error> for Refusal {
    fn from(error: Error) -> Self {
        refusal(error)
    }
}

pub(super) fn body(request: &DispatchRequest) -> Map<String, Value> {
    serde_json::from_slice(&request.body).unwrap_or_default()
}

fn who(value: &Value) -> Option<Who> {
    if let Some(bot) = value.get("bot").and_then(Value::as_str).filter(|bot| {
        bot.strip_prefix("bot_")
            .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric()))
    }) {
        return Some(Who::Bot { bot: bot.into() });
    }
    if let Some(project) = value
        .get("project")
        .and_then(Value::as_str)
        .filter(|path| path.starts_with('/'))
    {
        let mut parts = Vec::new();
        for part in project.split('/') {
            match part {
                "" | "." => (),
                ".." => {
                    parts.pop();
                }
                part => parts.push(part),
            }
        }
        return Some(Who::Project {
            project: format!("/{}", parts.join("/")),
        });
    }
    (value.get("everyone").and_then(Value::as_bool) == Some(true))
        .then_some(Who::Everyone { everyone: true })
}

impl Local {
    pub(super) fn client(&mut self, request: &DispatchRequest) -> Result<String> {
        self.clients
            .client(&mut self.vault, request)
            .ok_or_else(|| Error::refused(403, CLIENTS_ONLY))
    }

    fn require_unlocked(&self) -> Result<()> {
        if self.vault.locked() {
            Err(Error::refused(423, LOCKED))
        } else {
            Ok(())
        }
    }

    fn bot_here(&self, id: &str) -> bool {
        super::identity::Bot::read(&self.bots, id).is_some()
    }

    fn grants(&mut self) -> Result<Vec<Grant>> {
        Ok(self
            .vault
            .grants()?
            .into_iter()
            .filter(|grant| !matches!(&grant.who, Who::Bot { bot } if !self.bot_here(bot)))
            .collect())
    }

    fn state(&mut self) -> Result<State> {
        let mut state = self.vault.state(self.gate.agents, self.gate.waiting)?;
        state
            .paused_bots
            .retain(|who, _| !who.starts_with("bot_") || self.bot_here(who));
        Ok(state)
    }

    fn note(
        &mut self,
        what: Action,
        item: Option<&Item>,
        bot: Option<String>,
        how: Option<String>,
        by: String,
    ) -> Result<()> {
        self.activity
            .note(Entry::client(what, item, bot, how, by, millis())?)
    }
}

impl kuvo_setup::backup::vault::VaultBackend for Resident {
    fn holds_vault(&self) -> bool {
        true
    }

    fn state(&self) -> kuvo_setup::backup::Result<kuvo_setup::backup::vault::VaultState> {
        let state = self
            .with(|local| {
                local.vault.items()?;
                local.vault.grants()?;
                local.state()
            })
            .map_err(backup_error)?;
        Ok(kuvo_setup::backup::vault::VaultState {
            made: state.made,
            locked: state.locked,
            tampered: state.tampered,
            generations: state.generations,
        })
    }

    fn items(&self) -> kuvo_setup::backup::Result<Vec<kuvo_setup::backup::vault::VaultItem>> {
        self.with(|local| local.vault.items())
            .map_err(backup_error)?
            .into_iter()
            .map(|item| {
                let mut metadata = serde_json::to_value(&item)?
                    .as_object()
                    .cloned()
                    .unwrap_or_default();
                for field in ["id", "name", "fields"] {
                    metadata.remove(field);
                }
                Ok(kuvo_setup::backup::vault::VaultItem {
                    id: item.id,
                    name: item.name,
                    fields: item.fields,
                    metadata,
                })
            })
            .collect()
    }

    fn values(
        &self,
        id: &str,
    ) -> kuvo_setup::backup::Result<std::collections::BTreeMap<String, String>> {
        Ok(self
            .with(|local| local.vault.values(id))
            .map_err(backup_error)?
            .into_iter()
            .map(|(field, value)| (field, value.expose().to_owned()))
            .collect())
    }

    fn import_item(
        &self,
        item: kuvo_setup::backup::vault::VaultExportItem,
    ) -> kuvo_setup::backup::Result<()> {
        let mut given = item.metadata;
        given.insert("name".into(), Value::String(item.name));
        given.insert("values".into(), serde_json::to_value(item.values)?);
        self.with(|local| local.vault.add(given, millis()).map(|_| ()))
            .map_err(backup_error)
    }
}

fn backup_error(error: Error) -> kuvo_setup::backup::Error {
    kuvo_setup::backup::Error::Detail(error.to_string())
}
