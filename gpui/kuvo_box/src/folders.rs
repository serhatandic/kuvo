//! A box's folders as a Files panel shows them (PROTOCOL.md, `kuvo/folders`, `folders.files`): one
//! folder at a time, its folders and its files in the order the box gives, and why one can't be
//! read; and a file as a viewer shows it: its text, or that it isn't text (`kuvo/file/text`).

use serde::Deserialize;

use crate::client::BoxClient;
use crate::error::{Error, Refusal, Result};
use crate::parts::File;
use crate::paths;
use crate::transport::Request;

/// One folder, as a Files panel lists it: its folders (version control's own left out) and its
/// files, hidden ones too, in the order the box gives.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct Folder {
    pub path: String,
    #[serde(default)]
    pub folders: Vec<Named>,
    #[serde(default)]
    pub files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Named {
    pub name: String,
    /// A git repository's own checkout.
    #[serde(default)]
    pub repository: bool,
}

/// Why a folder can't be read, as the box says it: what to show, and the computer's own reason
/// ("Permission denied", "No such folder").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unreadable {
    pub message: String,
    pub reason: Option<String>,
}

/// A file as a viewer shows it (`kuvo/file/text`, `file.text`): the box says whether it's text.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FileText {
    /// Whether it's text at all (a picture, a program or another encoding isn't).
    pub text: bool,
    /// Its text, where it is.
    #[serde(default)]
    pub content: String,
}

/// The most of a file a viewer reads (`kuvo/file/text`'s `max`): past it, the box says it's too big.
pub const SHOWN_MOST: u64 = 2 * 1024 * 1024;

impl BoxClient {
    /// The folder at `path` (a full one, on the box), as a Files panel lists it. A folder the box
    /// can't read is `Err(Ok(why))`; one it couldn't be asked about, `Err(Err(error))`.
    pub fn folder(&self, path: &str) -> std::result::Result<Folder, std::result::Result<Unreadable, Error>> {
        let route = format!("kuvo/folders?path={}&files=1&hidden=1", paths::query_value(path));
        let response = self.send(&Request::get(route)).map_err(Err)?;
        if response.is_success() {
            return serde_json::from_slice(&response.body).map_err(|error| Err(Error::BadAnswer(format!("The answer couldn't be read: {error}"))));
        }
        let refusal = Refusal::from_answer(response.status, &response.body);
        if matches!(response.status, 403 | 404 | 500) && !refusal.older_box {
            return Err(Ok(Unreadable { message: refusal.message, reason: refusal.reason }));
        }
        Err(Err(Error::Refused(refusal)))
    }

    /// A file as a viewer shows it, at most [`SHOWN_MOST`] of it (a `413` refusal past it).
    pub fn read_text(&self, path: &str) -> Result<FileText> {
        let uri = File::at(std::path::Path::new(path)).uri;
        self.get(&format!("kuvo/file/text?uri={}&max={SHOWN_MOST}", paths::query_value(&uri)))
    }
}
