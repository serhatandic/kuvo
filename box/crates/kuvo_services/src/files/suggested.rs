//! Folders worth starting in (PROTOCOL.md, `kuvo/folders/suggested`): where this computer's agents
//! worked lately, newest first, then the git repositories directly in ~/Projects and ~/Developer.
//! Fast and bounded: one level of two folders, a few hundred entries read at most.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use kuvo_os::platform::Platform;
use serde::{Deserialize, Serialize};

use super::address::resolved;
use super::folders::Places;
use super::order::FinderOrder;

/// How many folders are suggested at most, and how many of them from the sessions.
const MOST_SUGGESTED: usize = 20;
const MOST_WORKED: usize = 12;
/// The folders looked in for repositories, from the home folder, one level each.
const LOOKED_IN: &[&str] = &["Projects", "Developer"];
/// How many entries of each are read at most.
const MOST_READ: usize = 500;

/// A session, as far as this goes: its kind, its project's folder and its own, when it last changed.
#[derive(Deserialize)]
pub(crate) struct WorkedIn {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    project: Option<serde_json::Value>,
    #[serde(default)]
    directory: Option<serde_json::Value>,
    #[serde(default)]
    updated: f64,
    #[serde(default)]
    archived: Option<bool>,
}

#[derive(Deserialize)]
struct Kept {
    #[serde(default)]
    sessions: InOrder,
}

/// A JSON object's values in the order they're written, as JavaScript's `Object.values` reads them.
#[derive(Default)]
struct InOrder(Vec<KeptSession>);

impl<'de> Deserialize<'de> for InOrder {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Values;
        impl<'de> serde::de::Visitor<'de> for Values {
            type Value = InOrder;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("an object of sessions")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<InOrder, A::Error> {
                let mut values = Vec::new();
                while let Some((_, value)) =
                    map.next_entry::<serde::de::IgnoredAny, KeptSession>()?
                {
                    values.push(value);
                }
                Ok(InOrder(values))
            }
        }
        deserializer.deserialize_map(Values)
    }
}

#[derive(Deserialize)]
struct KeptSession {
    facts: WorkedIn,
}

