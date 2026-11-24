//! Import a numbered Markdown backlog through the normal queue owner lock.

mod parse;

use super::book::{Book, assign, flag, imported_as, number, wanted};
use super::*;
use crate::queue::numbers::{contains, runs};
use kuvo_protocol::routes::post_kuvo_work_queue_import as route;
use parse::{Backlog, Imported, Parser};
use std::collections::BTreeMap;

const MOST_SKIPPED: usize = 40;

impl Writer {
    pub(super) fn register_import(&self, registry: &mut Registry) {
        let writer = self.clone();
        let parser = Arc::new(Parser::new());
        registry.register::<route::Spec, _, _>(Access::Any, move |call| {
            let writer = writer.clone();
            let parser = parser.clone();
            async move {
                if !work_queue_client(&call.request.caller) {
                    return Err(Refusal::new(StatusCode::FORBIDDEN, CLIENT));
                }
                let body: Value = serde_json::from_slice(&call.request.body).unwrap_or(Value::Null);
                let owner = body["owner"]
                    .as_str()
                    .filter(|owner| owner_id(owner))
                    .ok_or_else(|| {
                        bad("Say whose queue it goes in (owner): a conversation's address.")
                    })?
                    .to_owned();
                writer.admit(&owner)?;
                let home = writer.store.clone();
                let setup = writer.setup.queue_setup();
                let parsed = tokio::task::spawn_blocking(move || {
                    let labels = labels::shown(&home, setup.as_ref()).map_err(storage_refusal)?;
                    parser.files(&body["files"], &labels).map_err(bad)
                })
                .await
                .map_err(internal)??;
                typed(writer.import(owner, parsed, json!({"user":true})).await?)
            }
        });
    }

    pub(super) async fn agent_import(
        &self,
        owner: String,
        given: Value,
        actor: Value,
    ) -> Result<Value, Refusal> {
        self.admit(&owner)?;
        let store = self.store.clone();
        let setup = self.setup.queue_setup();
        let parsed = tokio::task::spawn_blocking(move || {
            let labels = labels::shown(&store, setup.as_ref()).map_err(storage_refusal)?;
            parse::agent_items(&given, &labels).map_err(bad)
        })
        .await
        .map_err(internal)??;
        self.import(owner, parsed, actor).await
    }

    async fn import(&self, owner: String, backlog: Backlog, who: Value) -> Result<Value, Refusal> {
        let guard = self.lock().await?;
        self.admit(&owner)?;
        let now = Timestamp::now().as_millisecond();
        let used = self.numbers.used().await.map_err(storage_refusal)?;
        let writer = self.clone();
        let (mut book, plan, compaction) = tokio::task::spawn_blocking(move || {
            let (items, compaction) = writer.changes(now)?;
            let mut book = Book::new(items);
            let elsewhere = told::known_numbers(&writer.store).map_err(storage_refusal)?;
            let plan = Plan::prepare(
                &mut book,
                &owner,
                backlog.items,
                &used,
                &elsewhere,
                now,
                &who,
            )?;
            Ok::<_, Refusal>((book, plan, compaction))
        })
        .await
        .map_err(internal)??;
        let high = highest(&book.items);
        let has_claims = plan
            .new
            .iter()
            .chain(&plan.back)
            .filter_map(|imported| wanted(&book.items[imported.index]))
            .collect();
        let restores = plan
            .back
            .iter()
            .filter_map(|imported| wanted(&book.items[imported.index]))
            .collect();
        let grant = self
            .numbers
            .claim(has_claims, restores, high, now)
            .await
            .map_err(storage_refusal)?;
        let (waiting, freshly_used) = book.settle(grant, |item| self.may_number(item), now);
        let allocated = self
            .numbers
            .allocate_with_used(waiting.len(), freshly_used, highest(&book.items), now)
            .await
            .map_err(storage_refusal)?;
        for (index, number) in waiting.into_iter().zip(allocated) {
            assign(book.mark(index), number);
        }
        let answer = plan.answer(&book, backlog.skipped);
        let writer = self.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            book.save(&writer, now)?;
            compaction.finish(&writer, now)
        })
        .await
        .map_err(internal)??;
        Ok(answer)
    }
}

struct Change {
    original: u64,
    index: usize,
}

