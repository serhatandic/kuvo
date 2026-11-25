//! Section descriptions and settings kept by the computer.
use super::*;
use kuvo_agents::session_view::{CHOICES, Compaction};
use kuvo_store::files;

fn description(
    section: Section,
) -> (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static [&'static str],
) {
    match section {
        Section::Clock => (
            "Clock",
            "General",
            "clock",
            "Your time zone, which bots' wake-ups and times of day go by on all your computers.",
            &[
                "time zone",
                "timezone",
                "clock",
                "wake-ups",
                "schedule",
                "travel",
            ],
        ),
        Section::Conversations => (
            "Conversations",
            "Agents & Tools",
            "message",
            "When supported agents summarize long conversations on this computer.",
            &["context", "tokens", "compact", "summarize"],
        ),
        Section::Permissions => (
            "Permissions",
            "Agents & Tools",
            "shield",
            "What agents may do without asking, and the sandbox their commands run in.",
            &[
                "approval",
                "full access",
                "ask first",
                "allow",
                "don't allow",
                "sandbox",
                "also allowed",
                "other computers",
                "rules",
            ],
        ),
        Section::Browser => (
            "Browser",
            "System",
            "globe",
            "The browser's search engine, and offering to save sign-ins.",
            &[
                "search engine",
                "google",
                "duckduckgo",
                "sign-ins",
                "passwords",
                "tabs",
                "never offered",
            ],
        ),
        Section::Computer => (
            "This Computer",
            "System",
            "computer",
            "Whether Kuvo runs here, its version and updates, and what may connect.",
            &[
                "running",
                "restart",
                "version",
                "update",
                "devices",
                "phone",
                "connect",
                "login items",
                "kuvo helper",
                "screen recording",
                "accessibility",
                "macos",
            ],
        ),
        Section::Git => (
            "Git",
            "Projects",
            "branch",
            "How sessions' branches are named, and commit messages and pull requests are written.",
            &[
                "branch prefix",
                "worktree",
                "commit",
                "pull request",
                "instructions",
            ],
        ),
        Section::Worktrees => (
            "Worktrees",
            "Projects",
            "folder",
            "Where new projects and worktrees go, and sessions’ own copies of a project.",
            &["projects", "folders", "branches", "remove"],
        ),
        Section::Storage => (
            "Storage",
            "System",
            "drive",
            "How much Kuvo keeps on this computer, and how much older versions of what agents made keep.",
            &[
                "copies",
                "older versions",
                "budget",
                "disk",
                "clear",
                "space",
                "size",
                "folder",
            ],
        ),
        Section::Archived => (
            "Archived Sessions",
            "System",
            "archive",
            "Sessions out of the sidebar, still searchable; bring them back or delete them.",
            &["archive", "unarchive", "delete", "sessions"],
        ),
        Section::Agents => (
            "Agents",
            "Agents & Tools",
            "bot",
            "The built-in agents and your own: how each is used, its model, instructions and what it may do.",
            &[
                "agents",
                "subagents",
                "modes",
                "build",
                "plan",
                "instructions",
                "prompt",
                "model",
                "permissions",
                "rules",
                "claude code",
                "import",
            ],
        ),
        Section::Vault => (
            "Vault",
            "Agents & Tools",
            "shield",
            "Sign-ins, keys, files and notes agents use without ever seeing them.",
            &[
                "vault",
                "secrets",
                "passwords",
                "sign-ins",
                "api keys",
                "files",
                "notes",
                "waiting for you",
                "who can use it",
                "pause",
                "activity",
            ],
        ),
        Section::Mcp => (
            "MCP Servers",
            "Agents & Tools",
            "sliders",
            "Programs and web services that give agents more tools.",
            &[
                "mcp",
                "servers",
                "tools",
                "add server",
                "sign in",
                "oauth",
                "command",
                "url",
                "timeout",
            ],
        ),
        Section::CustomAuto => (
            "Custom Auto",
            "Models",
            "sliders",
            "Your lists of models to go through in order when one reaches a usage limit.",
            &["auto", "models", "fallback", "limits", "effort"],
        ),
        Section::Skills => (
            "Skills",
            "Agents & Tools",
            "pencil",
            "Instructions and scripts agents load when a task calls for them.",
            &[
                "skills",
                "instructions",
                "scripts",
                "recently deleted",
                "catalog",
            ],
        ),
    }
}

pub async fn sections(ctx: &Context) -> Value {
    let computer = computer_title(ctx).await;
    json!({"sections":Section::ALL.into_iter().filter(|s| *s != Section::Skills).map(|s| {
        let (title, group, icon, line, keywords) = description(s);
        json!({"id":s.id(),"title":if s == Section::Computer { &computer } else { title },"group":group,"icon":icon,"line":line,"keywords":keywords})
    }).collect::<Vec<_>>()})
}

pub(super) async fn computer_title(ctx: &Context) -> String {
    let here = matches!(
        &ctx.request.caller,
        kuvo_server::Caller::Local(kuvo_server::Transport::Http {
            origin: kuvo_server::LocalOrigin::App,
            ..
        })
    );
    let info = ctx
        .ask(Method::GET, "kuvo/info", None)
        .await
        .unwrap_or_default();
    if here {
        if cfg!(target_os = "macos") {
            "This Mac".into()
        } else {
            "This Computer".into()
        }
    } else {
        info["name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("This Computer")
            .into()
    }
}

pub(super) async fn said(error: Refusal, fallback: &str) -> String {
    error
        .reply()
        .body
        .collect()
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes.to_bytes()).ok())
        .and_then(|v| {
            v["message"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| fallback.into())
}

fn conversations(ctx: &Context) -> Value {
    let mut card = group(
        "context",
        vec![json!({
            "id":"compaction","title":"Summarize long conversations at","kind":"choice",
            "line":"Smaller model windows summarize sooner. Never disables automatic summaries on large Claude Code models. Other agents may still summarize at their own limits.",
            "value":Compaction::read(&ctx.settings.home).id(),"can":true,
            "choices":CHOICES.into_iter().map(|c| json!({"id":c.id(),"title":match c { Compaction::Never => "Never (large windows)".into(), Compaction::At(n) => format!("{}k tokens",n/1000) }})).collect::<Vec<_>>(),
        })],
    );
    card["title"] = json!("Context");
    page("conversations", "Conversations", vec![card])
}

struct GitRow {
    id: &'static str,
    key: &'static str,
    group: &'static str,
    heading: Option<&'static str>,
    title: &'static str,
    line: &'static str,
    placeholder: &'static str,
}

const GIT_ROWS: [GitRow; 3] = [
    GitRow {
        id: "branch-prefix",
        key: "branch_prefix",
        group: "branches",
        heading: None,
        title: "Branch prefix",
        line: "How a session’s own branch in a worktree starts.",
        placeholder: "None",
    },
    GitRow {
        id: "commit-instructions",
        key: "commit_instructions",
        group: "commit-messages",
        heading: Some("Commit Instructions"),
        title: "Commit instructions",
        line: "Added when Suggest a Message in Changes asks the model for one.",
        placeholder: "For example: Start with the area changed, like “ui: …”, and never mention tickets.",
    },
    GitRow {
        id: "pull-request-instructions",
        key: "pull_request_instructions",
        group: "pull-requests",
        heading: Some("Pull Request Instructions"),
        title: "Pull request instructions",
        line: "Added when Create Pull Request writes the title and description.",
        placeholder: "For example: Add a Testing section, and link the issue if the branch names one.",
    },
];

fn clean_prefix(raw: &str) -> String {
    let mut result = String::new();
    for ch in raw.chars() {
        if ch.is_whitespace()
            || ch.is_ascii_control()
            || matches!(ch, '~' | '^' | ':' | '?' | '*' | '[' | '\\' | '\u{feff}')
        {
            continue;
        }
        if result.is_empty() && matches!(ch, '-' | '/' | '.') || ch == '.' && result.ends_with('.')
        {
            continue;
        }
        result.push(ch);
    }
    result
}

fn git_value(def: &GitRow, value: Option<&Value>) -> String {
    match value.and_then(Value::as_str) {
        Some(text) if def.key == "branch_prefix" => clean_prefix(text),
        Some(text) => text.trim().to_owned(),
        None if def.key == "branch_prefix" => "kuvo/".into(),
        None => String::new(),
    }
}

async fn git(ctx: &Context) -> Value {
    let config = match ctx.ask(Method::GET, "kuvo/config", None).await {
        Ok(value) => value,
        Err(error) => {
            return unread(
                "git",
                "Git",
                said(error, "This computer's settings couldn't be read.").await,
            );
        }
    };
    page("git","Git",GIT_ROWS.iter().map(|def| {
        let value = config.get(def.key);
        let can = if value.is_none_or(Value::is_string) { json!(true) } else { json!({"why":"This value is set in the settings file as something other than text. Change it there."}) };
        let mut row = json!({"id":def.id,"title":def.title,"placeholder":def.placeholder,"kind":"text","value":git_value(def,value),"can":can});
        if def.heading.is_some() { row["lines"] = json!({"min":5,"max":12}); } else { row["line"] = json!(def.line); }
        let mut card = group(def.group,vec![row]);
        if let Some(title) = def.heading { card["title"] = json!(title); card["note"] = json!(def.line); }
        card
    }).collect())
}

async fn browser(ctx: &Context) -> Value {
    let settings = match ctx.ask(Method::GET, "kuvo/browser/settings", None).await {
        Ok(value) => value,
        Err(error) => {
            return unread(
                "browser",
                "Browser",
                format!(
                    "The browser's settings couldn't be read: {}",
                    said(error, "they didn't answer.").await
                ),
            );
        }
    };
    let engines = settings["searchEngines"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| json!({"id":e["id"],"title":e["title"]}))
        .collect::<Vec<_>>();
    let never = settings["never"].as_array().into_iter().flatten().filter_map(Value::as_str).map(|host| json!({"id":format!("never-{host}"),"title":host,"kind":"actions","actions":[{"id":"offer-again","title":"Offer Again"}]})).collect::<Vec<_>>();
    page(
        "browser",
        "Browser",
        vec![
            group(
                "browser",
                vec![
                    json!({"id":"search-engine","title":"Search engine","line":"For anything typed in the address field that isn't an address.","kind":"choice","value":settings["searchEngine"],"choices":engines,"can":true}),
                    json!({"id":"tidies","title":"Close agents' tabs you haven't looked at","line":"An hour after an agent last used one, so tabs don't pile up.","kind":"switch","value":settings["tidiesAgentTabs"] == true,"can":true}),
                    json!({"id":"offers","title":"Offer to save sign-ins","line":"When you sign in to a site in the browser.","kind":"switch","value":settings["offersToSave"] == true,"can":true}),
                ],
            ),
            group(
                "never",
                vec![
                    json!({"id":"never","title":"Never Offered","line":"Sites you said Never to when it offered to save a sign-in.","kind":"list","empty":"None yet.","items":never}),
                ],
            ),
        ],
    )
}

pub async fn read(ctx: &Context, section: Section) -> Value {
    match section {
        Section::Clock => ctx.settings.setup.clock_page(),
        Section::Conversations => conversations(ctx),
        Section::Git => git(ctx).await,
        Section::Browser => browser(ctx).await,
        Section::Permissions => permissions::read(ctx).await,
        Section::Storage => storage::read(ctx).await,
        Section::Worktrees => worktrees::read(ctx).await,
        Section::Archived => archived::read(ctx).await,
        Section::Skills => skills::read(ctx).await,
        Section::Mcp => mcp::read(ctx).await,
        Section::Computer => computer::read(ctx).await,
        _ => unread(
            section.id(),
            description(section).0,
            "This computer's settings couldn't be read.",
        ),
    }
}

pub async fn change(
    ctx: &Context,
    section: Section,
    at: &Value,
    given: &Value,
) -> Result<Option<Value>, Refusal> {
    let value = &given["value"];
    match section {
        Section::Clock => {
            return ctx
                .settings
                .setup
                .change_clock(
                    value.as_str().unwrap_or_default().to_owned(),
                    given["was"].as_str().unwrap_or_default().to_owned(),
                )
                .await
                .map(Some);
        }
        Section::Conversations => {
            let choice = CHOICES
                .into_iter()
                .find(|c| value == &c.id())
                .ok_or_else(|| bad("Choose one of the listed thresholds."))?;
            if choice != Compaction::read(&ctx.settings.home) {
                files::write_secret(
                    &ctx.settings.home.join("box/conversation-compaction.json"),
                    &serde_json::to_vec(&choice.id()).expect("choice"),
                )
                .map_err(|e| {
                    refused(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("It couldn't be saved: {e}"),
                    )
                })?;
            }
        }
        Section::Git => {
            let def = GIT_ROWS
                .iter()
                .find(|r| at["id"] == r.id)
                .ok_or_else(|| bad("That isn't something to change."))?;
            let config = ctx.ask(Method::GET, "kuvo/config", None).await?;
            let actual = git_value(def, config.get(def.key));
            if at["value"] != actual {
                return Err(stale(git(ctx).await));
            }
            let value = git_value(def, Some(value));
            let result = ctx
                .ask(
                    Method::PATCH,
                    "kuvo/config",
                    Some(
                        json!({"changes":[{"path":[def.key],"value":value,"was":config[def.key]}]}),
                    ),
                )
                .await;
            result?;
        }
        Section::Permissions => permissions::change(ctx, at, value).await?,
        Section::Storage => storage::change(ctx, at, given).await?,
        Section::Worktrees => worktrees::change(ctx, at, value).await?,
        Section::Skills => skills::change(ctx, at, value).await?,
        Section::Mcp => mcp::change(ctx, at, value).await?,
        Section::Computer => computer::change(ctx, at, value).await?,
        Section::Browser => {
            let field = match at["id"].as_str() {
                Some("search-engine") => "searchEngine",
                Some("tidies") => "tidiesAgentTabs",
                Some("offers") => "offersToSave",
                _ => return Err(bad("That isn't something to change.")),
            };
            ctx.ask(
                Method::PATCH,
                "kuvo/browser/settings",
                Some(json!({field:value})),
            )
            .await?;
        }
        _ => return Err(bad("That isn't something to change.")),
    }
    Ok(None)
}

pub async fn act(
    ctx: &Context,
    section: Section,
    row: &Value,
    action: &Value,
    _values: &Value,
) -> Result<(), Refusal> {
    if section == Section::Mcp {
        return mcp::act(ctx, row, action, _values).await;
    }
    if section == Section::Archived {
        return archived::act(ctx, row, action).await;
    }
    if section == Section::Worktrees {
        return worktrees::act(ctx, row, action).await;
    }
    if section == Section::Computer {
        return computer::act(ctx, row, action).await;
    }
    if section == Section::Storage {
        return storage::act(ctx, row, action).await;
    }
    if section == Section::Skills {
        return skills::act(ctx, row, action, _values).await;
    }
    Err(no_action())
}

pub async fn check(
    ctx: &Context,
    section: Section,
    row: &Value,
    action: &Value,
    values: &Value,
) -> Value {
    if section == Section::Mcp {
        return mcp::check(ctx, row, action, values).await;
    }
    if section == Section::Skills {
        return skills::check(ctx, row, action, values).await;
    }
    json!({"fields":[],"ready":true})
}