/// This computer's sessions, as its session list keeps them (`run/lists/sessions.json`).
pub(crate) fn kept_sessions(box_home: &Path) -> Vec<WorkedIn> {
    fs::read(box_home.join("run/lists/sessions.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Kept>(&bytes).ok())
        .map(|kept| {
            kept.sessions
                .0
                .into_iter()
                .map(|session| session.facts)
                .collect()
        })
        .unwrap_or_default()
}

/// A folder suggested.
#[derive(Serialize, Debug, PartialEq)]
pub(crate) struct Suggested {
    path: String,
    name: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    repository: bool,
    seen: Option<f64>,
}

/// A folder as the system names it (links resolved), or as given where it can't say.
pub(crate) fn located(folder: &Path) -> PathBuf {
    fs::canonicalize(folder).unwrap_or_else(|_| resolved(folder))
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Whether `at` is inside `folder` (or is it).
fn within(at: &str, folder: &str) -> bool {
    let sep = std::path::MAIN_SEPARATOR;
    at == folder
        || at.starts_with(&if folder.ends_with(sep) {
            folder.to_owned()
        } else {
            format!("{folder}{sep}")
        })
}

fn is_folder(at: &Path) -> bool {
    fs::metadata(at).is_ok_and(|m| m.is_dir())
}

fn is_repository(at: &Path) -> bool {
    at.join(".git").exists()
}

fn name_of(at: &str) -> String {
    Path::new(at)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The folders suggested, from `sessions` and what's in `home`'s ~/Projects and ~/Developer.
/// `box_home`: this box's own folder, whose sessions' folders aren't yours to start in; `chats`,
/// its chats folder.
pub(crate) fn suggested_folders(
    home: &Path,
    sessions: &[WorkedIn],
    box_home: &Path,
    chats: &Path,
    platform: Platform,
) -> Vec<Suggested> {
    // Platform: on a Mac, looking into these would have macOS ask you first, as it does of another
    // disk (`/Volumes`): a folder there an agent worked in is suggested as it is, not looked into.
    let real_home = located(home);
    let mut prompted: Vec<String> = platform
        .prompted_folders()
        .iter()
        .flat_map(|name| [text(&home.join(name)), text(&real_home.join(name))])
        .collect();
    if platform == Platform::Mac {
        prompted.push("/Volumes".to_owned());
    }
    let asks = |at: &str| prompted.iter().any(|folder| within(at, folder));
    let (home_text, real_home_text) = (text(home), text(&real_home));
    let (box_text, real_box) = (text(box_home), text(&located(box_home)));
    let real = |at: &str| {
        if asks(at) {
            at.to_owned()
        } else {
            text(&located(Path::new(at)))
        }
    };
    let chats_text = text(chats);
    let is_chats = |at: &str| at == chats_text || located(Path::new(at)) == located(chats);

    let mut latest: Vec<(String, f64)> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for session in sessions {
        let absolute = |value: &Option<serde_json::Value>| {
            value
                .as_ref()
                .and_then(|v| v.as_str())
                .filter(|v| Path::new(v).is_absolute())
                .map(str::to_owned)
        };
        let folder = absolute(&session.project).or_else(|| {
            session
                .directory
                .as_ref()
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        });
        let Some(folder) = folder.filter(|f| Path::new(f).is_absolute()) else {
            continue;
        };
        if session.kind != "project" || session.archived == Some(true) {
            continue;
        }
        let at = real(&text(&resolved(Path::new(&folder))));
        let root = Path::new(&at)
            .ancestors()
            .last()
            .map(text)
            .unwrap_or_default();
        if at == real_home_text
            || at == home_text
            || at == root
            || within(&at, &real_box)
            || within(&at, &box_text)
            || (!asks(&at) && is_chats(&at))
        {
            continue;
        }
        match index.get(&at) {
            Some(&row) if latest[row].1 < session.updated => latest[row].1 = session.updated,
            Some(_) => {}
            None => {
                index.insert(at.clone(), latest.len());
                latest.push((at, session.updated));
            }
        }
    }
    latest.sort_by(|a, b| b.1.total_cmp(&a.1));

    let mut suggested = Vec::new();
    let mut taken = HashSet::new();
    for (at, seen) in latest {
        if suggested.len() >= MOST_WORKED {
            break;
        }
        let unasked = !asks(&at);
        // (One gone since isn't suggested; one the system asks about first is taken as there.)
        if unasked && !is_folder(Path::new(&at)) {
            continue;
        }
        let repository = unasked && is_repository(Path::new(&at));
        suggested.push(Suggested {
            name: name_of(&at),
            path: at.clone(),
            repository,
            seen: Some(seen),
        });
        taken.insert(at);
    }

    let mut found: Vec<(String, f64)> = Vec::new();
    for top in LOOKED_IN {
        let folder = real_home.join(top);
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.take(MOST_READ) {
            let Ok(entry) = entry else { break };
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let at = folder.join(&name);
            let kind = entry.file_type().ok();
            // A link is followed only where it doesn't lead somewhere macOS asks about.
            if kind.is_some_and(|kind| kind.is_symlink()) {
                let Ok(to) = fs::read_link(&at) else { continue };
                if asks(&text(&resolved(&folder.join(to)))) {
                    continue;
                }
            }
            let is_directory = kind.is_some_and(|kind| kind.is_dir())
                || (!kind.is_some_and(|kind| kind.is_file()) && is_folder(&at));
            if taken.contains(&text(&located(&at))) || !is_directory || !is_repository(&at) {
                continue;
            }
            let changed = fs::metadata(&at)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(0.0, |since| since.as_nanos() as f64 / 1e6);
            found.push((text(&at), changed));
        }
    }
    // The repositories found, the one changed latest first.
    let order = FinderOrder::plain();
    found.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| order.compare(&a.0, &b.0)));
    for (at, _) in found {
        if suggested.len() >= MOST_SUGGESTED {
            break;
        }
        suggested.push(Suggested {
            name: name_of(&at),
            path: at,
            repository: true,
            seen: None,
        });
    }
    suggested
}

/// `GET kuvo/folders/suggested`: `{home, folders}`, for this computer.
pub(crate) fn answer(places: &Places) -> serde_json::Value {
    // As the system names it, as every folder suggested is.
    let home = located(&places.home);
    let sessions = kept_sessions(&places.box_home);
    let folders = suggested_folders(
        &home,
        &sessions,
        &places.box_home,
        &places.box_home.join("chats"),
        places.platform,
    );
    serde_json::json!({ "home": home, "folders": folders })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn folder(home: &Path, at: &str, repository: bool) -> String {
        let path = home.join(at);
        fs::create_dir_all(&path).unwrap();
        if repository {
            fs::create_dir(path.join(".git")).unwrap();
        }
        text(&path)
    }

    fn worked(kind: &str, directory: &str, updated: f64) -> WorkedIn {
        WorkedIn {
            kind: kind.into(),
            project: None,
            directory: Some(directory.into()),
            updated,
            archived: None,
        }
    }

    fn row(path: &str, repository: bool, seen: Option<f64>) -> Suggested {
        Suggested {
            path: path.into(),
            name: name_of(path),
            repository,
            seen,
        }
    }

    #[test]
    fn worked_in_first_newest_first_then_repositories() {
        let scratch_dir = tempfile::tempdir().unwrap();
        let home = located(scratch_dir.path());
        let website = folder(&home, "Projects/website", true);
        let notes = folder(&home, "Documents/notes", true);
        let api = folder(&home, "work/api", true);
        let scratch = folder(&home, "scratch", false);
        folder(&home, "Projects/plain", false);
        folder(&home, "Projects/.hidden", true);
        let dotfiles = folder(&home, "Developer/dotfiles", true);
        folder(&home, "Projects/deep/inner", true);
        let linked = folder(&home, "Developer/linked", false);
        fs::write(
            Path::new(&linked).join(".git"),
            "gitdir: ../dotfiles/.git/worktrees/linked\n",
        )
        .unwrap();
        fs::write(home.join("Projects/README.md"), "not a folder").unwrap();
        folder(&home, "Documents/secret", true);
        symlink(
            home.join("Documents/secret"),
            home.join("Projects/secret-link"),
        )
        .unwrap();
        let box_home = PathBuf::from(folder(&home, ".kuvo-code", false));
        let chats = PathBuf::from(folder(&home, "AppChats", false));
        let chats_text = text(&chats);
        let mut sessions = vec![
            worked("project", &website, 300.0),
            worked("project", &scratch, 500.0),
            worked("project", &format!("{website}/"), 100.0),
            WorkedIn {
                project: Some(api.clone().into()),
                ..worked("project", &text(&box_home.join("worktree/api-1")), 450.0)
            },
            WorkedIn {
                archived: Some(true),
                ..worked("project", &api, 200.0)
            },
            worked("bot", &dotfiles, 900.0),
            worked("subagent", &dotfiles, 900.0),
            worked("chat", &chats_text, 950.0),
            worked("project", &chats_text, 940.0),
            worked("project", &text(&box_home.join("chats/c1")), 800.0),
            worked("project", &text(&home), 700.0),
            worked("project", &text(&home.join("gone")), 600.0),
            worked("project", &notes, 50.0),
        ];
        let suggested = suggested_folders(&home, &sessions, &box_home, &chats, Platform::Mac);
        assert_eq!(
            suggested[..4],
            [
                row(&scratch, false, Some(500.0)),
                row(&api, true, Some(450.0)),
                row(&website, true, Some(300.0)),
                row(&notes, false, Some(50.0))
            ]
        );
        let mut found: Vec<(String, bool, Option<f64>)> = suggested[4..]
            .iter()
            .map(|s| (s.name.clone(), s.repository, s.seen))
            .collect();
        found.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            found,
            [
                ("dotfiles".into(), true, None),
                ("linked".into(), true, None)
            ]
        );
        let on_linux = suggested_folders(&home, &sessions, &box_home, &chats, Platform::Linux);
        assert_eq!(
            on_linux.iter().find(|s| s.path == notes),
            Some(&row(&notes, true, Some(50.0)))
        );
        assert!(on_linux.iter().any(|s| s.name == "secret-link"));
        sessions.clear();
        assert!(
            suggested_folders(&home, &sessions, &box_home, &chats, Platform::Linux)
                .iter()
                .all(|s| s.seen.is_none())
        );
    }

    #[test]
    fn a_home_reached_through_a_link_names_each_folder_once() {
        let base = tempfile::tempdir().unwrap();
        let base = located(base.path());
        let real = base.join("var-home");
        let linked_home = base.join("home");
        fs::create_dir_all(real.join("Projects/app/.git")).unwrap();
        fs::create_dir_all(real.join("Developer/web/.git")).unwrap();
        symlink(&real, &linked_home).unwrap();
        let sessions = [
            worked("project", &text(&real.join("Projects/app")), 10.0),
            worked("project", &text(&real), 20.0),
            worked("project", &text(&real.join(".kuvo-code/worktree/x")), 30.0),
        ];
        let box_home = linked_home.join(".kuvo-code");
        assert_eq!(
            suggested_folders(
                &linked_home,
                &sessions,
                &box_home,
                &box_home.join("chats"),
                Platform::Linux
            ),
            [
                row(&text(&real.join("Projects/app")), true, Some(10.0)),
                row(&text(&real.join("Developer/web")), true, None)
            ]
        );
    }

    #[test]
    fn many_folders_a_bounded_list_nothing_deeper() {
        let many = tempfile::tempdir().unwrap();
        let many = located(many.path());
        for at in 0..3000 {
            fs::create_dir_all(many.join(format!("Projects/repo-{at}/.git"))).unwrap();
            if at < 50 {
                fs::create_dir_all(many.join(format!("Projects/repo-{at}/nested/.git"))).unwrap();
            }
        }
        for at in 0..200 {
            fs::create_dir_all(many.join(format!("Developer/plain-{at}"))).unwrap();
        }
        let sessions: Vec<WorkedIn> = (0..5000)
            .map(|at| {
                worked(
                    "project",
                    &text(&many.join(format!("Projects/repo-{}", at % 3000))),
                    at as f64,
                )
            })
            .collect();
        let box_home = many.join(".kuvo-code");
        let suggested = suggested_folders(
            &many,
            &sessions,
            &box_home,
            &box_home.join("chats"),
            Platform::Linux,
        );
        assert_eq!(suggested.len(), MOST_SUGGESTED);
        assert_eq!(
            suggested[0],
            row(&text(&many.join("Projects/repo-1999")), true, Some(4999.0))
        );
        assert_eq!(suggested.iter().filter(|s| s.seen.is_some()).count(), 12);
        assert_eq!(
            suggested
                .iter()
                .map(|s| &s.path)
                .collect::<HashSet<_>>()
                .len(),
            MOST_SUGGESTED
        );
        assert!(suggested.iter().all(|s| !s.path.contains("nested")));
    }

    #[test]
    fn nothing_to_suggest() {
        let empty = tempfile::tempdir().unwrap();
        let empty = located(empty.path());
        let box_home = empty.join(".kuvo-code");
        assert!(
            suggested_folders(
                &empty,
                &[],
                &box_home,
                &box_home.join("chats"),
                Platform::Linux
            )
            .is_empty()
        );
    }
}
