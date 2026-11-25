//! This computer's settings and the resident's existing restart/update routes.
use super::pages::computer_title;
use super::*;
use kuvo_agents::program_updates::notices::agents_update_themselves;
use kuvo_store::files;

pub async fn read(ctx: &Context) -> Value {
    let title = computer_title(ctx).await;
    let info = ctx
        .ask(Method::GET, "kuvo/info", None)
        .await
        .unwrap_or_default();
    let name = info["name"].as_str().unwrap_or("this computer");
    let where_ = match title.as_str() {
        "This Mac" => "this Mac",
        "This Computer" => "this computer",
        other => other,
    };
    let mac = info["kind"] == "mac";
    let running = std::fs::read(ctx.settings.home.join("run/engine.json"))
        .ok()
        .and_then(|v| serde_json::from_slice::<Value>(&v).ok())
        .is_some_and(|v| v["port"].as_u64().is_some_and(|p| p > 0));
    let working = ctx
        .ask(Method::GET, "kuvo/box/at-work", None)
        .await
        .ok()
        .and_then(|v| v["working"].as_u64())
        .map(|n| n != 0);
    let stops = match working {
        Some(true) => format!("Agents are at work on {where_} now: what they’re doing stops."),
        Some(false) => format!("Nothing is at work on {where_} now."),
        None => format!("What agents are doing on {where_} now stops."),
    };
    let mut state = json!({"id":"state","title":if running{"Running"}else{"Not running"},"kind":"actions","value":running,"line":if !running{"Its engine isn’t running, so no agent works here. Restart starts it again."}else if mac{"Kuvo Helper keeps it running, and starts it at login."}else{"It runs whether or not an app is open."},"actions":[{"id":"restart","title":"Restart…","confirm":{"title":format!("Restart Kuvo on {where_}?"),"body":format!("{stops} Kuvo is back in a moment."),"button":"Restart"}}]});
    if !running {
        state["tone"] = json!("warning");
    }
    let update = ctx
        .ask(Method::GET, "kuvo/update", None)
        .await
        .unwrap_or_default();
    let version = update["version"]
        .as_str()
        .map(|v| format!("Kuvo Code {v}"))
        .unwrap_or_else(|| "Kuvo Code".into());
    let mut version_row = json!({"id":"version","title":version,"kind":"status"});
    if let Some(updating) = update["updating"].as_str() {
        version_row["line"] = json!(if updating == "checking" {
            "Looking for a newer version."
        } else {
            "Getting the release ready."
        });
        version_row["busy"] = json!(if updating == "checking" {
            "Checking…"
        } else {
            "Updating…"
        });
    } else if update["setUp"] != true || update["blocked"].is_string() {
        version_row["line"] = json!(
            update["blocked"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("Updates aren’t set up for {where_} yet."))
        );
    } else {
        let setup = ctx
            .ask(Method::GET, "kuvo/setup", None)
            .await
            .unwrap_or_default();
        let main = setup["you"]["role"] != "member";
        version_row["line"] = json!(if update["last"]["result"] == "failed" {
            update["last"]["message"]
                .as_str()
                .unwrap_or("This computer updates with your setup.")
        } else {
            "This computer updates with your setup."
        });
        version_row["kind"] = json!(if main { "actions" } else { "status" });
        if let Some(v) = update.get("version") {
            version_row["value"] = v.clone();
        }
        version_row["actions"] = if main {
            json!([{"id":"update","title":"Update Now"}])
        } else {
            json!([])
        };
    }
    let reach = format!("Your phone and other computers reach {where_}, end to end encrypted.");
    let devices = if mac {
        json!({"id":"devices","title":"Let my devices connect","line":reach,"kind":"switch","value":false,"can":{"why":"This copy of Kuvo has no relay for your devices to connect through."}})
    } else {
        json!({"id":"devices","title":"Let my devices connect","line":format!("{reach} A computer you added always lets them in."),"kind":"status","value":"On"})
    };
    let mut agents = group(
        "agents",
        vec![
            json!({"id":"agents-update","title":"Update coding agents automatically","kind":"switch","value":agents_update_themselves(&ctx.settings.home),"can":true,"line":"Off: each agent keeps its version until you update it in Accounts. On: agents update themselves as they run."}),
        ],
    );
    agents["title"] = json!("Coding Agents");
    let mut groups = vec![group("computer", vec![state, version_row, devices]), agents];
    if let Ok(permissions) = ctx.ask(Method::GET, "kuvo/box/permissions", None).await
        && permissions["applies"] == true
    {
        let helper = permissions["name"].as_str().unwrap_or("Kuvo Helper");
        let here = matches!(
            &ctx.request.caller,
            kuvo_server::Caller::Local(kuvo_server::Transport::Http {
                origin: kuvo_server::LocalOrigin::App,
                ..
            })
        );
        let mut rows = Vec::new();
        for each in permissions["list"].as_array().into_iter().flatten() {
            let id = each["id"].as_str().unwrap_or_default();
            let line = each["line"].as_str().unwrap_or_default();
            let mut row = json!({"id":format!("allows-{id}"),"title":each["name"],"line":line,"kind":"status","value":"Allowed"});
            if each["allowed"] == false {
                row["tone"] = json!("warning");
                row["kind"] = json!("actions");
                row["value"] = json!("Not allowed");
                row["line"] = json!(match id {
                    "screenRecording" => "Not allowed. Agents can’t look at the screen.",
                    "accessibility" => "Not allowed. Agents can’t click or type in other apps.",
                    "fullDiskAccess" =>
                        "Not allowed. Agents may be blocked from opening protected files.",
                    _ => line,
                });
                let action = if here {
                    json!({"id":format!("open-{id}"),"title":"Open System Settings","opens":each["settings"]})
                } else {
                    json!({"id":format!("open-{id}"),"title":"Open System Settings","can":{"why":format!("Open it on {name} itself.")}})
                };
                row["actions"] = json!([action]);
            } else if each["allowed"] != true {
                row["value"] = json!("Not known yet");
                row["line"] = json!(
                    format!(
                        "{} {line}",
                        each["unknownLine"].as_str().unwrap_or_default()
                    )
                    .trim()
                );
            }
            rows.push(row);
        }
        let mut card = group("macos", rows);
        card["title"] = json!(format!("What macOS Allows {helper}"));
        card["note"] = json!(format!(
            "{helper} is the program your agents run under. Only you allow these, in System Settings."
        ));
        groups.push(card);
    }
    page("computer", &title, groups)
}

pub async fn change(ctx: &Context, at: &Value, value: &Value) -> Result<(), Refusal> {
    if at["id"] != "agents-update" {
        return Err(bad("That isn't something to change."));
    }
    if value.as_bool().unwrap_or_default() != agents_update_themselves(&ctx.settings.home) {
        files::write_secret(
            &ctx.settings.home.join("box/agents-update-themselves.json"),
            &serde_json::to_vec(value).expect("boolean"),
        )
        .map_err(|e| {
            refused(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("It couldn't be saved: {e}"),
            )
        })?;
    }
    Ok(())
}
pub async fn act(ctx: &Context, at: &Value, action: &Value) -> Result<(), Refusal> {
    match (at["id"].as_str(), action["id"].as_str()) {
        (Some("state"), Some("restart")) => {
            ctx.ask(Method::POST, "kuvo/engine/restart", None).await?;
        }
        (Some("version"), Some("update")) => {
            ctx.ask(Method::POST, "kuvo/update", Some(json!({"now":true})))
                .await?;
        }
        _ => return Err(no_action()),
    }
    Ok(())
}
