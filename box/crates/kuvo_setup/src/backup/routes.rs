//! Resident entry points. Requests carrying paths or keys require the local app.
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use http::header::{CACHE_CONTROL, HeaderValue};
use kuvo_protocol::{
    JsonStatus, Route, RouteSpec,
    routes::get_kuvo_box_backups_status as health,
    shape::{Primitive, Shape},
};
use kuvo_server::{Access, Caller, LocalOrigin, Refusal, Registry, RouteReply, Transport};
use kuvo_store::{files, home::Home};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    Backups, Destination, DestinationStore, DiskStore, Error, PointId, Reason, RecoveryKey,
    RestorePoint, Result, Source, SshStore, expired_points,
};
use crate::{SetupHandle, model::Link, storage::Files};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Config {
    version: u64,
    enabled: bool,
    source: Source,
    key: String,
    dest: Destination,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    disk: Option<u64>,
    projects: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last: Option<RestorePoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_daily: Option<RestorePoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Deserialize)]
struct HealthConfig {
    enabled: bool,
    dest: Destination,
    disk: Option<u64>,
    last: Option<VerifiedAt>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct VerifiedAt {
    at: i64,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Request {
    confirm: Option<bool>,
    enabled: Option<bool>,
    destination: Option<Value>,
    projects: Option<Value>,
    source: Option<String>,
    id: Option<String>,
    recovery_key: Option<String>,
    previous: Option<String>,
    replacement: Option<String>,
    kind: Option<String>,
    operation: Option<String>,
    decision: Option<String>,
}

struct Service {
    backups: Backups,
    setup: SetupHandle,
    account: PathBuf,
    relay: Option<super::relay::mail::PeerMail>,
    vault: Option<Arc<dyn super::vault::VaultBackend>>,
    queue_review: Option<Arc<dyn super::QueueReview>>,
}

impl Service {
    fn config_path(&self) -> PathBuf {
        self.backups.root().join("backup/configuration.json")
    }

    fn read_config<T: serde::de::DeserializeOwned>(&self) -> Result<Option<T>> {
        files::read(&self.config_path())?
            .map(|bytes| {
                let value: Value = serde_json::from_slice(&bytes)?;
                if value.get("version").and_then(Value::as_u64) != Some(1) {
                    return Err(Error::Message("Unsupported backup configuration."));
                }
                Ok(serde_json::from_value(value)?)
            })
            .transpose()
    }

    fn config(&self) -> Result<Option<Config>> {
        self.read_config()
    }

    fn save(&self, config: &Config) -> Result<()> {
        files::write_secret(&self.config_path(), &serde_json::to_vec(config)?)?;
        Ok(())
    }

    fn link(&self) -> Result<Option<Link>> {
        Files::new(self.backups.root().to_owned())
            .read(crate::actor::LINK)
            .map_err(|error| Error::Detail(error.to_string()))
    }

    fn boundary(&self, restoring: bool) -> Result<super::boundary::Boundary> {
        let files = Files::new(self.backups.root().to_owned());
        let roots = self.backups.roots.clone();
        if restoring {
            Ok(
                super::boundary::Boundary::read(files, self.setup.clone(), roots, true)?
                    .with_relay(self.relay.clone()),
            )
        } else {
            super::boundary::Boundary::capture_from(files, self.setup.clone(), roots)
        }
    }

    fn health(&self, config: Option<&HealthConfig>) -> Value {
        let available = config.map(|config| self.available(&config.dest, config.disk));
        json!({
            "enabled": config.is_some_and(|config| config.enabled),
            "destinationKind": config.map(|config| match config.dest {
                Destination::Disk { .. } => "disk",
                Destination::Computer { .. } => "computer",
            }),
            "destinationAvailable": available,
            "lastVerifiedAt": config.and_then(|config| config.last.as_ref().map(|point| point.at)),
            "attention": config.is_some_and(|config| config.error.as_ref().is_some_and(|error| !error.is_empty())),
        })
    }

    fn status(&self, config: Option<&Config>) -> Result<Value> {
        let roster = self.setup.rosters().borrow().clone();
        let local = roster
            .as_ref()
            .and_then(|roster| roster.here.map(|key| kuvo_wire::keys::encode(&key)));
        let computers: Vec<_> = roster
            .iter()
            .flat_map(|roster| &roster.members)
            .filter_map(|member| {
                let key = kuvo_wire::keys::encode(&member.key);
                let relay = self.relay.as_ref().is_some_and(|mail| mail.available(&key));
                let ssh = config.and_then(|config| match &config.dest {
                    Destination::Computer { computer, ssh } if computer == &key => ssh.as_ref(),
                    _ => None,
                });
                if !relay && ssh.is_none() {
                    return None;
                }
                let mut peer = json!({ "key": key, "name": member.name });
                if relay {
                    peer["relay"] = json!(true);
                }
                if let Some(ssh) = ssh {
                    peer["ssh"] = json!(ssh);
                }
                Some(peer)
            })
            .collect();
        let mut status = json!({
            "computer": local,
            "enabled": config.is_some_and(|config| config.enabled),
            "computers": computers,
            "defaultDestination": if computers.is_empty() { "disk" } else { "computer" },
            "destination": config.map(|config| &config.dest),
            "source": config.map(|config| &config.source),
            "projects": config.map(|config| config.projects.as_slice()).unwrap_or_default(),
            "destinationAvailable": config.map(|config| self.available(&config.dest, config.disk)),
            "lastVerifiedRestorePoint": config.and_then(|config| config.last.as_ref()),
        });
        if let Some(error) = config
            .and_then(|config| config.error.as_ref())
            .filter(|error| !error.is_empty())
        {
            status["error"] = json!(error);
        }
        if config.is_none_or(|config| config.last.is_none()) {
            status["restorePointStatus"] = json!("no separate backup");
        }
        Ok(status)
    }

    fn available(&self, dest: &Destination, disk: Option<u64>) -> bool {
        match dest {
            Destination::Disk { path } => self.separate_disk(path).is_ok_and(|id| Some(id) == disk),
            Destination::Computer {
                computer,
                ssh: Some(_),
            } => self.member(computer),
            Destination::Computer {
                computer,
                ssh: None,
            } => self
                .relay
                .as_ref()
                .is_some_and(|relay| relay.available(computer)),
        }
    }

    fn separate_disk(&self, path: &Path) -> Result<u64> {
        let device = |path| {
            files::file_stamp(path)?
                .map(|stamp| stamp.device)
                .ok_or_else(|| {
                    Error::Io(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "Backup folder is unavailable.",
                    ))
                })
        };
        let id = device(path)?;
        if id == device(self.backups.root())? {
            return Err(Error::Message("Choose a folder on a separate disk."));
        }
        Ok(id)
    }

    fn member(&self, key: &str) -> bool {
        let Some(key) = kuvo_wire::keys::decode_exact::<32>(key) else {
            return false;
        };
        self.setup
            .rosters()
            .borrow()
            .as_ref()
            .is_some_and(|roster| roster.member(&key).is_some())
    }

    fn store(&self, dest: &Destination, source: &Source) -> Result<Box<dyn DestinationStore>> {
        let root = self.backups.root();
        match dest {
            Destination::Disk { path } => Ok(Box::new(DiskStore::open(path, root, source)?)),
            Destination::Computer {
                ssh: Some(target), ..
            } => {
                let store = SshStore::new(root, &self.account, target, source)?;
                Ok(Box::new(store))
            }
            Destination::Computer {
                computer,
                ssh: None,
            } => {
                let relay = self
                    .relay
                    .as_ref()
                    .filter(|relay| relay.available(computer))
                    .ok_or(Error::Message(
                        "Choose an enrolled computer that supports Relay backups.",
                    ))?;
                let setup = self
                    .link()?
                    .and_then(|link| link.setup.map(|setup| setup.position.id))
                    .ok_or(Error::Message("Wait for accepted backup membership."))?;
                Ok(Box::new(super::relay::Store::new(
                    computer.clone(),
                    setup,
                    source.clone(),
                    relay.clone(),
                )))
            }
        }
    }

    fn before(&self, kept: &mut Config, scratch: &Path) -> Result<Option<RestorePoint>> {
        if !kept.enabled {
            return Ok(None);
        }
        let now = jiff::Timestamp::now().as_millisecond();
        let id = PointId::generate(Reason::Daily, now)?;
        if let Some(point) = &kept.last_daily
            && id.day() == point.id.day()
            && kept.error.is_none()
        {
            return Ok(Some(point.clone()));
        }
        let work: Result<Option<RestorePoint>> = (|| {
            let authority = self.boundary(false)?;
            if let Destination::Disk { path } = &kept.dest
                && files::file_stamp(path)?.map(|stamp| stamp.device) != kept.disk
            {
                return Err(Error::Message("Reconnect the selected backup disk."));
            }
            if let Destination::Computer {
                computer,
                ssh: Some(target),
            } = &kept.dest
            {
                if !self.member(computer) {
                    return Err(Error::Message(
                        "Choose a current member of this setup for backups.",
                    ));
                }
                let store =
                    SshStore::new(self.backups.root(), &self.account, target, &kept.source)?;
                store.verify(computer, &self.backups.root().join("backup"))?;
            }
            let store = self.store(&kept.dest, &kept.source)?;
            let key = RecoveryKey::parse(&kept.key)?;
            let point = self.backups.export_locked(
                &*store,
                super::PointTarget {
                    source: &kept.source,
                    key: &key,
                    id: &id,
                    at: now,
                },
                &authority,
                super::CaptureSelection {
                    projects: &kept.projects,
                    attachments: None,
                },
                scratch,
                || Ok(()),
            )?;
            kept.last = Some(point.clone());
            kept.last_daily = Some(point.clone());
            kept.error = None;
            self.save(kept)?;
            for expired in expired_points(&store.list()?, now)? {
                store.erase(&expired)?;
            }
            Ok(Some(point))
        })();
        if let Err(error) = &work {
            kept.error = Some(error.to_string());
            self.save(kept)?;
        }
        work
    }

    fn destination(&self, request: Request, kept: Option<Config>) -> Result<Value> {
        if request.enabled == Some(false) {
            let (_guard, _scratch) = self.backups.scratch()?;
            let mut kept = self.config()?;
            if let Some(kept) = &mut kept {
                kept.enabled = false;
                self.save(kept)?;
            }
            return self.status(kept.as_ref());
        }
        let dest = destination(request.destination)?;
        let projects =
            super::projects::selected(request.projects, &self.backups.roots, &self.account)?;
        if request.confirm != Some(true) {
            let preview: Vec<_> = projects
                .iter()
                .map(|folder| json!({ "folder": folder, "gitStatus": "unavailable" }))
                .collect();
            return Ok(
                json!({ "destination": dest, "current": self.status(kept.as_ref())?, "projects": preview }),
            );
        }
        let (_guard, _scratch) = self.backups.scratch()?;
        let kept = self.config()?;
        let disk = match &dest {
            Destination::Disk { path } => {
                let disk = self.separate_disk(path)?;
                if projects
                    .iter()
                    .any(|folder| folder.starts_with(path) || path.starts_with(folder))
                {
                    return Err(Error::Message(
                        "Choose a backup folder separate from the selected projects.",
                    ));
                }
                disk
            }
            Destination::Computer { computer, .. } => {
                if !self.member(computer) {
                    return Err(Error::Message(
                        "Choose a current member of this setup for backups.",
                    ));
                }
                0
            }
        };
        let source = kept
            .as_ref()
            .map(|config| config.source.clone())
            .map_or_else(Source::generate, Ok)?;
        let key = match &kept {
            Some(config) => config.key.clone(),
            None => RecoveryKey::generate()?.expose(),
        };
        if let Destination::Computer {
            computer,
            ssh: Some(target),
        } = &dest
        {
            let store = SshStore::new(self.backups.root(), &self.account, target, &source)?;
            store.verify(computer, &self.backups.root().join("backup"))?;
        }
        self.store(&dest, &source)?.list()?;
        let (last, last_daily) = kept
            .filter(|config| config.dest == dest)
            .map_or((None, None), |config| (config.last, config.last_daily));
        let config = Config {
            version: 1,
            enabled: true,
            key,
            source,
            disk: matches!(dest, Destination::Disk { .. }).then_some(disk),
            dest,
            projects,
            last,
            last_daily,
            error: None,
        };
        self.save(&config)?;
        let mut status = self.status(Some(&config))?;
        status["recoveryKey"] = json!(config.key);
        Ok(status)
    }

    fn handle(&self, action: Action, request: Request) -> Result<(u16, Value)> {
        let mut config = self.config()?;
        let reply = match action {
            Action::Read => {
                let mut status = self.status(config.as_ref())?;
                if let Some(journal) =
                    files::read(&self.backups.root().join("backup/restore.json"))?
                {
                    let mut journal: Value = serde_json::from_slice(&journal)?;
                    if let Some(journal) = journal.as_object_mut() {
                        journal.shift_remove("formatVersion");
                    }
                    status["restore"] = journal;
                }
                status
            }
            Action::Destination => self.destination(request, config)?,
            Action::Pending => super::pending::list(&self.backups, self.queue_review.as_deref())?,
            Action::ReviewPending => {
                if request.kind.as_deref() != Some("queueItem") {
                    return Err(Error::Message(
                        "Check whether this work already ran, then confirm it’s still due.",
                    ));
                }
                let review = self
                    .queue_review
                    .as_deref()
                    .ok_or(Error::Message("Work Queue review is unavailable."))?;
                super::pending::settle(
                    &self.backups,
                    review,
                    request.id.as_deref().unwrap_or_default(),
                    request.operation.as_deref().unwrap_or_default(),
                    request.decision.as_deref().unwrap_or_default(),
                )?
            }
            Action::Match => {
                let _operation = self.backups.scratch()?;
                let source = request.source.unwrap_or_default().try_into().map_err(|_| {
                    Error::Message("Choose the old computer and its new membership.")
                })?;
                let command = crate::actor::Command::BackupMatch {
                    source,
                    previous: request.previous.unwrap_or_default(),
                    replacement: request.replacement.unwrap_or_default(),
                };
                let response = tokio::runtime::Handle::current().block_on(self.setup.request(command))
                    .map_err(|_| Error::Message("On the main computer, remove the old membership and add the replacement with fresh keys first."))?;
                return Ok((response.status, response.value));
            }
            Action::Now => {
                if config.as_ref().is_none_or(|config| !config.enabled) {
                    return Ok((200, json!({ "point": null })));
                }
                // Read and publish configuration under the same operation lock.
                let (_guard, scratch) = self.backups.scratch()?;
                config = self.config()?;
                json!({ "point": config.as_mut().map(|config| self.before(config, scratch.path())).transpose()?.flatten() })
            }
            Action::VaultExport | Action::VaultImport => {
                let dest = destination(
                    request
                        .destination
                        .or_else(|| config.as_ref().map(|kept| json!(kept.dest))),
                )?;
                if request.confirm != Some(true) {
                    return Ok((
                        400,
                        json!({ "message": "Confirm this backup operation first." }),
                    ));
                }
                let Some(vault) = self.vault.as_deref().filter(|vault| vault.holds_vault()) else {
                    return Ok((
                        503,
                        json!({ "message": "Open the vault on this computer first." }),
                    ));
                };
                let source = request
                    .source
                    .or_else(|| config.as_ref().map(|kept| kept.source.to_string()));
                if matches!(action, Action::VaultExport) {
                    let Some(kept) = config.filter(|kept| {
                        kept.enabled
                            && kept.dest == dest
                            && source.as_deref() == Some(kept.source.0.as_str())
                    }) else {
                        return Err(Error::Message(
                            "Choose a separate backup destination before exporting the vault.",
                        ));
                    };
                    let store = self.store(&dest, &kept.source)?;
                    json!(self.backups.export_vault(
                        &*store,
                        &kept.source,
                        vault,
                        jiff::Timestamp::now().as_millisecond(),
                    )?)
                } else {
                    let Some(key) = request.recovery_key else {
                        return Ok((
                            400,
                            json!({ "message": "Choose a restore point and enter its recovery key." }),
                        ));
                    };
                    let id: PointId = request
                        .id
                        .unwrap_or_default()
                        .try_into()
                        .map_err(|_| Error::Message("Choose a vault export."))?;
                    if id.reason() != Reason::Vault {
                        return Err(Error::Message("Choose a vault export."));
                    }
                    let source: Source = source.unwrap_or_default().try_into()?;
                    let store = self.store(&dest, &source)?;
                    json!(self.backups.import_vault(
                        &*store,
                        &source,
                        &id,
                        &RecoveryKey::parse(&key)?,
                        vault
                    )?)
                }
            }
            Action::Points | Action::Restore | Action::Erase => {
                let dest = request
                    .destination
                    .or_else(|| config.as_ref().map(|config| json!(config.dest)));
                let dest = destination(dest)?;
                let source = request
                    .source
                    .map(Source::try_from)
                    .transpose()?
                    .or_else(|| config.as_ref().map(|config| config.source.clone()))
                    .ok_or(Error::Message("Choose the backup's computer."))?;
                if matches!(action, Action::Points) {
                    return Ok((
                        200,
                        json!({ "points": self.store(&dest, &source)?.list()? }),
                    ));
                }
                if request.confirm != Some(true) {
                    return Ok((
                        400,
                        json!({ "message": "Confirm this backup operation first." }),
                    ));
                }
                if matches!(action, Action::Restore) && request.recovery_key.is_none() {
                    return Ok((
                        400,
                        json!({ "message": "Choose a restore point and enter its recovery key." }),
                    ));
                }
                let id: PointId = request.id.unwrap_or_default().try_into()?;
                let store = self.store(&dest, &source)?;
                if matches!(action, Action::Erase) {
                    let (_guard, _scratch) = self.backups.scratch()?;
                    store.erase(&id)?;
                    if let Some(mut kept) = self
                        .config()?
                        .filter(|config| config.dest == dest && config.source == source)
                    {
                        if kept.last.as_ref().is_some_and(|point| point.id == id) {
                            kept.last = None;
                        }
                        if kept.last_daily.as_ref().is_some_and(|point| point.id == id) {
                            kept.last_daily = None;
                        }
                        self.save(&kept)?;
                    }
                    json!({ "erased": id })
                } else {
                    let key = RecoveryKey::parse(&request.recovery_key.expect("checked key"))?;
                    json!(
                        self.backups
                            .restore_with(&*store, &source, &key, &id, || self.boundary(true))?
                    )
                }
            }
        };
        Ok((200, reply))
    }
}

fn destination(given: Option<Value>) -> Result<Destination> {
    let invalid =
        || Error::Message("Choose an enrolled backup computer or a folder on a connected disk.");
    let given = given.ok_or_else(invalid)?;
    if given.get("ssh").is_some_and(Value::is_null) {
        return Err(invalid());
    }
    match serde_json::from_value(given).map_err(|_| invalid())? {
        Destination::Disk { path }
            if path.is_absolute() && !path.to_string_lossy().contains('\0') =>
        {
            let path = path.canonicalize().unwrap_or(path);
            #[cfg(windows)]
            let path = kuvo_os::platform::windows_conventional_path(&path).unwrap_or(path);
            Ok(Destination::Disk { path })
        }
        Destination::Computer { computer, ssh }
            if super::url_token(&computer, 43)
                && ssh.as_deref().is_none_or(super::destination::valid_ssh) =>
        {
            Ok(Destination::Computer { computer, ssh })
        }
        _ => Err(invalid()),
    }
}

#[derive(Clone, Copy)]
enum Action {
    Read,
    Destination,
    Now,
    Match,
    Pending,
    ReviewPending,
    Points,
    Restore,
    Erase,
    VaultExport,
    VaultImport,
}

fn local_app(caller: &Caller) -> bool {
    matches!(
        caller,
        Caller::Local(Transport::Http {
            origin: LocalOrigin::App | LocalOrigin::Mac,
            ..
        })
    )
}

fn no_store<R: RouteSpec>(mut answer: RouteReply<R>) -> RouteReply<R> {
    answer
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    answer
}

// The local-app operations are legacy routes in TS. Keep their looser decoding
// and register exact paths so they never fall through to the engine.
macro_rules! legacy {
    ($name:ident, $method:literal, $path:literal, $body:expr) => {
        struct $name;
        impl RouteSpec for $name {
            type Body = Request;
            const ROUTE: &'static Route = &Route {
                method: $method,
                path: concat!("kuvo/box/backups", $path),
                feature: "box.backups",
                answers: &[
                    (200, &ANY),
                    (400, &ANY),
                    (403, &ANY),
                    (409, &ANY),
                    (503, &ANY),
                ],
                body: $body,
            };
        }
        impl JsonStatus<200> for $name {
            type Body = Value;
        }
        impl JsonStatus<400> for $name {
            type Body = Value;
        }
        impl JsonStatus<403> for $name {
            type Body = Value;
        }
        impl JsonStatus<409> for $name {
            type Body = Value;
        }
        impl JsonStatus<503> for $name {
            type Body = Value;
        }
    };
}
static ANY: Shape = Shape::Primitive(Primitive::Any);
legacy!(Read, "GET", "", None);
legacy!(Pending, "GET", "/pending", None);
legacy!(ReviewPending, "POST", "/pending", Some(&ANY));
legacy!(Configure, "POST", "/destination", Some(&ANY));
legacy!(Now, "POST", "/now", Some(&ANY));
legacy!(Match, "POST", "/match", Some(&ANY));
legacy!(Points, "POST", "/points", Some(&ANY));
legacy!(Restore, "POST", "/restore", Some(&ANY));
legacy!(Erase, "POST", "/erase", Some(&ANY));
legacy!(VaultExport, "POST", "/vault", Some(&ANY));
legacy!(VaultImport, "POST", "/vault/import", Some(&ANY));

