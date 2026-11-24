//! Start an engine conversation, resolving its project and keeping its ancestry at creation.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use http::{Method, StatusCode};
use kuvo_protocol::routes::post_kuvo_sessions as start;
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use kuvo_wire::keys;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::{fs, process::Command, time};

use crate::{
    Engine, api, disk,
    lifetime::Lease,
    parts,
    queue::{Delivery, Prompt},
    send,
    session_list::LocalSessions,
    sessions::{self, Session},
};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Approval {
    AskFirst,
    AcceptEdits,
    Auto,
    FullAccess,
}

#[derive(Clone, Deserialize)]
struct Project {
    directory: PathBuf,
    name: Option<String>,
    #[serde(default)]
    folders: Vec<PathBuf>,
    #[serde(default)]
    removed: bool,
}

#[derive(Deserialize)]
struct Projects {
    projects: Vec<Project>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Permission {
    action: String,
    resource: String,
    effect: String,
}

impl Permission {
    pub fn new(action: &str, resource: &str, effect: &str) -> Self {
        Self {
            action: action.into(),
            resource: resource.into(),
            effect: effect.into(),
        }
    }

    fn denied(&self) -> bool {
        !matches!(self.effect.as_str(), "ask" | "allow")
    }
}

#[derive(Deserialize)]
struct ModelRef {
    #[serde(rename = "providerID")]
    provider: String,
    id: String,
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Wake {
    Finished,
    Asks,
}

fn wakes(given: &Value, by_bot: bool, waits: bool) -> Vec<Wake> {
    [Wake::Finished, Wake::Asks]
        .into_iter()
        .filter(|wake| {
            if given.is_null() {
                return by_bot && (!waits || *wake != Wake::Finished);
            }
            if given == "both" {
                return true;
            }
            if let Some(named) = given.as_array() {
                return named
                    .iter()
                    .any(|name| Wake::deserialize(name).ok().as_ref() == Some(wake));
            }
            Wake::deserialize(given).ok().as_ref() == Some(wake)
        })
        .collect()
}

pub(crate) fn register(engine: &Engine, list: LocalSessions, routes: &mut Registry) {
    let engine = engine.clone();
    routes.register::<start::Spec, _, _>(Access::Any, move |call| {
        let engine = engine.clone();
        let list = list.clone();
        async move {
            let given = sessions::body(&call)
                .map_err(|_| bad("Send {task}, and title, folder, startedBy, wait."))?;
            let mut started = tokio::spawn(async move {
                let started = create(&engine, &given).await?;
                list.refresh();
                Ok::<_, Refusal>(started)
            })
            .await
            .map_err(|error| Refusal::new(StatusCode::SERVICE_UNAVAILABLE, error.to_string()))??;
            // The reference includes the complete worktree record. The route
            // shape names only the fields desktop clients read.
            let extras = ["worktree", "wakes"]
                .into_iter()
                .filter_map(|field| Some((field, started.as_object_mut()?.remove(field)?)))
                .collect::<Vec<_>>();
            RouteReply::json_with::<200>(sessions::typed(started)?, extras)
                .map_err(|error| Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
        }
    });
}

fn bad(message: impl Into<String>) -> Refusal {
    Refusal::new(StatusCode::BAD_REQUEST, message)
}

async fn create(engine: &Engine, given: &Map<String, Value>) -> Result<Value, Refusal> {
    let string = |field| {
        given
            .get(field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
    };
    let flag = |field| given.get(field) == Some(&Value::Bool(true));
    let task = string("task");
    let title = string("title");
    let wait = flag("wait");
    let starter = given.get("startedBy").filter(|value| !value.is_null());
    if task.is_none() && (wait || starter.is_some()) {
        return Err(bad("Say what it's to do (task)."));
    }
    let link = disk::read::<Value>(&engine.home.join("run/link.json"))
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    let name = link["name"]
        .as_str()
        .filter(|name| !name.is_empty())
        .unwrap_or("a machine");
    let addressed = if let Some(computer) =
        string("computer").filter(|computer| *computer != "here")
    {
        let id = link["key"]
            .as_str()
            .and_then(keys::decode_exact::<32>)
            .map(|key| keys::encode(&keys::Identity::from_bytes(key, [0; 32]).public()))
            .unwrap_or_else(|| "here".into());
        let computer_info = json!({ "id": id, "name": name });
        if id != computer {
            return Err(Refusal::json(
                StatusCode::CONFLICT,
                json!({
                    "message": "Send the start request to the computer you chose.", "computer": computer_info
                }),
            ));
        }
        Some(computer_info)
    } else {
        None
    };

    // The engine starts only for what needs it: its own conversations, a
    // worktree, or a parent it keeps.
    let mut lease = Engaged::new(engine);
    let parent = match starter {
        Some(value) => Some(
            root(
                engine,
                &mut lease,
                value.as_str().filter(|id| sessions::valid_id(id)),
            )
            .await
            .ok_or_else(|| {
                Refusal::new(StatusCode::NOT_FOUND, "There's no such conversation here.")
            })?,
        ),
        None => None,
    };
    let parent_notes = |field| {
        parent
            .as_ref()
            .and_then(Session::notes)
            .and_then(|notes| notes.get(field))
            .unwrap_or(&Value::Null)
    };
    if !parent_notes("seat").is_null() {
        return Err(Refusal::new(
            StatusCode::FORBIDDEN,
            "Not from a group chat: talk it through here; the work is handed out after the wrap-up.",
        ));
    }
    let parent_agent = parent
        .as_ref()
        .and_then(|parent| parent.extra.get("agent"))
        .and_then(Value::as_str);
    let mode = given.get("mode").filter(|value| !value.is_null());
    if mode.is_some_and(|mode| !matches!(mode.as_str(), Some("build" | "plan"))) {
        return Err(bad("mode is build or plan."));
    }
    if mode.is_some_and(|mode| mode == "build") && parent_agent == Some("plan") {
        return Err(bad(
            "This conversation is in Plan mode, so the sessions it starts plan too. Start it in plan mode, or ask the user to switch to Build.",
        ));
    }

    let config = if engine.is_mac_box().await {
        "config/opencode/opencode.json"
    } else {
        "box/opencode.json"
    };
    let settings = disk::read::<Value>(&engine.home.join(config))
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    let approval_file = engine.home.join("box/approval.json");
    let approvals = disk::read::<Value>(&approval_file)
        .await
        .ok()
        .flatten()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    let allows_full = approvals["fullAccess"] == true;
    let default = from_approval(&approvals["last"])
        .filter(|last| allows_full || *last != Approval::FullAccess)
        .unwrap_or(
            if kuvo_os::platform::Platform::current().is_windows_host() {
                Approval::AskFirst
            } else {
                Approval::Auto
            },
        );
    let requested = match given.get("approval").filter(|value| !value.is_null()) {
        Some(value) if value == "ask" => Some(Approval::AskFirst),
        Some(value) if value == "approveAll" => Some(default),
        Some(value) => Some(
            from_approval(value)
                .ok_or_else(|| bad("approval is askFirst, acceptEdits, auto or fullAccess."))?,
        ),
        None => None,
    };
    if parent.is_none() && requested == Some(Approval::FullAccess) && !allows_full {
        return Err(Refusal::json(
            StatusCode::CONFLICT,
            json!({ "message": "Full Access isn't confirmed on this computer yet: confirm it first.", "reason": "fullAccessOff" }),
        ));
    }
    let parent_mode = from_approval(parent_notes("approval"));
    let wanted = if parent.is_some() {
        let upper = parent_mode.unwrap_or(Approval::AskFirst);
        requested.unwrap_or(upper).min(upper)
    } else {
        requested.unwrap_or(default)
    };
    // The engine supervisor does not yet install the sandbox policy from the settings port.
    let applied = match wanted {
        Approval::Auto => Approval::AskFirst,
        Approval::FullAccess if !allows_full => Approval::AskFirst,
        wanted => wanted,
    };

    let projects = disk::read::<Projects>(&engine.home.join("box/projects.json"))
        .await
        .ok()
        .flatten()
        .map_or_else(Vec::new, |file| {
            file.projects
                .into_iter()
                .filter(|project| !project.removed)
                .collect()
        });
    let chats = engine.home.join("chats");
    let mut named = None;
    let mut folder = if flag("chat") {
        chats.clone()
    } else {
        if let Some(project) = string("project") {
            named = Some(
                projects
                    .iter()
                    .find(|entry| {
                        entry.directory == Path::new(project)
                            || entry
                                .name
                                .as_deref()
                                .is_some_and(|name| name.eq_ignore_ascii_case(project))
                    })
                    .ok_or_else(|| {
                        bad(format!("There's no project called {project} on {name}."))
                    })?,
            );
        }
        let supplied = string("folder").map(PathBuf::from);
        let mut path = supplied
            .clone()
            .or_else(|| named.map(|project| project.directory.clone()))
            .or_else(|| {
                parent
                    .as_ref()
                    .map(|parent| PathBuf::from(parent.directory()))
            })
            .ok_or_else(|| bad("Give the folder it works in (folder), as a full path."))?;
        if !path.is_absolute() || !fs::metadata(&path).await.is_ok_and(|meta| meta.is_dir()) {
            return Err(bad(format!(
                "{} isn't a folder on {name}. Give a full path to one.",
                path.display()
            )));
        }
        let mapped = linked_location(&path).await;
        if let Some(project) = named
            && !in_project(&path, mapped.as_deref(), project).await
        {
            let title = project_name(project);
            return Err(bad(format!(
                "{} isn't in {title} on {name}: give a folder in it or in one of its worktrees, or leave out project.",
                path.display()
            )));
        }
        if parent.is_some() {
            path = fs::canonicalize(&path)
                .await
                .map_err(|_| bad("Give the folder it works in (folder), as a full path."))?;
            if cfg!(windows)
                && let Some(ordinary) = kuvo_os::platform::windows_conventional_path(&path)
                && fs::canonicalize(&ordinary)
                    .await
                    .as_ref()
                    .is_ok_and(|resolved| resolved == &path)
            {
                path = ordinary;
            }
            let allowed = parent
                .as_ref()
                .is_some_and(|parent| Path::new(parent.directory()) == path)
                || touched_project(&path, mapped.as_deref(), &projects)
                    .await
                    .is_some()
                || path.starts_with(&chats);
            if !allowed && applied != Approval::FullAccess {
                return Err(Refusal::new(
                    StatusCode::FORBIDDEN,
                    format!(
                        "An agent in Auto can't start a session in {} on {name}. Ask the user to start it there.",
                        path.display()
                    ),
                ));
            }
        }
        if path.starts_with(engine.home.join("bots")) {
            chats.clone()
        } else {
            path
        }
    };
    let chat = folder == chats || folder.starts_with(&chats);
    let mapped = if chat || named.is_some() {
        None
    } else {
        linked_location(&folder).await
    };
    let project = if chat {
        None
    } else {
        named.or(touched_project(&folder, mapped.as_deref(), &projects).await)
    };
    let under = project
        .map(|project| project.directory.clone())
        .or_else(|| {
            (parent_notes("filed")["folder"].as_str() == folder.to_str())
                .then(|| parent_notes("filed")["project"].as_str().map(PathBuf::from))
                .flatten()
        })
        .or_else(|| (!chat).then(|| folder.clone()));
    let project_title = project.map(project_name).or_else(|| {
        under
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .map(str::to_owned)
    });

    let model = given
        .get("model")
        .filter(|model| !model.is_null())
        .cloned()
        .or_else(|| parent.as_ref()?.extra.get("model").cloned());
    if let Some(model) = &model {
        let model: ModelRef = serde_json::from_value(model.clone())
            .map_err(|_| bad("Choose a model by its provider and id."))?;
        if model.id.is_empty() || model.provider.is_empty() {
            return Err(bad("Choose a model by its provider and id."));
        }
        if model.provider.split('@').next() == Some("auto") {
            return Err(Refusal::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "The coding agents aren't running on this computer.",
            ));
        }
    }
    let agents = match model
        .as_ref()
        .and_then(|model| model["providerID"].as_str())
        .and_then(|provider| provider.split('@').next())
    {
        Some("claude-code" | "codex-cli" | "cursor" | "grok-cli" | "antigravity") => {
            Some(engine.agent_sessions.clone().ok_or_else(|| {
                Refusal::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "The coding agents aren't running on this computer.",
                )
            })?)
        }
        _ => None,
    };

    let created_tree = if flag("worktree") {
        let under = under
            .as_ref()
            .ok_or_else(|| bad("A chat has no worktree: give a project."))?;
        if named.is_some() && &folder != under {
            return Err(bad(
                "Give a folder or worktree: true, not both: a new worktree is made from the project.",
            ));
        }
        let tree = make_tree(engine, lease.get().await?, &settings, under).await?;
        folder = tree.folder.clone();
        Some(tree)
    } else {
        None
    };

    let mut notes = json!({ "approval": wanted, "applied": applied });
    if chat {
        notes["chat"] = json!(folder);
    }
    if let Some(under) = under.as_ref().filter(|under| **under != folder) {
        notes["filed"] = json!({ "folder": folder, "project": under });
    }
    if let Some(parent) = &parent {
        notes["startedBy"] = json!(parent.id);
        let mut line = parent_notes("line")
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        if line.is_empty()
            && let Some(parent) = parent_notes("startedBy").as_str()
        {
            line.push(parent);
        }
        line.push(&parent.id);
        if line.len() > 1 {
            notes["line"] = json!(&line[line.len().saturating_sub(20)..]);
        }
    }
    let autopilot = if parent.is_some() {
        parent_notes("autopilot") == true
    } else {
        flag("autopilot")
    };
    if autopilot {
        if let Some(tree) = &created_tree
            && let Some(lease) = lease.held()
        {
            undo_tree(engine, lease, tree).await;
        }
        return Err(Refusal::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Autopilot isn't running on this computer.",
        ));
    }
    let mut body = json!({ "location": { "directory": folder }, "metadata": { "kuvo": notes },
        "permissions": permissions(applied, &settings, parent.as_ref(), project) });
    let agent = mode.and_then(Value::as_str).or_else(|| {
        if parent.is_none() {
            string("agent")
        } else {
            parent_agent
        }
    });
    if let Some(agent) = agent {
        body["agent"] = json!(agent);
    }
    if let Some(title) = title {
        body["title"] = json!(title);
    }
    if let Some(model) = model {
        body["model"] = model;
    }
    let result = match &agents {
        Some(source) => create_on_agents(engine, source.as_ref(), body).await,
        None => engine
            .call::<Value>(lease.get().await?, Method::POST, "api/session", Some(&body))
            .await
            .map_err(Made::Engine),
    };
    let made = match result {
        Ok(info) if info["id"].as_str().is_some_and(sessions::valid_id) => info,
        result => {
            if let Some(tree) = &created_tree
                && !matches!(&result, Err(Made::Engine(api::Error::Transport(_))))
                && let Some(lease) = lease.held()
                && tree_unused(engine, lease, tree).await
            {
                undo_tree(engine, lease, tree).await;
            }
            let reason = result
                .err()
                .map_or_else(|| "No session was made.".into(), |error| error.to_string());
            return Err(Refusal::new(
                StatusCode::BAD_GATEWAY,
                format!("Couldn't start a session in {}: {reason}", folder.display()),
            ));
        }
    };
    let id = made["id"].as_str().expect("checked session id");
    if parent.is_none() && requested.is_some() {
        let _turn = engine.turn("approval-settings").await;
        if let Ok(latest) = disk::read::<Value>(&approval_file).await {
            let mut updated = latest.filter(Value::is_object).unwrap_or_else(|| json!({}));
            updated["last"] = json!(wanted);
            if !updated["since"].is_number() {
                updated["since"] = json!(disk::now());
            }
            let _ = disk::durable(approval_file, updated).await;
        }
    }
    let mut answer =
        json!({ "session": id, "title": title, "folder": folder, "project": project_title });
    if applied < requested.unwrap_or(wanted) {
        answer["got"] = json!(applied);
    }
    if let Some(computer) = addressed {
        answer["computer"] = computer;
    }
    if let Some(task) = task {
        let requested = parent.as_ref().map_or_else(Vec::new, |parent| {
            wakes(
                given.get("wake").unwrap_or(&Value::Null),
                Path::new(parent.directory()).starts_with(engine.home.join("bots")),
                wait,
            )
        });
        let marked = if requested.is_empty() {
            false
        } else if let Ok(mut writer) = engine.note_writer(id).await {
            writer
                .edit(crate::NoteEdits {
                    kuvo: [("wake".to_owned(), Some(json!(requested)))].into(),
                    ..Default::default()
                })
                .await
                .is_ok()
        } else {
            false
        };
        if marked {
            answer["wakes"] = json!(requested);
        }
        let text = parent.as_ref().map_or_else(
            || task.to_owned(),
            |parent| {
                let title = if parent.title.is_empty() {
                    "another agent"
                } else {
                    &parent.title
                };
                let note = if wait {
                    "They're waiting for your reply: just answer, in your words.".to_owned()
                } else if marked && requested.contains(&Wake::Finished) {
                    format!("They get your last reply when you finish: end with your report there, not in a message. Use send_message to {} only to ask something or say how it's going.", parent.id)
                } else {
                    format!("To answer, use send_message to {}.", parent.id)
                };
                format!("[Message from {title} · {}] ({note})\n\n{task}", parent.id)
            },
        );
        let metadata = json!({ "kuvo": { "parts": parts::read_parts(&text) } });
        let prompt = Prompt {
            id: send::new_id(engine)?,
            text,
            metadata: Some(metadata),
            files: Vec::new(),
            delivery: Delivery::Steer,
        };
        let sent = match &agents {
            Some(source) => {
                let Prompt { id: message, text, metadata, .. } = prompt;
                source
                    .prompt(
                        id,
                        crate::AgentPrompt {
                            id: message,
                            text,
                            metadata: metadata.and_then(|notes| match notes {
                                Value::Object(notes) => Some(notes),
                                _ => None,
                            }),
                        },
                    )
                    .await
                    .is_ok()
            }
            None => send::post(engine, lease.get().await?, id, &prompt).await.is_ok(),
        };
        if !sent {
            return Err(Refusal::json(
                StatusCode::BAD_GATEWAY,
                json!({ "session": id,
                "message": format!("Started a session (address {id}), but the task didn't reach it.") }),
            ));
        }
        if wait {
            answer["answer"] = Value::Null;
        }
    } else {
        // A coding agent's conversation is told as clients read it.
        answer["info"] = match &agents {
            Some(source) => source
                .session(id)
                .await
                .ok()
                .flatten()
                .map_or(made, Value::Object),
            None => made,
        };
        if let Some(tree) = created_tree {
            answer["worktree"] = tree.record;
        }
    }
    Ok(answer)
}

fn from_approval(value: &Value) -> Option<Approval> {
    Approval::deserialize(value).ok()
}

/// The engine, started the first time something needs it.
struct Engaged<'a> {
    engine: &'a Engine,
    lease: Option<Lease>,
}

