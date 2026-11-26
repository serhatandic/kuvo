//! The picker contract supplied by the coding-agent and usage runtimes.

use std::{collections::BTreeMap, future::Future, io, pin::Pin};

use serde::Serialize;
use serde_json::{Value, json};

use crate::models::{ModelRef, ModelSpeed};

pub type CatalogRead<'a, T> = Pin<Box<dyn Future<Output = io::Result<T>> + Send + 'a>>;

pub trait AgentModelSource: Send + Sync {
    fn catalog(&self, refresh: bool) -> CatalogRead<'_, AgentCatalog>;
    fn session_folder<'a>(&'a self, session: &'a str) -> CatalogRead<'a, Option<String>>;
}

pub struct AgentCatalog {
    pub accounts: Vec<ModelAccount>,
    pub models: Vec<AgentModel>,
    /// Unknown while the runtime has not read the user's lists.
    pub auto: Option<Vec<AutoModel>>,
    pub unavailable: Vec<String>,
    pub generations: BTreeMap<String, String>,
}

pub struct ModelAccount {
    pub provider: String,
    pub kind: String,
    pub id: String,
    pub name: String,
    pub label: Option<String>,
    pub signed_in_as: Option<String>,
    pub plan: Option<String>,
    pub installed: bool,
    pub signed_in: bool,
    pub complete: bool,
}

pub struct AgentModel {
    pub provider: String,
    pub id: String,
    pub name: String,
    pub aliases: Vec<String>,
    pub efforts: Vec<String>,
    pub speeds: Vec<ModelSpeed>,
    pub hidden: bool,
    pub retiring: bool,
    pub provisional: bool,
}

pub struct AutoModel {
    pub id: String,
    pub name: String,
    pub entry_count: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OfferedAccount<'a> {
    agent: &'a str,
    id: &'a str,
    agent_name: &'a str,
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    plan: Option<&'a str>,
}

impl AgentCatalog {
    fn account(&self, provider: &str) -> Option<&ModelAccount> {
        self.accounts
            .iter()
            .find(|account| account.provider == provider)
    }

    fn runs(&self, provider: &str) -> bool {
        self.account(provider)
            .is_some_and(|account| account.installed && account.signed_in)
    }

    fn complete(&self, account: &ModelAccount) -> bool {
        account.complete
            && self
                .models
                .iter()
                .any(|model| model.provider == account.provider && !model.provisional)
    }

    pub(crate) fn listing(&self) -> Vec<&str> {
        self.accounts
            .iter()
            .filter(|account| account.installed && account.signed_in && !self.complete(account))
            .map(|account| account.provider.as_str())
            .collect()
    }

    pub(crate) fn starting(&self, mut wanted: ModelRef) -> Option<ModelRef> {
        if wanted.provider == "auto" {
            let Some(auto) = &self.auto else {
                return Some(wanted);
            };
            let list = auto
                .iter()
                .find(|list| list.id == wanted.id)
                .or_else(|| auto.first())?;
            return (list.entry_count > 0).then(|| ModelRef {
                id: list.id.clone(),
                variant: None,
                ..wanted
            });
        }
        let account = self
            .account(&wanted.provider)
            .filter(|account| account.installed && account.signed_in)?;
        let models = || {
            self.models
                .iter()
                .filter(|model| model.provider == wanted.provider)
        };
        if let Some(model) = models()
            .find(|model| model.id == wanted.id)
            .or_else(|| models().find(|model| model.aliases.contains(&wanted.id)))
        {
            if model.retiring {
                return None;
            }
            wanted.id.clone_from(&model.id);
            return Some(wanted);
        }
        (!self.complete(account) || self.unavailable.contains(&wanted.provider)).then_some(wanted)
    }

    /// Why `model` can't run now, in the words the engine's catalog uses for its own (`refusal`,
    /// `action`); none when it runs, or it isn't one of the coding agents'.
    pub(crate) fn readiness(&self, model: &ModelRef) -> Option<Value> {
        let account = self.account(&model.provider)?;
        if !account.installed || account.signed_in {
            return None;
        }
        let name = self
            .models
            .iter()
            .filter(|entry| entry.provider == model.provider)
            .find(|entry| entry.id == model.id || entry.aliases.contains(&model.id))
            .map_or(model.id.as_str(), |entry| entry.name.as_str());
        let provider = &account.name;
        Some(json!({
            "model": model, "runs": false, "missing": "signIn",
            "refusal": format!("{name} needs a sign-in to {provider}: sign in, or choose another model."),
            "action": crate::models::action("signIn", "Sign In", format!("Sign in to {provider}")),
        }))
    }

    pub(crate) fn first(&self) -> Option<ModelRef> {
        self.models
            .iter()
            .find(|model| !model.retiring && !model.hidden && self.runs(&model.provider))
            .map(|model| ModelRef {
                provider: model.provider.clone(),
                id: model.id.clone(),
                variant: None,
            })
    }

    pub(crate) fn models(&self) -> impl Iterator<Item = Value> + '_ {
        let models = self
            .models
            .iter()
            .filter(|model| self.runs(&model.provider))
            .map(|model| {
                let mut shown = json!({
                    "providerID": model.provider, "id": model.id, "name": model.name,
                    "aliases": model.aliases, "efforts": model.efforts, "side": "agents",
                });
                if model.retiring {
                    shown["retiring"] = json!(true);
                }
                if model.hidden {
                    shown["hidden"] = json!(true);
                }
                if !model.speeds.is_empty() {
                    shown["speeds"] = json!(model.speeds);
                }
                shown
            });
        let auto = self.auto.iter().flatten().filter(|list| list.entry_count > 0).map(|list| {
            json!({"providerID": "auto", "id": list.id, "name": list.name, "side": "agents", "aliases": [], "efforts": []})
        });
        models.chain(auto)
    }

    pub(crate) fn provider(&self, id: &str) -> Value {
        if id == "auto" {
            return json!({"id": id, "name": "Custom Auto"});
        }
        let Some(account) = self.account(id) else {
            return json!({"id": id, "name": id});
        };
        let default = self
            .accounts
            .iter()
            .find(|candidate| candidate.kind == account.kind && candidate.id == "default");
        let agent_name = default.map_or(account.name.as_str(), |default| default.name.as_str());
        let name = account
            .label
            .as_deref()
            .or(account.signed_in_as.as_deref())
            .unwrap_or(&account.name);
        let mut provider = json!({
            "id": id, "name": account.name,
            "account": OfferedAccount { agent: &account.kind, id: &account.id, agent_name, name, plan: account.plan.as_deref() },
        });
        if let Some(label) = &account.label {
            provider["label"] = json!(label);
        }
        provider
    }
}

impl crate::Engine {
    pub fn with_agent_model_source(mut self, source: std::sync::Arc<dyn AgentModelSource>) -> Self {
        self.agent_models = Some(source);
        self
    }
}
