//! Custom Auto model lists and settings. Running a turn on one belongs to the handover owner.
use super::*;
use kuvo_agents::custom_auto::{
    Entry, Error, Limit, List, Saved, Scope, Store, clean_entries, fold,
};
use serde::Deserialize;
const ROOT: &str = "kuvo/agents/api/kuvo/auto";
const NOTE: &str = "A model at its limit is skipped until the limit resets; an account at its limit skips all its models.";
const EMPTY: &str = "Add the models to go through, in the order you want them. Pick this list in a session's model picker to use it.";

#[derive(Clone, Deserialize)]
struct Model {
    #[serde(rename = "providerID")]
    provider: String,
    id: String,
    name: String,
    efforts: Vec<String>,
    #[serde(default)]
    aliases: Vec<String>,
}
async fn catalog(ctx: &Context) -> (Vec<Model>, std::collections::BTreeSet<String>) {
    let offered = ctx.settings.engine.settings_offered_models().await;
    let models = offered
        .models
        .into_iter()
        .filter_map(|model| serde_json::from_value::<Model>(model).ok())
        .filter(|model| model.provider != "auto")
        .collect();
    (models, offered.signed_out)
}
async fn models(ctx: &Context) -> Vec<Model> {
    catalog(ctx).await.0
}
fn alias(models: &[Model], entry: &Entry) -> Option<String> {
    models
        .iter()
        .find(|m| m.provider == entry.provider && m.aliases.contains(&entry.model))
        .map(|m| m.id.clone())
}
fn shown_lists(kept: &Saved, models: &[Model]) -> Vec<ShownList> {
    kept.lists
        .iter()
        .map(|list| ShownList {
            id: list.id.clone(),
            name: list.name.clone(),
            entries: fold(list.entries.clone(), |e| alias(models, e)),
            revision: list.revision(),
        })
        .collect()
}
fn answer(kept: &Saved, models: &[Model], read: bool) -> Value {
    let lists = shown_lists(kept, models);
    let mut data = json!({"lists":lists,"limits":kept.limits});
    if read {
        data["entries"] = json!(
            lists
                .first()
                .map(|list| list.entries.as_slice())
                .unwrap_or_default()
        );
    }
    json!({"data":data})
}
fn error(e: Error) -> Refusal {
    match e {
        Error::Stale => refused(
            StatusCode::CONFLICT,
            "This list changed. Read it again before saving.",
        ),
        Error::Invalid(words) => bad(words),
        Error::Disk(e) => refused(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}
fn now() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}
async fn kept(ctx: &Context) -> Result<Saved, Refusal> {
    let home = ctx.settings.home.clone();
    tokio::task::spawn_blocking(move || Store::new(&home).read(now()))
        .await
        .map_err(|e| refused(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| error(Error::Disk(e)))
}
fn as_kept(before: Option<&List>, entries: Vec<Entry>, models: &[Model]) -> Vec<Entry> {
    let Some(before) = before else {
        return entries;
    };
    entries
        .into_iter()
        .map(|entry| {
            before
                .entries
                .iter()
                .find(|e| {
                    e.provider == entry.provider
                        && alias(models, e).as_deref().unwrap_or(&e.model) == entry.model
                        && e.variant == entry.variant
                })
                .map(|e| Entry {
                    model: e.model.clone(),
                    ..entry.clone()
                })
                .unwrap_or(entry)
        })
        .collect()
}
#[derive(Clone, Copy)]
enum Mutation {
    Add,
    Edit,
    Remove,
    Order,
    Clear,
}
async fn mutate_list(
    ctx: Context,
    mutation: Mutation,
    id: String,
    given: Value,
) -> Result<Value, Refusal> {
    let offered = models(&ctx).await;
    let home = ctx.settings.home.clone();
    let provider = form_urlencoded::parse(
        ctx.request
            .path
            .split_once('?')
            .map(|(_, q)| q)
            .unwrap_or_default()
            .as_bytes(),
    )
    .find(|(k, _)| k == "provider")
    .map(|(_, v)| v.into_owned());
    let result = tokio::task::spawn_blocking(move || {
        let store = Store::new(&home);
        let now = now();
        match mutation {
            Mutation::Clear => {
                store.clear(provider.as_deref(), now)?;
                Ok(json!({}))
            }
            Mutation::Order => {
                let ids = serde_json::from_value::<Vec<String>>(given["ids"].clone())
                    .map_err(|_| Error::Invalid("Send every list id once."))?;
                let was = serde_json::from_value::<Vec<String>>(given["was"].clone())
                    .map_err(|_| Error::Stale)?;
                Ok(answer(&store.reorder(&ids, &was, now)?, &offered, false))
            }
            Mutation::Remove => Ok(answer(
                &store.remove(&id, given["revision"].as_str(), now)?,
                &offered,
                false,
            )),
            Mutation::Add | Mutation::Edit => {
                let id = if matches!(mutation, Mutation::Add) {
                    given["id"].as_str().unwrap_or_default()
                } else {
                    &id
                };
                let name = given
                    .get("name")
                    .map(|v| {
                        v.as_str()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .ok_or(Error::Invalid("Send a name."))
                    })
                    .transpose()?;
                let entries = given
                    .get("entries")
                    .map(|v| {
                        v.is_array()
                            .then(|| clean_entries(v))
                            .ok_or(Error::Invalid("Send entries."))
                    })
                    .transpose()?;
                let before = store.read(now)?;
                let entries =
                    entries.map(|e| as_kept(before.lists.iter().find(|l| l.id == id), e, &offered));
                Ok(answer(
                    &store.change(
                        id,
                        given["revision"].as_str(),
                        name,
                        entries,
                        matches!(mutation, Mutation::Add),
                        now,
                    )?,
                    &offered,
                    false,
                ))
            }
        }
    })
    .await
    .map_err(|e| refused(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    result.map_err(error)
}
pub fn register(routes: &mut Registry, state: Arc<Settings>) {
    let state_read = state.clone();
    routes.register::<get_kuvo_agents_api_kuvo_auto::Spec, _, _>(Access::Any, move |call| {
        let ctx = Context::new(state_read.clone(), &call);
        async move {
            let (kept, models) = tokio::join!(kept(&ctx), models(&ctx));
            Ok(typed(answer(&kept?, &models, true)))
        }
    });
    macro_rules! mutation {
        ($route:ty,$kind:expr) => {
            let state = state.clone();
            routes.register::<$route, _, _>(Access::Any, move |call| {
                let ctx = Context::new(state.clone(), &call);
                let id = decoded(call.param("id"));
                let body = serde_json::from_slice(&call.request.body).unwrap_or_default();
                async move { Ok(typed(mutate_list(ctx, $kind, id, body).await?)) }
            });
        };
    }
    mutation!(post_kuvo_agents_api_kuvo_auto::Spec, Mutation::Add);
    mutation!(put_kuvo_agents_api_kuvo_auto_id::Spec, Mutation::Edit);
    mutation!(delete_kuvo_agents_api_kuvo_auto_id::Spec, Mutation::Remove);
    mutation!(put_kuvo_agents_api_kuvo_auto_order::Spec, Mutation::Order);
    mutation!(
        delete_kuvo_agents_api_kuvo_auto_limits::Spec,
        Mutation::Clear
    );
}

#[derive(serde::Serialize)]
struct ShownList {
    id: String,
    name: String,
    entries: Vec<Entry>,
    revision: String,
}
struct Shown {
    lists: Vec<ShownList>,
    limits: Vec<Limit>,
}
struct Facts {
    shown: Shown,
    models: Vec<Model>,
    signed_out: std::collections::BTreeSet<String>,
}
async fn facts(ctx: &Context) -> Result<Facts, Refusal> {
    // The API and the page use the same typed store and alias fold; read the catalog only once.
    let (kept, (models, signed_out)) = tokio::join!(kept(ctx), catalog(ctx));
    let kept = kept?;
    let shown = Shown {
        lists: shown_lists(&kept, &models),
        limits: kept.limits,
    };
    Ok(Facts {
        shown,
        models,
        signed_out,
    })
}
fn name_form(name: Option<&str>) -> Value {
    json!({"title":name.unwrap_or("New List"),"submit":if name.is_some(){"Save"}else{"New List"},"fields":[{"id":"name","title":"Name","kind":"text","value":name.unwrap_or_default(),"line":"Its name, as the model picker shows it"}]})
}
fn model_key(entry: &Entry) -> String {
    json!([entry.provider, entry.model]).to_string()
}
fn model_id(model: &Model) -> String {
    json!([model.provider, model.id]).to_string()
}
fn effort_words(model: Option<&Model>) -> String {
    match model.filter(|m| !m.efforts.is_empty()) {
        Some(m) => format!(
            "Supported efforts: {}. Leave empty for the default.",
            m.efforts.join(", ")
        ),
        None => "This model uses its default effort.".into(),
    }
}
fn entry_form(models: &[Model], entry: Option<&Entry>) -> Value {
    let mut choices=models.iter().map(|m|json!({"id":model_id(m),"title":format!("{} ({})",m.name,m.provider),"model":{"providerID":m.provider,"id":m.id}})).collect::<Vec<_>>();
    if let Some(entry) = entry.filter(|e| !choices.iter().any(|c| c["id"] == model_key(e))) {
        choices.push(json!({"id":model_key(entry),"title":format!("{} ({})",entry.model,entry.provider),"model":{"providerID":entry.provider,"id":entry.model}}));
    }
    json!({"title":if entry.is_some(){"Change the model or its effort"}else{"Add Model"},"submit":if entry.is_some(){"Save"}else{"Add Model"},"fields":[
        {"id":"model","title":"Model","kind":"choice","value":entry.map(model_key).unwrap_or_default(),"choices":choices},
        {"id":"variant","title":"Effort","kind":"text","value":entry.and_then(|e|e.variant.as_deref()).unwrap_or_default(),"line":if let Some(entry)=entry{effort_words(models.iter().find(|m|model_id(m)==model_key(entry)))}else{"Leave empty for the default. Choose a model to see its supported efforts below.".into()}}
    ]})
}
fn moves(index: usize, length: usize) -> Vec<Value> {
    let mut actions = Vec::new();
    if index > 0 {
        actions.push(json!({"id":"up","title":"Move Up"}));
    }
    if index + 1 < length {
        actions.push(json!({"id":"down","title":"Move Down"}));
    }
    actions
}
fn entry_id(id: &str, index: usize) -> String {
    format!("entry-{id}-{index}")
}
fn reset_words(ctx: &Context, limit: &Limit) -> String {
    let zone = ctx
        .settings
        .setup
        .time_zone()
        .and_then(|(name, _)| jiff::tz::TimeZone::get(&name).ok())
        .unwrap_or_else(jiff::tz::TimeZone::system);
    let today = jiff::Timestamp::now().to_zoned(zone.clone()).date();
    let Ok(until) = jiff::Timestamp::from_millisecond(limit.until) else {
        return String::new();
    };
    let until = until.to_zoned(zone);
    let when = until
        .strftime(if today == until.date() {
            "%-I:%M %p"
        } else {
            "%b %-d, %Y, %-I:%M %p"
        })
        .to_string();
    if limit.known {
        format!("until {when}")
    } else {
        format!("for now (until about {when})")
    }
}
pub async fn read(ctx: &Context) -> Value {
    let known = match facts(ctx).await {
        Ok(k) => k,
        Err(e) => {
            return unread(
                "custom-auto",
                "Custom Auto",
                super::pages::said(e, "This computer's settings couldn't be read.").await,
            );
        }
    };
    let lists = &known.shown.lists;
    let items=lists.iter().enumerate().map(|(i,list)|{
        let mut actions=vec![json!({"id":"edit","title":"Edit…","form":name_form(Some(&list.name))})];
        actions.extend(moves(i,lists.len()));
        let mut remove=json!({"id":"remove","title":"Delete List","style":"destructive","confirm":{"title":format!("Delete {}?",list.name),"body":"This list will no longer be available in the model picker.","button":"Delete List"}});
        if lists.len()==1{remove["can"]=json!({"why":"Keep one list at least."});}actions.push(remove);
        json!({"id":format!("list-{}",list.id),"title":list.name,"kind":"actions","value":{"revision":list.revision,"index":i},"actions":actions})
    }).collect::<Vec<_>>();
    let mut card = group(
        "auto-lists",
        vec![
            json!({"id":"lists","title":"Lists","kind":"list","empty":"None yet.","add":{"id":"add","title":"New List","form":name_form(None)},"items":items}),
        ],
    );
    card["title"] = json!("Custom Auto");
    card["note"] = json!(NOTE);
    let mut groups = vec![card];
    for list in lists {
        let items=list.entries.iter().enumerate().map(|(i,entry)|{
            let model=known.models.iter().find(|m|m.provider==entry.provider&&m.id==entry.model);
            let name=model.map(|m|m.name.as_str()).unwrap_or(&entry.model);
            let mut actions=vec![json!({"id":"edit-model","title":"Change the model or its effort","form":entry_form(&known.models,Some(entry))})];
            actions.extend(moves(i,list.entries.len()));
            actions.push(json!({"id":"remove-model","title":"Remove","style":"destructive","confirm":{"title":format!("Remove {}?",entry.model),"body":"Custom Auto will skip this model.","button":"Remove"}}));
            let line=std::iter::once(entry.provider.as_str()).chain(entry.variant.as_deref()).filter(|s|!s.is_empty()).collect::<Vec<_>>().join(" · ");
            let mut row=json!({"id":entry_id(&list.id,i),"title":format!("{}. {name}",i+1),"line":line,"kind":"actions","value":{"entry":entry,"revision":list.revision},"actions":actions});
            if let Some(limit)=known.shown.limits.iter().find(|l|l.key==entry.provider||l.key==format!("{}/{}",entry.provider,entry.model)){
                row["badge"]=json!({"words":if matches!(limit.scope,Scope::Provider){"Its account is at its limit"}else{"At its limit"},"tone":"warning"});
                row["notice"]=json!({"words":reset_words(ctx,limit),"tone":"quiet"});
            }else if model.is_none(){row["notice"]=json!({"words":if known.signed_out.contains(&entry.provider){"Not signed in: Custom Auto passes it by"}else{"Not available now: Custom Auto passes it by"},"tone":"quiet"});}
            row
        }).collect::<Vec<_>>();
        let mut card = group(
            &format!("entries-{}", list.id),
            vec![
                json!({"id":format!("models-{}",list.id),"title":list.name,"kind":"list","empty":EMPTY,"add":{"id":"add-model","title":"Add Model","form":entry_form(&known.models,None)},"items":items}),
            ],
        );
        card["title"] = json!(list.name);
        groups.push(card);
    }
    if !known.shown.limits.is_empty() {
        let mut card = group(
            "auto-limits",
            vec![
                json!({"id":"limits","title":"Held back at a limit","line":"If a limit has reset sooner than its agent said, Custom Auto can try them all again.","kind":"actions","actions":[{"id":"clear","title":"Try All Again"}]}),
            ],
        );
        card["title"] = json!("Held back at a limit");
        groups.push(card);
    }
    page("custom-auto", "Custom Auto", groups)
}
fn form_check(values: &Value, model: bool, form: &Value, offered: &[Model]) -> Value {
    let mut fields = Vec::new();
    let key = if model { "model" } else { "name" };
    if values[key].as_str().is_none_or(|s| s.trim().is_empty()) {
        fields.push(json!({"id":key,"words":if model{"Give the model as text."}else{"Send a name."},"tone":"warning"}));
    }
    if model {
        if let Some(typed) = values["model"].as_str()
            && serde_json::from_str::<Vec<String>>(typed)
                .ok()
                .is_none_or(|v| v.len() != 2 || v.iter().any(|s| s.trim().is_empty()))
        {
            fields.push(json!({"id":"model","words":"Choose a model.","tone":"warning"}));
        }
        let model_field = form["fields"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|f| f["id"] == "model")
            .unwrap_or(&Value::Null);
        let variant = form["fields"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|f| f["id"] == "variant")
            .unwrap_or(&Value::Null);
        if !model_field["choices"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|c| c["id"] == values["model"])
        {
            fields.push(
                json!({"id":"model","words":"Choose one of the offered models.","tone":"warning"}),
            );
        }
        let selected = offered.iter().find(|m| values["model"] == model_id(m));
        let supported = values["variant"].as_str().is_some_and(|s| {
            s.is_empty()
                || selected.is_some_and(|m| m.efforts.iter().any(|e| e == s))
                || (model_field["value"] == values["model"] && variant["value"] == s)
        });
        if !supported {
            fields.push(json!({"id":"variant","words":"Choose an effort this model supports.","tone":"warning"}));
        } else {
            fields.push(json!({"id":"variant","words":effort_words(selected),"tone":"quiet"}));
        }
    }
    json!({"ready":!fields.iter().any(|f|f["tone"]=="warning"),"fields":fields})
}
pub async fn check(ctx: &Context, _at: &Value, action: &Value, values: &Value) -> Value {
    let offered = models(ctx).await;
    form_check(
        values,
        action["id"].as_str().is_some_and(|s| s.contains("model")),
        &action["form"],
        &offered,
    )
}
async fn request(
    ctx: &Context,
    method: Method,
    suffix: &str,
    body: Option<Value>,
) -> Result<(), Refusal> {
    let result = ctx.ask(method, &format!("{ROOT}{suffix}"), body).await;
    if let Err(error) = result {
        let reply = error.reply();
        if reply.status == StatusCode::CONFLICT {
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
    Ok(())
}
pub async fn act(ctx: &Context, at: &Value, action: &Value, values: &Value) -> Result<(), Refusal> {
    let known = facts(ctx).await?;
    let id = at["id"].as_str().unwrap_or_default();
    let action_id = action["id"].as_str().unwrap_or_default();
    if action.get("form").is_some() {
        let checked = form_check(
            values,
            action_id.contains("model"),
            &action["form"],
            &known.models,
        );
        if checked["ready"] != true {
            let words = checked["fields"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|f| f["tone"] == "warning")
                .and_then(|f| f["words"].as_str())
                .unwrap_or("Fill in what it needs first.");
            return Err(Refusal::json(
                StatusCode::BAD_REQUEST,
                json!({"message":words,"fields":checked["fields"]}),
            ));
        }
    }
    if id == "limits" {
        return request(ctx, Method::DELETE, "/limits", None).await;
    }
    if id == "lists" {
        let mut random = [0u8; 4];
        getrandom::fill(&mut random)
            .map_err(|e| refused(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        return request(ctx,Method::POST,"",Some(json!({"id":format!("auto-{}",hex::encode(random)),"name":values["name"].as_str().unwrap_or_default().trim(),"entries":[]}))).await;
    }
    let lists = &known.shown.lists;
    let Some(list) = lists.iter().find(|l| {
        id == format!("list-{}", l.id)
            || id == format!("models-{}", l.id)
            || l.entries
                .iter()
                .enumerate()
                .any(|(i, _)| id == entry_id(&l.id, i))
    }) else {
        return Err(stale(read(ctx).await));
    };
    let suffix = format!(
        "/{}",
        percent_encoding::utf8_percent_encode(&list.id, percent_encoding::NON_ALPHANUMERIC)
    );
    if id == format!("list-{}", list.id) {
        if action_id == "edit" {
            return request(ctx,Method::PUT,&suffix,Some(json!({"revision":list.revision,"name":values["name"].as_str().unwrap_or_default().trim()}))).await;
        }
        if action_id == "remove" {
            return request(
                ctx,
                Method::DELETE,
                &suffix,
                Some(json!({"revision":list.revision})),
            )
            .await;
        }
        let was = lists.iter().map(|l| l.id.clone()).collect::<Vec<_>>();
        let mut ids = was.clone();
        let index = ids.iter().position(|i| *i == list.id).unwrap();
        let other = if action_id == "up" {
            index.checked_sub(1)
        } else {
            index.checked_add(1)
        }
        .filter(|n| *n < ids.len())
        .ok_or_else(|| bad("Already at that end."))?;
        ids.swap(index, other);
        return request(
            ctx,
            Method::PUT,
            "/order",
            Some(json!({"ids":ids,"was":was})),
        )
        .await;
    }
    let mut entries = list.entries.clone();
    let index = entries
        .iter()
        .enumerate()
        .find(|(i, _)| id == entry_id(&list.id, *i))
        .map(|(i, _)| i);
    if matches!(action_id, "add-model" | "edit-model") {
        let model =
            serde_json::from_str::<Vec<String>>(values["model"].as_str().unwrap_or_default())
                .map_err(|_| bad("Choose a model."))?;
        let entry = Entry {
            provider: model[0].clone(),
            model: model[1].clone(),
            variant: values["variant"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
        };
        if action_id == "add-model" {
            entries.push(entry);
        } else {
            entries[index.ok_or_else(no_action)?] = entry;
        }
    } else if action_id == "remove-model" {
        entries.remove(index.ok_or_else(no_action)?);
    } else {
        let index = index.ok_or_else(no_action)?;
        let other = if action_id == "up" {
            index.checked_sub(1)
        } else {
            index.checked_add(1)
        }
        .filter(|i| *i < entries.len())
        .ok_or_else(|| bad("Already at that end."))?;
        entries.swap(index, other);
    }
    request(
        ctx,
        Method::PUT,
        &suffix,
        Some(json!({"revision":list.revision,"entries":entries})),
    )
    .await
}
