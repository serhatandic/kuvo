//! The Helper's peer rows use the same accepted roster and authenticated announcements.

use std::path::Path;

use kuvo_protocol::routes::types::{
    GetKuvoBotsStatus200ElsewhereItemKind3 as Kind,
    GetKuvoBoxAtWorkStatus200ComputersItem as Computer,
    GetKuvoBoxAtWorkStatus200ComputersItemSessionsItem as Session,
    GetKuvoComputersStatus200ComputersItemReach4 as Standing,
};
use kuvo_wire::keys::encode;
use serde_json::Value;

use crate::{
    kept,
    reach::{Millis, Reach},
    session::ComputerKind,
    setup_lists::View,
};

impl View {
    pub fn helper_peers(&self, run: &Path, now: Millis) -> Vec<Computer> {
        let reach = Reach::load(run);
        let tries: Value = kept::read(&run.join("reach-again.json")).unwrap_or_default();
        self.roster
            .members_by_name()
            .into_iter()
            .map(|peer| {
                let key = encode(&peer.key);
                let said = self.announced.get(&key);
                let heard = said.and_then(|said| said.heard);
                let away = reach.peer_is_away(&key, heard, now);
                let trying = tries[&key].as_f64().is_some_and(|at| {
                    (0.0..20_000.0).contains(&(now as f64 - at))
                        && heard.is_none_or(|heard| (heard as f64) < at)
                });
                let standing = if trying || !away && heard.is_none() {
                    Standing::Connecting
                } else if away {
                    Standing::Away
                } else {
                    Standing::Connected
                };
                let facts = said
                    .and_then(|said| said.computer.as_ref())
                    .filter(|_| !away);
                let working = facts.and_then(|facts| facts.working);
                let sessions = facts
                    .and_then(|facts| facts.at_work.as_ref())
                    .into_iter()
                    .flatten()
                    .map(|row| Session {
                        id: row.id.clone(),
                        title: row.title.clone(),
                        bot: None,
                    })
                    .collect();
                Computer {
                    key,
                    name: peer.name.clone(),
                    kind: if peer.kind == ComputerKind::Mac {
                        Kind::Mac
                    } else {
                        Kind::Machine
                    },
                    here: None,
                    reach: standing,
                    working: working.map(Into::into),
                    sessions,
                    waiting: facts.and_then(|facts| facts.waiting).map(Into::into),
                    asking: facts
                        .and_then(|facts| facts.asking.clone())
                        .unwrap_or_default(),
                    rings: vec![],
                }
            })
            .collect()
    }
}
