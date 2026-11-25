//! Spend persisted number leases and install the main computer's certified ledger.

mod claims;

pub(super) use claims::{Grant, contains, runs};

use super::store::{self, Error, Store};
use kuvo_peers::setup_lists::Reader;
use kuvo_setup::{QueueNumberLease, QueueSetup, SetupHandle};
use kuvo_store::{
    files,
    format::{Fields, ObjectFormat},
};
use serde_json::{Value, json};
use std::{io, path::PathBuf, time::Duration};

const LEDGER: ObjectFormat = ObjectFormat::additive("Work Queue number ledger");
const MAX: u64 = 999_999;
const BLOCK: u64 = 50;
const LOW: u64 = 15;
const MOST_RANGES: usize = 400;

#[derive(Clone)]
pub(super) struct Numbers {
    file: PathBuf,
    setup: SetupHandle,
    peers: Reader,
}

pub(super) fn check(store: &Store) -> Result<(), Error> {
    LEDGER.read(&store.root.join("numbers.json"))?;
    Ok(())
}

impl Numbers {
    pub fn new(store: &Store, setup: SetupHandle, peers: Reader) -> Self {
        Self {
            file: store.root.join("numbers.json"),
            setup,
            peers,
        }
    }

    pub async fn allocate(
        &self,
        count: usize,
        local_max: u64,
        now: i64,
    ) -> Result<Vec<u64>, Error> {
        self.allocate_with_used(count, Vec::new(), local_max, now)
            .await
    }

    pub async fn allocate_with_used(
        &self,
        count: usize,
        used: Vec<u64>,
        local_max: u64,
        now: i64,
    ) -> Result<Vec<u64>, Error> {
        if count == 0 && used.is_empty() {
            return Ok(Vec::new());
        }
        self.change(local_max, move |ledger, scope| {
            let allocated = if count == 0 {
                Vec::new()
            } else {
                ledger.allocate(scope, count, now)?
            };
            ledger.note_used(used);
            Ok(allocated)
        })
        .await
    }

    pub async fn ask_to_send(
        &self,
        local_max: u64,
        now: i64,
    ) -> Result<Option<(String, Value)>, Error> {
        let file = self.file.clone();
        if !tokio::task::spawn_blocking(move || file.try_exists())
            .await
            .map_err(io::Error::other)??
        {
            return Ok(None);
        }
        self.change(local_max, move |ledger, scope| {
            let Some(setup) = scope
                .setup
                .as_ref()
                .filter(|setup| !setup.main_here && !setup.merging)
            else {
                return Ok(None);
            };
            let top = ledger.top();
            let Some(asking) = ledger
                .fields
                .get_mut("asking")
                .filter(|asking| valid_ask(asking))
            else {
                return Ok(None);
            };
            if asking["sent"]
                .as_i64()
                .is_some_and(|sent| now.saturating_sub(sent) < 2_000)
            {
                return Ok(None);
            }
            asking["sent"] = now.into();
            let mut sent = json!({"id":asking["id"],"count":asking["count"]});
            for field in ["claims", "restores"] {
                if asking[field]
                    .as_array()
                    .is_some_and(|list| !list.is_empty())
                {
                    sent[field] = asking[field].clone();
                    if field == "claims" {
                        sent["own"] = true.into();
                    }
                }
            }
            if top > 0 {
                sent["top"] = top.into();
            }
            Ok(Some((setup.primary.clone(), sent)))
        })
        .await
    }

    pub async fn give(
        &self,
        by: String,
        ask: Value,
        local_max: u64,
        now: i64,
    ) -> Result<Option<Value>, Error> {
        let peers = LegacyPeers {
            store: Store {
                root: self
                    .file
                    .parent()
                    .expect("number ledger under queue")
                    .to_owned(),
            },
            keys: self
                .peers
                .view()
                .roster
                .members
                .iter()
                .map(|peer| kuvo_wire::keys::encode(&peer.key))
                .collect(),
        };
        self.change(local_max, move |ledger, scope| {
            ledger.give(scope, &by, &ask, now, Some(&peers))
        })
        .await
    }

    pub async fn accept(
        &self,
        by: String,
        block: Value,
        local_max: u64,
    ) -> Result<Option<Grant>, Error> {
        self.change(local_max, move |ledger, scope| {
            if !scope
                .setup
                .as_ref()
                .is_some_and(|setup| !setup.main_here && setup.primary == by && !setup.merging)
            {
                return Ok(None);
            }
            Ok(ledger.receive_block(&block))
        })
        .await
    }

