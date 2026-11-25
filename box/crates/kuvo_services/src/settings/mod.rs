//! Settings pages are projections of the computer's own routes. Changes to a
//! section run in order, against the revision of the affected row or form.
mod archived;
mod computer;
mod kit;
mod mcp;
mod pages;
mod permissions;
mod sandbox;
mod sizes;
mod skills;
mod storage;
mod worktrees;

use std::{io, path::PathBuf, sync::Arc, time::Duration};

use http::{Method, StatusCode};
use http_body_util::BodyExt;
use kuvo_protocol::{JsonStatus, RouteSpec, check_shape, routes::*};
use kuvo_server::{
    Access, DispatchRequest, Dispatcher, JsonLine, Refusal, Registry, RouteCall, RouteReply,
};
use kuvo_setup::SetupHandle;
use serde_json::{Value, json};
use tokio::sync::{Mutex, watch};
use tokio_util::sync::CancellationToken;

use crate::config::CHANGED_SINCE;
use kit::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Clock,
    Conversations,
    Git,
    Worktrees,
    Browser,
    Archived,
    Storage,
    Permissions,
    Computer,
    Agents,
    Vault,
    Mcp,
    CustomAuto,
    Skills,
}

impl Section {
    const ALL: [Self; 14] = [
        Self::Clock,
        Self::Conversations,
        Self::Permissions,
        Self::Browser,
        Self::Computer,
        Self::Git,
        Self::Worktrees,
        Self::Storage,
        Self::Archived,
        Self::Agents,
        Self::Vault,
        Self::Mcp,
        Self::CustomAuto,
        Self::Skills,
    ];

    fn id(self) -> &'static str {
        match self {
            Self::Clock => "clock",
            Self::Conversations => "conversations",
            Self::Git => "git",
            Self::Worktrees => "worktrees",
            Self::Browser => "browser",
            Self::Archived => "archived",
            Self::Storage => "storage",
            Self::Permissions => "permissions",
            Self::Computer => "computer",
            Self::Agents => "agents",
            Self::Vault => "vault",
            Self::Mcp => "mcp",
            Self::CustomAuto => "custom-auto",
            Self::Skills => "skills",
        }
    }

    fn parse(id: &str) -> Result<Self, Refusal> {
        Self::ALL.into_iter().find(|s| s.id() == id).ok_or_else(|| {
            refused(
                StatusCode::NOT_FOUND,
                "This computer has no settings by that name.",
            )
        })
    }
}

struct Settings {
    home: PathBuf,
    setup: SetupHandle,
    turns: [Mutex<()>; Section::ALL.len()],
    changed: watch::Sender<u64>,
    measurement: Arc<sizes::Measurement>,
}

#[derive(Clone)]
struct Context {
    settings: Arc<Settings>,
    dispatcher: Dispatcher,
    request: DispatchRequest,
    cancel: CancellationToken,
    receipts: Arc<std::sync::Mutex<Vec<String>>>,
}

impl Context {
    fn new<T>(settings: Arc<Settings>, call: &RouteCall<T>) -> Self {
        Self {
            settings,
            dispatcher: call.dispatcher(),
            request: call.request.clone(),
            cancel: call.cancel.clone(),
            receipts: Default::default(),
        }
    }