impl<'a> Engaged<'a> {
    fn new(engine: &'a Engine) -> Self {
        Self {
            engine,
            lease: None,
        }
    }

    async fn get(&mut self) -> Result<&Lease, Refusal> {
        if self.lease.is_none() {
            self.lease = Some(api::start(self.engine).await?);
        }
        Ok(self.lease.as_ref().expect("the lease was just taken"))
    }

    fn held(&self) -> Option<&Lease> {
        self.lease.as_ref()
    }
}

/// Why no conversation was made.
enum Made {
    Engine(api::Error),
    Agents(std::io::Error),
}

impl std::fmt::Display for Made {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Engine(error) => error.fmt(out),
            Self::Agents(error) => error.fmt(out),
        }
    }
}

async fn create_on_agents(
    engine: &Engine,
    source: &dyn crate::AgentSessionSource,
    body: Value,
) -> Result<Value, Made> {
    let Value::Object(mut body) = body else {
        return Err(Made::Agents(std::io::ErrorKind::InvalidInput.into()));
    };
    let mut take = |key: &str| body.remove(key);
    let new = crate::NewAgentSession {
        id: send::new_id_with(engine, "ses")
            .map_err(|_| Made::Agents(std::io::Error::other("No session ID could be made.")))?,
        directory: take("location")
            .and_then(|location| location["directory"].as_str().map(str::to_owned))
            .unwrap_or_default(),
        agent: take("agent").and_then(|agent| agent.as_str().map(str::to_owned)),
        model: take("model").unwrap_or(Value::Null),
        title: take("title").and_then(|title| title.as_str().map(str::to_owned)),
        permissions: take("permissions"),
        metadata: match take("metadata") {
            Some(Value::Object(notes)) => notes,
            _ => Map::new(),
        },
    };
    source
        .create(new)
        .await
        .map(Value::Object)
        .map_err(Made::Agents)
}

