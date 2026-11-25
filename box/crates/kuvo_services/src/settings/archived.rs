//! Local archived-session actions use the session menu and its confirmation revision.
use super::pages::said;
use super::*;
const EMPTY: &str = "None yet. Archive a session from its menu to put it away.";
fn encoded(id: &str) -> String {
    percent_encoding::utf8_percent_encode(id, percent_encoding::NON_ALPHANUMERIC).to_string()
}
pub(super) async fn read_here(ctx: &Context) -> Result<Value, Refusal> {
    let list = ctx.ask(Method::GET, "kuvo/sessions/list", None).await?;
    if list["pending"].as_array().is_some_and(|p| !p.is_empty()) {
        return Err(refused(
            StatusCode::SERVICE_UNAVAILABLE,
            "Some sessions can't be read right now. Try again in a moment.",
        ));
    }
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
                        .strftime("%b %-d, %Y at %-I:%M %p")
                        .to_string()
                })
                .unwrap_or_default();
            items.push(json!({"id":format!("session-{id}"),"title":facts["title"].as_str().filter(|s|!s.is_empty()).unwrap_or("New Session"),"line":timestamp,"kind":"actions","value":was,"actions":actions}));
        }
        let project = format!(
            "{}:{}",
            group_value["kind"].as_str().unwrap_or_default(),
            group_value["directory"].as_str().unwrap_or_default()
        );
        let mut card = group(
            &format!("project-{project}"),
            vec![
                json!({"id":format!("archived-{project}"),"title":group_value["title"],"kind":"list","items":items,"search":{"placeholder":"Search archived sessions","empty":"No archived session matches “{query}”."}}),
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
    groups.insert(0,group("archive-controls",vec![json!({"id":"delete-all","title":"Archived Sessions","kind":"actions","value":states,"actions":[action]})]));
    let mut page = page("archived", "Archived Sessions", groups);
    if states.is_empty() {
        page["notes"] = json!([{"words":EMPTY,"tone":"quiet"}]);
    }
    Ok(page)
}
pub async fn read(ctx: &Context) -> Value {
    let page = match read_here(ctx).await {
        Ok(page) => page,
        Err(e) => unread(
            "archived",
            "Archived Sessions",
            said(e, "This computer's settings couldn't be read.").await,
        ),
    };
    merged(ctx, page)
}
fn merged(ctx: &Context, mut page: Value) -> Value {
    let view = ctx.settings.peers.view();
    let local = json!(view.roster.local());
    let Some(groups) = page["groups"].as_array_mut() else {
        return page;
    };
    let here = groups.len();
    for group in groups.iter_mut() {
        for row in group["rows"].as_array_mut().into_iter().flatten() {
            row["computer"] = local.clone();
            for item in row
                .get_mut("items")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                item["computer"] = local.clone();
            }
        }
    }
    for peer in view.archives(ctx.settings.peers.now()) {
        let computer = json!(peer.computer);
        let key = &peer.computer.id;
        let stamp = |mut source: Value, id: String, session: Option<&str>| {
            source["id"] = json!(id);
            source["computer"] = computer.clone();
            source["away"] = json!(peer.standing.away());
            if let Some(at) = peer.standing.last_seen() {
                source["lastSeen"] = json!(at);
            }
            if let Some(session) = session {
                source["value"] =
                    json!({"peer":key,"session":session,"revision":source["revision"]});
                if peer.standing.away() {
                    source["actions"] = json!([]);
                }
            }
            row(source)
        };
        for group in &peer.copy.groups {
            let mut copied = serde_json::to_value(group).unwrap_or_default();
            let id = format!("{key}:{}", group.id);
            if group.id == "archive-controls" {
                let Some(source) = copied["rows"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|r| r["id"] == "delete-all")
                else {
                    continue;
                };
                let Some(states) = source["value"].as_array().filter(|s| !s.is_empty()) else {
                    continue;
                };
                if peer.standing.away()
                    || states.iter().any(|s| {
                        s["id"]
                            .as_str()
                            .is_none_or(|id| view.archive_moved(key, id))
                    })
                {
                    continue;
                }
                let Some(session) = states[0]["id"].as_str() else {
                    continue;
                };
                groups.push(json!({"id":id,"title":peer.computer.name,"rows":[stamp(source.clone(),format!("delete-all-{key}"),Some(session))]}));
                continue;
            }
            let rows = copied["rows"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|source| {
                    let items = source["items"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|entry| {
                            let session = entry["id"].as_str()?.strip_prefix("session-")?;
                            if view.archive_moved(key, session) {
                                return None;
                            }
                            Some(stamp(
                                entry.clone(),
                                format!("session-{key}:{session}"),
                                Some(session),
                            ))
                        })
                        .collect::<Vec<_>>();
                    if items.is_empty() {
                        return None;
                    }
                    let mut source = source.clone();
                    source["items"] = json!(items);
                    let id = format!("{key}:{}", source["id"].as_str()?);
                    Some(stamp(source, id, None))
                })
                .collect::<Vec<_>>();
            if !rows.is_empty() {
                copied["rows"] = json!(rows);
                copied["id"] = json!(id);
                groups.push(copied);
            }
        }
    }
    let added = groups.len() > here;
    page["pendingComputers"] = json!(view.pending_archives());
    if added && let Some(notes) = page.get_mut("notes").and_then(Value::as_array_mut) {
        notes.retain(|note| note["tone"] != "quiet");
    }
    page
}

pub async fn act(ctx: &Context, at: &Value, action: &Value) -> Result<(), Refusal> {
    if let Some(peer) = at["value"]["peer"].as_str() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let key: [u8; 32] = URL_SAFE_NO_PAD
            .decode(peer)
            .ok()
            .and_then(|v| v.try_into().ok())
            .ok_or_else(no_action)?;
        let session = at["value"]["session"].as_str().ok_or_else(no_action)?;
        let revision = at["value"]["revision"].as_str().ok_or_else(no_action)?;
        let action = action["id"].as_str().ok_or_else(no_action)?;
        let view = ctx.settings.peers.view();
        let computer = view
            .roster
            .member(&key)
            .filter(|_| !view.away(&key) && !view.archive_moved(peer, session))
            .ok_or_else(|| refused(StatusCode::CONFLICT, CHANGED_SINCE))?;
        return ctx
            .settings
            .archive_forward
            .write(
                key,
                json!({"key":peer,"name":computer.name,"kind":computer.kind}),
                session,
                action,
                revision,
                &ctx.request.caller,
            )
            .await;
    }
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
