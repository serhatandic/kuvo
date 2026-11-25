//! Artifact storage rows use the same store's budget and deletion guards.
use super::pages::{computer_title, said};
use super::*;
use crate::artifacts::budget::{budget_confirmation, file_size, file_size_unit};

fn unit(bytes: u64) -> Value {
    let unit = file_size_unit(bytes);
    json!({"name":unit.name,"factor":unit.factor,"decimals":unit.decimals,"rounding":unit.rounding})
}

pub async fn read(ctx: &Context) -> Value {
    let title = computer_title(ctx).await;
    let where_ = match title.as_str() {
        "This Mac" => "this Mac",
        "This Computer" => "this computer",
        other => other,
    };
    let (sizes, measuring, problem) = ctx.settings.measurement.read(&ctx.settings.home, false);
    let sizes = sizes.map(|s| serde_json::to_value(s).expect("sizes"));
    let mut meter = json!({"id":"used","title":"Measuring what Kuvo keeps…","kind":"meter","actions":[{"id":"refresh","title":"Refresh"}]});
    if let Some(sizes) = &sizes {
        meter["title"] = json!(format!(
            "{} used by Kuvo",
            file_size(sizes["total"].as_u64().unwrap_or(0))
        ));
        meter["parts"] = json!(sizes["parts"].as_array().into_iter().flatten().map(|p| {
            let bytes = p["bytes"].as_u64().unwrap_or(0);
            let mut part = json!({"id":p["id"],"title":p["title"],"value":bytes,"words":file_size(bytes)});
            if let Some(folders) = p["folders"].as_array().filter(|f|!f.is_empty()) {
                part["parts"] = json!(folders.iter().map(|f| json!({"title":f["title"],"value":f["bytes"],"words":file_size(f["bytes"].as_u64().unwrap_or(0))})).collect::<Vec<_>>());
            }
            part
        }).collect::<Vec<_>>());
    }
    if measuring {
        meter["busy"] = json!("Measuring…");
    }
    if !measuring && let Some(problem) = problem {
        meter["line"] = json!(format!("It couldn’t be measured just now: {problem}"));
        meter["tone"] = json!("warning");
    }
    let mut computer = group("computer", vec![meter]);
    computer["title"] = json!(title);
    computer["note"] = json!(format!("What Kuvo keeps on {where_}."));
    let mut folder = json!({"id":"home-folder","title":"Kuvo’s folder","line":crate::config::shown_root(&ctx.settings.home),"kind":"status"});
    if let Some(bytes) = sizes.as_ref().and_then(|s| s["home"].as_u64()) {
        folder["value"] = json!(bytes);
        folder["unit"] = unit(bytes);
    }
    let mut rows = vec![folder];
    if let Some(sizes) = sizes {
        for (index, each) in sizes["outside"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let n = each["trees"].as_u64().unwrap_or(0);
            let bytes = each["bytes"].as_u64().unwrap_or(0);
            rows.push(json!({"id":format!("worktree-folder-{index}"),"title":format!("{n} {} in",if n == 1 {"worktree"} else {"worktrees"}),"line":each["path"],"kind":"status","value":bytes,"unit":unit(bytes)}));
        }
    }
    let mut folders = group("folders", rows);
    folders["title"] = json!("Storage by Folder");
    let mut groups = vec![computer, folders];
    let mut notes = Vec::new();
    match ctx.ask(Method::GET,"kuvo/artifacts/storage",None).await {
        Ok(kept) => {
            let copies = kept["copies"].as_u64().unwrap_or(0);
            let budget = kept["budget"].as_u64().unwrap_or(0);
            let older = kept["older"].as_u64().unwrap_or(0);
            let confirmation = budget_confirmation(older);
            let unit_budget = if budget < 1_000_000_000 {json!({"name":"MB","factor":1_000_000,"decimals":2,"rounding":"nearest"})}else{json!({"name":"GB","factor":1_000_000_000,"decimals":2,"rounding":"nearest"})};
            let mut made = group("agents-made",vec![
                json!({"id":"copies","title":"Copies of what agents made","line":"Each version of the files agents write or change, so you can see what changed and put one back. Never what looks like a secret, and only the changes for code git keeps.","kind":"status","value":copies,"unit":unit(copies)}),
                json!({"id":"older-budget","title":"Older versions keep up to","line":"Past it, the oldest go first. The newest version of each file, memory, skills and what’s kept always stay.","kind":"number","value":budget,"unit":unit_budget,"min":0,"can":true,"suggestions":[{"value":250000000,"title":"0.25 GB"},{"value":500000000,"title":"0.5 GB"},{"value":1000000000,"title":"1 GB"},{"value":2000000000u64,"title":"2 GB"},{"value":5000000000u64,"title":"5 GB"},{"value":10000000000u64,"title":"10 GB"}],"confirmBelow":{"value":older,"confirm":{"title":confirmation.title,"body":confirmation.body,"button":confirmation.button}}}),
                json!({"id":"older-copies","title":"Older versions","line":if older == 0 {"None kept.".into()}else{format!("{} now.",file_size(older))},"kind":"actions","value":older,"actions":[{"id":"clear","title":"Clear…","style":"destructive","can":if older > 0 {json!(true)} else {json!({"why":"None kept."})},"confirm":{"title":"Clear older versions?","body":"Their copies go; each stays listed, without one. The newest version of each file, memory, skills and what’s kept stay.","button":"Clear"}}]}),
            ]);
            made["title"] = json!("What Agents Made"); groups.push(made);
            if kept["low"] == true {notes.push(json!({"words":format!("The disk is nearly full, so only the newest version of each file keeps a copy until {} is free again.",file_size(kept["floor"].as_u64().unwrap_or(0))),"tone":"warning"}));}
        }
        Err(e) => notes.push(json!({"words":format!("What agents made couldn’t be read: {}",said(e,"it didn’t answer.").await),"tone":"warning"})),
    }
    groups.push(group("manage",vec![
        json!({"id":"manage-worktrees","title":"Worktrees","line":"Sessions’ own copies of a project, and where they go.","kind":"open","section":"worktrees"}),
        json!({"id":"manage-archived","title":"Archived Sessions","line":"Bring sessions back, or delete them for good.","kind":"open","section":"archived"}),
    ]));
    let mut page = page("storage", "Storage", groups);
    if !notes.is_empty() {
        page["notes"] = json!(notes);
    }
    page
}

pub async fn change(ctx: &Context, at: &Value, given: &Value) -> Result<(), Refusal> {
    if at["id"] != "older-budget" {
        return Err(bad("That isn't something to change."));
    }
    let mut body = json!({"budget":given["value"].as_f64().unwrap_or_default().floor(),"was":at["value"],"wasOlder":at["confirmBelow"]["value"]});
    if let Some(confirmed) = given.get("confirmed") {
        body["confirmed"] = confirmed.clone();
    }
    ctx.ask(Method::POST, "kuvo/artifacts/storage", Some(body))
        .await?;
    Ok(())
}
pub async fn act(ctx: &Context, at: &Value, action: &Value) -> Result<(), Refusal> {
    if at["id"] == "used" && action["id"] == "refresh" {
        ctx.settings.measurement.read(&ctx.settings.home, true);
        return Ok(());
    }
    if at["id"] == "older-copies" && action["id"] == "clear" {
        ctx.ask(
            Method::POST,
            "kuvo/artifacts/storage/clear",
            Some(json!({"was":at["value"]})),
        )
        .await?;
        return Ok(());
    }
    Err(no_action())
}
