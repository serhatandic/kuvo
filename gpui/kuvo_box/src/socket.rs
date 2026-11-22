//! A WebSocket on a box's own server on this computer (PROTOCOL.md, "Sockets"): its handshake on a
//! connection to 127.0.0.1, then one thread that owns the socket, writing what's handed to it and
//! reading what comes, waiting on the socket and a wake pipe together (as the relay connection's
//! does), so nothing polls on a timer and nothing sent waits on a read. Unix only, where the app
//! runs; elsewhere it says so.

use std::io;
use std::net::TcpStream;

use crate::transport::Socket;

/// How long the handshake may take: the box answers once its engine's socket is open.
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// What came of asking a box's server for a socket: open, or its status and words.
pub(crate) enum Opened {
    Open(Socket),
    Declined { status: u16, body: Vec<u8> },
}

/// Opens the socket at `path` (a route, without its leading slash) on the server `stream` is
/// connected to at `port`, sending `headers`.
#[cfg(unix)]
pub(crate) fn open(stream: TcpStream, port: u16, path: &str, headers: &[(String, String)]) -> io::Result<Opened> {
    use std::os::fd::AsRawFd;
    use tungstenite::client::IntoClientRequest;
    use tungstenite::http::{HeaderName, HeaderValue};

    let invalid = |why: String| io::Error::new(io::ErrorKind::InvalidInput, why);
    let mut request = format!("ws://127.0.0.1:{port}/{}", path.trim_start_matches('/')).into_client_request().map_err(|error| invalid(error.to_string()))?;
    for (name, value) in headers {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|error| invalid(error.to_string()))?;
        let value = HeaderValue::from_str(value).map_err(|error| invalid(error.to_string()))?;
        request.headers_mut().insert(name, value);
    }
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;
    let fd = stream.as_raw_fd();
    let socket = match tungstenite::client(request, stream) {
        Ok((socket, _)) => socket,
        Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(response))) => {
            let status = response.status().as_u16();
            return Ok(Opened::Declined { status, body: response.into_body().unwrap_or_default() });
        }
        Err(error) => return Err(io::Error::new(io::ErrorKind::ConnectionAborted, error.to_string())),
    };
    socket.get_ref().set_nonblocking(true)?;
    Ok(Opened::Open(unix::serve(socket, fd)?))
}

#[cfg(not(unix))]
pub(crate) fn open(_stream: TcpStream, _port: u16, _path: &str, _headers: &[(String, String)]) -> io::Result<Opened> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "terminals aren't built for this system yet"))
}

#[cfg(unix)]
mod unix {
    use std::collections::VecDeque;
    use std::io;
    use std::net::TcpStream;
    use std::os::fd::RawFd;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
    use std::sync::{Arc, Mutex};

    use tungstenite::protocol::WebSocket;
    use tungstenite::Message;

    use crate::error::{Error, Result};
    use crate::link::relay::Wake;
    use crate::transport::{Socket, SocketMessage, SocketOut};

    enum Out {
        Text(String),
        Close,
    }

    /// What sends on the socket: hands what's sent to its thread and wakes it.
    struct Outgoing {
        sender: Mutex<Sender<Out>>,
        wake: Arc<Wake>,
        ended: Arc<AtomicBool>,
    }

    impl SocketOut for Outgoing {
        fn send_text(&self, text: &str) -> Result<()> {
            let gone = || Error::Unreachable("The connection closed.".into());
            if self.ended.load(Ordering::SeqCst) {
                return Err(gone());
            }
            self.sender.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).send(Out::Text(text.to_owned())).map_err(|_| gone())?;
            self.wake.wake();
            Ok(())
        }

