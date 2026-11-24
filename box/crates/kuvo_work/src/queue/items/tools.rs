//! Queue tool calls from a verified running conversation.

mod definitions;
mod mutate;
mod read;
mod receipt;

pub(super) use mutate::{deleted, keep_reply};
pub(super) use read::item_name;
pub(super) use receipt::kept_answer;

use super::*;
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
                let operation = receipt::Operation::for_call(&call.request.headers, &given)?;
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
                    Some(caller) => match writer.run_tool(name, args.clone(), caller, operation.as_ref().map(|op| op.id.clone())).await {
                        Ok(answer) => answer,
                        Err(error) => tool_refusal(error).await,
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
            "queue_import" => {
                let owner = self.tool_owner(&caller, true)?;
                let result = self
                    .agent_import(owner, args["items"].clone(), actor(&caller))
                    .await?;
                Ok(said(result["said"].as_str().unwrap_or_default()))
            }
            _ => Ok(refused("This Work Queue tool isn’t available yet.")),
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
        let (computer, _) = self.computer_words();
        let owner = self.tool_owner(&caller, true)?;
        self.admit(&owner)?;
        let issue = args["issue"].as_str().and_then(|ref_item| self.refs.issue(ref_item))
            .ok_or_else(|| bad("Give the issue or pull request (issue) as gh:owner/repo#12, or its address on github.com."))?;
        let preflight = self.clone();
        let original = issue.key();
        let queue = owner.clone();
        blocking(move || {
            let all = preflight.store.items()?;
            Ok(preflight.refuse_issue(&all, &original, None, &queue))
        })
        .await??;
        let (seen, body) = self
            .reads
            .github
            .read_issue(&issue, computer)
            .await
            .map_err(bad)?;
        let canonical = seen["url"]
            .as_str()
            .and_then(|url| self.refs.issue(url))
            .unwrap_or(issue);
        let canonical_key = canonical.key();
        let reference = format!(
            "gh:{}/{}#{}",
            canonical.owner, canonical.repo, canonical.number
        );
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
            let id = seen["id"].as_str();
            writer.refuse_issue(&all, &canonical_key, id, &owner)?;
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
            let title = seen["title"].as_str().expect("GitHub issue title");
            let source = json!({"kind":seen["kind"],"ref":reference,"url":seen["url"],"host":"github.com","id":seen["id"]});
            let labels = labels::shown(&writer.store, writer.setup.queue_setup().as_ref()).map_err(storage_refusal)?;
            let labels = args.get("labels").map(|given| labels_of(given, &labels)).transpose()?.unwrap_or_default();
            let order = place(&all, None, status, args["top"] == true);
            let mut row = json!({"id":id,"owner":owner,"source":source,
                "title":title,"body":limit(&body,100_000),"status":status,"order":order,
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
            if working {
                row["sessions"] = json!([{"session":caller.root, "since":now}]);
                append_event(&mut row, json!({"at":now,"by":who,"kind":"started"}));
            }
            let seen_words = seen_words(seen["state"].as_str().unwrap_or_default());
            mutate::fields(&mut row, &args, now, &who)?;
            let where_ = status_name(status);
            let working_words = if working { ", linked to you" } else { "" };
            let merged = if seen["kind"] == "github-pr" { " or merged" } else { "" };
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
            let mut document = writer.read_owner(&owner, now)?;
            document.items.push(row);
            document.save(&writer.store).map_err(storage_refusal)?;
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
            .filter(|item| item.source().is_some_and(matches))
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
        if let Some(item) = others.items.iter().find(|item| matches(&item["source"])) {
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
    use http_body_util::BodyExt;
    reply
        .body
        .collect()
        .await
        .ok()
        .and_then(|body| serde_json::from_slice::<Value>(&body.to_bytes()).ok())
        .and_then(|body| body["message"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "The Work Queue couldn't do that.".into())
}

fn refused(text: impl Into<String>) -> Value {
    json!({"text":text.into(),"isError":true})
}

pub(super) fn said(text: impl Into<String>) -> Value {
    json!({"text":text.into(),"isError":false})
}
