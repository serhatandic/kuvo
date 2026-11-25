//! The vault page contains metadata and masked form fields only. Its operations
//! run through the vault routes as the originally authenticated client.
use super::pages::computer_title;
use super::*;
use crate::vault::{Grant, How, Item, Kind, Who, site_of};
use serde::Deserialize;
use serde_json::Map;
use std::collections::BTreeMap;

const CANT_CONFIRM: &str = "This needs Touch ID or your Mac’s password. Update Kuvo to do it here.";
const KINDS: [Kind; 4] = [Kind::Login, Kind::Key, Kind::File, Kind::Note];
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct State {
    made: bool,
    locked: bool,
    paused: bool,
    paused_bots: BTreeMap<String, String>,
    store: String,
    agents: bool,
    #[serde(default)]
    agents_waiting: bool,
    #[serde(default)]
    tampered: bool,
}
#[derive(Deserialize)]
struct Facts {
    state: State,
    items: Vec<Item>,
    grants: Vec<Grant>,
    #[serde(default)]
    asks: Vec<Value>,
    #[serde(default)]
    activity: Vec<Value>,
    #[serde(default)]
    bots: Vec<Bot>,
}
#[derive(Deserialize)]
struct Bot {
    id: String,
    name: String,
}
async fn facts(ctx: &Context, activity: bool) -> Result<Facts, Refusal> {
    let mut known: Facts = serde_json::from_value(ctx.ask(Method::GET, "kuvo/vault", None).await?)
        .map_err(|e| refused(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    known.items.sort_by_key(|i| i.name.to_lowercase());
    if let Ok(asks) = ctx.ask(Method::GET, "kuvo/vault/asks", None).await {
        known.asks = asks["asks"].as_array().cloned().unwrap_or_default();
    }
    if activity
        && known.state.made
        && !known.state.locked
        && let Ok(lines) = ctx
            .ask(Method::GET, "kuvo/vault/activity?limit=50", None)
            .await
    {
        known.activity = lines["entries"].as_array().cloned().unwrap_or_default();
    }
    if let Ok(bots) = ctx.ask(Method::GET, "kuvo/bots", None).await {
        known.bots = bots["bots"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|b| serde_json::from_value(b.clone()).ok())
            .collect();
        known.bots.sort_by_key(|b| b.name.to_lowercase());
    }
    Ok(known)
}
fn text(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}
fn kind_id(kind: Kind) -> &'static str {
    match kind {
        Kind::Login => "login",
        Kind::Key => "key",
        Kind::File => "file",
        Kind::Note => "note",
    }
}
fn kind_title(kind: Kind) -> &'static str {
    match kind {
        Kind::Login => "Sign-in",
        Kind::Key => "API Key",
        Kind::File => "File",
        Kind::Note => "Note",
    }
}
fn capitalized(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map(|c| format!("{}{}", c.to_uppercase(), chars.as_str()))
        .unwrap_or_default()
}
async fn computer(ctx: &Context) -> String {
    let title = computer_title(ctx).await;
    match title.as_str() {
        "This Mac" => "this Mac".into(),
        "This Computer" => "this computer".into(),
        _ => title,
    }
}
fn line(item: &Item) -> String {
    let mut parts = vec![kind_title(item.kind).to_owned()];
    match item.kind {
        Kind::Login => {
            parts.extend(item.username.clone());
            parts.extend(item.sites.first().cloned());
        }
        Kind::File => {
            if let Some(name) = item.file_name.as_ref().filter(|n| *n != &item.name) {
                parts.push(name.clone());
            }
        }
        Kind::Note => {
            if item.readable == Some(true) {
                parts.push("Bots may read it".into());
            }
        }
        Kind::Key => {
            if !item.sites.is_empty() {
                parts.push(item.sites.join(", "));
            } else if item.fields.len() > 1 {
                parts.push(item.fields.join(", "));
            }
        }
    }
    parts
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}
fn label(s: &str) -> String {
    s.split('_')
        .filter(|s| !s.is_empty())
        .map(capitalized)
        .collect::<Vec<_>>()
        .join(" ")
}
fn show(item: &Item, field: &str) -> Value {
    json!({"route":format!("kuvo/vault/items/{}/values",item.id),"field":field})
}
fn form_fields(item: Option<&Item>) -> Vec<Value> {
    let mut fields = Vec::new();
    if item.is_none() {
        fields.push(json!({"id":"kind","title":"Kind","kind":"choice","value":"login","choices":KINDS.into_iter().map(|k|json!({"id":kind_id(k),"title":kind_title(k)})).collect::<Vec<_>>()}));
    }
    fields.push(json!({"id":"name","title":"Name","kind":"text","value":item.map(|i|i.name.as_str()).unwrap_or_default(),"placeholder":"What it’s for, like GitHub"}));
    let mut add_kind = |kind: Kind, mut specific: Vec<Value>| {
        if item.is_none_or(|i| i.kind == kind) {
            if item.is_none() {
                for field in &mut specific {
                    field["shown"] = json!({"field":"kind","is":[kind_id(kind)]});
                }
            }
            fields.extend(specific);
        }
    };
    let sites = item.map(|i| i.sites.join(", ")).unwrap_or_default();
    let mut password = json!({"id":"password","title":"Password","kind":"secret"});
    let mut value = json!({"id":"value","title":"Value","kind":"secret"});
    let mut note = json!({"id":"text","title":"Text","kind":"secret","multi":true});
    if let Some(item) = item {
        password["placeholder"] = json!("Unchanged");
        value["placeholder"] = json!("Unchanged");
        note["placeholder"] = json!("Unchanged. Type here to replace it.");
        for (field, at) in [
            ("password", &mut password),
            ("value", &mut value),
            ("text", &mut note),
        ] {
            if item.fields.iter().any(|f| f == field) {
                at["shows"] = show(item, field);
            }
        }
    }
    add_kind(
        Kind::Login,
        vec![
            json!({"id":"site","title":"Site","kind":"text","value":sites,"placeholder":"github.com","line":"It’s filled in only on this site and those under it."}),
            json!({"id":"username","title":"Username","kind":"text","value":item.and_then(|i|i.username.as_deref()).unwrap_or_default(),"placeholder":"Email or username"}),
            password,
            json!({"id":"signInOnly","title":"Only for Signing In","kind":"switch","value":item.is_none_or(|i|i.sign_in_only==Some(true)),"line":"Agents sign in with it in the browser, and can’t hand it to a command."}),
        ],
    );
    let mut key_fields = vec![
        json!({"id":"hosts","title":"Hosts","kind":"text","value":sites,"placeholder":"api.example.com (optional)"}),
        value,
    ];
    if let Some(item) = item.filter(|i| i.kind == Kind::Key) {
        let extras = item
            .fields
            .iter()
            .filter(|f| f.as_str() != "value")
            .collect::<Vec<_>>();
        for field in &extras {
            key_fields.push(json!({"id":format!("field-{field}"),"title":label(field),"kind":"secret","placeholder":"Unchanged","shows":show(item,field)}));
        }
        if !extras.is_empty() {
            let mut choices = vec![json!({"id":"none","title":"None"})];
            choices.extend(extras.iter().map(|f| json!({"id":f,"title":label(f)})));
            key_fields.push(json!({"id":"remove","title":"Remove a Field","kind":"choice","value":"none","choices":choices}));
        }
    }
    key_fields.push(json!({"id":"more","title":if item.is_some(){"Add Fields"}else{"Other Fields"},"kind":"pairs","secret":true,"value":[],"pairs":{"key":"field_name","value":"Value"},"line":"For a key with more than one part, like an issuer ID."}));
    add_kind(Kind::Key, key_fields);
    let mut file = json!({"id":"file","title":"File","kind":"file","file":{"choose":"Choose File…","none":"No file chosen"},"line":"At most about 700 KB."});
    if let Some(name) = item.and_then(|i| i.file_name.as_deref()) {
        file["value"] = json!({"name":name});
    }
    add_kind(Kind::File, vec![file]);
    add_kind(
        Kind::Note,
        vec![
            note,
            json!({"id":"readable","title":"Bots May Read It","kind":"switch","value":item.is_some_and(|i|i.readable==Some(true)),"line":"For what isn’t a secret, like a contact for App Review."}),
        ],
    );
    fields
}
fn kept_words(state: &State, computer: &str) -> String {
    if state.store == "keychain" {
        format!(
            "Kept in {computer}’s Keychain. Agents fill them in or hand them to a command without ever seeing them, only as you allow."
        )
    } else {
        format!(
            "Kept on {computer}, encrypted, with a key only the Kuvo that made it keeps. Agents fill them in or hand them to a command without ever seeing them, only as you allow. Its browser runs as you, so a program you or an agent runs there could get an item an agent there may use."
        )
    }
}
fn confirms(ctx: &Context) -> bool {
    ctx.request
        .headers
        .get("x-kuvo-confirms-you")
        .is_some_and(|v| v == "1")
}
fn who_id(who: &Who) -> String {
    match who {
        Who::Bot { bot } => format!("bot-{bot}"),
        Who::Project { project } => format!("project-{project}"),
        Who::Everyone { .. } => "everyone".into(),
    }
}
fn rank(how: &str) -> u8 {
    match how {
        "own" => 2,
        "ask" => 1,
        _ => 0,
    }
}
fn how_of(grants: &[&Grant], who: &Who) -> &'static str {
    grants
        .iter()
        .find(|g| g.who == *who)
        .map(|g| match g.how {
            How::Own => "own",
            How::Ask => "ask",
        })
        .unwrap_or("no")
}
fn who_form(known: &Facts, item: &Item, confirms: bool) -> Value {
    let grants = known
        .grants
        .iter()
        .filter(|g| g.item == item.id)
        .collect::<Vec<_>>();
    let listed = known
        .bots
        .iter()
        .filter(|b| {
            grants
                .iter()
                .any(|g| matches!(&g.who, Who::Bot{bot} if bot==&b.id))
        })
        .collect::<Vec<_>>();
    let projects = grants
        .iter()
        .filter_map(|g| {
            if let Who::Project { project } = &g.who {
                Some(project)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let choose = |who: Who, title: String, line: Option<&str>| {
        let now = how_of(&grants, &who);
        let choices = [
            ("own", "On Its Own"),
            ("ask", "Asks Each Time"),
            ("no", "No"),
        ]
        .into_iter()
        .filter(|(id, _)| confirms || rank(id) <= rank(now))
        .map(|(id, title)| {
            let mut choice = json!({"id":id,"title":title});
            if rank(id) > rank(now) {
                choice["verified"] = json!(true);
            }
            choice
        })
        .collect::<Vec<_>>();
        let mut field =
            json!({"id":who_id(&who),"title":title,"kind":"choice","value":now,"choices":choices});
        if let Some(line) = line {
            field["line"] = json!(line);
        }
        if !confirms && now == "no" {
            field["can"] = json!({"why":CANT_CONFIRM});
        }
        field
    };
    let mut fields = listed
        .iter()
        .map(|bot| {
            choose(
                Who::Bot {
                    bot: bot.id.clone(),
                },
                bot.name.clone(),
                None,
            )
        })
        .collect::<Vec<_>>();
    for project in &projects {
        fields.push(choose(
            Who::Project {
                project: (*project).clone(),
            },
            std::path::Path::new(project)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| (*project).clone()),
            Some(project),
        ));
    }
    fields.push(choose(
        Who::Everyone { everyone: true },
        if listed.is_empty() && projects.is_empty() {
            "Every agent".into()
        } else {
            "Other sessions and bots".into()
        },
        None,
    ));
    let others = known
        .bots
        .iter()
        .filter(|bot| !listed.iter().any(|b| b.id == bot.id))
        .collect::<Vec<_>>();
    if !others.is_empty() {
        let mut choices = vec![json!({"id":"none","title":"No One Else"})];
        choices.extend(
            others
                .into_iter()
                .map(|b| json!({"id":b.id,"title":b.name,"verified":true})),
        );
        let mut field = json!({"id":"add-bot","title":"Let a Bot Use It","kind":"choice","value":"none","line":"It asks you each time, until you choose On Its Own.","choices":choices});
        if !confirms {
            field["can"] = json!({"why":CANT_CONFIRM});
        }
        fields.push(field);
    }
    let note = if item.kind == Kind::Login {
        format!(
            "Only on {}. Agents never see the password: Kuvo fills it in.",
            item.sites.join(", ")
        )
    } else {
        "Agents never see it: Kuvo fills it in, or hands it to a command they run.".into()
    };
    json!({"title":format!("Who Can Use “{}”",item.name),"submit":"Save","fields":fields,"note":note})
}
fn may_always(ask: &Value) -> bool {
    ask.get("group").is_none_or(Value::is_null) && ask["always"] != false
}
fn ask_row(ask: &Value, known: &Facts, confirms: bool) -> Value {
    let asker = ask["botName"]
        .as_str()
        .or_else(|| ask["sessionName"].as_str())
        .unwrap_or("An agent");
    let use_item = ask["kind"] == "use";
    let title = if use_item {
        format!(
            "{asker} wants to use “{}”",
            ask["itemName"].as_str().unwrap_or("an item")
        )
    } else {
        format!(
            "{asker} needs {}",
            ask["need"].as_str().unwrap_or("something from your vault")
        )
    };
    let mut row = json!({"id":format!("ask-{}",text(&ask["id"])),"title":title,"kind":"actions"});
    let words = ask["why"]
        .as_str()
        .map(str::to_owned)
        .into_iter()
        .chain(ask["site"].as_str().map(|s| format!("On {s}")))
        .collect::<Vec<_>>()
        .join(" · ");
    if !words.is_empty() {
        row["line"] = json!(words);
    }
    let mut actions = vec![json!({"id":"no","title":"Don’t Allow"})];
    if use_item {
        actions.push(json!({"id":"once","title":"Allow Once"}));
        if may_always(ask) {
            let mut action = json!({"id":"always","title":"Always Allow…","verified":true,"confirm":{"title":format!("Let {asker} use “{}” on its own?",ask["itemName"].as_str().unwrap_or("it")),"body":"From now on it uses it without asking you. Change that in its Who Can Use It.","button":"Always Allow"}});
            if !confirms {
                action["can"] = json!({"why":CANT_CONFIRM});
            }
            actions.push(action);
        }
    } else if known.items.is_empty() {
        actions.push(json!({"id":"give","title":"Give…","can":{"why":"There’s nothing in the vault to give yet."}}));
    } else {
        let mut fields = vec![
            json!({"id":"item","title":"Item","kind":"choice","value":"","choices":known.items.iter().map(|i|json!({"id":i.id,"title":i.name,"line":line(i)})).collect::<Vec<_>>()}),
        ];
        if may_always(ask) {
            let mut field = json!({"id":"how","title":"For","kind":"choice","value":"once","choices":[{"id":"once","title":"This Once"},{"id":"always","title":"From Now On","verified":true}]});
            if !confirms {
                field["can"] = json!({"why":CANT_CONFIRM});
            }
            fields.push(field);
        }
        let mut form =
            json!({"title":format!("Give {asker} an Item"),"submit":"Give","fields":fields});
        if let Some(need) = ask["need"].as_str() {
            form["note"] = json!(format!("It needs {need}."));
        }
        actions.push(json!({"id":"give","title":"Give…","form":form}));
    }
    row["actions"] = json!(actions);
    row
}
fn activity_sentence(entry: &Value) -> String {
    let what = text(&entry["what"]);
    let how = text(&entry["how"]);
    let agent = entry["botName"]
        .as_str()
        .or_else(|| entry["sessionName"].as_str());
    let named = entry["itemName"].as_str().map(|n| format!("“{n}”"));
    let thing = named.as_deref().unwrap_or("an item");
    let actor = agent.unwrap_or("an agent");
    let title = match what {
        "added" => "Added",
        "changed" => "Changed",
        "removed" => "Removed",
        "shown" => "Looked at",
        _ => "",
    };
    if entry["mine"] == true {
        match what {
            "allowed" => {
                return format!(
                    "Let {actor} use {thing}{}",
                    if how == "always" {
                        " from now on"
                    } else {
                        " once"
                    }
                );
            }
            "declined" => return format!("Said no to {actor} using {thing}"),
            _ if !title.is_empty() => return format!("{title} {thing}"),
            _ => {}
        }
    }
    if entry.get("client").is_none() {
        match what {
            "allowed" => {
                return format!(
                    "{} {}",
                    agent.unwrap_or("An agent"),
                    if how == "always" {
                        format!("may use {thing} from now on")
                    } else {
                        format!("was allowed {thing} once")
                    }
                );
            }
            "declined" => return format!("{} wasn’t allowed {thing}", agent.unwrap_or("An agent")),
            "added" | "changed" | "removed" | "shown" => {
                return format!(
                    "{} was {}",
                    named.as_deref().unwrap_or("An item"),
                    match what {
                        "added" => "added",
                        "changed" => "changed",
                        "removed" => "removed",
                        _ => "looked at",
                    }
                );
            }
            _ => {}
        }
    }
    let you = entry["client"]
        .as_str()
        .map(|c| format!("You, on {},", if c == "Mac" { "your Mac" } else { c }))
        .unwrap_or("You".into());
    let who = agent.unwrap_or(if entry.get("client").is_none() {
        "An agent"
    } else {
        &you
    });
    let where_ = entry["site"]
        .as_str()
        .map(|s| format!(" on {s}"))
        .or_else(|| entry["program"].as_str().map(|p| format!(" with {p}")))
        .unwrap_or_default();
    match what {
        "used" => format!(
            "{who} used {thing}{where_}{}",
            match how {
                "once" => ", allowed once",
                "always" => ", now always allowed",
                _ => "",
            }
        ),
        "asked" => {
            if named.is_none() {
                format!("{who} asked for something from the vault")
            } else {
                format!("{who} asked to use {thing}{where_}")
            }
        }
        "allowed" => format!(
            "{you} let {actor} use {thing}{}",
            if how == "always" {
                " from now on"
            } else {
                " once"
            }
        ),
        "declined" => format!("{you} said no to {actor} using {thing}"),
        "refused" => match how {
            "wrong site" => format!(
                "Refused {thing}{where_}: not one of its sites. {} was paused",
                entry["botName"].as_str().unwrap_or("The bot")
            ),
            "paused" => format!("{who} couldn’t use {thing}: paused"),
            "autopilot" => {
                if named.is_none() {
                    format!("{who} went on without something from your vault, on Autopilot")
                } else {
                    format!("{who} left {thing} for you, on Autopilot")
                }
            }
            _ => format!("{who} wasn’t allowed {thing}"),
        },
        "added" | "changed" | "removed" | "shown" => {
            format!("{you} {} {thing}", title.to_lowercase())
        }
        "granted" => format!(
            "{} is now {} by {}",
            capitalized(thing),
            if how == "own" {
                "used on its own"
            } else {
                "asked for each time"
            },
            entry["botName"].as_str().unwrap_or("everyone allowed")
        ),
        "ungranted" => format!(
            "{} taken away from {}",
            capitalized(thing),
            entry["botName"].as_str().unwrap_or("an agent")
        ),
        "paused" => entry["botName"]
            .as_str()
            .map(|b| format!("{b}’s use of the vault was paused"))
            .unwrap_or("The vault was paused".into()),
        "resumed" => entry["botName"]
            .as_str()
            .map(|b| format!("{b}’s use of the vault goes on again"))
            .unwrap_or("The vault goes on again".into()),
        _ => format!("{who}: {what} {thing}"),
    }
}
pub async fn read(ctx: &Context) -> Value {
    let computer = computer(ctx).await;
    let known = match facts(ctx, !ctx.quiet).await {
        Ok(k) => k,
        Err(e) => {
            let reply = e.reply();
            let status = reply.status;
            let body = reply
                .body
                .collect()
                .await
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b.to_bytes()).ok())
                .unwrap_or_default();
            return unread(
                "vault",
                "Vault",
                if status == StatusCode::FORBIDDEN {
                    format!("Only Kuvo on your devices can open {computer}’s vault.")
                } else {
                    body["message"]
                        .as_str()
                        .unwrap_or("This computer's settings couldn't be read.")
                        .to_owned()
                },
            );
        }
    };
    let mut notes = Vec::new();
    let mut groups = Vec::new();
    let state = &known.state;
    if state.locked {
        return unread(
            "vault",
            "Vault",
            format!(
                "{}’s vault is locked since it started again, so its agents can’t use it. Unlocking it takes the key it was made with, which this app doesn’t keep.",
                if computer.starts_with("this ") {
                    capitalized(&computer)
                } else {
                    computer.clone()
                }
            ),
        );
    }
    if state.tampered {
        notes.push(json!({"words":"Its files were changed outside Kuvo, so it uses none of them, and every agent’s use is paused. Add its items again.","tone":"warning"}));
    }
    if !state.agents {
        notes.push(json!({"words":if state.agents_waiting{"Locked for now: some agents started before the sandbox was on. The vault opens when they finish.".into()}else{format!("Agents can’t use keys, files and notes from {computer}’s vault yet. Sign-ins you let them use on their own still fill.")},"tone":"quiet"}));
    }
    let confirms = confirms(ctx);
    if !known.asks.is_empty() {
        let mut card = group(
            "asks",
            known
                .asks
                .iter()
                .map(|ask| ask_row(ask, &known, confirms))
                .collect(),
        );
        card["title"] = json!("Waiting for You");
        groups.push(card);
    }
    let items=known.items.iter().map(|item|{
        let mut row=json!({"id":format!("item-{}",item.id),"title":item.name,"line":line(item),"kind":"actions","actions":[
            {"id":"edit","title":"Edit…","form":{"title":item.name,"submit":"Save","fields":form_fields(Some(item)),"note":"A value left empty stays as it is."}},
            {"id":"who","title":"Who Can Use It…","form":who_form(&known,item,confirms)},
            {"id":"delete","title":"Delete…","style":"destructive","confirm":{"title":format!("Delete “{}” from the vault?",item.name),"body":"Agents can’t use it any more. The account or service it’s for isn’t affected.","button":"Delete"}}
        ]});
        let grants=known.grants.iter().filter(|g|g.item==item.id).collect::<Vec<_>>();
        if grants.iter().any(|g|g.how==How::Own){row["badge"]=json!({"words":"On its own","tone":"good"});}else if !grants.is_empty(){row["badge"]=json!({"words":"Asks","tone":"quiet"});}row
    }).collect::<Vec<_>>();
    let mut add = json!({"id":"add","title":"Add Item…","form":{"title":"Add Item","submit":"Add","fields":form_fields(None),"note":kept_words(state,&computer)}});
    if !state.made {
        add["can"] = json!({"why":format!("There’s no vault on {computer} yet. This app can’t make one: it doesn’t keep the key that unlocks a vault after its computer restarts.")});
    }
    let mut list = json!({"id":"items","title":"Items","kind":"list","items":items,"add":add,"empty":"Nothing yet. Add a sign-in, a key, a file or a note for your agents to use."});
    if items.len() > 6 {
        list["search"] = json!({"placeholder":"Search the vault","empty":"Nothing in the vault matches “{query}”."});
    }
    let mut card = group("items", vec![list]);
    card["title"] = json!("Items");
    card["note"] = json!(kept_words(state, &computer));
    groups.push(card);
    if state.made {
        let mut rows = vec![
            json!({"id":"pause","title":"Pause Agents’ Use","line":format!("Stops every agent on {computer} from using the vault at once. What they ask still comes to you."),"kind":"switch","value":state.paused,"can":true}),
        ];
        for (who, why) in &state.paused_bots {
            let name = known
                .bots
                .iter()
                .find(|b| b.id == *who)
                .map(|b| b.name.as_str())
                .unwrap_or(if who.starts_with("bot_") {
                    "A bot"
                } else {
                    "A conversation"
                });
            rows.push(json!({"id":format!("paused-{who}"),"title":format!("{name} is paused"),"line":why,"kind":"actions","actions":[{"id":"go-on","title":"Let Go On"}]}));
        }
        groups.push(group("pause", rows));
        let zone = ctx
            .settings
            .setup
            .time_zone()
            .and_then(|(name, _)| jiff::tz::TimeZone::get(&name).ok())
            .unwrap_or_else(jiff::tz::TimeZone::system);
        let items=known.activity.iter().map(|entry|{
            let mut entry=entry.clone();if entry.get("botName").is_none()&&let Some(bot)=known.bots.iter().find(|b|entry["bot"]==b.id){entry["botName"]=json!(bot.name);}
            let when=entry["at"].as_i64().and_then(|at|jiff::Timestamp::from_millisecond(at).ok()).map(|at|at.to_zoned(zone.clone()).strftime("%b %-d, %Y, %-I:%M %p").to_string()).unwrap_or_default();
            let line=std::iter::once(when).chain(entry["why"].as_str().map(str::to_owned)).filter(|s|!s.is_empty()).collect::<Vec<_>>().join(" · ");
            let mut row=json!({"id":format!("activity-{}",text(&entry["id"])),"title":activity_sentence(&entry),"line":line,"kind":"status"});
            if entry["what"]=="refused"&&!matches!(text(&entry["how"]),"no grant"|"autopilot"){row["tone"]=json!("warning");}row
        }).collect::<Vec<_>>();
        let mut list = json!({"id":"activity","title":"Activity","kind":"list","empty":"Nothing used yet.","items":items});
        if items.len() > 6 {
            list["search"] = json!({"placeholder":"Search the activity","empty":"No activity matches “{query}”."});
        }
        let mut card = group("activity", vec![list]);
        card["title"] = json!("Activity");
        groups.push(card);
    }
    let mut page = page("vault", "Vault", groups);
    if !notes.is_empty() {
        page["notes"] = json!(notes);
    }
    page
}

