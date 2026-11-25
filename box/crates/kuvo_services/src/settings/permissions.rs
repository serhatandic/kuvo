//! Permission pages use the approval service and the engine's existing config route.
use super::pages::said;
use super::*;

const NOT_A_LIST: &str =
    "The engine's permissions here aren't a list of rules: change them in its settings file.";
pub(super) const ACTIONS: [(&str, &str, Option<&str>); 8] = [
    (
        "edit",
        "Edit files",
        Some("Changing, creating or deleting files. Plan mode doesn't, by default."),
    ),
    ("shell", "Run commands", Some("In the terminal.")),
    (
        "read",
        "Read files",
        Some("Files ending in .env ask first unless you allow this."),
    ),
    (
        "external_directory",
        "Work outside the project",
        Some("In folders that aren't the project's."),
    ),
    ("webfetch", "Open web pages", None),
    ("websearch", "Search the web", None),
    ("subagent", "Hand work to subagents", None),
    ("skill", "Use skills", None),
];
pub(super) const EFFECTS: [(&str, &str); 3] = [
    ("allow", "Allow"),
    ("ask", "Ask First"),
    ("deny", "Don't Allow"),
];

pub(super) fn engine_default(action: &str) -> &'static str {
    if action == "external_directory" {
        "ask"
    } else {
        "allow"
    }
}
pub(super) fn choice<'a>(rules: &'a Value, action: &str) -> Option<&'a str> {
    rules
        .as_array()?
        .iter()
        .rev()
        .find(|r| r["action"] == action && r["resource"] == "*" && r["effect"].is_string())
        .and_then(|r| r["effect"].as_str())
}

pub async fn read(ctx: &Context) -> Value {
    let info = ctx
        .ask(Method::GET, "kuvo/info", None)
        .await
        .unwrap_or_default();
    let name = info["name"].as_str().unwrap_or("this computer");
    let mut groups = Vec::new();
    let mut notes = Vec::new();
    match ctx.ask(Method::GET,"kuvo/approval",None).await {
        Ok(approval) => {
            let mode = approval["default"].as_str().unwrap_or("undefined");
            let mode_name = match mode { "askFirst"|"auto"=>"Ask First", "acceptEdits"=>"Accept Edits","fullAccess"=>"Full Access",_=>mode };
            let full = approval["fullAccess"] == true;
            let mut access = json!({"id":"full-access","title":"Full Access","kind":"switch","value":full,
                "line":if full {"Conversations set to Full Access work here without asking."} else {"Asked the first time you pick it in a conversation on this computer."},
                "can":if full {json!(true)} else {json!({"why":"Turn it on in a conversation: pick Full Access in its mode menu, and confirm it there."})}});
            if full { access["confirm"] = json!({"title":"Turn Off Full Access?","body":format!("Conversations on {name} in Full Access go on in Auto. You're asked again the next time you pick it."),"button":"Turn Off"}); }
            let mut rows = vec![json!({"id":"default-mode","title":"New sessions start in","line":"The mode you picked last on this computer. Change it in a conversation's mode menu.","kind":"status","value":mode_name}),access];
            if let Some(value) = approval["otherComputers"].as_bool() {
                rows.push(json!({"id":"other-computers","title":"Let your other computers start work here","line":format!("Their bots and agents can start sessions on {name}."),"kind":"switch","value":value,"can":true}));
            }
            let mut card = group("approval",rows); card["title"] = json!("Approval"); groups.push(card);
        }
        Err(e) => notes.push(json!({"words":format!("Approval couldn't be read: {}",said(e,"it didn't answer.").await),"tone":"warning"})),
    }
    match ctx.ask(Method::GET,"kuvo/config",None).await {
        Ok(config) => {
            let listed = config.get("permissions").is_none_or(Value::is_array);
            let rows = ACTIONS.into_iter().map(|(id,title,line)| {
                let mut row = json!({"id":format!("when-{id}"),"title":title});
                if let Some(line) = line { row["line"] = json!(line); }
                if !listed {
                    row["kind"] = json!("status"); row["value"] = json!("Set in Its Settings File"); row["can"] = json!({"why":NOT_A_LIST});
                    return row;
                }
                let chosen = choice(&config["permissions"],id);
                let default = engine_default(id);
                let own_default = chosen == Some(default);
                let mut choices = EFFECTS.into_iter().map(|(effect,title)| {
                    if effect != default { json!({"id":effect,"title":title}) }
                    else if own_default {json!({"id":effect,"title":format!("{title} (Your Rule)"),"line":"Applies in every mode, Plan mode too."})}
                    else {json!({"id":effect,"title":format!("{title} (Default)")})}
                }).collect::<Vec<_>>();
                if let Some(chosen) = chosen.filter(|c| !EFFECTS.iter().any(|(id,_)| id == c)) {choices.push(json!({"id":chosen,"title":"Set by a Newer Engine"}));}
                if own_default {
                    let title = EFFECTS.iter().find(|(id,_)| *id == default).unwrap().1;
                    choices.push(json!({"id":"default","title":format!("{title} (Default)")}));
                }
                row["kind"] = json!("choice"); row["value"] = json!(chosen.unwrap_or(default)); row["choices"] = json!(choices); row["can"] = json!(true);
                row
            }).collect();
            let mut card = group("when",rows); card["title"] = json!("When the Agent Wants To"); card["note"] = json!(if listed {"For every session and mode. Don't Allow holds even in Full Access. Codex, Cursor, Grok and Antigravity may read files without asking."} else {NOT_A_LIST}); groups.push(card);
        }
        Err(e) => notes.push(json!({"words":said(e,"The engine's settings couldn't be read.").await,"tone":"warning"})),
    }
    notes.push(json!({"words":"Rules for particular commands or files, approvals saved with Always, and blocks aren't on this page yet.","tone":"quiet"}));
    json!({"id":"permissions","title":"Permissions","groups":groups,"notes":notes})
}

pub async fn change(ctx: &Context, at: &Value, value: &Value) -> Result<(), Refusal> {
    match at["id"].as_str().unwrap_or_default() {
        "full-access" => {
            if value != false {
                return Err(bad("Full Access is only turned off here."));
            }
            ctx.ask(
                Method::PATCH,
                "kuvo/approval",
                Some(json!({"fullAccess":false})),
            )
            .await?;
        }
        "other-computers" => {
            ctx.ask(
                Method::PATCH,
                "kuvo/approval",
                Some(json!({"otherComputers":value})),
            )
            .await?;
        }
        id if id.starts_with("when-") => {
            let action = &id[5..];
            let config = ctx.ask(Method::GET, "kuvo/config", None).await?;
            let before = &config["permissions"];
            if !before.is_null() && !before.is_array() {
                return Err(refused(StatusCode::FORBIDDEN, NOT_A_LIST));
            }
            if choice(before, action).unwrap_or(engine_default(action)) != at["value"] {
                return Err(stale(read(ctx).await));
            }
            if value == &at["value"] {
                return Ok(());
            }
            let mut items = before.as_array().cloned().unwrap_or_default();
            items.retain(|r| {
                !(r["action"] == action && r["resource"] == "*" && r["effect"].is_string())
            });
            if value != "default" && value != engine_default(action) {
                items.insert(0, json!({"action":action,"resource":"*","effect":value}));
            }
            if before.as_array() != Some(&items) {
                ctx.ask(Method::PATCH,"kuvo/config",Some(json!({"changes":[{"path":["permissions"],"value":if items.is_empty(){Value::Null}else{json!(items)},"was":before}]}))).await?;
            }
        }
        _ => return Err(bad("That isn't something to change.")),
    }
    Ok(())
}
