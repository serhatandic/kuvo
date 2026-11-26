//! Freshness, batching and disk compatibility with the Work Queue's GitHub cache.

use super::{CACHE, command, hold::Hold};
use crate::queue::{
    refs::References,
    store::{self, Error, Store, limit, plain_name},
};
use kuvo_store::files;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::{AtomicI64, Ordering}},
    time::Duration,
};
use tokio::sync::watch;

const BACK_OFF: i64 = 300_000;
const FRESH_OPEN: i64 = 60_000;
const FRESH_DONE: i64 = 30 * 60_000;
const RETAIN: i64 = 30 * 86_400_000;
const PER_QUERY: usize = 50;
const MOST_READ: usize = 200;
pub(in crate::queue) const WAIT: Duration = Duration::from_secs(3);

#[derive(Clone)]
pub(in crate::queue) struct Reader {
    file: Arc<PathBuf>,
    running: Arc<Mutex<HashMap<String, watch::Receiver<bool>>>>,
    changed: watch::Sender<u64>,
    retry_until: Arc<AtomicI64>,
}

#[derive(Clone)]
struct Request {
    key: String,
    host: &'static str,
    owner: String,
    repo: String,
    number: i32,
    id: Option<String>,
    known: Option<String>,
}

#[derive(Default, Deserialize, Serialize)]
struct Cache {
    reads: BTreeMap<String, Reading>,
    hosts: BTreeMap<String, i64>,
}

#[derive(Clone, Deserialize, Serialize)]
struct Reading {
    at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checked: Option<i64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    gone: bool,
    #[serde(default, alias = "pull")]
    value: Option<Value>,
}

impl Reader {
    pub fn new(store: &Store) -> Self {
        Self {
            file: Arc::new(store.root.join("seen.json")),
            running: Arc::new(Mutex::new(HashMap::new())),
            changed: watch::channel(0).0,
            retry_until: Arc::new(AtomicI64::new(0)),
        }
    }

    pub fn changes(&self) -> watch::Receiver<u64> { self.changed.subscribe() }

