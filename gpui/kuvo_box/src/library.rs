//! The Library (`kuvo/library`, PROTOCOL.md "Library"): what was handed over to you across your
//! computers, as the window's box gathers it. The box decides what's in it and labels each item with
//! the computer it's on; a client lists, filters, opens and removes.
use serde::Deserialize;
use serde_json::json;

use crate::error::{Error, Refusal, Result};
use crate::follow::{Followed, follow_lines};
use crate::paths::segment;
use crate::transport::Request;
use crate::{BoxClient, Follow};

/// The Library's tabs: what a thing is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LibraryKind {
    Image,
    Document,
    Page,
    #[serde(other)]
    Other,
}

impl LibraryKind {
    pub fn slug(self) -> &'static str {
        match self {
            LibraryKind::Image => "image",
            LibraryKind::Document => "document",
            LibraryKind::Page => "page",
            LibraryKind::Other => "other",
        }
    }
}

/// How it came: an agent showed it, generated it, or you attached it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Handed {
    Shown,
    Generated,
    #[serde(other)]
    Attached,
}

/// The computer an item is on.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ItemComputer {
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub here: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct LibraryItem {
    pub id: String,
    pub name: String,
    pub kind: LibraryKind,
    #[serde(rename = "type")]
    pub media_type: String,
    pub how: Handed,
    pub path: String,
    #[serde(default)]
    pub bot: Option<String>,
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub part: Option<String>,
    /// When it was handed over, in seconds since 1970.
    pub at: f64,
    /// Its file is on disk, or the record keeps a copy of it.
    pub there: bool,
    /// A thumbnail can be made of it.
    pub thumbnail: bool,
    #[serde(default)]
    pub computer: Option<ItemComputer>,
}

impl LibraryItem {
    /// On this computer (the window's box), not another of yours.
    pub fn is_here(&self) -> bool {
        self.computer.as_ref().is_none_or(|computer| computer.here)
    }

    /// Its route on the box that gathers the Library (`suffix`: `/file`, `/thumbnail`, `/hide`).
    fn route(&self, suffix: &str) -> String {
        let id = segment(&self.id);
        match &self.computer {
            Some(computer) if !computer.here => format!("kuvo/library/{id}{suffix}?computer={}", segment(&computer.key)),
            _ => format!("kuvo/library/{id}{suffix}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct SourceBot {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    /// Its look, as `kuvo/bots` gives it, to draw it by.
    #[serde(default)]
    pub look: Option<crate::areas::Look>,
    pub computer: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Sources {
    #[serde(default)]
    pub bots: Vec<SourceBot>,
    #[serde(default)]
    pub you: bool,
}

/// How a computer's part of the Library stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PartState {
    Here,
    There,
    Away,
    Older,
    #[serde(other)]
    Failed,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct PartComputer {
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub here: bool,
    pub state: PartState,
    #[serde(default)]
    pub message: Option<String>,
    /// When this box last received the away computer’s copy, in milliseconds since 1970.
    #[serde(default)]
    pub seen: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Library {
    #[serde(default, rename = "pendingComputers")]
    pub pending_computers: Vec<crate::areas::SetupComputer>,
    pub items: Vec<LibraryItem>,
    #[serde(default)]
    pub more: bool,
    #[serde(default)]
    pub sources: Sources,
    #[serde(default)]
    pub computers: Vec<PartComputer>,
}

/// What a list asks for: whose (`all`, `you` or a bot's id), what kind, and words.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LibraryQuery {
    pub source: Option<String>,
    pub kind: Option<LibraryKind>,
    pub words: String,
}

impl LibraryQuery {
    fn query(&self) -> String {
        let mut parts = Vec::new();
        if let Some(source) = &self.source {
            parts.push(format!("source={}", segment(source)));
        }
        if let Some(kind) = self.kind {
            parts.push(format!("kind={}", kind.slug()));
        }
        let words = self.words.trim();
        if !words.is_empty() {
            parts.push(format!("q={}", segment(words)));
        }
        if parts.is_empty() { String::new() } else { format!("?{}", parts.join("&")) }
    }
}

impl BoxClient {
    /// The Library across your computers, newest first.
    pub fn library(&self, query: &LibraryQuery) -> Result<Library> {
        self.get(&format!("kuvo/library{}", query.query()))
    }

    /// The Library now, then again as it changes.
    pub fn follow_library(&self, query: &LibraryQuery) -> Follow<Followed<Library>> {
        let query = query.query();
        follow_lines(self, format!("kuvo/setup/library/events{query}"))
    }

    /// Its thumbnail's bytes (a PNG, or the picture as it is).
    pub fn library_thumbnail(&self, item: &LibraryItem) -> Result<Vec<u8>> {
        self.library_bytes(item.route("/thumbnail"))
    }

    /// Its file's bytes, from the computer it's on.
    pub fn library_file(&self, item: &LibraryItem) -> Result<Vec<u8>> {
        self.library_bytes(item.route("/file"))
    }

    /// Removed from the Library (its file stays), or put back.
    pub fn hide_library_item(&self, item: &LibraryItem, hidden: bool) -> Result<()> {
        self.done(&Request::post(item.route("/hide"), json!({ "hidden": hidden })))
    }

    fn library_bytes(&self, route: String) -> Result<Vec<u8>> {
        let response = self.send(&Request::get(route))?;
        if response.is_success() {
            Ok(response.body)
        } else if response.status == 404 || response.status == 410 {
            Err(Error::NotFound)
        } else {
            Err(Error::Refused(Refusal::from_answer(response.status, &response.body)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_says_only_what_it_narrows_to() {
        assert_eq!(LibraryQuery::default().query(), "");
        let query = LibraryQuery { source: Some("bot_ada".into()), kind: Some(LibraryKind::Page), words: " board brief ".into() };
        assert_eq!(query.query(), "?source=bot_ada&kind=page&q=board%20brief");
    }

    #[test]
    fn another_computers_item_is_asked_of_it() {
        let item: LibraryItem = serde_json::from_value(serde_json::json!({
            "id": "lib_1", "name": "a.png", "kind": "image", "type": "image/png", "how": "generated", "path": "/a.png",
            "at": 1.0, "there": true, "thumbnail": true, "computer": { "key": "k/1", "name": "Studio" }
        }))
        .unwrap();
        assert!(!item.is_here());
        assert_eq!(item.route("/file"), "kuvo/library/lib_1/file?computer=k%2F1");
    }
}
