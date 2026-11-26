//! The limits portion of authenticated peer mail. Only account hashes and
//! readings cross this edge; no provider credential, target ID or sign-in mark.
use super::{
    limits::{Account, Fact, Provider},
    plan_facts::{Balance, Extra, Facts, Problem, ProblemKind},
    readings::{NoticeState, Source, Trail, Window, equivalent_trail},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub const AWAY: u64 = 25 * 60_000;
const SKEW: u64 = 60_000;
const MOST_ACCOUNTS: usize = 100;
const MOST_WINDOWS: usize = 30;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountMail {
    pub provider: String,
    pub service: Provider,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<Provider>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(flatten)]
    pub facts: Facts,
    pub windows: Vec<Window>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notices: Vec<Fact>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub notice_state: BTreeMap<String, NoticeState>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub agents_reset: bool,
}
impl AccountMail {
    pub fn account(&self) -> Account {
        Account {
            provider: self.provider.clone(),
            service: self.service.clone(),
            account: self.account.clone(),
            label: self.label.clone(),
            identity: self.identity.clone(),
            sign_in: String::new(),
            notice_card: None,
        }
    }
    pub fn row(&self, key: &str) -> String {
        self.account().row(key)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    pub at: u64,
    pub accounts: Vec<AccountMail>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ask: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refresh: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice_setup: Option<String>,
}
fn words(value: &Value, most: usize) -> Option<String> {
    let text = value.as_str()?;
    if text.is_empty() {
        return None;
    }
    let mut units = 0;
    Some(
        text.chars()
            .take_while(|ch| {
                units += ch.len_utf16();
                units <= most
            })
            .collect(),
    )
}
fn clock(at: u64, offset: i128, now: u64) -> u64 {
    (i128::from(at) + offset).clamp(0, i128::from(now)) as u64
}
fn time(value: &Value) -> Option<u64> {
    value.as_u64().filter(|n| *n > 0)
}
fn account(value: &Value, offset: i128, now: u64, notices: bool) -> Option<AccountMail> {
    let provider = words(&value["provider"], 200)?;
    let service = serde_json::from_value::<Provider>(value["service"].clone()).ok()?;
    let agent = serde_json::from_value::<Provider>(value["agent"].clone())
        .ok()
        .filter(|agent| {
            matches!(
                agent,
                Provider::Claude
                    | Provider::Codex
                    | Provider::Cursor
                    | Provider::Grok
                    | Provider::Antigravity
            )
        });
    let account = agent.as_ref().and_then(|_| words(&value["account"], 200));
    let identity = words(&value["identity"], 17).filter(|s| {
        s.len() == 16
            && s.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    });
    let windows = value["windows"]
        .as_array()
        .into_iter()
        .flatten()
        .take(MOST_WINDOWS)
        .filter_map(|w| {
            let used = w["used"]
                .as_f64()
                .filter(|n| n.is_finite())?
                .clamp(0.0, 1.0);
            let amount = serde_json::from_value::<super::plan_facts::Amount>(w["amount"].clone())
                .ok()
                .filter(|a| a.limit.is_finite() && a.limit > 0.0 && a.used.is_finite());
            Some(Window {
                id: words(&w["id"], 200)?,
                used,
                minutes: time(&w["minutes"]).and_then(|n| n.try_into().ok()),
                resets_at: time(&w["resetsAt"]),
                model: words(&w["model"], 200),
                name: words(&w["name"], 200),
                amount,
                read: clock(time(&w["read"])?, offset, now),
                source: if w["source"] == "turn" {
                    Source::Turn
                } else {
                    Source::Usage
                },
            })
        })
        .collect::<Vec<_>>();
    let extras = value["extras"]
        .as_array()
        .into_iter()
        .flatten()
        .take(10)
        .filter_map(|e| {
            if !e.is_object() {
                return None;
            }
            let mut e = e.clone();
            e["id"] = json!(words(&e["id"], 200)?);
            e["name"] = json!(words(&e["name"], 100)?);
            if e["kind"] == "resets" {
                e["usable"] = json!(e["usable"] == true);
                if let Some(count) = e["count"].as_f64().filter(|n| n.is_finite() && *n >= 0.0) {
                    e["count"] = json!(count.floor() as u32);
                }
                if let Some(a) = e.get_mut("expiries").and_then(Value::as_array_mut) {
                    a.retain(|n| time(n).is_some());
                    a.truncate(50);
                }
                if let Some(a) = e.get_mut("refills").and_then(Value::as_array_mut) {
                    *a = a
                        .iter()
                        .filter_map(|v| words(v, 40).map(Value::String))
                        .take(10)
                        .collect();
                }
            }
            if e["kind"] == "unreadable" {
                e["message"] = json!(words(&e["message"], 500)?);
            }
            let kind = words(&e["kind"], 40)?;
            if matches!(
                kind.as_str(),
                "spent" | "left" | "capped" | "resets" | "unreadable"
            ) {
                serde_json::from_value::<Extra>(e)
                    .ok()
                    .filter(|extra| match extra {
                        Extra::Future(_) => false,
                        Extra::Capped { limit, .. } => *limit > 0.,
                        _ => true,
                    })
            } else {
                let id = words(&e["id"], 200)?;
                let name = words(&e["name"], 100)?;
                let fields = e
                    .as_object()?
                    .iter()
                    .take(20)
                    .filter(|(key, _)| !matches!(key.as_str(), "id" | "name" | "kind"))
                    .filter_map(|(key, v)| {
                        if let Some(x) = v.as_str().filter(|s| s.encode_utf16().count() <= 200) {
                            Some((key.clone(), super::plan_facts::Plain::Text(x.into())))
                        } else if let Some(x) = v.as_number() {
                            Some((key.clone(), super::plan_facts::Plain::Number(x.clone())))
                        } else {
                            v.as_bool()
                                .map(|b| (key.clone(), super::plan_facts::Plain::Bool(b)))
                        }
                    })
                    .collect();
                let extra = Extra::Future(super::plan_facts::FutureExtra {
                    id,
                    name,
                    kind,
                    fields,
                });
                (serde_json::to_vec(&extra).ok()?.len() <= 1000).then_some(extra)
            }
        })
        .collect();
    let balance = serde_json::from_value::<Balance>(value["balance"].clone())
        .ok()
        .filter(|b| {
            b.left.is_finite()
                && b.limit.is_none_or(f64::is_finite)
                && matches!(
                    b.unit,
                    super::plan_facts::Unit::Usd | super::plan_facts::Unit::Credits
                )
        });
    let problem = serde_json::from_value::<Problem>(value["problem"].clone())
        .ok()
        .filter(|p| p.kind != ProblemKind::None && !p.message.is_empty())
        .map(|mut p| {
            p.message = words(&json!(p.message), 500).unwrap_or_default();
            p.until = p
                .until
                .map(|t| (i128::from(t) + offset).clamp(0, i128::from(u64::MAX)) as u64);
            p
        });
    let facts = Facts {
        partial: false,
        plan: words(&value["plan"], 100),
        balance,
        extras,
        problem,
        checked: time(&value["checked"]).map(|t| clock(t, offset, now)),
        values_read: time(&value["valuesRead"]).map(|t| clock(t, offset, now)),
        next: time(&value["next"])
            .map(|t| (i128::from(t) + offset).clamp(0, i128::from(u64::MAX)) as u64),
    };
    let notice_state = if notices {
        value["noticeState"]
            .as_object()
            .into_iter()
            .flatten()
            .take(MOST_WINDOWS)
            .filter_map(|(id, value)| {
                if id.is_empty() || id.encode_utf16().count() > 200 {
                    return None;
                }
                let mut state: NoticeState = serde_json::from_value(value.clone()).ok()?;
                state.read = state.read.map(|at| clock(at, offset, now));
                Some((id.clone(), state))
            })
            .collect()
    } else {
        BTreeMap::new()
    };
    let notices = if notices {
        value["notices"]
            .as_array()
            .into_iter()
            .flatten()
            .take(100)
            .filter_map(|n| {
                if !n.is_object() {
                    return None;
                }
                let mut n = n.clone();
                n["accountName"] =
                    json!(words(&n["accountName"], 100).unwrap_or_else(|| service.name().into()));
                n["windowName"] =
                    json!(words(&n["windowName"], 100).unwrap_or_else(|| "Limit".into()));
                let mut fact = serde_json::from_value::<Fact>(n).ok()?;
                if !(0.0..=1.0).contains(&fact.remaining)
                    || fact.id.strip_prefix("usage_").is_none_or(|id| {
                        id.is_empty()
                            || id.len() > 80
                            || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
                    })
                    || fact.row.encode_utf16().count() > 300
                    || fact.window.encode_utf16().count() > 200
                    || fact
                        .forwarder
                        .as_ref()
                        .is_some_and(|id| id.encode_utf16().count() > 200)
                    || fact.service.name() == "devin"
                    || fact.service != service
                {
                    return None;
                }
                fact.at = clock(fact.at, offset, now);
                (now.saturating_sub(fact.at) <= 600_000 && fact.resets_at.is_none_or(|at| at > now))
                    .then_some(fact)
            })
            .collect()
    } else {
        vec![]
    };
    Some(AccountMail {
        provider,
        service,
        agent,
        account,
        label: words(&value["label"], 100),
        identity,
        facts,
        windows,
        notices,
        notice_state,
        agents_reset: value["agentsReset"] == true
            || value["extras"].as_array().is_some_and(|extras| {
                extras
                    .iter()
                    .any(|e| e["kind"] == "resets" && e["agents"] == true)
            }),
    })
}
impl Reading {
    /// Decode at most one mail's admitted contents. Provider reset times are the
    /// maker's absolute times and must not move with a peer computer's clock.
    pub fn from(value: &Value, now: u64, setup: Option<&str>) -> Option<Self> {
        let accounts = value["accounts"].as_array()?;
        let at = time(&value["at"]).unwrap_or(now);
        let offset = if at.abs_diff(now) >= SKEW {
            i128::from(now) - i128::from(at)
        } else {
            0
        };
        let notice_setup = words(&value["noticeSetup"], 100).filter(|s| Some(s.as_str()) == setup);
        Some(Self {
            at,
            accounts: accounts
                .iter()
                .take(MOST_ACCOUNTS)
                .filter_map(|a| account(a, offset, now, notice_setup.is_some()))
                .collect(),
            ask: value["ask"] == true,
            refresh: value["refresh"]
                .as_array()
                .into_iter()
                .flatten()
                .take(200)
                .filter_map(|id| words(id, 200))
                .collect(),
            notice_setup,
        })
    }
    pub fn wire(&self) -> Value {
        let mut reading = self.clone();
        // Fit the relay envelope without dropping current readings due to a long notice history.
        while serde_json::to_vec(&reading).map_or(true, |v| v.len() > 28_000)
            && reading.accounts.iter().any(|a| !a.notices.is_empty())
        {
            if let Some(account) = reading.accounts.iter_mut().max_by_key(|a| a.notices.len()) {
                account.notices.remove(0);
            }
        }
        while serde_json::to_vec(&reading).map_or(true, |v| v.len() > 40_000)
            && !reading.accounts.is_empty()
        {
            reading.accounts.pop();
        }
        let mut value = serde_json::to_value(reading).unwrap_or_default();
        if let Some(accounts) = value["accounts"].as_array_mut() {
            for account in accounts {
                if account["agentsReset"] == true {
                    if let Some(extras) = account["extras"].as_array_mut() {
                        for extra in extras.iter_mut().filter(|e| e["kind"] == "resets") {
                            extra["agents"] = json!(true);
                        }
                    }
                }
            }
        }
        value
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Kept {
    pub name: String,
    pub heard: u64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "noticeSetup"
    )]
    pub setup: Option<String>,
    pub accounts: Vec<AccountMail>,
    #[serde(default)]
    pub trails: BTreeMap<String, BTreeMap<String, Trail>>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Book {
    pub peers: BTreeMap<String, Kept>,
    #[serde(default)]
    pub groups: BTreeMap<String, BTreeMap<String, Trail>>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(skip)]
    pub roster: BTreeMap<String, String>,
}
impl Book {
    /// A named admitted computer whose first reading has not arrived yet.
    pub(super) fn pending(&self) -> impl Iterator<Item = (&str, &str)> + '_ {
        self.roster
            .iter()
            .filter(|(id, _)| !self.peers.contains_key(*id))
            .map(|(id, name)| (id.as_str(), name.as_str()))
    }
    /// Only a successful maker observation from a currently admitted peer
    /// stands in for our own fetch. Borrowed local rows have no checked time.
    pub fn freshest(&self, account: &Account) -> Option<&AccountMail> {
        let identity = account.identity.as_ref()?;
        self.peers
            .iter()
            .filter(|(key, peer)| {
                self.roster.contains_key(*key) && self.scope.is_some() && peer.setup == self.scope
            })
            .flat_map(|(_, peer)| &peer.accounts)
            .filter(|other| {
                other.service == account.service
                    && other.identity.as_ref() == Some(identity)
                    && other.facts.problem.is_none()
                    && other.facts.checked.is_some_and(|at| at > 0)
            })
            .max_by_key(|other| other.facts.checked)
    }
    pub fn read(path: &Path) -> std::io::Result<Self> {
        let Some(bytes) = kuvo_store::files::read(path)? else {
            return Ok(Self::default());
        };
        if let Ok(mut book) = serde_json::from_slice::<Self>(&bytes) {
            for group in book.groups.values_mut() {
                group.retain(|_, trail| trail.valid_kept());
            }
            for peer in book.peers.values_mut() {
                for group in peer.trails.values_mut() {
                    group.retain(|_, trail| trail.valid_kept());
                }
                for account in &mut peer.accounts {
                    let count = account.windows.len();
                    account.windows.retain(Window::valid_kept);
                    if count != account.windows.len() {
                        account.facts.checked = None;
                        account.facts.values_read = None;
                        account.facts.next = None;
                    }
                }
            }
            // Earlier Rust caches keyed shared windows by the maker's ID. Reuse
            // their accepted evidence, not those labels, when opening them.
            for group in book.groups.values_mut() {
                if group
                    .iter()
                    .all(|(key, trail)| *key == trail.current.limit_key())
                {
                    continue;
                }
                let mut old = std::mem::take(group).into_values().collect::<Vec<_>>();
                old.sort_by_key(|trail| trail.current.read);
                for trail in old {
                    let key = trail.current.limit_key();
                    if let Some(current) = group.get_mut(&key) {
                        current.observe(trail.current.clone(), &Default::default());
                        current.merge_fired(&trail);
                    } else {
                        group.insert(key, trail);
                    }
                }
            }
            return Ok(book);
        }
        let old: Value = serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
        let mut book = Self::default();
        if let Some(peers) = old["peers"].as_object() {
            for (key, kept) in peers {
                let Some(heard) = kept["heard"].as_u64() else {
                    continue;
                };
                let name = kept["name"].as_str().unwrap_or("Another computer");
                let scope = kept["noticeSetup"].as_str();
                let raw = json!({"at":heard,"accounts":kept["accounts"],"noticeSetup":scope});
                if let Some(reading) = Reading::from(&raw, heard, scope) {
                    book.take(key, name, reading, heard);
                }
            }
        }
        Ok(book)
    }
    pub fn store(&self, path: &Path) -> std::io::Result<()> {
        kuvo_store::files::write_secret(
            path,
            &serde_json::to_vec(self).map_err(std::io::Error::other)?,
        )
    }
    pub fn roster(&mut self, names: &BTreeMap<String, String>, setup: Option<&str>) -> bool {
        let count = self.peers.len();
        let mut changed = self.roster != *names;
        self.roster = names.clone();
        if self.scope.as_deref() != setup {
            if self.scope.is_some() {
                self.groups.clear();
            }
            self.scope = setup.map(str::to_owned);
            changed = true;
        }
        self.peers.retain(|id, peer| {
            let Some(name) = names.get(id) else {
                return false;
            };
            if &peer.name != name {
                peer.name = name.clone();
                changed = true;
            }
            if peer.setup.as_deref() != setup {
                changed |= peer.setup.is_some()
                    || peer
                        .accounts
                        .iter()
                        .any(|a| !a.notices.is_empty() || !a.notice_state.is_empty());
                peer.setup = None;
                for account in &mut peer.accounts {
                    account.notices.clear();
                    account.notice_state.clear();
                }
                for trail in peer
                    .trails
                    .values_mut()
                    .flat_map(|group| group.values_mut())
                {
                    trail.clear_fired();
                }
            }
            true
        });
        changed || self.peers.len() != count
    }
    pub fn take(&mut self, key: &str, name: &str, mut reading: Reading, now: u64) -> Vec<Fact> {
        let prior = self.peers.remove(key);
        let seen = prior
            .as_ref()
            .into_iter()
            .flat_map(|p| p.accounts.iter())
            .flat_map(|a| a.notices.iter())
            .map(|n| n.id.clone())
            .collect::<BTreeSet<_>>();
        let mut trails = BTreeMap::new();
        let mut notices = vec![];
        for account in &mut reading.accounts {
            let row = account.row(key);
            let mut trail = prior
                .as_ref()
                .and_then(|p| p.trails.get(&row))
                .cloned()
                .unwrap_or_default();
            // Only complete current windows participate in the row. A false refill or an
            // older reset from a peer is the same rejected observation as a local one.
            if trail.is_empty() {
                for window in prior
                    .as_ref()
                    .into_iter()
                    .flat_map(|p| p.accounts.iter())
                    .filter(|a| a.row(key) == row)
                    .flat_map(|a| &a.windows)
                {
                    if let Some(t) = Trail::new(window.clone()) {
                        trail.insert(window.id.clone(), t);
                    }
                }
            }
            if prior
                .as_ref()
                .is_some_and(|old| old.setup != reading.notice_setup)
            {
                for trail in trail.values_mut() {
                    trail.clear_fired();
                }
            }
            let ids = account
                .windows
                .iter()
                .map(Window::limit_key)
                .collect::<BTreeSet<_>>();
            trail.retain(|_, t| ids.contains(&t.current.limit_key()));
            let mut aliases = BTreeMap::new();
            let mut order = vec![];
            for window in std::mem::take(&mut account.windows) {
                let given = window.id.clone();
                let old_id = equivalent_trail(&trail, &window);
                let mut current = old_id.as_ref().and_then(|id| trail.remove(id));
                if let Some(existing) = current.as_mut() {
                    existing.observe(window, &Default::default());
                } else {
                    current = Trail::new(window);
                }
                if let Some(current) = current {
                    let id = current.current.id.clone();
                    if let Some(old_id) = old_id {
                        if old_id != id {
                            if let Some(position) = order.iter().position(|old| old == &old_id) {
                                order[position] = id.clone();
                            }
                        }
                    }
                    if !order.contains(&id) {
                        order.push(id.clone());
                    }
                    aliases.insert(given, id.clone());
                    trail.insert(id, current);
                }
            }
            account.windows = order
                .iter()
                .filter_map(|id| trail.get(id).map(|t| t.current.clone()))
                .collect();
            account.notices.retain_mut(|n| {
                if let Some(id) = aliases.get(&n.window) {
                    n.window = id.clone();
                }
                n.row == row
                    && trail
                        .get(&n.window)
                        .is_some_and(|t| t.current.resets_at == n.resets_at)
            });
            let states = std::mem::take(&mut account.notice_state);
            for (id, state) in states {
                let id = aliases.get(&id).cloned().unwrap_or(id);
                if let Some(t) = trail.get_mut(&id) {
                    t.take_notice_state(&state);
                    account.notice_state.insert(id, t.notice_state());
                }
            }
            for notice in &account.notices {
                if let Some(t) = trail.get_mut(&notice.window) {
                    t.mark_fired_at(notice.milestone, notice.at);
                }
            }
            notices.extend(
                account
                    .notices
                    .iter()
                    .filter(|n| !seen.contains(&n.id))
                    .cloned(),
            );
            trails.insert(row, trail);
        }
        self.peers.insert(
            key.into(),
            Kept {
                name: name.into(),
                heard: now,
                setup: reading.notice_setup,
                accounts: reading.accounts,
                trails,
            },
        );
        notices
    }
    pub fn fresh(&self, service: Provider, identity: &str, now: u64) -> Option<u64> {
        self.peers
            .values()
            .filter(|p| now.saturating_sub(p.heard) <= AWAY)
            .flat_map(|p| p.accounts.iter())
            .filter(|a| {
                a.service == service
                    && a.identity.as_deref() == Some(identity)
                    && a.facts.problem.is_none()
            })
            .filter_map(|a| a.facts.checked)
            .max()
    }
}