    /// An agent asking for a specific issue gets the full description and may retry a backed-off host.
    pub async fn read_issue(
        &self,
        issue: &crate::queue::refs::Issue<'_>,
        computer: &str,
    ) -> Result<(Value, String), String> {
        let ask = Request {
            key: issue.key(),
            host: "github.com",
            owner: issue.owner.into(),
            repo: issue.repo.into(),
            number: issue.number,
            id: None,
            known: None,
        };
        let file = self.file.clone();
        let mut hold = None;
        for round in 0..=50 {
            let file = file.clone();
            hold = tokio::task::spawn_blocking(move || {
                Hold::take(file.with_file_name("seen.json.lock"))
            })
            .await
            .ok()
            .and_then(Result::ok)
            .flatten();
            if hold.is_some() || round == 50 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        let reply = command::ask(ask.host, query(std::slice::from_ref(&ask), true)).await;
        let ref_text = format!("gh:{}/{}#{}", ask.owner, ask.repo, ask.number);
        let found = match &reply {
            Ok(answer) if !answer.held.is_empty() => {
                let why = answer
                    .said
                    .as_ref()
                    .filter(|said| !said.is_empty())
                    .map_or_else(|| ".".into(), |said| format!(": {said}"));
                Err(format!(
                    "GitHub wouldn't answer for {ref_text} to gh on {computer}{why}"
                ))
            }
            Ok(answer) => {
                let value = value_at(answer, 0, &ask).flatten();
                if let Some(value) = value {
                    let body = answer
                        .data
                        .get("p0")
                        .and_then(|data| data["issueOrPullRequest"]["body"].as_str())
                        .unwrap_or_default()
                        .to_owned();
                    Ok((value, body))
                } else {
                    Err(format!(
                        "GitHub has no issue or pull request {ref_text} that gh on {computer} may see (gh auth status says which account it uses)."
                    ))
                }
            }
            Err(unread) => Err(unread.words(computer)),
        };
        let _ = tokio::task::spawn_blocking(move || {
            let _hold = hold;
            let mut kept = Cache::read(&file)?;
            let instant = now();
            match reply {
                Err(_) => {
                    kept.hosts
                        .entry(ask.host.into())
                        .and_modify(|before| *before = (*before).max(instant + BACK_OFF))
                        .or_insert(instant + BACK_OFF);
                }
                Ok(answer) => {
                    kept.hosts.remove(ask.host);
                    if answer.held.is_empty() {
                        let value = value_at(&answer, 0, &ask).flatten();
                        let reading = Reading::seen(kept.reads.get(&ask.key), value, instant);
                        kept.reads.insert(ask.key, reading);
                    }
                }
            }
            kept.keep(&file, instant)
        })
        .await;
        found
    }

    pub async fn read_seen(&self, refs: Arc<References>, items: Vec<Value>, wait: Duration) -> Option<i64> {
        let reader = self.clone();
        let following = wait.is_zero();
        let wishes = tokio::task::spawn_blocking(move || reader.requests(&refs, &items, following)).await;
        let Ok(Ok((wishes, next))) = wishes else { return None };
        let mut new = Vec::new();
        let mut watched = Vec::new();
        let (done, watcher) = watch::channel(false);
        {
            let mut running = self.running.lock().expect("GitHub reads");
            for request in wishes {
                if let Some(prior) = running.get(&request.key) {
                    watched.push(prior.clone());
                } else {
                    running.insert(request.key.clone(), watcher.clone());
                    new.push(request);
                }
            }
        }
        if !new.is_empty() {
            let reader = self.clone();
            watched.push(watcher);
            tokio::spawn(async move {
                let keys: Vec<_> = new.iter().map(|request| request.key.clone()).collect();
                if !matches!(reader.read_due(new).await, Ok(true)) {
                    // A competing disk owner or a transient read failure is a
                    // retry of real visible work, not a periodic page scan.
                    reader.retry_until.store(now().saturating_add(2_000), Ordering::Relaxed);
                }
                let mut running = reader.running.lock().expect("GitHub reads");
                for id in keys {
                    running.remove(&id);
                }
                let _ = done.send(true);
                reader.changed.send_modify(|revision| *revision = revision.wrapping_add(1));
            });
        }
        if !watched.is_empty() && !wait.is_zero() {
            let _ = tokio::time::timeout(wait, async move {
                for mut watch in watched {
                    let _ = watch.wait_for(|done| *done).await;
                }
            })
            .await;
        }
        next
    }

    fn requests(&self, refs: &References, items: &[Value], following: bool) -> Result<(Vec<Request>, Option<i64>), Error> {
        let cache = Cache::read(&self.file)?;
        let now = now();
        let mut requests = Vec::new();
        let mut unique = HashSet::new();
        let mut next = None;
        let mut deadline = |at: i64| {
            next = Some(next.map_or(at, |before: i64| before.min(at)));
        };
        for item in items.iter().filter(|item| item["removed"].is_null()) {
            let source = &item["source"];
            let Some(issue) = source["ref"].as_str().and_then(|name| refs.issue(name)) else {
                continue;
            };
            let seen = &item["seen"];
            if let Some(checked) = seen["checked"].as_f64().or_else(|| seen["at"].as_f64())
                && seen["gone"] != true
            {
                let fresh = if done(&seen["state"]) { FRESH_DONE } else { BACK_OFF };
                let at = (checked + fresh as f64).ceil() as i64;
                if now < at {
                    deadline(at);
                    continue;
                }
            }
            let key = issue.key();
            if !unique.insert(key.clone()) {
                continue;
            }
            let saved = cache.reads.get(&key);
            let mut due = saved.map_or(0, |row| row.asked().saturating_add(row.fresh_for()))
                .max(cache.hosts.get("github.com").copied().unwrap_or_default());
            // Only a passive follow needs protection against a completion/retry
            // loop. A new explicit read must be allowed to claim a freed disk lock.
            if following {
                due = due.max(self.retry_until.load(Ordering::Relaxed));
            }
            if due > now {
                deadline(due);
                continue;
            }
            let latest = saved.and_then(|row| row.value.as_ref());
            let there = latest
                .and_then(|row| row["url"].as_str())
                .or_else(|| saved.is_none().then(|| source["url"].as_str()).flatten());
            let moved = there
                .and_then(|url| refs.issue(url))
                .is_some_and(|there| there.key() != key);
            let known = latest
                .and_then(|row| row["id"].as_str())
                .or_else(|| source["id"].as_str())
                .map(str::to_owned);
            let gone =
                saved.is_some_and(|row| row.gone) || (saved.is_none() && seen["gone"] == true);
            let id = if gone || moved { known.clone() } else { None };
            requests.push(Request {
                key,
                host: "github.com",
                owner: issue.owner.into(),
                repo: issue.repo.into(),
                number: issue.number,
                id,
                known,
            });
        }
        requests.sort_by_key(|request| cache.reads.get(&request.key).map_or(0, Reading::asked));
        requests.truncate(MOST_READ);
        Ok((requests, next))
    }

    async fn read_due(&self, asks: Vec<Request>) -> Result<bool, Error> {
        let file = self.file.clone();
        let start = tokio::task::spawn_blocking(move || {
            let old = Cache::read(&file)?;
            if !asks.iter().any(|ask| old.due(ask, now())) {
                return Ok::<_, Error>(None);
            }
            let Some(hold) = Hold::take(file.with_file_name("seen.json.lock"))? else {
                return Ok(None);
            };
            let cache = Cache::read(&file)?;
            Ok(Some((hold, cache, asks)))
        })
        .await
        .map_err(io::Error::other)??;
        let Some((hold, cache, asks)) = start else {
            return Ok(false);
        };
        let mut hosts = BTreeMap::<&str, Vec<Request>>::new();
        for ask in asks.into_iter().filter(|ask| cache.due(ask, now())) {
            hosts.entry(ask.host).or_default().push(ask);
        }
        let mut read = BTreeMap::new();
        let mut blocked = BTreeMap::new();
        for (host, asks) in hosts {
            for part in asks.chunks(PER_QUERY) {
                hold.touch();
                let Ok(answer) = command::ask(host, query(part, false)).await else {
                    blocked.insert(host.to_owned(), now() + BACK_OFF);
                    break;
                };
                let mut values = BTreeMap::new();
                for (index, ask) in part.iter().enumerate() {
                    if let Some(value) = value_at(&answer, index, ask) {
                        values.insert(ask.key.clone(), value);
                    } else {
                        read.insert(
                            ask.key.clone(),
                            Reading::withheld(cache.reads.get(&ask.key), now()),
                        );
                    }
                }
                let again: Vec<_> = part
                    .iter()
                    .filter_map(|ask| {
                        if matches!(values.get(&ask.key), Some(None)) && ask.id.is_none() {
                            let id = cache
                                .reads
                                .get(&ask.key)
                                .and_then(|row| row.value.as_ref())
                                .and_then(|row| row["id"].as_str())
                                .map(str::to_owned)
                                .or_else(|| ask.known.clone())?;
                            let mut ask = ask.clone();
                            ask.id = Some(id);
                            return Some(ask);
                        }
                        None
                    })
                    .collect();
                let mut unread = false;
                if !again.is_empty() {
                    match command::ask(host, query(&again, false)).await {
                        Ok(answer) => {
                            for (index, ask) in again.iter().enumerate() {
                                match value_at(&answer, index, ask) {
                                    Some(Some(value)) => {
                                        values.insert(ask.key.clone(), Some(value));
                                    }
                                    Some(None) => {}
                                    None => {
                                        values.remove(&ask.key);
                                        read.insert(
                                            ask.key.clone(),
                                            Reading::withheld(cache.reads.get(&ask.key), now()),
                                        );
                                    }
                                }
                            }
                        }
                        Err(_) => {
                            unread = true;
                            blocked.insert(host.to_owned(), now() + BACK_OFF);
                            for ask in again {
                                values.remove(&ask.key);
                            }
                        }
                    }
                }
                for (key, value) in values {
                    let next = Reading::seen(cache.reads.get(&key), value, now());
                    read.insert(key, next);
                }
                if unread {
                    break;
                }
            }
        }
        let file = self.file.clone();
        tokio::task::spawn_blocking(move || {
            let _hold = hold;
            let mut latest = Cache::read(&file)?;
            latest.reads.extend(read);
            for (host, until) in blocked {
                latest
                    .hosts
                    .entry(host)
                    .and_modify(|before| *before = (*before).max(until))
                    .or_insert(until);
            }
            latest.keep(&file, now())
        })
        .await
        .map_err(io::Error::other)??;
        Ok(true)
    }
}

impl Cache {
    fn read(path: &Path) -> Result<Self, Error> {
        let fields = store::read(path, CACHE)?.unwrap_or_default();
        let reads = fields
            .get("reads")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter_map(|(key, row)| Some((key.clone(), serde_json::from_value(row.clone()).ok()?)))
            .collect();
        let hosts = fields
            .get("hosts")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter_map(|(host, until)| Some((host.clone(), until.as_i64()?)))
            .collect();
        Ok(Self { reads, hosts })
    }

