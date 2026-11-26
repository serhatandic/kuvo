//! The browser preferences shared with released versions. Edits preserve other
//! fields and publish under the same on-disk lock as the previous resident.
use http::StatusCode;
use kuvo_protocol::{
    JsonStatus, RouteSpec,
    routes::{
        delete_kuvo_browser_settings_never_host as remove, get_kuvo_browser_settings as get,
        patch_kuvo_browser_settings as patch, put_kuvo_browser_settings_never_host as add,
    },
};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use kuvo_store::{files, locks::OwnedLock};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

const ENGINES: [(&str, &str); 6] = [
    ("google", "Google"),
    ("duckDuckGo", "DuckDuckGo"),
    ("bing", "Bing"),
    ("brave", "Brave Search"),
    ("kagi", "Kagi"),
    ("ecosia", "Ecosia"),
];
const MAX_NEVER: usize = 1000;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    search_engine: &'static str,
    search_engines: Vec<Engine>,
    tidies_agent_tabs: bool,
    offers_to_save: bool,
    never: Vec<String>,
}
#[derive(Serialize)]
struct Engine {
    id: &'static str,
    title: &'static str,
}

fn engine(value: &Value) -> Option<&'static str> {
    ENGINES
        .iter()
        .find(|(id, _)| value.as_str() == Some(id))
        .map(|(id, _)| *id)
}

