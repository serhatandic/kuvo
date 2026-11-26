//! Compact usage slots in the existing seat mail format. Each authenticated
//! sender supplies only its own source totals; raw logs and credentials stay local.
use super::history::{Account, Computer, Slots};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};
const SLOT: i64 = 900_000;
const KEEP: i64 = 35 * 86_400_000;
const AWAY: i64 = 25 * 60_000;
const MAX: usize = 400_000;
const USES: usize = 50_000;
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct KeptSlots {
    pub models: Vec<String>,
    pub used: Vec<(i64, usize, u64, f64)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unknown: Vec<(i64, usize)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallback: Vec<(i64, usize)>,
}
impl KeptSlots {
    fn name(&mut self, model: &str) -> usize {
        if let Some(index) = self.models.iter().position(|m| m == model) {
            index
        } else {
            self.models.push(model.into());
            self.models.len() - 1
        }
    }
    pub fn from_slots(slots: &Slots) -> Self {
        let mut kept = Self::default();
        for (slot, models) in &slots.used {
            for (model, (tokens, cost)) in models {
                let index = kept.name(model);
                kept.used
                    .push((slot / SLOT, index, *tokens, (cost * 1e6).round() / 1e6));
            }
        }
        for (slot, models) in &slots.unknown {
            for model in models {
                let index = kept.name(model);
                kept.unknown.push((slot / SLOT, index));
            }
        }
        for (slot, models) in &slots.fallback {
            for model in models {
                let index = kept.name(model);
                kept.fallback.push((slot / SLOT, index));
            }
        }
        kept
    }
    fn from_value(value: &Value) -> Self {
        let models = value["models"]
            .as_array()
            .into_iter()
            .flatten()
            .take(USES)
            .map(|m| {
                m.as_str()
                    .map(|s| s.chars().take(200).collect::<String>())
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>();
        let at = |v: &Value| v.as_i64().filter(|at| *at > 0 && *at < 1_000_000_000);
        let index = |v: &Value| {
            v.as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .filter(|i| models.get(*i).is_some_and(|s| !s.is_empty()))
        };
        let used = value["used"]
            .as_array()
            .into_iter()
            .flatten()
            .take(USES)
            .filter_map(|v| {
                Some((
                    at(&v[0])?,
                    index(&v[1])?,
                    v[2].as_u64().filter(|n| *n <= 9_007_199_254_740_991)?,
                    v[3].as_f64().filter(|n| n.is_finite() && *n >= 0.)?,
                ))
            })
            .collect();
        let marks = |name: &str| {
            value[name]
                .as_array()
                .into_iter()
                .flatten()
                .take(USES)
                .filter_map(|v| Some((at(&v[0])?, index(&v[1])?)))
                .collect()
        };
        let unknown = marks("unknown");
        let fallback = marks("fallback");
        Self {
            models,
            used,
            unknown,
            fallback,
        }
    }
    pub fn slots(&self, now: i64) -> Slots {
        let mut slots = Slots::default();
        let in_range = |at: i64| at * SLOT >= now - KEEP && at * SLOT <= now + 86_400_000;
        for (at, i, tokens, cost) in &self.used {
            if in_range(*at) {
                if let Some(model) = self.models.get(*i) {
                    slots.add(at * SLOT, model.clone(), *tokens, *cost);
                }
            }
        }
        for (at, i) in &self.unknown {
            if in_range(*at) {
                if let Some(model) = self.models.get(*i) {
                    slots
                        .unknown
                        .entry(at * SLOT)
                        .or_default()
                        .insert(model.clone());
                }
            }
        }
        for (at, i) in &self.fallback {
            if in_range(*at) {
                if let Some(model) = self.models.get(*i) {
                    slots
                        .fallback
                        .entry(at * SLOT)
                        .or_default()
                        .insert(model.clone());
                }
            }
        }
        slots
    }
    /// Retain the sender's slot/model order so mixed-version digests agree.
    fn merge(old: Option<&Self>, new: &Self, from: i64, whole: i64) -> Self {
        let mut kept = Self::default();
        let old = old.into_iter();
        for source in old.clone().chain(std::iter::once(new)) {
            for (slot, i, tokens, cost) in &source.used {
                if *slot < whole
                    || if std::ptr::eq(source, new) {
                        *slot < from
                    } else {
                        *slot >= from
                    }
                {
                    continue;
                }
                if let Some(model) = source.models.get(*i) {
                    let next = kept.name(model);
                    kept.used.push((*slot, next, *tokens, *cost));
                }
            }
        }
        for source in old.clone().chain(std::iter::once(new)) {
            for (slot, i) in &source.unknown {
                if *slot < whole
                    || if std::ptr::eq(source, new) {
                        *slot < from
                    } else {
                        *slot >= from
                    }
                {
                    continue;
                }
                if let Some(model) = source.models.get(*i) {
                    let next = kept.name(model);
                    kept.unknown.push((*slot, next));
                }
            }
        }
        for source in old.chain(std::iter::once(new)) {
            for (slot, i) in &source.fallback {
                if *slot < whole
                    || if std::ptr::eq(source, new) {
                        *slot < from
                    } else {
                        *slot >= from
                    }
                {
                    continue;
                }
                if let Some(model) = source.models.get(*i) {
                    let next = kept.name(model);
                    kept.fallback.push((*slot, next));
                }
            }
        }
        kept
    }
    fn recent(&mut self, from: i64) {
        self.used.retain(|u| u.0 >= from);
        self.unknown.retain(|u| u.0 >= from);
        self.fallback.retain(|u| u.0 >= from);
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Told {
    pub provider: String,
    pub service: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scanned: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub places: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated: Option<bool>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub account_wide: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cut: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub whole: Option<i64>,
    pub slots: KeptSlots,
}
fn text(v: &Value, most: usize) -> Option<String> {
    let text = v.as_str()?;
    (!text.is_empty()).then(|| text.chars().take(most).collect())
}
fn hash_value(s: &str, size: usize) -> bool {
    s.len() == size
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
impl Told {
    pub fn from_account(a: Account) -> Self {
        Self {
            provider: a.provider,
            service: a.service,
            agent: a.agent,
            account: a.account,
            label: a.label,
            identity: a.identity,
            scanned: a.scanned,
            places: a.places,
            estimated: a.billed.then_some(false),
            account_wide: a.wide,
            cut: a.cut,
            whole: None,
            slots: KeptSlots::from_slots(&a.slots),
        }
    }
    fn account(&self, now: i64) -> Account {
        Account {
            provider: self.provider.clone(),
            service: self.service.clone(),
            agent: self.agent.clone(),
            account: self.account.clone(),
            label: self.label.clone(),
            identity: self.identity.clone(),
            scanned: self.scanned,
            places: self.places.clone(),
            billed: self.estimated == Some(false),
            wide: self.account_wide,
            cut: self.cut,
            slots: self.slots.slots(now),
        }
    }
    fn from(value: &Value, off: i64, now: i64) -> Option<Self> {
        let provider = text(&value["provider"], 200)?;
        let service = text(&value["service"], 40)?;
        if !service
            .bytes()
            .enumerate()
            .all(|(i, b)| b.is_ascii_lowercase() || b.is_ascii_digit() || i > 0 && b == b'-')
        {
            return None;
        }
        let agent = text(&value["agent"], 40).filter(|s| {
            matches!(
                s.as_str(),
                "claude" | "codex" | "grok" | "cursor" | "antigravity"
            )
        });
        let account = agent.as_ref().and_then(|_| text(&value["account"], 200));
        let scanned = value["scanned"]
            .as_i64()
            .filter(|n| *n > 0)
            .map(|n| n.saturating_add(off).min(now));
        let identity = text(&value["identity"], 17).filter(|s| hash_value(s, 16));
        let places = value["places"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|s| hash_value(s, 16))
            .take(20)
            .map(str::to_owned)
            .collect();
        Some(Self {
            provider,
            service,
            agent,
            account,
            scanned,
            identity,
            places,
            label: text(&value["label"], 100),
            estimated: (value["estimated"] == false).then_some(false),
            account_wide: value["accountWide"] == true,
            cut: value["cut"] == true,
            whole: value["whole"].as_i64().filter(|n| *n > 0),
            slots: KeptSlots::from_value(&value["slots"]),
        })
    }
}
pub fn digest(accounts: &[Told]) -> String {
    let canonical = accounts
        .iter()
        .map(|a| {
            json!([
                a.provider,
                a.service,
                a.agent,
                a.account,
                a.label,
                a.identity,
                a.cut,
                a.places,
                a.slots.models,
                a.slots.used,
                a.slots.unknown
            ])
        })
        .collect::<Vec<_>>();
    hex::encode(&Sha256::digest(kuvo_wire::json::ordinary(&json!(canonical)))[..16])
}
pub fn bounded(mut accounts: Vec<Told>, now: i64) -> Vec<Told> {
    accounts.truncate(50);
    let start = (now - KEEP).div_euclid(86_400_000) * 96;
    for account in &mut accounts {
        account.whole = Some(start);
        account.slots = KeptSlots::merge(None, &account.slots, start, start);
    }
    // As TS does, cut oldest usage first, noting the first complete slot.
    loop {
        let bytes = kuvo_wire::json::ordinary(&json!(accounts)).len();
        if bytes <= MAX {
            break;
        }
        let mut uses = accounts
            .iter()
            .flat_map(|a| a.slots.used.iter().map(|u| u.0))
            .collect::<Vec<_>>();
        if uses.is_empty() {
            accounts.pop();
            if accounts.is_empty() {
                break;
            }
            continue;
        }
        let n = ((bytes - MAX) * uses.len())
            .div_ceil(bytes)
            .saturating_add(16)
            .min(uses.len())
            - 1;
        let oldest = *uses.select_nth_unstable(n).1;
        for account in &mut accounts {
            if account.slots.used.iter().any(|u| u.0 <= oldest) {
                account.whole = Some(account.whole.unwrap_or(0).max(oldest + 1));
                account.cut = true;
                account.slots = KeptSlots::merge(None, &account.slots, oldest + 1, oldest + 1);
            }
        }
    }
    accounts
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Kept {
    name: String,
    heard: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    digest: Option<String>,
    accounts: Vec<Told>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Book {
    peers: BTreeMap<String, Kept>,
    #[serde(skip)]
    roster: BTreeMap<String, String>,
}
impl Book {
    pub fn read(path: &Path) -> Self {
        kuvo_store::files::read(path)
            .ok()
            .flatten()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }
    pub fn keep(&self, path: &Path) -> std::io::Result<()> {
        if self.peers.is_empty() && !path.exists() {
            return Ok(());
        }
        kuvo_store::files::write_secret(
            path,
            &serde_json::to_vec(self).map_err(std::io::Error::other)?,
        )
    }
    pub fn roster(&mut self, names: &BTreeMap<String, String>) -> bool {
        let before = self.peers.len();
        let mut changed = self.roster != *names;
        self.roster = names.clone();
        self.peers.retain(|id, peer| {
            let Some(name) = names.get(id) else {
                return false;
            };
            if &peer.name != name {
                peer.name = name.clone();
                changed = true;
            }
            true
        });
        changed || before != self.peers.len()
    }
    pub fn take(&mut self, id: &str, mail: &Value, now: i64) -> Option<bool> {
        let name = self.roster.get(id)?.clone();
        let given = text(&mail["digest"], 33).filter(|s| hash_value(s, 32))?;
        let at = mail["at"].as_i64().unwrap_or(now);
        let off = if at.abs_diff(now) >= 60_000 {
            now.saturating_sub(at)
        } else {
            0
        };
        let prior = self.peers.get(id);
        let from = mail["from"]
            .as_i64()
            .filter(|n| *n > 0 && *n < 1_000_000_000);
        let Some(raw) = mail["accounts"].as_array() else {
            if let Some(peer) = self.peers.get_mut(id) {
                peer.heard = now;
                fresh(&mut peer.accounts, mail, off, now);
                return Some(peer.digest.as_ref() != Some(&given));
            }
            return Some(true);
        };
        let mut accounts = raw
            .iter()
            .take(50)
            .filter_map(|a| Told::from(a, off, now))
            .collect::<Vec<_>>();
        if let Some(from) = from {
            for account in &mut accounts {
                let old =
                    prior.and_then(|p| p.accounts.iter().find(|a| a.provider == account.provider));
                account.slots = KeptSlots::merge(
                    old.map(|a| &a.slots),
                    &account.slots,
                    from,
                    account.whole.unwrap_or(0),
                );
            }
        }
        let coherent = digest(&accounts) == given;
        fresh(&mut accounts, mail, off, now);
        self.peers.insert(
            id.into(),
            Kept {
                name,
                heard: now,
                digest: coherent.then_some(given),
                accounts,
            },
        );
        Some(!coherent)
    }
    pub fn reads_whole(&self, own: &str, service: &str, identity: &str, now: i64) -> bool {
        self.peers.iter().any(|(key, peer)| {
            key.as_str() < own
                && self.roster.contains_key(key)
                && now.saturating_sub(peer.heard) <= AWAY
                && peer.accounts.iter().any(|a| {
                    a.account_wide
                        && a.service == service
                        && a.identity.as_deref() == Some(identity)
                        && a.scanned.is_some_and(|at| now.saturating_sub(at) < AWAY)
                })
        })
    }
    pub fn computers(&self, now: i64) -> Vec<Computer> {
        let mut peers = self
            .peers
            .iter()
            .filter(|(key, _)| self.roster.contains_key(*key))
            .collect::<Vec<_>>();
        peers.sort_by(|(a, x), (b, y)| x.name.cmp(&y.name).then(a.cmp(b)));
        peers
            .into_iter()
            .map(|(key, peer)| Computer {
                key: key.clone(),
                name: peer.name.clone(),
                here: false,
                away: now.saturating_sub(peer.heard) > AWAY,
                accounts: peer.accounts.iter().map(|a| a.account(now)).collect(),
            })
            .collect()
    }
}
fn fresh(accounts: &mut [Told], mail: &Value, off: i64, now: i64) {
    for account in accounts.iter_mut().filter(|a| a.account_wide) {
        if let Some(at) = mail["fresh"][&account.provider].as_i64() {
            let at = at.saturating_add(off).min(now);
            if account.scanned.is_none_or(|old| at > old) {
                account.scanned = Some(at);
            }
        }
    }
}
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub accounts: Vec<Told>,
    pub digest: String,
    pub fallback_revision: String,
}
impl Snapshot {
    pub fn new(accounts: Vec<Account>, now: i64) -> Self {
        let accounts = bounded(accounts.into_iter().map(Told::from_account).collect(), now);
        let digest = digest(&accounts);
        let fallback_revision = hex::encode(Sha256::digest(kuvo_wire::json::ordinary(&json!(
            accounts
                .iter()
                .map(|a| (
                    &a.provider,
                    a.slots
                        .fallback
                        .iter()
                        .filter_map(|(slot, i)| Some((*slot, a.slots.models.get(*i)?)))
                        .collect::<Vec<_>>()
                ))
                .collect::<Vec<_>>()
        ))));
        Self {
            accounts,
            digest,
            fallback_revision,
        }
    }
    fn with_fresh(&self, mut value: Value) -> Value {
        let fresh = self
            .accounts
            .iter()
            .filter(|a| a.account_wide)
            .filter_map(|a| Some((&a.provider, a.scanned?)))
            .collect::<BTreeMap<_, _>>();
        if !fresh.is_empty() {
            value["fresh"] = json!(fresh);
        }
        value
    }
    pub fn full(&self, at: i64, ask: bool, refresh: bool) -> Value {
        let mut mail =
            json!({"kind":"usage","at":at,"accounts":self.accounts,"digest":self.digest});
        if ask {
            mail["ask"] = json!(true);
        }
        if refresh {
            mail["refresh"] = json!(true);
        }
        self.with_fresh(mail)
    }
    pub fn recent(&self, at: i64) -> Value {
        let from = (at - 2 * 86_400_000).div_euclid(SLOT);
        let mut accounts = self.accounts.clone();
        for account in &mut accounts {
            account.slots.recent(from);
        }
        self.with_fresh(
            json!({"kind":"usage","at":at,"from":from,"accounts":accounts,"digest":self.digest}),
        )
    }
    pub fn heartbeat(&self, at: i64) -> Value {
        self.with_fresh(json!({"kind":"usage","at":at,"digest":self.digest}))
    }
}

#[cfg(test)]
mod account_wide_tests {
    use super::*;
    use crate::usage::{history, pricing::Pricing};
    const AT: i64 = 1_794_916_800_000;
    const WHO: &str = "aaaaaaaaaaaaaaaa";
    fn wide(at: i64, tokens: u64) -> Account {
        let mut slots = Slots::default();
        slots.add(
            AT.div_euclid(SLOT) * SLOT,
            "composer-2".into(),
            tokens,
            tokens as f64 / 1000.0,
        );
        Account {
            provider: "cursor".into(),
            service: "cursor".into(),
            agent: Some("cursor".into()),
            account: Some("default".into()),
            label: None,
            identity: Some(WHO.into()),
            scanned: Some(at),
            places: vec![],
            billed: false,
            wide: true,
            cut: false,
            slots,
        }
    }
    #[test]
    fn whole_history_turns_follow_a_fresh_admitted_reader_and_move_when_it_stops() {
        let mut book = Book::default();
        book.roster(&BTreeMap::from([
            ("a".into(), "Studio".into()),
            ("c".into(), "Other".into()),
        ]));
        let first = Snapshot::new(vec![wide(AT, 10)], AT);
        assert_eq!(
            book.take("unknown", &first.full(AT, false, false), AT),
            None
        );
        assert!(!book.reads_whole("b", "cursor", WHO, AT));
        assert_eq!(
            book.take("a", &first.full(AT, false, false), AT),
            Some(false)
        );
        assert!(book.reads_whole("b", "cursor", WHO, AT));
        assert!(!book.reads_whole("0", "cursor", WHO, AT));
        assert!(!book.reads_whole("b", "cursor", "bbbbbbbbbbbbbbbb", AT));
        let second = Snapshot::new(vec![wide(AT + 20 * 60_000, 10)], AT + 20 * 60_000);
        assert_eq!(first.digest, second.digest);
        assert_eq!(
            book.take("a", &second.heartbeat(AT + 20 * 60_000), AT + 20 * 60_000),
            Some(false)
        );
        assert!(book.reads_whole("b", "cursor", WHO, AT + 30 * 60_000));
        // A live connection without a successful new read doesn't claim the account.
        assert_eq!(
            book.take("a", &second.heartbeat(AT + 44 * 60_000), AT + 44 * 60_000),
            Some(false)
        );
        assert!(!book.reads_whole("b", "cursor", WHO, AT + 46 * 60_000));
        let third = Snapshot::new(vec![wide(AT + 50 * 60_000, 11)], AT + 50 * 60_000);
        assert_eq!(
            book.take(
                "a",
                &third.full(AT + 50 * 60_000, false, false),
                AT + 50 * 60_000
            ),
            Some(false)
        );
        assert!(book.reads_whole("b", "cursor", WHO, AT + 50 * 60_000));
        assert!(!book.reads_whole("b", "cursor", WHO, AT + 76 * 60_000));
        book.roster(&BTreeMap::new());
        assert!(book.computers(AT + 76 * 60_000).is_empty());
    }
    #[test]
    fn account_wide_rows_use_the_newest_complete_read_not_the_sum() {
        let now = AT + 60_000;
        let mut book = Book::default();
        book.roster(&BTreeMap::from([
            ("b".into(), "Second".into()),
            ("c".into(), "Third".into()),
        ]));
        for (key, at, tokens) in [("b", AT + 10_000, 30), ("c", AT + 20_000, 50)] {
            let snapshot = Snapshot::new(vec![wide(at, tokens)], now);
            assert_eq!(
                book.take(key, &snapshot.full(now, false, false), now),
                Some(false)
            );
        }
        let mut computers = vec![Computer {
            key: "a".into(),
            name: "First".into(),
            here: true,
            away: false,
            accounts: vec![wide(AT, 10)],
        }];
        computers.extend(book.computers(now));
        let view = history::view(
            computers,
            now,
            jiff::tz::TimeZone::UTC,
            &Pricing::load(None).unwrap(),
            None,
            false,
        )
        .unwrap();
        let rows = view["data"]["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], format!("cursor#{WHO}"));
        assert_eq!(rows[0]["accountWide"], true);
        assert_eq!(rows[0]["boxes"].as_array().unwrap().len(), 3);
        assert_eq!(
            rows[0]["days"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|d| d["tokens"].as_u64())
                .sum::<u64>(),
            50
        );
    }
    #[test]
    fn a_fallback_flag_changes_the_send_revision_not_the_older_peer_digest() {
        let mut with = wide(AT, 100);
        with.provider = "codex-cli".into();
        with.service = "codex".into();
        with.agent = Some("codex".into());
        with.wide = false;
        let without_fallback = Snapshot::new(vec![with.clone()], AT);
        let slot = AT.div_euclid(SLOT) * SLOT;
        with.slots
            .fallback
            .entry(slot)
            .or_default()
            .insert("gpt-5".into());
        let with_fallback = Snapshot::new(vec![with], AT);
        assert_ne!(
            with_fallback.fallback_revision,
            without_fallback.fallback_revision
        );
        // Older receivers retain the sent model table but omit the new flag.
        // An independently built snapshot may omit the fallback-only model name.
        let mut older = with_fallback.accounts.clone();
        older[0].slots.fallback.clear();
        assert_eq!(with_fallback.digest, digest(&older));
        let mut mail = with_fallback.full(AT, false, false);
        // The old reader keeps no new flags but the hash it computes is still valid.
        mail["accounts"][0]["slots"]
            .as_object_mut()
            .unwrap()
            .remove("fallback");
        let mut book = Book::default();
        book.roster(&BTreeMap::from([("a".into(), "Studio".into())]));
        assert_eq!(book.take("a", &mail, AT), Some(false));
        assert_eq!(
            book.take("a", &with_fallback.heartbeat(AT + 60_000), AT + 60_000),
            Some(false)
        );
    }
    #[test]
    fn computers_with_the_same_name_and_path_count_distinct_machine_places() {
        let path = tempfile::tempdir().unwrap();
        let now = AT + 60_000;
        let computers = [("a", 10), ("b", 20)]
            .into_iter()
            .map(|(key, count)| {
                let mut account = wide(AT, count);
                account.provider = "claude-code".into();
                account.service = "claude".into();
                account.agent = Some("claude".into());
                account.wide = false;
                account.places = vec![crate::usage::native_index::place_of(key, path.path(), None)];
                Computer {
                    key: key.into(),
                    name: "Pi".into(),
                    here: key == "a",
                    away: false,
                    accounts: vec![account],
                }
            })
            .collect();
        let view = history::view(
            computers,
            now,
            jiff::tz::TimeZone::UTC,
            &Pricing::load(None).unwrap(),
            None,
            false,
        )
        .unwrap();
        let row = &view["data"]["rows"][0];
        assert_eq!(row["boxes"].as_array().unwrap().len(), 2);
        assert_eq!(row["days"][0]["tokens"], 30);
        assert!(
            row["boxes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|b| b.get("same").is_none())
        );
    }
}