fn register_local<R>(routes: &mut Registry, service: &Arc<Service>, action: Action)
where
    R: RouteSpec<Body = Request>
        + JsonStatus<200, Body = Value>
        + JsonStatus<400, Body = Value>
        + JsonStatus<403, Body = Value>
        + JsonStatus<409, Body = Value>
        + JsonStatus<503, Body = Value>,
{
    let service = service.clone();
    routes.register_decoded::<R, _, _, _>(
        Access::Any,
        |body| {
            let value: Value = serde_json::from_slice(body).unwrap_or_default();
            Ok(serde_json::from_value(value).unwrap_or_default())
        },
        move |call| {
            let service = service.clone();
            async move {
                if !local_app(&call.request.caller) {
                    return Ok(no_store(RouteReply::json::<403>(
                        json!({ "message": "Open backups on this computer." }),
                    )));
                }
                let reply =
                    tokio::task::spawn_blocking(move || service.handle(action, call.body)).await;
                Ok(no_store(match reply {
                    Ok(Ok((200, value))) => RouteReply::json::<200>(value),
                    Ok(Ok((400, value))) => RouteReply::json::<400>(value),
                    Ok(Ok((409, value))) => RouteReply::json::<409>(value),
                    Ok(Ok((503, value))) => RouteReply::json::<503>(value),
                    Ok(Err(error)) => {
                        RouteReply::json::<409>(json!({ "message": error.to_string() }))
                    }
                    _ => {
                        return Err(Refusal::new(
                            http::StatusCode::SERVICE_UNAVAILABLE,
                            "Backup status is unavailable.",
                        ));
                    }
                }))
            }
        },
    );
}

