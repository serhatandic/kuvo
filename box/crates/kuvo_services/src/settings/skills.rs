//! The compatibility page for Skills; files and trash remain owned by the skills routes.
use super::pages::said;
use super::*;
use crate::skills::names::{skill_about, skill_name};
use std::collections::{BTreeMap, BTreeSet};

const NOTE: &str = "The agent loads a skill when a task calls for it. Skills in ~/.claude/skills and ~/.agents/skills, and in a project's .opencode/skills, are found on their own.";
const DESCRIPTION: &str = "The agent reads this to decide when the skill is needed, and loads the instructions only then.";
const INSTRUCTIONS: &str = "Markdown. Scripts and other files can go in the skill's folder; mention them here so the agent knows to use them.";
const DELETION: &str = "Its folder, with anything else in it, is kept for 30 days in Recently Deleted, where you can restore it.";

struct Known {
    config: Value,
    own: Value,
    files: BTreeMap<String, String>,
    engine: Vec<Value>,
}
async fn facts(ctx: &Context) -> Result<Known, Refusal> {
    let own = ctx.ask(Method::GET, "kuvo/skills", None).await?;
    let config = ctx.ask(Method::GET, "kuvo/config", None).await?;
    let mut files = BTreeMap::new();
    for name in own["folders"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if let Ok(reply) = ctx
            .ask(Method::GET, &format!("kuvo/skills/{}", encoded(name)), None)
            .await
            && let Some(text) = reply["text"].as_str()
        {
            files.insert(name.into(), text.into());
        }
    }
    let engine = ctx
        .ask(Method::GET, "api/skill", None)
        .await
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    Ok(Known {
        config,
        own,
        files,
        engine,
    })
}
fn encoded(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}
fn disabled(config: &Value, name: &str) -> bool {
    config["permissions"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|r| {
            r["action"] == "skill"
                && r["resource"] == name
                && r["effect"]
                    .as_str()
                    .is_some_and(|s| s != "allow" && s != "ask")
        })
}
fn split_document(text: &str) -> (String, String) {
    let text = text.replace("\r\n", "\n");
    if let Some(rest) = text.strip_prefix("---\n")
        && let Some((front, body)) = rest.split_once("\n---")
        && (body.is_empty() || body.starts_with('\n'))
    {
        return (front.into(), body.trim().into());
    }
    (String::new(), text.trim().into())
}
fn document(name: &str, description: &str, body: &str, existing: &str) -> String {
    let (front, _) = split_document(existing);
    let mut others = Vec::new();
    let mut skipping = false;
    for line in front.lines() {
        if line.starts_with("name:") || line.starts_with("description:") {
            skipping = true;
            continue;
        }
        if skipping && line.starts_with(char::is_whitespace) {
            continue;
        }
        skipping = false;
        if !line.is_empty() {
            others.push(line);
        }
    }
    let others = if others.is_empty() {
        String::new()
    } else {
        format!("{}\n", others.join("\n"))
    };
    format!(
        "---\nname: {}\ndescription: {}\n{others}---\n\n{}\n",
        json!(name),
        json!(description.trim()),
        body.trim()
    )
}
fn form(name: Option<&str>, text: &str) -> Value {
    let about = skill_about(text);
    let mut fields = Vec::new();
    if name.is_none() {
        fields.push(json!({"id":"name","title":"Name","kind":"text","value":"","placeholder":"release-notes"}));
    }
    fields.extend([
        json!({"id":"description","title":"Use it when","kind":"lines","value":about.description.unwrap_or_default(),"placeholder":"Writing release notes from the commits since the last tag","line":DESCRIPTION}),
        json!({"id":"instructions","title":"Instructions","kind":"lines","mono":true,"value":split_document(text).1,"line":INSTRUCTIONS}),
    ]);
    json!({"title":name.unwrap_or("New Skill"),"submit":if name.is_some(){"Save"}else{"Create Skill"},"fields":fields})
}
pub async fn read(ctx: &Context) -> Value {
    let known = match facts(ctx).await {
        Ok(k) => k,
        Err(e) => {
            return unread(
                "skills",
                "Skills",
                said(e, "This computer’s skills couldn’t be read.").await,
            );
        }
    };
    let names = known
        .files
        .keys()
        .map(String::as_str)
        .chain(known.engine.iter().filter_map(|v| v["id"].as_str()))
        .collect::<BTreeSet<_>>();
    let mut items = Vec::new();
    for name in names {
        let text = known.files.get(name);
        let source = known.own["github"].get(name);
        let discovered = known.engine.iter().find(|v| v["id"] == name);
        let about = skill_about(text.map(String::as_str).unwrap_or_default());
        let title = about
            .name
            .filter(|s| !s.is_empty())
            .or_else(|| discovered.and_then(|v| v["name"].as_str().map(str::to_owned)))
            .unwrap_or_else(|| name.into());
        let location = discovered.and_then(|v| v["path"].as_str());
        let origin = source.and_then(|s| s["repository"].as_str()).or_else(|| {
            if text.is_some() {
                Some("Added here")
            } else if location.is_some_and(|l| l.starts_with("/builtin/")) {
                Some("Built in")
            } else {
                None
            }
        });
        let mut actions = Vec::new();
        if let Some(text) = text {
            if source.is_none() {
                actions.push(json!({"id":"edit","title":"Edit…","form":form(Some(name),text)}));
            }
            actions.push(json!({"id":"remove","title":if source.is_some(){"Remove…"}else{"Delete Skill…"},"style":"destructive","confirm":{"title":format!("Delete {name}?"),"body":DELETION,"button":"Delete"}}));
        }
        let mut item = json!({"id":format!("skill-{name}"),"title":title,"kind":"switch","value":!disabled(&known.config,name),"can":if known.config.get("permissions").is_none_or(Value::is_array){json!(true)}else{json!({"why":"Permissions are set as something other than a list. Change them in the settings file."})},"actions":actions});
        if let Some(description) = about
            .description
            .filter(|s| !s.is_empty())
            .or_else(|| discovered.and_then(|v| v["description"].as_str().map(str::to_owned)))
        {
            item["line"] = json!(description);
        }
        if let Some(origin) = origin {
            item["badge"] = json!({"words":origin,"tone":"quiet"});
        } else if let Some(location) = location {
            item["notice"] = json!({"words":location,"tone":"quiet"});
        }
        items.push(item);
    }
    let places = known.config.get("skills");
    if places.is_some_and(|v| !v.is_array()) {
        return unread(
            "skills",
            "Skills",
            "Other Places is set as something other than a list. Change it in the settings file.",
        );
    }
    let mut list = json!({"id":"skills","title":"Skills","kind":"list","empty":"None yet.","add":{"id":"add","title":"New Skill…","form":form(None,"")},"items":items});
    if items.len() > 6 {
        list["search"] =
            json!({"placeholder":"Search skills","empty":"No skills match “{query}”."});
    }
    let mut card = group("available-skills", vec![list]);
    card["title"] = json!("Skills the Agent Can Use");
    card["note"] = json!(NOTE);
    let mut groups = vec![card];
    if let Ok(trash) = ctx.ask(Method::GET, "kuvo/skill-trash", None).await
        && let Some(entries) = trash["entries"].as_array().filter(|v| !v.is_empty())
    {
        let mut list = json!({"id":"deleted","title":"Recently Deleted","kind":"list","items":entries.iter().map(|e|json!({"id":format!("trash-{}",e["id"].as_str().unwrap_or_default()),"title":e["name"],"line":e["description"],"kind":"actions","actions":[{"id":"restore","title":"Restore"}]})).collect::<Vec<_>>()});
        if entries.len() > 6 {
            list["search"] = json!({"placeholder":"Search recently deleted","empty":"No deleted skill matches “{query}”."});
        }
        let mut card = group("deleted-skills", vec![list]);
        card["title"] = json!("Recently Deleted");
        card["note"] = json!(DELETION);
        groups.push(card);
    }
    let mut card = group(
        "get-skills",
        vec![
            json!({"id":"install","title":"Get Skills","kind":"actions","actions":[{"id":"install","title":"Add Skill…","form":{"title":"Add Skill","submit":"Add Skill","fields":[{"id":"repository","title":"Repository","kind":"text","value":"","placeholder":"Another repository on GitHub, like owner/skills"},{"id":"path","title":"Skill’s folder in the repository","kind":"text","value":"","placeholder":"skills/release-notes"}]}}]}),
        ],
    );
    card["title"] = json!("Get Skills");
    card["note"] = json!(
        "These come from their authors on GitHub, not from Kuvo Code. A skill can include scripts the agent may run, so look at what's in one before you add it."
    );
    groups.push(card);
    let mut card = group(
        "other-places",
        vec![
            json!({"id":"places","title":"Other Places","kind":"list","items":places.and_then(Value::as_array).into_iter().flatten().enumerate().map(|(i,p)|json!({"id":format!("place-{i}"),"title":p,"kind":"actions","actions":[{"id":"remove","title":"Remove…","confirm":{"title":"Remove this place?","body":p,"button":"Remove"}}]})).collect::<Vec<_>>(),"add":{"id":"add","title":"Add…","form":{"title":"Other Places","submit":"Add","fields":[{"id":"place","title":"A folder, or the address of a skill catalog","kind":"text","value":""}]}}}),
        ],
    );
    card["title"] = json!("Other Places");
    card["note"] = json!("A catalog is a web address with an index.json listing its skills.");
    groups.push(card);
    page("skills", "Skills", groups)
}