    pub async fn answer_own(&self, local_max: u64, now: i64) -> Result<Option<Grant>, Error> {
        let Some(setup) = self.setup.queue_setup().filter(|setup| setup.main_here) else {
            return Ok(None);
        };
        self.change(local_max, move |ledger, scope| {
            let Some(mut ask) = ledger
                .fields
                .get("asking")
                .filter(|ask| valid_ask(ask))
                .cloned()
            else {
                return Ok(None);
            };
            ask["own"] = true.into();
            let reply = ledger.give(scope, &setup.primary, &ask, now, None)?;
            Ok(reply.as_ref().and_then(|block| ledger.receive_block(block)))
        })
        .await
    }

    async fn change<T: Send + 'static>(
        &self,
        local_max: u64,
        change: impl FnOnce(&mut Ledger, Scope) -> io::Result<T> + Send + 'static,
    ) -> Result<T, Error> {
        let lock = self.file.with_file_name(".numbers.json.lock");
        let guard =
            tokio::task::spawn_blocking(move || store::exclusive(&lock, Duration::from_secs(3)))
                .await
                .map_err(io::Error::other)??;
        let setup = self.setup.queue_setup();
        let lease = if setup.as_ref().is_some_and(|setup| setup.main_here) {
            self.setup
                .queue_number_lease()
                .await
                .map_err(io::Error::other)?
        } else {
            None
        };
        let independent = setup.is_none() && self.peers.view().roster.members.is_empty();
        let scope = Scope {
            setup,
            lease,
            independent,
        };
        let file = self.file.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            let stored = LEDGER.read(&file)?;
            let legacy = stored.as_ref().is_some_and(|record| record.version == 0);
            let fields = stored.map_or_else(Fields::new, |record| record.fields);
            let mut ledger = Ledger::read(fields.clone(), local_max);
            let result = change(&mut ledger, scope)?;
            if legacy || ledger.fields != fields {
                let through = file
                    .parent()
                    .and_then(std::path::Path::parent)
                    .ok_or_else(|| io::Error::other("Work Queue ledger has no run folder"))?;
                let bytes =
                    serde_json::to_vec(&LEDGER.encode(ledger.fields)).map_err(io::Error::other)?;
                files::write_atomic_through(&file, &bytes, through)?;
            }
            Ok(result)
        })
        .await
        .map_err(io::Error::other)?
    }
}

struct LegacyPeers {
    store: Store,
    keys: Vec<String>,
}

struct Scope {
    setup: Option<QueueSetup>,
    lease: Option<QueueNumberLease>,
    independent: bool,
}

impl Scope {
    fn main(&self) -> bool {
        self.independent || self.setup.as_ref().is_some_and(|setup| setup.main_here)
    }
}

struct Ledger {
    fields: Fields,
    fallback_next: u64,
    leases: Vec<[u64; 2]>,
    used: Vec<[u64; 2]>,
}

impl Ledger {
    fn read(fields: Fields, local_max: u64) -> Self {
        let leases = ranges(fields.get("leases"));
        let used = ranges(fields.get("used"));
        Self {
            fields,
            fallback_next: local_max.saturating_add(1),
            leases,
            used,
        }
    }

    fn allocate(&mut self, scope: Scope, count: usize, now: i64) -> io::Result<Vec<u64>> {
        let setup = scope.setup.as_ref();
        if setup.is_some_and(|setup| setup.merging) {
            return Ok(Vec::new());
        }
        let main = scope.main();
        let primary = if main {
            Some("self")
        } else {
            setup.map(|setup| setup.primary.as_str())
        };
        if let Some(primary) = primary {
            let previous = self.fields.get("primary").and_then(Value::as_str);
            if previous != Some(primary) {
                if previous == Some("self") {
                    self.ask(now)?;
                }
                self.fields.insert("primary".into(), primary.into());
            }
            self.fields.remove("through");
        }
        let before = self.leases.clone();
        let mut allocated = Vec::new();
        let mut used = Vec::new();
        for _ in 0..count {
            if self.leases.is_empty() && main {
                let ceiling = self.ceiling(setup, scope.lease);
                if let Some(ceiling) = ceiling {
                    let from = self.next();
                    let to = from.saturating_add(BLOCK - 1).min(ceiling);
                    if from <= to {
                        self.fields.insert("next".into(), (to + 1).into());
                        self.leases.push([from, to]);
                    }
                }
            }
            let Some(range) = self.leases.first_mut() else {
                break;
            };
            let number = range[0];
            if number == range[1] {
                self.leases.remove(0);
            } else {
                range[0] += 1;
            }
            allocated.push(number);
            used.push(number);
        }
        if self.leases != before {
            self.fields.insert("leases".into(), json!(self.leases));
        }
        self.note_used(used);
        let left: u64 = self.leases.iter().map(|[from, to]| to - from + 1).sum();
        if allocated.len() != count || (!main && left <= LOW) {
            self.ask(now)?;
        }
        Ok(allocated)
    }

