//! Finding a folder's files by name, and whether a text is a page to open (PROTOCOL.md,
//! `kuvo/files/find`, `files.find`): the side panel's "Filter files…" and its New tab page's
//! search. The box matches and ranks; this only asks.

use serde::Deserialize;

use crate::client::BoxClient;
use crate::error::Result;
use crate::paths;

pub const FILES_FIND: &str = "files.find";

/// The most a search asks for.
pub const FOUND_MOST: usize = 50;

/// A file found, best first: its full path on the box, its name, and the folder it's in under the
/// folder searched ("" at its top).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FoundFile {
    pub path: String,
    pub name: String,
    #[serde(default)]
    pub folder: String,
}

/// What a search found: the files, best first, and the page the text reads as, when it does.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct Found {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub files: Vec<FoundFile>,
}

impl BoxClient {
    /// The files under `directory` whose path has `query`, best first (at most [`FOUND_MOST`]), and
    /// the page `query` reads as.
    pub fn find_files(&self, directory: &str, query: &str) -> Result<Found> {
        self.get(&format!("kuvo/files/find?directory={}&q={}&limit={FOUND_MOST}", paths::query_value(directory), paths::query_value(query)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_search_reads_its_files_best_first_and_the_page_it_reads_as() {
        let found: Found = serde_json::from_str(r#"{"url":null,"files":[{"path":"/p/src/auth/signup.ts","name":"signup.ts","folder":"src/auth"},{"path":"/p/signup.md","name":"signup.md","folder":""}]}"#).unwrap();
        assert_eq!(found.files[0].folder, "src/auth");
        assert_eq!(found.url, None);
        let page: Found = serde_json::from_str(r#"{"url":"http://localhost:3000","files":[]}"#).unwrap();
        assert_eq!(page.url.as_deref(), Some("http://localhost:3000"));
    }
}
