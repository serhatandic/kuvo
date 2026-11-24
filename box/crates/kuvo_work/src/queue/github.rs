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
        row["closesOn"] = "github".into();
        if let Some(read) = self.reads.get(&key) {
            let kept = read.get("value").unwrap_or(&read["pull"]);
            if !kept.is_null() {
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
                let state = kept["state"].as_str().unwrap_or_default();
                let gone = read["gone"].as_bool() == Some(true);
                let mut seen = json!({"at": read["at"], "state": state});
                if gone {
                    seen["gone"] = true.into();
                }
                row["seen"] = seen;
                if gone || matches!(state, "not_planned" | "closed") {
                    row["struck"] = true.into();
                }
                if gone || matches!(state, "completed" | "not_planned" | "merged" | "closed") {
                    row["keptIn"] = row["status"].clone();
                    row["status"] = "done".into();
                    row["doneAt"] = kept
                        .get("closedAt")
                        .filter(|at| !at.is_null())
                        .unwrap_or(&read["at"])
                        .clone();
                    row["doneAs"] = if gone {
                        "Gone from GitHub"
                    } else if state == "merged" {
                        "Merged on GitHub"
                    } else {
                        "Closed on GitHub"
                    }
                    .into();
                }
            }
        }
        row["source"] = source;
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
