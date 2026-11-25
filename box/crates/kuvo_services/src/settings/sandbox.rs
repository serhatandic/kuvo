//! Compatibility answers while the sandbox and Auto are postponed.
//! Existing Also Allowed records can be listed and removed, not added.
use super::*;
use kuvo_store::files;
use std::path::Path;

fn state() -> Value {
    json!({"on":false,"able":false,"agents":false,"reason":"windows","why":"Coming soon on Windows.","needsYou":true})
}
fn read(home: &Path) -> Vec<Value> {
    std::fs::read(home.join("box/sandbox-allowed.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v["allowed"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter(|r| {
            r["id"].is_string()
                && r["project"].is_string()
                && r["kind"] == "write"
                && r["path"].is_string()
        })
        .collect()
}
fn bot(home: &Path, id: &str) -> Option<Value> {
    if !id
        .strip_prefix("bot_")
        .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric()))
    {
        return None;
    }
    std::fs::read(home.join("bots").join(id).join("bot.json"))
        .ok()
        .and_then(|v| serde_json::from_slice(&v).ok())
}
fn listed(home: &Path, rules: Vec<Value>, project: Option<&str>) -> Value {
    let mut allowed = Vec::new();
    for mut rule in rules {
        let folder = PathBuf::from(rule["project"].as_str().unwrap_or_default());
        if project.is_some_and(|p| Path::new(p) != folder) {
            continue;
        }
        let real = std::fs::canonicalize(&folder).unwrap_or_else(|_| folder.clone());
        let chats = home.join("chats");
        let owner = if real == std::fs::canonicalize(&chats).unwrap_or(chats) {
            json!({"kind":"chats","name":"Chats"})
        } else if let Some(id) = real
            .parent()
            .and_then(Path::file_name)
            .and_then(|n| n.to_str())
            .filter(|id| {
                real.file_name().is_some_and(|f| f == "workspace")
                    && real.starts_with(home.join("bots"))
                    && id.starts_with("bot_")
            })
        {
            let name = bot(home, id)
                .and_then(|b| b["name"].as_str().map(str::to_owned))
                .unwrap_or("A bot".into());
            json!({"kind":"bot","id":id,"name":name})
        } else {
            json!({"kind":"project","name":real.file_name().map(|n|n.to_string_lossy()).unwrap_or_else(||real.to_string_lossy())})
        };
        rule["owner"] = owner;
        if !folder.exists() {
            rule["gone"] = json!(true);
        }
        allowed.push(rule);
    }
    let mut answer = json!({"allowed":allowed});
    if project.is_none() {
        let mut owners = std::fs::read_dir(home.join("bots"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let id = entry.file_name().into_string().ok()?;
                let bot = bot(home, &id)?;
                if bot["project"].is_string() {
                    return None;
                }
                Some(json!({"kind":"bot","id":id,"name":bot["name"].as_str().unwrap_or("A bot")}))
            })
            .collect::<Vec<_>>();
        owners.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        owners.push(json!({"kind":"chats","name":"Chats"}));
        answer["owners"] = json!(owners);
    }
    answer
}
pub fn register(routes: &mut Registry, home: PathBuf) {
    routes.register::<get_kuvo_sandbox::Spec, _, _>(Access::Any, |_| async { Ok(typed(state())) });
    routes.register::<post_kuvo_sandbox_check::Spec, _, _>(Access::Any, |_| async {
        Ok(typed(state()))
    });
    let folder = home.clone();
    routes.register::<get_kuvo_sandbox_allowed::Spec, _, _>(Access::Any, move |call| {
        let home = folder.clone();
        async move {
            let project = form_urlencoded::parse(
                call.request
                    .path
                    .split_once('?')
                    .map(|(_, q)| q)
                    .unwrap_or_default()
                    .as_bytes(),
            )
            .find(|(k, _)| k == "project")
            .map(|(_, v)| v.into_owned());
            Ok(typed(listed(&home, read(&home), project.as_deref())))
        }
    });
    let turn = Arc::new(Mutex::new(()));
    routes.register::<delete_kuvo_sandbox_allowed_id::Spec, _, _>(Access::Any, move |call| {
        let home = home.clone();
        let turn = turn.clone();
        async move {
            let _guard = turn.lock().await;
            let id = decoded(call.param("id"));
            let mut rules = read(&home);
            let project = rules
                .iter()
                .find(|r| r["id"] == id)
                .and_then(|r| r["project"].as_str())
                .map(str::to_owned)
                .ok_or_else(|| missing("That isn't in Also Allowed."))?;
            rules.retain(|r| r["id"] != id);
            let mut bytes = serde_json::to_vec_pretty(&json!({"allowed":rules})).expect("rules");
            bytes.push(b'\n');
            files::write_secret(&home.join("box/sandbox-allowed.json"), &bytes)
                .map_err(|e| refused(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            let all = form_urlencoded::parse(
                call.request
                    .path
                    .split_once('?')
                    .map(|(_, q)| q)
                    .unwrap_or_default()
                    .as_bytes(),
            )
            .any(|(k, _)| k == "all");
            Ok(typed(listed(
                &home,
                rules,
                if all { None } else { Some(&project) },
            )))
        }
    });
}
