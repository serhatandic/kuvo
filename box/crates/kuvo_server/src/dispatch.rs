use std::{future::Future, io, marker::PhantomData, pin::Pin, sync::Arc};

use futures_core::Stream;
use futures_util::StreamExt;
use http::{HeaderMap, HeaderValue, Method, StatusCode, header::CONTENT_TYPE};
use http_body_util::{BodyExt, Full, StreamBody, combinators::UnsyncBoxBody};
use hyper::body::{Bytes, Frame};
use kuvo_protocol::{
    ByteStatus, JsonStatus, Route, RouteSpec,
    version::{ROUTE_VERSION, ROUTE_VERSION_HEADER, UnsupportedFormatVersion},
};
use serde::Serialize;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::{
    line::{LineHub, LineTiming},
    path,
    socket::Socket,
};

pub type Body = UnsyncBoxBody<Bytes, io::Error>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalOrigin {
    Direct,
    App,
    Mac,
    VaultHolder,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Http { origin: LocalOrigin, internal: bool },
    Ssh { internal: bool },
    AddressedSsh,
    Relay,
}

#[derive(Clone)]
pub struct Pairing {
    pub key: [u8; 32],
    pub id: String,
    pub name: String,
    pub desktop: bool,
    current: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl Pairing {
    /// The transport calls this only after authenticating the device. The check reads
    /// a current, in-memory pairing view and can revoke an established connection.
    pub fn verified(
        key: [u8; 32],
        id: String,
        name: String,
        desktop: bool,
        current: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            key,
            id,
            name,
            desktop,
            current: Arc::new(current),
        }
    }

    pub fn is_current(&self) -> bool {
        (self.current)()
    }
}

#[derive(Clone)]
pub enum Caller {
    Local(Transport),
    Paired {
        transport: Transport,
        pairing: Pairing,
    },
}

impl Caller {
    pub fn transport(&self) -> Transport {
        match self {
            Self::Local(transport) | Self::Paired { transport, .. } => *transport,
        }
    }

    pub fn pairing(&self) -> Option<&Pairing> {
        match self {
            Self::Paired { pairing, .. } => Some(pairing),
            Self::Local(_) => None,
        }
    }

    pub(crate) fn same_owner(&self, other: &Self) -> bool {
        match (self.pairing(), other.pairing()) {
            (Some(a), Some(b)) => a.key == b.key && a.id == b.id,
            (None, None) => self.transport() == other.transport(),
            _ => false,
        }
    }

    fn desktop(&self) -> bool {
        match self {
            Self::Paired { pairing, .. } => pairing.desktop,
            Self::Local(
                Transport::Ssh { .. }
                | Transport::Http {
                    origin: LocalOrigin::App | LocalOrigin::Mac,
                    ..
                },
            ) => true,
            Self::Local(_) => false,
        }
    }

    fn local(&self) -> bool {
        matches!(
            self,
            Self::Local(Transport::Http { .. } | Transport::Ssh { .. })
        )
    }
}

/// Additional authority required by a route. Setup and record authority can be
/// decided from area state using the authenticated caller carried in RouteCall.
#[derive(Clone, Copy)]
pub enum Access {
    Any,
    Local,
    LocalApp,
    Desktop,
    DesktopOrPaired,
    Internal,
}

impl Access {
    fn admits(self, caller: &Caller) -> bool {
        match self {
            Self::Any => true,
            Self::Local => caller.local(),
            Self::LocalApp => matches!(
                caller,
                Caller::Local(Transport::Http {
                    origin: LocalOrigin::App | LocalOrigin::Mac,
                    ..
                })
            ),
            Self::Desktop => caller.desktop(),
            Self::DesktopOrPaired => caller.desktop() || caller.pairing().is_some(),
            Self::Internal => matches!(
                caller,
                Caller::Local(
                    Transport::Http { internal: true, .. } | Transport::Ssh { internal: true }
                )
            ),
        }
    }
}

#[derive(Clone)]
pub struct DispatchRequest {
    pub method: Method,
    /// Without a leading slash, with the route's query intact.
    pub path: String,
    pub headers: HeaderMap,
    pub body: Bytes,
    pub caller: Caller,
}

impl DispatchRequest {
    pub(crate) fn topic(&self, path: String) -> Self {
        Self {
            method: Method::GET,
            path,
            body: Bytes::new(),
            ..self.clone()
        }
    }
}

pub struct RouteCall<T> {
    pub request: DispatchRequest,
    pub body: T,
    pub cancel: CancellationToken,
    params: Vec<(&'static str, String)>,
}

impl<T> RouteCall<T> {
    /// An encoded path segment matched by the generated route pattern.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.as_str())
    }
}

pub struct Reply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Body,
}

