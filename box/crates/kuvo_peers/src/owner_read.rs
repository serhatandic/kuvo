//! Current session ownership and the last authoritative page from its owner.

use std::{
    collections::HashSet,
    io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
    time::Duration,
};

use http::StatusCode;
use kuvo_engine::Engine;
use kuvo_protocol::routes::get_kuvo_data_owner as route;
use kuvo_server::{Access, Refusal, Registry, RouteCall, RouteReply};
use kuvo_store::{files, format::ObjectFormat, locks::OwnedLock};
use kuvo_wire::keys::{decode_exact, encode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    memory_read::{Answer, Handle as Reads, Kind, Read},
    ownership::{Handle as Owners, PairingState, PeerOwner},
    setup_lists::Reader,
};

pub const FEATURE: &str = "data.ownership";
const FORMAT: ObjectFormat = ObjectFormat::additive("Owned data");
const MAX_READ: usize = 512 * 1024;

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
enum DataKind {
    Conversations,
    SessionFiles,
    SessionQueue,
    ConversationSettings,
    SessionNotes,
}

#[derive(Serialize, Deserialize)]
struct Reference {
    kind: DataKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    offset: Option<String>,
}

impl Reference {
    fn readable(&self, id: &str) -> bool {
        session_id(id)
            && self.offset.as_deref().is_none_or(|offset| {
                matches!(self.kind, DataKind::Conversations)
                    && (1..=200).contains(&offset.len())
                    && offset.bytes().all(|b| token(b) || b == b':')
            })
            && matches!(
                self.kind,
                DataKind::Conversations | DataKind::SessionFiles | DataKind::SessionQueue
            )
    }
}

fn token(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}

fn session_id(id: &str) -> bool {
    id.strip_prefix("ses_")
        .is_some_and(|tail| !tail.is_empty() && id.len() <= 100 && tail.bytes().all(token))
}

#[derive(Clone, Default, Deserialize, Serialize)]
struct Snapshot {
    at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value: Option<Value>,
    #[serde(default)]
    absent: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    generation: Option<String>,
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    messages: Vec<String>,
}

#[derive(Default, Deserialize, Serialize)]
struct Mark {
    #[serde(flatten)]
    saved: Snapshot,
    #[serde(default)]
    started: i64,
    #[serde(default)]
    revision: Option<String>,
    #[serde(default)]
    cleared: bool,
    #[serde(default)]
    withheld: bool,
}

struct Cache {
    mark: PathBuf,
    page: PathBuf,
}

impl Cache {
    fn new(run: &Path, setup: &str, owner: &str, kind: &str, id: &str, offset: &str) -> Self {
        let root = run.join("owned-data/reads");
        let key = |parts: &[&str]| {
            let bytes = serde_json::to_vec(parts).expect("snapshot address is serializable");
            root.join(format!("{}.json", encode(&Sha256::digest(bytes))))
        };
        Self {
            mark: key(&[setup, owner, kind, id]),
            page: key(&[setup, owner, kind, id, offset]),
        }
    }

    async fn mark(&self) -> io::Result<Mark> {
        let path = self.mark.clone();
        tokio::task::spawn_blocking(move || Ok(read::<Mark>(&path)?.unwrap_or_default())).await?
    }

