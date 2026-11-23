//! Notice history keeps the publisher's identity and IDs when clients switch computers.

use std::path::Path;

use kuvo_protocol::routes::{get_kuvo_notices, get_kuvo_setup_notices_events, types::*};
use kuvo_wire::keys::encode;
use serde_json::Number;

use crate::{kept, reach::Millis, setup_lists::View};

pub type Notice = GetKuvoNoticesStatus200NoticesItem;
pub type History = get_kuvo_notices::Status200;
pub const FEATURE: &str = "line.merged-notices";
const NEWS_FOR: Millis = 600_000;

pub fn load(run: &Path, telling: bool) -> History {
    let notices = kept::read::<Vec<serde_json::Value>>(&run.join("notices/told.json"))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| serde_json::from_value(entry).ok())
        .collect();
    History { notices, telling }
}

fn recent(entry: &Notice, now: Millis) -> bool {
    let at = entry.at.as_f64().unwrap_or_default();
    (now as f64) - at < NEWS_FOR as f64
        && (entry.notice.category != "usage"
            || entry
                .notice
                .data
                .as_ref()
                .and_then(|data| data.get("expires"))
                .is_none_or(|expires| {
                    expires.is_empty() || expires.parse::<Millis>().is_ok_and(|at| at > now)
                }))
}

impl View {
    pub fn notices(&self, now: Millis) -> History {
        let mut history = (*self.notices).clone();
        history.notices.retain(|entry| recent(entry, now));
        history
    }

    pub fn merged_notices(&self, now: Millis) -> get_kuvo_setup_notices_events::Status200 {
        let here = GetKuvoSetupNoticesEventsStatus200OriginsItem {
            computer: self.roster.local(),
            away: false,
            telling: self.notices.telling,
            last_seen: None,
        };
        let peers = self.roster.members.iter().filter_map(|peer| {
            let copy = self.notice_copies.get(&peer.key)?;
            let key = encode(&peer.key);
            let release = self.announced.get(&key).and_then(|entry| entry.release());
            let standing = self
                .reach
                .copy_standing(&key, Some(copy.seen), release, now);
            let origin = GetKuvoSetupNoticesEventsStatus200OriginsItem {
                computer: peer.computer(),
                away: standing.away(),
                last_seen: standing.last_seen().map(Number::from),
                telling: !standing.away() && copy.value.telling,
            };
            Some((origin, &copy.value))
        });
        let mut origins = vec![];
        let mut notices = vec![];
        for (origin, copy) in std::iter::once((here, self.notices.as_ref())).chain(peers) {
            notices.extend(
                copy.notices
                    .iter()
                    .filter(|entry| recent(entry, now))
                    .map(|entry| GetKuvoSetupNoticesEventsStatus200NoticesItem {
                        id: entry.id.clone(),
                        at: entry.at.clone(),
                        by: entry.by,
                        notice: entry.notice.clone(),
                        computer: origin.computer.clone(),
                        away: origin.away,
                        last_seen: origin.last_seen.clone(),
                    }),
            );
            origins.push(origin);
        }
        notices.sort_by(|a, b| {
            a.at.as_f64()
                .unwrap_or_default()
                .total_cmp(&b.at.as_f64().unwrap_or_default())
                .then(a.computer.id.cmp(&b.computer.id))
                .then(a.id.cmp(&b.id))
        });
        get_kuvo_setup_notices_events::Status200 {
            pending_computers: Some(
                self.pending(FEATURE, |key| self.notice_copies.contains_key(key)),
            ),
            origins,
            notices,
        }
    }
}
