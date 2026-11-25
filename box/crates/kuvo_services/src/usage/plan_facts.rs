//! Plan facts read from a maker, apart from the accepted time-window trail.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    Requests,
    Usd,
    Credits,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Amount {
    pub used: f64,
    pub limit: f64,
    pub unit: Unit,
}
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Balance {
    pub left: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,
    pub unit: Unit,
}
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Extra {
    Spent {
        id: String,
        name: String,
        amount: f64,
        unit: Unit,
        #[serde(rename = "resetsAt", skip_serializing_if = "Option::is_none")]
        resets_at: Option<u64>,
    },
    Capped {
        id: String,
        name: String,
        amount: f64,
        limit: f64,
        unit: Unit,
        #[serde(rename = "resetsAt", skip_serializing_if = "Option::is_none")]
        resets_at: Option<u64>,
    },
    Left {
        id: String,
        name: String,
        amount: f64,
        unit: Unit,
    },
    Resets {
        id: String,
        name: String,
        count: u32,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        expiries: Vec<u64>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        refills: Vec<String>,
        usable: bool,
    },
    Unreadable {
        id: String,
        name: String,
        message: String,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProblemKind {
    None,
    Ended,
    Expired,
    Unreadable,
    RateLimited,
    Failed,
}
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Problem {
    pub kind: ProblemKind,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<u64>,
    #[serde(default, skip_serializing_if = "false_value")]
    pub unknown: bool,
}
impl Problem {
    pub fn new(kind: ProblemKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            until: None,
            unknown: false,
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct Facts {
    /// A completed maker read that could not contain every pool.
    #[serde(default, skip_serializing_if = "false_value")]
    pub partial: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance: Option<Balance>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extras: Vec<Extra>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<Problem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<u64>,
}

fn false_value(value: &bool) -> bool {
    !*value
}

/// A Retry-After can be a delay or an HTTP date. A past date means now.
pub(super) fn retry_at(raw: &str, now: u64) -> Option<u64> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if raw.bytes().all(|b| b.is_ascii_digit()) {
        return Some(now.saturating_add(raw.parse::<u64>().ok()?.saturating_mul(1000)));
    }
    if let Ok(date) = httpdate::parse_http_date(raw) {
        let at = date
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        return Some(u64::try_from(at).ok()?.max(now));
    }
    let stamp = raw.parse::<jiff::Timestamp>().ok()?.as_millisecond();
    Some(u64::try_from(stamp).unwrap_or(0).max(now))
}