    async fn observe(
        self,
        before: Mark,
        started: i64,
        at: i64,
        answer: Option<Answer>,
    ) -> io::Result<(Option<Snapshot>, Refresh)> {
        tokio::task::spawn_blocking(move || {
            let _guard =
                OwnedLock::acquire(&self.mark.with_extension("lock"), Duration::from_secs(2))?;
            let mut mark = read::<Mark>(&self.mark)?.unwrap_or_default();
            let last_page = |mark: &Mark| -> io::Result<Option<Snapshot>> {
                if mark.withheld {
                    return Ok(None);
                }
                let page = read::<Snapshot>(&self.page)?
                    .filter(|page| page.generation == mark.saved.generation);
                Ok(page
                    .or_else(|| (mark.saved.absent && !mark.cleared).then(|| mark.saved.clone())))
            };
            let status = answer.as_ref().map(|answer| answer.status);
            let refresh = Refresh::for_status(status);
            if !matches!(status, Some(200 | 401 | 403 | 404 | 409)) {
                return Ok((
                    if matches!(refresh, Refresh::Failed | Refresh::Unavailable) {
                        last_page(&mark)?
                    } else {
                        None
                    },
                    refresh,
                ));
            }
            if mark.revision != before.revision
                && (mark.started >= started || mark.saved.generation != before.saved.generation)
            {
                let refresh = if mark.withheld {
                    Refresh::Withheld
                } else {
                    Refresh::Failed
                };
                return Ok((last_page(&mark)?, refresh));
            }
            let mut nonce = [0; 16];
            getrandom::fill(&mut nonce).map_err(io::Error::other)?;
            let revision = encode(&nonce);
            mark.started = started;
            mark.revision = Some(revision.clone());
            if let Some(Answer { status: 200, value }) = answer {
                let mut page = snapshot(at, value);
                page.generation.clone_from(&mark.saved.generation);
                mark.cleared = true;
                mark.withheld = false;
                keep(&self.page, &page)?;
                keep(&self.mark, &mark)?;
                return Ok((Some(page), Refresh::Live));
            }
            mark = Mark {
                saved: Snapshot {
                    at,
                    absent: true,
                    generation: Some(revision),
                    ..Snapshot::default()
                },
                started,
                revision: mark.revision,
                cleared: false,
                withheld: !matches!(status, Some(404)),
            };
            keep(&self.mark, &mark)?;
            Ok(((!mark.withheld).then_some(mark.saved), refresh))
        })
        .await?
    }
}

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    FORMAT
        .read(path)
        .map_err(io::Error::other)?
        .map(|saved| serde_json::from_value(Value::Object(saved.fields)).map_err(io::Error::other))
        .transpose()
}

fn keep(path: &Path, saved: &impl Serialize) -> io::Result<()> {
    let fields = serde_json::to_value(saved)?;
    let Value::Object(fields) = fields else {
        return Err(io::Error::other("The owner snapshot isn't an object."));
    };
    files::write_secret(path, &serde_json::to_vec(&FORMAT.encode(fields))?)
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
enum Refresh {
    Live,
    Unavailable,
    Failed,
    Withheld,
    MetadataOnly,
}

impl Refresh {
    fn for_status(status: Option<u16>) -> Self {
        match status {
            Some(200 | 404) => Self::Live,
            Some(500..=599) => Self::Failed,
            None => Self::Unavailable,
            _ => Self::Withheld,
        }
    }
}

type Query = Vec<(String, String)>;

fn response(
    mut owner: PeerOwner,
    saved: Option<Snapshot>,
    refresh: Refresh,
    query: &Query,
    available_files: &[String],
) -> Result<RouteReply<route::Spec>, Refusal> {
    if matches!(refresh, Refresh::Unavailable) {
        owner.reachable = false;
    }
    let stale = match refresh {
        Refresh::Live => false,
        Refresh::MetadataOnly => !owner.reachable,
        _ => true,
    };
    let requested = |kind| {
        query
            .iter()
            .filter(move |(name, _)| name == kind)
            .take(100)
            .map(|(_, value)| value)
            .filter(|value| (1..=500).contains(&value.encode_utf16().count()))
    };
    let mut unique = HashSet::new();
    let unavailable_files = saved
        .iter()
        .flat_map(|page| &page.files)
        .chain(requested("file"))
        .filter(|name| !available_files.contains(name) && unique.insert(*name))
        .collect::<Vec<_>>();
    let mut unique = HashSet::new();
    let unavailable_messages = requested("message")
        .filter(|id| {
            saved
                .as_ref()
                .is_none_or(|page| !page.messages.contains(id))
                && unique.insert(*id)
        })
        .collect::<Vec<_>>();
    let state = match &saved {
        Some(page) if page.absent => "absent",
        Some(_) => "received",
        None => "neverReceived",
    };
    let mut view = json!({
        "owner": {"key":owner.key.map(|key| encode(&key)), "scope":"session computer", "name":owner.name,
            "local":owner.local, "reachable":owner.reachable},
        "stale":stale, "state":state, "refresh":refresh, "lastReceivedAt":saved.as_ref().map(|page| page.at),
        "unavailable":{"files": unavailable_files, "messages": unavailable_messages},
    });
    if let Some(value) = saved.and_then(|page| page.value) {
        view["data"] = value;
    }
    serde_json::from_value::<route::Status200>(view)
        .map(RouteReply::json::<200>)
        .map_err(storage_error)
}

fn storage_error(error: impl std::fmt::Display) -> Refusal {
    Refusal::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("Reading the owner snapshot: {error}"),
    )
}

