//! Resolve compact session headers, not conversations or provider message bodies.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct Node {
    pub parent: Option<String>,
    pub starters: Vec<String>,
    pub bot: Option<String>,
    pub name: Option<String>,
    pub own: bool,
}
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct Line {
    pub root: String,
    pub bot: Option<String>,
    pub name: Option<String>,
    pub own: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct Bot {
    id: String,
    name: String,
    look: Option<Value>,
}
#[derive(Default)]
pub struct Bots {
    by_session: BTreeMap<String, Bot>,
    by_id: BTreeMap<String, Bot>,
}
fn word(value: &Value) -> Option<String> {
    value.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}
impl Bots {
    pub fn read(home: &Path) -> Self {
        let mut result = Self::default();
        let path = home.join("bots");
        for entry in kuvo_store::files::list_of(&path) {
            let Some(id) = entry.to_str() else {
                continue;
            };
            if !id.starts_with("bot_")
                || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                continue;
            }
            let folder = path.join(&entry);
            let Some(data) = kuvo_store::files::text_of(&folder.join("bot.json"))
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            else {
                continue;
            };
            if data["id"].as_str() != Some(id) || data["kuvo"].as_u64() != Some(1) {
                continue;
            }
            let Some(name) = word(&data["name"]) else {
                continue;
            };
            let look = data.get("look").filter(|v| v.is_object()).cloned();
            let bot = Bot {
                id: id.into(),
                name,
                look,
            };
            if let Some(session) = kuvo_store::files::text_of(&folder.join("session"))
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
            {
                result.by_session.insert(session, bot.clone());
            }
            result.by_id.insert(id.into(), bot);
        }
        result
    }
    pub fn appearance(&self, id: &str) -> Option<(&str, Option<&Value>)> {
        self.by_id
            .get(id)
            .map(|bot| (bot.name.as_str(), bot.look.as_ref()))
    }
    pub fn decorate(&self, id: &str, node: &mut Node) {
        if let Some(bot) = self.by_session.get(id) {
            node.bot = Some(bot.id.clone());
            node.name = Some(bot.name.clone());
            node.own = true;
        } else if node.name.is_none() {
            node.name = node
                .bot
                .as_ref()
                .and_then(|id| self.by_id.get(id))
                .map(|bot| bot.name.clone());
        }
    }
}
impl Node {
    pub fn from_metadata(parent: Option<String>, metadata: &Value) -> Self {
        let kuvo = &metadata["kuvo"];
        let starter = word(&kuvo["startedBy"]);
        let mut starters = starter.iter().cloned().collect::<Vec<_>>();
        if let Some(line) = kuvo["line"].as_array() {
            for other in line.iter().rev().take(20).filter_map(word) {
                if !starters.contains(&other) {
                    starters.push(other);
                }
            }
        }
        let seat = word(&kuvo["seat"]["bot"]);
        let by = word(&kuvo["by"]["bot"]);
        Self {
            parent: parent.filter(|s| !s.is_empty()),
            starters,
            own: seat.is_some(),
            name: if seat.is_none() {
                word(&kuvo["by"]["name"])
            } else {
                None
            },
            bot: seat.or(by),
        }
    }
}
pub struct Resolver<'a> {
    nodes: &'a BTreeMap<String, Node>,
    roots: BTreeMap<String, String>,
    owners: BTreeMap<String, (String, Option<String>)>,
}
impl<'a> Resolver<'a> {
    pub fn new(nodes: &'a BTreeMap<String, Node>) -> Self {
        Self {
            nodes,
            roots: BTreeMap::new(),
            owners: BTreeMap::new(),
        }
    }
    fn root(&mut self, id: &str) -> String {
        let mut path: Vec<String> = vec![];
        let mut current = id.to_owned();
        loop {
            if let Some(root) = self.roots.get(&current) {
                current = root.clone();
                break;
            }
            if let Some(cycle) = path.iter().position(|seen| seen == &current) {
                if let Some(root) = path[cycle..].iter().min() {
                    current = root.clone();
                }
                break;
            }
            let Some(parent) = self
                .nodes
                .get(&current)
                .and_then(|node| node.parent.clone())
            else {
                break;
            };
            path.push(current);
            current = parent;
        }
        for found in path {
            self.roots.insert(found, current.clone());
        }
        current
    }
    fn owner(
        &mut self,
        root: &str,
        path: &mut BTreeSet<String>,
    ) -> Option<(String, Option<String>)> {
        if let Some(owner) = self.owners.get(root) {
            return Some(owner.clone());
        }
        let node = self.nodes.get(root)?;
        if let Some(bot) = &node.bot {
            return Some((bot.clone(), node.name.clone()));
        }
        if !path.insert(root.to_owned()) {
            return None;
        }
        let starters = node.starters.clone();
        for starter in &starters {
            let start = self.root(starter);
            if let Some(owner) = self.owner(&start, path) {
                path.remove(root);
                self.owners.insert(root.to_owned(), owner.clone());
                return Some(owner);
            }
        }
        path.remove(root);
        None
    }
    pub fn line(&mut self, id: &str) -> Line {
        let root = self.root(id);
        let own = self.nodes.get(&root).is_some_and(|node| node.own);
        let (bot, name) = self
            .owner(&root, &mut BTreeSet::new())
            .map_or((None, None), |(bot, name)| (Some(bot), name));
        Line {
            root,
            bot,
            name,
            own,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn own_chats_starters_nested_children_and_old_starts_share_the_correct_bot() {
        let mut nodes = BTreeMap::from([
            (
                "lead".into(),
                Node {
                    bot: Some("bot_lead".into()),
                    name: Some("Lead".into()),
                    own: true,
                    ..Default::default()
                },
            ),
            (
                "lead-child".into(),
                Node {
                    parent: Some("lead".into()),
                    ..Default::default()
                },
            ),
            (
                "work".into(),
                Node::from_metadata(
                    None,
                    &json!({"kuvo":{"startedBy":"lead","by":{"bot":"bot_lead","name":"Lead"}}}),
                ),
            ),
            (
                "work-child".into(),
                Node {
                    parent: Some("work".into()),
                    ..Default::default()
                },
            ),
            (
                "work-grandchild".into(),
                Node {
                    parent: Some("work-child".into()),
                    bot: Some("bot_other".into()),
                    ..Default::default()
                },
            ),
            (
                "old-start".into(),
                Node::from_metadata(None, &json!({"kuvo":{"startedBy":"work"}})),
            ),
            (
                "seat".into(),
                Node::from_metadata(None, &json!({"kuvo":{"seat":{"bot":"bot_lead"}}})),
            ),
            ("alone".into(), Node::default()),
        ]);
        nodes.insert(
            "removed-child".into(),
            Node {
                parent: Some("removed-parent".into()),
                ..Default::default()
            },
        );
        let mut resolve = Resolver::new(&nodes);
        for (session, root, own) in [
            ("lead", "lead", true),
            ("lead-child", "lead", true),
            ("work", "work", false),
            ("work-child", "work", false),
            ("work-grandchild", "work", false),
            ("old-start", "old-start", false),
            ("seat", "seat", true),
        ] {
            let line = resolve.line(session);
            assert_eq!(line.root, root);
            assert_eq!(line.bot.as_deref(), Some("bot_lead"));
            assert_eq!(line.own, own);
        }
        assert!(resolve.line("alone").bot.is_none());
        assert_eq!(resolve.line("removed-child").root, "removed-parent");
    }
}

use rusqlite::{Connection, OptionalExtension, params};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
pub fn schema(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS insight_line_node(session TEXT NOT NULL,source INTEGER NOT NULL,body TEXT NOT NULL,PRIMARY KEY(session,source));
        CREATE TABLE IF NOT EXISTS insight_line(session TEXT PRIMARY KEY,body TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS insight_line_channel(channel TEXT NOT NULL,session TEXT NOT NULL,PRIMARY KEY(channel,session));")
}
/// Source 0 is the engine; source 1 is a portable transcript, whose defined
/// header fields take precedence. Only changed compact headers are stored.
pub fn take(
    db: &Connection,
    source: u8,
    nodes: &BTreeMap<String, Node>,
    bots: &Bots,
    seq: &mut u64,
    repair: bool,
) -> Result<bool> {
    let mut changed = false;
    for (id, node) in nodes {
        changed |= db.execute("INSERT INTO insight_line_node VALUES(?,?,?) ON CONFLICT(session,source) DO UPDATE SET body=excluded.body WHERE body<>excluded.body",params![id,source,serde_json::to_string(node)?])? > 0;
    }
    let bot_stamp = super::hash(&serde_json::to_value(&bots.by_id)?);
    let kept = super::get(db, "rust-lineage-bots")?.and_then(|v| v.as_str().map(str::to_owned));
    // The session marker is part of bot ownership, not just appearance.
    let link_stamp = super::hash(&serde_json::to_value(&bots.by_session)?);
    let stamp = format!("{bot_stamp}/{link_stamp}");
    if kept.as_deref() != Some(&stamp) {
        changed = true;
        super::set(db, "rust-lineage-bots", &serde_json::json!(stamp))?;
    }
    if !changed && !repair {
        return Ok(false);
    }
    let mut all = BTreeMap::<String, Node>::new();
    let mut query =
        db.prepare("SELECT session,body FROM insight_line_node ORDER BY session,source")?;
    for result in query.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (id, body) = result?;
        let given: Node = serde_json::from_str(&body)?;
        let node = all.entry(id).or_default();
        if given.parent.is_some() {
            node.parent = given.parent;
        }
        if !given.starters.is_empty() {
            node.starters = given.starters;
        }
        if given.bot.is_some() {
            node.bot = given.bot;
            node.name = given.name;
            node.own = given.own;
        }
    }
    for id in bots.by_session.keys() {
        all.entry(id.clone()).or_default();
    }
    for (id, node) in &mut all {
        bots.decorate(id, node);
    }
    let mut resolver = Resolver::new(&all);
    for id in all.keys() {
        let line = resolver.line(id);
        let body = serde_json::to_string(&line)?;
        let changed=db.execute("INSERT INTO insight_line VALUES(?,?) ON CONFLICT(session) DO UPDATE SET body=excluded.body WHERE body<>excluded.body",params![id,body])?;
        if changed == 0 && !(repair && nodes.contains_key(id)) {
            continue;
        }
        let mut patch = serde_json::json!({"root":(line.root!=*id).then_some(&line.root)});
        if let Some(bot) = &line.bot {
            patch["bot"] = serde_json::json!(bot);
            patch["botName"] = serde_json::json!(line.name);
            patch["botOwn"] = serde_json::json!(line.own.then_some(true));
        }
        let patch = serde_json::to_string(&patch)?;
        let rows = {
            let mut query=db.prepare("SELECT key,seq,json_patch(body,?1) FROM event WHERE session=?2 AND body<>json_patch(body,?1)")?;
            let rows = query.query_map(params![patch, id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<u64>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (key, before, body) in rows {
            let revision = before.map(|_| {
                *seq += 1;
                *seq
            });
            db.execute("UPDATE event SET seq=?1,body=?2,bot=json_extract(?2,'$.bot'),bot_name=json_extract(?2,'$.botName') WHERE key=?3",params![revision,body,key])?;
        }
    }
    Ok(true)
}
pub fn line_of(db: &Connection, id: &str) -> Result<Line> {
    let saved: Option<String> = db
        .query_row(
            "SELECT body FROM insight_line WHERE session=?",
            [id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(saved
        .map(|s| serde_json::from_str(&s))
        .transpose()?
        .unwrap_or_else(|| Line {
            root: id.into(),
            ..Default::default()
        }))
}

pub fn channel_sessions(db: &Connection, channel: &str) -> Result<Vec<String>> {
    let mut q = db.prepare("SELECT session FROM insight_line_channel WHERE channel=?")?;
    let rows = q.query_map([channel], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}
pub fn keep_channel(db: &Connection, channel: &str, nodes: &BTreeMap<String, Node>) -> Result<()> {
    for id in nodes.keys() {
        db.execute(
            "INSERT OR IGNORE INTO insight_line_channel VALUES(?,?)",
            params![channel, id],
        )?;
    }
    Ok(())
}
