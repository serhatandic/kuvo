//! The Changes route boundary. Parse request fields here; repository operations use their own types.
use super::{
    actions::{self, Action},
    branches, changes, list, pulls,
};
use http::StatusCode;
use kuvo_protocol::routes::*;
use kuvo_server::{Access, JsonLine, Refusal, Registry, RouteReply};
use serde_json::{Value, json};
use std::path::PathBuf;

fn query_folder(path: &str) -> Result<PathBuf, Refusal> {
    let query = path.split_once('?').map_or("", |(_, query)| query);
    let given = form_urlencoded::parse(query.as_bytes())
        .find(|(key, _)| key == "directory")
        .map(|(_, value)| value.into_owned())
        .unwrap_or_default();
    changes::folder(&given)
}
fn body(raw: &[u8]) -> Value {
    serde_json::from_slice(raw).unwrap_or_default()
}
fn directory(body: &Value) -> Result<PathBuf, Refusal> {
    changes::scope(body)?;
    changes::folder(body["directory"].as_str().unwrap_or(""))
}

pub(super) fn register(registry: &mut Registry, home: PathBuf, is_mac: bool) {
    registry.register::<get_kuvo_git_files::Spec, _, _>(Access::Any, |call| async move {
        let folder = query_folder(&call.request.path)?;
        Ok(RouteReply::json_as_kept::<200>(list::files(&folder).await?))
    });
    let followers = super::follow::Followers::default();
    registry.register::<get_kuvo_git_files_events::Spec, _, _>(Access::Any, move |call| {
        let followers = followers.clone();
        async move {
            let folder = query_folder(&call.request.path)?;
            let requested=tokio::time::Instant::now();
            let mut rx = followers.subscribe(folder);
            let stream = async_stream::try_stream! {
                let mut last=None;
                loop {
                    let read=rx.borrow_and_update().clone();
                    if let Some(read)=read && read.started>=requested && last.as_ref()!=Some(&read.value) {
                        last=Some(read.value.clone());
                        let body=serde_json::from_value(read.value).map_err(std::io::Error::other)?;
                        yield JsonLine::<get_kuvo_git_files_events::Spec,200>::new(body)?;
                    }
                    tokio::select! {
                        _=call.cancel.cancelled()=>break,
                        changed=rx.changed()=>if changed.is_err(){break},
                    }
                }
            };
            Ok(RouteReply::ndjson::<200>(stream))
        }
    });
    registry.register::<post_kuvo_git_changes::Spec, _, _>(Access::Any, |call| async move {
        let body = body(&call.request.body);
        let folder = directory(&body)?;
        let invalid = || changes::no(StatusCode::BAD_REQUEST, "Name a file in the repository.");
        let only = if body.get("path").is_none() {
            None
        } else {
            Some(
                body["path"]
                    .as_str()
                    .filter(|s| changes::name(s).is_some())
                    .ok_or_else(invalid)?,
            )
        };
        let old = if body["oldPath"].is_null() {
            None
        } else {
            Some(
                body["oldPath"]
                    .as_str()
                    .and_then(changes::name)
                    .ok_or_else(invalid)?,
            )
        };
        Ok(RouteReply::json_as_kept::<200>(
            changes::diff(
                &folder,
                changes::scope(&body)?,
                body["base"].as_str(),
                only,
                old,
            )
            .await?,
        ))
    });
    macro_rules! file_action {
        ($route:ident,$action:expr) => {{
            let home = home.clone();
            registry.register::<$route::Spec, _, _>(Access::Any, move |call| {
                let home = home.clone();
                async move {
                    Ok(RouteReply::json_as_kept::<200>(
                        actions::file_action($action, &home, &body(&call.request.body)).await?,
                    ))
                }
            });
        }};
    }
    file_action!(post_kuvo_git_stage, Action::Stage);
    file_action!(post_kuvo_git_unstage, Action::Unstage);
    file_action!(post_kuvo_git_discard, Action::Discard);
    registry.register::<post_kuvo_git_commit::Spec, _, _>(Access::Any, |call| async move {
        let body = body(&call.request.body);
        Ok(RouteReply::json_as_kept::<200>(
            changes::commit(&directory(&body)?, &body).await?,
        ))
    });
    registry.register::<post_kuvo_git_push::Spec, _, _>(Access::Any, |call| async move {
        let body = body(&call.request.body);
        let root = changes::root_of(&directory(&body)?).await.ok_or_else(|| {
            changes::no(StatusCode::UNPROCESSABLE_ENTITY, "Not a git repository.")
        })?;
        changes::push(&root, &[])
            .await
            .map_err(|why| changes::no(StatusCode::UNPROCESSABLE_ENTITY, why))?;
        Ok(RouteReply::json_as_kept::<200>(json!({})))
    });
    registry.register::<post_kuvo_git_switch::Spec, _, _>(Access::Any, |call| async move {
        let body = body(&call.request.body);
        Ok(RouteReply::json_as_kept::<200>(
            branches::switch(&directory(&body)?, &body).await?,
        ))
    });
    let here = home.clone();
    registry.register::<post_kuvo_git_branches::Spec, _, _>(Access::Any, move |call| {
        let here = here.clone();
        async move {
            Ok(RouteReply::json_as_kept::<200>(
                branches::branches(&here, &directory(&body(&call.request.body))?).await?,
            ))
        }
    });
    macro_rules! with_home {
        ($route:ident,$handler:path) => {{
            let home = home.clone();
            registry.register::<$route::Spec, _, _>(Access::Any, move |call| {
                let home = home.clone();
                async move {
                    let body = body(&call.request.body);
                    Ok(RouteReply::json_as_kept::<200>(
                        $handler(&home, &directory(&body)?, &body).await?,
                    ))
                }
            });
        }};
    }
    with_home!(post_kuvo_git_merge, branches::merge);
    let draft_home = home.clone();
    registry.register::<post_kuvo_git_pull_request_draft::Spec, _, _>(Access::Any, move |call| {
        let home = draft_home.clone();
        async move {
            let body = body(&call.request.body);
            Ok(RouteReply::json_as_kept::<200>(
                pulls::draft(&home, &directory(&body)?, &body, is_mac).await?,
            ))
        }
    });
    with_home!(post_kuvo_git_pull_request, pulls::create);
}
