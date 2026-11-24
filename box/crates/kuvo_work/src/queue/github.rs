mod command;
mod hold;
mod readings;
pub(super) mod sign_in;

pub(super) use readings::{Reader, WAIT};

use super::{
    refs::{Issue, References},
    store::{self, Error, Store},
};
use kuvo_store::format::{Fields, ObjectFormat};
use serde_json::{Value, json};

const CACHE: ObjectFormat = ObjectFormat::additive("GitHub readings cache");

#[derive(Default)]
pub(super) struct Seen {
    reads: Fields,
}

impl Seen {
    pub fn read(store: &Store) -> Result<Self, Error> {
        let fields = store::read(&store.root.join("seen.json"), CACHE)?.unwrap_or_default();
        let mut reads = fields
            .get("reads")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        reads.retain(|_, read| read["at"].is_number());
        Ok(Self { reads })
    }

    pub fn apply(&self, refs: &References, row: &mut Value) {
        let Some(source) = row.get("source") else {
            return;
        };
        let Some(issue) = source["ref"]
            .as_str()
            .and_then(|reference| refs.issue(reference))
        else {
            return;
        };
        let mut source = source.clone();
        source["name"] = self.name(&issue).into();
        let key = issue.key();
        let mut seen = row.get("seen").cloned();
        let kept_in = row.get("keptIn").cloned();
        let done_at = row.get("doneAt").cloned();
        let status = kept_in.as_ref().unwrap_or(&row["status"]).clone();
        let mut closed_at = None;
        if let Some(read) = self.reads.get(&key) {
            let kept = read.get("value").unwrap_or(&read["pull"]);
            let gone = read["gone"] == true;
            let time = |entry: &Value| {
                entry["checked"]
                    .as_f64()
                    .or_else(|| entry["at"].as_f64())
                    .unwrap_or(f64::NEG_INFINITY)
            };
            let newer = seen.as_ref().is_none_or(|remote| {
                let remote_gone = remote["gone"] == true;
                if gone == remote_gone {
                    time(read) >= time(remote)
                } else {
                    !gone
                }
            });
            if !kept.is_null() && newer {
                for field in ["kind", "url", "id"] {
                    if let Some(value) = kept.get(field) {
                        source[field] = value.clone();
                    } else if let Some(fields) = source.as_object_mut() {
                        fields.remove(field);
                    }
                }
                if let Some(title) = kept.get("title") {
                    row["title"] = title.clone();
                }
                let mut local = json!({"at":read["at"],"state":kept["state"]});
                if gone {
                    local["gone"] = true.into();
                }
                seen = Some(local);
                closed_at = kept
                    .get("closedAt")
                    .filter(|time| time.is_number())
                    .cloned();
            }
        }
        let fields = row.as_object_mut().expect("Work Queue summary");
        for name in ["keptIn", "doneAt", "struck", "doneAs"] {
            fields.remove(name);
        }
        fields.insert("source".into(), source);
        fields.insert("closesOn".into(), "github".into());
        fields.insert("status".into(), status.clone());
        if let Some(seen) = seen {
            let state = seen["state"].as_str().unwrap_or_default();
            let gone = seen["gone"] == true;
            if gone || matches!(state, "not_planned" | "closed") {
                fields.insert("struck".into(), true.into());
            }
            if gone || matches!(state, "completed" | "not_planned" | "merged" | "closed") {
                fields.insert("keptIn".into(), status);
                fields.insert("status".into(), "done".into());
                let time = closed_at
                    .or_else(|| kept_in.as_ref().and(done_at.clone()))
                    .unwrap_or_else(|| seen["at"].clone());
                fields.insert("doneAt".into(), time);
                fields.insert(
                    "doneAs".into(),
                    if gone {
                        "Gone from GitHub"
                    } else if state == "merged" {
                        "Merged on GitHub"
                    } else {
                        "Closed on GitHub"
                    }
                    .into(),
                );
            }
            fields.insert("seen".into(), seen);
        }
        if kept_in.is_none()
            && !fields.contains_key("doneAt")
            && let Some(at) = done_at
        {
            fields.insert("doneAt".into(), at);
        }
    }

    pub fn last_checked(&self, refs: &References, source: &Value) -> Option<Value> {
        let reference = refs.issue(source["ref"].as_str()?)?;
        let reading = self.reads.get(&reference.key())?;
        reading
            .get("checked")
            .or_else(|| reading.get("at"))
            .filter(|stamp| stamp.is_number())
            .cloned()
    }

    fn name(&self, issue: &Issue<'_>) -> String {
        let shared = self.reads.keys().any(|key| {
            let mut parts = key.splitn(3, '/');
            let _host = parts.next();
            let Some(owner) = parts.next() else {
                return false;
            };
            let Some((repo, _)) = parts.next().and_then(|part| part.split_once('#')) else {
                return false;
            };
            repo == issue.repo.to_ascii_lowercase() && owner != issue.owner.to_ascii_lowercase()
        });
        if shared {
            format!("{}/{}#{}", issue.owner, issue.repo, issue.number)
        } else {
            format!("{}#{}", issue.repo, issue.number)
        }
    }
}
