//! `GET kuvo/mcp/catalog` and `POST kuvo/mcp/catalog/:id`: popular MCP servers from `bin/mcp-catalog.json`.

use std::{fs, path::{Path, PathBuf}};

use http::StatusCode;
use kuvo_protocol::routes::{get_kuvo_mcp_catalog, post_kuvo_mcp_catalog_id};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::config::{ConfigPaths, SERVERS, SettingsChanged, SettingsRead, UNOWNED_SETTINGS, change_settings_recorded, map_value_at, problem_refusal, read_settings_file};

pub const FEATURES: &[&str] = &["mcp.catalog"];
const MCP_ACCESS: &[&str] = &["signIn", "open", "local"];

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawServer { id: Option<Value>, title: Option<Value>, summary: Option<Value>, access: Option<Value>, mac_only: Option<Value>, integration: Option<Value>, settings: Option<Value> }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCatalogServer { pub id: String, pub title: String, pub summary: String, pub access: String, pub added: bool, pub integration: bool, pub mac_only: bool }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpCatalog { pub servers: Vec<McpCatalogServer> }

fn truthy(v: Option<&Value>) -> bool {
    match v { Some(Value::Bool(b)) => *b, Some(Value::Number(n)) => n.as_f64().is_some_and(|x| x != 0.0), Some(Value::String(s)) => !s.is_empty(), Some(Value::Array(_) | Value::Object(_)) => true, _ => false }
}
fn text_of(v: Option<&Value>) -> String {
    match v { None | Some(Value::Null) => String::new(), Some(Value::String(s)) => s.clone(), Some(o) => o.to_string() }
}
fn read_raw(file: &Path) -> Vec<RawServer> {
    fs::read_to_string(file).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("servers")?.as_array().cloned()).unwrap_or_default()
        .into_iter().filter_map(|i| serde_json::from_value(i).ok()).collect()
}
fn offered(s: &RawServer, is_mac: bool) -> Option<(&str, &'static str)> {
    let id = s.id.as_ref()?.as_str()?;
    if !is_mac && truthy(s.mac_only.as_ref()) { return None; }
    let raw = match &s.access { None | Some(Value::Null) => "open", Some(Value::String(v)) => v.as_str(), _ => return None };
    Some((id, MCP_ACCESS.iter().copied().find(|&k| k == raw)?))
}

pub fn mcp_catalog(settings: &Map<String, Value>, file: &Path, is_mac: bool) -> McpCatalog {
    McpCatalog {
        servers: read_raw(file).into_iter().filter_map(|s| {
            let (id, access) = offered(&s, is_mac)?;
            let added = map_value_at(settings, &[SERVERS[0], SERVERS[1], id]).is_some();
            Some(McpCatalogServer { id: id.into(), title: text_of(s.title.as_ref()), summary: text_of(s.summary.as_ref()), access: access.into(), added, integration: added && truthy(s.integration.as_ref()), mac_only: is_mac && truthy(s.mac_only.as_ref()) })
        }).collect(),
    }
}

pub fn catalog_settings(id: &str, file: &Path, is_mac: bool) -> Option<Value> {
    let Value::Object(raw) = read_raw(file).into_iter().find(|s| offered(s, is_mac).is_some_and(|(o, _)| o == id))?.settings? else { return None };
    let mut merged = Map::from_iter([("codemode".into(), Value::Bool(false))]);
    merged.extend(raw);
    Some(Value::Object(merged))
}

pub enum AddCatalogResult { Ok(Map<String, Value>), NotFound, Err { status: u16, message: String, problem: Option<String> } }

