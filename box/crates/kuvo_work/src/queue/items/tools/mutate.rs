//! Create ordinary queue items and apply the fields that agents can submit.
use super::*;

impl Writer {
    pub(super) async fn add_ordinary(
        &self,
        mut args: Value,
        caller: ActiveTool,
        operation: Option<String>,
    ) -> Result<Value, Refusal> {
        let owner = self.tool_owner(&caller, true)?;
        self.admit(&owner)?;
        let now = Timestamp::now().as_millisecond();
        let who = actor(&caller);
        let working = args["working"] == true && !caller.seat;
        if working && args.get("status").is_none() {
            args["status"] = "in-progress".into();
        }
        let writer = self.clone();
        let folder = caller.directory.clone();
        let file_args = args.get("files").cloned();
        let staged = tokio::task::spawn_blocking(move || {
            writer
                .files
                .stage_agent(&writer.store, file_args.as_ref(), folder.as_deref())
        })
        .await
        .map_err(internal)??;
        let mut bytes = [0; 15];
        getrandom::fill(&mut bytes).map_err(internal)?;
        let id = format!("wqi_{}", kuvo_wire::keys::encode(&bytes));
        let guard = self.lock().await?;
        let writer = self.clone();
        let prepared = tokio::task::spawn_blocking(move || {
            let (all, compaction) = writer.changes(now)?;
            if let Some(answer) = operation
                .as_deref()
                .and_then(|operation| receipt::kept_answer(&all, operation))
            {
                compaction.finish(&writer, now)?;
                return Ok(AgentAdd::Replay(answer));
            }
            writer.admit(&owner)?;
            let people = Assignees::read(&writer.store);
            let mut row = writer.new_row(&id, &owner, &args, &all, &people, now, &who)?;
            if working && !status::closed(row["status"].as_str().expect("item status")) {
                row["sessions"] = json!([{"session":caller.root,"at":now,"by":who}]);
                append_event(&mut row, json!({"at":now,"by":who,"kind":"started"}));
            }
            fields(&mut row, &args, now, &who)?;
            let document = writer.read_owner(&owner, now)?;
            Ok::<_, Refusal>(AgentAdd::New {
                all,
                document,
                row,
                staged,
                compaction,
                operation,
            })
        })
        .await
        .map_err(internal)??;
        let AgentAdd::New {
            all,
            mut document,
            mut row,
            staged,
            compaction,
            operation,
        } = prepared
        else {
            let AgentAdd::Replay(answer) = prepared else {
                unreachable!()
            };
            return Ok(answer);
        };
        let allocated = self
            .numbers
            .allocate(1, highest(&all), now)
            .await
            .map_err(storage_refusal)?;
        if let Some(number) = allocated.first() {
            row["number"] = (*number).into();
        }
        let writer = self.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            let owner = &document.owner;
            let published = writer.files.publish(&writer.store, &mut row, &staged, true, now)?;
            let item = Item::read(owner, &row).expect("new Work Queue item");
            row["mentionIndex"] = json!(Index::rebuild(&writer.refs.mentions, &item, &all, None));
            let where_ = status_name(&item.status);
            let answer = said(if let Some(number) = item.number {
                let linked = if working { ", linked to you" } else { "" };
                format!("Added #{number} \"{}\" as {where_}{linked}. Refer to it as #{number}.", item.title)
            } else {
                let (computer, _) = writer.computer_words();
                format!("Added \"{}\" as {where_}. It has no number yet: {computer} gets one from the user's main computer, which isn't answering now. Name it as {} meanwhile.", item.title, item.id)
            });
            if let Some(operation) = operation {
                row["toolReplies"] = json!({operation:answer});
            }
            document.items.push(row);
            document.save(&writer.store).map_err(storage_refusal)?;
            published.commit();
            compaction.finish(&writer, now)?;
            Ok(answer)
        }).await.map_err(internal)?
    }
}

enum AgentAdd {
    Replay(Value),
    New {
        all: Vec<Item>,
        document: Document,
        row: Value,
        staged: Staged,
        compaction: Compaction,
        operation: Option<String>,
    },
}

pub(super) fn fields(
    row: &mut Value,
    args: &Value,
    now: i64,
    actor: &Value,
) -> Result<(), Refusal> {
    let mut progress = row["progress"].as_object().cloned().unwrap_or_default();
    if let Some(value) = args.get("progress") {
        if value.is_null() {
            progress.remove("value");
        } else if value
            .as_f64()
            .is_some_and(|value| (0.0..=1.0).contains(&value))
        {
            progress.insert("value".into(), value.clone());
        } else {
            return Err(bad(
                "progress is a number from 0 to 1 (3 of 8 is 0.375), or null to take it away.",
            ));
        }
    }
    if let Some(note) = args.get("progress_note") {
        let note = if note.is_null() {
            String::new()
        } else {
            limit(&plain_name(&text(note, 600, "progress_note")?), 300)
        };
        if note.is_empty() {
            progress.remove("note");
        } else {
            progress.insert("note".into(), note.into());
        }
    }
    if progress.is_empty() {
        row.as_object_mut().expect("queue item").remove("progress");
    } else {
        row["progress"] = Value::Object(progress);
    }
    if let Some(note) = args.get("note") {
        let note = text(note, 20_000, "The note")?;
        if !note.trim().is_empty() {
            let mut notes = store::array(row.get("notes")).to_vec();
            notes.push(json!({"at":now,"by":actor,"text":note.trim()}));
            notes.drain(..notes.len().saturating_sub(200));
            row["notes"] = notes.into();
        }
    }
    let mut links = store::array(row.get("links")).to_vec();
    if let Some(removing) = args.get("remove") {
        let removed = removing
            .as_array()
            .map_or(std::slice::from_ref(removing), Vec::as_slice);
        links.retain(|link| !removed.iter().any(|name| name == &link["url"]));
    }
    if let Some(given) = args.get("links") {
        for value in given
            .as_array()
            .map_or(std::slice::from_ref(given), Vec::as_slice)
        {
            let address = value
                .as_str()
                .or_else(|| value["url"].as_str())
                .unwrap_or_default();
            let url = address.trim();
            if !url.split_once("://").is_some_and(|(scheme, rest)| {
                ["http", "https"]
                    .iter()
                    .any(|prefix| scheme.eq_ignore_ascii_case(prefix))
                    && !rest.is_empty()
                    && !rest.chars().any(char::is_whitespace)
            }) {
                let shown = if address.is_empty() {
                    "A link"
                } else {
                    address
                };
                return Err(bad(format!("{shown} isn't a web address (https://…).")));
            }
            let url = limit(url, 2_000);
            if !links.iter().any(|link| link["url"] == url) {
                let mut link = json!({"url":url});
                if let Some(title) = value["title"].as_str() {
                    let title = limit(&plain_name(title), 200);
                    if !title.is_empty() {
                        link["title"] = title.into();
                    }
                }
                links.push(link);
            }
        }
        if links.len() > 50 {
            return Err(bad("An item keeps 50 links at most."));
        }
    }
    row["links"] = links.into();
    Ok(())
}
