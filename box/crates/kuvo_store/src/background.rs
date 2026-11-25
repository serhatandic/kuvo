//! The shared background-work inventory used by the session list and the work readers.

use std::{collections::HashSet, path::Path};

use serde_json::Value;

/// How long unfinished background work still counts as work (milliseconds).
pub const AT_WORK_LIMIT: u64 = 6 * 3_600_000;

pub fn active_sessions(run: &Path, now: u64) -> HashSet<String> {
    let kept: Value = crate::files::text_of(&run.join("work/background.json"))
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    kept.as_object()
        .into_iter()
        .flat_map(|places| places.values())
        .flat_map(|place| place["sessions"].as_object().into_iter().flatten())
        .filter(|(_, work)| {
            [
                ("commands", "started"),
                ("subagents", "since"),
                ("tasks", "since"),
            ]
            .iter()
            .any(|(kind, time)| {
                work[*kind].as_array().into_iter().flatten().any(|row| {
                    row[*time]
                        .as_f64()
                        .is_some_and(|at| (now as f64 - at) < AT_WORK_LIMIT as f64)
                })
            })
        })
        .map(|(id, _)| id.clone())
        .collect()
}
