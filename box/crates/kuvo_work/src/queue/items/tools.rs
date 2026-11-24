//! Queue tool calls from a verified running conversation.

mod definitions;
mod mutate;
mod read;
mod receipt;

pub(super) use mutate::{deleted, keep_reply};
pub(super) use read::item_name;
pub(super) use receipt::kept_answer;

use super::*;
use crate::queue::forward;
use kuvo_engine::ActiveTool;
use kuvo_protocol::routes::{
    get_kuvo_work_queue_tools as list, post_kuvo_work_queue_tools as route,
};

impl Writer {
    pub(super) fn register_tools(&self, registry: &mut Registry, engine: Engine) {
        registry
            .register::<list::Spec, _, _>(Access::Any, |_| async { typed(definitions::offered()) });
        let writer = self.clone();
        registry.register::<route::Spec, _, _>(Access::Any, move |call| {
            let engine = engine.clone();
            let writer = writer.clone();
            async move {
                let given: Value = serde_json::from_slice(&call.request.body).unwrap_or_default();
                let name = given["name"].as_str().unwrap_or_default();
                let session = given["session"].as_str().unwrap_or_default();
                let empty = json!({});
                let args = given.get("arguments").filter(|args| args.is_object()).unwrap_or(&empty);
                if !matches!(name, "queue_add" | "queue_update" | "queue_list" | "queue_show" | "queue_hand" | "queue_import" | "queue_remove" | "queue_labels") {
                    let name = given["name"].as_str().unwrap_or("undefined");
                    return typed(refused(format!("There's no tool {name}.")));
                }
                let reading = matches!(name, "queue_list" | "queue_show") ||
                    (name == "queue_labels" && !["add","rename","recolor","remove"].iter()
                    .any(|key| args[*key].as_str().is_some_and(|value| !value.trim().is_empty())));
                let operation = if reading { None } else { receipt::Operation::for_call(&call.request.headers, &given)? };
                let previous = if let Some(operation) = operation.as_ref() {
                    let store = writer.store.clone();
                    let operation = operation.clone();
                    tokio::task::spawn_blocking(move || operation.claim(&store)).await.map_err(internal)??
                } else {
                    None
                };
                if let Some(previous) = previous {
                    return typed(previous);
                }
                let answer = match engine.active_tool(session, name, args).await {
                    Some(caller) => match writer.run_tool(name, args.clone(), caller, operation.as_ref().map(|op| op.id.clone()), &engine).await {
                        Ok(answer) => answer,
                        Err(error) => {
                            let reply = error.reply();
                            let status = reply.status;
                            let value = refusal_value(reply).await;
                            if status == StatusCode::SERVICE_UNAVAILABLE && value["retryToolCall"] == true {
                                return Err(Refusal::json(status, value));
                            }
                            refused(value["message"].as_str().unwrap_or("The Work Queue couldn't do that."))
                        },
                    },
                    None => refused("Only a conversation's own tool call can use Work Queue tools, and this one isn't running in it."),
                };
                if let Some(operation) = operation {
                    let store = writer.store.clone();
                    return typed(tokio::task::spawn_blocking(move || operation.finish(&store, answer)).await.map_err(internal)??);
                }
                typed(answer)
            }
        });
    }

