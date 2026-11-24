//! Startup integration and MCP server upgrades (`upgrade_before_engine`, `upgrade_integrations`).

use std::{collections::HashSet, fs, path::{Path, PathBuf}, time::Duration};

use kuvo_store::{format::ObjectFormat, locks::OwnedLock};
use serde_json::{Map, Value};

use crate::config::{ConfigPaths, SERVERS, SettingChange, SettingsChanged, SettingsRead, is_own_home_mac, map_value_at, read_settings_file, rewrite_key_refs, sorted_json, write_settings_file};

use super::keys::{KeyStore, is_stored_key};

const LAUNCHER: &str = "kuvo-npx";
const APART: &str = "KUVO_NPX_APART";
const OWN_SERVER: &str = "kuvo-browser";
const DEVTOOLS: &str = "chrome-devtools";
const DEVTOOLS_PKG: &str = "chrome-devtools-mcp";
const DEVTOOLS_FLAGS: &[(&str, &str)] = &[("--no-usage-statistics", "--usage-statistics"), ("--isolated", "--no-isolated")];

#[derive(Debug, Clone)]
pub struct CatalogIntegration {
    pub id: String, pub way: String, pub title: String, pub raw: Map<String, Value>,
    pub settings: Option<Value>, pub variables: Option<Map<String, Value>>, pub sign_in_file: Option<String>,
}

pub fn integrations_file_for(home: &Path, is_mac: bool) -> PathBuf {
    let mac = home.join("bin/integrations-mac.json");
    if is_own_home_mac(home, is_mac) && mac.exists() { mac } else { home.join("bin/integrations.json") }
}

pub fn integration_catalog(file: &Path, is_mac: bool) -> Vec<CatalogIntegration> {
    let Some(items) = fs::read_to_string(file).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("integrations")?.as_array().cloned()) else { return Vec::new() };
    items.into_iter().filter_map(|item| {
        let obj = item.as_object()?;
        let id = obj.get("id")?.as_str()?.to_owned();
        if !is_mac && obj.get("macOnly").and_then(Value::as_bool) == Some(true) { return None; }
        Some(CatalogIntegration {
            id, way: obj.get("way").and_then(Value::as_str).unwrap_or("").into(),
            title: obj.get("title").and_then(Value::as_str).unwrap_or("").into(), raw: obj.clone(),
            settings: obj.get("settings").cloned(), variables: obj.get("variables").and_then(Value::as_object).cloned(),
            sign_in_file: obj.get("signInFile").and_then(Value::as_str).map(str::to_owned),
        })
    }).collect()
}

fn package_name(spec: &str) -> &str { spec.rfind('@').filter(|&at| at > 0).map_or(spec, |at| &spec[..at]) }
fn left_for_its_maker(e: &CatalogIntegration, srv: &Map<String, Value>) -> bool {
    e.way == "official" && e.settings.as_ref().is_some_and(Value::is_object)
        && srv.get("command").and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_str) == Some(LAUNCHER)
}

fn brought_up_to_date(e: &CatalogIntegration, srv: &Map<String, Value>, keys_named: &str) -> Value {
    if left_for_its_maker(e, srv) {
        let mut next = e.settings.as_ref().and_then(Value::as_object).cloned().unwrap_or_default();
        for name in ["disabled", "codemode"] { if let Some(v) = srv.get(name) { next.insert(name.into(), v.clone()); } }
        return Value::Object(next);
    }
    let mut next = srv.clone();
    next.entry("codemode").or_insert(Value::Bool(false));
    if e.way == "package"
        && let Some(pinned) = e.settings.as_ref().and_then(|s| s.get("command")?.as_array())
        && let Some(old) = srv.get("command").and_then(Value::as_array)
        && let (Some(pp), Some(ol), Some(op)) = (pinned.get(1).and_then(Value::as_str), old.first().and_then(Value::as_str), old.get(1).and_then(Value::as_str))
        && ol == LAUNCHER && package_name(op) == package_name(pp)
    {
        next.insert("command".into(), Value::Array(pinned.clone()));
        let env_opt = match srv.get("environment") { None => Some(Map::new()), Some(Value::Object(m)) => Some(m.clone()), _ => None };
        if let Some(mut env) = env_opt && !env.contains_key(APART) {
            env.insert(APART.into(), Value::String("1".into()));
            next.insert("environment".into(), Value::Object(env));
        }
    }
    rewrite_key_refs(&Value::Object(next), keys_named, true)
}

