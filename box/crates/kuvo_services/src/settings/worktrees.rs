//! Worktree and project settings are projections of the owner routes.
use super::pages::said;
use super::*;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use std::path::{Path, PathBuf};
const TITLE: &str = "Worktrees";

async fn read_inner(ctx: &Context) -> Result<Value, Refusal> {
    let config = ctx.ask(Method::GET, "kuvo/config", None).await?;
    let work = ctx.ask(Method::GET, "kuvo/work/settings", None).await?;
    for name in ["box/work.json", "box/worktrees.json", "box/projects.json"] {
        if let Some(problem) =
            crate::config::read_settings_file(&ctx.settings.home.join(name)).problem()
        {
            return Err(refused(StatusCode::CONFLICT, problem));
        }
    }
    let chosen = config["worktree"].get("directory");
    let query = chosen
        .and_then(Value::as_str)
        .map(|s| {
            format!(
                "?directory={}",
                percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC)
            )
        })
        .unwrap_or_default();
    let where_ = ctx
        .ask(Method::GET, &format!("kuvo/worktrees/where{query}"), None)
        .await?;
    let listed = ctx.ask(Method::GET, "kuvo/worktrees", None).await?;
    let default = where_["default"].as_str().unwrap_or_default();
    let folder = chosen.and_then(Value::as_str).unwrap_or_default();
    let mut rows = vec![
        json!({"id":"worktrees-folder","title":"Worktree folder","line":"Each session’s worktree is a folder in it. Leave it empty for the default.","kind":"folder","value":folder,"placeholder":crate::config::shown_root(Path::new(default)),"folder":{"choose":"Choose a Folder…","clear":"Default Folder","prompt":"Choose","message":"Choose where new worktrees go. Each one is a folder in it."},"can":if config.get("worktree").is_none_or(Value::is_object)&&chosen.is_none_or(Value::is_string){json!(true)}else{json!({"why":"This folder is set in the settings file as something other than text. Change it there."})}}),
    ];
    let projects = where_["projects"].as_array().cloned().unwrap_or_default();
    let common = !projects.is_empty()
        && projects
            .iter()
            .all(|p| p["folder"] == projects[0]["folder"]);
    let expected = if folder.is_empty() {
        default.to_owned()
    } else {
        expand(folder)
    };
    let needs = |p: &Value| {
        p["exists"] == false && p["canMake"] == false || p["inside"] == true && p["ignored"] != true
    };
    let projects = if common {
        vec![
            projects
                .iter()
                .find(|p| needs(p))
                .unwrap_or(&projects[0])
                .clone(),
        ]
    } else {
        projects
    };
    let places = projects
        .iter()
        .filter(|p| {
            needs(p)
                || p["folder"]
                    .as_str()
                    .is_some_and(|s| s != expected && !Path::new(s).starts_with(&expected))
        })
        .collect::<Vec<_>>();
    for each in places.iter().take(5) {
        let name = each["name"].as_str().unwrap_or_default();
        let mut row = json!({"id":format!("place-{}",each["project"].as_str().unwrap_or_default()),"title":if common{"New worktrees go in"}else{name},"kind":"status","value":each["folder"].as_str().unwrap_or("Known once the engine runs")});
        if each["exists"] == false && each["canMake"] == false {
            row["line"] = json!("Kuvo can’t create this folder. Choose another one.");
            row["tone"] = json!("warning");
        } else if each["inside"] == true && each["ignored"] != true {
            let whose = if common {
                format!("{name}’s")
            } else {
                "this project’s".into()
            };
            row["line"] = json!(format!(
                "It’s inside {whose} repository, so git would list each worktree as new files there. Choose a folder outside it."
            ));
            row["tone"] = json!("warning");
        }
        rows.push(row);
    }
    if places.len() > 5 {
        let count = places.len() - 5;
        rows.push(json!({"id":"more-worktree-projects","title":format!("And {count} more {}",if count==1{"project"}else{"projects"}),"kind":"status"}));
    }
    rows.push(json!({"id":"projects-folder","title":"New projects folder","line":"Where projects your bots make go. A bot asks you first, and Kuvo never deletes them.","kind":"folder","value":work["projectsFolder"].as_str().unwrap_or_default(),"placeholder":"None","folder":{"choose":"Choose a Folder…","clear":"None","prompt":"Choose","message":"Choose where new projects go. Each one is a folder in it."},"can":true}));
    let records: Value = std::fs::read(ctx.settings.home.join("box/worktrees.json"))
        .ok()
        .and_then(|v| serde_json::from_slice(&v).ok())
        .unwrap_or_default();
    let running = std::fs::read(ctx.settings.home.join("run/engine.json"))
        .ok()
        .and_then(|v| serde_json::from_slice::<Value>(&v).ok())
        .is_some_and(|v| v["port"].as_u64().is_some_and(|p| p > 0));
    let items=listed["worktrees"].as_array().into_iter().flatten().map(|tree|{
        let branch=tree["branch"].as_str().unwrap_or_default();
        let path=tree["path"].as_str().unwrap_or_default();
        let name=tree["projectName"].as_str().unwrap_or_default();
        let sessions=tree["sessions"].as_u64().unwrap_or_default();
        let mut value=json!({});
        if let Some(record)=records["worktrees"].as_array().into_iter().flatten().find(|r|r["root"]==path){value["record"]=record.clone();}
        for key in ["uncommitted","ahead"]{if let Some(v)=tree.get(key){value[key]=v.clone();}}
        let can=if tree["working"]==true{json!({"why":format!("An agent is still working in the worktree of {branch}, so it stays. Stop it first, then remove the worktree.")})}
        else if running{json!(true)}else{json!({"why":format!("The engine isn't running, so the worktree of {branch} stays. Try again once it's started.")})};
        let mut words=match tree["uncommitted"].as_bool(){Some(true)=>"Its folder is deleted, with changes in it that aren’t committed.",Some(false)=>"Its folder is deleted. Git sees nothing in it that isn’t committed.",None=>"Its folder and anything in it that isn’t committed are deleted."}.to_owned();
        if let Some(ahead)=tree["ahead"].as_u64().filter(|n|*n>0){words.push_str(&format!(" {branch} keeps its {ahead} {} not merged yet.",if ahead==1{"commit"}else{"commits"}));}
        words.push_str(" Its sessions stay, without a folder to run in.");
        json!({"id":format!("tree-{}", URL_SAFE_NO_PAD.encode(path)),"title":branch,"line":format!("{name} · {sessions} {}",if sessions==1{"session"}else{"sessions"}),"kind":"actions","value":value,"actions":[{"id":"remove","title":"Remove…","style":"destructive","can":can,"confirm":{"title":format!("Remove the worktree for {branch}?"),"body":words,"button":"Remove"}}]})
    }).collect::<Vec<_>>();
    let mut card = group(
        "session-worktrees",
        vec![
            json!({"id":"worktrees","title":"Sessions’ Worktrees","kind":"list","empty":"None yet. Choose Worktree in a new session’s composer to give it one.","items":items}),
        ],
    );
    card["title"] = json!("Sessions’ Worktrees");
    card["note"] = json!(
        "Removing one deletes its folder and anything in it that isn’t committed. Its branch stays if it isn’t merged yet."
    );
    Ok(page("worktrees", TITLE, vec![group("folders", rows), card]))
}
fn expand(path: &str) -> String {
    if path == "~" || path.starts_with("~/") {
        use kuvo_os::platform::{Account, Environment, Platform};
        let env: Environment = std::env::vars_os().collect();
        if let Ok(home) = Account::current().and_then(|a| a.home_folder(Platform::current(), &env))
        {
            return home
                .join(path.trim_start_matches('~').trim_start_matches('/'))
                .to_string_lossy()
                .into_owned();
        }
    }
    path.into()
}
pub async fn read(ctx: &Context) -> Value {
    match read_inner(ctx).await {
        Ok(p) => p,
        Err(e) => unread(
            "worktrees",
            TITLE,
            said(e, "This computer's settings couldn't be read.").await,
        ),
    }
}
pub async fn change(ctx: &Context, at: &Value, value: &Value) -> Result<(), Refusal> {
    let text = value.as_str().unwrap_or_default().trim();
    if at["id"] == "projects-folder" {
        ctx.ask(Method::PATCH,"kuvo/work/settings",Some(json!({"projectsFolder":if text.is_empty(){Value::Null}else{json!(text)},"was":{"projectsFolder":at["value"].as_str().filter(|s|!s.is_empty())}}))).await?;
        return Ok(());
    }
    if at["id"] != "worktrees-folder" {
        return Err(bad("That isn't something to change."));
    }
    let config = ctx.ask(Method::GET, "kuvo/config", None).await?;
    if config["worktree"]["directory"].as_str().unwrap_or_default() != at["value"] {
        return Err(stale(read(ctx).await));
    }
    let folder = if text.is_empty() {
        Value::Null
    } else {
        let path = PathBuf::from(expand(text));
        if !path.is_absolute() {
            return Err(bad("Give a full path to the folder."));
        }
        let mut normal = PathBuf::new();
        for part in path.components() {
            match part {
                std::path::Component::ParentDir => {
                    if normal.parent().is_some() {
                        normal.pop();
                    }
                }
                std::path::Component::CurDir => {}
                other => normal.push(other.as_os_str()),
            }
        }
        json!(normal)
    };
    ctx.ask(Method::PATCH,"kuvo/config",Some(json!({"changes":[{"path":["worktree","directory"],"value":folder,"was":config["worktree"]["directory"]}]}))).await?;
    Ok(())
}
pub async fn act(ctx: &Context, at: &Value, action: &Value) -> Result<(), Refusal> {
    if action["id"] != "remove" {
        return Err(no_action());
    }
    // A row ID is a route segment, not a path. Keep the canonical path guard
    // unchanged and carry the checkout's full native path in the opaque ID.
    let path = at["id"].as_str().and_then(|s| s.strip_prefix("tree-"))
        .and_then(|encoded| URL_SAFE_NO_PAD.decode(encoded).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(no_action)?;
    ctx.ask(
        Method::POST,
        "kuvo/worktrees/remove",
        Some(json!({"path":path,"was":at["value"]})),
    )
    .await?;
    Ok(())
}
