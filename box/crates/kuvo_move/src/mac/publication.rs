use super::{AppStep, BESIDE, StagedApp, bundle_id, invalid, prepare, source};
use crate::{Disk, Error, entries, json_at, metadata};
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

mod git;
mod keychain;
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
const VERSION: u8 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CutoverStep {
    Staging(AppStep),
    Prepared,
    KeyCopies,
    Keys,
    FirstPlacement,
    Placed,
    FirstGitRepair,
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
            "done" => 0,
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

fn not_set(value: &bool) -> bool {
    !value
}

fn has_pending(home: &Home) -> Result<bool, Error> {
    let disk = Disk(home.root().into());
    Ok(metadata(&disk.path(PENDING)?)?.is_some())
}

pub fn blocks_open(home: &Home) -> Result<bool, Error> {
    let disk = Disk(home.root().into());
    Ok(read_pending(&disk.path(PENDING)?)?.is_some_and(|saved| saved.phase != Phase::Settle))
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
        match self.run(&mut complete) {
            Ok(outcome) => outcome,
            Err(error) => {
                let pending = has_pending(self.home).unwrap_or(true);
                Outcome {
                    result: if pending { "stuck" } else { "refused" },
                    status: status(None),
                    started: None,
                    message: Some(error.to_string()),
                }
            }
        }
    }

    fn run(&self, complete: &mut impl FnMut(CutoverStep)) -> Result<Outcome, Error> {
        let id = bundle_id(self.bundle)?;
        let real = is_account_home(self.home, self.system_account)?;
        if real != (id == "run.kuvo.code") {
            return Err(invalid(
                "Only Kuvo itself moves its data in ~/.kuvo-code. A copy of the app uses a home of its own.",
            ));
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
        if let Some(mut publication) = read_pending(&pending)? {
            publication.validate(self, &app)?;
            return publication.finish(self, &disk, real, complete);
        }
        if layout::inspect(self.home)? == layout::Layout::Current {
            return published_answer(&disk, &app);
        }
        if layout::inspect(self.home)? != layout::Layout::Unmarked {
            return Err(Error::UnknownHome(disk.0));
        }
        let source = self
            .account
            .join("Library/Application Support")
            .join(&app.id);
        release::check(self, real)?;
        source::no_open_database(&source)?;
        let began = now();
        let input = source::fingerprint(&source)?;
        let staged = prepare(
            self.home,
            self.account,
            self.bundle,
            self.release,
            self.engine,
            |step| complete(CutoverStep::Staging(step)),
        )?;
        let plan = Plan::new(self, source, app, began)?;
        let mut prepared = Publication::prepare(plan, staged, input, &disk)?;
        prepared.write(&disk)?;
        complete(CutoverStep::Prepared);
        prepared.finish(self, &disk, real, complete)
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
        let kept_at = jiff::Timestamp::from_millisecond(self.began as i64)
            .map_err(io::Error::other)?
            .strftime("%Y-%m-%dT%H-%M-%S-%3fZ")
            .to_string();
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
            if !used.insert(name) || metadata(&disk.path(name)?)?.is_some() {
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
        let mut dropped = Vec::new();
        if staged.dropped_waiting_messages > 0 {
            dropped.push(format!(
                "{} messages waiting in the earlier engine couldn't come along.",
                staged.dropped_waiting_messages
            ));
        }
        if staged.dropped_letters > 0 {
            dropped.push(format!(
                "{} old letters couldn't come along.",
                staged.dropped_letters
            ));
        }
        files::sync_tree(&target, &disk.0)?;
        Ok(Self {
            version: VERSION,
            phase: Phase::Keys,
            plan,
            placed,
            identities,
            input,
            repairs,
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
        if self.phase == Phase::Keys {
            self.plan.write_journal(disk, "keys")?;
            let checked = self
                .source_is_unchanged()
                .and_then(|_| source::no_open_database(&self.plan.from));
            if let Err(error) = checked.and_then(|_| {
                keychain::copy(&self.plan, run.account, || complete(CutoverStep::KeyCopies))
            }) {
                self.advance(Phase::Abort, disk)?;
                self.rollback(disk, run.account)?;
                return Err(error);
            }
            clean_key_tool_lock(disk)?;
            self.advance(Phase::Place, disk)?;
            complete(CutoverStep::Keys);
        }
        if self.phase == Phase::Place {
            self.plan.write_journal(disk, "place")?;
            if let Err(error) = self.source_is_unchanged() {
                self.advance(Phase::Abort, disk)?;
                self.rollback(disk, run.account)?;
                return Err(error);
            }
            for (position, &index) in self.placed.iter().enumerate() {
                let item = &self.plan.placements[index];
                let to = disk.path(placement_name(item)?)?;
                let from = disk.path(Path::new(STAGING).join(staged_name(item)?))?;
                let identity = self.identities[&index];
                match (metadata(&from)?, metadata(&to)?) {
                    (Some(meta), None) if EntryIdentity::from(&meta) == identity => {
                        files::place_mac_entry(&from, &to)?
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
            if let Err(error) = self
                .source_is_unchanged()
                .and_then(|_| source::no_open_database(&self.plan.from))
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
            self.repairs
                .apply(|| complete(CutoverStep::FirstGitRepair))?;
            self.advance(Phase::Clear, disk)?;
            complete(CutoverStep::Git);
        }
        if self.phase == Phase::Clear {
            self.plan.write_journal(disk, "clear")?;
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

    fn write_actions(&self, disk: &Disk) -> Result<(), Error> {
        let mut log = String::new();
        let mut record = |value: Value| {
            log.push_str(&value.to_string());
            log.push('\n');
        };
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
        for effect in self.repairs.actions() {
            record(effect);
        }
        files::write_atomic(&disk.path(ACTS)?, log.as_bytes())?;
        Ok(())
    }

    fn source_is_unchanged(&self) -> Result<(), Error> {
        if source::fingerprint(&self.plan.from)? != self.input {
            return Err(invalid("The app's data changed during the staged move."));
        }
        Ok(())
    }

    fn rollback(&self, disk: &Disk, account: &Path) -> Result<(), Error> {
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
            owed.push("passwords");
            dropped.push(SAVED_PASSWORDS.into());
        }
        let state = json!({
            "state":"done","way":"move","from":self.plan.from,"to":self.plan.to,
            "failed":null,"dropped":dropped,"leftovers":self.leftovers,"counts":self.counts.report(),
            "movedBack":false,"movedAt":self.moved_at,"app":self.plan.app,"merged":[],
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

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
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

fn published_answer(disk: &Disk, app: &App) -> Result<Outcome, Error> {
    let state = format::ObjectFormat::additive("Rehome state")
        .read(&disk.path(STATE)?)?
        .map(|record| Value::Object(record.fields))
        .filter(|record| {
            record["state"] == "done" && record["way"] == "move" && record["app"] == json!(app)
        })
        .ok_or_else(|| Error::UnknownHome(disk.0.clone()))?;
    Ok(Outcome {
        result: "done",
        status: status(Some(&state)),
        started: None,
        message: None,
    })
}

fn done(state: &Value, real: bool) -> Outcome {
    Outcome {
        result: "done",
        status: status(Some(state)),
        started: Some(false),
        message: Some(if real { DEFERRED_START } else { COPY_START }.into()),
    }
}

fn status(state: Option<&Value>) -> Value {
    const STEPS: &[(&str, &str)] = &[
        ("plan", "Checking this Mac"),
        ("install", "Getting ready"),
        ("keys", "Handing over your keys"),
        ("clone", "Copying your sessions and settings"),
        ("upgrade", "Bringing your settings along"),
        ("database", "Bringing your sessions up to date"),
        ("rewrite", "Pointing everything at its new place"),
        ("check", "Checking nothing is missing"),
        ("place", "Putting everything in place"),
        ("fix", "Reconnecting your worktrees"),
        ("clear", "Finishing up"),
    ];
    let state = state.unwrap_or(&Value::Null);
    json!({
        "state":state["state"],"way":state["way"],"step":null,"tries":0,"nextTry":null,
        "waitingSince":null,"overdue":false,"failed":null,"movedAt":state["movedAt"],"keptUntil":null,
        "owed":state["owed"].as_array().cloned().unwrap_or_default(),"asked":null,"from":state["from"],
        "to":state["to"],"progress":null,"dropped":state["dropped"].as_array().cloned().unwrap_or_default(),
        "steps":STEPS.iter().map(|&(step,title)|json!({"step":step,"title":title})).collect::<Vec<_>>(),"signIns":null
    })
}