fn is_older_version(version: Option<&str>, pinned: &str) -> bool {
    let Some(version) = version.filter(|v| !v.is_empty() && *v != "latest") else { return true };
    let parse = |s: &str| -> Vec<u64> { s.split('.').map(|p| p.chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap_or(0)).collect() };
    let (mine, theirs) = (parse(version), parse(pinned.rfind('@').map_or(pinned, |i| &pinned[i + 1..])));
    for i in 0..mine.len().max(theirs.len()) {
        let (a, b) = (mine.get(i).copied().unwrap_or(0), theirs.get(i).copied().unwrap_or(0));
        if a != b { return a < b; }
    }
    false
}

fn dev_tools_up_to_date(srv: &Map<String, Value>, offered: &Map<String, Value>) -> Option<Map<String, Value>> {
    let pinned = offered.get("command")?.as_array()?.get(1)?.as_str()?;
    let words: Vec<String> = srv.get("command")?.as_array()?.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect();
    let at = words.iter().position(|w| w == DEVTOOLS_PKG || w.starts_with(&format!("{DEVTOOLS_PKG}@")))?;
    let mut opts = words[at + 1..].to_vec();
    for &(flag, opp) in DEVTOOLS_FLAGS {
        let opp_eq = format!("{opp}=");
        if !opts.iter().any(|w| w == flag || w == opp || w.starts_with(&opp_eq)) { opts.insert(0, flag.into()); }
    }
    let mut cmd = vec![LAUNCHER.into(), if is_older_version(words[at].find('@').map(|i| &words[at][i + 1..]), pinned) { pinned.into() } else { words[at].clone() }];
    cmd.extend(opts);
    let before_env = srv.get("environment").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut env = before_env.clone();
    if let Some(off_env) = offered.get("environment").and_then(Value::as_object) {
        for (k, v) in off_env { if !env.contains_key(k) || k == APART { env.insert(k.clone(), v.clone()); } }
    }
    if cmd == words && sorted_json(&Value::Object(env.clone())) == sorted_json(&Value::Object(before_env)) { return None; }
    let mut next = srv.clone();
    next.insert("command".into(), Value::Array(cmd.into_iter().map(Value::String).collect()));
    next.insert("environment".into(), Value::Object(env));
    Some(next)
}

fn dev_tools_offered(popular_file: &Path, is_mac: bool) -> Option<Map<String, Value>> {
    if !is_mac { return None; }
    serde_json::from_str::<Value>(&fs::read_to_string(popular_file).ok()?).ok()?.get("servers")?.as_array()?.iter()
        .find(|e| e.get("id").and_then(Value::as_str) == Some(DEVTOOLS))?.get("settings")?.as_object().cloned()
}

fn upgrade_changes(settings: &Map<String, Value>, catalog: &[CatalogIntegration], keys_gone: &HashSet<String>, popular: Option<&Map<String, Value>>, keys_named: &str) -> Vec<SettingChange> {
    let Some(Value::Object(servers)) = map_value_at(settings, SERVERS) else { return Vec::new() };
    let mut changes = Vec::new();
    for (id, raw) in servers {
        let Some(srv) = raw.as_object() else { continue };
        let Some(entry) = catalog.iter().find(|e| e.id == *id) else {
            if id == DEVTOOLS && let Some(pop) = popular && let Some(mut next) = dev_tools_up_to_date(srv, pop) {
                next.entry("codemode").or_insert(Value::Bool(false));
                changes.push(SettingChange { path: vec![SERVERS[0].into(), SERVERS[1].into(), id.clone()], value: Value::Object(next), was: None });
            } else if id != OWN_SERVER && !srv.contains_key("codemode") {
                changes.push(SettingChange { path: vec![SERVERS[0].into(), SERVERS[1].into(), id.clone(), "codemode".into()], value: Value::Bool(false), was: None });
            }
            continue;
        };
        if left_for_its_maker(entry, srv) && !keys_gone.contains(id) { continue; }
        let next = brought_up_to_date(entry, srv, keys_named);
        if sorted_json(&next) != sorted_json(raw) {
            changes.push(SettingChange { path: vec![SERVERS[0].into(), SERVERS[1].into(), id.clone()], value: next, was: None });
        }
    }
    changes
}

