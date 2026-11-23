//! Admission and publication for a new Mac home.

use super::{
    Outcome, invalid,
    publication::{destination::Destination, now},
    source,
};
use crate::{Disk, Error, entries, install::HomeFacts, metadata};
use kuvo_store::{files, format, home::Home, layout, locks::OwnedLock};
use serde::Serialize;
use serde_json::json;
use std::{
    fs, io,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

const NEW: &str = "updates/rehome/new";
const STAGE: &str = "updates/rehome/new-stage";
const PLUGIN: &str = "config/opencode/plugins/kuvo-code.js";
const APP_BOX_NEEDED: &str =
    "kuvo-link.js rehome needs KUVO_HOME, the app's own box's folder (<app folder>/box).";
const RELEASE_NEEDED: &str = "Kuvo's own files to put in its new home weren't given.";
const WAITS: &str = "Another move is under way: this one waits for it.";

pub(crate) fn interrupted(home: &Home) -> Result<Option<PathBuf>, Error> {
    let disk = Disk(home.root().to_owned());
    let marker = disk.path(NEW)?;
    if metadata(&marker)?.is_none() {
        return Ok(None);
    }
    Ok(HomeFacts::read(&disk)?
        .making_cut_short(&disk)?
        .then_some(marker))
}

#[derive(Serialize)]
pub struct NewReadiness {
    ready: bool,
    made: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    why: Option<String>,
}

impl NewReadiness {
    fn refused(why: impl Into<String>) -> Self {
        Self {
            ready: false,
            made: false,
            why: Some(why.into()),
        }
    }
}

pub fn new_readiness(
    home: &Home,
    app_box: Option<&Path>,
    engine_data: Option<&Path>,
    account: &Path,
) -> NewReadiness {
    check(home, app_box, engine_data, account)
        .unwrap_or_else(|error| NewReadiness::refused(error.to_string()))
}

fn check(
    home: &Home,
    app_box: Option<&Path>,
    engine_data: Option<&Path>,
    account: &Path,
) -> Result<NewReadiness, Error> {
    layout::inspect(home)?;
    let located = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_owned());
    let app_box = app_box
        .filter(|path| path.ends_with("box") && located(path) != located(home.root()))
        .ok_or_else(|| invalid(APP_BOX_NEEDED))?;
    let app = app_box.parent().ok_or_else(|| invalid(APP_BOX_NEEDED))?;
    if super::unfinished(home)?.is_some() {
        return Ok(NewReadiness::refused(
            "Kuvo on this Mac is moving: that finishes first.",
        ));
    }
    let disk = Disk(home.root().to_owned());
    let facts = HomeFacts::read(&disk)?;
    let interrupted = facts.making_cut_short(&disk)?;
    if facts.is("mac") && !interrupted {
        return Ok(NewReadiness {
            ready: false,
            made: true,
            why: None,
        });
    }
    let default_data = app.join("opencode/data/opencode");
    let database = entries(engine_data.unwrap_or(&default_data))
        .unwrap_or_default()
        .into_iter()
        .any(|file| {
            let name = file.file_name();
            let name = name.to_string_lossy();
            name.starts_with("opencode") && name.ends_with(".db")
        });
    let run = app_box.join("run");
    if database
        || ["setup-card.json", "link.json", "serve.json", "keeper.json"]
            .iter()
            .any(|name| run.join(name).exists())
    {
        return Ok(NewReadiness::refused(
            "Kuvo Code's data is in its folder: it moves into its home instead.",
        ));
    }
    if located(home.root()) == located(&account.join(".kuvo-code"))
        && !app.ends_with("run.kuvo.code")
    {
        return Ok(NewReadiness::refused(
            "Only Kuvo Code itself sets up Kuvo in ~/.kuvo-code: a copy makes it in a home of its own.",
        ));
    }
    let workers = source::recorded_workers(&disk)?;
    if !interrupted
        && metadata(&disk.path("bin/kuvo-start")?)?.is_some()
        && metadata(&disk.path("run/off")?)?.is_none()
        && (facts.identity() || facts.is("machine") || workers.contains(&"its keeper"))
    {
        return Ok(NewReadiness::refused(
            "Kuvo is set up in this Mac's ~/.kuvo-code already: both keep running as they are.",
        ));
    }
    if !workers.is_empty() {
        return Ok(NewReadiness::refused(
            "Something is running in this Mac's ~/.kuvo-code: it's made there once that's stopped.",
        ));
    }
    if entries(&app.join("engine")).unwrap_or_default().is_empty() {
        return Ok(NewReadiness::refused(
            "The engine isn't downloaded yet: Kuvo is set up on this Mac once it is.",
        ));
    }
    Ok(NewReadiness {
        ready: true,
        made: false,
        why: None,
    })
}