async fn root(engine: &Engine, lease: &mut Engaged<'_>, id: Option<&str>) -> Option<Session> {
    let mut found: Option<Session> = None;
    let mut next = id.map(str::to_owned);
    for _ in 0..6 {
        let Some(id) = next else {
            break;
        };
        let session = match engine.agent_session(&id).await.ok()? {
            Some((_, header)) => serde_json::from_value::<Session>(Value::Object(header)).ok(),
            None => engine
                .call::<Session>(lease.get().await.ok()?, Method::GET, &sessions::path(&id), None)
                .await
                .ok(),
        };
        let Some(session) = session else {
            break;
        };
        next = session.parent.clone().or_else(|| {
            session
                .notes()?
                .get("engineFor")?
                .as_str()
                .map(str::to_owned)
        });
        found = Some(session);
    }
    found
}

fn permissions(
    approval: Approval,
    settings: &Value,
    parent: Option<&Session>,
    project: Option<&Project>,
) -> Vec<Permission> {
    let local = Vec::<Permission>::deserialize(&settings["permissions"]).unwrap_or_default();
    let inherited = parent
        .and_then(|parent| parent.extra.get("permissions"))
        .and_then(|permissions| Vec::<Permission>::deserialize(permissions).ok())
        .unwrap_or_default();
    let mut rules = Vec::new();
    if approval <= Approval::AcceptEdits {
        let outside = local
            .iter()
            .rev()
            .find(|rule| rule.action == "external_directory" && rule.resource == "*")
            .is_some_and(|rule| rule.effect == "allow");
        for action in [
            "edit",
            "write",
            "patch",
            "shell",
            "webfetch",
            "external_directory",
            "kuvo-browser_browser_save_pdf",
            "kuvo-browser_browser_upload",
            "kuvo-browser_watch_for_change",
        ] {
            if action == "external_directory" && !outside {
                continue;
            }
            let effect = if approval == Approval::AcceptEdits
                && matches!(action, "edit" | "write" | "patch")
            {
                "allow"
            } else {
                "ask"
            };
            rules.push(Permission::new(action, "*", effect));
        }
        rules.extend(
            local
                .iter()
                .filter(|rule| {
                    !rule.denied() && rule.resource != "*" && rule.action != "external_directory"
                })
                .cloned(),
        );
    } else if approval == Approval::FullAccess {
        rules.extend([
            Permission::new("external_directory", "*", "allow"),
            Permission::new("doom_loop", "*", "allow"),
            Permission::new("read", "*.env", "allow"),
            Permission::new("read", "*.env.*", "allow"),
        ]);
        rules.extend(
            local
                .iter()
                .filter(|rule| rule.effect == "ask")
                .map(|rule| Permission::new(&rule.action, &rule.resource, "allow")),
        );
    }
    if let Some(project) = project {
        for folder in &project.folders {
            let resource = folder.join("*");
            rules.push(Permission::new(
                "external_directory",
                &resource.to_string_lossy(),
                "allow",
            ));
        }
    }
    rules.extend(
        local
            .into_iter()
            .chain(inherited)
            .filter(Permission::denied),
    );
    rules
}

