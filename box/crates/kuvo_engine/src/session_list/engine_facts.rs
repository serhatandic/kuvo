//! List information stored in OpenCode's databases and Git worktree records.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

use rusqlite::{Connection, ErrorCode, OpenFlags};

use super::Observed;

const BUSY: Duration = Duration::from_secs(2);
const LEGACY: &str = "SELECT id, coalesce((SELECT time_created FROM message WHERE session_id = session.id ORDER BY time_created DESC LIMIT 1), time_created) FROM session";
const CURRENT: &str = "SELECT id, coalesce((SELECT time_created FROM session_message WHERE session_id = session_v2.id ORDER BY seq DESC LIMIT 1), time_created) FROM session_v2";
const WORKTREES: &str = "SELECT worktree.directory, project.worktree FROM worktree JOIN project ON project.id = worktree.project_id WHERE project.vcs = 'git'";

#[derive(Default)]
pub(super) struct EngineFacts {
    pub(super) databases: Arc<Databases>,
    linked: BTreeMap<PathBuf, PathBuf>,
}

#[derive(Default)]
pub(super) struct Databases {
    files: Vec<Stamp>,
    activity: BTreeMap<String, u64>,
    roots: BTreeMap<PathBuf, PathBuf>,
}

#[derive(PartialEq, Eq)]
struct Stamp {
    path: PathBuf,
    db: Option<(u64, SystemTime)>,
    wal: Option<(u64, SystemTime)>,
}

impl EngineFacts {
    pub(super) fn activity(&self, session: &str) -> Option<u64> {
        self.databases.activity.get(session).copied()
    }

    pub(super) fn worktree(&self, folder: &str) -> Option<&Path> {
        let folder = Path::new(folder);
        self.linked.get(folder).map(PathBuf::as_path).or_else(|| {
            folder
                .ancestors()
                .find_map(|root| self.databases.roots.get(root).map(PathBuf::as_path))
        })
    }

    pub(super) async fn add_linked<'a>(&mut self, rows: impl Iterator<Item = &'a Observed>) {
        let folders = rows
            .map(|row| Path::new(row.session.directory()))
            .filter(|folder| folder.is_absolute())
            .collect::<BTreeSet<_>>();
        let mut origins = BTreeMap::new();
        for folder in folders {
            let mut worktree = None;
            for ancestor in folder.ancestors() {
                match tokio::fs::symlink_metadata(ancestor.join(".git")).await {
                    Ok(marker) => {
                        worktree = marker.is_file().then_some(ancestor);
                        break;
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(_) => break,
                }
            }
            let Some(worktree) = worktree else { continue };
            let origin = match origins.entry(worktree) {
                std::collections::btree_map::Entry::Occupied(known) => known.into_mut(),
                std::collections::btree_map::Entry::Vacant(missing) => {
                    let origin =
                        crate::start_session::linked_location(worktree)
                            .await
                            .map(|origin| {
                                kuvo_os::platform::windows_conventional_path(&origin)
                                    .unwrap_or(origin)
                            });
                    missing.insert(origin)
                }
            };
            if let Some(origin) = origin {
                self.linked.insert(folder.to_owned(), origin.clone());
            }
        }
    }
}

pub(super) async fn read(home: &Path, previous: Option<Arc<Databases>>) -> EngineFacts {
    let directory = std::env::var_os("KUVO_ENGINE_DATA")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("data/opencode"));
    let databases = tokio::task::spawn_blocking(move || read_directory(&directory, previous))
        .await
        .unwrap_or_default();
    EngineFacts {
        databases,
        ..Default::default()
    }
}