fn write_upgrade(catalog: &[CatalogIntegration], popular: Option<&Map<String, Value>>, keys_gone: &HashSet<String>, config: &ConfigPaths) {
    let Ok(_g) = OwnedLock::acquire(&config.lock_file, Duration::from_secs(15)) else { return };
    let read = read_settings_file(&config.config_file);
    let (changes, written) = match &read {
        SettingsRead::Ok(s) => {
            let c = upgrade_changes(s, catalog, keys_gone, popular, &config.keys_named);
            let w = (!c.is_empty()).then(|| write_settings_file(s, &c, &config.config_file, &config.keys_named));
            (c, w)
        }
        SettingsRead::Problem(_) => (Vec::new(), None),
    };
    let trouble = read.problem().map(str::to_owned).or_else(|| match &written { Some(SettingsChanged::Err { message, .. }) => Some(message.clone()), _ => None });
    if let Some(t) = trouble && (!keys_gone.is_empty() || !changes.is_empty()) { eprintln!("Integrations couldn't be brought up to date: {t}"); }
}

pub fn update_on_trial(home: &Path) -> bool {
    let p = home.join("run/update-trial.json");
    if !p.exists() { return false; }
    let _ = ObjectFormat::additive("update trial").read(&p);
    true
}

fn find_kuvo_keys(val: &Value, out: &mut Vec<String>) {
    match val {
        Value::String(s) => {
            let mut rest = s.as_str();
            while let Some(i) = rest.find("KUVO_KEY_") {
                let tail = &rest[i..];
                let end = tail.find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')).unwrap_or(tail.len());
                let cand = &tail[..end];
                if is_stored_key(cand) && !out.iter().any(|x| x == cand) { out.push(cand.into()); }
                rest = &tail[end.max(1)..];
            }
        }
        Value::Array(items) => items.iter().for_each(|v| find_kuvo_keys(v, out)),
        Value::Object(map) => map.values().for_each(|v| find_kuvo_keys(v, out)),
        _ => {}
    }
}

pub fn upgrade_before_engine(config: &ConfigPaths, catalog_file: &Path, popular_file: &Path) {
    write_upgrade(&integration_catalog(catalog_file, config.is_mac), dev_tools_offered(popular_file, config.is_mac).as_ref(), &HashSet::new(), config);
}

pub async fn upgrade_integrations(config: &ConfigPaths, keys: &KeyStore, catalog_file: &Path, popular_file: &Path) {
    keys.key_work(|| async {
        let catalog = integration_catalog(catalog_file, config.is_mac);
        let mut keys_gone = HashSet::new();
        if !update_on_trial(&config.home) && let SettingsRead::Ok(before) = read_settings_file(&config.config_file) {
            for entry in &catalog {
                let Some(raw) = map_value_at(&before, &[SERVERS[0], SERVERS[1], entry.id.as_str()]) else { continue };
                if !raw.as_object().is_some_and(|srv| left_for_its_maker(entry, srv)) { continue; }
                let mut vars = Vec::new();
                find_kuvo_keys(raw, &mut vars);
                let mut ok = true;
                for name in &vars { if keys.delete_key(name).await.is_err() { ok = false; break; } }
                if ok { keys_gone.insert(entry.id.clone()); }
            }
        }
        write_upgrade(&catalog, dev_tools_offered(popular_file, config.is_mac).as_ref(), &keys_gone, config);
    }).await;
}
