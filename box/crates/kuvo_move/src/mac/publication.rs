use super::{
    AppStep, BESIDE, ChangePoint, StagedApp, bundle_id, history, invalid, merge, preferences,
    prepare, processes, source,
};
use crate::{
    Disk, Error, entries, json_at, metadata,
    status::{UNREADABLE_JOURNAL, progress, status},
};
use kuvo_store::{files, format, home::Home, layout, locks::OwnedLock};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

mod destination;
mod git;
mod keychain;
mod legacy;
mod release;

const PENDING: &str = "updates/rehome/publication.json";
const JOURNAL: &str = "updates/rehome/journal.json";
const ACTS: &str = "updates/rehome/journal.acts";
const STATE: &str = "updates/rehome/state.json";
const STAGING: &str = "updates/rehome/staging";
const SAVED_PASSWORDS: &str = "Passwords the earlier Kuvo Code saved for websites stay in your Keychain for now: they aren't in Kuvo's vault yet.";
const COPY_START: &str = "It's moved, and starts by itself only in ~/.kuvo-code.";
const DEFERRED_START: &str =
    "Your sessions are moved, but Kuvo couldn't be started on this Mac. Try again.";
const STUCK: &str = "Kuvo was stopped part way through moving your sessions and couldn't finish by itself, so it's changing nothing more. The earlier Kuvo Code's own folder is as it was. Try again; if this stays, send ~/.kuvo-code/updates/rehome/run.log to Kuvo's support.";
const FAILED: &str =
    "Kuvo couldn't move your sessions this time, and left everything as it was. Try again.";
const TWO: &str = "This Mac is set up as one of your machines, and has an earlier Kuvo Code's sessions too: putting them together isn't in this version yet.";
const OURS: &str = "Kuvo is set up on this Mac already, so nothing was moved over it. An earlier Kuvo Code's sessions are still in its own folder.";
const NOT_BACK: &str = "Kuvo on this Mac was moved from an earlier Kuvo Code, whose own folder is as it was: there's nothing to move back.";
const NOT_MOVED: &str = "Kuvo on this Mac hasn't moved: there's nothing to move back.";
const ROUNDS: usize = 3;
const VERSION: u8 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CutoverStep {
    Staging(AppStep),
    Prepared,
    FirstDisplacement,
    KeyCopies,
    Keys,
    FirstPlacement,
    Placed,
    FirstGitRepair,
    FirstHistory,
    Git,
    Layout,
    Published,
    OriginalKeys,
    Settled,
}

