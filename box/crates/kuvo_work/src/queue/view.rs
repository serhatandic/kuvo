use super::{
    assignees::Assignees,
    github::Seen,
    labels,
    refs::{self, Reference, References},
    sessions::Sessions,
    status::{self, PRIORITIES, STATUSES},
    store::{COLORS, Error, Item, Store, array, limit, positive_integer},
    told,
};
use icu_collator::{Collator, options::CollatorOptions};
use jiff::{Timestamp, Zoned, civil::Date};
use kuvo_peers::setup_lists::View;
use kuvo_setup::{QueueLabel as Label, QueueSetup};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashSet};

#[derive(Default)]
pub(super) struct Query {
    pub owner: String,
    pub counts: bool,
    pub glance: bool,
    search: String,
    filters: BTreeMap<String, Vec<String>>,
}

impl Query {
    pub fn read(path: &str) -> Self {
        let mut query = Self::default();
        let mut seen = HashSet::new();
        let text = path.split_once('?').map_or("", |(_, query)| query);
        for (key, value) in url::form_urlencoded::parse(text.as_bytes()) {
            if !seen.insert(key.clone()) {
                continue;
            }
            match key.as_ref() {
                "owner" => query.owner = value.into_owned(),
                "counts" => query.counts = value == "1",
                "glance" => query.glance = value == "1",
                "q" => query.search = limit(value.trim(), 200),
                "computers" | "sources" | "owners" | "labels" => {
                    let mut names = HashSet::new();
                    let values = value
                        .split(',')
                        .map(str::trim)
                        .filter(|name| {
                            (key == "computers" || !name.is_empty())
                                && name.encode_utf16().count() <= 100
                                && names.insert(*name)
                        })
                        .take(50)
                        .map(str::to_owned)
                        .collect();
                    query.filters.insert(key.into_owned(), values);
                }
                _ => {}
            }
        }
        query
    }

    pub(super) fn visible(&self, item: &Item, sessions: &Sessions) -> bool {
        self.owner == "all"
            || self.owner == item.owner
            || (self.owner.starts_with("ses_") && sessions.under(&self.owner, &item.owner))
            || item.handed_by() == Some(&self.owner)
            || item
                .handed
                .iter()
                .any(|handoff| handoff["from"].as_str() == Some(&self.owner))
            || (item.removed.is_none()
                && item
                    .sessions
                    .iter()
                    .any(|link| link["session"].as_str() == Some(&self.owner)))
    }

    fn finds(&self, item: &Item, labels: &[Label]) -> bool {
        if self.search.is_empty() {
            return true;
        }
        let mut fields = vec![item.title.clone(), item.body.clone()];
        fields.extend(item.number.map(|number| format!("#{number}")));
        fields.extend(number_aliases(item).iter().map(|alias| {
            format!(
                "#{} {}",
                alias["number"],
                alias["name"].as_str().unwrap_or_default()
            )
        }));
        if let Some(source) = item.source() {
            fields.extend(
                ["ref", "name"]
                    .iter()
                    .filter_map(|key| source[*key].as_str())
                    .map(str::to_owned),
            );
        }
        fields.extend(
            item.notes
                .iter()
                .filter_map(|note| note["text"].as_str())
                .map(str::to_owned),
        );
        fields.extend(
            labels
                .iter()
                .filter(|label| item.labels.contains(&label.id))
                .map(|label| label.name.clone()),
        );
        let search = fields.join("\n").to_lowercase();
        self.search
            .to_lowercase()
            .split_whitespace()
            .all(|word| search.contains(word))
    }

    fn includes(&self, summary: &Value, key: &str) -> bool {
        self.filters.iter().all(|(filter, names)| {
            let contains = |name: &str| names.iter().any(|each| each == name);
            match filter.as_str() {
                "computers" => summary["computer"]["key"]
                    .as_str()
                    .map_or_else(|| contains("") || contains(key), contains),
                "owners" => contains(summary["owner"].as_str().unwrap_or_default()),
                "sources" => contains(summary["source"]["kind"].as_str().unwrap_or("none")),
                "labels" => {
                    let labels = array(summary.get("labels"));
                    if labels.is_empty() {
                        contains("none")
                    } else {
                        labels.iter().filter_map(Value::as_str).any(contains)
                    }
                }
                _ => true,
            }
        })
    }