fn project_name(project: &Project) -> String {
    project.name.clone().unwrap_or_else(|| {
        project
            .directory
            .file_name()
            .unwrap_or(project.directory.as_os_str())
            .to_string_lossy()
            .into_owned()
    })
}

async fn in_project(path: &Path, linked: Option<&Path>, project: &Project) -> bool {
    for folder in std::iter::once(&project.directory).chain(&project.folders) {
        if let (Ok(path), Ok(folder)) =
            (fs::canonicalize(path).await, fs::canonicalize(folder).await)
            && (path.starts_with(&folder) || linked.is_some_and(|path| path.starts_with(folder)))
        {
            return true;
        }
    }
    false
}

async fn touched_project<'a>(
    path: &Path,
    linked: Option<&Path>,
    projects: &'a [Project],
) -> Option<&'a Project> {
    let mut best: Option<&Project> = None;
    for project in projects {
        if best.is_none_or(|before| {
            before.directory.as_os_str().len() < project.directory.as_os_str().len()
        }) && in_project(path, linked, project).await
        {
            best = Some(project);
        }
    }
    best
}

pub(crate) async fn git(folder: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command
        .kill_on_drop(true)
        .current_dir(folder)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR");
    let output = time::timeout(Duration::from_secs(5), command.output())
        .await
        .ok()?
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

pub(crate) async fn linked_location(folder: &Path) -> Option<PathBuf> {
    let folder = fs::canonicalize(folder).await.ok()?;
    let top = fs::canonicalize(git(&folder, &["rev-parse", "--show-toplevel"]).await?)
        .await
        .ok()?;
    let common = fs::canonicalize(
        git(
            &folder,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .await?,
    )
    .await
    .ok()?;
    let private = fs::canonicalize(git(&folder, &["rev-parse", "--absolute-git-dir"]).await?)
        .await
        .ok()?;
    let main = common.parent()?;
    if main == top || common.file_name()? != ".git" || private.parent()? != common.join("worktrees")
    {
        return None;
    }
    let file = private.join("gitdir");
    let kind = fs::symlink_metadata(&file).await.ok()?;
    if !kind.is_file() || kind.len() > 4096 {
        return None;
    }
    let backlink = fs::read_to_string(file).await.ok()?;
    if fs::canonicalize(private.join(backlink.trim())).await.ok()? != top.join(".git") {
        return None;
    }
    let nested = folder.strip_prefix(top).ok()?;
    if nested.as_os_str().is_empty() {
        Some(main.to_owned())
    } else {
        Some(main.join(nested))
    }
}

struct Tree {
    project: PathBuf,
    project_id: String,
    root: PathBuf,
    folder: PathBuf,
    record: Value,
}

async fn make_tree(
    engine: &Engine,
    lease: &Lease,
    settings: &Value,
    project: &Path,
) -> Result<Tree, Refusal> {
    let failed = |why: String| {
        Refusal::new(
            StatusCode::BAD_GATEWAY,
            format!(
                "Couldn't make a worktree of {}: {why}",
                project
                    .file_name()
                    .unwrap_or(project.as_os_str())
                    .to_string_lossy()
            ),
        )
    };
    let prefix = git(project, &["rev-parse", "--show-prefix"])
        .await
        .ok_or_else(|| bad("That folder isn't in a git repository."))?;
    let base = git(project, &["symbolic-ref", "--quiet", "--short", "HEAD"]).await;
    let project_header = api::segment(&project.to_string_lossy());
    let send = |method, path| {
        engine
            .upstream
            .request(&lease.record, method, path)
            .header("x-opencode-directory", &project_header)
            .timeout(api::WAIT)
    };
    let location = send(Method::GET, "api/location")
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| failed(error.to_string()))?
        .json::<Value>()
        .await
        .map_err(|error| failed(error.to_string()))?;
    let project_id = location
        .pointer("/project/id")
        .or_else(|| location.pointer("/data/project/id"))
        .and_then(Value::as_str)
        .ok_or_else(|| failed("The engine didn't name the project.".into()))?
        .to_owned();
    let created = send(Method::POST, "api/worktree")
        .json(&json!({ "projectID": project_id }))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| failed(error.to_string()))?
        .json::<Value>()
        .await
        .map_err(|error| failed(error.to_string()))?;
    let root = created
        .get("directory")
        .or_else(|| created.pointer("/data/directory"))
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| failed("The engine didn't make it.".into()))?;
    let folder = root.join(prefix.trim_end_matches('/'));
    let mut tree = Tree {
        project: project.to_owned(),
        project_id,
        root,
        folder,
        record: Value::Null,
    };
    let prefix = settings["branch_prefix"]
        .as_str()
        .map_or_else(|| "kuvo/".to_owned(), clean_branch_prefix);
    let root_name = tree.root.file_name().unwrap_or_default().to_string_lossy();
    let mut branch = format!("{prefix}{root_name}");
    let mut suffix = 1;
    while git(
        &tree.root,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .await
    .is_some()
        && suffix <= 100
    {
        suffix += 1;
        branch = format!("{prefix}{root_name}-{suffix}");
    }
    if git(&tree.root, &["switch", "--quiet", "-c", &branch])
        .await
        .is_none()
    {
        undo_tree(engine, lease, &tree).await;
        return Err(failed("Its branch couldn't be made.".into()));
    }
    tree.record = json!({ "project": project, "root": tree.root, "folder": tree.folder,
        "branch": branch, "made": branch, "prefix": prefix });
    if let Some(base) = base {
        tree.record["base"] = json!(base);
    }
    if let Err(error) = store_tree(engine, &tree, true).await {
        undo_tree(engine, lease, &tree).await;
        return Err(failed(error.to_string()));
    }
    Ok(tree)
}

