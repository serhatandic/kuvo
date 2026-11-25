//! Resolve claims before allocating an ordinary number to each remaining item.

use super::*;
use crate::queue::numbers::Grant;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

pub(super) struct Book {
    pub items: Vec<Item>,
    dirty: BTreeSet<usize>,
}

impl Book {
    pub fn new(items: Vec<Item>) -> Self {
        Self {
            items,
            dirty: BTreeSet::new(),
        }
    }

    pub fn push(&mut self, item: Item) -> usize {
        let place = self.items.len();
        self.items.push(item);
        self.dirty.insert(place);
        place
    }

    pub fn mark(&mut self, index: usize) -> &mut Item {
        self.dirty.insert(index);
        &mut self.items[index]
    }

    pub fn settle(
        &mut self,
        answer: Option<Grant>,
        may_number: impl Fn(&Item) -> bool,
        now: i64,
    ) -> (Vec<usize>, Vec<u64>) {
        let mut used = Vec::new();
        if let Some(answer) = answer {
            let mut held: HashSet<_> = self.items.iter().filter_map(number).collect();
            for index in 0..self.items.len() {
                let item = &self.items[index];
                let Some(wanted) = wanted(item) else { continue };
                let Some(granted) = answer.accepts(wanted) else {
                    continue;
                };
                if flag(item, "backGranted") || !may_number(item) {
                    continue;
                }
                let item = self.mark(index);
                if let Some(current) = number(item) {
                    if granted {
                        item.saved.insert("backGranted".into(), true.into());
                    } else {
                        stays(
                            item,
                            format!(
                                "Its own number, #{wanted}, couldn't be given back (another of your computers was given it already, or it's far past the numbers in use), so it stays #{current}."
                            ),
                            now,
                        );
                    }
                    continue;
                }
                if granted && held.insert(wanted) {
                    assign(item, wanted);
                    used.push(wanted);
                } else {
                    note(
                        item,
                        format!(
                            "Imported as #{wanted}, a number that couldn't be kept (another of your computers was given it already, or it's far past the numbers in use), so it has a new one."
                        ),
                        now,
                    );
                    item.saved.insert("importedAs".into(), wanted.into());
                }
                item.saved.remove("wanted");
            }
        }
        self.restore(now, &mut used);
        let mut waiting = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                number(item).is_none() && wanted(item).is_none() && may_number(item)
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        waiting.sort_by(|a, b| {
            self.items[*a]
                .created
                .at
                .total_cmp(&self.items[*b].created.at)
        });
        (waiting, used)
    }

    fn restore(&mut self, now: i64, used: &mut Vec<u64>) {
        let back = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                (flag(item, "backGranted"))
                    .then(|| Some((index, number(item)?, wanted(item)?)))
                    .flatten()
            })
            .collect::<Vec<_>>();
        if back.is_empty() {
            return;
        }
        let mut first = HashMap::new();
        for (index, _, wanted) in &back {
            first.entry(*wanted).or_insert(*index);
        }
        let mut holders = HashMap::<_, Vec<_>>::new();
        for (index, item) in self.items.iter().enumerate() {
            if let Some(held) = number(item).or_else(|| wanted(item)) {
                holders.entry(held).or_default().push(index);
            }
        }
        let mut going: HashSet<_> = first.values().copied().collect();
        let blocker = |index, target, going: &HashSet<usize>| {
            let first = first[&target];
            (first != index).then_some(first).or_else(|| {
                holders
                    .get(&target)?
                    .iter()
                    .copied()
                    .find(|each| *each != index && !going.contains(each))
            })
        };
        // Removing a blocked restore can expose another blocker. A cycle can move together.
        loop {
            let mut changed = false;
            for (index, _, target) in &back {
                if going.contains(index) && blocker(*index, *target, &going).is_some() {
                    changed |= going.remove(index);
                }
            }
            if !changed {
                break;
            }
        }
        for (index, previous, target) in back {
            if going.contains(&index) {
                let item = self.mark(index);
                note(
                    item,
                    format!("Has its own number, #{target}, again (it was #{previous})."),
                    now,
                );
                assign(item, target);
                for key in ["wanted", "importedAs", "backGranted"] {
                    item.saved.remove(key);
                }
                used.push(target);
            } else if let Some(holder) =
                blocker(index, target, &going).map(|holder| &self.items[holder])
            {
                if holder.removed.is_none()
                    && !flag(holder, "stays")
                    && number(holder) == Some(target)
                    && wanted(holder).is_some()
                {
                    continue;
                }
                stays(
                    self.mark(index),
                    format!(
                        "Its own number, #{target}, is another item's here, so it stays #{previous}."
                    ),
                    now,
                );
            }
        }
    }

    // Binding a newly numbered item belongs in the same owner-file write as
    // the number itself, not in a second asynchronous update.
    pub fn sync_mentions(&mut self, writer: &Writer, before: &HashMap<String, u32>) {
        let updates =
            crate::queue::mentions::sync_indexes(&writer.refs.mentions, &self.items, before);
        let mut admitted = HashMap::new();
        for (at, index) in updates {
            let item = &self.items[at];
            if !*admitted
                .entry(item.owner.clone())
                .or_insert_with(|| writer.admit(&item.owner).is_ok())
            {
                continue;
            }
            let saved = &mut self.mark(at).saved;
            saved.insert("mentionIndex".into(), json!(index));
            saved.remove("mentionLinks");
        }
    }

    pub fn save(self, writer: &Writer, now: i64) -> Result<(), Refusal> {
        let mut documents = BTreeMap::<String, Document>::new();
        for index in self.dirty {
            let item = &self.items[index];
            let owner = &item.owner;
            if !documents.contains_key(owner) {
                documents.insert(owner.clone(), writer.read_owner(owner, now)?);
            }
            let document = documents.get_mut(owner).expect("opened queue");
            let value = Value::Object(item.saved.clone());
            if let Some(saved) = document
                .items
                .iter_mut()
                .find(|saved| saved["id"] == item.id)
            {
                *saved = value;
            } else {
                document.items.push(value);
            }
        }
        for document in documents.into_values() {
            document.save(&writer.store).map_err(storage_refusal)?;
        }
        Ok(())
    }
}

pub(super) fn assign(item: &mut Item, number: u64) {
    item.number = Some(number as f64);
    item.saved.insert("number".into(), number.into());
}

pub(super) fn number(item: &Item) -> Option<u64> {
    item.number.map(|number| number as u64)
}

pub(super) fn wanted(item: &Item) -> Option<u64> {
    item.saved
        .get("wanted")
        .and_then(store::positive_integer)
        .map(|value| value as u64)
}

pub(super) fn imported_as(item: &Item) -> Option<u64> {
    item.saved
        .get("importedAs")
        .and_then(store::positive_integer)
        .map(|value| value as u64)
}

pub(super) fn flag(item: &Item, key: &str) -> bool {
    item.saved.get(key) == Some(&Value::Bool(true))
}

fn note(item: &mut Item, text: String, now: i64) {
    item.notes
        .push(json!({"at":now,"by":{"user":true},"text":text}));
    item.saved.insert("notes".into(), item.notes.clone().into());
}

fn stays(item: &mut Item, message: String, now: i64) {
    note(item, message, now);
    item.saved.remove("wanted");
    item.saved.remove("backGranted");
    item.saved.insert("stays".into(), true.into());
}