struct Checked {
    answer: Value,
    given: Option<Value>,
}
struct Check {
    fields: Vec<Value>,
    ready: bool,
}
impl Check {
    fn say(&mut self, id: &str, words: impl Into<String>, tone: &str) {
        self.fields
            .push(json!({"id":id,"words":words.into(),"tone":tone}));
        if tone == "warning" {
            self.ready = false;
        }
    }
    fn secret(
        &mut self,
        values: &Map<String, Value>,
        typed: &Value,
        id: &str,
        field: &str,
        kind: Kind,
        required: bool,
    ) -> Option<Value> {
        let secret = text(&typed[id]);
        if secret.is_empty() {
            if required {
                self.say(
                    id,
                    match kind {
                        Kind::Note => "What it says.",
                        Kind::Login => "Its password.",
                        _ => "Its value.",
                    },
                    "quiet",
                );
                self.ready = false;
            }
            return None;
        }
        if secret.chars().count() < 4 {
            self.say(
                id,
                "A value needs at least 4 characters, so it can be hidden from agents.",
                "warning",
            );
            return None;
        }
        if values.contains_key(field) {
            self.say(id, format!("{field} is given twice."), "warning");
            return None;
        }
        Some(json!(secret))
    }
    fn answer(&self) -> Value {
        json!({"fields":self.fields,"ready":self.ready})
    }
}
fn checked(known: &Facts, item: Option<&Item>, values: &Value, submit: bool) -> Checked {
    let mut check = Check {
        fields: vec![],
        ready: true,
    };
    let Some(kind) = item
        .map(|i| i.kind)
        .or_else(|| serde_json::from_value::<Kind>(values["kind"].clone()).ok())
    else {
        return Checked {
            answer: json!({"fields":[{"id":"kind","words":"Pick what it is.","tone":"warning"}],"ready":false}),
            given: None,
        };
    };
    let name = text(&values["name"]).trim();
    if name.is_empty() {
        check.say("name", "Give it a name.", "quiet");
        check.ready = false;
    } else if known.items.iter().any(|other| {
        item.is_none_or(|i| i.id != other.id) && other.name.to_lowercase() == name.to_lowercase()
    }) {
        check.say(
            "name",
            format!("There’s an item called “{name}” already."),
            "warning",
        );
    }
    let site_field = if kind == Kind::Login { "site" } else { "hosts" };
    let mut sites = Vec::new();
    if matches!(kind, Kind::Login | Kind::Key) {
        for typed in text(&values[site_field])
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|s| !s.is_empty())
        {
            if let Some(site) = site_of(typed) {
                if !sites.contains(&site) {
                    sites.push(site);
                }
            } else {
                check.say(
                    site_field,
                    format!(
                        "“{}” isn’t a site.",
                        typed.chars().take(80).collect::<String>()
                    ),
                    "warning",
                );
            }
        }
        if kind == Kind::Login
            && sites.is_empty()
            && !check.fields.iter().any(|f| f["id"] == "site")
        {
            check.say("site", "The site it’s for, like github.com.", "quiet");
            check.ready = false;
        }
    }
    let mut given = json!({"kind":kind_id(kind),"name":name,"sites":sites,"username":if kind==Kind::Login{text(&values["username"]).trim()}else{""}});
    if kind == Kind::Login {
        given["signInOnly"] = json!(values["signInOnly"] == true);
    }
    if kind == Kind::Note {
        given["readable"] = json!(values["readable"] == true);
    }
    let mut secrets = Map::new();
    let first = match kind {
        Kind::Login => Some("password"),
        Kind::Key => Some("value"),
        Kind::Note => Some("text"),
        _ => None,
    };
    if let Some(first) = first
        && let Some(secret) = check.secret(&secrets, values, first, first, kind, item.is_none())
    {
        secrets.insert(first.into(), secret);
    }
    if kind == Kind::Key {
        let removed = text(&values["remove"]);
        for field in item
            .into_iter()
            .flat_map(|i| i.fields.iter())
            .filter(|f| f.as_str() != "value")
        {
            let id = format!("field-{field}");
            if field == removed {
                if !text(&values[&id]).is_empty() {
                    check.say(
                        &id,
                        format!(
                            "{} is being removed: leave it empty, or don’t remove it.",
                            label(field)
                        ),
                        "warning",
                    );
                } else {
                    secrets.insert(field.clone(), Value::Null);
                }
            } else if let Some(secret) = check.secret(&secrets, values, &id, field, kind, false) {
                secrets.insert(field.clone(), secret);
            }
        }
        for pair in values["more"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|p| p.is_object())
        {
            let key = text(&pair["key"]).trim();
            let typed = text(&pair["value"]);
            if key.is_empty() && typed.is_empty() {
                continue;
            }
            let field = key.to_lowercase().replace(' ', "_");
            let valid = field.len() <= 40
                && field
                    .as_bytes()
                    .first()
                    .is_some_and(|b| b.is_ascii_lowercase())
                && field
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
            if field.is_empty() {
                check.say("more", "Give each value a name.", "warning");
            } else if !valid {
                check.say("more",format!("{} isn’t a field’s name: lowercase letters, digits and _, starting with a letter.",field.chars().take(40).collect::<String>()),"warning");
            } else if field == "value"
                || secrets.contains_key(&field)
                || item.is_some_and(|i| i.fields.contains(&field))
            {
                check.say("more", format!("{field} is given twice."), "warning");
            } else if typed.is_empty() {
                check.say("more", format!("Give {field} a value."), "warning");
            } else if typed.chars().count() < 4 {
                check.say(
                    "more",
                    "A value needs at least 4 characters, so it can be hidden from agents.",
                    "warning",
                );
            } else {
                secrets.insert(field, json!(typed));
            }
        }
    }
    if kind == Kind::File {
        let file = &values["file"];
        let data = file["data"].as_str();
        let size = data
            .map(|s| {
                (s.len() * 3 / 4).saturating_sub(s.bytes().rev().take_while(|b| *b == b'=').count())
                    as f64
            })
            .or_else(|| file["size"].as_f64());
        if size.is_none() || submit && data.is_none() && item.is_none() {
            if item.is_none() {
                check.say("file", "Choose the file.", "quiet");
                check.ready = false;
            }
        } else if size.is_some_and(|s| s > 700_000.0) {
            check.say(
                "file",
                "That file is too big for the vault (at most about 700 KB).",
                "warning",
            );
        } else if size == Some(0.0) {
            check.say("file", "That file is empty.", "warning");
        } else {
            if let Some(name) = std::path::Path::new(text(&file["name"]).trim())
                .file_name()
                .filter(|n| !n.is_empty())
            {
                given["fileName"] = json!(name.to_string_lossy());
            }
            if let Some(data) = data {
                secrets.insert("file".into(), json!(data));
            }
        }
    }
    given["values"] = Value::Object(secrets);
    Checked {
        answer: check.answer(),
        given: check.ready.then_some(given),
    }
}
fn form_refusal(check: &Value) -> Refusal {
    let fields = check["fields"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let words = fields
        .iter()
        .find(|f| f["tone"] == "warning")
        .or_else(|| fields.first())
        .and_then(|f| f["words"].as_str())
        .unwrap_or("Fill in what it needs first.");
    Refusal::json(
        StatusCode::BAD_REQUEST,
        json!({"message":words,"fields":fields}),
    )
}
async fn item_refused(result: Result<Value, Refusal>, kind: Kind) -> Result<(), Refusal> {
    match result {
        Ok(_) => Ok(()),
        Err(error) => {
            let reply = error.reply();
            let status = reply.status;
            let body = reply
                .body
                .collect()
                .await
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b.to_bytes()).ok())
                .unwrap_or_default();
            let why = body["message"].as_str().unwrap_or_default();
            let field = if why.contains("item called") {
                Some("name")
            } else if why.contains("site") {
                Some(if kind == Kind::Login { "site" } else { "hosts" })
            } else {
                None
            };
            if let Some(field) = field {
                Err(Refusal::json(
                    StatusCode::BAD_REQUEST,
                    json!({"message":why,"fields":[{"id":field,"words":why,"tone":"warning"}]}),
                ))
            } else {
                Err(Refusal::json(status, body))
            }
        }
    }
}
pub async fn change(ctx: &Context, at: &Value, value: &Value) -> Result<(), Refusal> {
    if at["id"] != "pause" {
        return Err(bad("That isn't something to change."));
    }
    if value != &at["value"] {
        ctx.ask(
            Method::POST,
            "kuvo/vault/pause",
            Some(json!({"paused":value==true})),
        )
        .await?;
    }
    Ok(())
}
fn encoded(id: &str) -> String {
    percent_encoding::utf8_percent_encode(id, percent_encoding::NON_ALPHANUMERIC).to_string()
}
pub async fn act(ctx: &Context, at: &Value, action: &Value, values: &Value) -> Result<(), Refusal> {
    let known = facts(ctx, false).await?;
    let id = text(&at["id"]);
    let action = text(&action["id"]);
    if id == "items" && action == "add" {
        let checked = checked(&known, None, values, true);
        let mut given = checked.given.ok_or_else(|| form_refusal(&checked.answer))?;
        let kind = serde_json::from_value::<Kind>(given["kind"].clone())
            .map_err(|_| bad("Pick what it is."))?;
        if text(&given["username"]).is_empty() {
            given.as_object_mut().unwrap().shift_remove("username");
        }
        return item_refused(
            ctx.ask(Method::POST, "kuvo/vault/items", Some(given)).await,
            kind,
        )
        .await;
    }
    if let Some(id) = id.strip_prefix("ask-") {
        let Some(ask) = known.asks.iter().find(|a| a["id"] == id) else {
            return Err(stale(read(ctx).await));
        };
        let mut body = json!({"answer":action});
        match action {
            "no" | "once" => {}
            "always" => {
                if !may_always(ask) {
                    return Err(refused(
                        StatusCode::FORBIDDEN,
                        "This one can only be allowed once.",
                    ));
                }
                if !confirms(ctx) {
                    return Err(refused(StatusCode::FORBIDDEN, CANT_CONFIRM));
                }
            }
            "give" => {
                if !known.items.iter().any(|i| values["item"] == i.id) {
                    return Err(Refusal::json(
                        StatusCode::BAD_REQUEST,
                        json!({"message":"Pick an item to give.","fields":[{"id":"item","words":"Pick an item to give.","tone":"warning"}]}),
                    ));
                }
                let always = values["how"] == "always";
                if always && !may_always(ask) {
                    return Err(refused(
                        StatusCode::FORBIDDEN,
                        "This one can only be given once.",
                    ));
                }
                if always && !confirms(ctx) {
                    return Err(refused(StatusCode::FORBIDDEN, CANT_CONFIRM));
                }
                body = json!({"answer":if always{"always"}else{"once"},"item":values["item"]});
            }
            _ => return Err(no_action()),
        }
        let result = ctx
            .ask(
                Method::POST,
                &format!("kuvo/vault/asks/{}", encoded(id)),
                Some(body),
            )
            .await;
        if let Err(error) = result {
            let reply = error.reply();
            if reply.status == StatusCode::NOT_FOUND {
                return Err(stale(read(ctx).await));
            }
            let status = reply.status;
            let body = reply
                .body
                .collect()
                .await
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b.to_bytes()).ok())
                .unwrap_or_default();
            return Err(Refusal::json(status, body));
        }
        return Ok(());
    }
    if let Some(who) = id.strip_prefix("paused-").filter(|_| action == "go-on") {
        ctx.ask(
            Method::POST,
            "kuvo/vault/pause",
            Some(json!({"paused":false,"bot":who})),
        )
        .await?;
        return Ok(());
    }
    let id = id.strip_prefix("item-").ok_or_else(no_action)?;
    let Some(item) = known.items.iter().find(|i| i.id == id) else {
        return Err(stale(read(ctx).await));
    };
    let route = format!("kuvo/vault/items/{}", encoded(id));
    match action {
        "edit" => {
            let checked = checked(&known, Some(item), values, true);
            let given = checked.given.ok_or_else(|| form_refusal(&checked.answer))?;
            let mut changes = Map::new();
            for (field, value) in given["values"].as_object().into_iter().flatten() {
                if value.is_string()
                    && item.fields.contains(field)
                    && let Ok(same) = ctx
                        .ask(
                            Method::POST,
                            &format!("{route}/same"),
                            Some(json!({"values":{field:value}})),
                        )
                        .await
                    && same["same"] == true
                {
                    continue;
                }
                changes.insert(field.clone(), value.clone());
            }
            let mut body = json!({});
            if given["name"] != item.name {
                body["name"] = given["name"].clone();
            }
            if matches!(item.kind, Kind::Login | Kind::Key) && given["sites"] != json!(item.sites) {
                body["sites"] = given["sites"].clone();
            }
            if item.kind == Kind::Login {
                if given["username"] != item.username.as_deref().unwrap_or_default() {
                    body["username"] = given["username"].clone();
                }
                if given["signInOnly"] != (item.sign_in_only == Some(true)) {
                    body["signInOnly"] = given["signInOnly"].clone();
                }
            }
            if item.kind == Kind::Note && given["readable"] != (item.readable == Some(true)) {
                body["readable"] = given["readable"].clone();
            }
            if item.kind == Kind::File
                && given["fileName"].is_string()
                && given["fileName"].as_str() != item.file_name.as_deref()
            {
                body["fileName"] = given["fileName"].clone();
            }
            if !changes.is_empty() {
                body["values"] = Value::Object(changes);
            }
            if body.as_object().is_some_and(|b| b.is_empty()) {
                return Ok(());
            }
            item_refused(ctx.ask(Method::PATCH, &route, Some(body)).await, item.kind).await
        }
        "who" => {
            let grants = known
                .grants
                .iter()
                .filter(|g| g.item == id)
                .collect::<Vec<_>>();
            let mut wanted = Vec::<(Who, String)>::new();
            for (field, value) in values.as_object().into_iter().flatten() {
                if field == "add-bot" {
                    continue;
                }
                let who = if field == "everyone" {
                    Some(Who::Everyone { everyone: true })
                } else if let Some(bot) = field.strip_prefix("bot-") {
                    Some(Who::Bot { bot: bot.into() })
                } else if let Some(project) = field.strip_prefix("project-").filter(|p| {
                    grants
                        .iter()
                        .any(|g| matches!(&g.who,Who::Project{project}if project==p))
                }) {
                    Some(Who::Project {
                        project: project.into(),
                    })
                } else {
                    None
                };
                let Some(who) = who else {
                    return Err(bad("That isn't someone it can be given to."));
                };
                let how = text(value);
                if !matches!(how, "own" | "ask" | "no") {
                    return Err(bad("That isn't someone it can be given to."));
                }
                if matches!(&who,Who::Bot{bot}if !known.bots.iter().any(|b|b.id==*bot)) {
                    return Err(stale(read(ctx).await));
                }
                let now = how_of(&grants, &who);
                if rank(how) > rank(now) && !confirms(ctx) {
                    return Err(refused(StatusCode::FORBIDDEN, CANT_CONFIRM));
                }
                if now != how {
                    wanted.push((who, how.into()));
                }
            }
            let more = text(&values["add-bot"]);
            if !more.is_empty() && more != "none" {
                if !confirms(ctx) {
                    return Err(refused(StatusCode::FORBIDDEN, CANT_CONFIRM));
                }
                if !known.bots.iter().any(|b| b.id == more) {
                    return Err(stale(read(ctx).await));
                }
                let who = Who::Bot { bot: more.into() };
                if how_of(&grants, &who) == "no" && !wanted.iter().any(|(w, _)| *w == who) {
                    wanted.push((who, "ask".into()));
                }
            }
            for (who, how) in wanted {
                ctx.ask(Method::POST,"kuvo/vault/grants",Some(json!({"item":id,"who":who,"how":if how=="no"{Value::Null}else{json!(how)}}))).await?;
            }
            Ok(())
        }
        "delete" => {
            ctx.ask(Method::DELETE, &route, None).await?;
            Ok(())
        }
        _ => Err(no_action()),
    }
}
pub async fn check(ctx: &Context, at: &Value, action: &Value, values: &Value) -> Value {
    let Ok(known) = facts(ctx, false).await else {
        return json!({"fields":[],"ready":false});
    };
    let id = text(&at["id"]);
    if id == "items" && action["id"] == "add" {
        return checked(&known, None, values, false).answer;
    }
    if let Some(id) = id.strip_prefix("item-").filter(|_| action["id"] == "edit") {
        return known
            .items
            .iter()
            .find(|i| i.id == id)
            .map(|i| checked(&known, Some(i), values, false).answer)
            .unwrap_or_else(|| json!({"fields":[],"ready":false}));
    }
    if action["id"] == "give" {
        return json!({"fields":[],"ready":known.items.iter().any(|i|values["item"]==i.id)});
    }
    json!({"fields":[],"ready":true})
}