    async fn run_tool(
        &self,
        name: &str,
        args: Value,
        caller: ActiveTool,
        operation: Option<String>,
        engine: &Engine,
    ) -> Result<Value, Refusal> {
        if args.get("category").is_some() && name != "queue_import" {
            return Err(bad(format!(
                "Items have a status and labels now, not a category. Give status ({}) for where it stands, and labels for what kind of work it is (queue_labels lists them).",
                STATUSES
                    .iter()
                    .map(|status| status.name)
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        match name {
            "queue_add" if args.get("issue").is_some() => {
                self.add_issue(args, caller, operation).await
            }
            "queue_add" => self.add_ordinary(args, caller, operation).await,
            "queue_list" => {
                let owner = self.tool_owner(&caller, false)?;
                self.list_words(args, owner).await
            }
            "queue_show" => {
                self.show_words(&args["item"], self.tool_owner(&caller, false)?)
                    .await
            }
            "queue_update" => self.update_words(args, caller, operation).await,
            "queue_remove" => self.remove_words(args, caller, operation).await,
            "queue_hand" => {
                self.hand_words(args, caller, operation, engine, false)
                    .await
            }
            "queue_import" => {
                let owner = self.tool_owner(&caller, true)?;
                let result = self
                    .agent_import(owner, args["items"].clone(), actor(&caller))
                    .await?;
                Ok(said(result["said"].as_str().unwrap_or_default()))
            }
            "queue_labels" => {
                self.tool_owner(&caller, false)?;
                labels::tool(&self.store, &self.setup, args, actor(&caller)).await
            }
            _ => Ok(refused(format!("There's no tool {name}."))),
        }
    }

    pub(super) async fn peer_tool(&self, mut act: Act) -> Value {
        use forward::records::{Method, Tool};
        let agent = act.agent.take().expect("parsed forwarded agent");
        let mut args = act
            .body
            .take()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        args["item"] = act.item.as_str().into();
        args["undo"] = Value::Bool(
            matches!(act.method, Method::Post) && act.action.as_deref() == Some("restore"),
        );
        let operation = act.id.starts_with("wqa_tool_").then_some(act.id);
        let caller = ActiveTool {
            root: agent.session,
            name: Some(agent.name),
            seat: agent.owner.starts_with("grp_"),
            group: agent.owner.starts_with("grp_").then_some(agent.owner),
            directory: None,
        };
        let id = act.item;
        let reply = match agent.tool {
            Tool::Update => {
                self.update_local(id.clone(), args, caller, operation, true)
                    .await
            }
            Tool::Remove => {
                let removal = if args["undo"] == true {
                    Removal::Restore
                } else {
                    Removal::Delete
                };
                self.change_as(id.clone(), removal, actor(&caller), operation)
                    .await
            }
            Tool::Hand => {
                self.hand_words(args, caller, operation, &self.engine, true)
                    .await
            }
        };
        match reply {
            Ok(mut answer) => {
                let here = self.clone();
                let item = blocking(move || {
                    let seen = super::super::github::Seen::read(&here.store)?;
                    let sessions =
                        super::super::sessions::Sessions::read(here.sessions.latest_record());
                    Ok(here
                        .store
                        .items()?
                        .into_iter()
                        .find(|item| item.id == id)
                        .map(|item| {
                            view::summary(
                                &item,
                                jiff::Zoned::now().date(),
                                &here.refs,
                                &seen,
                                &sessions,
                                &Assignees::read(&here.store),
                            )
                        }))
                })
                .await;
                if let Ok(Some(item)) = item {
                    answer["item"] = item;
                }
                answer
            }
            Err(error) => tool_refusal(error).await,
        }
    }

    async fn change_there(
        &self,
        target: &Value,
        tool: forward::records::Tool,
        mut args: Value,
        caller: ActiveTool,
        operation: Option<String>,
    ) -> Result<Value, Refusal> {
        let comp = &target["computer"];
        let id = target["id"]
            .as_str()
            .expect("remote Work Queue item id")
            .to_owned();
        let key = comp["key"]
            .as_str()
            .and_then(kuvo_wire::keys::decode_exact)
            .ok_or_else(|| bad("That computer is not in this setup."))?;
        if !self.peers.view().advertises(&key, forward::AGENT_FEATURE) {
            let name = comp["name"].as_str().unwrap_or_default();
            return Err(bad(format!(
                "{} is on {name}. Update Kuvo there so it can record which agent made the change.",
                read::item_name(target)
            )));
        }
        let body = args.as_object_mut().expect("tool arguments");
        body.remove("item");
        if let Some(given) = body
            .remove("files")
            .filter(|_| matches!(tool, forward::records::Tool::Update))
        {
            let writer = self.clone();
            let directory = caller.directory.clone();
            let name = comp["name"].as_str().unwrap_or_default().to_owned();
            let files = tokio::task::spawn_blocking(move || {
                writer
                    .files
                    .stage_agent(&writer.store, Some(&given), directory.as_deref())?
                    .for_computer(&name)
            })
            .await
            .map_err(internal)??;
            body.insert("files".into(), files);
        }
        let agent = forward::records::Agent {
            tool,
            owner: self.tool_owner(&caller, false)?,
            session: caller.root,
            name: caller.name.unwrap_or_else(|| "An agent".into()),
        };
        let name = comp["name"].as_str().unwrap_or_default().to_owned();
        let undo = args["undo"] == true;
        let action = forward::CallAgent {
            agent,
            undo,
            body: args,
            operation: operation.clone(),
        };
        let changed = match self
            .forward
            .agent_change(key, comp.clone(), id.clone(), action)
            .await
        {
            Ok(reply) => reply,
            Err(error) if operation.is_some() => {
                let reply = error.reply();
                if reply.status == StatusCode::SERVICE_UNAVAILABLE {
                    return Err(Refusal::json(
                        StatusCode::SERVICE_UNAVAILABLE,
                        json!({"retryToolCall":true,"message":format!("Waiting for {name} to confirm the Work Queue change.")}),
                    ));
                }
                return Ok(refused(refusal_message(reply).await));
            }
            Err(error) => return Ok(tool_refusal(error).await),
        };
        if changed["isError"] == false && changed["item"]["id"] == id {
            let store = self.store.clone();
            let item = changed["item"].clone();
            let deleted = matches!(tool, forward::records::Tool::Remove) && !undo;
            let previous = target.clone();
            let _ = tokio::task::spawn_blocking(move || {
                told::apply_reply(&store, &key, &item, deleted, &previous)
            })
            .await;
        }
        if let (Some(text), Some(error)) = (changed["text"].as_str(), changed["isError"].as_bool())
        {
            Ok(json!({"text":text,"isError":error}))
        } else {
            Ok(refused(changed["message"].as_str().map_or_else(
                || format!("{name} didn't confirm the change."),
                str::to_owned,
            )))
        }
    }

    fn tool_item_reference(&self, value: &Value) -> Result<String, Refusal> {
        value.as_str().map(str::to_owned)
            .or_else(|| value.as_u64().map(|number| number.to_string()))
            .filter(|given| self.refs.parse(given).is_some())
            .ok_or_else(|| bad("Say which item (item), by its number, like #243 (or the id an item without one was given), or the GitHub issue or pull request it is, like gh:owner/repo#12."))
    }

    fn computer_words(&self) -> (&'static str, &'static str) {
        let home = self
            .store
            .root
            .parent()
            .and_then(Path::parent)
            .expect("Work Queue under home");
        let is_mac = std::env::var_os("KUVO_BOX").is_some_and(|kind| kind == "mac")
            || fs::read_to_string(home.join("box/kind")).is_ok_and(|kind| kind.trim() == "mac");
        if is_mac {
            ("this Mac", "This Mac")
        } else {
            ("this machine", "This machine")
        }
    }

    fn tool_owner(&self, caller: &ActiveTool, needs_own: bool) -> Result<String, Refusal> {
        if !caller.seat {
            return Ok(caller.root.clone());
        }
        let group = caller.group.as_deref().ok_or_else(|| bad(
            "A group chat's seat keeps the group's queue, and this one's group isn't known here.",
        ))?;
        let group = format!("grp_{}", group.strip_prefix("grp_").unwrap_or(group));
        if !needs_own {
            return Ok(group);
        }
        match self
            .peers
            .view()
            .owner_of(RecordScope::Group, &group, self.peers.now())
        {
            Some(found) if found.local => Ok(group),
            Some(_) => Err(bad(
                "This group chat lives on another of the user's computers, and its Work Queue is kept there: a seat on this one can't change it yet. Say what to add in the chat, and the seat there adds it.",
            )),
            None => Err(bad(format!(
                "{} can't tell where this group chat lives right now. Try again in a moment.",
                self.computer_words().1
            ))),
        }
    }

    async fn add_issue(
        &self,
        args: Value,
        caller: ActiveTool,
        operation: Option<String>,
    ) -> Result<Value, Refusal> {
        let owner = self.tool_owner(&caller, true)?;
        self.admit(&owner)?;
        let issue = self.issue_for(&args["issue"], &owner, None).await?;
        let writer = self.clone();
        let folder = caller.directory.clone();
        let files = args.get("files").cloned();
        let staged = tokio::task::spawn_blocking(move || {
            writer
                .files
                .stage_agent(&writer.store, files.as_ref(), folder.as_deref())
        })
        .await
        .map_err(internal)??;
        let writer = self.clone();
        let guard = self.lock().await?;
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            let now = Timestamp::now().as_millisecond();
            let (all, compaction) = writer.changes(now)?;
            if let Some(answer) = operation.as_deref().and_then(|operation| receipt::kept_answer(&all, operation)) {
                compaction.finish(&writer, now)?;
                return Ok(answer);
            }
            writer.admit(&owner)?;
            writer.refuse_issue(&all, &issue.source_key, issue.source["id"].as_str(), &owner, None)?;
            let reference = issue.display();
            if all.iter().filter(|item| item.owner == owner && item.removed.is_none() && !status::closed(&item.status)).count() >= 2000 {
                return Err(bad("Your queue has 2000 open items already: mark some done or remove them first."));
            }
            let working = args["working"] == true && !caller.seat;
            let status = args.get("status").map(status_of).transpose()?.unwrap_or(if working { "in-progress" } else { "todo" });
            if status::closed(status) {
                return Err(bad(format!("{reference} is Done or Canceled once it's closed on GitHub (or, a pull request, merged): close it there, or let the pull request that fixes it close it.")));
            }
            let mut random = [0; 15];
            getrandom::fill(&mut random).map_err(internal)?;
            let id = format!("wqi_{}", kuvo_wire::keys::encode(&random));
            let who = actor(&caller);
            let stamp = json!({"at":now,"by":who});
            let title = &issue.title;
            let source = &issue.source;
            let labels = labels::shown(&writer.store, writer.setup.queue_setup().as_ref()).map_err(storage_refusal)?;
            let labels = args.get("labels").map(|given| labels_of(given, &labels)).transpose()?.unwrap_or_default();
            let order = place(&all, None, status, args["top"] == true);
            let mut row = json!({"id":id,"owner":owner,"source":source,
                "title":title,"body":limit(&issue.body,100_000),"status":status,"order":order,
                "labels":labels,"notes":[],"files":[],"links":[],"sessions":[],"created":stamp,"updated":stamp,
            });
            if let Some(priority) = args.get("priority").map(priority_of).transpose()?.flatten() {
                row["priority"] = priority.into();
            }
            if let Some(due) = args.get("due").map(due_of).transpose()?.flatten() {
                row["due"] = due.into();
            }
            if let Some(assignee) = assignee(&args, &Assignees::read(&writer.store))? {
                row["assignee"] = assignee;
            }
            let mut docs = std::collections::BTreeMap::new();
            if working {
                writer.link_session(&all, &mut docs, &mut row, &caller.root, &who, now)?;
                append_event(&mut row, json!({"at":now,"by":who,"kind":"started"}));
            }
            let seen_words = seen_words(&issue.state);
            mutate::fields(&mut row, &args, now, &who)?;
            let published = writer.files.publish(&writer.store, &mut row, &staged, false, now)?;
            let where_ = status_name(status);
            let working_words = if working { ", linked to you" } else { "" };
            let merged = if issue.source["kind"] == "github-pr" { " or merged" } else { "" };
            let ignored = match (args.get("title").is_some(), args.get("body").is_some()) {
                (true, true) => " (The title and body you gave aren't used.)",
                (true, false) => " (The title you gave isn't used.)",
                (false, true) => " (The body you gave isn't used.)",
                _ => "",
            };
            let answer = json!({"text":format!("Added {reference} \"{title}\" ({seen_words} on GitHub) as {where_}{working_words}. Refer to it as {reference}. Its title and description are GitHub's, and it's Done once it's closed there{merged}.{ignored}"),"isError":false});
            if let Some(operation) = operation {
                row["toolReplies"] = json!({operation:answer});
            }
            let item = Item::read(&owner, &row).expect("new GitHub queue item");
            row["mentionIndex"] = json!(Index::rebuild(&writer.refs.mentions, &item, &all, None));
            writer.document(&mut docs, &owner, now)?.items.push(row);
            docs.remove(&owner).expect("new item's queue").save(&writer.store).map_err(storage_refusal)?;
            published.commit();
            for document in docs.into_values() {
                document.save(&writer.store).map_err(storage_refusal)?;
            }
            compaction.finish(&writer, now)?;
            Ok(answer)
        }).await.map_err(internal)?
    }

    fn refuse_issue(
        &self,
        all: &[Item],
        lookup: &str,
        id: Option<&str>,
        owner: &str,
        except: Option<&str>,
    ) -> Result<(), Refusal> {
        let matches = |source: &Value| {
            id.is_some_and(|id| source["id"] == id)
                || source["ref"]
                    .as_str()
                    .and_then(|reference| self.refs.issue(reference))
                    .is_some_and(|issue| issue.key() == lookup)
        };
        let named = |reference: &str, title: &str| format!("{reference} \"{title}\"");
        let existing = all
            .iter()
            .filter(|item| Some(item.id.as_str()) != except && item.source().is_some_and(matches))
            .collect::<Vec<_>>();
        if let Some(item) = existing.iter().find(|item| item.removed.is_none()) {
            let reference = item
                .source()
                .and_then(|source| source["ref"].as_str())
                .unwrap_or_default();
            let name = named(reference, &item.title);
            if item.owner == owner {
                return Err(bad(format!(
                    "{name} is in your Work Queue already: refer to it as {reference}."
                )));
            }
            return Err(bad(format!(
                "{name} is in the Work Queue already, in the queue of {}: one item for each issue. Ask that conversation, or the user.",
                item.owner
            )));
        }
        let others = told::items_elsewhere(
            &self.store,
            all,
            &self.peers.view(),
            "all",
            &super::super::sessions::Sessions::default(),
            &[],
            self.peers.now(),
        )
        .map_err(storage_refusal)?;
        if let Some(item) = others
            .items
            .iter()
            .find(|item| item["id"].as_str() != except && matches(&item["source"]))
        {
            let name = named(
                item["source"]["ref"].as_str().unwrap_or_default(),
                item["title"].as_str().unwrap_or_default(),
            );
            let computer = item["computer"]["name"].as_str().unwrap_or_default();
            return Err(bad(format!(
                "{name} is in the Work Queue already, on {computer}: one item for each issue."
            )));
        }
        if let Some(item) = existing.first() {
            let reference = item
                .source()
                .and_then(|source| source["ref"].as_str())
                .unwrap_or_default();
            let name = named(reference, &item.title);
            let removed = item.removed.as_ref().expect("removed");
            return Err(bad(if removed.by["user"] == true {
                format!(
                    "The user deleted {name}: only they bring it back. Ask them if it's still needed."
                )
            } else {
                format!(
                    "{name} was deleted by {}: queue_remove with undo brings it back.",
                    removed.by["name"].as_str().unwrap_or("an agent")
                )
            }));
        }
        Ok(())
    }
}

struct LinkedIssue {
    source: Value,
    title: String,
    body: String,
    state: String,
    source_key: String,
}

impl LinkedIssue {
    fn display(&self) -> &str {
        self.source["ref"]
            .as_str()
            .expect("canonical GitHub reference")
    }
}

impl Writer {
    async fn issue_for(
        &self,
        given: &Value,
        owner: &str,
        except: Option<&str>,
    ) -> Result<LinkedIssue, Refusal> {
        let issue = given.as_str().and_then(|reference| self.refs.issue(reference))
            .ok_or_else(|| bad("Give the issue or pull request (issue) as gh:owner/repo#12, or its address on github.com."))?;
        let preflight = self.clone();
        let original = issue.key();
        let queue = owner.to_owned();
        let except_id = except.map(str::to_owned);
        blocking(move || {
            let all = preflight.store.items()?;
            Ok(preflight.refuse_issue(&all, &original, None, &queue, except_id.as_deref()))
        })
        .await??;
        let (seen, body) = self
            .reads
            .github
            .read_issue(&issue, self.computer_words().0)
            .await
            .map_err(bad)?;
        let canonical = seen["url"]
            .as_str()
            .and_then(|url| self.refs.issue(url))
            .unwrap_or(issue);
        let source_key = canonical.key();
        let reference = format!(
            "gh:{}/{}#{}",
            canonical.owner, canonical.repo, canonical.number
        );
        let title = seen["title"]
            .as_str()
            .expect("GitHub issue title")
            .to_owned();
        let state = seen["state"].as_str().unwrap_or_default().to_owned();
        let source = json!({"kind":seen["kind"],"ref":reference,"url":seen["url"],"host":"github.com","id":seen["id"]});
        Ok(LinkedIssue {
            source,
            title,
            body,
            state,
            source_key,
        })
    }

    fn attach_issue(
        &self,
        item: &Item,
        issue: &LinkedIssue,
        who: &Value,
        now: i64,
    ) -> Result<Value, Refusal> {
        let mut row = Value::Object(item.saved.clone());
        let display = read::item_name(&row);
        if let Some(source) = item.source() {
            return Err(bad(format!(
                "{display} is {} already: one item is one issue or pull request.",
                source["ref"].as_str().unwrap_or_default()
            )));
        }
        if status::closed(&item.status) {
            return Err(bad(format!(
                "{display} is {}: give it an open status first (status), or add the issue as an item of its own (queue_add with issue).",
                status_name(&item.status)
            )));
        }
        let before = item.body.trim();
        row["source"] = issue.source.clone();
        let title = limit(&plain_name(&issue.title), 200);
        if !title.is_empty() {
            row["title"] = title.into();
        }
        if !issue.body.trim().is_empty() {
            row["body"] = limit(&issue.body, 100_000).into();
            if !before.is_empty() {
                let text = limit(
                    &format!(
                        "What it said before it was {}:\n\n{before}",
                        issue.display()
                    ),
                    20_000,
                );
                let mut notes = item.notes.clone();
                notes.push(json!({"at":now,"by":who,"text":text}));
                notes.drain(..notes.len().saturating_sub(200));
                row["notes"] = notes.into();
            }
        }
        Ok(row)
    }
}

fn actor(caller: &ActiveTool) -> Value {
    json!({"session":caller.root,"name":caller.name.as_deref().unwrap_or("An agent")})
}

fn status_name(status: &str) -> &str {
    STATUSES
        .iter()
        .find(|known| known.id == status)
        .map_or(status, |known| known.name)
}

fn seen_words(state: &str) -> &str {
    match state {
        "draft" => "a draft",
        "completed" => "closed",
        "not_planned" => "closed as not planned",
        "closed" => "closed without merging",
        other => other,
    }
}

async fn tool_refusal(error: Refusal) -> Value {
    refused(refusal_message(error.reply()).await)
}

async fn refusal_message(reply: kuvo_server::Reply) -> String {
    refusal_value(reply).await["message"]
        .as_str()
        .unwrap_or("The Work Queue couldn't do that.")
        .to_owned()
}

async fn refusal_value(reply: kuvo_server::Reply) -> Value {
    use http_body_util::BodyExt;
    reply
        .body
        .collect()
        .await
        .ok()
        .and_then(|body| serde_json::from_slice::<Value>(&body.to_bytes()).ok())
        .unwrap_or_else(|| json!({"message":"The Work Queue couldn't do that."}))
}

fn refused(text: impl Into<String>) -> Value {
    json!({"text":text.into(),"isError":true})
}

pub(super) fn said(text: impl Into<String>) -> Value {
    json!({"text":text.into(),"isError":false})
}