fn kept(folder: &Path) -> Map<String, Value> {
    files::read(&folder.join("settings.json"))
        .ok()
        .flatten()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn never(kept: &Map<String, Value>) -> Vec<String> {
    kept.get("never")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn settings(folder: &Path) -> Settings {
    let kept = kept(folder);
    Settings {
        search_engine: kept
            .get("searchEngine")
            .and_then(engine)
            .unwrap_or("google"),
        search_engines: ENGINES
            .iter()
            .map(|&(id, title)| Engine { id, title })
            .collect(),
        tidies_agent_tabs: !folder.join("keeps-agent-tabs").exists(),
        offers_to_save: kept.get("offersToSave") != Some(&Value::Bool(false)),
        never: never(&kept),
    }
}

fn invalid(message: impl Into<String>) -> Refusal {
    Refusal::new(StatusCode::BAD_REQUEST, message.into())
}
fn failed(error: impl std::fmt::Display) -> Refusal {
    Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

fn site(value: &str) -> Option<String> {
    let mut host = value.trim().to_lowercase();
    if host.contains("://") {
        let url = url::Url::parse(&host).ok()?;
        host = url.host_str()?.to_owned();
        if let Some(port) = url.port() {
            host.push_str(&format!(":{port}"));
        }
    }
    (!host.is_empty()
        && host.len() <= 260
        && host
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b".:[-]".contains(&b)))
    .then_some(host)
}

fn save(folder: &Path, fields: &Map<String, Value>) -> Result<(), Refusal> {
    files::write_secret(
        &folder.join("settings.json"),
        &serde_json::to_vec(fields).map_err(failed)?,
    )
    .map_err(failed)
}

fn change(folder: &Path, given: Value) -> Result<(), Refusal> {
    let given = given
        .as_object()
        .ok_or_else(|| invalid("Send the settings to change, as an object."))?;
    if let Some(name) = given.keys().find(|name| {
        !["searchEngine", "tidiesAgentTabs", "offersToSave", "never"].contains(&name.as_str())
    }) {
        return Err(invalid(format!(
            "There's no browser setting {}.",
            json!(name)
        )));
    }
    if given
        .get("searchEngine")
        .is_some_and(|v| engine(v).is_none())
    {
        return Err(invalid(format!(
            "The search engine is one of {}.",
            ENGINES.map(|(id, _)| id).join(", ")
        )));
    }
    for name in ["tidiesAgentTabs", "offersToSave"] {
        if given.get(name).is_some_and(|v| !v.is_boolean()) {
            return Err(invalid(format!("{name} is true or false.")));
        }
    }
    let sites = if let Some(list) = given.get("never") {
        let list = list
            .as_array()
            .filter(|list| list.len() <= MAX_NEVER)
            .ok_or_else(|| invalid("never is a list of sites."))?;
        let mut sites = list
            .iter()
            .map(|v| v.as_str().and_then(site))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| invalid("never is a list of sites, each a host (example.com)."))?;
        sites.sort();
        sites.dedup();
        Some(sites)
    } else {
        None
    };
    let mut kept = kept(folder);
    for name in ["searchEngine", "offersToSave"] {
        if let Some(value) = given.get(name) {
            kept.insert(name.into(), value.clone());
        }
    }
    if let Some(sites) = sites {
        kept.insert("never".into(), json!(sites));
    }
    save(folder, &kept)?;
    let tab_file = folder.join("keeps-agent-tabs");
    match given.get("tidiesAgentTabs").and_then(Value::as_bool) {
        Some(true) => files::remove_file_synced(&tab_file).map_err(failed),
        Some(false) => files::write_secret(&tab_file, b"").map_err(failed),
        None => Ok(()),
    }
}

fn change_site(folder: &Path, name: &str, add: bool) -> Result<(), Refusal> {
    let mut kept = kept(folder);
    let mut sites = never(&kept);
    let bad = || invalid("Name a site by its host (example.com).");
    let host = if !add && sites.iter().any(|v| v == name) {
        name.to_owned()
    } else {
        site(name).ok_or_else(bad)?
    };
    if add
        && (host.ends_with(':')
            || url::Url::parse(&format!("http://{host}"))
                .ok()
                .and_then(|u| u.host_str().map(str::to_owned))
                .is_none())
    {
        return Err(bad());
    }
    if sites.contains(&host) == add {
        return Ok(());
    }
    if add {
        if sites.len() >= MAX_NEVER {
            return Err(invalid(format!(
                "No more than {MAX_NEVER} sites can be listed."
            )));
        }
        sites.push(host);
        sites.sort();
    } else {
        sites.retain(|v| v != &host);
    }
    kept.insert("never".into(), json!(sites));
    save(folder, &kept)
}

async fn edit<R>(
    folder: PathBuf,
    change: impl FnOnce(&Path) -> Result<(), Refusal> + Send + 'static,
) -> Result<RouteReply<R>, Refusal>
where
    R: RouteSpec + JsonStatus<200>,
{
    let answer = tokio::task::spawn_blocking(move || {
        let _lock = OwnedLock::acquire(&folder.join(".settings.lock"), Duration::from_secs(3))
            .map_err(failed)?;
        change(&folder)?;
        Ok::<_, Refusal>(settings(&folder))
    })
    .await
    .map_err(failed)??;
    Ok(RouteReply::json_as_kept::<200>(json!(answer)))
}

pub fn register(routes: &mut Registry, home: &Path) {
    let folder = home.join("browser");
    let read = folder.clone();
    routes.register::<get::Spec, _, _>(Access::Any, move |_| {
        let folder = read.clone();
        async move {
            let answer = tokio::task::spawn_blocking(move || settings(&folder))
                .await
                .map_err(failed)?;
            Ok(RouteReply::json_as_kept::<200>(json!(answer)))
        }
    });
    let write = folder.clone();
    routes.register::<patch::Spec, _, _>(Access::Any, move |call| {
        let folder = write.clone();
        let given = serde_json::from_slice(&call.request.body).unwrap_or(Value::Null);
        edit(folder, move |folder| change(folder, given))
    });
    let insert = folder.clone();
    routes.register::<add::Spec, _, _>(Access::Any, move |call| {
        let folder = insert.clone();
        let host = call.param("host").unwrap_or_default().to_owned();
        edit(folder, move |folder| {
            change_site(folder, &decode(&host)?, true)
        })
    });
    routes.register::<remove::Spec, _, _>(Access::Any, move |call| {
        let folder = folder.clone();
        let host = call.param("host").unwrap_or_default().to_owned();
        edit(folder, move |folder| {
            change_site(folder, &decode(&host)?, false)
        })
    });
}

fn decode(host: &str) -> Result<String, Refusal> {
    percent_encoding::percent_decode_str(host)
        .decode_utf8()
        .map(|v| v.into_owned())
        .map_err(|_| invalid("Name a site by its host (example.com)."))
}