    fn finds_told(&self, item: &Value, labels: &[Label]) -> bool {
        if self.search.is_empty() {
            return true;
        }
        let mut fields = vec![item["title"].as_str().unwrap_or_default().to_owned()];
        fields.extend(item["number"].as_f64().map(|number| format!("#{number}")));
        fields.extend(array(item.get("numberAliases")).iter().map(|alias| {
            format!(
                "#{} {}",
                alias["number"],
                alias["name"].as_str().unwrap_or_default()
            )
        }));
        for value in [
            &item["source"]["ref"],
            &item["source"]["name"],
            &item["lastNote"]["text"],
        ] {
            fields.extend(value.as_str().map(str::to_owned));
        }
        fields.extend(
            labels
                .iter()
                .filter(|label| array(item.get("labels")).iter().any(|id| id == &label.id))
                .map(|label| label.name.clone()),
        );
        let words = fields.join("\n").to_lowercase();
        self.search
            .to_lowercase()
            .split_whitespace()
            .all(|word| words.contains(word))
    }
}

#[derive(Serialize)]
pub(super) struct Owner<'a> {
    owner: &'a str,
    kind: &'static str,
    title: Option<&'a str>,
    #[serde(rename = "startedBy", skip_serializing_if = "Option::is_none")]
    started_by: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bot: Option<Value>,
}

pub(super) fn owner<'a>(
    id: &'a str,
    sessions: &'a Sessions,
    assignees: &'a Assignees,
) -> Owner<'a> {
    let bot = assignees.for_session(id);
    let (kind, title) = if id == "usr_you" {
        ("you", Some("Your own"))
    } else if id.starts_with("grp_") {
        ("group", None)
    } else if let Some(bot) = bot {
        ("bot", Some(bot.info.name.as_str()))
    } else {
        ("session", Some(sessions.title(id)))
    };
    Owner {
        owner: id,
        kind,
        title,
        started_by: sessions.started_by(id),
        bot: bot.map(|bot| json!({"id": bot.info.id, "name": bot.info.name})),
    }
}

