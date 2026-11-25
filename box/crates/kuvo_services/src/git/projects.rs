//! The box's projects (`areas/projects.ts`), as every client reads them: each project as
//! `box/projects.json` keeps it, and the project each worktree's folder and each project's other
//! folders are filed under.

use std::collections::BTreeMap;
use std::path::Path;

use kuvo_store::format::{self, ObjectFormat};
use serde_json::{Map, Value};

use super::trees::LinkedTrees;

const PROJECT_LIST: &str = "Project list";
/// The project list's format: one step, the first.
const PROJECT_LIST_VERSION: u32 = 1;
const WORKTREE_RECORDS: ObjectFormat = ObjectFormat::additive("Worktree records");

/// A record this box can't read: one written by a newer box.
pub type Unsupported = format::Error;

/// The projects `box/projects.json` keeps, each with a full path for its folder, its other folders
/// those named by text, and without the retired terminal setting. A file that can't be read is
/// none; one in a newer format is refused.
pub fn read_projects(box_home: &Path) -> Result<Vec<Map<String, Value>>, Unsupported> {
    let Ok(bytes) = std::fs::read(box_home.join("box/projects.json")) else {
        return Ok(Vec::new());
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(Vec::new());
    };
    let Value::Object(fields) = value else {
        return Ok(Vec::new());
    };
    format::check_version(PROJECT_LIST, fields.get("version"), 0, PROJECT_LIST_VERSION)?;
    let Some(Value::Array(list)) = fields.get("projects") else {
        return Ok(Vec::new());
    };
    Ok(list
        .iter()
        .filter_map(Value::as_object)
        .filter(|each| {
            each.get("directory")
                .and_then(Value::as_str)
                .is_some_and(absolute)
        })
        .map(|each| {
            let mut project = each.clone();
            project.remove("terminal");
            // Removed or not, as the route says it: one marked any other way by hand reads as it's filed.
            if removed(&project) {
                project.insert("removed".into(), Value::Bool(true));
            } else {
                project.remove("removed");
            }
            let folders = match each.get("folders") {
                Some(Value::Array(folders)) => folders
                    .iter()
                    .filter(|folder| folder.is_string())
                    .cloned()
                    .collect(),
                _ => Vec::new(),
            };
            project.insert("folders".into(), Value::Array(folders));
            project
        })
        .collect())
}

