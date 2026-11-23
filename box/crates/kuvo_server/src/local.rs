use std::{
    convert::Infallible,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, ready},
    time::Duration,
};

use http::{Request, Response, StatusCode, Version, header};
use http_body_util::{BodyExt, Full};
use hyper::{
    body::{Bytes, Frame, Incoming, SizeHint},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::{TokioIo, TokioTimer};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
    time::{self, Instant},
};
use tokio_util::sync::{CancellationToken, DropGuard};

use crate::{Body, Caller, DispatchRequest, Dispatcher, LocalOrigin, Reply, Transport, path};

pub(crate) async fn serve(
    socket: TcpStream,
    authorization: Arc<str>,
    dispatcher: Dispatcher,
    leaving: CancellationToken,
) {
    let _ = socket.set_nodelay(true);
    let socket = Arc::new(socket);
    let connection = http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(Duration::from_secs(65))
        .serve_connection(
            TokioIo::new(Socket(socket.clone())),
            service_fn(move |request| {
                let authorization = authorization.clone();
                let dispatcher = dispatcher.clone();
                let socket = socket.clone();
                async move {
                    Ok::<_, Infallible>(answer(request, &authorization, dispatcher, socket).await)
                }
            }),
        );
    tokio::pin!(connection);
    tokio::select! {
        _ = &mut connection => {},
        () = leaving.cancelled() => {
            connection.as_mut().graceful_shutdown();
            let _ = connection.await;
        }
    }
}

struct ResponseBody {
    inner: Body,
    _cancel: Option<DropGuard>,
}

impl hyper::body::Body for ResponseBody {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<io::Result<Frame<Bytes>>>> {
        Pin::new(&mut self.inner).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

fn empty(status: StatusCode) -> Response<ResponseBody> {
    let body = Full::default()
        .map_err(|never| match never {})
        .boxed_unsync();
    let mut response = Response::new(ResponseBody {
        inner: body,
        _cancel: None,
    });
    *response.status_mut() = status;
    response
}

async fn answer(
    request: Request<Incoming>,
    authorization: &str,
    dispatcher: Dispatcher,
    socket: Arc<TcpStream>,
) -> Response<ResponseBody> {
    let (head, body) = request.into_parts();
    let mut given = head.headers.get_all(header::AUTHORIZATION).iter();
    if !given
        .next()
        .is_some_and(|given| bool::from(given.as_bytes().ct_eq(authorization.as_bytes())))
        || given.next().is_some()
    {
        return empty(StatusCode::UNAUTHORIZED);
    }
    let Some(path) = head
        .uri
        .path_and_query()
        .and_then(|path| path::canonical(path.as_str()))
    else {
        return empty(StatusCode::FORBIDDEN);
    };
    let Ok(body) = body.collect().await else {
        return empty(StatusCode::BAD_REQUEST);
    };
    let origin = match head
        .headers
        .get("x-kuvo-from")
        .and_then(|value| value.to_str().ok())
    {
        Some("app") => LocalOrigin::App,
        Some("mac") => LocalOrigin::Mac,
        _ => LocalOrigin::Direct,
    };
    let internal = head
        .headers
        .get("x-kuvo-internal")
        .is_some_and(|value| value == "1");
    let cancel = CancellationToken::new();
    let guard = cancel.clone().drop_guard();
    let reply = dispatcher.dispatch(
        DispatchRequest {
            method: head.method,
            path,
            headers: head.headers,
            body: body.to_bytes(),
            caller: Caller::Local(Transport::Http { origin, internal }),
        },
        cancel,
    );
    let Reply {
        status,
        mut headers,
        body,
    } = if head.version == Version::HTTP_11 {
        tokio::pin!(reply);
        let beat = Duration::from_secs(20);
        let mut pulse = time::interval_at(Instant::now() + beat, beat);
        pulse.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                ready = &mut reply => break ready,
                _ = pulse.tick() => {
                    if processing(&socket).await.is_err() {
                        return empty(StatusCode::BAD_GATEWAY);
                    }
                }
            }
        }
    } else {
        reply.await
    };
    headers.insert(
        "keep-alive",
        "timeout=5".parse().expect("keep-alive header"),
    );
    let mut response = Response::new(ResponseBody {
        inner: body,
        _cancel: Some(guard),
    });
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    response
}

/// Hyper serves all final HTTP responses. Its server API has no arbitrary 1xx
/// response. While this request still owns the slot and has read its entire body,
/// write the complete informational frame before releasing a final response.
async fn processing(socket: &TcpStream) -> io::Result<()> {
    let mut message: &[u8] = b"HTTP/1.1 102 Processing\r\n\r\n";
    while !message.is_empty() {
        socket.writable().await?;
        match socket.try_write(message) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(sent) => message = &message[sent..],
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Share the socket with the service's informational writer without duplicating
/// its operating system descriptor. Hyper owns all normal reads and writes.
struct Socket(Arc<TcpStream>);

impl AsyncRead for Socket {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        target: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            ready!(self.0.poll_read_ready(cx))?;
            match self.0.try_read(target.initialize_unfilled()) {
                Ok(read) => {
                    target.advance(read);
                    return Poll::Ready(Ok(()));
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Poll::Ready(Err(error)),
            }
        }
    }
}

impl AsyncWrite for Socket {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            ready!(self.0.poll_write_ready(cx))?;
            match self.0.try_write(bytes) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                result => return Poll::Ready(result),
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    // No clone escapes the connection or its service future. Closing the last
    // owner closes both directions. Keep Hyper's final writes ahead of this poll.
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
