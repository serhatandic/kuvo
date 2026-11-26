//! Preview worktree locations without creating folders or starting the engine.
use super::{
    projects,
    safe::{self, Intent},
};
use http::Method;
use kuvo_engine::Engine;
use kuvo_server::Refusal;
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Clone, Serialize)]
pub(super) struct Preview {
    directory: Option<String>,
    default: PathBuf,
    home: PathBuf,
    projects: Vec<Place>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Place {
    project: PathBuf,
    name: String,
    folder: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exists: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    can_make: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inside: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ignored: Option<bool>,
}

async fn git(folder: &Path, args: &[&str]) -> Option<String> {
    let run = safe::run(folder, args, Intent::Own, &[], Duration::from_secs(10))
        .await
        .ok()?;
    run.status
        .success()
        .then(|| String::from_utf8_lossy(&run.stdout).trim().to_owned())
}

#[derive(Clone, PartialEq, Eq)]
struct Key {
    directory: Option<String>,
    state: kuvo_engine::State,
    projects: Vec<PathBuf>,
}

#[derive(Default)]
struct Cached {
    last: Option<(Instant, Preview)>,
    refreshing: bool,
}
type Entry = Arc<tokio::sync::Mutex<Cached>>;

#[derive(Clone, Default)]
pub(super) struct Cache(Arc<tokio::sync::Mutex<VecDeque<(Key, Entry)>>>);

impl Cache {
    pub async fn preview(
        &self,
        engine: &Engine,
        box_home: &Path,
        home: &Path,
        directory: Option<String>,
    ) -> Result<Preview, Refusal> {
        let projects: Vec<PathBuf> = projects::read_projects(box_home)
            .map_err(super::unsupported)?
            .into_iter()
            .filter(|row| row.get("removed") != Some(&Value::Bool(true)))
            .filter_map(|row| {
                row.get("directory")
                    .and_then(Value::as_str)
                    .map(PathBuf::from)
            })
            .collect();
        let key = Key {
            directory: directory.clone(),
            state: engine.state().await,
            projects,
        };
        let entry = {
            let mut cache = self.0.lock().await;
            if let Some(at) = cache.iter().position(|(existing, _)| existing == &key) {
                let found = cache.remove(at).expect("indexed entry");
                let entry = found.1.clone();
                cache.push_back(found);
                entry
            } else {
                let entry = Entry::default();
                if cache.len() == 16 {
                    cache.pop_front();
                }
                cache.push_back((key.clone(), entry.clone()));
                entry
            }
        };
        let mut previous = entry.lock().await;
        if let Some((at, answer)) = &previous.last {
            let answer = answer.clone();
            if at.elapsed() >= Duration::from_secs(5) && !previous.refreshing {
                previous.refreshing = true;
                drop(previous);
                let (engine, box_home, home) =
                    (engine.clone(), box_home.to_owned(), home.to_owned());
                tokio::spawn(async move {
                    let refreshed =
                        preview(&engine, &box_home, &home, directory, key.projects).await;
                    let mut cached = entry.lock().await;
                    cached.last = Some((Instant::now(), refreshed));
                    cached.refreshing = false;
                });
            }
            return Ok(answer);
        }
        // The first read waits, and other callers sharing this input wait on that same read.
        let answer = preview(engine, box_home, home, directory, key.projects).await;
        previous.last = Some((Instant::now(), answer.clone()));
        Ok(answer)
    }
}

async fn preview(
    engine: &Engine,
    box_home: &Path,
    home: &Path,
    directory: Option<String>,
    projects: Vec<PathBuf>,
) -> Preview {
    let default = std::env::var_os("KUVO_ENGINE_DATA")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| box_home.join("data").join("opencode"))
        .join("worktree");
    let default = crate::files::resolved(&default);
    let mut answer = Preview {
        directory,
        default,
        home: home.to_owned(),
        projects: Vec::new(),
    };
    let known = engine
        .call_passive(Method::GET, "api/project", Duration::from_secs(2))
        .await
        .unwrap_or_default();
    let ids: BTreeMap<_, _> = known
        .get("data")
        .unwrap_or(&known)
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let id = row["id"].as_str()?;
            let checkout = Path::new(row["canonical"].as_str()?);
            Some((
                crate::files::real(checkout).unwrap_or_else(|| checkout.to_owned()),
                (row["vcs"] == "git" && id != "global").then_some(id),
            ))
        })
        .collect();
    for project in projects {
        let Some(common) = git(&project, &["rev-parse", "--git-common-dir"]).await else {
            continue;
        };
        let common = crate::files::resolved(&project.join(common));
        if common.file_name().is_none_or(|name| name != ".git") {
            continue;
        }
        let Some(checkout) = common.parent() else {
            continue;
        };
        let checkout = crate::files::real(checkout).unwrap_or_else(|| checkout.to_owned());
        let id = ids.get(&checkout);
        if id == Some(&None)
            || (id.is_none()
                && git(&project, &["rev-list", "-n", "1", "--all"])
                    .await
                    .is_none_or(|head| head.is_empty()))
        {
            continue;
        }
        let folder = match answer.directory.as_deref() {
            None => id
                .and_then(|id| *id)
                .map(|id| answer.default.join(id.chars().take(6).collect::<String>())),
            Some(given) => Some(crate::files::resolved(
                &if let Some(tail) = given.strip_prefix("~/") {
                    home.join(tail)
                } else {
                    checkout.join(given)
                },
            )),
        };
        let name = project
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let mut place = Place {
            project,
            name,
            folder,
            exists: None,
            can_make: None,
            inside: None,
            ignored: None,
        };
        if let Some(folder) = &place.folder {
            let exists = folder.is_dir();
            place.exists = Some(exists);
            if !exists {
                place.can_make = Some(can_make(folder));
            }
            if folder.starts_with(&checkout) {
                place.inside = Some(true);
                place.ignored = Some(
                    git(
                        &checkout,
                        &[
                            "check-ignore",
                            "-q",
                            &folder.join("worktree").to_string_lossy(),
                        ],
                    )
                    .await
                    .is_some(),
                );
            }
        }
        answer.projects.push(place);
    }
    distinguish(&mut answer.projects, home);
    answer
}

