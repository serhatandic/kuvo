//! Engine settings (`opencode.json`): atomic symlink-preserving save, key-path rewriting,
//! and `GET/PATCH kuvo/config`.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use http::StatusCode;
use kuvo_protocol::routes::{get_kuvo_config, patch_kuvo_config};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use kuvo_store::locks::OwnedLock;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const SERVERS: &[&str] = &["mcp", "servers"];
pub const CHANGED_SINCE: &str = "It changed since you read it.";
pub const UNOWNED_SETTINGS: &str = "This machine's settings still come from your Mac. Connect it from Kuvo Code on your Mac once.";
const BAD_CHANGES: &str = r#"Send {"changes": [{"path": [keys…], "value": …}]}."#;

pub fn is_key_name(name: &str) -> bool {
    name.strip_prefix("KUVO_KEY_").is_some_and(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
}

pub fn in_test_folder(home: &Path, account_home: &Path) -> bool {
    let loc = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let resolved = loc(home);
    resolved != loc(&account_home.join(".kuvo-code"))
        && !resolved.parent().and_then(Path::file_name).and_then(|n| n.to_str()).is_some_and(|n| n.to_ascii_lowercase().starts_with("run.kuvo.code"))
}

pub fn shown_root(home: &Path) -> String {
    let raw = home.to_string_lossy();
    if let Some(u) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).filter(|v| !v.is_empty()) {
        let p = u.to_string_lossy();
        if let Some(rest) = raw.strip_prefix(p.trim_end_matches(['/', '\\'])) && (rest.starts_with('/') || rest.starts_with('\\')) {
            return format!("~{rest}");
        }
    }
    raw.into_owned()
}

pub fn box_env(home: &Path, var: &str) -> Option<String> {
    if let Ok(kh) = std::env::var("KUVO_HOME") && !kh.is_empty() {
        let loc = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        if loc(Path::new(&kh)) != loc(home) { return None; }
    }
    std::env::var(var).ok().filter(|v| !v.is_empty())
}

pub fn app_key_files(home: &Path) -> Option<String> {
    box_env(home, "KUVO_KEYS").map(|v| v.trim_end_matches('/').to_owned()).filter(|v| !v.is_empty())
}

pub fn is_own_home_mac(home: &Path, is_mac: bool) -> bool {
    is_mac && box_env(home, "KUVO_BOX").as_deref() != Some("mac")
}

#[derive(Debug, Clone)]
pub struct ConfigPaths {
    pub home: PathBuf,
    pub is_mac: bool,
    pub config_file: PathBuf,
    pub lock_file: PathBuf,
    pub keys_named: String,
}

impl ConfigPaths {
    pub fn for_home(home: &Path, is_mac: bool) -> Self {
        let config_file = if is_mac {
            box_env(home, "KUVO_ENGINE_CONFIG").map(PathBuf::from).unwrap_or_else(|| home.join("config")).join("opencode").join("opencode.json")
        } else {
            home.join("box").join("opencode.json")
        };
        let default_keys = shown_root(&home.join("box").join("keys"));
        let keys_named = if is_mac { app_key_files(home).unwrap_or(default_keys) } else { default_keys };
        Self { home: home.to_owned(), is_mac, config_file, lock_file: home.join("run").join(".config.lock"), keys_named }
    }

    pub fn owns_settings(&self) -> bool { self.is_mac || self.config_file.exists() }
    pub fn read_settings(&self) -> SettingsRead { read_settings_file(&self.config_file) }
    pub fn change_settings(&self, changes: &Value) -> SettingsChanged {
        change_settings_file(changes, &self.config_file, &self.lock_file, &self.keys_named)
    }
}

#[derive(Debug, Clone)]
pub enum SettingsRead { Ok(Map<String, Value>), Problem(String) }

impl SettingsRead {
    pub fn problem(&self) -> Option<&str> { match self { Self::Problem(p) => Some(p), Self::Ok(_) => None } }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingChange {
    pub path: Vec<String>,
    pub value: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub was: Option<Value>,
}

#[derive(Debug, Clone)]
pub enum SettingsChanged { Ok(Map<String, Value>), Err { status: u16, message: String, problem: Option<String> } }

impl SettingsChanged {
    pub fn into_refusal(status: u16, message: String, problem: Option<String>) -> Refusal {
        let code = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST);
        match problem {
            Some(p) => Refusal::json(code, json!({ "message": message, "problem": p })),
            None => Refusal::new(code, message),
        }
    }
}

pub fn problem_refusal(problem: &str) -> Refusal {
    Refusal::json(StatusCode::CONFLICT, json!({ "message": format!("The engine's settings file can't be changed from here: {problem}"), "problem": problem }))
}

