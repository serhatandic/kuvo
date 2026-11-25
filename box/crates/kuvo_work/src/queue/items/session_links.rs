//! Link work without keeping a conversation attached to more than three items.
use super::*;
use std::collections::BTreeMap;

const MAX_LINKS: usize = 3;

impl Writer {
    pub(in crate::queue::items) fn link_session(
        &self,
        all: &[Item],
        docs: &mut BTreeMap<String, Document>,
        row: &mut Value,
        session: &str,
        who: &Value,
        now: i64,
    ) -> Result<bool, Refusal> {
        let mut sessions = store::array(row.get("sessions")).to_vec();
        if sessions.iter().any(|entry| entry["session"] == session) {
            return Ok(false);
        }
        let others = all
            .iter()
            .filter(|item| row["id"] != item.id)
            .filter_map(|item| {
                let link = item
                    .sessions
                    .iter()
                    .find(|link| link["session"] == session)?;
                Some((item, link["since"].as_f64().unwrap_or(0.0)))
            });
        if others.clone().count() >= MAX_LINKS
            && let Some((oldest, _)) = others.min_by(|(_, left), (_, right)| left.total_cmp(right))
        {
            let document = self.document(docs, &oldest.owner, now)?;
            let saved = document
                .items
                .iter_mut()
                .find(|entry| entry["id"] == oldest.id)
                .ok_or_else(|| bad("The earlier Work Queue link could not be found."))?;
            if !unlink_session(saved, session, who, now) {
                append_event(
                    saved,
                    json!({"at":now,"by":who,"kind":"edited","fields":["sessions"]}),
                );
            }
            saved["updated"] = json!({"at":now,"by":who});
            self.observe_known(saved, &oldest.id);
        }
        sessions.push(json!({"session":session,"since":now}));
        row["sessions"] = sessions.into();
        Ok(true)
    }
}

pub(in crate::queue::items) fn last_word<'a>(row: &'a Value, session: &str) -> Option<&'a str> {
    for event in store::array(row.get("events")).iter().rev() {
        let kind = event["kind"].as_str();
        if kind == Some("done") {
            break;
        }
        if matches!(kind, Some("started" | "stopped"))
            && event["session"]
                .as_str()
                .or_else(|| event["by"]["session"].as_str())
                == Some(session)
        {
            return kind;
        }
    }
    None
}

// True when a prior start requires a stop event. A simple link can be removed quietly.
pub(in crate::queue::items) fn unlink_session(
    row: &mut Value,
    session: &str,
    who: &Value,
    now: i64,
) -> bool {
    let mut links = store::array(row.get("sessions")).to_vec();
    let before = links.len();
    links.retain(|entry| entry["session"] != session);
    if links.len() == before {
        return false;
    }
    row["sessions"] = links.into();
    if last_word(row, session) != Some("started") {
        return false;
    }
    let mut stop = json!({"at":now,"by":who,"kind":"stopped"});
    if who["session"] != session {
        stop["session"] = session.into();
    }
    append_event(row, stop);
    true
}

/// The item a session started with a tool is for, linked before the session sees its task: the one
/// the call names (`item`), else the first its task mentions. Nothing where it's closed, gone, or
/// can't be changed here.
#[derive(Clone)]
pub struct StartLink(pub(super) Writer);

impl StartLink {
    pub async fn link(&self, item: Option<&Value>, task: &str, starter: Value, session: &str) {
        let writer = self.0.clone();
        let reference = match item.filter(|item| !item.is_null()) {
            Some(Value::String(text)) => text.clone(),
            Some(Value::Number(number)) => number.to_string(),
            Some(_) => return,
            None => match writer.refs.mentions.find_in_task(task).first() {
                Some(mention) => mention.number.to_string(),
                None => return,
            },
        };
        let Ok(guard) = writer.lock().await else {
            return;
        };
        let session = session.to_owned();
        let _ = tokio::task::spawn_blocking(move || {
            let _guard = guard;
            let now = Timestamp::now().as_millisecond();
            let (all, compaction) = writer.changes(now)?;
            let found = writer.refs.parse(&reference).and_then(|parsed| {
                let open = |item: &&Item| item.removed.is_none() && !status::closed(&item.status);
                writer.refs.named(&all, &parsed).find(open).cloned()
            });
            if let Some(item) = found
                && writer.admit(&item.owner).is_ok()
                && edit::refuse_handing(&item).is_ok()
                && !item
                    .sessions
                    .iter()
                    .any(|link| link["session"] == session.as_str())
            {
                let mut docs = BTreeMap::new();
                let mut row = Value::Object(item.saved.clone());
                writer.link_session(&all, &mut docs, &mut row, &session, &starter, now)?;
                append_event(
                    &mut row,
                    json!({"at":now,"by":starter,"kind":"linked","to":session}),
                );
                row["updated"] = json!({"at":now,"by":starter});
                let document = writer.document(&mut docs, &item.owner, now)?;
                document.items.retain(|each| each["id"] != item.id);
                document.items.push(row);
                for document in docs.into_values() {
                    document.save(&writer.store).map_err(storage_refusal)?;
                }
            }
            compaction.finish(&writer, now)
        })
        .await;
    }
}