/// Why the projects weren't changed.
#[derive(Debug, thiserror::Error)]
pub enum ChangeFailed {
    #[error(transparent)]
    Unsupported(#[from] Unsupported),
    #[error("Could not lock the file for writing")]
    Busy,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// The list as `change` makes it, read again just before, under its lock (`box/projects.json.lock`,
/// shared with the TS box); the session list sees the file change.
pub fn change_projects<T>(
    box_home: &Path,
    change: impl FnOnce(&mut Vec<Map<String, Value>>) -> T,
) -> Result<T, ChangeFailed> {
    let file = box_home.join("box/projects.json");
    let _lock = kuvo_store::locks::StaleLock::acquire(
        &box_home.join("box/projects.json.lock"),
        std::time::Duration::from_secs(10),
        std::time::Duration::from_secs(3),
        kuvo_store::locks::OnBusy::Refuse,
    )?
    .ok_or(ChangeFailed::Busy)?;
    let mut projects = read_projects(box_home)?;
    let made = change(&mut projects);
    let kept = serde_json::json!({ "version": PROJECT_LIST_VERSION, "projects": projects });
    let text = serde_json::to_string_pretty(&kept).map_err(std::io::Error::other)?;
    kuvo_store::files::write_secret(&file, text.as_bytes())?;
    Ok(made)
}

/// Whether a folder is named by a full path, as Node's `path.isAbsolute` reads it here: on Windows,
/// one that starts at a drive's top (`\\work`, `/work`) is too.
fn absolute(folder: &str) -> bool {
    if cfg!(windows) {
        let bytes = folder.as_bytes();
        let separator = |byte: u8| byte == b'/' || byte == b'\\';
        bytes.first().is_some_and(|byte| separator(*byte))
            || (bytes.len() > 2
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && separator(bytes[2]))
    } else {
        Path::new(folder).is_absolute()
    }
}

fn directory(project: &Map<String, Value>) -> &str {
    project
        .get("directory")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// Whether a project was removed, as JavaScript reads `project.removed` (truthy).
fn removed(project: &Map<String, Value>) -> bool {
    match project.get("removed") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(removed)) => *removed,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Some(Value::String(text)) => !text.is_empty(),
        Some(_) => true,
    }
}

/// The projects' folders whose worktrees are looked for: those not removed.
pub(crate) fn kept_directories(box_home: &Path) -> Vec<String> {
    read_projects(box_home)
        .unwrap_or_default()
        .iter()
        .filter(|project| !removed(project))
        .map(|project| directory(project).to_owned())
        .collect()
}

/// The project each worktree's folder belongs to (`worktreeProjects`): the projects' linked
/// worktrees, then this box's own records (handed over, removed, restorable and kept).
fn worktree_projects(
    box_home: &Path,
    linked: &LinkedTrees,
) -> Result<BTreeMap<String, String>, Unsupported> {
    let kept = match WORKTREE_RECORDS.read(&box_home.join("box/worktrees.json")) {
        Ok(record) => record.map(|record| record.fields).unwrap_or_default(),
        Err(error @ format::Error::Unsupported { .. }) => return Err(error),
        Err(_) => Map::new(),
    };
    let mut projects = linked.as_last_found(|| kept_directories(box_home));
    for named in ["handedOver", "removed"] {
        if let Some(Value::Object(folders)) = kept.get(named) {
            for (folder, project) in folders {
                if let Some(project) = project.as_str() {
                    projects.insert(folder.clone(), project.to_owned());
                }
            }
        }
    }
    let records = |named: &str| -> Vec<(String, Option<String>, String)> {
        let Some(Value::Array(list)) = kept.get(named) else {
            return Vec::new();
        };
        list.iter()
            .filter_map(|tree| {
                let root = tree.get("root")?.as_str()?.to_owned();
                let project = tree.get("project")?.as_str()?.to_owned();
                Some((
                    root,
                    tree.get("folder")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    project,
                ))
            })
            .collect()
    };
    for (root, folder, project) in records("restorable") {
        projects.insert(folder.unwrap_or(root), project);
    }
    for (root, folder, project) in records("worktrees") {
        projects.insert(root.clone(), project.clone());
        projects.insert(folder.unwrap_or(root), project);
    }
    Ok(projects)
}

/// The project each folder sessions run in is filed under (`filedProjects`): a worktree's, and each
/// project's other folders (not one that's a project of its own).
fn filed_projects(
    projects: &[Map<String, Value>],
    trees: BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let kept: Vec<&Map<String, Value>> = projects
        .iter()
        .filter(|project| !removed(project))
        .collect();
    let own: std::collections::HashSet<&str> =
        kept.iter().map(|project| directory(project)).collect();
    let mut filed = BTreeMap::new();
    for project in &kept {
        let Some(Value::Array(folders)) = project.get("folders") else {
            continue;
        };
        for folder in folders.iter().filter_map(Value::as_str) {
            if !own.contains(folder) && !filed.contains_key(folder) {
                filed.insert(folder.to_owned(), directory(project).to_owned());
            }
        }
    }
    // (A worktree of a project's other folder's repository is that project's too.)
    let trees: Vec<(String, String)> = trees
        .into_iter()
        .map(|(folder, project)| {
            let filed_as = filed.get(&project).cloned().unwrap_or(project);
            (folder, filed_as)
        })
        .collect();
    filed.extend(trees);
    filed
}

/// Projects' names as every client shows them (`projectNameOf`), worktrees as last found.
#[derive(Clone, Default)]
pub struct ProjectNames(LinkedTrees);

impl ProjectNames {
    /// The name of the project `folder` is in: its own, or its folder's; a worktree's or another
    /// folder's is its project's, and so is that of a session filed under it from there (`filed`,
    /// the session's `kuvo.filed`, while it's in that folder).
    pub fn name_of(&self, box_home: &Path, folder: &str, filed: &Value) -> Option<String> {
        if folder.is_empty() {
            return None;
        }
        let projects = read_projects(box_home).unwrap_or_default();
        let trees = worktree_projects(box_home, &self.0).unwrap_or_default();
        let under = filed_projects(&projects, trees).remove(folder);
        let project = match (under, filed["project"].as_str()) {
            (None, Some(project)) if filed["folder"] == folder => project.to_owned(),
            (under, _) => under.unwrap_or_else(|| folder.to_owned()),
        };
        let named = projects.iter().find(|each| directory(each) == project);
        let name = named
            .and_then(|each| each.get("name"))
            .and_then(Value::as_str);
        name.filter(|name| !name.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                Path::new(&project)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
    }
}

/// `GET kuvo/projects`: `{projects, worktrees}`.
pub(crate) fn listed(box_home: &Path, linked: &LinkedTrees) -> Result<Value, Unsupported> {
    let projects = read_projects(box_home)?;
    let worktrees = filed_projects(&projects, worktree_projects(box_home, linked)?);
    Ok(serde_json::json!({ "projects": projects, "worktrees": worktrees }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn project(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn other_folders_are_filed_under_their_project_and_worktrees_follow_them() {
        let projects = [
            project(json!({ "directory": "/a", "folders": ["/a-web", "/b", "/shared"] })),
            project(json!({ "directory": "/b", "folders": ["/shared"] })),
            project(json!({ "directory": "/gone", "folders": ["/gone-web"], "removed": true })),
        ];
        let trees = BTreeMap::from([
            ("/t/a-web".to_owned(), "/a-web".to_owned()),
            ("/t/b".to_owned(), "/b".to_owned()),
        ]);
        let filed = filed_projects(&projects, trees);
        assert_eq!(
            filed,
            BTreeMap::from([
                ("/a-web".to_owned(), "/a".to_owned()),
                ("/shared".to_owned(), "/a".to_owned()),
                ("/t/a-web".to_owned(), "/a".to_owned()),
                ("/t/b".to_owned(), "/b".to_owned()),
            ])
        );
    }

    #[test]
    fn the_list_as_kept_and_a_newer_one_refused() {
        let home = tempfile::tempdir().unwrap();
        let write = |value: Value| {
            std::fs::create_dir_all(home.path().join("box")).unwrap();
            std::fs::write(home.path().join("box/projects.json"), value.to_string()).unwrap();
        };
        assert!(read_projects(home.path()).unwrap().is_empty());
        write(json!({ "version": 1, "projects": [
            { "directory": "/a", "terminal": "x", "folders": ["/b", 3], "run": "make" },
            { "directory": "relative" },
            { "directory": "/c" },
        ] }));
        assert_eq!(
            read_projects(home.path()).unwrap(),
            [
                project(json!({ "directory": "/a", "folders": ["/b"], "run": "make" })),
                project(json!({ "directory": "/c", "folders": [] }))
            ]
        );
        write(json!({ "version": 2, "projects": [] }));
        assert_eq!(
            read_projects(home.path()).unwrap_err().to_string(),
            "Project list format version 2 is unsupported (this build writes 1)."
        );
    }
}