pub(super) fn list(
    store: &Store,
    query: &Query,
    key: &str,
    references: &References,
    sessions: &Sessions,
    setup: Option<&QueueSetup>,
    peers: &View,
) -> Result<Value, Error> {
    let all = store.items()?;
    let seen = Seen::read(store)?;
    let mut labels = labels::shown(store, setup)?;
    let assignees = Assignees::read(store);
    let today = Zoned::now().date();
    let mut names = HashSet::new();
    let mut owners = Vec::new();
    if query.owner != "all" {
        names.insert(query.owner.as_str());
        owners.push(json!(owner(&query.owner, sessions, &assignees)));
    }
    let visible: Vec<_> = all
        .iter()
        .filter(|item| query.visible(item, sessions))
        .collect();
    for name in visible
        .iter()
        .map(|item| item.owner.as_str())
        .chain(visible.iter().filter_map(|item| item.handed_by()))
    {
        if names.insert(name) {
            owners.push(json!(owner(name, sessions, &assignees)));
        }
    }
    let now = Timestamp::now().as_millisecond();
    let retired = setup
        .and_then(|setup| setup.labels.as_ref())
        .map_or(&[][..], |accepted| accepted.library.removed.as_slice());
    let elsewhere =
        told::items_elsewhere(store, &all, peers, &query.owner, sessions, retired, now)?;
    for label in elsewhere.labels {
        if !labels.iter().any(|mine| mine.id == label.id) {
            labels.push(label);
        }
    }
    owners.extend(elsewhere.owners.into_iter().filter(|owner| {
        owner["owner"]
            .as_str()
            .is_some_and(|owner| !names.contains(owner))
    }));
    let mut items = Vec::new();
    let mut removed = Vec::new();
    for item in visible
        .into_iter()
        .filter(|item| query.finds(item, &labels))
    {
        let mut row = summary(item, today, references, &seen, sessions, &assignees);
        row["labels"] = json!(
            item.labels
                .iter()
                .filter(|id| labels.iter().any(|label| label.id == **id))
                .collect::<Vec<_>>()
        );
        if !query.includes(&row, key) {
            continue;
        }
        if let Some(stamp) = &item.removed {
            removed.push((stamp.at, row));
        } else {
            items.push(row);
        }
    }
    let remote = |mut row: Value| {
        seen.apply(references, &mut row);
        row.as_object_mut().expect("peer item").remove("handedFrom");
        let away = elsewhere.computers.iter().any(|computer| {
            computer["key"] == row["computer"]["key"] && computer.get("away").is_some()
        });
        row["can"] = can_elsewhere(&row, peers, away, true);
        row["labels"] = json!(
            array(row.get("labels"))
                .iter()
                .filter(|id| labels.iter().any(|label| **id == label.id))
                .collect::<Vec<_>>()
        );
        if row.get("removed").is_none()
            && row["status"]
                .as_str()
                .is_some_and(|status| !status::closed(status))
            && row["due"]
                .as_str()
                .and_then(|due| due.parse::<Date>().ok())
                .is_some_and(|date| date < today)
        {
            row["overdue"] = true.into();
        }
        row
    };
    let keep_remote = |row: &Value| query.finds_told(row, &labels) && query.includes(row, key);
    items.extend(elsewhere.items.into_iter().filter(keep_remote).map(remote));
    removed.extend(
        elsewhere
            .removed
            .into_iter()
            .filter(keep_remote)
            .map(remote)
            .map(|row| (row["removed"]["at"].as_f64().unwrap_or_default(), row)),
    );
    let place = |item: &Value| {
        STATUSES
            .iter()
            .position(|status| item["status"].as_str() == Some(status.id))
            .unwrap_or(99)
    };
    let computer = |item: &Value| {
        item.get("computer").map_or_else(
            || "0".to_owned(),
            |computer| {
                format!(
                    "1 {} {}",
                    computer["name"].as_str().unwrap_or_default(),
                    computer["key"].as_str().unwrap_or_default()
                )
            },
        )
    };
    let collation = Collator::try_new(Default::default(), CollatorOptions::default())
        .expect("the root collation is compiled in");
    items.sort_by(|left, right| {
        place(left)
            .cmp(&place(right))
            .then_with(|| collation.compare(&computer(left), &computer(right)))
            .then_with(|| {
                left["order"]
                    .as_f64()
                    .unwrap_or_default()
                    .total_cmp(&right["order"].as_f64().unwrap_or_default())
            })
    });
    removed.sort_by(|a, b| b.0.total_cmp(&a.0));
    let removed: Vec<_> = removed.into_iter().map(|(_, row)| row).collect();
    let mut answer = json!({
        "owner": query.owner, "statuses": STATUSES, "priorities": PRIORITIES,
        "labels": labels, "colors": COLORS, "assignees": assignees.rows(),
        "new": { "status": "todo", "priority": "normal" }, "today": today.to_string(),
        "items": items, "owners": owners, "removed": removed, "computers": elsewhere.computers, "pendingComputers": elsewhere.pending,
    });
    if query.counts {
        answer["counts"] = counts(&items, &labels, removed.len(), key, &elsewhere.computers);
    }
    if !query.search.is_empty() {
        answer["q"] = query.search.as_str().into();
    }
    if !query.filters.is_empty() {
        answer["filters"] = json!(query.filters);
    }
    Ok(answer)
}

