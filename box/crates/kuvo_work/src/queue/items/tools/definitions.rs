//! The queue tool descriptions and input schemas published to agent clients.

use serde_json::{Value, json};

const INSTRUCTIONS: &str = " Keep work items in your Work Queue (queue_*), not files. Add GitHub issues and PRs with queue_add issue:, Done when closed. Read or edit any item on the user's computers. Edits record your name and session. Use #243 in chat and gh:owner/repo#12 for GitHub. Omit local IDs in commits, branches and PRs. Set working: true, leave a progress_note, then set Done, In review, or Needs you if only the user can proceed.";

pub(super) fn offered() -> Value {
    let item = field(
        json!(["string", "integer"]),
        "Like #243 or gh:owner/repo#12.",
    );
    let status = field(
        json!("string"),
        "Needs you (only the user can move it on), In review, In progress, Waiting, Todo, Backlog, Done or Canceled.",
    );
    let labels = array("All its labels by name, replacing those it had ([]: none).");
    let priority = field(json!("string"), "Urgent, High, Normal or Low.");
    let assignee = field(
        json!(["string", "null"]),
        "Who it's for: a bot's or coding agent's name (null: no one). Doesn't hand it on.",
    );
    let due = field(json!(["string", "null"]), "YYYY-MM-DD, or null.");
    let top = field(json!("boolean"), "At the top of its status.");
    let progress = field(
        json!(["number", "null"]),
        "0 to 1, where it can be counted.",
    );
    let progress_note = field(json!(["string", "null"]), "What's being done now, briefly.");
    let working = field(
        json!("boolean"),
        "true: you're on it now (Todo, Backlog or Waiting becomes In progress). false: you stopped (its status stays as it is).",
    );
    let links = array("Web links (a PR, a doc).");
    let files = array("Full paths of files to keep with it (in the body: ![](name)).");
    let string = json!({"type":"string"});
    let boolean = json!({"type":"boolean"});

    let list = tool(
        "queue_list",
        "List the Work Queue",
        "List open items across all Work Queues unless done.",
        json!({
            "owner":field(json!("string"),"Filter by session address, group ID, or usr_you."),
            "status":field(json!("string"),"Only this one."),
            "label":field(json!("string"),"Only with this label."),
            "query":field(json!("string"),"Words in it or its notes."),
            "done":field(json!("boolean"),"Done and Canceled too."),
        }),
        &[],
    );
    let show = tool(
        "queue_show",
        "Read a Work Queue item",
        "Read an item and its history, or its last shared summary if offline.",
        json!({"item":item}),
        &["item"],
    );
    let add = tool(
        "queue_add",
        "Add to the Work Queue",
        "Add an item to your Work Queue (Todo and Normal unless you say); you get its number. With issue, it's that GitHub issue or PR (no title).",
        json!({
            "title":field(json!("string"),"One short line."),
            "issue":field(json!("string"),"gh:owner/repo#12 or its URL."),
            "body":field(json!("string"),"Markdown: what, why, what's decided."),
            "status":status,"priority":priority,"labels":labels,"assignee":assignee,"due":due,
            "top":top,"progress":progress,"progress_note":progress_note,"links":links,"files":files,"working":working,
        }),
        &[],
    );
    let update = tool(
        "queue_update",
        "Update a Work Queue item",
        "Change supplied fields. note appends a note; issue sets the GitHub issue or PR.",
        json!({
            "item":item,"issue":string,"title":string,"body":field(json!("string"),"Replaces the description."),
            "status":status,"priority":priority,"labels":labels,"assignee":assignee,"due":due,"top":top,
            "before":field(json!(["string","integer"]),"Just before this item."),
            "after":field(json!(["string","integer"]),"Just after this item."),
            "progress":progress,"progress_note":progress_note,"note":string,"links":links,"files":files,
            "remove":array("Links or files to take away."),"working":working,"sessions":array("Sessions to link, by address."),
            "unlink":json!({"type":"array","items":{"type":"string"}}),
        }),
        &["item"],
    );
    let hand = tool(
        "queue_hand",
        "Hand off a Work Queue item",
        "Move an item to a session or bot's queue and link it to them.",
        json!({"item":item,"to":field(json!("string"),"Its address, or a bot's name.")}),
        &["item", "to"],
    );
    let remove = tool(
        "queue_remove",
        "Remove a Work Queue item",
        "Soft-delete an item. undo restores agent deletions for 30 days. The user can also restore their deletions. Use Canceled for abandoned work.",
        json!({"item":item,"undo":boolean}),
        &["item"],
    );
    let labeling = tool(
        "queue_labels",
        "Manage labels",
        "List shared labels. On request add, rename, recolor, or remove from all items.",
        json!({"add":string,"rename":string,"to":string,"recolor":string,"remove":string,"color":field(json!("string"),"coral, orange, amber, lime, mint, sky, indigo, violet, pink or graphite.")}),
        &[],
    );
    let import = tool(
        "queue_import",
        "Import items",
        "Import an old backlog's items with their numbers (read the files yourself); up to 200 a call.",
        json!({"items":{"type":"array","items":{"type":"object","properties":{
            "number":{"type":"integer"},"title":string,"body":string,
            "status":field(json!("string"),"Todo if none."),
            "label":field(json!("string"),"A label's name."),
            "done":field(json!("boolean"),"Done, whatever status says."),
        },"required":["number","title"]}}}),
        &["items"],
    );
    json!({"tools":[list,show,add,update,hand,remove,labeling,import],"instructions":INSTRUCTIONS})
}

fn tool(name: &str, title: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    let mut schema = json!({"type":"object","properties":properties,"additionalProperties":false});
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    json!({"name":name,"title":title,"description":description,"inputSchema":schema})
}

fn field(kind: Value, description: &str) -> Value {
    json!({"type":kind,"description":description})
}

fn array(description: &str) -> Value {
    json!({"type":"array","items":{"type":"string"},"description":description})
}
