//! Local archived-session actions use the session menu and its confirmation revision.
use super::pages::said;
use super::*;
const EMPTY: &str = "None yet. Archive a session from its menu to put it away.";
fn encoded(id: &str) -> String {
    percent_encoding::utf8_percent_encode(id, percent_encoding::NON_ALPHANUMERIC).to_string()
}
async fn read_inner(ctx: &Context) -> Result<Value, Refusal> {
    let list = ctx.ask(Method::GET, "kuvo/sessions/list", None).await?;
    if list["pending"].as_array().is_some_and(|p| !p.is_empty()) {
        return Err(refused(
            StatusCode::SERVICE_UNAVAILABLE,
            "Some sessions can't be read right now. Try again in a moment.",
        ));
    }
    let info = ctx
        .ask(Method::GET, "kuvo/info", None)
        .await
        .unwrap_or_default();
    let setup = ctx
        .ask(Method::GET, "kuvo/setup", None)
        .await
        .unwrap_or_default();
    let computer = json!({"id":setup["you"]["key"].as_str().unwrap_or("here"),"name":info["name"].as_str().unwrap_or("Computer")});
    let zone = ctx
        .settings
        .setup
        .time_zone()
        .and_then(|(name, _)| jiff::tz::TimeZone::get(&name).ok())
        .unwrap_or_else(jiff::tz::TimeZone::system);
    let mut groups = Vec::new();
    let mut states = Vec::new();
    let mut held = None;
    for group_value in list["archivedGroups"].as_array().into_iter().flatten() {
        let mut items = Vec::new();
        for id in group_value["sessions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            let facts = &list["sessions"][id];
            let menu = ctx
                .ask(
                    Method::GET,
                    &format!("kuvo/sessions/{}/actions", encoded(id)),
                    None,
                )
                .await?;
            if held.is_none() {
                held = menu["held"].as_str().map(str::to_owned);
            }
            let mut was = json!({"updated":facts["updated"],"archived":true});
            let mut actions = Vec::new();
            for action in menu["actions"].as_array().into_iter().flatten() {
                let mut item = json!({"id":action["id"],"title":action["label"]});
                if action["destructive"] == true {
                    item["style"] = json!("destructive");
                }
                if let Some(confirm) = action.get("confirm") {
                    item["confirm"] = json!({"title":confirm["title"],"body":confirm["message"],"button":confirm["label"]});
                    if action["id"] == "delete" {
                        was["confirm"] = confirm.clone();
                    }
                }
                if let Some(held) = menu["held"].as_str() {
                    item["can"] = json!({"why":held});
                }
                actions.push(item);
            }
            states.push(json!({"id":id,"was":was}));
            let timestamp = facts["updated"]
                .as_i64()
                .and_then(|at| jiff::Timestamp::from_millisecond(at).ok())
                .map(|at| {
                    at.to_zoned(zone.clone())
                        .strftime("%b %-d, %Y, %-I:%M %p")
                        .to_string()
                })
                .unwrap_or_default();
            items.push(json!({"id":format!("session-{id}"),"title":facts["title"].as_str().filter(|s|!s.is_empty()).unwrap_or("New Session"),"line":timestamp,"kind":"actions","value":was,"actions":actions,"computer":computer}));
        }
        let project = format!(
            "{}:{}",
            group_value["kind"].as_str().unwrap_or_default(),
            group_value["directory"].as_str().unwrap_or_default()
        );
        let mut card = group(
            &format!("project-{project}"),
            vec![
                json!({"id":format!("archived-{project}"),"title":group_value["title"],"kind":"list","items":items,"computer":computer,"search":{"placeholder":"Search archived sessions","empty":"No archived session matches “{query}”."}}),
            ],
        );
        card["title"] = group_value["title"].clone();
        groups.push(card);
    }
    let mut action = json!({"id":"delete-all","title":"Delete All…","style":"destructive","can":if let Some(held)=held{json!({"why":held})}else if states.is_empty(){json!({"why":EMPTY})}else{json!(true)}});
    if !states.is_empty() {
        let all = ctx
            .ask(
                Method::POST,
                "kuvo/sessions/deletes",
                Some(json!({"sessions":states.iter().map(|s|&s["id"]).collect::<Vec<_>>()})),
            )
            .await?;
        action["confirm"] =
            json!({"title":all["title"],"body":all["message"],"button":all["label"]});
    }
    groups.insert(0,group("archive-controls",vec![json!({"id":"delete-all","title":"Archived Sessions","kind":"actions","value":states,"actions":[action],"computer":computer})]));
    let mut page = page("archived", "Archived Sessions", groups);
    if states.is_empty() {
        page["notes"] = json!([{"words":EMPTY,"tone":"quiet"}]);
    }
    page["pendingComputers"] = json!([]);
    Ok(page)
}
pub async fn read(ctx: &Context) -> Value {
    match read_inner(ctx).await {
        Ok(page) => page,
        Err(e) => unread(
            "archived",
            "Archived Sessions",
            said(e, "This computer's settings couldn't be read.").await,
        ),
    }
}
pub async fn act(ctx: &Context, at: &Value, action: &Value) -> Result<(), Refusal> {
    if at["id"] == "delete-all" && action["id"] == "delete-all" {
        for state in at["value"].as_array().into_iter().flatten() {
            let id = state["id"].as_str().ok_or_else(no_action)?;
            ctx.ask(
                Method::DELETE,
                &format!("kuvo/sessions/{}", encoded(id)),
                Some(json!({"was":state["was"]})),
            )
            .await?;
        }
        return Ok(());
    }
    let id = at["id"]
        .as_str()
        .and_then(|s| s.strip_prefix("session-"))
        .ok_or_else(no_action)?;
    let route = format!("kuvo/sessions/{}", encoded(id));
    match action["id"].as_str() {
        Some("delete") => {
            ctx.ask(Method::DELETE, &route, Some(json!({"was":at["value"]})))
                .await?;
        }
        Some("unarchive") => {
            ctx.ask(
                Method::POST,
                &format!("{route}/archive"),
                Some(json!({"archived":false,"was":at["value"]})),
            )
            .await?;
        }
        _ => return Err(no_action()),
    }
    Ok(())
}
