use super::store::Store;
use icu_collator::{Collator, options::CollatorOptions};
use kuvo_store::files;
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) const AGENTS: [(&str, &str); 5] = [
    ("claude-code", "Claude Code"),
    ("codex-cli", "Codex"),
    ("cursor", "Cursor"),
    ("grok-cli", "Grok"),
    ("antigravity", "Antigravity"),
];

#[derive(Default)]
pub(super) struct Assignees {
    bots: Vec<Bot>,
}

pub(super) struct Bot {
    pub info: Description,
    pub session: String,
}

#[derive(Deserialize)]
pub(super) struct Description {
    kuvo: f64,
    pub id: String,
    pub name: String,
    created: Option<String>,
    look: Option<Value>,
}

impl Bot {
    fn assignee(&self) -> Value {
        let mut row = json!({"kind": "bot", "id": self.info.id, "name": self.info.name});
        if let Some(look) = &self.info.look {
            row["look"] = look.clone();
        }
        row
    }
}

impl Assignees {
    pub fn read(store: &Store) -> Self {
        let Some(home) = store.root.parent().and_then(std::path::Path::parent) else {
            return Self::default();
        };
        let home = home.join("bots");
        let mut bots: Vec<_> = files::list_of(&home)
            .into_iter()
            .filter_map(|name| {
                let id = name.to_str()?;
                let tail = id.strip_prefix("bot_")?;
                if tail.is_empty() || !tail.bytes().all(|ch| ch.is_ascii_alphanumeric()) {
                    return None;
                }
                let path = home.join(id);
                let description = files::text_of(&path.join("bot.json"))?;
                let info: Description = serde_json::from_str(&description).ok()?;
                let session = files::text_of(&path.join("session"))?.trim().to_owned();
                (info.kuvo == 1.0 && info.id == id && !session.is_empty())
                    .then_some(Bot { info, session })
            })
            .collect();
        let order = Collator::try_new(Default::default(), CollatorOptions::default())
            .expect("the root collation is compiled in");
        bots.sort_by(|a, b| {
            order.compare(
                a.info.created.as_deref().unwrap_or_default(),
                b.info.created.as_deref().unwrap_or_default(),
            )
        });
        Self { bots }
    }

    pub fn for_session(&self, session: &str) -> Option<&Bot> {
        self.bots.iter().find(|bot| bot.session == session)
    }

    pub fn bot_session(&self, id: &str) -> Option<&str> {
        self.bots
            .iter()
            .find(|bot| bot.info.id == id)
            .map(|bot| bot.session.as_str())
    }

    pub fn rows(&self) -> Vec<Value> {
        self.bots
            .iter()
            .map(Bot::assignee)
            .chain(
                AGENTS
                    .iter()
                    .map(|(id, name)| json!({"kind": "agent", "id": id, "name": name})),
            )
            .collect()
    }

    pub fn named(&self, kind: &str, id: &str) -> Value {
        let name = if kind == "bot" {
            if let Some(bot) = self.bots.iter().find(|bot| bot.info.id == id) {
                return bot.assignee();
            }
            "A bot on another computer"
        } else {
            AGENTS
                .iter()
                .find(|(key, _)| *key == id)
                .map_or(id, |(_, name)| name)
        };
        json!({"kind": kind, "id": id, "name": name})
    }

    pub fn title<'a>(&'a self, sessions: &'a super::sessions::Sessions, id: &str) -> &'a str {
        self.for_session(id)
            .map_or_else(|| sessions.title(id), |bot| bot.info.name.as_str())
    }
}
