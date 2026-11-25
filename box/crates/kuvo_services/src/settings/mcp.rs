//! MCP server settings, using the existing config, catalog and program routes.
use super::pages::{computer_title, said};
use super::*;
use crate::integrations::{
    mcp_setup::{command_line, command_words, server_setups, web_address},
    upgrade::{integration_catalog, integrations_file_for},
};
use std::collections::BTreeMap;

struct Facts {
    config: Value,
    servers: Option<Vec<Value>>,
    integrations: BTreeMap<String, String>,
    where_: String,
}
fn str_of(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}
fn pairs(value: &Value) -> Vec<Value> {
    value.as_object().into_iter().flatten().map(|(k,v)|json!({"key":k,"value":v.as_str().map(str::to_owned).unwrap_or_else(||v.to_string())})).collect()
}
fn command(entry: &Value) -> String {
    command_line(
        &entry["command"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>(),
    )
}
async fn facts(ctx: &Context) -> Result<Facts, Refusal> {
    let config = ctx.ask(Method::GET, "kuvo/config", None).await?;
    let info = ctx
        .ask(Method::GET, "kuvo/info", None)
        .await
        .unwrap_or_default();
    let mac = info["kind"] == "mac";
    let integrations = integration_catalog(&integrations_file_for(&ctx.settings.home, mac), mac)
        .into_iter()
        .map(|i| {
            (
                i.id.clone(),
                if i.title.is_empty() { i.id } else { i.title },
            )
        })
        .collect();
    let running = std::fs::read(ctx.settings.home.join("run/engine.json"))
        .ok()
        .and_then(|v| serde_json::from_slice::<Value>(&v).ok())
        .is_some_and(|v| v["port"].as_u64().is_some_and(|p| p > 0));
    let servers = if running {
        ctx.ask(Method::GET, "api/mcp", None)
            .await
            .ok()
            .and_then(|v| v.as_array().cloned())
    } else {
        None
    };
    let title = computer_title(ctx).await;
    let where_ = match title.as_str() {
        "This Mac" => "this Mac".into(),
        "This Computer" => "this computer".into(),
        _ => title,
    };
    Ok(Facts {
        config,
        servers,
        integrations,
        where_,
    })
}
fn form(known: &Facts, name: Option<&str>, entry: &Value) -> Value {
    let remote = entry["type"] == "remote";
    let custom = entry["oauth"].is_object();
    let where_ = &known.where_;
    let key_note = "To keep a key out of the settings file, write {env:NAME} to use one of your environment variables.";
    let mut fields = Vec::new();
    if name.is_none() {
        fields.push(json!({"id":"name","title":"Name","kind":"text","value":"","placeholder":"files, or paste its setup from the README"}));
    }
    let mut oauth = json!({"id":"oauth","title":"Sign in when the server asks","kind":"switch","value":custom||entry["oauth"]!=false,"shown":{"field":"runs","is":["remote"]},"line":if custom{"With the sign-in set up in the settings file."}else{"Turn off for a server that takes an API key in a header instead."}});
    if custom {
        oauth["can"] =
            json!({"why":"Its sign-in is set up in the settings file: change it there."});
    }
    fields.extend([
        json!({"id":"runs","title":"Runs","kind":"choice","value":if remote{"remote"}else{"local"},"choices":[{"id":"local","title":format!("On {where_}")},{"id":"remote","title":"On the web"}]}),
        json!({"id":"command","title":"Command","kind":"lines","mono":true,"value":command(entry),"placeholder":"npx -y @modelcontextprotocol/server-filesystem ~/Documents","shown":{"field":"runs","is":["local"]}}),
        json!({"id":"cwd","title":"Working folder","kind":"folder","value":str_of(&entry["cwd"]),"placeholder":"The project’s folder","folder":{"choose":"Choose…","clear":"The project’s folder","prompt":"Choose","message":"Choose the folder the server runs in."},"shown":{"field":"runs","is":["local"]}}),
        json!({"id":"environment","title":"Environment Variables","kind":"pairs","value":pairs(&entry["environment"]),"pairs":{"key":"NAME","value":"Value"},"line":format!("Each by its name, like GITHUB_TOKEN. {key_note}"),"shown":{"field":"runs","is":["local"]}}),
        json!({"id":"url","title":"URL","kind":"text","value":str_of(&entry["url"]),"placeholder":"https://example.com/mcp","shown":{"field":"runs","is":["remote"]}}),
        oauth,
        json!({"id":"headers","title":"Headers","kind":"pairs","value":pairs(&entry["headers"]),"pairs":{"key":"Header, like Authorization","value":"Value"},"line":key_note,"shown":{"field":"runs","is":["remote"]}}),
        json!({"id":"codemode","title":"Let the agent call its tools from code","kind":"switch","value":name.is_some()&&entry["codemode"]!=false,"line":"Code Mode: the agent looks up this server's tools and combines several in one step, which keeps each message small for a server with many tools. Smaller models may not find its tools this way."}),
    ]);
    let mut form = json!({"title":name.unwrap_or("Add Server"),"submit":if name.is_some(){"Save"}else{"Add Server"},"fields":fields});
    if name.is_none() {
        form["note"] = json!(format!(
            "A server on {where_} runs a command; one on the web is a URL."
        ));
    }
    form
}
pub async fn read(ctx: &Context) -> Value {
    let known = match facts(ctx).await {
        Ok(f) => f,
        Err(e) => {
            return unread(
                "mcp",
                "MCP Servers",
                said(e, "This computer's settings couldn't be read.").await,
            );
        }
    };
    let where_ = &known.where_;
    let mut entries = known.config["mcp"]["servers"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(n, v)| {
            v.is_object() && n.as_str() != "kuvo-browser" && !known.integrations.contains_key(*n)
        })
        .collect::<Vec<_>>();
    entries.sort_by(|(a, _), (b, _)| a.to_lowercase().cmp(&b.to_lowercase()));
    let items=entries.into_iter().map(|(name,entry)|{
        let on=entry["disabled"]!=true;
        let server=known.servers.as_ref().and_then(|s|s.iter().find(|s|s["name"]==name.as_str()));
        let status=server.and_then(|s|s["status"]["status"].as_str());
        let line=entry["url"].as_str().filter(|s|!s.is_empty()).map(str::to_owned).unwrap_or_else(||command(entry));
        let mut item=json!({"id":format!("server-{name}"),"title":name,"kind":"switch","value":on,"can":true,"actions":[{"id":"edit","title":"Edit…","form":form(&known,Some(name),entry)},{"id":"remove","title":"Remove…","style":"destructive","confirm":{"title":format!("Remove {name}?"),"body":"The agent loses its tools.","button":"Remove"}}]});
        if !line.is_empty(){item["line"]=json!(line);}
        let badge=if !on{Some(("Off","quiet"))}else if known.servers.is_none(){None}else{Some(match status{Some("connected")=>("Connected","good"),Some("pending")=>("Connecting…","busy"),Some("needs_auth"|"needs_client_registration")=>("Needs sign-in","warning"),Some("failed")=>("Couldn’t connect","failed"),Some("disabled")=>("Off","quiet"),_=>("Not started","quiet")})};
        if let Some((words,tone))=badge{item["badge"]=json!({"words":words,"tone":tone});}
        if on&&status==Some("failed")&&let Some(error)=server.and_then(|s|s["status"]["error"].as_str()).map(str::trim).filter(|s|!s.is_empty()){item["notice"]=json!({"words":error,"tone":"warning"});}
        item
    }).collect::<Vec<_>>();
    let mut card = group(
        "servers",
        vec![
            json!({"id":"servers","title":"Servers","kind":"list","items":items,"empty":"None yet.","add":{"id":"add","title":"Add Server…","form":form(&known,None,&json!({}))}}),
        ],
    );
    card["title"] = json!("Servers");
    card["note"] = json!(format!(
        "A server on {where_} runs a command; one on the web is a URL."
    ));
    let mut groups = vec![card];
    if let Ok(catalog) = ctx.ask(Method::GET, "kuvo/mcp/catalog", None).await {
        let popular = catalog["servers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|s| !known.integrations.contains_key(str_of(&s["id"])))
            .collect::<Vec<_>>();
        let accesses = ["signIn", "open", "local"]
            .into_iter()
            .filter(|id| popular.iter().any(|p| p["access"] == *id))
            .collect::<Vec<_>>();
        for access in &accesses {
            let rows=popular.iter().filter(|p|p["access"]==*access).map(|p|{
                let mut row=json!({"id":format!("popular-{}",str_of(&p["id"])),"title":p["title"],"line":p["summary"]});
                if p["added"]==true{row["kind"]=json!("status");row["value"]=json!("Added");}else{row["kind"]=json!("actions");row["actions"]=json!([{"id":"add","title":"Add"}]);}row
            }).collect();
            let mut card = group(&format!("popular-{access}"), rows);
            card["title"] = json!(match *access {
                "signIn" => "Popular Servers That Need an Account".into(),
                "open" => "No Account Needed".into(),
                _ => format!(
                    "Run on {}{}",
                    where_
                        .chars()
                        .next()
                        .map(|c| c.to_uppercase().to_string())
                        .unwrap_or_default(),
                    where_.get(1..).unwrap_or_default()
                ),
            });
            if accesses.last() == Some(access) {
                card["note"] = json!(
                    "Popular servers, each from the company behind it, as listed in the official MCP Registry. One that needs an account asks you to sign in once it's added."
                );
            }
            groups.push(card);
        }
    }
    let rows=[("startup","Time to start, in milliseconds","30000"),("catalog","Time to list tools, in milliseconds","30000"),("execution","Time for one tool call, in milliseconds","43200000")].into_iter().map(|(key,title,placeholder)|{
        let value=known.config["mcp"]["timeout"].get(key);
        let mut row=json!({"id":format!("timeout-{key}"),"title":title,"kind":"number","placeholder":placeholder,"min":1,"can":if value.is_none_or(Value::is_number){json!(true)}else{json!({"why":"This is set in the settings file as something other than a number. Change it there."})}});
        if let Some(v)=value.filter(|v|v.is_number()){row["value"]=v.clone();}row
    }).collect();
    let mut card = group("timeouts", rows);
    card["title"] = json!("Timeouts");
    card["note"] = json!("For every server, unless one sets its own.");
    groups.push(card);
    let mut page = page("mcp", "MCP Servers", groups);
    if known.servers.is_none() {
        page["notes"] = json!([{"words":format!("The engine isn’t running on {where_}, so how each server is doing isn’t known. They’re listed as the settings have them."),"tone":"warning"}]);
    }
    page
}
struct Checked {
    answer: Value,
    entry: Value,
    id: Option<String>,
}
async fn checked(
    ctx: &Context,
    known: &Facts,
    name: Option<&str>,
    values: &Value,
    before: &Value,
) -> Checked {
    let mut values = values.clone();
    let mut fields = Vec::new();
    let mut ready = true;
    let mut fill = None;
    for at in if name.is_none() {
        vec!["name", "command"]
    } else {
        vec!["command"]
    } {
        let setups = server_setups(str_of(&values[at]));
        if setups.len() > 1 {
            fields.push(json!({"id":at,"words":format!("This sets up {} servers ({}). Paste one server’s part at a time.",setups.len(),setups.iter().map(|s|s.name.as_deref().unwrap_or("unnamed")).collect::<Vec<_>>().join(", ")),"tone":"warning"}));
            ready = false;
        }
        if setups.len() == 1 {
            let setup = &setups[0];
            let mut filled = json!({"runs":if setup.url.is_some(){"remote"}else{"local"}});
            if at == "name" {
                filled["name"] = json!(setup.name.as_deref().unwrap_or_default());
            }
            if let Some(url) = &setup.url {
                filled["url"] = json!(url);
                filled["headers"] = json!(pairs(&json!(setup.headers)));
            } else {
                filled["command"] = json!(command_line(&setup.command));
                filled["environment"] = json!(pairs(&json!(setup.environment)));
            }
            if let Some(obj) = values.as_object_mut() {
                obj.extend(filled.as_object().unwrap().clone());
            }
            fields.push(json!({"id":if at=="name"{if setup.url.is_some(){"url"}else{"command"}}else{at},"words":"Filled in from the setup you pasted.","tone":"good"}));
            fill = Some(filled);
        }
    }
    let id = if name.is_none() {
        let typed = str_of(&values["name"]).trim();
        let id = typed
            .replace(' ', "-")
            .chars()
            .filter(|c| c.is_alphanumeric() || matches!(c, '_' | '-'))
            .collect::<String>();
        let warning = if id.is_empty() && !typed.is_empty() {
            Some("A name needs a letter or a number.".into())
        } else if id == "kuvo-browser" {
            Some("Kuvo Code uses that name for its own tools.".into())
        } else if let Some(title) = known.integrations.get(&id) {
            Some(format!(
                "{title} uses that name: add it in Integrations, or pick another name."
            ))
        } else if known.config["mcp"]["servers"].get(&id).is_some()
            || known
                .servers
                .as_ref()
                .is_some_and(|s| s.iter().any(|s| s["name"] == id))
        {
            Some(format!("There's a server called {id} already."))
        } else {
            None
        };
        if let Some(warning) = warning {
            fields.push(json!({"id":"name","words":warning,"tone":"warning"}));
            ready = false;
        } else if !id.is_empty() && id != typed {
            fields
                .push(json!({"id":"name","words":format!("It'll be called {id}."),"tone":"quiet"}));
        } else if id.is_empty() {
            fields.push(json!({"id":"name","words":if fill.is_some(){"Name it to add it with the setup you pasted."}else{"A short name without spaces. The server's tools are named after it."},"tone":"quiet"}));
            ready = false;
        }
        Some(id)
    } else {
        None
    };
    let remote = values["runs"] == "remote";
    let mut entry = before.as_object().cloned().unwrap_or_default();
    for key in ["command", "cwd", "environment", "url", "headers", "oauth"] {
        entry.shift_remove(key);
    }
    entry.insert(
        "type".into(),
        json!(if remote { "remote" } else { "local" }),
    );
    if remote {
        if before["oauth"].is_object() {
            entry.insert("oauth".into(), before["oauth"].clone());
        } else if values["oauth"] == false {
            entry.insert("oauth".into(), json!(false));
        }
        let typed = str_of(&values["url"]).trim();
        match web_address(typed) {
            Some(url) => {
                if url != typed {
                    fields.push(json!({"id":"url","words":format!("It'll be saved as {url}."),"tone":"quiet"}));
                }
                entry.insert("url".into(), json!(url));
            }
            None => {
                ready = false;
                fields.push(json!({"id":"url","words":if typed.is_empty(){"Its address, like https://example.com/mcp."}else{"That isn’t a web address. Give one like https://example.com/mcp."},"tone":if typed.is_empty(){"quiet"}else{"warning"}}));
            }
        }
    } else {
        let words = command_words(str_of(&values["command"]));
        if words.is_empty() {
            ready = false;
            if !fields.iter().any(|f| f["id"] == "command") {
                fields.push(
                    json!({"id":"command","words":"The command that starts it.","tone":"quiet"}),
                );
            }
        } else {
            if !fields.iter().any(|f| f["id"] == "command") {
                let name = percent_encoding::utf8_percent_encode(
                    &words[0],
                    percent_encoding::NON_ALPHANUMERIC,
                )
                .to_string();
                if let Ok(found) = ctx
                    .ask(Method::GET, &format!("kuvo/programs?name={name}"), None)
                    .await
                {
                    if found["path"].is_string() {
                        fields.push(json!({"id":"command","words":format!("{} is on {}.",str_of(&found["name"]),known.where_),"tone":"good"}));
                    }
                }
            }
            entry.insert("command".into(), json!(words));
        }
        let cwd = str_of(&values["cwd"]).trim();
        if !cwd.is_empty() {
            entry.insert("cwd".into(), json!(cwd));
        }
    }
    let field = if remote { "headers" } else { "environment" };
    let mut kept = serde_json::Map::new();
    for pair in values[field]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p.is_object())
    {
        let key = str_of(&pair["key"]).trim();
        let value = str_of(&pair["value"]);
        if key.is_empty() && value.is_empty() {
            continue;
        }
        let warning = if key.is_empty() {
            Some("Give each value a name.".into())
        } else if !remote
            && (!key
                .as_bytes()
                .first()
                .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
                || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        {
            Some(format!(
                "{key} isn't a variable's name: letters, digits and underscores, like GITHUB_TOKEN."
            ))
        } else if kept.contains_key(key) {
            Some(format!("{key} is given twice."))
        } else {
            None
        };
        if let Some(warning) = warning {
            ready = false;
            fields.push(json!({"id":field,"words":warning,"tone":"warning"}));
        } else {
            kept.insert(key.into(), json!(value));
        }
    }
    if !kept.is_empty() {
        entry.insert(field.into(), json!(kept));
    }
    entry.insert("codemode".into(), json!(values["codemode"] == true));
    let mut answer = json!({"ready":ready,"fields":fields});
    if let Some(fill) = fill {
        answer["values"] = fill;
    }
    Checked {
        answer,
        entry: Value::Object(entry),
        id,
    }
}
pub async fn check(ctx: &Context, at: &Value, action: &Value, values: &Value) -> Value {
    let Ok(known) = facts(ctx).await else {
        return json!({"ready":false,"fields":[]});
    };
    let name = if action["id"] == "edit" {
        str_of(&at["id"]).strip_prefix("server-")
    } else {
        None
    };
    let before = name
        .map(|n| &known.config["mcp"]["servers"][n])
        .unwrap_or(&Value::Null);
    checked(ctx, &known, name, values, before).await.answer
}
async fn write(ctx: &Context, path: Vec<&str>, value: Value, was: &Value) -> Result<(), Refusal> {
    ctx.ask(
        Method::PATCH,
        "kuvo/config",
        Some(json!({"changes":[{"path":path,"value":value,"was":was}]})),
    )
    .await?;
    Ok(())
}
pub async fn change(ctx: &Context, at: &Value, value: &Value) -> Result<(), Refusal> {
    let known = facts(ctx).await?;
    let id = str_of(&at["id"]);
    if let Some(name) = id.strip_prefix("server-") {
        let entry = &known.config["mcp"]["servers"][name];
        if !entry.is_object()
            || (entry["disabled"] != true) != at["value"].as_bool().unwrap_or_default()
        {
            return Err(stale(read(ctx).await));
        }
        if value == &at["value"] {
            return Ok(());
        }
        return write(
            ctx,
            vec!["mcp", "servers", name, "disabled"],
            if value == true {
                Value::Null
            } else {
                json!(true)
            },
            &entry["disabled"],
        )
        .await;
    }
    if let Some(key) = id
        .strip_prefix("timeout-")
        .filter(|k| matches!(*k, "startup" | "catalog" | "execution"))
    {
        let before = &known.config["mcp"]["timeout"][key];
        if before != &at["value"] {
            return Err(stale(read(ctx).await));
        }
        return write(
            ctx,
            vec!["mcp", "timeout", key],
            json!(value.as_f64().unwrap_or_default().floor() as u64),
            before,
        )
        .await;
    }
    Err(bad("That isn't something to change."))
}
pub async fn act(ctx: &Context, at: &Value, action: &Value, values: &Value) -> Result<(), Refusal> {
    let known = facts(ctx).await?;
    let id = str_of(&at["id"]);
    let action = str_of(&action["id"]);
    if let Some(name) = id.strip_prefix("popular-").filter(|_| action == "add") {
        ctx.ask(
            Method::POST,
            &format!(
                "kuvo/mcp/catalog/{}",
                percent_encoding::utf8_percent_encode(name, percent_encoding::NON_ALPHANUMERIC)
            ),
            None,
        )
        .await?;
        return Ok(());
    }
    let name = id.strip_prefix("server-");
    let before = name
        .map(|n| &known.config["mcp"]["servers"][n])
        .unwrap_or(&Value::Null);
    if action == "remove" {
        let name = name.ok_or_else(no_action)?;
        if !before.is_object() {
            return Err(stale(read(ctx).await));
        }
        return write(ctx, vec!["mcp", "servers", name], Value::Null, before).await;
    }
    if (id == "servers" && action == "add") || name.is_some() && action == "edit" {
        let checked = checked(ctx, &known, name, values, before).await;
        if checked.answer["ready"] != true {
            let words = checked.answer["fields"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|f| f["tone"] == "warning")
                .and_then(|f| f["words"].as_str())
                .unwrap_or("Fill in what it needs first.");
            return Err(Refusal::json(
                StatusCode::BAD_REQUEST,
                json!({"message":words,"fields":checked.answer["fields"]}),
            ));
        }
        let name = name.or(checked.id.as_deref()).ok_or_else(no_action)?;
        return write(ctx, vec!["mcp", "servers", name], checked.entry, before).await;
    }
    Err(no_action())
}