fn counts(
    items: &[Value],
    labels: &[Label],
    removed: usize,
    key: &str,
    computers: &[Value],
) -> Value {
    let mut by_status: BTreeMap<&str, usize> =
        STATUSES.iter().map(|status| (status.id, 0)).collect();
    let mut by_priority: BTreeMap<&str, usize> =
        PRIORITIES.iter().map(|priority| (priority.id, 0)).collect();
    let mut by_label: BTreeMap<&str, usize> =
        labels.iter().map(|label| (label.id.as_str(), 0)).collect();
    let mut sources = BTreeMap::<&str, usize>::new();
    let mut by_computer = BTreeMap::from([(key, 0)]);
    by_computer.extend(
        computers
            .iter()
            .filter_map(|computer| computer["key"].as_str())
            .map(|key| (key, 0)),
    );
    let mut open = 0;
    for row in items {
        *by_computer
            .entry(row["computer"]["key"].as_str().unwrap_or(key))
            .or_default() += 1;
        if let Some(status) = row["status"].as_str() {
            *by_status.entry(status).or_default() += 1;
            open += usize::from(!status::closed(status));
        }
        if let Some(priority) = row["priority"].as_str() {
            *by_priority.entry(priority).or_default() += 1;
        }
        for label in array(row.get("labels")).iter().filter_map(Value::as_str) {
            *by_label.entry(label).or_default() += 1;
        }
        if let Some(source) = row["source"]["kind"].as_str() {
            *sources.entry(source).or_default() += 1;
        }
    }
    json!({
        "open": open, "statuses": by_status, "priorities": by_priority, "labels": by_label,
        "sources": sources, "computers": by_computer, "removed": removed,
    })
}

pub(super) fn number_aliases(item: &Item) -> Vec<Value> {
    array(item.saved.get("numberAliases"))
        .iter()
        .filter_map(|alias| {
            let setup = alias["setup"].as_str()?.strip_prefix("set_")?;
            let number = positive_integer(&alias["number"]).filter(|n| *n < 1_000_000.0)?;
            let name = limit(alias["name"].as_str()?, 100);
            (setup.len() == 22
                && setup
                    .bytes()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'-')))
            .then(|| json!({"setup": alias["setup"], "name": name, "number": number}))
        })
        .collect()
}

pub(super) fn summary(
    item: &Item,
    today: Date,
    references: &References,
    seen: &Seen,
    sessions: &Sessions,
    assignees: &Assignees,
) -> Value {
    let mut row = json!({
        "id": item.id, "owner": item.owner, "title": item.title, "status": item.status, "order": item.order,
        "priority": item.priority, "labels": item.labels,
        "sessions": item.sessions.iter().filter_map(|link| {
            let id = link["session"].as_str()?;
            Some(json!({"session": id, "title": assignees.title(sessions, id),
                "working": item.removed.is_none() && !status::closed(&item.status) && sessions.running(id)}))
        }).collect::<Vec<_>>(),
        "noteCount": item.notes.len(), "fileCount": item.files.len(), "linkCount": item.links.len(),
        "created": item.created, "updated": item.updated,
    });
    if let Some(number) = item.number {
        row["number"] = json!(number);
    } else if item.source().is_none() {
        row["numberPending"] = true.into();
    }
    let aliases = number_aliases(item);
    if !aliases.is_empty() {
        row["numberAliases"] = json!(aliases);
    }
    if let Some(wanted) = item.saved.get("wanted").and_then(positive_integer) {
        row["wanted"] = json!(wanted);
    }
    if let Some(assignee) = item.saved.get("assignee")
        && let (Some(kind @ ("bot" | "agent")), Some(id)) =
            (assignee["kind"].as_str(), assignee["id"].as_str())
        && (1..=80).contains(&id.len())
        && id
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'.' | b'-'))
    {
        row["assignee"] = assignees.named(kind, id);
    }
    if let Some(due) = item.saved.get("due").and_then(Value::as_str).map(str::trim)
        && due.len() == 10
        && let Ok(date) = due.parse::<Date>()
        && date.to_string() == due
    {
        row["due"] = due.into();
        if date < today && item.removed.is_none() && !status::closed(&item.status) {
            row["overdue"] = true.into();
        }
    }
    if let Some(progress) = item.saved.get("progress").and_then(Value::as_object) {
        let fields: Map<_, _> = progress
            .iter()
            .filter(|(name, value)| {
                (name.as_str() == "value" && value.is_number())
                    || (name.as_str() == "note" && value.is_string())
            })
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        row["progress"] = fields.into();
    }
    if let Some(note) = item.notes.last() {
        row["lastNote"] = note.clone();
    }
    if let Some(owner) = item.handed_by() {
        row["handedBy"] = owner.into();
    }
    if let Some(source) = item.source() {
        row["source"] = source.clone();
    }
    let handing = item.saved.get("handing").filter(|handing| {
        handing["id"].as_str().is_some_and(|id| {
            id.strip_prefix("wqh_").is_some_and(|tail| {
                (8..=40).contains(&tail.len())
                    && tail
                        .bytes()
                        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'-'))
            })
        }) && handing["to"].as_str().is_some_and(super::store::owner_id)
            && handing["key"].is_string()
            && handing["at"].is_number()
    });
    if let Some(handing) = handing {
        row["handing"] =
            json!({"to": handing["to"], "name": handing["name"].as_str().unwrap_or_default()});
    }
    if let Some(done) = item.saved.get("doneAt").filter(|done| done.is_number()) {
        row["doneAt"] = done.clone();
    }
    if let Some(removed) = &item.removed {
        row["removed"] = json!(removed);
    }
    seen.apply(references, &mut row);
    row["can"] = can(&row);
    row
}

