//! Authenticated relay sessions use the same dispatcher as HTTP and addressed SSH.

use std::collections::HashMap;

use http::{HeaderMap, HeaderName, HeaderValue, Method};
use http_body_util::BodyExt;
use kuvo_server::{Caller, DispatchRequest, Dispatcher, Pairing, Transport, ssh::Pairings};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::{
    devices::Devices,
    relay,
    session::{self, Id, Message},
};

const MAX_REQUESTS: usize = 64;
const MAX_BODY: usize = 16 * 1024 * 1024;
const CHUNK: usize = 48_000;

#[derive(Default)]
pub(crate) struct Routes {
    sessions: HashMap<Id, Session>,
    jobs: JoinSet<()>,
}

impl Drop for Routes {
    fn drop(&mut self) {
        for session in self.sessions.values() {
            session.cancel.cancel();
        }
    }
}

struct Session {
    grant: Pairing,
    cancel: CancellationToken,
    requests: HashMap<u64, Request>,
}

enum Request {
    Reading(Box<Pending>),
    Running(CancellationToken),
}

struct Pending {
    request: DispatchRequest,
    body: Vec<u8>,
}

#[derive(Deserialize)]
struct Head {
    id: u64,
    #[serde(flatten)]
    request: Input,
}

#[derive(Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
enum Input {
    Req {
        #[serde(rename = "m", default = "get")]
        method: String,
        #[serde(rename = "p")]
        path: String,
        #[serde(rename = "h", default)]
        headers: HashMap<String, String>,
        end: Option<bool>,
    },
    Data {
        end: Option<bool>,
    },
    Cancel,
}

fn get() -> String {
    "GET".into()
}

impl Routes {
    pub(crate) fn event(
        &mut self,
        event: session::Event,
        relay: &relay::Sender,
        dispatcher: Option<&Dispatcher>,
        devices: Option<&Devices>,
    ) {
        while self.jobs.try_join_next().is_some() {}
        match event {
            session::Event::Accepted(id, device) => {
                let Some(devices) = devices.cloned() else {
                    return;
                };
                let key = device.key;
                let grant = device.grant;
                let paired =
                    Pairing::verified(key, grant.clone(), device.hello.name, true, move || {
                        devices
                            .paired(&key)
                            .is_some_and(|current| current.id == grant)
                    });
                if let Some(old) = self.sessions.insert(
                    id,
                    Session {
                        grant: paired,
                        cancel: CancellationToken::new(),
                        requests: HashMap::new(),
                    },
                ) {
                    old.cancel.cancel();
                }
            }
            session::Event::Ended(id) => {
                if let Some(session) = self.sessions.remove(&id) {
                    session.cancel.cancel();
                }
            }
            session::Event::Message(id, message) => {
                if let Some(dispatcher) = dispatcher {
                    self.message(id, message, relay, dispatcher);
                }
            }
            _ => {}
        }
    }

    async fn job_result(&mut self) {
        let _ = self.jobs.join_next().await;
    }

    fn message(
        &mut self,
        id: Id,
        message: Message,
        relay: &relay::Sender,
        dispatcher: &Dispatcher,
    ) {
        let Some(session) = self.sessions.get_mut(&id) else {
            return;
        };
        session.requests.retain(|_, request| match request {
            Request::Running(cancel) => !cancel.is_cancelled(),
            Request::Reading(_) => true,
        });
        let Ok(head) = serde_json::from_value::<Head>(message.head) else {
            return;
        };
        if matches!(head.request, Input::Cancel) {
            if let Some(Request::Running(cancel)) = session.requests.remove(&head.id) {
                cancel.cancel();
            }
            return;
        }
        let request = match head.request {
            Input::Req {
                method,
                path,
                headers,
                end,
            } => {
                if session.requests.len() >= MAX_REQUESTS || session.requests.contains_key(&head.id)
                {
                    return;
                }
                let Ok(method) = Method::from_bytes(method.as_bytes()) else {
                    return;
                };
                let mut map = HeaderMap::new();
                for (name, value) in headers {
                    let (Ok(name), Ok(value)) = (
                        HeaderName::from_bytes(name.as_bytes()),
                        HeaderValue::from_str(&value),
                    ) else {
                        return;
                    };
                    map.append(name, value);
                }
                let request = DispatchRequest {
                    method,
                    path,
                    headers: map,
                    body: Default::default(),
                    caller: Caller::Paired {
                        transport: Transport::Relay,
                        pairing: session.grant.clone(),
                    },
                };
                if end == Some(false) {
                    session.requests.insert(
                        head.id,
                        Request::Reading(Box::new(Pending {
                            request,
                            body: message.body,
                        })),
                    );
                    return;
                }
                DispatchRequest {
                    body: message.body.into(),
                    ..request
                }
            }
            Input::Data { end } => {
                if !matches!(session.requests.get(&head.id), Some(Request::Reading(_))) {
                    return;
                }
                let Some(Request::Reading(mut request)) = session.requests.remove(&head.id) else {
                    return;
                };
                if request.body.len() + message.body.len() > MAX_BODY {
                    return;
                }
                request.body.extend_from_slice(&message.body);
                if end != Some(true) {
                    session.requests.insert(head.id, Request::Reading(request));
                    return;
                }
                DispatchRequest {
                    body: request.body.into(),
                    ..request.request
                }
            }
            Input::Cancel => unreachable!(),
        };
        let cancel = session.cancel.child_token();
        let dispatcher = dispatcher.clone();
        let relay = relay.clone();
        let stop = cancel.clone();
        self.jobs.spawn(async move {
            let _guard = stop.clone().drop_guard();
            tokio::select! {
                _ = run(id, head.id, request, dispatcher, relay, stop.clone()) => {}
                _ = stop.cancelled() => {}
            }
        });
        session.requests.insert(head.id, Request::Running(cancel));
    }

    pub(crate) async fn reap(&mut self) {
        if self.jobs.is_empty() {
            std::future::pending::<()>().await;
        }
        self.job_result().await;
    }
}

async fn run(
    id: Id,
    request_id: u64,
    request: DispatchRequest,
    dispatcher: Dispatcher,
    relay: relay::Sender,
    cancel: CancellationToken,
) {
    let mut reply = dispatcher.dispatch(request, cancel).await;
    let headers: HashMap<_, _> = reply
        .headers
        .iter()
        .filter_map(|(name, value)| Some((name.as_str(), value.to_str().ok()?)))
        .collect();
    let head = json!({"t":"res","id":request_id,"s":reply.status.as_u16(),"h":headers});
    if send(&relay, id, head, vec![]).await.is_err() {
        return;
    }
    while let Some(frame) = reply.body.frame().await {
        let Ok(frame) = frame else {
            let _ = send(&relay, id, json!({"t":"cancel","id":request_id}), vec![]).await;
            return;
        };
        let Ok(data) = frame.into_data() else {
            continue;
        };
        for chunk in data.chunks(CHUNK) {
            if send(
                &relay,
                id,
                json!({"t":"data","id":request_id}),
                chunk.to_vec(),
            )
            .await
            .is_err()
            {
                return;
            }
        }
    }
    let _ = send(
        &relay,
        id,
        json!({"t":"data","id":request_id,"end":true}),
        vec![],
    )
    .await;
}

async fn send(
    relay: &relay::Sender,
    id: Id,
    head: Value,
    body: Vec<u8>,
) -> Result<(), relay::Error> {
    relay.send(id, Message { head, body }).await
}