fn snapshot(at: i64, value: Value) -> Snapshot {
    let rows = value["messages"].as_array();
    let messages = rows
        .into_iter()
        .flatten()
        .filter_map(|row| row["id"].as_str().map(str::to_owned))
        .collect();
    let files = value.as_array().map_or_else(
        || {
            crate::moves::owner_attachment_files(rows.map_or(&[], Vec::as_slice), None)
                .into_iter()
                .map(|(file, _)| file)
                .collect()
        },
        |files| {
            files
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        },
    );
    Snapshot {
        at,
        value: Some(value),
        messages,
        files,
        ..Snapshot::default()
    }
}

#[derive(Clone)]
pub struct Service {
    reader: Reader,
    owners: Owners,
    reads: Reads,
    engine: Engine,
    started: Arc<AtomicI64>,
}

impl Service {
    pub fn new(reader: Reader, owners: Owners, reads: Reads, engine: Engine) -> Self {
        Self {
            reader,
            owners,
            reads,
            engine,
            started: Arc::new(AtomicI64::new(0)),
        }
    }

    pub fn register(&self, routes: &mut Registry) {
        let service = self.clone();
        routes.register::<route::Spec, _, _>(Access::DesktopOrPaired, move |call| {
            let service = service.clone();
            async move { service.route(call).await }
        });
    }