impl Reply {
    pub fn bytes(status: StatusCode, content_type: &'static str, bytes: impl Into<Bytes>) -> Self {
        let mut reply = Self {
            status,
            headers: HeaderMap::new(),
            body: Full::new(bytes.into())
                .map_err(|never| match never {})
                .boxed_unsync(),
        };
        reply
            .headers
            .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
        reply
    }

    pub fn stream(
        status: StatusCode,
        content_type: &'static str,
        body: impl Stream<Item = io::Result<Bytes>> + Send + 'static,
    ) -> Self {
        let mut reply = Self {
            status,
            headers: HeaderMap::new(),
            body: StreamBody::new(body.map(|part| part.map(Frame::data))).boxed_unsync(),
        };
        reply
            .headers
            .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
        reply
    }

    pub fn json(status: StatusCode, value: impl Serialize) -> Self {
        match serde_json::to_vec(&value) {
            Ok(bytes) => Self::bytes(status, "application/json", bytes),
            Err(error) => {
                Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).reply()
            }
        }
    }

    fn empty_json(status: StatusCode) -> Self {
        Self::bytes(status, "application/json", Bytes::from_static(b"{}"))
    }
}

pub struct Refusal(Reply);

impl Refusal {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self::json(status, json!({ "message": message.into() }))
    }

    pub fn json(status: StatusCode, fields: impl Serialize) -> Self {
        Self(Reply::json(status, fields))
    }

    pub fn unsupported_record(error: UnsupportedFormatVersion) -> Self {
        Self::json(StatusCode::CONFLICT, error)
    }

    pub fn reply(self) -> Reply {
        self.0
    }
}

pub struct RouteReply<R>(Reply, PhantomData<fn() -> R>);

/// Encoded line of a route whose declared response shape describes each item.
pub struct JsonLine<R, const STATUS: u16>(Bytes, PhantomData<fn() -> R>);

impl<R: JsonStatus<STATUS>, const STATUS: u16> JsonLine<R, STATUS> {
    pub fn new(value: <R as JsonStatus<STATUS>>::Body) -> io::Result<Self> {
        let mut bytes = serde_json::to_vec(&value)?;
        bytes.push(b'\n');
        Ok(Self(bytes.into(), PhantomData))
    }

    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }
}

impl<R, const STATUS: u16> Clone for JsonLine<R, STATUS> {
    fn clone(&self) -> Self {
        Self(self.0.clone(), PhantomData)
    }
}

impl<R: RouteSpec> RouteReply<R> {
    pub fn json<const STATUS: u16>(body: <R as JsonStatus<STATUS>>::Body) -> Self
    where
        R: JsonStatus<STATUS>,
    {
        Self(Reply::json(status(STATUS), body), PhantomData)
    }