pub fn read_settings_file(file: &Path) -> SettingsRead {
    let text = match fs::read_to_string(file) {
        Ok(t) => t,
        Err(err) => {
            let is_link = fs::symlink_metadata(file).is_ok_and(|m| m.file_type().is_symlink());
            if err.kind() == io::ErrorKind::NotFound && !is_link { return SettingsRead::Ok(Map::new()); }
            return SettingsRead::Problem(if is_link && err.kind() == io::ErrorKind::NotFound {
                "It's a link to a file that isn't there, so Kuvo Code leaves it alone.".into()
            } else {
                format!("It couldn't be read, so Kuvo Code leaves it alone ({}).", err.raw_os_error().map_or_else(|| err.to_string(), |c| c.to_string()))
            });
        }
    };
    if text.trim().is_empty() { return SettingsRead::Ok(Map::new()); }
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => SettingsRead::Ok(map),
        _ => SettingsRead::Problem("It isn't plain JSON, so Kuvo Code leaves it alone. (Comments aren't supported here.)".into()),
    }
}

pub fn value_at<'a>(object: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().try_fold(object, |cur, k| cur.as_object()?.get(*k))
}

pub fn map_value_at<'a>(map: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    let (&first, rest) = keys.split_first()?;
    value_at(map.get(first)?, rest)
}

pub fn setting_at(object: &Map<String, Value>, keys: &[String], value: Value) -> Map<String, Value> {
    let Some((first, rest)) = keys.split_first() else { return object.clone() };
    let mut next = object.clone();
    if rest.is_empty() {
        if value.is_null() { next.shift_remove(first); } else { next.insert(first.clone(), value); }
        return next;
    }
    let inner = setting_at(&next.get(first).and_then(Value::as_object).cloned().unwrap_or_default(), rest, value);
    if inner.is_empty() { next.shift_remove(first); } else { next.insert(first.clone(), Value::Object(inner)); }
    next
}

pub fn sorted_keys(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(sorted_keys).collect()),
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            Value::Object(keys.into_iter().map(|k| (k.clone(), sorted_keys(&map[k]))).collect())
        }
        other => other.clone(),
    }
}

pub fn sorted_json(value: &Value) -> String { serde_json::to_string(&sorted_keys(value)).unwrap_or_default() }

pub fn rewrite_key_refs(value: &Value, keys_named: &str, include_env: bool) -> Value {
    match value {
        Value::String(s) => Value::String(rewrite_key_refs_str(s, keys_named, include_env)),
        Value::Array(items) => Value::Array(items.iter().map(|i| rewrite_key_refs(i, keys_named, include_env)).collect()),
        Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), rewrite_key_refs(v, keys_named, include_env))).collect()),
        other => other.clone(),
    }
}

fn rewrite_key_refs_str(input: &str, keys_named: &str, include_env: bool) -> String {
    let (mut out, mut rest) = (String::with_capacity(input.len()), input);
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let p = if after.starts_with("file:") { Some("file:") } else if include_env && after.starts_with("env:") { Some("env:") } else { None };
        if let Some(p) = p && let Some(close) = after.find('}') {
            let inner = &after[p.len()..close];
            if !inner.contains('{') && let Some(i) = inner.find("KUVO_KEY_") && is_key_name(&inner[i..]) {
                out.push_str(&format!("{{file:{keys_named}/{}}}", &inner[i..]));
                rest = &after[close + 1..];
                continue;
            }
        }
        out.push('{');
        rest = after;
    }
    out.push_str(rest);
    out
}

fn link_target(file: &Path) -> PathBuf {
    let mut target = file.to_path_buf();
    for _ in 0..8 {
        let Ok(dest) = fs::read_link(&target) else { break };
        target = if dest.is_absolute() { dest } else { target.parent().unwrap_or_else(|| Path::new(".")).join(dest) };
    }
    target
}

pub fn write_settings_file(settings: &Map<String, Value>, changes: &[SettingChange], file: &Path, keys_named: &str) -> SettingsChanged {
    let next = changes.iter().fold(settings.clone(), |acc, ch| setting_at(&acc, &ch.path, rewrite_key_refs(&ch.value, keys_named, false)));
    let target = link_target(file);
    let res = serde_json::to_string_pretty(&sorted_keys(&Value::Object(next.clone())))
        .map_err(|e| io::Error::other(e.to_string()))
        .and_then(|mut s| { s.push('\n'); kuvo_store::files::write_atomic(&target, s.as_bytes()) });
    match res {
        Ok(()) => SettingsChanged::Ok(next),
        Err(e) => SettingsChanged::Err { status: 500, message: format!("It couldn't be saved: {e}"), problem: None },
    }
}

