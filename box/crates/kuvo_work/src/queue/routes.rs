use super::{
    github::Seen,
    items, labels, numbers,
    peer_reads::PeerReads,
    refs::References,
    sessions::Sessions,
    store::{Error, Store, owner_id},
    told,
    view::{self, Query},
};
use http::{HeaderValue, StatusCode, header::CONTENT_TYPE};
use kuvo_engine::LocalSessions;
use kuvo_os::watch::{Options, Subscription, WatchHub};
use kuvo_peers::{
    memory_read::Handle as MemoryRead, ownership, relay::RelayHandle, setup_lists::Reader,
};
use kuvo_protocol::{
    JsonStatus,
    routes::{
        get_kuvo_work_queue, get_kuvo_work_queue_events as events, get_kuvo_work_queue_items_item,
        get_kuvo_work_queue_items_item_files_name, get_kuvo_work_queue_refs,
        post_kuvo_work_queue_refs,
    },
};
use kuvo_server::{
    Access, Caller, JsonLine, LocalOrigin, Refusal, Registry, RouteReply, Transport,
};
use kuvo_setup::SetupHandle;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::{io, path::Path, sync::Arc, time::Duration};

pub async fn register(
    registry: &mut Registry,
    home: &Path,
    peers: Reader,
    sessions: LocalSessions,
    setup: SetupHandle,
    relay: Option<RelayHandle>,
    memory: MemoryRead,
    owners: ownership::Handle,
) -> Result<Runtime, Box<dyn std::error::Error>> {
    let store = Store::new(home);
    let startup = store.clone();
    tokio::task::spawn_blocking(move || {
        numbers::check(&startup)?;
        let _ = kuvo_store::files::create_private_dirs(&startup.root.join("forward"));
        startup.items()?;
        Ok::<_, Error>(())
    })
    .await??;

    let references = Arc::new(References::new());
    let lists = Views {
        store: store.clone(),
        parser: references.clone(),
        peers: peers.clone(),
        sessions: sessions.clone(),
        setup: setup.clone(),
    };
    let reader = lists.clone();
    registry.register::<get_kuvo_work_queue::Spec, _, _>(Access::Any, move |call| {
        let reader = reader.clone();
        let query = query(&call.request.path);
        async move { reader.read(Arc::new(query?)).await.and_then(typed) }
    });
    follow(registry, lists.clone());

    let reads = PeerReads {
        store: store.clone(),
        refs: references.clone(),
        peers: peers.clone(),
        sessions: sessions.clone(),
        transport: memory,
    };
    labels::register(registry, store.clone(), setup.clone());
    let runtime = items::register(
        registry,
        store.clone(),
        setup,
        peers,
        references.clone(),
        sessions.clone(),
        relay,
        reads.clone(),
        owners,
    );

    let here = lists.clone();
    registry.register::<get_kuvo_work_queue_refs::Spec, _, _>(Access::Any, move |call| {
        let here = here.clone();
        let query = call
            .request
            .path
            .split_once('?')
            .map_or("", |(_, query)| query);
        let listed = url::form_urlencoded::parse(query.as_bytes())
            .find(|(key, _)| key == "refs")
            .map_or_else(String::new, |(_, value)| value.into_owned());
        async move {
            blocking(move || here.references(listed.split(',')))
                .await
                .and_then(typed)
        }
    });

    let here = lists;
    registry.register::<post_kuvo_work_queue_refs::Spec, _, _>(Access::Any, move |call| {
        let here = here.clone();
        async move {
            let body: Value = serde_json::from_slice(&call.request.body).unwrap_or(Value::Null);
            if let Some(text) = body["text"].as_str() {
                if !work_queue_client(&call.request.caller) {
                    return Err(Refusal::new(StatusCode::FORBIDDEN,
                        "Only Kuvo Code on your Mac or iPhone can do this: agents change the Work Queue with their tools."));
                }
                let text = super::store::limit(text, 100_000);
                return blocking(move || Ok(json!({"mentions":here.parser.mentions.find(&text)}))).await.and_then(typed);
            }
            blocking(move || {
                let refs = super::store::array(body.get("refs")).iter().filter_map(Value::as_str);
                here.references(refs)
            }).await.and_then(typed)
        }
    });

    let here = reads.clone();
    registry.register::<get_kuvo_work_queue_items_item::Spec, _, _>(Access::Any, move |call| {
        let here = here.clone();
        let name = decode(call.param("item"));
        let client = work_queue_client(&call.request.caller);
        async move { typed(here.item(name, client).await?) }
    });

    let here = reads.clone();
    registry.register::<get_kuvo_work_queue_items_item_files_name::Spec, _, _>(
        Access::Any,
        move |call| {
            let here = here.clone();
            let reference = decode(call.param("item"));
            let name = decode(call.param("name"));
            let client = work_queue_client(&call.request.caller);
            async move {
                let file = here.file(reference, name, client).await?;
                let mut answer = RouteReply::bytes::<200>("application/octet-stream", file.bytes);
                answer.headers_mut().insert(
                    CONTENT_TYPE,
                    HeaderValue::from_str(&file.media_type).map_err(internal)?,
                );
                Ok(answer)
            }
        },
    );
    Ok(Runtime {
        _writer: runtime,
        reads,
    })
}