pub(super) fn mutation(item: &Item, assignees: &Assignees) -> Value {
    let mut row = summary(
        item,
        Zoned::now().date(),
        &References::new(),
        &Seen::default(),
        &Sessions::default(),
        assignees,
    );
    row["mentionTargets"] = json!(array(
        item.saved
            .get("mentionIndex")
            .and_then(|index| index.get("targets"))
    ));
    json!({"item": row})
}

pub(super) fn detail(
    item: &Item,
    all: &[Item],
    references: &References,
    seen: &Seen,
    sessions: &Sessions,
    assignees: &Assignees,
) -> Value {
    let mut row = summary(
        item,
        Zoned::now().date(),
        references,
        seen,
        sessions,
        assignees,
    );
    let index = item.saved.get("mentionIndex").unwrap_or(&Value::Null);
    row["body"] = json!(item.body);
    row["notes"] = json!(
        item.notes
            .iter()
            .enumerate()
            .map(|(at, note)| {
                let mut note = note.clone();
                note["mentions"] = json!(bound_mentions(&index["notes"][at]));
                note
            })
            .collect::<Vec<_>>()
    );
    row["events"] = json!(item.events);
    row["files"] = json!(item.files);
    row["links"] = json!(item.links);
    row["handed"] = json!(item.handed);
    row["itemRelations"] = json!(refs::relations(item, all));
    row["bodyMentions"] = json!(bound_mentions(&index["body"]));
    row["canLinkItems"] = (item.removed.is_none() && row.get("handing").is_none()).into();
    json!({"item": row})
}

fn bound_mentions(value: &Value) -> Vec<&Value> {
    array(Some(value))
        .iter()
        .filter(|range| {
            range["item"].as_str().is_some_and(super::store::item_id)
                && ["start", "end", "number"]
                    .iter()
                    .all(|key| range[key].is_number())
        })
        .collect()
}

fn ref_facts(item: &Item, today: Date, references: &References, seen: &Seen) -> Value {
    let row = summary(
        item,
        today,
        references,
        seen,
        &Sessions::default(),
        &Assignees::default(),
    );
    ref_summary(&row)
}

fn ref_summary(row: &Value) -> Value {
    let mut facts = Map::new();
    for key in [
        "id",
        "number",
        "numberPending",
        "numberAliases",
        "source",
        "seen",
        "struck",
        "closesOn",
        "title",
        "status",
        "owner",
        "computer",
    ] {
        if let Some(value) = row.get(key) {
            facts.insert(key.into(), value.clone());
        }
    }
    if row.get("removed").is_some() {
        facts.insert("removed".into(), true.into());
    }
    facts.into()
}