    fn top(&self) -> u64 {
        self.next()
            .saturating_sub(1)
            .max(self.fallback_next.saturating_sub(1))
            .max(
                self.leases
                    .iter()
                    .chain(&self.used)
                    .map(|range| range[1])
                    .max()
                    .unwrap_or(0),
            )
    }

    fn give(
        &mut self,
        scope: Scope,
        by: &str,
        ask: &Value,
        now: i64,
        peers: Option<&LegacyPeers>,
    ) -> io::Result<Option<Value>> {
        let Some(setup) = scope
            .setup
            .as_ref()
            .filter(|setup| setup.main_here && !setup.merging)
        else {
            return Ok(None);
        };
        let Some(ceiling) = self.ceiling(Some(setup), scope.lease) else {
            return Ok(None);
        };
        let Some(id) = ask["id"].as_str().filter(|id| {
            id.strip_prefix("wqn_").is_some_and(|nonce| {
                (8..=40).contains(&nonce.len())
                    && nonce
                        .bytes()
                        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'-'))
            })
        }) else {
            return Ok(None);
        };
        if let Some(saved) = self.fields.get("given").and_then(|given| given.get(id)) {
            if saved
                .get("by")
                .is_some_and(|owner| owner.as_str() != Some(by))
            {
                return Ok(None);
            }
            return Ok(Some(
                json!({"id":id, "from":saved["from"], "to":saved["to"],
                "granted":saved.get("granted").cloned().unwrap_or_else(|| json!([]))}),
            ));
        }
        let count = integer(&ask["count"]).map_or(BLOCK, |count| count.clamp(1, 200));
        let top = integer(&ask["top"])
            .filter(|top| (1..=MAX).contains(top))
            .map_or(0, |top| top.min(MAX - 4 * BLOCK));
        let next = self.next().max(top + 1);
        self.fields.insert("next".into(), next.into());
        let claims = ranges(ask.get("claims"));
        if !self.claim_room(&claims, count, ceiling) {
            return Ok(None);
        }
        let own = ask["own"] == true;
        let legacy = if let Some(peers) = peers.filter(|_| own) {
            self.legacy_blocks(by, &intersect(&ranges(ask.get("restores")), &claims), peers)?
        } else {
            Vec::new()
        };
        let offered = self.grant_claims(by, &claims, own, &legacy, ceiling - count);
        let from = self.next();
        let to = from + count - 1;
        self.fields.insert("next".into(), (to + 1).into());
        let mut given = self
            .fields
            .get("given")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        given.insert(
            id.into(),
            json!({"from":from,"to":to,"granted":offered,"at":now,"by":by}),
        );
        let mut ordered: Vec<_> = given.into_iter().collect();
        ordered.sort_by_key(|(_, block)| block["at"].as_i64().unwrap_or_default());
        ordered.drain(..ordered.len().saturating_sub(500));
        self.fields
            .insert("given".into(), Value::Object(ordered.into_iter().collect()));
        Ok(Some(json!({"id":id,"from":from,"to":to,"granted":offered})))
    }

    fn receive_block(&mut self, block: &Value) -> Option<Grant> {
        let id = block["id"].as_str()?;
        let from = integer(&block["from"])?;
        let to = integer(&block["to"])?;
        let asked = self.fields.get("asking")?;
        if from == 0 || to < from || to > MAX || to - from > 1000 || asked["id"] != id {
            return None;
        }
        let answer = claims::grant(asked, block);
        self.fields.remove("asking");
        self.leases.push([from, to]);
        self.leases = joined(std::mem::take(&mut self.leases));
        self.fields.insert("leases".into(), json!(self.leases));
        Some(answer)
    }

    fn next(&self) -> u64 {
        self.fields
            .get("next")
            .and_then(integer)
            .filter(|n| *n > 0)
            .unwrap_or(self.fallback_next)
    }

    fn ceiling(
        &mut self,
        setup: Option<&QueueSetup>,
        lease: Option<QueueNumberLease>,
    ) -> Option<u64> {
        let Some(setup) = setup else { return Some(MAX) };
        let lease = lease?;
        if let Some(allocation) = &setup.allocation {
            let stamp = format!("{}:{}", setup.id, setup.epoch);
            if self.fields.get("leadership").and_then(Value::as_str) != Some(&stamp)
                || self.fields.get("allocation").and_then(integer) != Some(allocation.sequence)
            {
                let high = self
                    .leases
                    .iter()
                    .chain(&self.used)
                    .map(|range| range[1])
                    .max()
                    .unwrap_or(0);
                let next = self
                    .next()
                    .max(self.fallback_next)
                    .max(high + 1)
                    .max(allocation.fields.get("next").and_then(integer).unwrap_or(1));
                self.fields.insert("next".into(), next.into());
                for field in ["given", "granted"] {
                    if let Some(value) = allocation
                        .fields
                        .get(field)
                        .filter(|value| value.is_object())
                    {
                        self.fields.insert(field.into(), value.clone());
                    } else {
                        self.fields.remove(field);
                    }
                }
                if let Some(granted) = self
                    .fields
                    .get_mut("granted")
                    .and_then(Value::as_object_mut)
                    && let Some(own) = granted.remove(&setup.primary)
                {
                    granted.insert("self".into(), own);
                }
                self.fields.insert("leadership".into(), stamp.into());
                self.fields
                    .insert("allocation".into(), allocation.sequence.into());
            }
        }
        self.fields.insert(
            "next".into(),
            self.next().max(lease.floor.saturating_add(1)).into(),
        );
        Some(MAX.min(lease.high))
    }

    fn ask(&mut self, now: i64) -> io::Result<()> {
        let valid = self.fields.get("asking").is_some_and(valid_ask);
        if !valid {
            let id = ask_id()?;
            self.fields
                .insert("asking".into(), json!({"id":id,"count":BLOCK,"since":now}));
        }
        Ok(())
    }
}