fn parse_changes(raw: &Value) -> Option<Vec<SettingChange>> {
    raw.as_array().filter(|a| !a.is_empty() && a.len() <= 100)?.iter().map(|item| {
        let obj = item.as_object()?;
        let path = obj.get("path")?.as_array().filter(|a| !a.is_empty() && a.len() <= 16)?
            .iter().map(|s| s.as_str().filter(|k| !k.is_empty() && k.len() <= 200).map(str::to_owned))
            .collect::<Option<Vec<_>>>()?;
        Some(SettingChange { path, value: obj.get("value")?.clone(), was: obj.get("was").cloned() })
    }).collect()
}

pub fn change_settings_file(raw: &Value, file: &Path, lock: &Path, keys_named: &str) -> SettingsChanged {
    change_settings_recorded(raw,file,lock,keys_named,|_,_,_|{})
}

pub fn change_settings_recorded(raw: &Value, file: &Path, lock: &Path, keys_named: &str,
    record: impl FnOnce(&Map<String,Value>,&Map<String,Value>,&[SettingChange])) -> SettingsChanged {
    let Some(parsed) = parse_changes(raw) else {
        return SettingsChanged::Err { status: 400, message: BAD_CHANGES.into(), problem: None };
    };
    let Ok(_guard) = OwnedLock::acquire(lock, Duration::from_secs(15)) else {
        return SettingsChanged::Err { status: 500, message: "It couldn't be saved: lock failed".into(), problem: None };
    };
    let settings = match read_settings_file(file) {
        SettingsRead::Ok(s) => s,
        SettingsRead::Problem(p) => return SettingsChanged::Err { status: 409, message: format!("The engine's settings file can't be changed from here: {p}"), problem: Some(p) },
    };
    let val = Value::Object(settings.clone());
    for ch in &parsed {
        for depth in 1..ch.path.len() {
            let prefix: Vec<&str> = ch.path[..depth].iter().map(String::as_str).collect();
            if value_at(&val, &prefix).is_some_and(|v| !v.is_object()) {
                return SettingsChanged::Err { status: 400, message: "A path goes through objects only; send a list whole, at its own path.".into(), problem: None };
            }
        }
        if let Some(expected) = &ch.was {
            let refs: Vec<&str> = ch.path.iter().map(String::as_str).collect();
            let actual = value_at(&val, &refs).cloned().unwrap_or(Value::Null);
            if sorted_json(&actual) != sorted_json(expected) {
                return SettingsChanged::Err { status: 409, message: CHANGED_SINCE.into(), problem: None };
            }
        }
    }
    let result=write_settings_file(&settings, &parsed, file, keys_named);
    if let SettingsChanged::Ok(after)=&result {record(&settings,after,&parsed);}
    result
}

pub fn register(routes: &mut Registry, paths: ConfigPaths, setup: kuvo_setup::SetupHandle) {
    let gp = paths.clone();
    routes.register::<get_kuvo_config::Spec, _, _>(Access::Any, move |_| {
        let p = gp.clone();
        async move {
            if !p.owns_settings() { return Err(Refusal::new(StatusCode::CONFLICT, UNOWNED_SETTINGS)); }
            match p.read_settings() {
                SettingsRead::Ok(s) => Ok(RouteReply::json::<200>(s.into_iter().collect::<BTreeMap<_, _>>())),
                SettingsRead::Problem(e) => Err(problem_refusal(&e)),
            }
        }
    });
    routes.register::<patch_kuvo_config::Spec, _, _>(Access::Any, move |call| {
        let (p, body) = (paths.clone(), call.request.body.clone());
        let setup=setup.clone();
        let receipt = form_urlencoded::parse(call.request.path.split_once('?').map(|(_,q)|q).unwrap_or_default().as_bytes()).any(|(k,v)|k=="receipt"&&v=="1");
        async move {
            if !p.owns_settings() { return Err(Refusal::new(StatusCode::CONFLICT, UNOWNED_SETTINGS)); }
            let changes = serde_json::from_slice::<Value>(&body).ok().and_then(|v| v.get("changes").cloned()).unwrap_or(Value::Null);
            let (res,receipts) = tokio::task::spawn_blocking(move || {
                let mut receipts=Vec::new();
                let result=change_settings_recorded(&changes,&p.config_file,&p.lock_file,&p.keys_named,|before,after,changes|{
                    if receipt && let Some(scope)=setup.config_receipt_scope() && let Ok(Some(id))=crate::config_receipts::record(&p.home,scope,before,after,changes,None) {receipts.push(id);}
                });
                (result,receipts)
            }).await
                .map_err(|e| Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            match res {
                SettingsChanged::Ok(s) => Ok(RouteReply::json_as_kept::<200>(if receipt {json!({"settings":s,"configChanges":receipts})} else {Value::Object(s)})),
                SettingsChanged::Err { status, message, problem } => Err(SettingsChanged::into_refusal(status, message, problem)),
            }
        }
    });
}