    pub fn bytes<const STATUS: u16>(content_type: &'static str, body: impl Into<Bytes>) -> Self
    where
        R: ByteStatus<STATUS>,
    {
        Self(
            Reply::bytes(status(STATUS), content_type, body),
            PhantomData,
        )
    }

    pub fn stream<const STATUS: u16>(
        content_type: &'static str,
        body: impl Stream<Item = io::Result<Bytes>> + Send + 'static,
    ) -> Self
    where
        R: ByteStatus<STATUS>,
    {
        Self(
            Reply::stream(status(STATUS), content_type, body),
            PhantomData,
        )
    }

    pub fn ndjson<const STATUS: u16>(
        body: impl Stream<Item = io::Result<JsonLine<R, STATUS>>> + Send + 'static,
    ) -> Self
    where
        R: JsonStatus<STATUS>,
    {
        Self(
            Reply::stream(
                status(STATUS),
                "application/x-ndjson",
                body.map(|part| part.map(|line| line.0)),
            ),
            PhantomData,
        )
    }

    /// Add transport metadata to an otherwise typed object answer.
    pub fn json_with<const STATUS: u16>(
        body: <R as JsonStatus<STATUS>>::Body,
        fields: impl IntoIterator<Item = (&'static str, serde_json::Value)>,
    ) -> Result<Self, serde_json::Error>
    where
        R: JsonStatus<STATUS>,
    {
        let mut body = serde_json::to_value(body)?;
        if let Some(object) = body.as_object_mut() {
            for (name, value) in fields {
                object.entry(name).or_insert(value);
            }
        }
        Ok(Self(Reply::json(status(STATUS), body), PhantomData))
    }

    pub fn headers_mut(&mut self) -> &mut HeaderMap {
        &mut self.0.headers
    }
}

fn status(value: u16) -> StatusCode {
    StatusCode::from_u16(value).expect("generated HTTP status")
}

type FutureReply = Pin<Box<dyn Future<Output = Reply> + Send>>;
type Handler = Box<
    dyn Fn(DispatchRequest, CancellationToken, Vec<(&'static str, String)>) -> FutureReply
        + Send
        + Sync,
>;
type Fallback = Box<dyn Fn(DispatchRequest, CancellationToken) -> FutureReply + Send + Sync>;
type ActivityReply = Pin<Box<dyn Future<Output = ()> + Send>>;
type Activity = Box<dyn Fn(&Caller) -> ActivityReply + Send + Sync>;

type SocketResult = Result<Socket, Refusal>;
type SocketHandler = Box<
    dyn Fn(DispatchRequest, CancellationToken) -> Pin<Box<dyn Future<Output = SocketResult> + Send>>
        + Send
        + Sync,
>;

struct Entry {
    route: &'static Route,
    access: Access,
    handler: Handler,
}

#[derive(Default)]
pub struct Registry {
    entries: Vec<Entry>,
    engine: Option<Fallback>,
    socket: Option<SocketHandler>,
    activity: Option<Activity>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Observe the authenticated callers of admitted requests.
    pub fn observe_activity<F, Fut>(&mut self, activity: F)
    where
        F: Fn(&Caller) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.activity = Some(Box::new(move |caller| Box::pin(activity(caller))));
    }

    pub fn register<R, H, F>(&mut self, access: Access, handler: H)
    where
        R: RouteSpec,
        H: Fn(RouteCall<R::Body>) -> F + Send + Sync + 'static,
        F: Future<Output = Result<RouteReply<R>, Refusal>> + Send + 'static,
    {
        self.register_decoded::<R, _, _, _>(
            access,
            |bytes| {
                serde_json::from_slice(bytes).map_err(|_| {
                    Refusal::new(StatusCode::BAD_REQUEST, "Send a valid request body.")
                })
            },
            handler,
        );
    }

    /// Use a route's compatibility decoder when it accepts a legacy body
    /// alongside its generated current form.
    pub fn register_decoded<R, D, H, F>(&mut self, access: Access, decode: D, handler: H)
    where
        R: RouteSpec,
        D: Fn(&[u8]) -> Result<R::Body, Refusal> + Send + Sync + 'static,
        H: Fn(RouteCall<R::Body>) -> F + Send + Sync + 'static,
        F: Future<Output = Result<RouteReply<R>, Refusal>> + Send + 'static,
    {
        assert!(
            !self
                .entries
                .iter()
                .any(|entry| std::ptr::eq(entry.route, R::ROUTE)),
            "route already registered: {} {}",
            R::ROUTE.method,
            R::ROUTE.path
        );
        self.entries.push(Entry {
            route: R::ROUTE,
            access,
            handler: Box::new(move |request, cancel, params| {
                let bytes = if R::ROUTE.body.is_none() {
                    b"null"
                } else {
                    request.body.as_ref()
                };
                match decode(bytes) {
                    Ok(body) => {
                        let answer = handler(RouteCall {
                            request,
                            body,
                            cancel,
                            params,
                        });
                        Box::pin(async move {
                            answer.await.map_or_else(Refusal::reply, |typed| typed.0)
                        })
                    }
                    Err(error) => Box::pin(std::future::ready(error.reply())),
                }
            }),
        });
    }

    /// Engine passthrough has no fixed response shape.
    pub fn engine<F, Fut>(&mut self, handle: F)
    where
        F: Fn(DispatchRequest, CancellationToken) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Reply> + Send + 'static,
    {
        self.engine = Some(Box::new(move |request, cancel| {
            Box::pin(handle(request, cancel))
        }));
    }

    pub fn sockets<F, Fut>(&mut self, handle: F)
    where
        F: Fn(DispatchRequest, CancellationToken) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = SocketResult> + Send + 'static,
    {
        assert!(
            self.socket.is_none(),
            "socket dispatcher already registered"
        );
        self.socket = Some(Box::new(move |request, cancel| {
            Box::pin(handle(request, cancel))
        }));
    }

    /// Start with a line hub owned by this dispatcher. No process-global routes or lines.
    pub fn build(mut self, timing: LineTiming) -> Dispatcher {
        // The line looks through a weak dispatcher so its topic tasks cannot keep the server alive.
        let (hub, pending) = LineHub::channel(timing);
        hub.register(&mut self);
        let dispatcher = Dispatcher(Arc::new(self));
        pending.run(Arc::downgrade(&dispatcher.0));
        dispatcher
    }
}

#[derive(Clone)]
pub struct Dispatcher(Arc<Registry>);

impl Dispatcher {
    pub(crate) fn from_registry(inner: Arc<Registry>) -> Self {
        Self(inner)
    }

    /// All transport entries enter here after transport authentication.
    pub async fn dispatch(&self, request: DispatchRequest, cancel: CancellationToken) -> Reply {
        let reply = self.route(request, cancel).await;
        let mut reply = reply;
        reply.headers.insert(
            ROUTE_VERSION_HEADER,
            HeaderValue::from_static(ROUTE_VERSION),
        );
        reply
    }

    /// All socket transports enter here after transport authentication.
    pub async fn open_socket(
        &self,
        mut request: DispatchRequest,
        cancel: CancellationToken,
    ) -> SocketResult {
        admissible(&request)?;
        if request.method != Method::GET || !request.body.is_empty() {
            return Err(unavailable_path());
        }
        if let Some(activity) = self.0.activity.as_ref() {
            activity(&request.caller).await;
        }
        request.path = path::canonical(&request.path).ok_or_else(unavailable_path)?;
        let handler = self.0.socket.as_ref().ok_or_else(unavailable_path)?;
        handler(request, cancel).await
    }

    async fn route(&self, mut request: DispatchRequest, cancel: CancellationToken) -> Reply {
        if let Err(refusal) = admissible(&request) {
            return refusal.reply();
        }
        if let Some(activity) = self.0.activity.as_ref() {
            activity(&request.caller).await;
        }
        let Some(canonical) = path::canonical(&request.path) else {
            return unavailable_path().reply();
        };
        if request
            .method
            .as_str()
            .bytes()
            .any(|c| c.is_ascii_lowercase())
        {
            request.method =
                Method::from_bytes(request.method.as_str().to_ascii_uppercase().as_bytes())
                    .expect("uppercasing preserves a valid HTTP method");
        }
        let path = request.path.split('?').next().unwrap_or_default();
        let mut best = None;
        let mut known = false;
        for entry in &self.0.entries {
            let Some(params) = parameters(entry.route.path, path) else {
                continue;
            };
            known = true;
            if request.method == entry.route.method
                && best
                    .as_ref()
                    .is_none_or(|(_, current): &(&Entry, Vec<_>)| params.len() < current.len())
            {
                best = Some((entry, params));
            }
        }
        if let Some((entry, params)) = best {
            if entry.access.admits(&request.caller) {
                return (entry.handler)(request, cancel, params).await;
            }
            return Refusal::new(
                StatusCode::FORBIDDEN,
                "Open this from a paired Kuvo Code client.",
            )
            .reply();
        }
        if known {
            return Reply::empty_json(StatusCode::METHOD_NOT_ALLOWED);
        }
        if canonical == request.path && path.starts_with("kuvo/") {
            return Reply::empty_json(StatusCode::NOT_FOUND);
        }
        let canonical_route = canonical.split('?').next().unwrap_or_default();
        if canonical_route == "api" || canonical_route.starts_with("api/") {
            request.path = canonical;
            if let Some(engine) = &self.0.engine {
                return engine(request, cancel).await;
            }
            return Refusal::new(StatusCode::SERVICE_UNAVAILABLE, "The engine isn't running.")
                .reply();
        }
        unavailable_path().reply()
    }
}

fn admissible(request: &DispatchRequest) -> Result<(), Refusal> {
    let versions = request.headers.get_all(ROUTE_VERSION_HEADER);
    let mut versions = versions.iter();
    if let Some(value) = versions.next() {
        let value = value
            .to_str()
            .ok()
            .filter(|_| versions.next().is_none())
            .unwrap_or("");
        if let Some(error) = UnsupportedFormatVersion::for_route(Some(value)) {
            return Err(Refusal::json(StatusCode::BAD_REQUEST, error));
        }
    }
    match &request.caller {
        Caller::Local(_) if !request.caller.local() => {
            return Err(Refusal::new(
                StatusCode::FORBIDDEN,
                "Pair this device before connecting.",
            ));
        }
        Caller::Paired { pairing, .. } if !pairing.is_current() => {
            return Err(Refusal::new(
                StatusCode::FORBIDDEN,
                "This device’s pairing changed. Connect again.",
            ));
        }
        _ => {}
    }

    Ok(())
}

fn unavailable_path() -> Refusal {
    Refusal(Reply::bytes(
        StatusCode::FORBIDDEN,
        "text/plain",
        "That isn't available over the link.",
    ))
}

fn parameters(pattern: &'static str, path: &str) -> Option<Vec<(&'static str, String)>> {
    let mut params = Vec::new();
    let mut path = path.split('/');
    for part in pattern.split('/') {
        let value = path.next()?;
        if let Some(name) = part.strip_prefix(':') {
            if value.is_empty() {
                return None;
            }
            params.push((name, value.to_owned()));
        } else if part != value {
            return None;
        }
    }
    path.next().is_none().then_some(params)
}