fn clean_branch_prefix(given: &str) -> String {
    let mut prefix = String::new();
    for ch in given.chars() {
        if ch.is_ascii_control()
            || ch.is_whitespace()
            || matches!(ch, '~' | '^' | ':' | '?' | '*' | '[' | '\\' | '\u{feff}')
        {
            continue;
        }
        if prefix.is_empty() && matches!(ch, '-' | '/' | '.') || ch == '.' && prefix.ends_with('.')
        {
            continue;
        }
        prefix.push(ch);
    }
    prefix
}

async fn store_tree(engine: &Engine, tree: &Tree, keep: bool) -> std::io::Result<()> {
    let _guard = engine.turn("worktree-records").await;
    let path = engine.home.join("box/worktrees.json");
    let mut record = disk::read::<Value>(&path)
        .await?
        .unwrap_or_else(|| json!({ "worktrees": [], "removed": {}, "restorable": [] }));
    if record
        .get("formatVersion")
        .is_some_and(|format| !matches!(format.as_u64(), Some(0 | 1)))
    {
        return Err(std::io::Error::other("Unsupported worktree record format."));
    }
    let roots = record["worktrees"]
        .as_array_mut()
        .ok_or_else(|| std::io::Error::other("The worktree records couldn't be read."))?;
    roots.retain(|row| {
        row["root"]
            .as_str()
            .is_none_or(|root| Path::new(root) != tree.root)
    });
    if keep {
        roots.push(tree.record.clone());
    }
    record["formatVersion"] = json!(1);
    disk::durable(path, record).await
}

async fn tree_unused(engine: &Engine, lease: &Lease, tree: &Tree) -> bool {
    api::Scan::Sessions
        .read(engine, lease, "api/session")
        .await
        .is_ok_and(|rows| {
            rows.into_iter().all(|row| {
                serde_json::from_value::<Session>(row).is_ok_and(|session| {
                    let directory = Path::new(session.directory());
                    !directory.starts_with(&tree.root)
                })
            })
        })
}

async fn undo_tree(engine: &Engine, lease: &Lease, tree: &Tree) {
    let removed = engine
        .upstream
        .request(&lease.record, Method::DELETE, "api/worktree")
        .header(
            "x-opencode-directory",
            api::segment(&tree.project.to_string_lossy()),
        )
        .timeout(api::WAIT)
        .json(&json!({ "projectID": tree.project_id, "directory": tree.root, "force": true }))
        .send()
        .await;
    if removed.is_ok_and(|response| response.status().is_success()) && !tree.record.is_null() {
        if let Some(branch) = tree.record["made"].as_str() {
            let _ = git(&tree.project, &["branch", "-D", "--", branch]).await;
        }
        let _ = store_tree(engine, tree, false).await;
    }
}
