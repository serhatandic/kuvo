//! Import only the reading belonging to a confirmed maker identity. Old sign-in
//! fingerprints are not reinterpreted as live credentials, and old process state
//! cannot enter the new reader's acceptance/projection evidence.
use super::{
    limits::Account,
    plan_facts::Facts,
    readings::{NoticeState, Trail, Window},
};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};
#[derive(Default, Deserialize)]
pub struct Cache {
    #[serde(default)]
    accounts: BTreeMap<String, Value>,
    #[serde(default)]
    paces: BTreeMap<String, Value>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Legacy {
    identity: Option<String>,
    #[serde(default)]
    windows: Vec<Window>,
    #[serde(default)]
    notice_state: BTreeMap<String, NoticeState>,
    #[serde(flatten)]
    facts: Facts,
}
impl Cache {
    pub fn load(home: &Path) -> Self {
        kuvo_store::files::text_of(&home.join("agents/limits.json"))
            .and_then(|data| serde_json::from_str(&data).ok())
            .unwrap_or_default()
    }
    pub fn import(
        &mut self,
        account: &Account,
        now: u64,
    ) -> Option<(Facts, BTreeMap<String, Trail>, Vec<String>)> {
        let identity = account.identity.as_ref()?;
        let old: Legacy = serde_json::from_value(self.accounts.remove(&account.provider)?).ok()?;
        if old.identity.as_ref() != Some(identity) {
            return None;
        }
        let mut trails = BTreeMap::new();
        let mut order = vec![];
        for window in old.windows {
            if window.read > now + 60_000 || window.read < now.saturating_sub(366 * 86400_000) {
                continue;
            }
            let key = window.limit_key();
            let pace_key = format!("{}#{identity}|{key}", account.service.name());
            let mut trail = None;
            // The old pace is useful only if its reset agrees with the accepted
            // current window. Re-run the samples through the new acceptance rule.
            if let Some(pace) = self
                .paces
                .get(&pace_key)
                .filter(|p| p["resetsAt"].as_u64() == window.resets_at)
            {
                let since = window
                    .resets_at
                    .zip(window.minutes)
                    .map_or(0, |(reset, minutes)| {
                        reset.saturating_sub(u64::from(minutes) * 60_000)
                    });
                if let Some(samples) = pace["samples"].as_array() {
                    let mut samples = samples
                        .iter()
                        .filter_map(|s| Some((s["at"].as_u64()?, s["used"].as_f64()?)))
                        .filter(|(at, used)| *at >= since && *at < window.read && used.is_finite())
                        .collect::<Vec<_>>();
                    samples.sort_by_key(|(at, _)| *at);
                    for (at, used) in samples
                        .into_iter()
                        .rev()
                        .take(120)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                    {
                        let mut sample = window.clone();
                        sample.read = at;
                        sample.used = used;
                        if let Some(existing) = trail.as_mut() {
                            let existing: &mut Trail = existing;
                            existing.observe(sample, &Default::default());
                        } else {
                            trail = Trail::new(sample);
                        }
                    }
                }
            }
            let mut trail = match trail {
                Some(mut trail) => {
                    if trail.observe(window.clone(), &Default::default()).is_some() {
                        trail
                    } else {
                        let Some(current) = Trail::new(window) else {
                            continue;
                        };
                        current
                    }
                }
                None => {
                    let Some(current) = Trail::new(window) else {
                        continue;
                    };
                    current
                }
            };
            if let Some(state) = old.notice_state.get(&trail.current.id) {
                trail.take_notice_state(state);
            }
            if !order.contains(&trail.current.id) {
                order.push(trail.current.id.clone());
            }
            trails.insert(trail.current.id.clone(), trail);
        }
        if trails.is_empty() && old.facts.extras.is_empty() && old.facts.balance.is_none() {
            return None;
        }
        Some((old.facts, trails, order))
    }
}