fn can_make(folder: &Path) -> bool {
    for parent in folder.ancestors().skip(1) {
        match parent.metadata() {
            Ok(meta) => return meta.is_dir() && writable(parent, &meta),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return false,
        }
    }
    false
}

#[cfg(unix)]
fn writable(folder: &Path, _: &std::fs::Metadata) -> bool {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(folder.as_os_str().as_bytes())
        .is_ok_and(|path| unsafe { libc::access(path.as_ptr(), libc::W_OK) == 0 })
}
#[cfg(windows)]
fn writable(_: &Path, meta: &std::fs::Metadata) -> bool {
    !meta.permissions().readonly()
}

fn distinguish(places: &mut [Place], home: &Path) {
    let mut groups = BTreeMap::<String, Vec<usize>>::new();
    for (at, place) in places.iter().enumerate() {
        groups.entry(place.name.clone()).or_default().push(at);
    }
    for (name, group) in groups.into_iter().filter(|(_, group)| group.len() > 1) {
        let parents: Vec<Vec<String>> = group
            .iter()
            .map(|&at| {
                let parent = places[at].project.parent().unwrap_or(Path::new(""));
                let (parent, in_home) = parent
                    .strip_prefix(home)
                    .map_or((parent, false), |rest| (rest, true));
                let mut parts: Vec<_> = parent
                    .components()
                    .filter_map(|part| match part {
                        std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                        _ => None,
                    })
                    .rev()
                    .collect();
                if in_home {
                    parts.push("~".into());
                }
                parts
            })
            .collect();
        let longest = parents.iter().map(Vec::len).max().unwrap_or_default();
        for depth in 1..=longest {
            let labels: Vec<_> = parents
                .iter()
                .map(|parts| {
                    parts
                        .iter()
                        .take(depth)
                        .rev()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("/")
                })
                .collect();
            if depth == longest
                || labels
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    == labels.len()
            {
                for (&at, label) in group.iter().zip(labels) {
                    if !label.is_empty() {
                        places[at].name = format!("{name} — {label}");
                    }
                }
                break;
            }
        }
    }
}
