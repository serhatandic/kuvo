//! Attach coding agents to the resident router and model picker.

use std::{
    collections::{BTreeMap, HashMap},
    io,
    sync::Arc,
};

use kuvo_agents::{
    accounts::{AgentKind, Provider},
    resident::ResidentAgents,
    roster::Factory,
    runtime::Runtime,
    sign_in::Request,
};
use kuvo_engine::{
    AgentCatalog, AgentModel, AgentModelSource, CatalogRead, ModelAccount, ModelSpeed,
};
use kuvo_protocol::routes::{
    delete_kuvo_agents_api_integration_provider_connect_oauth_attempt as cancel_oauth,
    delete_kuvo_agents_api_kuvo_accounts_provider as delete_account,
    get_kuvo_agents_api_integration_provider_connect_oauth_attempt as oauth_status,
    patch_kuvo_agents_api_kuvo_accounts_provider as patch_account,
    post_kuvo_agents_api_integration_provider_connect_key as add_key,
    post_kuvo_agents_api_integration_provider_connect_oauth as begin_oauth,
    post_kuvo_agents_api_integration_provider_connect_oauth_attempt_complete as complete_oauth,
    post_kuvo_agents_api_integration_provider_disconnect as disconnect,
    post_kuvo_agents_api_kuvo_accounts as post_account,
};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use serde_json::json;

pub(super) fn factories(_home: &std::path::Path) -> HashMap<AgentKind, Arc<dyn Factory>> {
    #[cfg(feature = "agent-runtime-fixture")]
    if let Some(fixture) = fixture::for_home(_home) {
        return HashMap::from([(AgentKind::Codex, fixture)]);
    }

    // Rows 4.2–4.5 register their compiled factories here.
    HashMap::new()
}

pub(super) struct Models {
    pub runtime: Runtime,
    pub agents: ResidentAgents,
}

impl AgentModelSource for Models {
    fn catalog(&self, refresh: bool) -> CatalogRead<'_, AgentCatalog> {
        Box::pin(async move {
            let roster = self.agents.roster();
            let mut catalog = AgentCatalog {
                accounts: Vec::new(),
                models: Vec::new(),
                auto: None,
                unavailable: Vec::new(),
                generations: BTreeMap::new(),
            };
            for row in roster.catalog(refresh).await {
                let provider = row.account.provider().to_string();
                let known = row.status.ok();
                let status = known.as_ref();
                if row.unavailable || status.is_none() {
                    catalog.unavailable.push(provider.clone());
                }
                catalog.accounts.push(ModelAccount {
                    provider: provider.clone(),
                    kind: row.account.kind.name().into(),
                    id: row.account.id,
                    name: display_name(row.account.kind).into(),
                    label: row.account.label,
                    signed_in_as: status.and_then(|status| status.label.clone()),
                    plan: None,
                    installed: status.is_some_and(|status| status.installed),
                    signed_in: status.is_some_and(|status| status.signed_in),
                    complete: row.models.as_ref().is_some_and(|models| models.complete),
                });
                let Some(models) = row.models else { continue };
                if let Some(generation) = models.generation {
                    catalog.generations.insert(provider.clone(), generation);
                }
                catalog
                    .models
                    .extend(models.models.into_iter().map(|model| {
                        AgentModel {
                            provider: provider.clone(),
                            id: model.id,
                            name: model.name,
                            aliases: model.aliases,
                            efforts: model.efforts,
                            hidden: model.hidden,
                            retiring: model.retiring,
                            provisional: model.provisional,
                            speeds: model
                                .speeds
                                .into_iter()
                                .map(|speed| ModelSpeed {
                                    id: speed.id,
                                    name: speed.name,
                                    description: speed.description,
                                })
                                .collect(),
                        }
                    }));
            }
            Ok(catalog)
        })
    }

    fn session_folder<'a>(&'a self, session: &'a str) -> CatalogRead<'a, Option<String>> {
        Box::pin(self.runtime.session_folder(session))
    }
}

fn display_name(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Claude => "Claude Code",
        AgentKind::Codex => "Codex",
        AgentKind::Cursor => "Cursor",
        AgentKind::Grok => "Grok",
        AgentKind::Antigravity => "Antigravity",
    }
}

pub(super) fn register(agents: ResidentAgents, routes: &mut Registry) {
    register_sign_in(agents.clone(), routes);
    let add = agents.clone();
    routes.register::<post_account::Spec, _, _>(Access::Any, move |call| {
        let agents = add.clone();
        async move {
            let kind = if let Some(given) = &call.body.agent {
                AgentKind::ALL.into_iter().find(|kind| kind.name() == given)
            } else {
                call.body
                    .provider
                    .as_deref()
                    .and_then(Provider::parse)
                    .map(|provider| provider.kind)
            };
            let kind = kind.ok_or_else(|| {
                Refusal::new(
                    http::StatusCode::BAD_REQUEST,
                    "That isn't one of the coding agents.",
                )
            })?;
            let account = agents.add(kind, call.body.label).await.map_err(refuse)?;
            let answer = serde_json::from_value(
                json!({"data":{"providerID":account.provider().to_string()}}),
            )
            .map_err(|error| refuse(error.into()))?;
            Ok(RouteReply::json::<200>(answer))
        }
    });

    let rename = agents.clone();
    routes.register::<patch_account::Spec, _, _>(Access::Any, move |call| {
        let agents = rename.clone();
        async move {
            let provider = provider(call.param("provider"))?;
            agents
                .rename(provider, call.body.label)
                .await
                .map_err(refuse)?;
            let answer = serde_json::from_value(json!({})).map_err(|error| refuse(error.into()))?;
            Ok(RouteReply::json::<200>(answer))
        }
    });

    routes.register::<delete_account::Spec, _, _>(Access::Any, move |call| {
        let agents = agents.clone();
        async move {
            let provider = provider(call.param("provider"))?;
            let only_unsigned = call.request.path.split_once('?').is_some_and(|(_, query)| {
                query
                    .split('&')
                    .any(|parameter| parameter == "onlyUnsigned=1")
            });
            agents
                .remove(provider, only_unsigned)
                .await
                .map_err(refuse)?;
            let answer = serde_json::from_value(json!({})).map_err(|error| refuse(error.into()))?;
            Ok(RouteReply::json::<200>(answer))
        }
    });
}