pub fn add_from_catalog(id: &str, config_file: &Path, catalog_file: &Path, lock_file: &Path, keys_named: &str, is_mac: bool) -> AddCatalogResult {
    add_recorded(id,config_file,catalog_file,lock_file,keys_named,is_mac,|_,_,_|{})
}
fn add_recorded(id: &str, config_file: &Path, catalog_file: &Path, lock_file: &Path, keys_named: &str, is_mac: bool,
    record: impl FnOnce(&Map<String, Value>,&Map<String,Value>,&[crate::config::SettingChange])) -> AddCatalogResult {
    let Some(value) = catalog_settings(id, catalog_file, is_mac) else { return AddCatalogResult::NotFound };
    let settings = match read_settings_file(config_file) {
        SettingsRead::Ok(s) => s,
        SettingsRead::Problem(p) => return AddCatalogResult::Err { status: 409, message: format!("The engine's settings file can't be changed from here: {p}"), problem: Some(p) },
    };
    let path_refs = [SERVERS[0], SERVERS[1], id];
    if map_value_at(&settings, &path_refs).is_some() { return AddCatalogResult::Ok(settings); }
    match change_settings_recorded(&json!([{ "path": [SERVERS[0], SERVERS[1], id], "value": value, "was": Value::Null }]), config_file, lock_file, keys_named,record) {
        SettingsChanged::Ok(next) => AddCatalogResult::Ok(next),
        SettingsChanged::Err { status: 409, message, problem } => {
            if let SettingsRead::Ok(latest) = read_settings_file(config_file) && map_value_at(&latest, &path_refs).is_some() {
                return AddCatalogResult::Ok(latest);
            }
            AddCatalogResult::Err { status: 409, message, problem }
        }
        SettingsChanged::Err { status, message, problem } => AddCatalogResult::Err { status, message, problem },
    }
}

pub fn catalog_file_for(home: &Path) -> PathBuf { home.join("bin/mcp-catalog.json") }

pub fn register(routes: &mut Registry, config: ConfigPaths, setup: kuvo_setup::SetupHandle) {
    let cat_path = catalog_file_for(&config.home);
    let (gc, gp) = (config.clone(), cat_path.clone());
    routes.register::<get_kuvo_mcp_catalog::Spec, _, _>(Access::Any, move |_| {
        let (cfg, cat) = (gc.clone(), gp.clone());
        async move {
            if !cfg.owns_settings() { return Err(Refusal::new(StatusCode::CONFLICT, UNOWNED_SETTINGS)); }
            let settings = match cfg.read_settings() { SettingsRead::Ok(s) => s, SettingsRead::Problem(p) => return Err(problem_refusal(&p)) };
            Ok(RouteReply::json_as_kept::<200>(serde_json::to_value(mcp_catalog(&settings, &cat, cfg.is_mac)).unwrap_or_default()))
        }
    });
    routes.register::<post_kuvo_mcp_catalog_id::Spec, _, _>(Access::Any, move |call| {
        let (cfg, cat, raw) = (config.clone(), cat_path.clone(), call.param("id").unwrap_or("").to_owned());
        let setup=setup.clone();
        let receipt=form_urlencoded::parse(call.request.path.split_once('?').map(|(_,q)|q).unwrap_or_default().as_bytes()).any(|(k,v)|k=="receipt"&&v=="1");
        async move {
            if !cfg.owns_settings() { return Err(Refusal::new(StatusCode::CONFLICT, UNOWNED_SETTINGS)); }
            let id = percent_decode_str(&raw).decode_utf8().map(|s| s.into_owned()).unwrap_or_default();
            let (result,receipts)=tokio::task::spawn_blocking(move ||{
                let mut receipts=Vec::new();
                let result=add_recorded(&id,&cfg.config_file,&cat,&cfg.lock_file,&cfg.keys_named,cfg.is_mac,|before,after,changes|{
                    if receipt && let Some(scope)=setup.config_receipt_scope() && let Ok(Some(saved))=crate::config_receipts::record(&cfg.home,scope,before,after,changes,Some(&id)){receipts.push(saved);}
                });
                (result,receipts)
            }).await.map_err(|e|Refusal::new(StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;
            match result{
                AddCatalogResult::Ok(s) => Ok(RouteReply::json_as_kept::<200>(if receipt{json!({"settings":s,"configChanges":receipts})}else{Value::Object(s)})),
                AddCatalogResult::NotFound => Err(Refusal::json(StatusCode::NOT_FOUND, json!({}))),
                AddCatalogResult::Err { status, message, problem } => Err(SettingsChanged::into_refusal(status, message, problem)),
            }
        }
    });
}
