//! Engine agents, their settings, and the shared permission-rule form.
use super::pages::{computer_title, said};
use super::permissions::{ACTIONS, EFFECTS, choice, engine_default};
use super::*;

const BUILT_IN: [&str; 4] = ["build", "plan", "general", "explore"];
const HIDDEN: [&str; 3] = ["compaction", "title", "summary"];
const NOT_LIST: &str =
    "The engine's agents in its settings file aren't in a form Kuvo can change: change them there.";
const RULE_FIELDS: [(&str, &str, &str, &str); 3] = [
    (
        "allowed",
        "allow",
        "Allowed Without Asking",
        "Run commands: npm test *",
    ),
    ("asks", "ask", "Asks First", "Run commands: git push *"),
    ("never", "deny", "Never Allowed", "Edit files: *.pem"),
];
const OTHER: [&str; 6] = [
    "question",
    "browser",
    "todowrite",
    "tool.use",
    "provider.use",
    "integration.use",
];
const MODES: [(&str, &str, &str, &str); 3] = [
    (
        "primary",
        "Mode",
        "A mode new sessions can start in.",
        "New sessions can start in it, from the mode menu.",
    ),
    (
        "subagent",
        "Subagent",
        "It works when you name it in a message, or when another agent hands it part of the work.",
        "It works when you name it in a message, or when another agent hands it part of the work; its description tells them when.",
    ),
    (
        "all",
        "Both",
        "A mode, and a subagent other agents hand work to.",
        "New sessions can start in it, and it works when you name it or another agent hands it part of the work.",
    ),
];
fn built_in(id: &str) -> bool {
    BUILT_IN.contains(&id) || HIDDEN.contains(&id)
}
fn builtin_mode(id: &str) -> &'static str {
    match id {
        "build" | "plan" => "primary",
        "general" | "explore" => "subagent",
        _ => "all",
    }
}
fn builtin_words(id: &str) -> &'static str {
    match id {
        "build" => "The default agent: it can change files and run commands.",
        "plan" => "Plans and answers without changing files.",
        "general" => "Researches questions and runs multi-step tasks for another agent.",
        "explore" => "Finds files and code quickly, without changing anything.",
        _ => "",
    }
}
fn text(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}
struct Facts {
    config: Value,
    agents: Option<Vec<Value>>,
    models: Option<Vec<Value>>,
    where_: String,
}
async fn facts(ctx: &Context) -> Result<Facts, Refusal> {
    let config = ctx.ask(Method::GET, "kuvo/config", None).await?;
    let (agents, models) = tokio::join!(
        ctx.engine_get("api/agent"),
        ctx.settings.engine.settings_models()
    );
    let models = models.map(|models| {
        models.into_iter().map(|model|json!({
        "id":format!("{}/{}",model.provider,model.id),"title":model.name,"line":model.provider,
        "model":{"providerID":model.provider,"id":model.id}
    })).collect()
    });
    let agents = agents.and_then(|v| v.as_array().cloned());
    let title = computer_title(ctx).await;
    let where_ = match title.as_str() {
        "This Mac" => "this Mac".into(),
        "This Computer" => "this computer".into(),
        _ => title,
    };
    Ok(Facts {
        config,
        agents,
        models,
        where_,
    })
}
impl Facts {
    fn listed(&self) -> Vec<String> {
        let mut yours = self.config["agents"]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(id, v)| !built_in(id) && v.is_object())
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        yours.sort_by_key(|id| id.to_lowercase());
        BUILT_IN
            .into_iter()
            .map(str::to_owned)
            .chain(yours)
            .collect()
    }
    fn editable(&self) -> bool {
        self.config.get("agents").is_none_or(Value::is_object)
    }
    fn entry(&self, id: &str) -> &Value {
        self.config["agents"]
            .get(id)
            .filter(|v| v.is_object())
            .unwrap_or(&Value::Null)
    }
    fn engine(&self, id: &str) -> Option<&Value> {
        self.agents.as_ref()?.iter().find(|a| a["id"] == id)
    }
    fn name(&self, id: &str) -> String {
        if !built_in(id) {
            return id.into();
        }
        self.engine(id)
            .and_then(|v| v["name"].as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{}{}", id[..1].to_uppercase(), &id[1..]))
    }
    fn mode<'a>(&'a self, id: &str, entry: &'a Value) -> &'a str {
        entry["mode"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| self.engine(id).and_then(|v| v["mode"].as_str()))
            .unwrap_or_else(|| builtin_mode(id))
    }
    fn inherited(&self, id: &str, action: &str) -> &str {
        choice(&self.config["permissions"], action).unwrap_or_else(|| match (id, action) {
            ("plan", "edit")
            | ("general", "subagent")
            | ("explore", "edit" | "subagent" | "skill") => "deny",
            ("explore", "shell" | "read" | "external_directory" | "webfetch" | "websearch") => {
                "allow"
            }
            _ => engine_default(action),
        })
    }
}
fn is_rule(v: &Value) -> bool {
    v["action"].is_string() && v["resource"].is_string() && v["effect"].is_string()
}
fn is_choice(v: &Value) -> bool {
    is_rule(v) && v["resource"] == "*" && ACTIONS.iter().any(|(id, _, _)| v["action"] == *id)
}
fn title_of(action: &str) -> &str {
    ACTIONS
        .iter()
        .find(|(id, _, _)| *id == action)
        .map(|(_, title, _)| *title)
        .unwrap_or(action)
}
fn own_rules(entry: &Value) -> (Vec<&Value>, bool, bool) {
    if entry.get("permissions").is_some_and(|v| !v.is_array()) {
        return (vec![], false, false);
    }
    let all = entry["permissions"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let rules = all.iter().filter(|r| !is_choice(r)).collect::<Vec<_>>();
    let choices_first = all
        .iter()
        .skip_while(|r| is_choice(r))
        .all(|r| !is_choice(r));
    let rank = |r: &Value| {
        if is_rule(r) {
            RULE_FIELDS
                .iter()
                .position(|(_, effect, _, _)| r["effect"] == *effect)
        } else {
            None
        }
    };
    let ordered = rules.iter().enumerate().all(|(i, r)| {
        rank(r).is_some_and(|rank_r| {
            i == 0 || rank(rules[i - 1]).is_some_and(|previous| previous <= rank_r)
        })
    });
    (rules, choices_first && ordered, true)
}
fn parse_rules(typed: &str, effect: &str, known: &[&str]) -> Result<Vec<Value>, String> {
    let names = ACTIONS
        .iter()
        .flat_map(|(id, title, _)| [(*title, *id), (*id, *id)])
        .chain(known.iter().map(|n| (*n, *n)))
        .collect::<Vec<_>>();
    let mut rules = Vec::new();
    for line in typed.lines().map(str::trim).filter(|s| !s.is_empty()) {
        let lower = line.to_lowercase();
        let Some((name, id)) = names
            .iter()
            .filter(|(name, _)| lower.starts_with(&format!("{}:", name.to_lowercase())))
            .max_by_key(|(name, _)| name.len())
            .copied()
        else {
            return Err(format!(
                "“{line}” needs what it's for first, like Run commands: npm test *."
            ));
        };
        let resource = line[name.len() + 1..].trim();
        if resource.is_empty() {
            return Err(format!(
                "Add a pattern after {name}:, like Run commands: npm test *."
            ));
        }
        if resource == "*" && ACTIONS.iter().any(|(at, _, _)| *at == id) {
            return Err(format!("For all of them, set {} above.", title_of(id)));
        }
        rules.push(json!({"action":id,"resource":resource,"effect":effect}));
    }
    Ok(rules)
}
fn form(known: &Facts, id: Option<&str>) -> Value {
    let before = id.map(|id| known.entry(id)).unwrap_or(&Value::Null);
    let built = id.is_some_and(built_in);
    let (rules, editable, list) = own_rules(before);
    let lock = if !list {
        Some("Its permissions in the settings file aren't a list of rules: change them there.")
    } else if !editable {
        Some("Its permissions are in an order only the settings file keeps: change them there.")
    } else {
        None
    };
    let model = text(&before["model"]);
    let mut models = vec![
        json!({"id":"","title":"Default","line":"A mode uses the session's model; a subagent the one Settings › Models gives subagents."}),
    ];
    models.extend(known.models.clone().unwrap_or_default());
    if !model.is_empty() && !models.iter().any(|v| v["id"] == model) {
        models.push(json!({"id":model,"title":model,"line":if known.models.is_some(){"Not available here now"}else{"As the settings name it"}}));
    }
    let mut fields = Vec::new();
    if id.is_none() {
        fields.push(
            json!({"id":"name","title":"Name","kind":"text","value":"","placeholder":"Bug Hunter"}),
        );
    }
    fields.extend([
        json!({"id":"mode","title":"Use as","kind":"choice","value":id.map(|id|known.mode(id,before)).unwrap_or("subagent"),"choices":MODES.iter().map(|(id,title,line,_)|json!({"id":id,"title":title,"line":line})).collect::<Vec<_>>()}),
        json!({"id":"description","title":"Description","kind":"lines","value":text(&before["description"]),"placeholder":if built{builtin_words(id.unwrap())}else{"What it's for, like: Finds bugs and explains them, without changing code"},"line":"Other agents read it to know when to hand it work."}),
        json!({"id":"model","title":"Model","kind":"choice","value":model,"choices":models}),
    ]);
    if id.is_some() {
        fields.push(json!({"id":"on","title":"Use this agent","kind":"switch","value":before["disabled"]!=true}));
    }
    fields.push(json!({"id":"system","title":"Instructions","kind":"lines","value":text(&before["system"]),"placeholder":if built{"Its built-in instructions"}else{"How it works: what to look for, what to leave alone, how to report back."},"line":if built{"Leave empty to keep its built-in instructions. Anything here replaces them; the project's AGENTS.md, skills and MCP servers still apply."}else{"The project's AGENTS.md, skills and MCP servers apply too."}}));
    for (i, (action, title, _)) in ACTIONS.iter().enumerate() {
        let chosen = choice(&before["permissions"], action);
        let inherited = known.inherited(id.unwrap_or_default(), action);
        let mut choices=EFFECTS.iter().map(|(effect,title)|json!({"id":effect,"title":if *effect==inherited{format!("{title} (Default)")}else{title.to_string()}})).collect::<Vec<_>>();
        if let Some(chosen) = chosen.filter(|c| !EFFECTS.iter().any(|(effect, _)| effect == c)) {
            choices.push(json!({"id":chosen,"title":"Set by a Newer Engine"}));
        }
        let mut field = json!({"id":format!("may-{action}"),"title":title,"kind":"choice","value":chosen.unwrap_or(inherited),"choices":choices});
        if i == 0 {
            field["line"] = json!(
                "Default does what Settings › Permissions says. A choice here wins over it for this agent."
            );
        }
        if let Some(why) = lock {
            field["can"] = json!({"why":why});
        }
        fields.push(field);
    }
    for (i, (id, effect, title, placeholder)) in RULE_FIELDS.iter().enumerate() {
        let value = if editable {
            rules
                .iter()
                .filter(|r| is_rule(r) && r["effect"] == *effect)
                .map(|r| format!("{}: {}", title_of(text(&r["action"])), text(&r["resource"])))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            String::new()
        };
        let mut field = json!({"id":id,"title":title,"kind":"lines","mono":true,"value":value,"placeholder":placeholder});
        if i == 0 {
            field["line"] = json!(
                "For particular commands, files or sites, one a line: what it's for, then a pattern. * matches anything."
            );
        }
        if i == 2 {
            field["line"] = json!(
                "Never Allowed wins over Asks First, which wins over Allowed Without Asking."
            );
        }
        if let Some(why) = lock {
            field["can"] = json!({"why":why});
        }
        fields.push(field);
    }
    if let Some(id) = id {
        json!({"title":known.name(id),"submit":"Save","fields":fields})
    } else {
        json!({"title":"New Agent","submit":"Create Agent","fields":fields,"note":format!("On {}, for every project.",known.where_)})
    }
}
pub async fn read(ctx: &Context) -> Value {
    let known = match facts(ctx).await {
        Ok(v) => v,
        Err(e) => {
            return unread(
                "agents",
                "Agents",
                said(e, "This computer's settings couldn't be read.").await,
            );
        }
    };
    let editable = known.editable();
    let items=known.listed().iter().map(|id|{
        let entry=known.entry(id);let built=built_in(id);let mode=known.mode(id,entry);let name=known.name(id);
        let desc=entry["description"].as_str().filter(|s|!s.is_empty()).unwrap_or_else(||if built{builtin_words(id)}else{""}).lines().next().unwrap_or_default();
        let mode_line=match mode{"primary"=>"A mode","subagent"=>"A subagent","all"=>"A mode and a subagent",_=>mode};
        let mut actions=vec![json!({"id":"edit","title":"Edit…","form":form(&known,Some(id))})];
        if built&&entry.is_object(){actions.push(json!({"id":"reset","title":"Reset to Built-In…","style":"destructive","confirm":{"title":format!("Reset {name} to how it's built in?"),"body":"Your changes to it are removed.","button":"Reset"}}));}
        if !built{actions.push(json!({"id":"delete","title":"Delete Agent…","style":"destructive","confirm":{"title":format!("Delete {name}?"),"body":format!("It's removed from {}'s settings.",known.where_),"button":"Delete"}}));}
        let mut item=json!({"id":format!("agent-{id}"),"title":name,"line":([mode_line,desc].into_iter().filter(|s|!s.is_empty()).collect::<Vec<_>>().join(" · ")),"kind":"switch","value":entry["disabled"]!=true,"can":if editable{json!(true)}else{json!({"why":NOT_LIST})},"actions":actions});
        if built&&entry.is_object(){item["badge"]=json!({"words":"Changed","tone":"quiet"});}
        if !MODES.iter().any(|(id,_,_,_)|*id==mode){item["notice"]=json!({"words":format!("The engine leaves it out: “{mode}” isn't a way to use an agent. Pick one in Edit…."),"tone":"warning"});}item
    }).collect::<Vec<_>>();
    let mut list =
        json!({"id":"agents","title":"Agents","kind":"list","items":items,"empty":"None yet."});
    if editable {
        list["add"] = json!({"id":"add","title":"New Agent…","form":form(&known,None)});
    }
    let mut card = group("agents", vec![list]);
    card["title"] = json!("Agents");
    card["note"] = json!(
        "Plan is what /plan turns on. Your own agents work when you name one in a message, or when an agent hands one part of a task; Claude Code, Codex, Cursor, Grok and Antigravity get your subagents too."
    );
    let mut groups = vec![card];
    if let Ok(claude) = ctx.ask(Method::GET, "kuvo/claude-code", None).await
        && let Some(agents) = claude["agents"].as_array().filter(|a| !a.is_empty())
    {
        let mut card=group("claude-code",agents.iter().map(|each|{
            let mut row=json!({"id":format!("claude-{}",text(&each["path"][1])),"title":each["name"]});
            if let Some(line)=each["description"].as_str().and_then(|s|s.lines().next()).filter(|s|!s.is_empty()){row["line"]=json!(line);}
            if each["imported"]==true{row["kind"]=json!("status");row["value"]=json!("Imported");}else{row["kind"]=json!("actions");let mut action=json!({"id":"import","title":"Import"});if !editable{action["can"]=json!({"why":NOT_LIST});}row["actions"]=json!([action]);}row
        }).collect());
        card["title"] = json!("From Claude Code");
        card["note"] = json!(format!(
            "Agents in ~/.claude/agents on {}, brought over as subagents with their description and instructions. They use the session's model and the permissions here, not the tools and model Claude Code gives them.",
            known.where_
        ));
        groups.push(card);
    }
    let mut notes = Vec::new();
    if !editable {
        notes.push(json!({"words":NOT_LIST,"tone":"warning"}));
    }
    if known.agents.is_none() {
        notes.push(json!({"words":format!("The engine on {} isn’t answering, so the models to pick from and what each agent may do by default aren’t known. The agents are listed as the settings have them.",known.where_),"tone":"warning"}));
    }
    let mut page = page("agents", "Agents", groups);
    if !notes.is_empty() {
        page["notes"] = json!(notes);
    }
    page
}
struct Checked {
    answer: Value,
    changes: Vec<Value>,
}
fn checked(known: &Facts, id: Option<&str>, values: &Value) -> Checked {
    let mut fields = Vec::new();
    let mut ready = true;
    let mut target = id.unwrap_or_default().to_owned();
    if !known.editable() {
        return Checked {
            answer: json!({"ready":false,"fields":[{"id":"name","words":NOT_LIST,"tone":"warning"}]}),
            changes: vec![],
        };
    }
    if id.is_none() {
        let typed = text(&values["name"]).trim();
        target = typed
            .chars()
            .filter(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | ' '))
            .collect::<String>()
            .split(' ')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let lower = target.to_lowercase();
        let taken = known.config["agents"]
            .as_object()
            .into_iter()
            .flatten()
            .any(|(id, _)| id.to_lowercase() == lower)
            || built_in(&lower)
            || known.agents.as_ref().is_some_and(|a| {
                a.iter().any(|v| {
                    text(&v["id"]).to_lowercase() == lower
                        || text(&v["name"]).to_lowercase() == lower
                })
            });
        let warning = if target.is_empty() && !typed.is_empty() {
            Some("A name needs a letter or a number.".into())
        } else if target.encode_utf16().count() > 64 {
            Some("A name of 64 characters at most.".into())
        } else if matches!(
            target.as_str(),
            "__proto__" | "constructor" | "prototype" | "toString" | "valueOf" | "hasOwnProperty"
        ) {
            Some("Pick another name.".into())
        } else if taken {
            Some(format!("There's an agent called {target} already."))
        } else {
            None
        };
        if let Some(warning) = warning {
            fields.push(json!({"id":"name","words":warning,"tone":"warning"}));
            ready = false;
        } else if target.is_empty() {
            fields.push(json!({"id":"name","words":"How it shows in the composer, and how other agents ask for it.","tone":"quiet"}));
            ready = false;
        } else if target != typed {
            fields.push(
                json!({"id":"name","words":format!("It'll be called {target}."),"tone":"quiet"}),
            );
        }
    }
    let before = if id.is_some() {
        known.entry(&target)
    } else {
        &Value::Null
    };
    let mode = text(&values["mode"]);
    if let Some((_, _, _, detail)) = MODES.iter().find(|(id, _, _, _)| *id == mode) {
        fields.push(json!({"id":"mode","words":detail,"tone":"quiet"}));
    } else {
        fields.push(json!({"id":"mode","words":"Pick how it's used.","tone":"warning"}));
        ready = false;
    }
    let model = text(&values["model"]);
    if !model.is_empty()
        && model != text(&before["model"])
        && !known
            .models
            .as_ref()
            .is_some_and(|m| m.iter().any(|v| v["id"] == model))
    {
        fields.push(
            json!({"id":"model","words":format!("{model} isn't a model here."),"tone":"warning"}),
        );
        ready = false;
    }
    let (own, editable, _) = own_rules(before);
    let mut entry = before.as_object().cloned().unwrap_or_default();
    if editable {
        let mut permissions = Vec::new();
        for (action, _, _) in ACTIONS {
            let field = format!("may-{action}");
            let chosen = choice(&before["permissions"], action);
            let inherited = known.inherited(&target, action);
            let picked = values[&field].as_str();
            if picked.is_none_or(|pick| {
                !EFFECTS.iter().any(|(effect, _)| *effect == pick) && chosen != Some(pick)
            }) {
                fields.push(json!({"id":field,"words":"Pick one.","tone":"warning"}));
                ready = false;
                continue;
            }
            let picked = picked.unwrap();
            if picked == inherited && chosen != Some(picked) {
                continue;
            }
            permissions.push(json!({"action":action,"resource":"*","effect":picked}));
        }
        let known = OTHER
            .into_iter()
            .chain(
                own.iter()
                    .filter(|r| is_rule(r))
                    .map(|r| text(&r["action"])),
            )
            .collect::<Vec<_>>();
        for (field, effect, _, _) in RULE_FIELDS {
            match parse_rules(text(&values[field]), effect, &known) {
                Ok(rules) => permissions.extend(rules),
                Err(words) => {
                    fields.push(json!({"id":field,"words":words,"tone":"warning"}));
                    ready = false;
                }
            }
        }
        if permissions.is_empty() {
            entry.shift_remove("permissions");
        } else {
            entry.insert("permissions".into(), json!(permissions));
        }
    }
    if built_in(&target) && mode == builtin_mode(&target) && before.get("mode").is_none() {
        entry.shift_remove("mode");
    } else {
        entry.insert("mode".into(), json!(mode));
    }
    for (key, value) in [
        ("description", text(&values["description"]).trim()),
        ("model", model),
        ("system", text(&values["system"]).trim()),
    ] {
        if !value.is_empty() {
            entry.insert(key.into(), json!(value));
        } else if before[key].is_string() {
            entry.shift_remove(key);
        }
    }
    if id.is_some() && values["on"] == false {
        entry.insert("disabled".into(), json!(true));
    } else {
        entry.shift_remove("disabled");
    }
    let mut changes = Vec::new();
    if ready {
        let reset = entry.is_empty() && built_in(&target);
        let remove_default = (entry.get("disabled") == Some(&Value::Bool(true))
            || entry.get("mode").and_then(Value::as_str) == Some("subagent"))
            && known.config["default_agent"] == target;
        changes.push(json!({"path":["agents",target],"value":if reset{Value::Null}else{Value::Object(entry)},"was":before}));
        if remove_default {
            changes.push(json!({"path":["default_agent"],"value":null,"was":target}));
        }
    }
    Checked {
        answer: json!({"ready":ready,"fields":fields}),
        changes,
    }
}
pub async fn check(ctx: &Context, at: &Value, action: &Value, values: &Value) -> Value {
    let Ok(known) = facts(ctx).await else {
        return json!({"ready":false,"fields":[]});
    };
    checked(
        &known,
        if action["id"] == "edit" {
            text(&at["id"]).strip_prefix("agent-")
        } else {
            None
        },
        values,
    )
    .answer
}
async fn write(ctx: &Context, changes: Vec<Value>) -> Result<(), Refusal> {
    ctx.ask(
        Method::PATCH,
        "kuvo/config",
        Some(json!({"changes":changes})),
    )
    .await?;
    Ok(())
}
pub async fn change(ctx: &Context, at: &Value, value: &Value) -> Result<(), Refusal> {
    let known = facts(ctx).await?;
    let id = text(&at["id"])
        .strip_prefix("agent-")
        .filter(|id| known.listed().iter().any(|s| s == id))
        .ok_or_else(|| bad("That isn't something to change."))?;
    let entry = known.entry(id);
    if (entry["disabled"] != true) != at["value"].as_bool().unwrap_or_default() {
        return Err(stale(read(ctx).await));
    }
    if value == &at["value"] {
        return Ok(());
    }
    let keeps = !built_in(id) && entry.as_object().is_some_and(|e| e.len() == 1);
    let mut changes = vec![
        json!({"path":["agents",id,"disabled"],"value":if value==true{if keeps{json!(false)}else{Value::Null}}else{json!(true)},"was":entry["disabled"]}),
    ];
    if value == false && known.config["default_agent"] == id {
        changes.push(json!({"path":["default_agent"],"value":null,"was":id}));
    }
    write(ctx, changes).await
}
pub async fn act(ctx: &Context, at: &Value, action: &Value, values: &Value) -> Result<(), Refusal> {
    let known = facts(ctx).await?;
    let id = text(&at["id"]);
    let action = text(&action["id"]);
    if let Some(id) = id.strip_prefix("claude-").filter(|_| action == "import") {
        let claude = ctx.ask(Method::GET, "kuvo/claude-code", None).await?;
        let found = claude["agents"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|v| v["path"][1] == id && v["imported"] != true)
            .ok_or_else(|| stale(json!({})))?;
        return write(
            ctx,
            vec![json!({"path":found["path"],"value":found["settings"],"was":null})],
        )
        .await;
    }
    let name = id.strip_prefix("agent-");
    if action == "add" && id == "agents" || action == "edit" && name.is_some() {
        let checked = checked(&known, name, values);
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
        return write(ctx, checked.changes).await;
    }
    if matches!(action, "reset" | "delete")
        && let Some(id) = name
    {
        let before = known.entry(id);
        if !before.is_object() || (action == "reset") != built_in(id) {
            return Err(stale(read(ctx).await));
        }
        let mut changes = vec![json!({"path":["agents",id],"value":null,"was":before})];
        if action == "delete" && known.config["default_agent"] == id {
            changes.push(json!({"path":["default_agent"],"value":null,"was":id}));
        }
        return write(ctx, changes).await;
    }
    Err(no_action())
}
