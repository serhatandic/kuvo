//! Bounded Insight metadata at the authenticated peer-mail edge. Prompts,
//! replies, and provider credentials are not part of this protocol.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Cursor {
    pub after: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub epoch: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum Mail {
    Pull {
        #[serde(flatten)]
        cursor: Cursor,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        refresh: bool,
    },
    Page {
        after: u64,
        next: u64,
        more: bool,
        epoch: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        reset: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        scanned: Option<i64>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        issues: Vec<String>,
        records: Vec<Record>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub key: String,
    pub at: i64,
    pub session: String,
    pub call: String,
    pub computer: String,
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pricing_models: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reported_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<Buckets>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub attributed: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub account_observed: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bot_own: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub human: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub partial: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Buckets {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_write_5m: Option<u64>,
    pub cache_write_1h: Option<u64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fast: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ultrafast: bool,
}
impl Buckets {
    pub fn complete(&self) -> Option<super::pricing::Tokens> {
        Some(super::pricing::Tokens {
            input: self.input?,
            output: self.output?,
            cache_read: self.cache_read?,
            cache_write_5m: self.cache_write_5m?,
            cache_write_1h: self.cache_write_1h?,
            fast: self.fast,
        })
    }
    pub fn fields(&self) -> [Option<u64>; 5] {
        [
            self.input,
            self.output,
            self.cache_read,
            self.cache_write_5m,
            self.cache_write_1h,
        ]
    }
}
pub fn total(fields: impl IntoIterator<Item = Option<u64>>) -> Option<u64> {
    fields.into_iter().flatten().reduce(u64::saturating_add)
}
fn key(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn small(s: &str) -> bool {
    !s.is_empty() && s.len() <= 512
}
const DAY: i64 = 86_400_000;
impl Record {
    /// Peers can identify only origins authorised by the current signed setup.
    pub fn validate(&mut self, now: i64, origin: &BTreeSet<String>) -> bool {
        if !key(&self.key)
            || self.supersedes.as_ref().is_some_and(|s| !key(s))
            || self.at < now - 366 * DAY
            || self.at > now + DAY
            || ![&self.session, &self.call, &self.provider, &self.computer]
                .into_iter()
                .all(|s| small(s))
            || !origin.contains(&self.computer)
            || self.skills.len() > 32
            || self.pricing_models.len() > 32
            || !self
                .skills
                .iter()
                .chain(self.pricing_models.iter())
                .all(|x| small(x))
            || self.tokens.as_ref().is_some_and(|t| {
                t.fields()
                    .into_iter()
                    .flatten()
                    .any(|n| n > 9_007_199_254_740_991)
            })
            || self
                .reported_total
                .is_some_and(|n| n > 9_007_199_254_740_991)
        {
            return false;
        }
        for text in [
            &mut self.account,
            &mut self.agent,
            &mut self.title,
            &mut self.bot,
            &mut self.bot_name,
            &mut self.root,
            &mut self.model,
            &mut self.native_session,
        ] {
            if let Some(value) = text.as_mut() {
                *value = value.chars().take(512).collect();
                if value.is_empty() {
                    *text = None;
                }
            }
        }
        if self.root.as_ref() == Some(&self.session) {
            self.root = None;
        }
        self.account_observed &= self.account.is_some();
        self.partial |= !self.human && self.tokens.as_ref().and_then(Buckets::complete).is_none();
        true
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scope {
    pub setup: Option<String>,
    pub own: Option<String>,
    pub peers: BTreeMap<String, String>,
    pub historical: BTreeMap<String, String>,
    pub connected: BTreeSet<String>,
}
impl Scope {
    pub fn eligible(&self) -> BTreeSet<String> {
        self.own
            .iter()
            .chain(self.peers.keys())
            .chain(self.historical.keys())
            .cloned()
            .collect()
    }
}