fn check_form(known: &Known, name: Option<&str>, values: &Value) -> (Value, String) {
    let typed = values["name"].as_str().unwrap_or_default().trim();
    let id = name.map(str::to_owned).unwrap_or_else(|| skill_name(typed));
    let mut fields = Vec::new();
    let mut ready = true;
    if name.is_none() {
        let warning = if !typed.is_empty() && id.is_empty() {
            Some("Names use the letters a to z and digits, and this one has none.".into())
        } else if known.own["folders"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|n| n == &id)
            || known.engine.iter().any(|e| e["id"] == id)
        {
            Some(format!("There’s a skill called {id} already."))
        } else {
            None
        };
        ready = !id.is_empty() && warning.is_none();
        let tone = if warning.is_some() {
            "warning"
        } else {
            "quiet"
        };
        let words = warning.unwrap_or_else(|| {
            if !id.is_empty() && id != typed {
                format!("It's saved as {id}.")
            } else {
                "Names use lowercase letters a to z, digits and hyphens.".into()
            }
        });
        fields.push(json!({"id":"name","words":words,"tone":tone}));
    }
    if values["description"]
        .as_str()
        .is_none_or(|s| s.trim().is_empty())
    {
        ready = false;
        fields.push(json!({"id":"description","words":"Say when the agent should use it.","tone":"warning"}));
    }
    if !values["instructions"].is_string() {
        ready = false;
        fields.push(
            json!({"id":"instructions","words":"Instructions must be text.","tone":"warning"}),
        );
    }
    (json!({"ready":ready,"fields":fields}), id)
}