pub fn new_home(
    home: &Home,
    app_box: Option<&Path>,
    engine_data: Option<&Path>,
    account: &Path,
    release: Option<&Path>,
    keeps_awake_off: bool,
) -> Outcome {
    let say = |result, message| Outcome {
        result,
        message,
        started: None,
        status: crate::rehome_status(home),
    };
    let make = || -> Result<Outcome, Error> {
        let admit = || -> Result<Option<Outcome>, Error> {
            let readiness = check(home, app_box, engine_data, account)?;
            Ok(if readiness.made {
                Some(say("nothing", None))
            } else if let Some(why) = readiness.why {
                Some(say("refused", Some(why)))
            } else {
                None
            })
        };
        if let Some(outcome) = admit()? {
            return Ok(outcome);
        }
        let Some(release) = release else {
            return Ok(say("refused", Some(RELEASE_NEEDED.into())));
        };
        if let Some(why) = release_refusal(release)? {
            return Ok(say("refused", Some(why.into())));
        }
        let disk = Disk(home.root().to_owned());
        let wait = Duration::from_secs(3);
        let _layout = match layout::lock_for_move(home, wait) {
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                return Ok(say("waits", Some(WAITS.into())));
            }
            other => other?,
        };
        let _rehome = match OwnedLock::acquire_rehome(&disk.path("updates/rehome/lock")?, wait) {
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                return Ok(say("waits", Some(WAITS.into())));
            }
            other => other?,
        };
        if let Some(outcome) = admit()? {
            return Ok(outcome);
        }
        let app = app_box
            .and_then(Path::parent)
            .ok_or_else(|| invalid(APP_BOX_NEEDED))?;
        let interrupted = HomeFacts::read(&disk)?.making_cut_short(&disk)?;
        let began = now();
        let marker = disk.path(NEW)?;
        files::write_atomic(
            &marker,
            &serde_json::to_vec(&json!({"at":began,"formatVersion":1}))
                .map_err(io::Error::other)?,
        )?;
        if let Err(error) = publish(
            &disk,
            release,
            &app.join("engine"),
            began,
            keeps_awake_off,
            interrupted,
        ) {
            files::remove_tree_synced(&disk.path(STAGE)?)?;
            if !disk.path("box/kind")?.is_file() {
                files::remove_file_synced(&marker)?;
            }
            return Err(error);
        }
        layout::publish(home)?;
        files::remove_file_synced(&marker)?;
        Ok(say("done", None))
    };
    make().unwrap_or_else(|error| say("failed", Some(error.to_string())))
}

fn release_refusal(release: &Path) -> Result<Option<&'static str>, Error> {
    let bin = release.join("bin");
    if !bin.join("kuvo-start").is_file()
        || !release.join(PLUGIN).is_file()
        || !(bin.join("kuvo-box").is_file() || bin.join("kuvo-link.js").is_file())
    {
        return Ok(Some(RELEASE_NEEDED));
    }
    if !bin
        .join("Kuvo Helper.app/Contents/MacOS/Kuvo Helper")
        .is_file()
    {
        return Ok(Some(
            "Kuvo Helper isn't in this copy of Kuvo, so nothing would start Kuvo on this Mac in its own home.",
        ));
    }
    format::BOX_FILES.read(&release.join("box.json"))?;
    Ok(None)
}

fn publish(
    disk: &Disk,
    release: &Path,
    engine: &Path,
    began: u64,
    keeps_awake_off: bool,
    interrupted: bool,
) -> Result<(), Error> {
    let stage = disk.path(STAGE)?;
    files::remove_tree_synced(&stage)?;
    if interrupted {
        for name in ["bin.new", "engine.new"] {
            files::remove_tree_synced(&disk.path(name)?)?;
        }
    }
    files::create_private_dirs(&stage)?;
    let bin = release.join("bin");
    let output = Command::new("/bin/cp")
        .env_clear()
        .arg("-RLp")
        .arg(&bin)
        .arg(stage.join("bin"))
        .output()?;
    if !output.status.success() {
        return Err(invalid(&format!(
            "{} couldn't be cloned: {}",
            bin.display(),
            String::from_utf8_lossy(&output.stderr)
                .trim()
                .chars()
                .take(300)
                .collect::<String>()
        )));
    }
    let plugin = stage.join(PLUGIN);
    files::create_private_dirs(plugin.parent().expect("the plugin has a parent"))?;
    fs::copy(release.join(PLUGIN), &plugin)?;
    fs::set_permissions(&plugin, fs::Permissions::from_mode(0o644))?;
    files::clone_mac_entry(engine, &stage.join("engine"), true)?;
    if keeps_awake_off {
        files::write_new(&stage.join("box/work.json"), br#"{"keepsAwake":false}"#)?;
    }
    files::write_new(&stage.join("box/kind"), b"mac\n")?;
    files::sync_tree(&stage, &disk.0)?;
    Destination::fresh(disk, began)?.take(disk, began, || {})?;
    for name in ["bin", "config", "engine", "box"] {
        files::place_mac_entry(&stage.join(name), &disk.path(name)?)?;
    }
    files::remove_empty_dir_synced(&stage)?;
    Ok(())
}