fn register_sign_in(agents: ResidentAgents, routes: &mut Registry) {
    let start = agents.clone();
    routes.register::<begin_oauth::Spec, _, _>(Access::Any, move |call| {
        let agents = start.clone();
        async move {
            let provider = provider(call.param("provider"))?;
            let started = agents
                .start_sign_in(
                    &provider,
                    Request {
                        method: call.body.method_id,
                        remote: call.body.remote.unwrap_or_default(),
                        browser_return: call.body.browser_return.unwrap_or_default(),
                    },
                )
                .await
                .map_err(refuse)?;
            let answer = serde_json::from_value(located(json!(started)))
                .map_err(|error| refuse(error.into()))?;
            Ok(RouteReply::json::<200>(answer))
        }
    });

    let status = agents.clone();
    routes.register::<oauth_status::Spec, _, _>(Access::Any, move |call| {
        let agents = status.clone();
        async move {
            let provider = provider(call.param("provider"))?;
            let attempt = call.param("attempt").unwrap_or_default();
            let status = agents
                .sign_in_status(&provider, attempt)
                .await
                .map_err(refuse)?;
            let answer = serde_json::from_value(located(json!(status)))
                .map_err(|error| refuse(error.into()))?;
            Ok(RouteReply::json::<200>(answer))
        }
    });

    let complete = agents.clone();
    routes.register::<complete_oauth::Spec, _, _>(Access::Any, move |call| {
        let agents = complete.clone();
        async move {
            let provider = provider(call.param("provider"))?;
            let attempt = call.param("attempt").unwrap_or_default().to_owned();
            agents
                .submit_code(&provider, &attempt, call.body.code)
                .await
                .map_err(refuse)?;
            let answer = serde_json::from_value(json!({})).map_err(|error| refuse(error.into()))?;
            Ok(RouteReply::json::<200>(answer))
        }
    });

    let cancel = agents.clone();
    routes.register::<cancel_oauth::Spec, _, _>(Access::Any, move |call| {
        let agents = cancel.clone();
        async move {
            let provider = provider(call.param("provider"))?;
            let attempt = call.param("attempt").unwrap_or_default();
            agents
                .cancel_sign_in(&provider, attempt)
                .await
                .map_err(refuse)?;
            let answer = serde_json::from_value(json!({})).map_err(|error| refuse(error.into()))?;
            Ok(RouteReply::json::<200>(answer))
        }
    });

    let key = agents.clone();
    routes.register::<add_key::Spec, _, _>(Access::Any, move |call| {
        let agents = key.clone();
        async move {
            let provider = provider(call.param("provider"))?;
            agents
                .sign_in_with_key(&provider, call.body.key)
                .await
                .map_err(refuse)?;
            let answer = serde_json::from_value(json!({})).map_err(|error| refuse(error.into()))?;
            Ok(RouteReply::json::<200>(answer))
        }
    });

    routes.register::<disconnect::Spec, _, _>(Access::Any, move |call| {
        let agents = agents.clone();
        async move {
            let provider = provider(call.param("provider"))?;
            agents.sign_out(&provider).await.map_err(refuse)?;
            let answer = serde_json::from_value(json!({})).map_err(|error| refuse(error.into()))?;
            Ok(RouteReply::json::<200>(answer))
        }
    });
}

fn located(data: serde_json::Value) -> serde_json::Value {
    let directory = std::env::current_dir().unwrap_or_default();
    json!({"data":data,"location":{"directory":directory.to_string_lossy()}})
}

fn provider(raw: Option<&str>) -> Result<Provider, Refusal> {
    raw.and_then(|raw| percent_encoding::percent_decode_str(raw).decode_utf8().ok())
        .and_then(|decoded| Provider::parse(&decoded))
        .ok_or_else(|| Refusal::new(http::StatusCode::NOT_FOUND, "No such account."))
}

fn refuse(error: io::Error) -> Refusal {
    use http::StatusCode;
    let status = match error.kind() {
        io::ErrorKind::NotFound => StatusCode::NOT_FOUND,
        io::ErrorKind::InvalidInput | io::ErrorKind::Unsupported => StatusCode::BAD_REQUEST,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    Refusal::new(status, error.to_string())
}

#[cfg(test)]
#[path = "agents_tests.rs"]
mod tests;

#[cfg(feature = "agent-runtime-fixture")]
#[path = "../../tests/fixtures/agent_runtime.rs"]
mod fixture;