pub fn register(
    routes: &mut Registry,
    home: Home,
    account: PathBuf,
    setup: SetupHandle,
    relay: Option<super::relay::mail::PeerMail>,
    vault: Option<Arc<dyn super::vault::VaultBackend>>,
    queue_review: Option<Arc<dyn super::QueueReview>>,
) -> Resident {
    let service = Arc::new(Service {
        backups: Backups::with_environment(home, &std::env::vars_os().collect()),
        setup,
        account,
        relay,
        vault,
        queue_review,
    });
    let health = service.clone();
    routes.register::<health::Spec, _, _>(Access::Any, move |call| {
        let service = health.clone();
        async move {
            if !local_app(&call.request.caller) && call.request.caller.pairing().is_none() {
                return Ok(no_store(RouteReply::json::<403>(health::Status403 {
                    message: "Open backup status from a paired Kuvo Code client.".into(),
                })));
            }
            let status = tokio::task::spawn_blocking(move || {
                let config = service.read_config()?;
                Ok::<_, Error>(service.health(config.as_ref()))
            })
            .await;
            let status = status
                .ok()
                .and_then(Result::ok)
                .and_then(|value| serde_json::from_value(value).ok());
            Ok(no_store(match status {
                Some(status) => RouteReply::json::<200>(status),
                None => RouteReply::json::<503>(health::Status503 {
                    message: "Backup status is unavailable.".into(),
                }),
            }))
        }
    });
    register_local::<Read>(routes, &service, Action::Read);
    register_local::<Pending>(routes, &service, Action::Pending);
    register_local::<ReviewPending>(routes, &service, Action::ReviewPending);
    register_local::<Configure>(routes, &service, Action::Destination);
    register_local::<Now>(routes, &service, Action::Now);
    register_local::<Match>(routes, &service, Action::Match);
    register_local::<Points>(routes, &service, Action::Points);
    register_local::<Restore>(routes, &service, Action::Restore);
    register_local::<Erase>(routes, &service, Action::Erase);
    register_local::<VaultExport>(routes, &service, Action::VaultExport);
    register_local::<VaultImport>(routes, &service, Action::VaultImport);
    let daily = service.clone();
    let task = tokio::spawn(async move {
        let mut check = tokio::time::interval_at(
            tokio::time::Instant::now() + std::time::Duration::from_secs(60),
            std::time::Duration::from_secs(15 * 60),
        );
        check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            check.tick().await;
            let service = daily.clone();
            let _ = tokio::task::spawn_blocking(move || {
                if service.config()?.is_some_and(|config| config.enabled) {
                    service.handle(Action::Now, Request::default())?;
                }
                Ok::<_, Error>(())
            })
            .await;
        }
    });
    Resident { service, task }
}

/// Keeps the backup service and its scheduler alive for this resident.
pub struct Resident {
    service: Arc<Service>,
    task: tokio::task::JoinHandle<()>,
}

impl Resident {
    /// Verify the current configured point before the removal owner commits its
    /// receipt. Call on a blocking worker, as readback may use SSH or Relay.
    pub fn verify_configured_wipe<T>(
        &self,
        source: &Source,
        id: &PointId,
        commit: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let service = &self.service;
        let backups = &service.backups;
        let (_guard, scratch) = backups.scratch()?;
        let kept = service
            .config()?
            .filter(|kept| &kept.source == source)
            .ok_or(Error::Message("The pre-wipe backup configuration changed."))?;
        if let Destination::Disk { path } = &kept.dest
            && files::file_stamp(path)?.map(|stamp| stamp.device) != kept.disk
        {
            return Err(Error::Message("Reconnect the selected backup disk."));
        }
        let store = service.store(&kept.dest, source)?;
        backups.verify_locked(
            &*store,
            source,
            &RecoveryKey::parse(&kept.key)?,
            id,
            scratch.path(),
        )?;
        commit()
    }
}

impl Drop for Resident {
    fn drop(&mut self) {
        self.task.abort();
    }
}