    async fn route(&self, call: RouteCall<()>) -> Result<RouteReply<route::Spec>, Refusal> {
        let request = call.request;
        let invalid = || {
            Refusal::new(
                StatusCode::BAD_REQUEST,
                "Name a data kind and its record or computer.",
            )
        };
        let no_owner = || {
            Refusal::new(
                StatusCode::NOT_FOUND,
                "This box doesn’t know which computer owns that record.",
            )
        };
        let query: Query = form_urlencoded::parse(
            request
                .path
                .split_once('?')
                .map_or("", |(_, query)| query)
                .as_bytes(),
        )
        .into_owned()
        .collect();
        let get = |name| {
            query
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, val)| val.as_str())
        };
        let kind = get("kind").ok_or_else(invalid)?;
        let id = get("id").unwrap_or("_");
        let data = Reference {
            kind: serde_json::from_value(json!(kind)).map_err(|_| invalid())?,
            offset: get("offset").map(str::to_owned),
        };
        if !(1..=100).contains(&id.len())
            || !id.bytes().all(token)
            || get("queue").is_some_and(|value| {
                !value.is_empty()
                    && !(value == "usr_you"
                        || session_id(value)
                        || value.strip_prefix("grp_").is_some_and(|tail| {
                            !tail.is_empty()
                                && tail
                                    .bytes()
                                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
                        }))
            })
        {
            return Err(invalid());
        }
        if let Some(named) = get("computer").filter(|named| !named.is_empty()) {
            let key = decode_exact(named)
                .filter(|key| encode(key) == named)
                .ok_or_else(invalid)?;
            let view = self.reader.view();
            if view.roster.here != Some(key) && view.roster.member(&key).is_none() {
                return Err(Refusal::new(
                    StatusCode::NOT_FOUND,
                    "There’s no such computer in this setup.",
                ));
            }
        }
        if let Some(pair) = request.caller.pairing()
            && self.owners.current_pairing(&pair.key, &pair.id) != PairingState::Allowed
        {
            return Err(Refusal::new(
                StatusCode::FORBIDDEN,
                "Open this from Kuvo Code.",
            ));
        }
        if !session_id(id) {
            return Err(no_owner());
        }
        let scope = self.owners.scope(id, None).map_err(storage_error)?;
        let owner = scope.owner.ok_or_else(no_owner)?;
        if !data.readable(id) {
            return response(owner, None, Refresh::MetadataOnly, &query, &[]);
        }
        if owner.local {
            let (answer, available) = self.local(id, &data, true).await;
            let saved = match answer.status {
                200 => Some(snapshot(self.reader.now(), answer.value)),
                404 => Some(Snapshot {
                    at: self.reader.now(),
                    absent: true,
                    ..Snapshot::default()
                }),
                _ => None,
            };
            return response(
                owner,
                saved,
                Refresh::for_status(Some(answer.status)),
                &query,
                &available,
            );
        }

        let Some((setup, peer)) = scope.setup.zip(owner.key) else {
            return response(owner, None, Refresh::Unavailable, &query, &[]);
        };
        if self.reader.view().roster.member(&peer).is_none() {
            return response(owner, None, Refresh::Unavailable, &query, &[]);
        }
        if owner.reachable && !owner.features.iter().any(|feature| feature == FEATURE) {
            return response(owner, None, Refresh::Withheld, &query, &[]);
        }
        let cache = Cache::new(
            &self.reader.run,
            &setup,
            &encode(&peer),
            kind,
            id,
            data.offset.as_deref().unwrap_or(""),
        );
        let before = cache.mark().await.map_err(storage_error)?;
        let now = self.reader.now();
        let started = self
            .started
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |last| {
                Some(now.max(last.saturating_add(1)))
            })
            .unwrap_or_default()
            .saturating_add(1)
            .max(now);
        let answer = if owner.reachable {
            let body = serde_json::to_value(data).map_err(storage_error)?;
            call.cancel
                .run_until_cancelled(self.reads.request(
                    peer,
                    Read {
                        area: Some("ownership".into()),
                        item: id.into(),
                        kind: Kind::Record,
                        body: body.as_object().cloned(),
                    },
                ))
                .await
                .and_then(Result::ok)
        } else {
            None
        };
        let current = self.owners.scope(id, None).map_err(storage_error)?;
        let mut owner = current
            .owner
            .filter(|owner| owner.key == Some(peer) && !owner.local)
            .filter(|_| {
                current.setup.as_deref() == Some(setup.as_str())
                    && self.reader.view().roster.member(&peer).is_some()
            })
            .ok_or_else(|| {
                Refusal::new(
                    StatusCode::CONFLICT,
                    "The owner changed. Read the record again.",
                )
            })?;
        if answer.is_some() {
            owner.reachable = true;
        }
        let (snapshot, refresh) = cache
            .observe(before, started, self.reader.now(), answer)
            .await
            .map_err(storage_error)?;
        response(owner, snapshot, refresh, &query, &[])
    }

    pub async fn answer(&self, read: Read) -> Answer {
        let result = read
            .body
            .and_then(|body| serde_json::from_value::<Reference>(Value::Object(body)).ok());
        let answer = match result {
            Some(reference) if read.kind == Kind::Record && reference.readable(&read.item) => {
                self.local(&read.item, &reference, false).await.0
            }
            _ => empty(400),
        };
        if serde_json::to_vec(&answer.value).is_ok_and(|bytes| bytes.len() <= MAX_READ) {
            answer
        } else {
            empty(413)
        }
    }

    async fn local(&self, id: &str, data: &Reference, inventory: bool) -> (Answer, Vec<String>) {
        let here = || {
            self.owners
                .scope(id, None)
                .is_ok_and(|scope| !scope.held && scope.owner.is_some_and(|owner| owner.local))
        };
        if !here() {
            return (empty(409), Vec::new());
        }
        let result = match data.kind {
            DataKind::Conversations | DataKind::SessionFiles => {
                self.engine
                    .owner_message_page(id, data.offset.as_deref())
                    .await
            }
            DataKind::SessionQueue => self.engine.owner_queue(id).await,
            _ => return (empty(400), Vec::new()),
        };
        if !here() {
            return (empty(409), Vec::new());
        }
        match result {
            Ok(mut value) => {
                let mut files = Vec::new();
                if let Some(rows) = value["messages"].as_array()
                    && (inventory || matches!(data.kind, DataKind::SessionFiles))
                {
                    let home = inventory
                        .then(|| (self.reader.run.parent().unwrap_or(&self.reader.run), id));
                    files = crate::moves::owner_attachment_files(rows, home);
                }
                if matches!(data.kind, DataKind::SessionFiles) {
                    value = json!(files.iter().map(|(file, _)| file).collect::<Vec<_>>());
                }
                let available = files
                    .into_iter()
                    .filter_map(|(name, have)| have.then_some(name))
                    .collect();
                (Answer { status: 200, value }, available)
            }
            Err(refusal) => (empty(refusal.reply().status.as_u16()), Vec::new()),
        }
    }
}

fn empty(status: u16) -> Answer {
    Answer {
        status,
        value: Value::Null,
    }
}