    async fn ask(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value, Refusal> {
        let receipt = (method == Method::PATCH && path == "kuvo/config") || (method == Method::POST && path.starts_with("kuvo/mcp/catalog/"));
        let request = DispatchRequest {
            method,
            path: if receipt {
                format!("{path}?receipt=1")
            } else {
                path.to_owned()
            },
            headers: Default::default(),
            body: body
                .map(|body| serde_json::to_vec(&body).expect("JSON value").into())
                .unwrap_or_default(),
            caller: self.request.caller.clone(),
        };
        let reply = self
            .dispatcher
            .dispatch(request, self.cancel.child_token())
            .await;
        let status = reply.status;
        let bytes = reply
            .body
            .collect()
            .await
            .map_err(|e| refused(StatusCode::BAD_GATEWAY, e.to_string()))?
            .to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
        if status.is_success() {
            if receipt {
                self.receipts.lock().expect("receipts").extend(
                    body["configChanges"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(str::to_owned),
                );
                return Ok(body.get("settings").cloned().unwrap_or(body));
            }
            Ok(body)
        } else {
            Err(Refusal::json(status, body))
        }
    }

    async fn read(&self, section: Section) -> Value {
        pages::read(self, section).await
    }

    async fn change(
        &self,
        section: Section,
        row: &Value,
        body: &Value,
    ) -> Result<Option<Value>, Refusal> {
        pages::change(self, section, row, body).await
    }
}

fn refused(status: StatusCode, message: impl Into<String>) -> Refusal {
    Refusal::new(status, message)
}
fn bad(message: impl Into<String>) -> Refusal {
    refused(StatusCode::BAD_REQUEST, message)
}
fn missing(message: impl Into<String>) -> Refusal {
    refused(StatusCode::NOT_FOUND, message)
}
fn stale(page: Value) -> Refusal {
    Refusal::json(
        StatusCode::CONFLICT,
        json!({"message":CHANGED_SINCE,"page":page}),
    )
}
fn no_action() -> Refusal {
    missing("That isn't something it does.")
}

fn decoded(part: Option<&str>) -> String {
    percent_encoding::percent_decode_str(part.unwrap_or_default())
        .decode_utf8()
        .map(|v| v.into_owned())
        .unwrap_or_default()
}

fn body<R: RouteSpec>(bytes: &[u8], message: &str) -> Result<Value, Refusal> {
    let body: Value = serde_json::from_slice(bytes).unwrap_or_default();
    if R::ROUTE
        .body
        .is_some_and(|shape| check_shape(shape, Some(&body)).is_ok())
    {
        Ok(body)
    } else {
        Err(bad(message))
    }
}

fn decode_or<T: serde::de::DeserializeOwned>(bytes: &[u8], fallback: Value) -> Result<T, Refusal> {
    serde_json::from_slice(bytes)
        .or_else(|_| serde_json::from_value(fallback))
        .map_err(|_| bad("Send a valid request body."))
}

fn typed<R: JsonStatus<200>>(value: Value) -> RouteReply<R> {
    RouteReply::json_as_kept::<200>(value)
}

async fn mutate(
    ctx: Context,
    section: Section,
    id: String,
    action_id: Option<String>,
    given: Value,
) -> Result<Value, Refusal> {
    let _turn = ctx.settings.turns[section as usize].lock().await;
    let page = ctx.read(section).await;
    let row = find_row(&page, &id).ok_or_else(|| missing("That isn't on this page now."))?;
    let action = action_id.as_deref().and_then(|id| find_action(row, id));
    if revision(row, action) != &given["was"] {
        return Err(stale(page));
    }
    let changed = if action_id.is_some() {
        let action = action.ok_or_else(no_action)?;
        if action.get("opens").is_some() {
            return Err(bad("That opens a page: open it in the app."));
        }
        if let Some(why) = why_not(action) {
            return Err(refused(StatusCode::FORBIDDEN, why));
        }
        if action.get("form").is_some() && !given["values"].is_object() {
            return Err(bad("Fill in its form (values)."));
        }
        pages::act(&ctx, section, row, action, &given["values"])
            .await
            .map(|()| None)
    } else {
        if row.get("can").is_none() {
            return Err(bad("That isn't something to change."));
        }
        if let Some(why) = why_not(row) {
            return Err(refused(StatusCode::FORBIDDEN, why));
        }
        let rule: Rule = serde_json::from_value(row.clone())
            .map_err(|e| refused(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        rule.fits(&given["value"]).map_err(bad)?;
        if let Some((value, limit)) = given["value"]
            .as_f64()
            .zip(row["confirmBelow"]["value"].as_f64())
            && value < limit
            && given["confirmed"] != true
        {
            return Err(bad(row["confirmBelow"]["confirm"]["body"]
                .as_str()
                .unwrap_or_default()));
        }
        ctx.change(section, row, &given).await
    };
    let changed = match changed {
        Ok(changed) => changed,
        Err(error) => {
            let reply = error.reply();
            let status = reply.status;
            let bytes = reply
                .body
                .collect()
                .await
                .map_err(|e| refused(StatusCode::BAD_GATEWAY, e.to_string()))?
                .to_bytes();
            let value: Value = serde_json::from_slice(&bytes).unwrap_or_default();
            if status == StatusCode::CONFLICT
                && value["message"] == CHANGED_SINCE
                && value.get("page").is_none()
            {
                return Err(stale(ctx.read(section).await));
            }
            return Err(Refusal::json(status, value));
        }
    };
    ctx.settings.changed.send_modify(|n| *n = n.wrapping_add(1));
    let mut page = match changed {
        Some(page) => page,
        None => ctx.read(section).await,
    };
    let receipts = ctx.receipts.lock().expect("receipts");
    if !receipts.is_empty() {
        page["configChanges"] = json!(*receipts);
    }
    Ok(page)
}

pub fn register(routes: &mut Registry, home: PathBuf, setup: SetupHandle) {
    sandbox::register(routes, home.clone());
    let settings = Arc::new(Settings {
        home,
        setup,
        turns: std::array::from_fn(|_| Mutex::new(())),
        changed: watch::channel(0).0,
        measurement: Arc::new(sizes::Measurement::default()),
    });
    let state = settings.clone();
    routes.register::<get_kuvo_settings::Spec, _, _>(Access::Any, move |call| {
        let ctx = Context::new(state.clone(), &call);
        async move { Ok(typed(pages::sections(&ctx).await)) }
    });
    let state = settings.clone();
    routes.register::<get_kuvo_settings_section::Spec, _, _>(Access::Any, move |call| {
        let ctx = Context::new(state.clone(), &call);
        async move {
            let section = Section::parse(&decoded(call.param("section")))?;
            Ok(typed(ctx.read(section).await))
        }
    });
    let state = settings.clone();
    routes.register::<get_kuvo_settings_section_events::Spec, _, _>(Access::Any, move |call| {
        let ctx = Context::new(state.clone(), &call);
        async move {
            let section = Section::parse(&decoded(call.param("section")))?;
            let mut changed = ctx.settings.changed.subscribe();
            Ok(RouteReply::ndjson::<200>(async_stream::stream! {
                let mut previous = None;
                loop {
                    let page = ctx.read(section).await;
                    if previous.as_ref() != Some(&page) {
                        let line = serde_json::from_value(page.clone()).map_err(io::Error::other).and_then(JsonLine::new);
                        previous = Some(page);
                        yield line;
                    }
                    tokio::select! {
                        _ = ctx.cancel.cancelled() => break,
                        result = changed.changed() => if result.is_err() { break; },
                        _ = tokio::time::sleep(Duration::from_secs(1)) => {},
                    }
                }
            }))
        }
    });
    let state = settings.clone();
    routes.register_decoded::<patch_kuvo_settings_section::Spec, _, _, _>(
        Access::Any,
        |bytes| decode_or(bytes, json!({"row":"","value":null,"was":""})),
        move |call| {
            let ctx = Context::new(state.clone(), &call);
            async move {
                let section = Section::parse(&decoded(call.param("section")))?;
                let raw: Value = serde_json::from_slice(&call.request.body).unwrap_or_default();
                let id = raw["row"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| bad("Name the row to change (row)."))?
                    .to_owned();
                if !raw["was"].is_string() {
                    return Err(bad("Send the row's revision as it was read (was)."));
                }
                let given = body::<patch_kuvo_settings_section::Spec>(
                    &call.request.body,
                    "Send a row, its value and revision, and confirmed only as true or false.",
                )?;
                Ok(typed(mutate(ctx, section, id, None, given).await?))
            }
        },
    );
    let state = settings.clone();
    routes.register_decoded::<post_kuvo_settings_section_row_action::Spec, _, _, _>(
        Access::Any,
        |bytes| decode_or(bytes, json!({"was":""})),
        move |call| {
            let ctx = Context::new(state.clone(), &call);
            async move {
                let section = Section::parse(&decoded(call.param("section")))?;
                let raw: Value = serde_json::from_slice(&call.request.body).unwrap_or_default();
                if !raw["was"].is_string() {
                    return Err(bad("Send the row's revision as it was read (was)."));
                }
                let given = body::<post_kuvo_settings_section_row_action::Spec>(
                    &call.request.body,
                    "Send the row's revision (was), and its form's values as an object (values).",
                )?;
                Ok(typed(
                    mutate(
                        ctx,
                        section,
                        decoded(call.param("row")),
                        Some(decoded(call.param("action"))),
                        given,
                    )
                    .await?,
                ))
            }
        },
    );
    routes.register_decoded::<post_kuvo_settings_section_row_action_check::Spec, _, _, _>(
        Access::Any,
        |bytes| decode_or(bytes, json!({"values":{}})),
        move |call| {
            let ctx = Context::new(settings.clone(), &call);
            async move {
                let section = Section::parse(&decoded(call.param("section")))?;
                let given = body::<post_kuvo_settings_section_row_action_check::Spec>(
                    &call.request.body,
                    "Send its form's values as an object (values).",
                )?;
                let page = ctx.read(section).await;
                let row = find_row(&page, &decoded(call.param("row")))
                    .ok_or_else(|| missing("That isn't on this page now."))?;
                let action = find_action(row, &decoded(call.param("action")))
                    .filter(|a| a.get("form").is_some())
                    .ok_or_else(|| missing("That has no form to check."))?;
                if given
                    .get("was")
                    .is_some_and(|was| was != revision(row, Some(action)))
                {
                    return Err(stale(page));
                }
                Ok(typed(
                    pages::check(&ctx, section, row, action, &given["values"]).await,
                ))
            }
        },
    );
}
