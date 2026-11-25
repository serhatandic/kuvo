//! The menu's projection of the account rows published by the plan-limits service.

use std::{collections::BTreeSet, sync::Arc};

use kuvo_protocol::routes::types::{
    GetKuvoBoxAtWorkStatus200ComputersItemRingsItem as Rings,
    GetKuvoBoxAtWorkStatus200ComputersItemRingsItemLong as Ring,
};
use serde_json::{Number, Value};
use tokio::sync::watch;

#[derive(Clone, Debug, PartialEq)]
pub struct RingsOn {
    pub rings: Rings,
    on: BTreeSet<String>,
}

impl RingsOn {
    pub fn on(&self, key: &str, here: bool) -> bool {
        !key.is_empty() && self.on.contains(key) || here && self.on.contains("here")
    }
}

fn ring_of(value: &Value) -> Option<Ring> {
    Some(Ring {
        used: Number::from_f64(value["used"].as_f64()?.clamp(0.0, 1.0))?,
        level: value["level"].as_str()?.to_owned(),
    })
}

fn rings_of(row: &Value) -> Option<RingsOn> {
    if row["ended"] == true {
        return None;
    }
    Some(RingsOn {
        rings: Rings {
            service: row["service"].as_str()?.to_owned(),
            title: row["title"].as_str()?.to_owned(),
            short: ring_of(&row["rings"]["short"]),
            long: ring_of(&row["rings"]["long"]),
        },
        on: row["boxes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|computer| {
                if computer["here"] == true {
                    Some("here")
                } else {
                    computer["key"].as_str()
                }
            })
            .map(str::to_owned)
            .collect(),
    })
}

/// A resident's current plan-limit rows, including each account's computer membership.
/// The service publishes on readings, account removal and peer changes. Until it has a reading,
/// the Helper has no rings to draw; it does not fetch or infer account limits itself.
#[derive(Clone)]
pub struct MenuLimits(watch::Sender<Arc<Vec<RingsOn>>>);

impl Default for MenuLimits {
    fn default() -> Self {
        Self(watch::channel(Arc::new(Vec::new())).0)
    }
}

impl MenuLimits {
    pub fn publish(&self, rows: &[Value]) {
        let rings: Vec<_> = rows.iter().filter_map(rings_of).collect();
        self.0.send_if_modified(|previous| {
            if **previous == rings {
                return false;
            }
            *previous = Arc::new(rings);
            true
        });
    }

    pub fn rings(&self) -> Arc<Vec<RingsOn>> {
        self.0.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<Arc<Vec<RingsOn>>> {
        self.0.subscribe()
    }
}
