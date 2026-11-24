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
