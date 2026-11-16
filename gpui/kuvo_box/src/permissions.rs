//! What macOS allows the program a Mac's box runs under (`kuvo/box/permissions`, PROTOCOL.md,
//! "What macOS allows"): the box's facts and words, for a client to draw. The box says each
//! permission's name, the address of its list in System Settings, whether you add the program
//! there by hand, and whether to bring it up now; a client names none itself.

use serde::Deserialize;
use serde_json::json;

use crate::client::BoxClient;
use crate::error::Result;
use crate::transport::Request;

/// One permission, as the box says it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Permission {
    pub id: String,
    /// What System Settings calls it.
    pub name: String,
    /// What agents need it for, in a sentence (none from a box older than these words).
    #[serde(default)]
    pub line: Option<String>,
    /// Whether it's allowed; none while the box can't say.
    pub allowed: Option<bool>,
    /// The address that opens its list in System Settings, on the box's own Mac.
    pub settings: String,
    /// macOS asks nothing for it: you add the program to the list yourself.
    #[serde(default)]
    pub by_hand: bool,
    /// Whether a client brings it up now (not allowed, and not put off).
    #[serde(default)]
    pub asks: bool,
}

/// The box's answer.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub struct Permissions {
    /// Whether the box runs under such a program at all (a Mac's own box).
    #[serde(default)]
    pub applies: bool,
    /// The program's name, as System Settings lists it ("Kuvo Helper").
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub known: bool,
    /// The program's bundle on that Mac: what's dragged into a list in System Settings.
    #[serde(default)]
    pub bundle: Option<String>,
    /// Each permission in words (none from a box older than these words).
    #[serde(default)]
    pub list: Vec<Permission>,
}

impl Permissions {
    /// The first permission the box says to bring up now.
    pub fn asked(&self) -> Option<&Permission> {
        self.list.iter().find(|permission| permission.asks)
    }

    pub fn of(&self, id: &str) -> Option<&Permission> {
        self.list.iter().find(|permission| permission.id == id)
    }
}

impl BoxClient {
    /// What macOS allows the program this box runs under.
    pub fn permissions(&self) -> Result<Permissions> {
        self.get("kuvo/box/permissions")
    }

    /// Puts a permission off ("Not Now"): the box keeps it, so no client brings it up again
    /// until it's been allowed and taken away.
    pub fn permission_later(&self, id: &str) -> Result<Permissions> {
        self.value(&Request::post("kuvo/box/permissions", json!({ "later": id })))
    }
}
