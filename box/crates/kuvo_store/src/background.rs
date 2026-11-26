//! The shared background-work inventory used by the session list and the work readers.

use std::{collections::HashSet, path::Path};

use serde_json::Value;

/// How long unfinished background work still counts as work (milliseconds).
pub const AT_WORK_LIMIT: u64 = 6 * 3_600_000;

pub fn active_sessions(run: &Path, now: u64) -> HashSet<String> {
    active_sessions_until(run, now).0
}

/// Earliest loss of an active session, from the same file read.
pub fn active_sessions_until(run: &Path, now: u64) -> (HashSet<String>, Option<u64>) {
    let kept: Value = crate::files::text_of(&run.join("work/background.json"))
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let mut rows = std::collections::HashMap::<String, u64>::new();
    for (id, work) in kept.as_object().into_iter().flat_map(|places| places.values())
        .flat_map(|place| place["sessions"].as_object().into_iter().flatten()) {
        let until = [("commands", "started"), ("subagents", "since"), ("tasks", "since")].iter()
            .flat_map(|(kind, field)| work[*kind].as_array().into_iter().flatten().filter_map(move |row| row[*field].as_f64()))
            .filter(|at| (now as f64 - at) < AT_WORK_LIMIT as f64)
            .map(|at| (at + AT_WORK_LIMIT as f64).ceil() as u64).max();
        if let Some(until) = until {
            rows.entry(id.clone()).and_modify(|was| *was = (*was).max(until)).or_insert(until);
        }
    }
    let next = rows.values().copied().min();
    (rows.into_keys().collect(), next)
}