        fn close(&self) {
            let _ = self.sender.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).send(Out::Close);
            self.wake.wake();
        }
    }

    impl Drop for Outgoing {
        fn drop(&mut self) {
            // Its thread sees the sender gone, and closes.
            self.wake.wake();
        }
    }

    fn would_block(error: &tungstenite::Error) -> bool {
        matches!(error, tungstenite::Error::Io(error) if error.kind() == io::ErrorKind::WouldBlock)
    }

    /// Runs `socket` (past its handshake, not blocking, its descriptor `fd`) on a thread of its own.
    pub(super) fn serve(mut socket: WebSocket<TcpStream>, fd: RawFd) -> io::Result<Socket> {
        let wake = Arc::new(Wake::new()?);
        let (to_send, sending): (Sender<Out>, Receiver<Out>) = channel();
        let (received, incoming) = async_channel::unbounded();
        let ended = Arc::new(AtomicBool::new(false));
        let (woken, over) = (wake.clone(), ended.clone());
        std::thread::Builder::new().name("kuvo-socket".into()).spawn(move || {
            let mut pending: VecDeque<Message> = VecDeque::new();
            let mut unflushed = false;
            let why: Option<String> = 'run: loop {
                woken.drain();
                loop {
                    match sending.try_recv() {
                        Ok(Out::Text(text)) => pending.push_back(Message::text(text)),
                        Ok(Out::Close) | Err(TryRecvError::Disconnected) => {
                            let _ = socket.close(None);
                            let _ = socket.flush();
                            break 'run None;
                        }
                        Err(TryRecvError::Empty) => break,
                    }
                }
                while let Some(message) = pending.pop_front() {
                    match socket.write(message) {
                        Ok(()) => unflushed = true,
                        Err(error) if would_block(&error) => {
                            unflushed = true;
                            break;
                        }
                        Err(error) => break 'run Some(format!("It couldn’t be written to ({error}).")),
                    }
                }
                if unflushed {
                    match socket.flush() {
                        Ok(()) => unflushed = false,
                        Err(error) if would_block(&error) => {}
                        Err(error) => break 'run Some(format!("It couldn’t send ({error}).")),
                    }
                }
                loop {
                    let message = match socket.read() {
                        Ok(Message::Text(text)) => SocketMessage::Text(text.as_bytes().to_vec()),
                        Ok(Message::Binary(bytes)) => SocketMessage::Binary(bytes.to_vec()),
                        // (Its pong is queued by the socket itself, and goes with the next flush.)
                        Ok(Message::Ping(_)) => {
                            unflushed = true;
                            continue;
                        }
                        Ok(Message::Close(frame)) => break 'run frame.map(|frame| frame.reason.to_string()).filter(|reason| !reason.is_empty()),
                        Ok(_) => continue,
                        Err(error) if would_block(&error) => break,
                        Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => break 'run None,
                        Err(error) => break 'run Some(format!("It ended ({error}).")),
                    };
                    if received.send_blocking(message).is_err() {
                        // No one reads it any more.
                        let _ = socket.close(None);
                        let _ = socket.flush();
                        break 'run None;
                    }
                }
                let mut waits = [
                    libc::pollfd { fd, events: libc::POLLIN | if unflushed { libc::POLLOUT } else { 0 }, revents: 0 },
                    libc::pollfd { fd: woken.read, events: libc::POLLIN, revents: 0 },
                ];
                // SAFETY: two entries, both descriptors open for as long as this thread runs.
                unsafe { libc::poll(waits.as_mut_ptr(), 2, -1) };
            };
            over.store(true, Ordering::SeqCst);
            let _ = received.send_blocking(SocketMessage::Ended(why));
        })?;
        Ok(Socket { incoming, outgoing: Arc::new(Outgoing { sender: Mutex::new(to_send), wake, ended }) })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::net::TcpListener;

    use tungstenite::Message;
    use tungstenite::handshake::server::{Request, Response};

    use super::*;
    use crate::transport::SocketMessage;

    /// Each request's head, as the server heard it.
    type Heard = std::sync::mpsc::Receiver<Vec<(String, String)>>;

    /// A server that takes one socket, hands its request's head out, and runs `then` on it.
    fn serve(then: impl FnOnce(&mut tungstenite::WebSocket<TcpStream>) + Send + 'static, refuse: Option<u16>) -> (u16, Heard, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (heard, heads) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let accepted = tungstenite::accept_hdr(stream, |request: &Request, response: Response| {
                let mut head: Vec<(String, String)> = request.headers().iter().map(|(name, value)| (name.as_str().to_owned(), value.to_str().unwrap_or("").to_owned())).collect();
                head.push(("path".into(), request.uri().to_string()));
                heard.send(head).unwrap();
                match refuse {
                    Some(status) => Err(tungstenite::http::Response::builder().status(status).body(Some("Refused".into())).unwrap()),
                    None => Ok(response),
                }
            });
            if let Ok(mut socket) = accepted {
                then(&mut socket);
            }
        });
        (port, heads, server)
    }

    fn connect(port: u16) -> TcpStream {
        TcpStream::connect(("127.0.0.1", port)).unwrap()
    }

    #[test]
    fn a_socket_on_this_computers_box_carries_text_both_ways_and_says_when_it_ends() {
        let (port, heads, server) = serve(|socket| {
            socket.send(Message::text("\u{1b}[32mready\u{1b}[0m $ ")).unwrap();
            socket.send(Message::binary(b"\0{\"cursor\":12}".to_vec())).unwrap();
            let typed = loop {
                match socket.read().unwrap() {
                    Message::Text(text) => break text.to_string(),
                    _ => continue,
                }
            };
            socket.send(Message::text(format!("you typed {typed}"))).unwrap();
            socket.close(None).unwrap();
            while socket.read().is_ok() {}
        }, None);
        let headers = vec![("authorization".to_owned(), "Basic b3BlbmNvZGU6cHc=".to_owned()), ("x-opencode-directory".to_owned(), "%2FUsers%2Fsam%2Fapp".to_owned())];
        let Opened::Open(socket) = open(connect(port), port, "api/pty/pty_1/connect", &headers).unwrap() else { panic!("it opened") };
        let head = heads.recv().unwrap();
        let said = |name: &str| head.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone());
        assert_eq!(said("path").as_deref(), Some("/api/pty/pty_1/connect"));
        assert_eq!(said("authorization").as_deref(), Some("Basic b3BlbmNvZGU6cHc="));
        assert_eq!(said("x-opencode-directory").as_deref(), Some("%2FUsers%2Fsam%2Fapp"));

        assert_eq!(socket.incoming.recv_blocking().unwrap(), SocketMessage::Text(b"\x1b[32mready\x1b[0m $ ".to_vec()));
        assert_eq!(socket.incoming.recv_blocking().unwrap(), SocketMessage::Binary(b"\0{\"cursor\":12}".to_vec()));
        socket.outgoing.send_text("ls\r").unwrap();
        assert_eq!(socket.incoming.recv_blocking().unwrap(), SocketMessage::Text(b"you typed ls\r".to_vec()));
        assert_eq!(socket.incoming.recv_blocking().unwrap(), SocketMessage::Ended(None));
        assert!(socket.incoming.recv_blocking().is_err(), "nothing after its end");
        assert!(socket.outgoing.send_text("more").is_err());
        server.join().unwrap();
    }

    #[test]
    fn closing_it_from_here_tells_the_other_side() {
        let (port, _heads, server) = serve(|socket| {
            let closed = loop {
                match socket.read() {
                    Ok(Message::Close(_)) | Err(_) => break true,
                    Ok(_) => continue,
                }
            };
            assert!(closed);
        }, None);
        let Opened::Open(socket) = open(connect(port), port, "api/pty/pty_1/connect", &[]).unwrap() else { panic!("it opened") };
        socket.outgoing.close();
        server.join().unwrap();
        assert_eq!(socket.incoming.recv_blocking().unwrap(), SocketMessage::Ended(None));
    }

    #[test]
    fn a_server_that_wont_open_one_says_its_status() {
        let (port, _heads, server) = serve(|_| {}, Some(403));
        match open(connect(port), port, "api/pty/pty_1/connect", &[]).unwrap() {
            Opened::Declined { status, .. } => assert_eq!(status, 403),
            Opened::Open(_) => panic!("it was refused"),
        }
        server.join().unwrap();
    }
}
