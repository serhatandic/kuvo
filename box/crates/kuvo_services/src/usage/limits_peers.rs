//! The limits portion of authenticated peer mail. Only account hashes and
//! readings cross this edge; no provider credential, target ID or sign-in mark.
use super::{
    limits::{Account, Fact, Provider},
    plan_facts::{Balance, Extra, Facts, Problem, ProblemKind},
    readings::{Source, Trail, Window},
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
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub agents_reset: bool,
}
impl AccountMail {
    pub fn account(&self) -> Account {
        Account {
            provider: self.provider.clone(),
            service: self.service,
            account: self.account.clone(),
            label: self.label.clone(),
            identity: self.identity.clone(),
            sign_in: String::new(),
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
    let text = value.as_str()?.trim();
    (!text.is_empty()).then(|| text.chars().take(most).collect())
}
fn clock(at: u64, offset: i128, now: u64) -> u64 {
    (i128::from(at) + offset).clamp(0, i128::from(now)) as u64
}
fn time(value: &Value) -> Option<u64> {
    value.as_u64().filter(|n| *n > 0)
}
fn account(value: &Value, offset: i128, now: u64, notices: bool) -> Option<AccountMail> {
    let provider = words(&value["provider"], 140)?;
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
    let account = agent.and_then(|_| words(&value["account"], 100));
    let identity = words(&value["identity"], 17)
        .filter(|s| s.len() == 16 && s.bytes().all(|b| b.is_ascii_hexdigit()));
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
                id: words(&w["id"], 100)?,
                used,
                minutes: time(&w["minutes"]).and_then(|n| n.try_into().ok()),
                resets_at: time(&w["resetsAt"]),
                model: words(&w["model"], 140),
                name: words(&w["name"], 140),
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
            e["id"] = json!(words(&e["id"], 100)?);
            e["name"] = json!(words(&e["name"], 100)?);
            if e["kind"] == "resets" {
                e["usable"] = json!(e["usable"] == true);
                if let Some(count) = e["count"].as_f64().filter(|n| n.is_finite() && *n >= 0.0) {
                    e["count"] = json!(count.floor() as u32);
                }
                if let Some(a) = e["expiries"].as_array_mut() {
                    a.retain(|n| time(n).is_some());
                    a.truncate(50);
                }
                if let Some(a) = e["refills"].as_array_mut() {
                    a.truncate(10);
                }
            }
            if e["kind"] == "unreadable" {
                e["message"] = json!(words(&e["message"], 500)?);
            }
            serde_json::from_value::<Extra>(e).ok()
        })
        .collect();
    let balance = serde_json::from_value::<Balance>(value["balance"].clone())
        .ok()
        .filter(|b| b.left.is_finite() && b.limit.is_none_or(f64::is_finite));
    let problem = serde_json::from_value::<Problem>(value["problem"].clone())
        .ok()
        .filter(|p| p.kind != ProblemKind::None)
        .map(|mut p| {
            p.message = p.message.chars().take(500).collect();
            p.until = p
                .until
                .map(|t| (i128::from(t) + offset).clamp(0, i128::from(u64::MAX)) as u64);
            p
        });
    let facts = Facts {
        plan: words(&value["plan"], 100),
        balance,
        extras,
        problem,
        checked: time(&value["checked"]).map(|t| clock(t, offset, now)),
        next: time(&value["next"])
            .map(|t| (i128::from(t) + offset).clamp(0, i128::from(u64::MAX)) as u64),
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
                    || fact.id.len() > 150
                    || fact.row.len() > 200
                    || fact.window.len() > 100
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
        agents_reset: value["agentsReset"] == true,
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
        serde_json::to_value(reading).unwrap_or_default()
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
    pub fn read(path: &Path) -> std::io::Result<Self> {
        let Some(bytes) = kuvo_store::files::read(path)? else {
            return Ok(Self::default());
        };
        if let Ok(book) = serde_json::from_slice(&bytes) {
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
                changed |=
                    peer.setup.is_some() || peer.accounts.iter().any(|a| !a.notices.is_empty());
                peer.setup = None;
                for account in &mut peer.accounts {
                    account.notices.clear();
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
            let ids = account
                .windows
                .iter()
                .map(|w| w.id.clone())
                .collect::<BTreeSet<_>>();
            trail.retain(|id, _| ids.contains(id));
            for window in std::mem::take(&mut account.windows) {
                if let Some(existing) = trail.get_mut(&window.id) {
                    existing.observe(window, &Default::default());
                } else if let Some(next) = Trail::new(window) {
                    trail.insert(next.current.id.clone(), next);
                }
            }
            account.windows = trail.values().map(|t| t.current.clone()).collect();
            account.notices.retain(|n| {
                n.row == row
                    && trail
                        .get(&n.window)
                        .is_some_and(|t| t.current.resets_at == n.resets_at)
            });
            for notice in &account.notices {
                if let Some(t) = trail.get_mut(&notice.window) {
                    t.mark_fired(notice.milestone);
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