fn ask_id() -> io::Result<String> {
    let mut bytes = [0; 12];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    Ok(format!("wqn_{}", kuvo_wire::keys::encode(&bytes)))
}

fn integer(value: &Value) -> Option<u64> {
    let number = value.as_f64()?;
    (number.fract() == 0.0 && (0.0..=9_007_199_254_740_991.0).contains(&number))
        .then_some(number as u64)
}

fn ranges(value: Option<&Value>) -> Vec<[u64; 2]> {
    store::array(value)
        .iter()
        .filter_map(|value| {
            let range = value.as_array()?;
            let [from, to] = range.as_slice() else {
                return None;
            };
            let (from, to) = (integer(from)?, integer(to)?);
            (from > 0 && from <= to && to <= MAX).then_some([from, to])
        })
        .rev()
        .take(MOST_RANGES)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn joined(mut given: Vec<[u64; 2]>) -> Vec<[u64; 2]> {
    given.sort_by_key(|range| range[0]);
    let mut joined: Vec<[u64; 2]> = Vec::new();
    for [from, to] in given {
        if let Some(last) = joined.last_mut()
            && from <= last[1] + 1
        {
            last[1] = last[1].max(to);
        } else {
            joined.push([from, to]);
        }
    }
    joined.drain(..joined.len().saturating_sub(MOST_RANGES));
    joined
}

fn valid_ask(ask: &Value) -> bool {
    ask["id"].is_string() && ask["count"].is_number() && ask["since"].is_number()
}

fn intersect(left: &[[u64; 2]], right: &[[u64; 2]]) -> Vec<[u64; 2]> {
    joined(
        left.iter()
            .flat_map(|[from, to]| {
                right.iter().filter_map(move |[first, last]| {
                    let (first, last) = ((*from).max(*first), (*to).min(*last));
                    (first <= last).then_some([first, last])
                })
            })
            .collect(),
    )
}
