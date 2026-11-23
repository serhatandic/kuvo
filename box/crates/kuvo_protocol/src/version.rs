//! Version markers shared across the protocol and its readers.

use serde::{Deserialize, Serialize};

pub const ROUTE_VERSION_HEADER: &str = "x-kuvo-route-version";
pub const ROUTE_VERSION: &str = "1";
pub const SERVE_RECORD_VERSION: u32 = 1;
pub const BRIDGE_RECORD_VERSION: u32 = 1;
pub const FOLDER_LAYOUT_VERSION: u32 = 23;
pub const LEGACY_FORMAT_VERSION: u32 = 0;
pub const ADDITIVE_FORMAT_VERSION: u32 = 1;

/// The refusal body for an unsupported route header or stored record.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct UnsupportedFormatVersion {
    pub code: FormatRefusalCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FormatRefusalCode {
    UnsupportedFormatVersion,
}

impl UnsupportedFormatVersion {
    /// Refusal for an explicit route header. An absent header is the legacy format.
    pub fn for_route(given: Option<&str>) -> Option<Self> {
        let given = given?;
        if given == ROUTE_VERSION {
            return None;
        }
        let found = if !given.is_empty()
            && given.len() <= 15
            && given.bytes().all(|byte| byte.is_ascii_digit())
        {
            given.trim_start_matches('0')
        } else {
            "invalid"
        };
        let found = if found.is_empty() { "0" } else { found };
        Some(Self {
            code: FormatRefusalCode::UnsupportedFormatVersion,
            message: format!(
                "Kuvo route format version {found} is unsupported (this build writes {ROUTE_VERSION})."
            ),
        })
    }
}