#[cfg(test)]
mod parity_corpus {
    use super::*;
    const NOW: u64 = 1_800_000_000_000;
    fn raw(at: u64) -> Value {
        json!({"at":at,"noticeSetup":"set_fixture","accounts":[{"provider":"claude-code","service":"claude","identity":"abcdefabcdefabcd","checked":at,"next":at+600000,"windows":[{"id":"five_hour","used":0.35,"minutes":300,"resetsAt":NOW+3600000,"read":at,"source":"usage"}]}]})
    }
    #[test]
    fn reset_inventory_without_optional_arrays_survives_the_peer_wire() {
        let mut given = raw(NOW);
        let account = &mut given["accounts"][0];
        account["provider"] = json!("openai");
        account["service"] = json!("codex");
        account["extras"] = json!([{"kind":"resets","id":"resets","name":"Rate Limit Resets","count":1,"usable":true,"expiries":[NOW+60000]}]);
        let read = Reading::from(&given, NOW, Some("set_fixture")).unwrap();
        let extras = &read.accounts[0].facts.extras;
        assert!(
            matches!(extras.as_slice(),[Extra::Resets{count:1,usable:true,refills,..}] if refills.is_empty())
        );
        let sent = read.wire();
        assert!(sent["accounts"][0]["extras"][0].get("refills").is_none());
        assert_eq!(
            &Reading::from(&sent, NOW, Some("set_fixture"))
                .unwrap()
                .accounts[0]
                .facts
                .extras,
            extras
        );
    }
    #[test]
    fn peer_clock_skew_normalizes_checks_windows_and_hold_deadlines() {
        let ahead = NOW + 3_600_000;
        let mut given = raw(ahead);
        given["accounts"][0]["problem"] =
            json!({"kind":"rate-limited","message":"Fixture hold","until":ahead+600000});
        given["accounts"][0]["valuesRead"] = json!(ahead - 120000);
        let read = Reading::from(&given, NOW, Some("set_fixture")).unwrap();
        assert_eq!(read.accounts[0].windows[0].read, NOW);
        assert_eq!(read.accounts[0].facts.checked, Some(NOW));
        assert_eq!(read.accounts[0].facts.values_read, Some(NOW - 120000));
        assert_eq!(read.accounts[0].facts.next, Some(NOW + 600000));
        assert_eq!(
            read.accounts[0].facts.problem.as_ref().unwrap().until,
            Some(NOW + 600000)
        );
    }
    #[test]
    fn peer_books_keep_confirmed_accounts_across_restart_and_forget_removed_members() {
        let scratch = tempfile::tempdir().unwrap();
        let file = scratch.path().join("limits.json");
        let name = "Fixture".to_string();
        let key = "fixture-peer".to_string();
        let names = BTreeMap::from([(key.clone(), name.clone())]);
        let mut book = Book::default();
        book.roster(&names, Some("set_fixture"));
        let read = Reading::from(&raw(NOW), NOW, Some("set_fixture")).unwrap();
        let account = read.accounts[0].account();
        book.take(&key, &name, read, NOW);
        assert!(book.freshest(&account).is_some());
        book.store(&file).unwrap();
        let mut book = Book::read(&file).unwrap();
        assert!(book.freshest(&account).is_none());
        book.roster(&names, Some("set_fixture"));
        assert_eq!(book.freshest(&account).unwrap().windows[0].used, 0.35);
        assert_eq!(
            book.fresh(Provider::Claude, "abcdefabcdefabcd", NOW + AWAY + 1),
            None
        );
        book.roster(&BTreeMap::new(), Some("set_fixture"));
        assert!(book.peers.is_empty());
    }
    #[test]
    fn peer_mail_keeps_known_readings_and_only_bounded_plain_extra_fields() {
        let mut given = raw(NOW);
        let account = &mut given["accounts"][0];
        account["signIn"] = json!("fixture-secret-never-shared");
        account["identity"] = json!("abcdefabcdefabcd");
        account["extras"] = json!([{"kind":"spent","id":"on_demand","name":"On-demand","amount":43.21,"unit":"usd","secret":"not-a-meter"},{"kind":"grants","id":"x","name":"X","amount":1,"unit":"usd"},"nope",{"kind":"grants","id":"i".repeat(300),"name":"Big","a":"p".repeat(190),"b":"p".repeat(190),"c":"p".repeat(190),"d":"p".repeat(190)}]);
        let wire = Reading::from(&given, NOW, Some("set_fixture"))
            .unwrap()
            .wire();
        let encoded = serde_json::to_string(&wire).unwrap();
        assert!(!encoded.contains("fixture-secret-never-shared"));
        assert!(!encoded.contains("not-a-meter"));
        assert_eq!(
            wire["accounts"][0]["extras"],
            json!([{"kind":"spent","id":"on_demand","name":"On-demand","amount":43.21,"unit":"usd"},{"kind":"grants","id":"x","name":"X","amount":1,"unit":"usd"}])
        );
    }
    #[test]
    fn peer_service_names_allow_valid_future_services_but_not_arbitrary_text() {
        let given = json!({"at":NOW,"accounts":[{"provider":"kimi","service":"kimi-code","identity":"abcdefabcdefabcd","checked":NOW,"windows":[{"id":"week","used":0.25,"minutes":10080,"read":NOW,"source":"usage"}]},{"provider":"invalid","service":"Not A Name!","windows":[]}]});
        let wire = Reading::from(&given, NOW, None).unwrap().wire();
        assert_eq!(wire["accounts"].as_array().unwrap().len(), 1);
        assert_eq!(wire["accounts"][0]["service"], "kimi-code");
        assert_eq!(wire["accounts"][0]["windows"][0]["used"], 0.25);
    }
    #[test]
    fn future_extra_and_provider_survive_the_kept_peer_round_trip() {
        let mut given = raw(NOW);
        let account = &mut given["accounts"][0];
        account["service"] = json!("kimi-code");
        account["extras"] = json!([
            {"kind":"grants","id":"future","name":"Credit grants","count":2,"next":true,"nested":{"never":"sent"}},
            {"kind":"left","id":"credits","name":"Credits","amount":5,"unit":"credits","resetsAt":NOW+60_000},
            {"kind":"capped","id":"invalid","name":"Invalid","amount":1,"limit":0,"unit":"usd"}
        ]);
        let read = Reading::from(&given, NOW, Some("set_fixture")).unwrap();
        let wire = read.wire();
        let scratch = tempfile::tempdir().unwrap();
        let file = scratch.path().join("kept.json");
        let mut book = Book::default();
        book.roster(
            &BTreeMap::from([("peer".into(), "Peer".into())]),
            Some("set_fixture"),
        );
        book.take("peer", "Peer", read, NOW);
        book.store(&file).unwrap();
        let saved = Book::read(&file).unwrap();
        assert_eq!(saved.scope.as_deref(), Some("set_fixture"));
        assert_eq!(
            saved.peers.get("peer").unwrap().setup.as_deref(),
            Some("set_fixture")
        );
        let account = &saved.peers.get("peer").unwrap().accounts[0];
        assert_eq!(account.service.name(), "kimi-code");
        assert_eq!(
            serde_json::to_value(&account.facts.extras).unwrap(),
            wire["accounts"][0]["extras"]
        );
        assert_eq!(
            wire["accounts"][0]["extras"],
            json!([
                {"kind":"grants","id":"future","name":"Credit grants","count":2,"next":true},
                {"kind":"left","id":"credits","name":"Credits","amount":5.0,"unit":"credits","resetsAt":NOW+60_000}
            ])
        );
    }
    #[test]
    fn unsupported_devin_notice_is_not_admitted_even_when_its_future_service_is_listed() {
        let mut given = raw(NOW);
        let account = &mut given["accounts"][0];
        account["service"] = json!("devin");
        account["identity"] = json!("ABCDEFABCDEFABCD");
        account["notices"] = json!([{"id":"usage_fixture","row":"devin#abcdefabcdefabcd","service":"devin","window":"five_hour","windowName":"Weekly","accountName":"Devin","milestone":"underTenPercent","at":NOW,"remaining":0.05,"resetsAt":NOW+3600000}]);
        let read = Reading::from(&given, NOW, Some("set_fixture")).unwrap();
        assert_eq!(read.accounts.len(), 1);
        assert!(read.accounts[0].identity.is_none());
        assert!(read.accounts[0].notices.is_empty());
    }
    #[test]
    fn long_notice_history_cannot_displace_the_current_account_windows() {
        let mut given = raw(NOW);
        let account = &mut given["accounts"][0];
        account["notices"]=json!((0..100).map(|n|json!({"id":format!("usage_fixture-{n}"),"row":"claude#abcdefabcdefabcd","service":"claude","window":"five_hour","windowName":"x".repeat(100),"accountName":"x".repeat(100),"milestone":"underTenPercent","at":NOW,"remaining":0.05,"resetsAt":NOW+3600000})).collect::<Vec<_>>());
        let read = Reading::from(&given, NOW, Some("set_fixture")).unwrap();
        assert_eq!(read.accounts[0].notices.len(), 100);
        let wire = read.wire();
        assert_eq!(wire["accounts"].as_array().unwrap().len(), 1);
        assert_eq!(wire["accounts"][0]["windows"][0]["used"], 0.35);
        assert!(wire["accounts"][0]["notices"].as_array().unwrap().len() < 100);
        assert!(serde_json::to_vec(&wire).unwrap().len() <= 40_000);
    }
    #[test]
    fn a_named_member_waits_for_its_first_reading_and_scope_changes_only_forget_notices() {
        let mut given = raw(NOW);
        given["accounts"][0]["notices"] = json!([{"id":"usage_fixture","row":"claude#abcdefabcdefabcd","service":"claude","window":"five_hour","windowName":"Session","accountName":"Claude","milestone":"underTenPercent","at":NOW,"remaining":0.05,"resetsAt":NOW+3600000,"forwarder":"peer"}]);
        let mut book = Book::default();
        let names = BTreeMap::from([("peer".into(), "Lab".into())]);
        book.roster(&names, Some("set_fixture"));
        assert_eq!(book.pending().collect::<Vec<_>>(), vec![("peer", "Lab")]);
        let reading = Reading::from(&given, NOW, Some("set_fixture")).unwrap();
        assert_eq!(reading.accounts[0].notices.len(), 1);
        let windows = reading.accounts[0].windows.clone();
        book.take("peer", "Lab", reading, NOW);
        assert_eq!(book.pending().count(), 0);
        assert_eq!(book.peers["peer"].accounts[0].notices.len(), 1);
        book.roster(&names, Some("set_next"));
        assert!(book.peers["peer"].accounts[0].notices.is_empty());
        assert!(book.peers["peer"].accounts[0].notice_state.is_empty());
        assert_eq!(book.peers["peer"].accounts[0].windows, windows);
        let old = Reading::from(&given, NOW, Some("set_next")).unwrap();
        assert!(old.accounts[0].notices.is_empty());
        assert_eq!(old.accounts[0].windows, windows);
    }
}