pub(super) fn references<'a>(
    items: &[Item],
    far: &[Value],
    parser: &References,
    seen: &Seen,
    listed: impl IntoIterator<Item = &'a str>,
) -> Value {
    let today = Zoned::now().date();
    let mut found = Map::new();
    let mut aliases = Map::new();
    for text in listed
        .into_iter()
        .map(str::trim)
        .filter(|text| !text.is_empty() && text.encode_utf16().count() <= 300)
        .take(200)
    {
        let reference = parser.parse(text);
        let item = reference
            .as_ref()
            .and_then(|reference| parser.find(items, reference));
        let across = if item.is_none_or(|item| item.removed.is_some()) {
            reference.as_ref().and_then(|reference| {
                far.iter()
                    .filter(|row| parser.names_summary(row, reference))
                    .max_by(|a, b| {
                        a["updated"]["at"]
                            .as_f64()
                            .unwrap_or_default()
                            .total_cmp(&b["updated"]["at"].as_f64().unwrap_or_default())
                    })
            })
        } else {
            None
        };
        let facts = across
            .map(ref_summary)
            .or_else(|| item.map(|item| ref_facts(item, today, parser, seen)));
        found.insert(text.into(), facts.unwrap_or(Value::Null));
        if let Some(Reference::Number {
            number,
            setup: None,
        }) = reference
        {
            let matches: Vec<_> = items
                .iter()
                .filter(|item| {
                    number_aliases(item)
                        .iter()
                        .any(|alias| alias["number"].as_f64() == Some(f64::from(number)))
                })
                .map(|item| ref_facts(item, today, parser, seen))
                .chain(
                    far.iter()
                        .filter(|item| {
                            array(item.get("numberAliases"))
                                .iter()
                                .any(|alias| alias["number"].as_f64() == Some(f64::from(number)))
                        })
                        .map(ref_summary),
                )
                .collect();
            if !matches.is_empty() {
                aliases.insert(text.into(), matches.into());
            }
        }
    }
    let mut answer = json!({"refs": found});
    if !aliases.is_empty() {
        answer["aliases"] = aliases.into();
    }
    answer
}

pub(super) fn can_elsewhere(row: &Value, peers: &View, away: bool, client: bool) -> Value {
    let key = row["computer"]["key"]
        .as_str()
        .and_then(kuvo_wire::keys::decode_exact::<32>);
    let has = |feature| key.is_some_and(|key| peers.advertises(&key, feature));
    let there = row["computer"]["name"].as_str().unwrap_or("Its computer");
    let blocked = if away {
        Some(format!(
            "{there} isn't answering now (it may be asleep, off or offline): it can be changed once it's back."
        ))
    } else if !client || !has("work-queue.forward") {
        Some(format!("It's on {there}: change it there."))
    } else {
        None
    };
    if let Some(why) = blocked {
        return json!({"edit":false,"set":false,"assign":false,"close":false,"delete":false,"restore":false,"attach":false,"why":why});
    }
    let mut can = can(row);
    can["assign"] = false.into();
    can["attach"] = (can["attach"] == true && has("work-queue.attachments")).into();
    can
}

fn can(row: &Value) -> Value {
    let mut can = json!({"edit": false, "set": false, "assign": false, "close": false, "delete": false, "restore": false, "attach": false});
    if row.get("removed").is_some() {
        can["restore"] = true.into();
        can["why"] = "It's deleted: bring it back to change it.".into();
    } else if let Some(handing) = row.get("handing") {
        let name = handing["name"]
            .as_str()
            .filter(|name| !name.is_empty())
            .unwrap_or("a conversation");
        can["delete"] = true.into();
        can["why"] = format!("It's on its way to {name} on another computer: it can be changed there once it's arrived.").into();
    } else if row.get("source").is_some() {
        let open = row.get("keptIn").is_none();
        can["set"] = open.into();
        can["assign"] = open.into();
        can["delete"] = true.into();
        can["why"] = if open {
            "Its title, its description and closing it are GitHub's."
        } else {
            "It's closed on GitHub: it's back where it stood once it's reopened there."
        }
        .into();
    } else {
        for field in ["edit", "set", "assign", "close", "delete", "attach"] {
            can[field] = true.into();
        }
    }
    can
}