#[derive(Default)]
struct Plan {
    statuses: Vec<(&'static str, usize)>,
    already: Vec<u64>,
    skipped: Vec<String>,
    new: Vec<Change>,
    fresh: Vec<Change>,
    back: Vec<Change>,
}

impl Plan {
    fn prepare(
        book: &mut Book,
        owner: &str,
        backlog: Vec<Imported>,
        used: &[[u64; 2]],
        elsewhere: &BTreeMap<u64, String>,
        now: i64,
        by: &Value,
    ) -> Result<Self, Refusal> {
        let mut plan = Self::default();
        let same = |a: &str, b: &str| a.to_lowercase() == b.to_lowercase();
        let mut holders = BTreeMap::<_, Vec<_>>::new();
        for (index, item) in book.items.iter().enumerate() {
            for held in [number(item), wanted(item)].into_iter().flatten() {
                holders.entry(held).or_default().push(index);
            }
        }
        for task in backlog {
            let requested = task.number;
            let here = holders.get(&requested).map_or(&[][..], Vec::as_slice);
            let earlier = book.items.iter().position(|item| {
                imported_as(item) == Some(requested) && same(&item.title, &task.title)
            });
            if here
                .iter()
                .any(|index| same(&book.items[*index].title, &task.title))
                || earlier.is_some_and(|index| {
                    let item = &book.items[index];
                    item.removed.is_some() || number(item).is_none() || wanted(item).is_some()
                })
            {
                plan.already.push(requested);
                continue;
            }
            if let Some(item) = earlier
                .map(|index| &book.items[index])
                .filter(|item| flag(item, "stays"))
            {
                let previous = number(item).expect("previous numbered item");
                plan.skipped.push(format!("#{requested} (imported before as #{previous}; its own number couldn't be given back)"));
                continue;
            }
            let far = elsewhere.get(&requested);
            if far.is_some_and(|title| same(title, &task.title)) {
                plan.skipped.push(format!(
                    "#{requested} (already in the Work Queue on another of your computers)"
                ));
                continue;
            }
            let gone = here.is_empty() && contains(used, requested);
            if let Some(index) = earlier {
                let previous = number(&book.items[index]).expect("previous numbered item");
                let kept = here.iter().any(|index| {
                    let item = &book.items[*index];
                    !(number(item) == Some(requested)
                        && wanted(item).is_some_and(|number| number != requested))
                });
                if kept || far.is_some() || gone {
                    let place = holder(kept, far.is_some());
                    plan.skipped.push(format!("#{requested} (imported before as #{previous}; #{requested} is another item's{place})"));
                    continue;
                }
                book.mark(index)
                    .saved
                    .insert("wanted".into(), requested.into());
                holders.entry(requested).or_default().push(index);
                plan.back.push(Change {
                    original: previous,
                    index,
                });
                continue;
            }
            let known = !here.is_empty() || far.is_some() || gone;
            let mut bytes = [0; 15];
            getrandom::fill(&mut bytes).map_err(internal)?;
            let id = format!("wqi_{}", kuvo_wire::keys::encode(&bytes));
            let stamp = json!({"at":now,"by":by});
            let order = place(&book.items, None, task.status, false);
            let mut row = json!({
                "id":id, "owner":owner, "title":task.title, "body":task.body, "status":task.status,
                "order":order, "labels":task.label.into_iter().collect::<Vec<_>>(), "notes":[],
                "files":[], "links":[], "sessions":[], "created":stamp, "updated":stamp,
            });
            if status::closed(task.status) {
                row["doneAt"] = now.into();
            }
            if known {
                row["importedAs"] = requested.into();
                let place = holder(!here.is_empty(), far.is_some());
                row["notes"] = json!([{"at":now,"by":{"user":true},"text":format!("Imported as #{requested}, a number already given to another item{place}, so it has a new one.")}]);
            } else {
                row["wanted"] = requested.into();
            }
            let index = book.push(Item::read(owner, &row).expect("new import"));
            let change = Change {
                original: requested,
                index,
            };
            if known {
                plan.fresh.push(change);
            } else {
                holders.entry(requested).or_default().push(index);
                plan.new.push(change);
            }
            if let Some((_, count)) = plan.statuses.iter_mut().find(|(id, _)| *id == task.status) {
                *count += 1;
            } else {
                plan.statuses.push((task.status, 1));
            }
        }
        plan.skipped.truncate(MOST_SKIPPED);
        Ok(plan)
    }