    fn due(&self, ask: &Request, now: i64) -> bool {
        self.hosts.get(ask.host).is_none_or(|until| *until <= now)
            && self
                .reads
                .get(&ask.key)
                .is_none_or(|read| now - read.asked() >= read.fresh_for())
    }

    fn keep(mut self, path: &Path, now: i64) -> Result<(), Error> {
        self.reads.retain(|_, read| now - read.asked() <= RETAIN);
        self.hosts.retain(|_, until| *until > now);
        let fields = serde_json::to_value(self)
            .map_err(io::Error::other)?
            .as_object()
            .cloned()
            .expect("GitHub cache");
        let mut bytes = serde_json::to_vec(&CACHE.encode(fields)).map_err(io::Error::other)?;
        bytes.push(b'\n');
        files::write_atomic(path, &bytes)?;
        Ok(())
    }
}

impl Reading {
    fn asked(&self) -> i64 {
        self.checked.unwrap_or(self.at)
    }

    fn fresh_for(&self) -> i64 {
        if self.gone {
            return BACK_OFF;
        }
        match &self.value {
            None => BACK_OFF,
            Some(value) if done(&value["state"]) => FRESH_DONE,
            Some(_) => FRESH_OPEN,
        }
    }

    fn withheld(before: Option<&Self>, now: i64) -> Self {
        before.map_or_else(
            || Self {
                at: now,
                checked: None,
                gone: false,
                value: None,
            },
            |before| Self {
                checked: Some(now),
                ..before.clone()
            },
        )
    }