#[derive(Serialize)]
pub struct Outcome {
    result: &'static str,
    status: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    started: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

impl Outcome {
    pub fn exit_code(&self) -> u8 {
        match self.result {
            "done" | "nothing" => 0,
            "stuck" => 3,
            _ => 1,
        }
    }
}

pub struct Run<'a> {
    pub home: &'a Home,
    pub account: &'a Path,
    pub system_account: &'a Path,
    pub bundle: &'a Path,
    pub release: &'a Path,
    pub engine: &'a Path,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Phase {
    Install,
    Keys,
    Place,
    Fix,
    Clear,
    Settle,
    Abort,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Publication {
    version: u8,
    phase: Phase,
    plan: Plan,
    placed: Vec<usize>,
    identities: BTreeMap<usize, EntryIdentity>,
    input: String,
    repairs: git::Repairs,
    #[serde(default)]
    destination: destination::Destination,
    #[serde(default)]
    history: history::External,
    #[serde(default)]
    merged: Vec<merge::Merged>,
    counts: super::counts::Comparison,
    leftovers: Vec<String>,
    dropped: Vec<String>,
    moved_at: Option<u64>,
}

#[derive(Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
struct EntryIdentity {
    device: u64,
    inode: u64,
}

impl From<&std::fs::Metadata> for EntryIdentity {
    fn from(meta: &std::fs::Metadata) -> Self {
        Self {
            device: meta.dev(),
            inode: meta.ino(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Plan {
    from: PathBuf,
    to: PathBuf,
    app: App,
    release: PathBuf,
    engine: PathBuf,
    keys: Option<PathBuf>,
    source: Source,
    began: u64,
    cloned_at: u64,
    database_folders: BTreeMap<PathBuf, Vec<String>>,
    placements: Vec<Placement>,
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
struct App {
    bundle: PathBuf,
    id: String,
}

#[derive(Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Source {
    #[serde(rename = "app-07")]
    App07,
    AppBox,
}

#[derive(Serialize, Deserialize)]
struct Placement {
    from: PathBuf,
    to: PathBuf,
    staged: PathBuf,
    #[serde(default, skip_serializing_if = "not_set")]
    run: bool,
    #[serde(default, skip_serializing_if = "not_set")]
    installed: bool,
}

struct Progress(PathBuf);

impl Progress {
    fn show(&self, step: &str) {
        // The journal itself is authoritative for recovery. The phase is
        // only a hint while an authenticated mover holds the lock.
        let _ = files::write_atomic(&self.0, step.as_bytes());
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        let _ = files::remove_file_synced(&self.0);
    }
}

fn not_set(value: &bool) -> bool {
    !value
}

fn has_pending(home: &Home) -> Result<bool, Error> {
    let disk = Disk(home.root().into());
    Ok(metadata(&disk.path(PENDING)?)?.is_some())
}

pub fn unfinished(home: &Home) -> Result<Option<PathBuf>, Error> {
    let disk = Disk(home.root().into());
    let pending = disk.path(PENDING)?;
    if let Some(saved) = read_pending(&pending)? {
        return Ok((saved.phase != Phase::Settle).then_some(pending));
    }
    Ok(legacy_fence(&disk)?.map(|_| home.root().join(JOURNAL)))
}

/// Resume a Rust publication using the installer's original inputs. The
/// mover validates the saved paths again after it has taken the lock.
pub fn resume(home: &Home, account: &Path, system_account: &Path) -> Result<Outcome, Error> {
    let disk = Disk(home.root().into());
    let pending = disk.path(PENDING)?;
    let publication = match read_pending(&pending) {
        Ok(Some(publication)) => publication,
        Ok(None) => {
            if let Some(source) = legacy::previous(&disk, account)?
                && is_account_home(home, system_account)?
                    == source.file_name().is_some_and(|id| id == "run.kuvo.code")
            {
                let _layout = layout::lock_for_move(home, Duration::from_secs(3))?;
                let _handover = OwnedLock::acquire_rehome(
                    &disk.path("updates/rehome/lock")?,
                    Duration::from_secs(3),
                )?;
                if let Some(message) = legacy::undo(&disk, &source, layout::inspect(home)?)? {
                    return Ok(Outcome {
                        result: "failed",
                        status: crate::rehome_status(home),
                        started: None,
                        message: Some(message),
                    });
                }
            }
            if let Some(outcome) = legacy_fence(&disk)? {
                return Ok(outcome);
            }
            return Ok(Outcome {
                result: "nothing",
                status: crate::rehome_status(home),
                started: None,
                message: None,
            });
        }
        Err(error) => {
            eprintln!("The move could not read its publication record: {error}");
            return Ok(Outcome {
                result: "stuck",
                status: crate::rehome_status(home),
                started: None,
                message: Some(STUCK.into()),
            });
        }
    };
    let plan = &publication.plan;
    Ok(Run {
        home,
        account,
        system_account,
        bundle: &plan.app.bundle,
        release: &plan.release,
        engine: &plan.engine,
    }
    .execute(|_| {}))
}

/// The standalone move keeps its predecessor where it was. A legacy move
/// that relocated the predecessor needs its own rollback protocol.
pub fn back(home: &Home) -> Result<Outcome, Error> {
    let disk = Disk(home.root().into());
    let status = crate::rehome_status(home);
    let state = format::ObjectFormat::additive("Rehome state").read(&disk.path(STATE)?)?;
    let moved = status["state"] == "done" && status["way"] == "move";
    let standalone =
        state.is_some_and(|record| record.fields.get("app").is_some_and(Value::is_object));
    let (result, message) = if unfinished(home)?.is_some() || (moved && !standalone) {
        ("stuck", STUCK)
    } else if moved {
        ("refused", NOT_BACK)
    } else {
        ("refused", NOT_MOVED)
    };
    Ok(Outcome {
        result,
        status,
        started: None,
        message: Some(message.into()),
    })
}

/// Settle what a moved home has left without reading a saved sign-in. For a
/// real account Helper's authenticated startup owns retiring original keys.
pub fn settle(home: &Home, account: &Path, system_account: &Path) -> Result<Outcome, Error> {
    let disk = Disk(home.root().into());
    let nothing = || Outcome {
        result: "nothing",
        status: crate::rehome_status(home),
        started: None,
        message: None,
    };
    if unfinished(home)?.is_some() {
        return Ok(nothing());
    }
    let state = disk.path(STATE)?;
    let format = format::ObjectFormat::additive("Rehome state");
    let Some(record) = format.read(&state)? else {
        return Ok(nothing());
    };
    let record = Value::Object(record.fields);
    if record["state"] != "done"
        || record["way"] != "move"
        || record["owed"].as_array().is_none_or(Vec::is_empty)
    {
        return Ok(nothing());
    }
    let Ok(app) = serde_json::from_value::<App>(record["app"].clone()) else {
        return Ok(nothing());
    };
    let real = is_account_home(home, system_account)?;
    if real != (app.id == "run.kuvo.code")
        || !app.bundle.is_absolute()
        || !bundle_id(&app.bundle).is_ok_and(|id| id == app.id)
    {
        return Ok(nothing());
    }
    let origin = account.join("Library/Application Support").join(&app.id);
    if record["from"].as_str().map(Path::new) != Some(origin.as_path()) {
        return Ok(nothing());
    }
    let _layout = layout::lock_for_move(home, Duration::from_secs(3))?;
    let _handover =
        OwnedLock::acquire_rehome(&disk.path("updates/rehome/lock")?, Duration::from_secs(3))?;
    let Some(record) = format.read(&state)? else {
        return Ok(nothing());
    };
    let mut record = Value::Object(record.fields);
    if record["state"] != "done"
        || record["way"] != "move"
        || record["app"] != json!(app)
        || record["from"].as_str().map(Path::new) != Some(origin.as_path())
    {
        return Ok(nothing());
    }
    let Some(owed) = record["owed"].as_array_mut() else {
        return Ok(nothing());
    };
    let keys = app.bundle.join("Contents/MacOS/kuvo-keys");
    if !real
        && !owed.iter().any(|each| each == "start")
        && owed.iter().any(|each| each == "keychain")
        && metadata(&keys)?.is_some_and(|meta| meta.is_file())
        && keychain::delete_at(&keys, home.root(), account)
    {
        owed.retain(|each| each != "keychain");
    }
    if saved_sign_ins_were_imported(&origin) {
        owed.retain(|each| each != "passwords");
    }
    record["formatVersion"] = json!(1);
    write_json(&state, &record, true)?;
    Ok(Outcome {
        result: "done",
        status: status(Some(&record)),
        started: None,
        message: None,
    })
}

fn saved_sign_ins_were_imported(source: &Path) -> bool {
    preferences::read(source)
        .is_ok_and(|prefs| prefs.get("passwords.movedToVault") == Some(&Value::Bool(true)))
}

/// Called by the resident once it has the transferred identity from Helper.
/// A copied or test home never acts on the account's production Keychain.
pub fn settle_started(home: &Home, account: &Path) -> Result<(), Error> {
    if !has_pending(home)? || !is_account_home(home, account)? {
        return Ok(());
    }
    let _layout = layout::lock_for_move(home, Duration::from_secs(3))?;
    let disk = Disk(home.root().into());
    let pending = disk.path(PENDING)?;
    let _handover =
        OwnedLock::acquire_rehome(&disk.path("updates/rehome/lock")?, Duration::from_secs(3))?;
    let Some(publication) = read_pending(&pending)? else {
        return Ok(());
    };
    if publication.phase != Phase::Settle || layout::inspect(home)? != layout::Layout::Current {
        return Ok(());
    }
    let plan = &publication.plan;
    if plan.app.id != "run.kuvo.code" || bundle_id(&plan.app.bundle)? != plan.app.id {
        return Err(invalid("The move's Keychain handover is for another app."));
    }
    let run = Run {
        home,
        account,
        system_account: account,
        bundle: &plan.app.bundle,
        release: &plan.release,
        engine: &plan.engine,
    };
    publication.validate(&run, &plan.app)?;
    let keys_owed = !keychain::delete_originals(plan, account);
    publication.write_state(&disk, false, keys_owed)?;
    files::remove_tree_synced(&disk.path(STAGING)?)?;
    files::remove_tree_synced(&disk.path("updates/install")?)?;
    if !keys_owed {
        files::remove_file_synced(&pending)?;
    }
    Ok(())
}

fn is_account_home(home: &Home, account: &Path) -> io::Result<bool> {
    let target = home.root().canonicalize()?;
    Ok(account
        .join(".kuvo-code")
        .canonicalize()
        .is_ok_and(|actual| actual == target))
}

impl Run<'_> {
    /// The layout lock excludes startup until the marker commits. Hold the
    /// rehome lock as well for the signed key tool and any legacy caller.
    pub fn execute(&self, mut complete: impl FnMut(CutoverStep)) -> Outcome {
        self.run(&mut complete).unwrap_or_else(|error| {
            let pending = has_pending(self.home).unwrap_or(true);
            Outcome {
                result: if pending {
                    "stuck"
                } else if matches!(error, Error::Format(_) | Error::Layout(_)) {
                    "failed"
                } else {
                    "refused"
                },
                status: status(None),
                started: None,
                message: Some(error.to_string()),
            }
        })
    }

    fn run(&self, complete: &mut impl FnMut(CutoverStep)) -> Result<Outcome, Error> {
        let id = bundle_id(self.bundle)?;
        let real = is_account_home(self.home, self.system_account)?;
        if real != (id == "run.kuvo.code") {
            return Ok(refused(if real {
                "Only Kuvo itself moves into ~/.kuvo-code: a copy moves into a home of its own."
            } else {
                "Kuvo itself moves into ~/.kuvo-code only."
            }));
        }
        let app = App {
            bundle: self.bundle.into(),
            id,
        };
        let _layout = layout::lock_for_move(self.home, Duration::from_secs(3))?;
        let disk = Disk(self.home.root().into());
        let pending = disk.path(PENDING)?;
        let lock = disk.path("updates/rehome/lock")?;
        let _handover = OwnedLock::acquire_rehome(&lock, Duration::from_secs(3))?;
        let progress = Progress(disk.path("updates/rehome/run.phase")?);
        // A legacy record is a startup fence in its own right, even when
        // there is no Rust publication record beside it.
        if metadata(&pending)?.is_none() {
            let mut layout = layout::inspect(self.home)?;
            let origin = self
                .account
                .join("Library/Application Support")
                .join(&app.id);
            match legacy::undo(&disk, &origin, layout) {
                Ok(Some(_)) => layout = layout::inspect(self.home)?,
                Ok(None) => {}
                Err(error) => {
                    eprintln!("The move could not undo the earlier placement: {error}");
                    return legacy_fence(&disk)?.ok_or(error);
                }
            }
            if let Some(outcome) = legacy_fence(&disk)? {
                return Ok(outcome);
            }
            if layout == layout::Layout::Current
                && let Some(answer) = published_answer(&disk, &app)?
            {
                return Ok(answer);
            }
            if let Some(message) = occupied_home(&disk)? {
                return Ok(refused(message));
            }
            if !matches!(
                layout,
                layout::Layout::Unmarked | layout::Layout::Older { version: 0 }
            ) {
                return Err(Error::UnknownHome(disk.0));
            }
        }

        for round in 1..=ROUNDS {
            let attempt = if let Some(mut publication) = read_pending(&pending)? {
                publication.validate(self, &app)?;
                if publication.phase == Phase::Settle {
                    progress.show("settle");
                }
                publication.finish(self, &disk, real, complete)
            } else {
                progress.show("plan");
                self.attempt(&disk, &app, real, &progress, complete)
            };
            match attempt {
                Err(error @ Error::SourceChanged(point)) => {
                    files::remove_tree_synced(&disk.path(super::STAGE)?)?;
                    if round == ROUNDS {
                        let step = match point {
                            ChangePoint::Check => "check",
                            ChangePoint::Keys => "keys",
                            ChangePoint::Place => "place",
                            ChangePoint::Fix => "fix",
                        };
                        return note_failure(&disk, "failed", step, &error.to_string());
                    }
                    eprintln!(
                        "Running the move again ({} of {ROUNDS}): the source changed while it was copied.",
                        round + 1
                    );
                }
                outcome => return outcome,
            }
        }
        unreachable!("the last move round always returns");
    }

    fn attempt(
        &self,
        disk: &Disk,
        app: &App,
        real: bool,
        progress: &Progress,
        complete: &mut impl FnMut(CutoverStep),
    ) -> Result<Outcome, Error> {
        let source = self
            .account
            .join("Library/Application Support")
            .join(&app.id);
        source::validate_legacy(&source)?;
        super::engine::check_marker_path(&source.join("box"))?;
        let work = disk.path("updates/rehome")?;
        if let Err(error) = release::check(self, real)
            .and_then(|_| super::engine::admit_schema(&source, &work))
            .and_then(|_| processes::quiesce(&source, &app.bundle, &app.id, real))
            .and_then(|_| source::require_quiet(&Disk(source.join("box"))))
        {
            return note_failure(disk, "refused", "plan", &error.to_string());
        }
        if !source::recorded_workers(disk)?.is_empty() {
            return note_failure(
                disk,
                "refused",
                "plan",
                "Kuvo is already running from ~/.kuvo-code on this Mac. Restart this Mac, then try again.",
            );
        }
        let began = now();
        let input = source::fingerprint(&source)?;
        let mut at = "clone";
        progress.show(at);
        let staged = match prepare(
            self.home,
            self.account,
            self.bundle,
            self.release,
            self.engine,
            |step| {
                at = match step {
                    AppStep::Clone => "upgrade",
                    AppStep::Import => "database",
                    AppStep::Database => "rewrite",
                    AppStep::Rewrite | AppStep::Check => "check",
                };
                progress.show(at);
                complete(CutoverStep::Staging(step));
            },
        ) {
            Ok(staged) => staged,
            Err(error @ Error::SourceChanged(_)) => return Err(error),
            Err(error) => {
                files::remove_tree_synced(&disk.path(super::STAGE)?)?;
                let message = match error {
                    Error::AppPreferences(_) => error.to_string(),
                    _ => {
                        eprintln!("The move stopped at {at}: {error}");
                        FAILED.into()
                    }
                };
                return note_failure(disk, "failed", at, &message);
            }
        };
        let plan = Plan::new(self, source, app.clone(), began)?;
        let mut prepared = Publication::prepare(plan, staged, input, disk)?;
        prepared.history.validate(self.account)?;
        prepared.write(disk)?;
        complete(CutoverStep::Prepared);
        prepared.finish(self, disk, real, complete)
    }
}

impl Plan {
    fn new(run: &Run<'_>, source: PathBuf, app: App, began: u64) -> Result<Self, Error> {
        let source_box = source.join("box");
        let mut names: BTreeSet<_> = ["box", "machines.json", "run"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        for entry in entries(&source_box)? {
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| invalid("A box entry needs a UTF-8 name."))?;
            if name != "bin" && name != layout::FILE {
                names.insert(name);
            }
        }
        let root = run.home.root();
        let mut placements = Vec::new();
        let mut push = |from, name: &str, installed| {
            placements.push(Placement {
                from,
                to: root.join(name),
                staged: root
                    .join(STAGING)
                    .join(format!("{}-{name}", placements.len())),
                run: name == "run",
                installed,
            });
        };
        for name in names {
            push(source_box.join(&name), &name, false);
        }
        for (old, new) in BESIDE {
            push(source.join(old), new, false);
        }
        push(run.engine.into(), "engine", true);
        push(run.release.join("bin"), "bin", true);
        let database_dir = source.join("opencode/data/opencode");
        let databases = entries(&database_dir)?
            .into_iter()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect();
        let keys = app.bundle.join("Contents/MacOS/kuvo-keys");
        let keys = metadata(&keys)?
            .is_some_and(|meta| meta.is_file())
            .then_some(keys);
        let mut source_kind = Source::App07;
        for name in ["link.json", "serve.json", "keeper.json", "setup-card.json"] {
            if metadata(&source_box.join("run").join(name))?.is_some() {
                source_kind = Source::AppBox;
                break;
            }
        }
        Ok(Self {
            from: source,
            to: root.into(),
            app,
            release: run.release.into(),
            engine: run.engine.into(),
            keys,
            source: source_kind,
            began,
            cloned_at: began,
            database_folders: BTreeMap::from([(database_dir, databases)]),
            placements,
        })
    }

    fn write_journal(&self, disk: &Disk, step: &str) -> Result<(), Error> {
        let kept_at = stamp(self.began)?;
        write_json(
            &disk.path(JOURNAL)?,
            &json!({
                "v":1, "way":"move", "began":self.began, "step":step,
                "from":self.from, "to":self.to, "placements":self.placements, "clearing":[],
                "kept":self.to.join("updates/rehome/kept").join(kept_at),
                "run":{"bundle":self.app.bundle,"id":self.app.id,"release":self.release,"engine":self.engine,
                    "source":self.source,"keys":self.keys},
                "clonedAt":self.cloned_at,"databaseFolders":self.database_folders
            }),
            true,
        )
    }
}

impl Publication {
    fn prepare(plan: Plan, staged: StagedApp, input: String, disk: &Disk) -> Result<Self, Error> {
        let staged_disk = Disk(staged.path.clone());
        let destination = destination::Destination::capture(disk, plan.began)?;
        let target = disk.path(STAGING)?;
        files::remove_tree_synced(&target)?;
        files::create_private_dirs(&target)?;
        let mut placed = Vec::new();
        let mut identities = BTreeMap::new();
        let mut used = BTreeSet::new();
        files::create_private_dirs(&staged_disk.path("box")?)?;
        files::remove_file_synced(&staged_disk.path("box/kind")?)?;
        for (index, item) in plan.placements.iter().enumerate() {
            let name = placement_name(item)?;
            let input = staged_disk.path(name)?;
            let Some(meta) = metadata(&input)? else {
                continue;
            };
            if !used.insert(name)
                || (metadata(&disk.path(name)?)?.is_some()
                    && !destination.contains(Path::new(name)))
            {
                return Err(Error::Busy(item.to.clone()));
            }
            let entry = disk.path(Path::new(STAGING).join(staged_name(item)?))?;
            files::place_mac_entry(&input, &entry)?;
            placed.push(index);
            identities.insert(index, EntryIdentity::from(&meta));
        }
        if !entries(&staged.path)?.is_empty() {
            return Err(invalid("The staged home has an unplanned entry."));
        }
        files::remove_tree_synced(&staged.path)?;
        let repairs = git::plan(&plan, &placed, disk)?;
        layout::publish(&Home::new(target.join("work"))?)?;
        let mut dropped = staged.dropped;
        match staged.dropped_waiting_messages {
            0 => {}
            1 => dropped.push("A message still waiting to reach an agent when the earlier Kuvo Code last quit wasn't kept: send it again.".into()),
            count => dropped.push(format!("{count} messages still waiting to reach an agent when the earlier Kuvo Code last quit weren't kept: send them again.")),
        }
        files::sync_tree(&target, &disk.0)?;
        Ok(Self {
            version: VERSION,
            phase: Phase::Install,
            plan,
            placed,
            identities,
            input,
            repairs,
            destination,
            history: staged.history,
            merged: staged.merged,
            counts: staged.counts,
            leftovers: staged.leftovers,
            dropped,
            moved_at: None,
        })
    }

    fn write(&self, disk: &Disk) -> Result<(), Error> {
        write_json(&disk.path(PENDING)?, self, false)
    }

    fn validate(&self, run: &Run<'_>, app: &App) -> Result<(), Error> {
        if self.version != VERSION
            || self.plan.to != run.home.root()
            || &self.plan.app != app
            || self.plan.from
                != run
                    .account
                    .join("Library/Application Support")
                    .join(&app.id)
            || self.plan.release != run.release
            || self.plan.engine != run.engine
            || self
                .plan
                .keys
                .as_ref()
                .is_some_and(|keys| keys != &app.bundle.join("Contents/MacOS/kuvo-keys"))
        {
            return Err(invalid(
                "The saved publication is for a different app, home or release.",
            ));
        }
        let mut names = BTreeSet::new();
        for (index, item) in self.plan.placements.iter().enumerate() {
            let name = placement_name(item)?;
            let actual = item.to == run.home.root().join(name)
                && item.staged
                    == run.home.root().join(STAGING).join(format!(
                        "{}-{}",
                        index,
                        name.to_string_lossy()
                    ));
            if !actual {
                return Err(invalid("A saved placement does not belong to the move."));
            }
        }
        for &index in &self.placed {
            let item = self
                .plan
                .placements
                .get(index)
                .ok_or_else(|| invalid("Invalid staged placement index."))?;
            if !names.insert(placement_name(item)?) {
                return Err(invalid("A saved placement appears twice."));
            }
            if !self.identities.contains_key(&index) {
                return Err(invalid("A saved placement is missing its file identity."));
            }
        }
        self.destination.validate()?;
        self.history.validate(run.account)?;
        self.repairs.validate()
    }

    fn advance(&mut self, phase: Phase, disk: &Disk) -> Result<(), Error> {
        self.phase = phase;
        self.write(disk)
    }

    fn finish(
        &mut self,
        run: &Run<'_>,
        disk: &Disk,
        real: bool,
        complete: &mut impl FnMut(CutoverStep),
    ) -> Result<Outcome, Error> {
        if self.phase == Phase::Install {
            self.plan.write_journal(disk, "install")?;
            if let Err(error) = self.destination.take(disk, self.plan.began, || {
                complete(CutoverStep::FirstDisplacement)
            }) {
                self.advance(Phase::Abort, disk)?;
                self.rollback(disk, run.account)?;
                eprintln!("The move stopped at install: {error}");
                return note_failure(disk, "failed", "install", FAILED);
            }
            self.advance(Phase::Keys, disk)?;
        }
        if self.phase == Phase::Keys {
            self.plan.write_journal(disk, "keys")?;
            let checked = self
                .source_is_unchanged(ChangePoint::Keys)
                .and_then(|_| processes::no_open_writers(&self.plan.from));
            if let Err(error) = checked.and_then(|_| {
                keychain::copy(&self.plan, run.account, || complete(CutoverStep::KeyCopies))
            }) {
                self.advance(Phase::Abort, disk)?;
                self.rollback(disk, run.account)?;
                if matches!(error, Error::SourceChanged(_)) {
                    return Err(error);
                }
                return note_failure(disk, "failed", "keys", &error.to_string());
            }
            clean_key_tool_lock(disk)?;
            self.advance(Phase::Place, disk)?;
            complete(CutoverStep::Keys);
        }
        if self.phase == Phase::Place {
            self.plan.write_journal(disk, "place")?;
            if let Err(error) = self.source_is_unchanged(ChangePoint::Place) {
                self.advance(Phase::Abort, disk)?;
                self.rollback(disk, run.account)?;
                return Err(error);
            }
            if let Err(error) = self.place(disk, complete) {
                self.advance(Phase::Abort, disk)?;
                self.rollback(disk, run.account)?;
                eprintln!("The move stopped at place: {error}");
                return note_failure(disk, "failed", "place", FAILED);
            }
            if let Err(error) = self
                .source_is_unchanged(ChangePoint::Place)
                .and_then(|_| processes::no_open_writers(&self.plan.from))
            {
                self.advance(Phase::Abort, disk)?;
                self.rollback(disk, run.account)?;
                return Err(error);
            }
            self.advance(Phase::Fix, disk)?;
            complete(CutoverStep::Placed);
        }
        if self.phase == Phase::Fix {
            self.plan.write_journal(disk, "fix")?;
            if let Err(error) = self
                .repairs
                .apply(|| complete(CutoverStep::FirstGitRepair))
                .and_then(|_| self.history.apply(|| complete(CutoverStep::FirstHistory)))
                .and_then(|_| self.source_is_unchanged(ChangePoint::Fix))
            {
                self.advance(Phase::Abort, disk)?;
                self.rollback(disk, run.account)?;
                if matches!(error, Error::SourceChanged(_)) {
                    return Err(error);
                }
                eprintln!("The move stopped at fix: {error}");
                return note_failure(disk, "failed", "fix", FAILED);
            }
            self.advance(Phase::Clear, disk)?;
            complete(CutoverStep::Git);
        }
        if self.phase == Phase::Clear {
            self.plan.write_journal(disk, "clear")?;
            self.history.retire_caches()?;
            self.moved_at.get_or_insert_with(now);
            self.write(disk)?;
            self.write_actions(disk)?;
            self.write_state(disk, true, self.plan.keys.is_some())?;
            if layout::inspect(run.home)? != layout::Layout::Current {
                let marker = disk.path(Path::new(STAGING).join("work").join(layout::FILE))?;
                files::place_mac_entry(&marker, &disk.path(layout::FILE)?)?;
                complete(CutoverStep::Layout);
            }
            self.plan.write_journal(disk, "done")?;
            self.advance(Phase::Settle, disk)?;
            complete(CutoverStep::Published);
        }
        if self.phase == Phase::Settle {
            let keys_owed = self.plan.keys.is_some()
                && (real || !keychain::delete_originals(&self.plan, run.account));
            if self.plan.keys.is_some() && !keys_owed {
                complete(CutoverStep::OriginalKeys);
            }
            let state = self.write_state(disk, real, keys_owed)?;
            files::remove_tree_synced(&disk.path(STAGING)?)?;
            if !real {
                files::remove_tree_synced(&disk.path("updates/install")?)?;
            }
            if !real && !keys_owed {
                files::remove_file_synced(&disk.path(PENDING)?)?;
            }
            complete(CutoverStep::Settled);
            return Ok(done(&state, real));
        }
        if self.phase == Phase::Abort {
            self.rollback(disk, run.account)?;
            return Err(invalid("The unfinished move was taken back. Try again."));
        }
        Err(invalid("The saved move is at an invalid step."))
    }

    fn place(&self, disk: &Disk, complete: &mut impl FnMut(CutoverStep)) -> Result<(), Error> {
        for (position, &index) in self.placed.iter().enumerate() {
            let item = &self.plan.placements[index];
            let to = disk.path(placement_name(item)?)?;
            let from = disk.path(Path::new(STAGING).join(staged_name(item)?))?;
            let identity = self.identities[&index];
            match (metadata(&from)?, metadata(&to)?) {
                (Some(meta), None) if EntryIdentity::from(&meta) == identity => {
                    files::place_mac_entry(&from, &to)?;
                }
                (None, Some(meta)) if EntryIdentity::from(&meta) == identity => {}
                _ => return Err(Error::Busy(to)),
            }
            if position == 0 {
                complete(CutoverStep::FirstPlacement);
            }
        }
        let kind = disk.path("box/kind")?;
        if files::read(&kind)?.as_deref() != Some(b"mac\n") {
            files::write_atomic(&kind, b"mac\n")?;
        }
        Ok(())
    }

    fn write_actions(&self, disk: &Disk) -> Result<(), Error> {
        let mut log = String::new();
        let mut record = |value: Value| {
            log.push_str(&value.to_string());
            log.push('\n');
        };
        for effect in self.destination.actions(disk, self.plan.began)? {
            record(effect);
        }
        for &index in &self.placed {
            let item = &self.plan.placements[index];
            record(
                json!({"act":"rename","from":item.staged,"to":item.to,"source":item.from,"merged":false,"formatVersion":1}),
            );
        }
        record(
            json!({"act":"rename","from":self.plan.to.join(STAGING).join("work").join(layout::FILE),
            "to":self.plan.to.join(layout::FILE),"formatVersion":1}),
        );
        record(json!({"act":"made","path":self.plan.to.join("box/kind"),"formatVersion":1}));
        for effect in self.repairs.actions().chain(self.history.actions()) {
            record(effect);
        }
        files::write_atomic(&disk.path(ACTS)?, log.as_bytes())?;
        Ok(())
    }

    fn source_is_unchanged(&self, point: ChangePoint) -> Result<(), Error> {
        if source::fingerprint(&self.plan.from)? != self.input {
            return Err(Error::SourceChanged(point));
        }
        Ok(())
    }

    fn rollback(&self, disk: &Disk, account: &Path) -> Result<(), Error> {
        self.history.undo()?;
        self.repairs.undo_outside()?;
        for &index in self.placed.iter().rev() {
            let item = &self.plan.placements[index];
            let staged = disk.path(Path::new(STAGING).join(staged_name(item)?))?;
            let to = disk.path(placement_name(item)?)?;
            if let (None, Some(meta)) = (metadata(&staged)?, metadata(&to)?)
                && EntryIdentity::from(&meta) == self.identities[&index]
            {
                files::place_mac_entry(&to, &staged)?;
            }
        }
        keychain::uncopy(&self.plan, account)?;
        clean_key_tool_lock(disk)?;
        self.destination.undo(disk, self.plan.began)?;
        files::remove_tree_synced(&disk.path(STAGING)?)?;
        files::remove_file_synced(&disk.path(JOURNAL)?)?;
        files::remove_file_synced(&disk.path(PENDING)?)?;
        Ok(())
    }

    fn write_state(&self, disk: &Disk, start_owed: bool, keys_owed: bool) -> Result<Value, Error> {
        let mut owed = Vec::new();
        let mut dropped = self.dropped.clone();
        if start_owed {
            owed.push("start");
        }
        if self.plan.keys.is_some() {
            if keys_owed {
                owed.push("keychain");
            }
            if !saved_sign_ins_were_imported(&self.plan.from) {
                owed.push("passwords");
                dropped.push(SAVED_PASSWORDS.into());
            }
        }
        let state = json!({
            "state":"done","way":"move","from":self.plan.from,"to":self.plan.to,
            "failed":null,"dropped":dropped,"leftovers":self.leftovers,"counts":self.counts.report(),
            "movedBack":false,"movedAt":self.moved_at,"app":self.plan.app,"merged":self.merged,
            "owed":owed,"tries":0,"nextTry":null,"asked":null,"waitingSince":null,"formatVersion":1
        });
        write_json(&disk.path(STATE)?, &state, true)?;
        Ok(state)
    }
}

fn clean_key_tool_lock(disk: &Disk) -> Result<(), Error> {
    let run = disk.path("run")?;
    if metadata(&run)?.is_some_and(|meta| meta.is_dir()) {
        let entries = entries(&run)?;
        if entries.len() == 1
            && entries[0].file_name() == "helper.lock"
            && entries[0].file_type()?.is_file()
        {
            let path = entries[0].path();
            let _guard = kuvo_store::locks::try_file_lock(&path)?;
            files::remove_file_synced(&path)?;
            files::remove_empty_dir_synced(&run)?;
        }
    }
    Ok(())
}

fn placement_name(item: &Placement) -> Result<&OsStr, Error> {
    let name = item
        .to
        .file_name()
        .ok_or_else(|| invalid("A placement needs a filename."))?;
    if matches!(
        name.to_str(),
        Some("updates" | "desktop" | "folder-version.json")
    ) || name.to_string_lossy().starts_with(".folder-upgrade.lock")
    {
        return Err(invalid("A placement overlaps move or desktop state."));
    }
    Ok(name)
}

fn staged_name(item: &Placement) -> Result<&OsStr, Error> {
    item.staged
        .file_name()
        .ok_or_else(|| invalid("A placement needs a staged filename."))
}

fn read_pending(path: &Path) -> Result<Option<Publication>, Error> {
    if metadata(path)?.is_some_and(|meta| !meta.is_file()) {
        return Err(Error::Busy(path.into()));
    }
    json_at(path)
}

fn legacy_fence(disk: &Disk) -> Result<Option<Outcome>, Error> {
    let path = disk.path(JOURNAL)?;
    let Some(meta) = metadata(&path)? else {
        return Ok(None);
    };
    let journal = meta
        .is_file()
        .then(|| json_at::<Value>(&path).ok().flatten())
        .flatten()
        .filter(|record| record["v"] == 1 && record["step"].is_string());
    let (state, message) = match journal {
        Some(ref record) if record["step"] == "done" => return Ok(None),
        // The TypeScript action log has its own replay protocol. Keep
        // startup fenced until this journal has been recovered.
        Some(record) => (record, STUCK),
        None => (Value::Null, UNREADABLE_JOURNAL),
    };
    let mut status = status(None);
    status["state"] = json!(if state["step"] == "clear" {
        "clearing"
    } else {
        "moving"
    });
    status["failed"] = json!({ "at": now(), "step": null, "message": message });
    for name in ["step", "way", "from", "to"] {
        status[name] = state[name].clone();
    }
    Ok(Some(Outcome {
        result: "stuck",
        status,
        started: None,
        message: Some(message.into()),
    }))
}

fn note_failure(
    disk: &Disk,
    result: &'static str,
    step: &str,
    message: &str,
) -> Result<Outcome, Error> {
    let path = disk.path(STATE)?;
    let mut record = format::ObjectFormat::additive("Rehome state")
        .read(&path)?
        .map(|record| Value::Object(record.fields))
        .unwrap_or_else(|| json!({}));
    record["state"] = json!("failed");
    record["way"] = json!("move");
    record["tries"] = json!(
        record["tries"]
            .as_u64()
            .unwrap_or_default()
            .saturating_add(1)
    );
    record["nextTry"] = Value::Null;
    record["asked"] = Value::Null;
    record["failed"] = json!({ "at": now(), "step": step, "message": message });
    record["formatVersion"] = json!(1);
    write_json(&path, &record, true)?;
    let mut status = status(Some(&record));
    status["progress"] = progress("plan");
    Ok(Outcome {
        result,
        status,
        started: None,
        message: Some(message.into()),
    })
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn stamp(time: u64) -> Result<String, Error> {
    Ok(jiff::Timestamp::from_millisecond(time as i64)
        .map_err(io::Error::other)?
        .strftime("%Y-%m-%dT%H-%M-%S-%3fZ")
        .to_string())
}

fn write_json(path: &Path, value: &impl Serialize, pretty: bool) -> Result<(), Error> {
    let mut bytes = if pretty {
        let mut bytes = Vec::new();
        let formatter = serde_json::ser::PrettyFormatter::with_indent(b" ");
        value
            .serialize(&mut serde_json::Serializer::with_formatter(
                &mut bytes, formatter,
            ))
            .map_err(io::Error::other)?;
        bytes
    } else {
        serde_json::to_vec(value).map_err(io::Error::other)?
    };
    bytes.push(b'\n');
    files::write_atomic(path, &bytes)?;
    Ok(())
}

fn published_answer(disk: &Disk, app: &App) -> Result<Option<Outcome>, Error> {
    let state = format::ObjectFormat::additive("Rehome state")
        .read(&disk.path(STATE)?)?
        .map(|record| Value::Object(record.fields))
        .filter(|record| {
            record["state"] == "done" && record["way"] == "move" && record["app"] == json!(app)
        });
    Ok(state.map(|state| Outcome {
        result: "done",
        status: status(Some(&state)),
        started: None,
        message: None,
    }))
}

fn occupied_home(disk: &Disk) -> Result<Option<&'static str>, Error> {
    let facts = crate::install::HomeFacts::read(disk)?;
    if facts.is("mac") {
        return Ok(Some(OURS));
    }
    Ok(facts
        .machine_owned(disk, kuvo_os::platform::Platform::Mac)?
        .then_some(TWO))
}

fn refused(message: &str) -> Outcome {
    let mut status = status(None);
    status["progress"] = progress("plan");
    Outcome {
        result: "refused",
        status,
        started: None,
        message: Some(message.into()),
    }
}

fn done(state: &Value, real: bool) -> Outcome {
    Outcome {
        result: "done",
        status: status(Some(state)),
        started: Some(false),
        message: Some(if real { DEFERRED_START } else { COPY_START }.into()),
    }
}
