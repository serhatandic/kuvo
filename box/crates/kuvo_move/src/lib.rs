//! One-time imports before the resident opens an older home.

pub mod install;
mod machine;
mod status;

pub use status::{progress as rehome_progress, rehome_status};

#[cfg(target_os = "macos")]
pub mod mac;

use kuvo_os::{platform::Platform, process::ProcessIdentity};
use kuvo_store::{files, format, home::Home, layout};
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[cfg(target_os = "macos")]
    #[error(transparent)]
    Database(#[from] kuvo_store::sqlite::Error),
    #[cfg(target_os = "macos")]
    #[error("reading the staged engine database: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error(transparent)]
    Layout(#[from] layout::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Format(#[from] format::Error),
    #[error("Cannot read the move input at {}", path.display())]
    Input {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("The Mac app folder at {} needs a staged move.", .0.display())]
    AppFolder(PathBuf),
    #[error("The previous move at {} is unfinished.", .0.display())]
    PreviousMove(PathBuf),
    #[error("The home at {} is not a supported predecessor installation.", .0.display())]
    UnknownHome(PathBuf),
    #[error("Cannot move while {} is present.", .0.display())]
    Busy(PathBuf),
    #[cfg(target_os = "macos")]
    #[error(
        "Something kept changing your sessions while Kuvo copied them, so nothing was moved. Quit the earlier Kuvo Code wherever it's open, then try again."
    )]
    SourceChanged(mac::ChangePoint),
    #[cfg(target_os = "macos")]
    #[error(
        "Kuvo couldn't read the earlier Kuvo Code's settings, so nothing was moved. Try again."
    )]
    AppPreferences(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// A boundary reached after all writes for a machine step are durable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Step {
    Settings,
    Kit,
    Link,
    Outbox,
    Pages,
    Agents,
    Presence,
    Layout,
}

/// Prefer the store's regular opener. Older homes return a typed handoff;
/// those steps replay under the same lock used to publish a fresh layout.
/// Step callbacks hold the lock until the layout commits.
pub fn open(
    home: &Home,
    platform: Platform,
    account_home: Option<&Path>,
    mut completed: impl FnMut(Step),
) -> Result<(), Error> {
    #[cfg(target_os = "macos")]
    if let Some(journal) = mac::unfinished(home)? {
        return Err(Error::PreviousMove(journal));
    }
    #[cfg(target_os = "macos")]
    if let Some(marker) = mac::unfinished_install(home)? {
        return Err(Error::Busy(marker));
    }
    if layout::inspect(home)? == layout::Layout::Current {
        return Ok(());
    }
    if let Some(account_home) = account_home
        && platform == Platform::Mac
        && home.root() == account_home.join(".kuvo-code")
    {
        let source = account_home.join("Library/Application Support/run.kuvo.code");
        if app_source(&source)?.is_some() {
            return Err(Error::AppFolder(source));
        }
    }
    match layout::open(home) {
        Ok(()) => return Ok(()),
        Err(layout::Error::MoveRequired { .. } | layout::Error::ResumeMove { .. }) => {}
        Err(error) => return Err(error.into()),
    }
    let guard = layout::lock_for_move(home, Duration::from_secs(3))?;
    match layout::inspect(home)? {
        layout::Layout::Current => return Ok(()),
        layout::Layout::Unmarked
        | layout::Layout::Older { version: 0 | 1 }
        | layout::Layout::Interrupted {
            version: 0,
            upgrading_to: 1,
        } => {}
        _ => return Err(Error::UnknownHome(home.root().into())),
    }
    let disk = Disk(home.root().to_owned());
    let journal = disk.path("updates/rehome/journal.json")?;
    if let Some(record) = json_at::<Journal>(&journal)?
        && record.step != "done"
    {
        return Err(Error::PreviousMove(journal));
    }
    for relative in ["run/update-trial", "run/.rolling-back", "run/.launching"] {
        let path = disk.path(relative)?;
        if metadata(&path)?.is_some() {
            return Err(Error::Busy(path));
        }
    }
    check_writers(&disk)?;
    if !machine::recognized(&disk)? {
        return Err(Error::UnknownHome(home.root().into()));
    }
    machine::run(&disk, account_home, &mut completed)?;
    layout::publish(home)?;
    drop(guard);
    completed(Step::Layout);
    Ok(())
}

fn check_writers(disk: &Disk) -> Result<(), Error> {
    let link = disk.path("run/link.pid")?;
    let link_pid = files::read(&link)?
        .and_then(|bytes| String::from_utf8_lossy(&bytes).trim().parse::<u32>().ok());
    require_stopped(link, link_pid)?;
    for relative in ["run/serve.json", "run/bridge.json"] {
        let record = disk.path(relative)?;
        let pid = json_at::<ValueWithPid>(&record)?.and_then(|value| value.pid);
        require_stopped(record, pid)?;
    }
    Ok(())
}

fn require_stopped(record: PathBuf, pid: Option<u32>) -> Result<(), Error> {
    if let Some(pid) = pid.filter(|pid| *pid > 1)
        && ProcessIdentity::read(pid)?.is_some()
    {
        return Err(Error::Busy(record));
    }
    Ok(())
}

#[derive(serde::Deserialize)]
struct ValueWithPid {
    pid: Option<u32>,
}

#[derive(serde::Deserialize)]
struct Journal {
    step: String,
}

fn app_source(app: &Path) -> Result<Option<install::Verdict>, Error> {
    for file in ["setup-card.json", "link.json", "serve.json", "keeper.json"] {
        if metadata(&app.join("box/run").join(file))?.is_some() {
            return Ok(Some(install::Verdict::AppBox));
        }
    }
    let rules: &[(&str, &[&str])] = &[
        (
            "",
            &[
                "engine", "box", "desktop", "Caches", "cache", "tmp", "logs", "Logs", "opencode",
            ],
        ),
        ("opencode", &["cache"]),
        ("box", &["run", "bin"]),
    ];
    for (folder, excluded) in rules {
        for entry in entries(&app.join(folder))? {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with('.') && !excluded.contains(&name.as_ref()) {
                return Ok(Some(install::Verdict::App07));
            }
        }
    }
    Ok(None)
}

/// Resolve known in-home paths without using a linked directory as a write target.
struct Disk(PathBuf);

impl Disk {
    fn path(&self, relative: impl AsRef<Path>) -> io::Result<PathBuf> {
        let path = self.0.join(relative);
        let mut parent = path.parent();
        while let Some(folder) = parent.filter(|folder| *folder != self.0) {
            if let Some(meta) = metadata(folder)?
                && !meta.is_dir()
            {
                return Err(io::Error::other(format!(
                    "{} is not a regular folder",
                    folder.display()
                )));
            }
            parent = folder.parent();
        }
        Ok(path)
    }
}

fn metadata(path: &Path) -> io::Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(meta) => Ok(Some(meta)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn entries(path: &Path) -> io::Result<Vec<fs::DirEntry>> {
    match fs::read_dir(path) {
        Ok(entries) => entries.collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn json_at<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, Error> {
    files::read(path)?
        .map(|bytes| {
            serde_json::from_slice(&bytes).map_err(|source| Error::Input {
                path: path.into(),
                source,
            })
        })
        .transpose()
}

fn save_json(path: &Path, value: &impl serde::Serialize) -> Result<(), Error> {
    let bytes = serde_json::to_vec(value).map_err(|source| Error::Input {
        path: path.into(),
        source,
    })?;
    Ok(files::write_atomic(path, &bytes)?)
}