    fn answer(self, book: &Book, skipped: Vec<String>) -> Value {
        let mut words = Vec::new();
        let added = self.new.len() + self.fresh.len();
        let waiting = self
            .new
            .iter()
            .filter(|change| number(&book.items[change.index]).is_none())
            .count();
        let mut renumbered = self
            .fresh
            .iter()
            .chain(
                self.new
                    .iter()
                    .filter(|change| imported_as(&book.items[change.index]).is_some()),
            )
            .collect::<Vec<_>>();
        renumbered.sort_by_key(|item| item.original);
        let mut renumbered = renumbered
            .into_iter()
            .map(|change| {
                let from = change.original;
                match number(&book.items[change.index]) {
                    Some(number) => format!("#{from} is #{number}"),
                    None => format!("#{from} waits for a new number"),
                }
            })
            .collect::<Vec<_>>();
        let mut restored = Vec::new();
        let mut restoring = Vec::new();
        for change in self.back {
            let item = &book.items[change.index];
            let from = change.original;
            if let Some(wanted) = wanted(item) {
                let promised = if flag(item, "backGranted") {
                    ", given: once the item that has it moves to its own"
                } else {
                    ""
                };
                restoring.push(format!("#{from} (for #{wanted}{promised})"));
            } else if let Some(imported) = imported_as(item) {
                let current = number(item).expect("previous numbered item");
                renumbered.push(format!("#{imported} stays #{current}"));
            } else if let Some(current) = number(item) {
                restored.push(format!("#{from} is #{current} again"));
            }
        }
        if added > 0 {
            let parts = self
                .statuses
                .iter()
                .map(|(id, count)| {
                    let name = STATUSES
                        .iter()
                        .find(|status| status.id == *id)
                        .map_or(*id, |status| status.name);
                    format!("{count} {name}")
                })
                .collect::<Vec<_>>();
            words.push(format!(
                "Imported {added} {} with their numbers: {}.",
                if added == 1 { "item" } else { "items" },
                parts.join(", ")
            ));
        } else if restored.is_empty() && restoring.is_empty() {
            words.push("Nothing was imported.".into());
        }
        match waiting {
            0 => {},
            1 => words.push("1 waits for the main computer to grant its number; until then it shows as new.".into()),
            many => words.push(format!("{many} wait for the main computer to grant their numbers; until then they show as new.")),
        }
        if !renumbered.is_empty() {
            words.push(format!("Renumbered, as their own number is another item's or couldn't be kept (each has a note saying why): {}.", renumbered.join(", ")));
        }
        if !restored.is_empty() {
            words.push(format!(
                "Given their own numbers back, as imported before: {}.",
                restored.join(", ")
            ));
        }
        match restoring.len() {
            0 => {},
            1 => words.push(format!("1 item imported before asks for its own number back, keeping the one it has until then: {}.", restoring.join(", "))),
            count => words.push(format!("{count} items imported before ask for their own number back, keeping the one they have until then: {}.", restoring.join(", "))),
        }
        let all = skipped
            .iter()
            .chain(&self.skipped)
            .cloned()
            .collect::<Vec<_>>();
        let left = all.len() + self.already.len();
        if left > 0 {
            let mut phrased = Vec::new();
            if !self.already.is_empty() {
                let range = runs(self.already.clone())
                    .into_iter()
                    .map(|[from, to]| {
                        if from == to {
                            format!("#{from}")
                        } else {
                            format!("#{from}–{to}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                phrased.push(format!("{range} (already in the Work Queue here)"));
            }
            phrased.extend(all.iter().take(MOST_SKIPPED).cloned());
            let more = if all.len() > MOST_SKIPPED {
                "; …"
            } else {
                ""
            };
            words.push(format!("Left out ({left}): {}{more}.", phrased.join("; ")));
        }
        let skipped = skipped
            .into_iter()
            .chain(
                self.already
                    .into_iter()
                    .map(|number| format!("#{number} (already in the Work Queue here)")),
            )
            .chain(self.skipped)
            .collect::<Vec<_>>();
        json!({
            "added": added, "byStatus": self.statuses.into_iter().collect::<BTreeMap<_,_>>(),
            "waiting":waiting,"skipped":skipped,"said":words.join(" "),
        })
    }
}

fn holder(here: bool, far: bool) -> &'static str {
    match (here, far) {
        (true, _) => " here",
        (_, true) => " on another of your computers",
        _ => ", one no longer here",
    }
}