pub async fn check(ctx: &Context, at: &Value, action: &Value, values: &Value) -> Value {
    let id = at["id"].as_str().unwrap_or_default();
    if id == "places" {
        return json!({"ready":values["place"].as_str().is_some_and(|s|!s.trim().is_empty()),"fields":[]});
    }
    if id == "install" {
        return install(values).0;
    }
    let Ok(known) = facts(ctx).await else {
        return json!({"ready":false,"fields":[]});
    };
    check_form(
        &known,
        if action["id"] == "edit" {
            id.strip_prefix("skill-")
        } else {
            None
        },
        values,
    )
    .0
}

fn install(values: &Value) -> (Value, String, String) {
    let raw = values["repository"].as_str().unwrap_or_default().trim();
    let mut typed = raw;
    for prefix in ["https://github.com/", "http://github.com/", "github.com/"] {
        if raw.to_ascii_lowercase().starts_with(prefix) {
            typed = &raw[prefix.len()..];
            break;
        }
    }
    let parts = typed.trim_end_matches('/').split('/').collect::<Vec<_>>();
    let repo = parts.iter().take(2).copied().collect::<Vec<_>>().join("/");
    let repo = repo.strip_suffix(".git").unwrap_or(&repo).to_owned();
    let folder = values["path"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_owned();
    let branch = parts.get(2).is_some_and(|s| matches!(*s, "tree" | "blob"));
    let mut fields = Vec::new();
    if !crate::skills::store::is_repo_id(&repo) || (parts.len() > 2 && !branch) {
        fields.push(json!({"id":"repository","words":"Give a GitHub link or a repository like owner/skills.","tone":"warning"}));
    }
    if branch {
        fields.push(json!({"id":"repository","words":"Use the repository’s main link and give the skill’s folder separately. Links to a branch or file aren’t supported here.","tone":"warning"}));
    }
    if !values["path"].is_string() {
        fields.push(json!({"id":"path","words":"Give the skill’s folder as text, or leave it empty for the repository itself.","tone":"warning"}));
    }
    (
        json!({"ready":fields.is_empty(),"fields":fields}),
        repo,
        folder,
    )
}
fn form_refusal(result: &Value) -> Refusal {
    let words = result["fields"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|f| f["tone"] == "warning")
        .and_then(|f| f["words"].as_str())
        .unwrap_or("Fill in what it needs first.");
    Refusal::json(
        StatusCode::BAD_REQUEST,
        json!({"message":words,"fields":result["fields"]}),
    )
}
async fn write_config(ctx: &Context, key: &str, value: Value, was: &Value) -> Result<(), Refusal> {
    ctx.ask(
        Method::PATCH,
        "kuvo/config",
        Some(json!({"changes":[{"path":[key],"value":value,"was":was}]})),
    )
    .await?;
    Ok(())
}
pub async fn change(ctx: &Context, at: &Value, value: &Value) -> Result<(), Refusal> {
    let name = at["id"]
        .as_str()
        .and_then(|s| s.strip_prefix("skill-"))
        .ok_or_else(|| bad("That isn't something to change."))?;
    let known = facts(ctx).await?;
    let before = &known.config["permissions"];
    let mut rules = before.as_array().cloned().unwrap_or_default();
    rules.retain(|r| {
        !(r["action"] == "skill"
            && r["resource"] == name
            && r["effect"]
                .as_str()
                .is_some_and(|e| e != "allow" && e != "ask"))
    });
    if value == false {
        rules.push(json!({"action":"skill","resource":name,"effect":"deny"}));
    }
    write_config(ctx, "permissions", json!(rules), before).await
}
pub async fn act(ctx: &Context, at: &Value, action: &Value, values: &Value) -> Result<(), Refusal> {
    let known = facts(ctx).await?;
    let id = at["id"].as_str().unwrap_or_default();
    if id == "install" {
        let (check, repo, folder) = install(values);
        if check["ready"] != true {
            return Err(form_refusal(&check));
        }
        ctx.ask(
            Method::POST,
            "kuvo/skill-library/install",
            Some(json!({"repository":repo,"path":folder})),
        )
        .await?;
    } else if let Some(id) = id.strip_prefix("trash-") {
        ctx.ask(
            Method::POST,
            &format!("kuvo/skill-trash/{}/restore", encoded(id)),
            None,
        )
        .await?;
    } else if id == "places" || id.starts_with("place-") {
        let before = &known.config["skills"];
        let mut places = before.as_array().cloned().unwrap_or_default();
        if action["id"] == "add" {
            let place = values["place"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| bad("Give a folder or catalog address."))?;
            places.push(json!(place));
        } else if let Some(index) = id
            .strip_prefix("place-")
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|i| *i < places.len())
        {
            places.remove(index);
        }
        write_config(ctx, "skills", json!(places), before).await?;
    } else {
        let name = id.strip_prefix("skill-");
        let before = name.and_then(|n| known.files.get(n));
        if action["id"] == "remove" {
            let name = name.ok_or_else(no_action)?;
            ctx.ask(
                Method::DELETE,
                &format!("kuvo/skills/{}", encoded(name)),
                Some(json!({"was":before})),
            )
            .await?;
        } else {
            let (check, id) = check_form(&known, name, values);
            if check["ready"] != true {
                return Err(form_refusal(&check));
            }
            let text = document(
                &id,
                values["description"].as_str().unwrap_or_default(),
                values["instructions"].as_str().unwrap_or_default(),
                before.map(String::as_str).unwrap_or_default(),
            );
            ctx.ask(
                Method::PUT,
                &format!("kuvo/skills/{}", encoded(&id)),
                Some(json!({"text":text,"new":name.is_none(),"was":before})),
            )
            .await?;
        }
    }
    Ok(())
}