pub struct Runtime {
    _writer: items::Runtime,
    reads: PeerReads,
}

impl Runtime {
    pub fn peer_reads(&self) -> PeerReads {
        self.reads.clone()
    }
}

#[derive(Clone)]
struct Views {
    store: Store,
    parser: Arc<References>,
    peers: Reader,
    sessions: LocalSessions,
    setup: SetupHandle,
}

impl Views {
    fn references<'a>(&self, listed: impl IntoIterator<Item = &'a str>) -> Result<Value, Error> {
        let local = self.store.items()?;
        let seen = Seen::read(&self.store)?;
        let others = told::items_elsewhere(
            &self.store,
            &local,
            &self.peers.view(),
            "all",
            &Sessions::default(),
            &[],
            self.peers.now(),
        )?;
        Ok(view::references(
            &local,
            &others.items,
            &self.parser,
            &seen,
            listed,
        ))
    }

    async fn read(&self, query: Arc<Query>) -> Result<Value, Refusal> {
        let here = self.clone();
        let key = self
            .peers
            .view()
            .roster
            .here
            .map_or_else(String::new, |key| kuvo_wire::keys::encode(&key));
        blocking(move || {
            view::list(
                &here.store,
                &query,
                &key,
                &here.parser,
                &Sessions::read(here.sessions.latest_record()),
                here.setup.queue_setup().as_ref(),
                &here.peers.view(),
            )
        })
        .await
    }
}

fn query(path: &str) -> Result<Query, Refusal> {
    let query = Query::read(path);
    if query.owner != "all" && !owner_id(&query.owner) {
        return Err(Refusal::new(
            StatusCode::BAD_REQUEST,
            "Say whose queue (owner): a conversation's address, a group chat's id, or all.",
        ));
    }
    Ok(query)
}

fn follow(registry: &mut Registry, views: Views) {
    let watchers = WatchHub::default();
    registry.register::<events::Spec, _, _>(Access::Any, move |call| {
        let reader = views.clone();
        let watchers = watchers.clone();
        let requested = query(&call.request.path);
        async move {
            let query = Arc::new(requested?);
            let folder = &reader.store.root;
            let peers = folder.join("peers");
            let _ = tokio::task::spawn_blocking(move || kuvo_store::files::create_private_dirs(&peers)).await;
            let mut files = watchers.subscribe(folder, Options { recursive: true, create: false }).await.ok();
            let mut catalog = reader.sessions.clone();
            Ok(RouteReply::ndjson::<200>(async_stream::try_stream! {
                let mut last = None;
                loop {
                    let Ok(value) = reader.read(query.clone()).await else { break };
                    if last.as_ref() != Some(&value) {
                        let answer: events::Status200 = serde_json::from_value(value.clone()).map_err(io::Error::other)?;
                        last = Some(value);
                        yield JsonLine::new(answer)?;
                    }
                    tokio::select! {
                        () = call.cancel.cancelled() => break,
                        () = queue_change(&mut files) => {},
                        changed = catalog.changed() => if !changed { break },
                        () = tokio::time::sleep(Duration::from_secs(3)) => {},
                    }
                }
            }))
        }
    });
}

async fn queue_change(watcher: &mut Option<Subscription>) {
    use kuvo_os::watch::Notice;
    if let Some(watcher) = watcher {
        while let Some(notice) = watcher.recv().await {
            match notice {
                Notice::Changed { paths }
                    if !paths.iter().any(|path| {
                        path.components().next().is_some_and(|part| {
                            ["queues", "peers", "labels.json", "seen.json"]
                                .iter()
                                .any(|name| part.as_os_str() == *name)
                        })
                    }) =>
                {
                    continue;
                }
                _ => return,
            }
        }
    }
    std::future::pending::<()>().await;
}

pub(super) fn decode(value: Option<&str>) -> String {
    percent_encoding::percent_decode_str(value.unwrap_or_default())
        .decode_utf8()
        .unwrap_or_default()
        .into_owned()
}

pub(super) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> Result<T, Refusal> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(internal)?
        .map_err(storage_refusal)
}

pub(super) fn storage_refusal(error: Error) -> Refusal {
    match error {
        Error::Format(kuvo_store::format::Error::Unsupported { .. }) => Refusal::json(
            StatusCode::CONFLICT,
            json!({
                "code": "unsupportedFormatVersion",
                "message": error.to_string(),
            }),
        ),
        _ => internal(error),
    }
}

pub(super) fn internal(error: impl std::fmt::Display) -> Refusal {
    Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

pub(super) fn typed<R: JsonStatus<200>>(value: Value) -> Result<RouteReply<R>, Refusal>
where
    <R as JsonStatus<200>>::Body: DeserializeOwned,
{
    serde_json::from_value(value)
        .map(RouteReply::json::<200>)
        .map_err(internal)
}

pub(super) fn work_queue_client(caller: &Caller) -> bool {
    local_work_queue_app(caller) || caller.pairing().is_some_and(|pairing| pairing.is_current())
}

pub(super) fn local_work_queue_app(caller: &Caller) -> bool {
    matches!(
        caller,
        Caller::Local(Transport::Http {
            origin: LocalOrigin::App | LocalOrigin::Mac,
            ..
        })
    )
}