    fn seen(before: Option<&Self>, value: Option<Value>, now: i64) -> Self {
        if let Some(before) = before {
            if value.is_none() && before.value.is_some() {
                return Self {
                    at: if before.gone { before.at } else { now },
                    value: before.value.clone(),
                    checked: Some(now),
                    gone: true,
                };
            }
            if before.value == value && !before.gone {
                return Self {
                    at: before.at,
                    value,
                    checked: Some(now),
                    gone: false,
                };
            }
        }
        Self {
            at: now,
            value,
            checked: None,
            gone: false,
        }
    }
}

fn done(state: &Value) -> bool {
    matches!(
        state.as_str(),
        Some("completed" | "not_planned" | "merged" | "closed")
    )
}

fn now() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

fn value_at(answer: &command::Answered, index: usize, ask: &Request) -> Option<Option<Value>> {
    let alias = format!("p{index}");
    (!answer.held.contains(&alias)).then(|| {
        answer.data.get(&alias).and_then(|found| {
            let node = if ask.id.is_some() {
                found
            } else {
                &found["issueOrPullRequest"]
            };
            node_value(node)
        })
    })
}

fn node_value(node: &Value) -> Option<Value> {
    let id = node["id"].as_str()?;
    let number = node["number"].as_number()?;
    let title = limit(&plain_name(node["title"].as_str()?), 200);
    let url = node["url"]
        .as_str()
        .filter(|url| url.starts_with("https://github.com/"))?;
    let pull = match node["__typename"].as_str() {
        Some("PullRequest") => true,
        Some("Issue") => false,
        _ => return None,
    };
    let state = if pull {
        if node["merged"] == true || node["state"] == "MERGED" {
            "merged"
        } else if node["state"] == "CLOSED" {
            "closed"
        } else if node["isDraft"] == true {
            "draft"
        } else {
            "open"
        }
    } else if node["state"] == "CLOSED" {
        if matches!(
            node["stateReason"].as_str(),
            Some("NOT_PLANNED" | "DUPLICATE")
        ) {
            "not_planned"
        } else {
            "completed"
        }
    } else {
        "open"
    };
    let mut value = json!({
        "kind":if pull { "github-pr" } else { "github" },
        "id":limit(id,200),"number":number,
        "title":if title.is_empty() { format!("#{number}") } else { title },
        "url":limit(url, 2000),"state":state,
    });
    if let Some(at) = node["closedAt"]
        .as_str()
        .and_then(|text| text.parse::<jiff::Timestamp>().ok())
    {
        value["closedAt"] = at.as_millisecond().into();
    }
    Some(value)
}

fn query(asks: &[Request], body: bool) -> String {
    let mut query = String::from("query { ");
    let body = if body { " body" } else { "" };
    let fields = format!(
        "__typename ... on Issue {{ id number title url state stateReason closedAt{body} }} ... on PullRequest {{ id number title url state isDraft merged closedAt{body} }}"
    );
    for (index, ask) in asks.iter().enumerate() {
        use std::fmt::Write;
        if let Some(id) = &ask.id {
            let _ = write!(query, "p{index}: node(id: {}) {{ {fields} }} ", json!(id));
        } else {
            let _ = write!(
                query,
                "p{index}: repository(owner: {}, name: {}) {{ issueOrPullRequest(number: {}) {{ {fields} }} }} ",
                json!(ask.owner),
                json!(ask.repo),
                ask.number
            );
        }
    }
    query.push('}');
    query
}