fn read_directory(directory: &Path, previous: Option<Arc<Databases>>) -> Arc<Databases> {
    let mut facts = Databases::default();
    let Ok(root) = fs::canonicalize(directory) else {
        return previous.unwrap_or_default();
    };
    let Ok(entries) = fs::read_dir(&root) else {
        return previous.unwrap_or_default();
    };
    let sources = entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("opencode") && name.ends_with(".db"))
        })
        .filter_map(|entry| fs::canonicalize(entry.path()).ok())
        .filter(|path| path != &root && path.starts_with(&root) && path.is_file())
        .collect::<BTreeSet<_>>();
    let stamp = |file: &Path| {
        let metadata = fs::metadata(file).ok()?;
        Some((metadata.len(), metadata.modified().ok()?))
    };
    facts.files = sources
        .into_iter()
        .map(|path| Stamp {
            db: stamp(&path),
            wal: stamp(&side_file(&path, "-wal")),
            path,
        })
        .collect();
    if let Some(previous) = &previous
        && previous.files == facts.files
    {
        return Arc::clone(previous);
    }
    let mut complete = true;
    for Stamp { path: source, .. } in &facts.files {
        let inside = |suffix| match fs::canonicalize(side_file(source, suffix)) {
            Ok(side) => side != root && side.starts_with(&root) && side.is_file(),
            Err(error) => error.kind() == io::ErrorKind::NotFound,
        };
        if !["-wal", "-shm", "-journal"].into_iter().all(inside) {
            continue;
        }
        if let Ok(channel) = read_channel(source) {
            for (session, at) in channel.activity {
                let newest = facts.activity.entry(session).or_default();
                *newest = (*newest).max(at);
            }
            facts.roots.extend(channel.roots);
        } else {
            complete = false;
        }
    }
    // Try a channel again on the next inventory if it was busy or unavailable.
    if !complete {
        facts.files.clear();
    }
    Arc::new(facts)
}

fn side_file(path: &Path, suffix: &str) -> PathBuf {
    let mut side = path.as_os_str().to_owned();
    side.push(suffix);
    side.into()
}

fn open(database: &Path) -> rusqlite::Result<Connection> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let connection = |path: &Path, flags| {
        let db = Connection::open_with_flags(path, flags)?;
        db.busy_timeout(BUSY)?;
        db.pragma_update(None, "cache_size", -1024)?;
        db.pragma_update(None, "trusted_schema", false)?;
        Ok::<_, rusqlite::Error>(db)
    };
    let normal = connection(database, flags).and_then(|db| {
        db.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
            row.get::<_, u64>(0)
        })?;
        Ok(db)
    });
    let Err(error) = normal else {
        return normal;
    };
    if !matches!(
        error.sqlite_error_code(),
        Some(ErrorCode::CannotOpen | ErrorCode::ReadOnly)
    ) || fs::metadata(side_file(database, "-wal")).is_ok_and(|wal| wal.len() != 0)
    {
        return Err(error);
    }
    let native = kuvo_os::platform::windows_conventional_path(database);
    let Ok(mut file) = url::Url::from_file_path(native.as_deref().unwrap_or(database)) else {
        return Err(error);
    };
    file.query_pairs_mut().append_pair("immutable", "1");
    connection(Path::new(file.as_str()), flags | OpenFlags::SQLITE_OPEN_URI)
}

fn read_channel(source: &Path) -> rusqlite::Result<Databases> {
    let db = open(source)?;
    let mut tables = db.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name IN ('message','session_message')")?;
    let tables = tables
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<BTreeSet<_>, _>>()?;
    let mut facts = Databases::default();
    for (table, query) in [("message", LEGACY), ("session_message", CURRENT)] {
        if !tables.contains(table) {
            continue;
        }
        let mut read = db.prepare(query)?;
        let activity = read.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        facts.activity.extend(activity.filter_map(Result::ok));
    }
    if tables.contains("session_message") {
        let mut read = db.prepare(WORKTREES)?;
        let roots = read.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        facts.roots.extend(
            roots
                .filter_map(Result::ok)
                .filter(|(folder, project)| folder != project)
                .map(|(folder, project)| (PathBuf::from(folder), PathBuf::from(project))),
        );
    }
    Ok(facts)
}
