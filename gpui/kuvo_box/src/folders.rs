//! A box's folders as a Files panel shows them (PROTOCOL.md, `kuvo/folders`, `folders.files`): one
//! folder at a time, its folders and its files in the order the box gives, and why one can't be
//! read; and a file as a viewer shows it: its text, or that it isn't text (`kuvo/file/text`).

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;

use crate::client::BoxClient;
use crate::error::{Error, Refusal, Result};
use crate::parts::File;
use crate::paths;
use crate::transport::Request;

pub const FOLDER_PATHS: &str = "folders.paths";
pub const FILE_TEXT_PATH: &str = "file.text.path";

/// One folder, as a Files panel lists it: its folders (version control's own left out) and its
/// files, hidden ones too, in the order the box gives.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Folder {
    pub path: String,
    /// The folder it's in, as the box says; none for the top one.
    pub parent: Option<String>,
    pub folders: Vec<Named>,
    pub files: Vec<Named>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Named {
    pub name: String,
    #[serde(default)]
    pub path: String,
    /// A git repository's own checkout.
    #[serde(default)]
    pub repository: bool,
}

#[derive(Deserialize)]
struct ListedFolder {
    path: String,
    #[serde(default)]
    parent: Option<String>,
    #[serde(default)]
    folders: Vec<Named>,
    #[serde(default)]
    files: Vec<String>,
    #[serde(rename = "fileEntries")]
    file_entries: Option<Vec<Named>>,
}

/// A folder worth starting in, as the box suggests it (`kuvo/folders/suggested`,
/// `folders.suggested`): its full path on the box, its name, whether it's a git repository, and
/// when an agent last worked there (ms; none for a repository found in ~/Projects or ~/Developer).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Suggested {
    pub path: String,
    pub name: String,
    #[serde(default)]
    pub repository: bool,
    /// (Read whether the box writes it whole or with a fraction, as a file's time may have.)
    #[serde(default, deserialize_with = "milliseconds")]
    pub seen: Option<u64>,
}

fn milliseconds<'de, D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Option<u64>, D::Error> {
    Ok(Option::<f64>::deserialize(deserializer)?.filter(|ms| ms.is_finite() && *ms >= 0.0).map(|ms| ms as u64))
}

/// The folders worth starting in on a box's computer, in its order (those worked in first, the
/// latest first), and its home folder, for showing a path from it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SuggestedFolders {
    pub home: String,
    pub folders: Vec<Suggested>,
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
    /// The folders worth starting in on this box's computer, as it suggests them.
    pub fn suggested_folders(&self) -> Result<SuggestedFolders> {
        self.get("kuvo/folders/suggested")
    }

    /// The folders in the folder at `path` (`~`: the home folder), as a folder chooser lists them:
    /// no files, and none of the hidden ones. Refused as [`BoxClient::folder`] is.
    pub fn folders_in(&self, path: &str) -> std::result::Result<Folder, std::result::Result<Unreadable, Error>> {
        self.read_folder(format!("kuvo/folders?path={}", paths::query_value(path)), false)
    }

    /// The folder at `path` (a full one, on the box), as a Files panel lists it. A folder the box
    /// can't read is `Err(Ok(why))`; one it couldn't be asked about, `Err(Err(error))`.
    pub fn folder(&self, path: &str) -> std::result::Result<Folder, std::result::Result<Unreadable, Error>> {
        self.read_folder(format!("kuvo/folders?path={}&files=1&hidden=1", paths::query_value(path)), true)
    }

    fn read_folder(&self, route: String, with_files: bool) -> std::result::Result<Folder, std::result::Result<Unreadable, Error>> {
        let has_paths = self.has_feature(FOLDER_PATHS).map_err(Err)?;
        let response = self.send(&Request::get(route)).map_err(Err)?;
        if response.is_success() {
            let ListedFolder { path, parent, mut folders, files, file_entries } = serde_json::from_slice(&response.body)
                .map_err(|error| Err(Error::BadAnswer(format!("The answer couldn't be read: {error}"))))?;
            let files = if has_paths {
                if (with_files && file_entries.is_none()) || folders.iter().chain(file_entries.iter().flatten()).any(|entry| entry.path.is_empty()) {
                    return Err(Err(Error::BadAnswer("The box didn’t give a path for each entry.".into())));
                }
                file_entries.unwrap_or_default()
            } else {
                let inside = |name: &str| format!("{}/{name}", path.trim_end_matches('/'));
                for entry in &mut folders {
                    entry.path = inside(&entry.name);
                }
                files.into_iter().map(|name| Named { path: inside(&name), name, repository: false }).collect()
            };
            return Ok(Folder { path, parent, folders, files });
        }
        let refusal = Refusal::from_answer(response.status, &response.body);
        if matches!(response.status, 403 | 404 | 500) && !refusal.older_box {
            return Err(Ok(Unreadable { message: refusal.message, reason: refusal.reason }));
        }
        Err(Err(Error::Refused(refusal)))
    }

    /// A file as a viewer shows it, at most [`SHOWN_MOST`] of it (a `413` refusal past it).
    pub fn read_text(&self, path: &str) -> Result<FileText> {
        if self.has_feature(FILE_TEXT_PATH)? {
            return self.get(&format!("kuvo/file/text?path64={}&max={SHOWN_MOST}", URL_SAFE_NO_PAD.encode(path)));
        }
        let uri = File::at(std::path::Path::new(path)).uri;
        self.get(&format!("kuvo/file/text?uri={}&max={SHOWN_MOST}", paths::query_value(&uri)))
    }
}
