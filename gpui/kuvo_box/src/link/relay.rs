//! This device's connection to the relay (PROTOCOL.md, "The relay"): one WebSocket of binary
//! messages, pinged every 25 s and given up on when the pong is 10 s late, as the phone's is. Late
//! counts from when the ping has gone out, not from when it was written: a ping waits behind what
//! was being sent before it (a picture, on a slow connection), and the connection isn't dead
//! while that's still going.
//!
//! One thread owns the socket: it writes what's handed to it and reads what comes, waiting on the
//! socket and a wake pipe together, so nothing here polls on a timer and nothing sent waits on a
//! read. Unix only for now (macOS and Linux, where the app runs); elsewhere it says so.

use std::io;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

/// How a client reaches a relay: what sends on the connection, and what reads from it.
pub trait Dial: Send + Sync {
    fn dial(&self, url: &str) -> io::Result<(Box<dyn Outgoing>, Box<dyn Incoming>)>;
}

/// The sending half. Dropping it closes the connection.
pub trait Outgoing: Send {
    fn send(&mut self, message: Vec<u8>) -> io::Result<()>;
    /// Whether what it was handed is still going out (a big message on a slow connection): an
    /// answer can't be expected before it's gone.
    fn busy(&self) -> bool {
        false
    }
}

/// The reading half: the next message, waiting for it; an error once the connection has ended.
pub trait Incoming: Send {
    fn next(&mut self) -> io::Result<Vec<u8>>;
}

/// A connection's two halves.
pub type Halves = (Box<dyn Outgoing>, Box<dyn Incoming>);

/// How long connecting (TCP, TLS, the WebSocket's own handshake) may take.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// How often the relay is pinged, and how late its pong may be.
pub const PING_EVERY: Duration = Duration::from_secs(25);
pub const PONG_WITHIN: Duration = Duration::from_secs(10);
/// How long a ping may wait behind what's being sent before it, the connection making no way.
pub const STALL_WITHIN: Duration = Duration::from_secs(120);

/// When a connection pings, and when it's given up on: a pong late by `within` from when its ping
/// went out (flushed), or a ping that couldn't go out at all for `stall`.
#[derive(Debug, Clone)]
pub struct PingClock {
    every: Duration,
    within: Duration,
    stall: Duration,
    next: Instant,
    /// Written, not gone out yet: since when.
    queued: Option<Instant>,
    /// Gone out, its pong not back: since when.
    out: Option<Instant>,
}

impl PingClock {
    pub fn new(now: Instant, every: Duration, within: Duration, stall: Duration) -> PingClock {
        PingClock { every, within, stall, next: now + every, queued: None, out: None }
    }

    /// Whether to write a ping now (one at a time).
    pub fn ping_due(&mut self, now: Instant) -> bool {
        if now < self.next {
            return false;
        }
        self.next = now + self.every;
        if self.queued.is_some() || self.out.is_some() {
            return false;
        }
        self.queued = Some(now);
        true
    }

    /// Everything written has gone out: a ping among it is out now.
    pub fn flushed(&mut self, now: Instant) {
        if self.queued.take().is_some() {
            self.out = Some(now);
        }
    }

    pub fn pong(&mut self) {
        self.queued = None;
        self.out = None;
    }

    /// Whether the other side is taken to be gone.
    pub fn dead(&self, now: Instant) -> bool {
        self.out.is_some_and(|since| now.saturating_duration_since(since) > self.within) || self.queued.is_some_and(|since| now.saturating_duration_since(since) > self.stall)
    }

    /// When it next has something to do.
    pub fn next_at(&self) -> Instant {
        let mut at = self.next;
        if let Some(since) = self.out {
            at = at.min(since + self.within);
        }
        if let Some(since) = self.queued {
            at = at.min(since + self.stall);
        }
        at
    }
}

/// The real thing: a WebSocket to `ws://` or `wss://`.
pub struct WebSocketDial;

struct ChannelIncoming(Receiver<Vec<u8>>);

impl Incoming for ChannelIncoming {
    fn next(&mut self) -> io::Result<Vec<u8>> {
        self.0.recv().map_err(|_| io::Error::new(io::ErrorKind::ConnectionAborted, "the relay connection ended"))
    }
}

#[cfg(unix)]
pub use unix::serve as pump;

#[cfg(unix)]
mod unix {
    use std::io::{self, Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};
    use std::os::fd::{AsRawFd, RawFd};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
    use std::time::Instant;

    use tungstenite::protocol::WebSocket;
    use tungstenite::stream::MaybeTlsStream;
    use tungstenite::{Connector, Message};

    use super::{CONNECT_TIMEOUT, ChannelIncoming, Dial, Incoming, Outgoing, PING_EVERY, PONG_WITHIN, PingClock, STALL_WITHIN, WebSocketDial};

    /// A pipe that wakes the socket's thread: written by whoever hands it something to send.
    struct Wake {
        read: RawFd,
        write: RawFd,
    }

    impl Wake {
        fn new() -> io::Result<Wake> {
            let mut ends = [0 as RawFd; 2];
            // SAFETY: `ends` is two descriptors' room, which pipe fills.
            if unsafe { libc::pipe(ends.as_mut_ptr()) } != 0 {
                return Err(io::Error::last_os_error());
            }
            for end in ends {
                // SAFETY: `end` is a descriptor pipe just made.
                unsafe {
                    libc::fcntl(end, libc::F_SETFL, libc::fcntl(end, libc::F_GETFL) | libc::O_NONBLOCK);
                    libc::fcntl(end, libc::F_SETFD, libc::FD_CLOEXEC);
                }
            }
            Ok(Wake { read: ends[0], write: ends[1] })
        }

        fn wake(&self) {
            // SAFETY: one byte from a buffer of one, to a descriptor this owns. Full means woken already.
            unsafe { libc::write(self.write, [1u8].as_ptr().cast(), 1) };
        }

        fn drain(&self) {
            let mut buffer = [0u8; 64];
            // SAFETY: reads at most the buffer's length from a descriptor this owns.
            while unsafe { libc::read(self.read, buffer.as_mut_ptr().cast(), buffer.len()) } > 0 {}
        }
    }

    impl Drop for Wake {
        fn drop(&mut self) {
            // SAFETY: both are this pipe's own, closed once.
            unsafe {
                libc::close(self.read);
                libc::close(self.write);
            }
        }
    }

    struct PipeOutgoing {
        sender: Sender<Vec<u8>>,
        wake: Arc<Wake>,
        /// Set while what was handed over is still going out.
        busy: Arc<AtomicBool>,
    }

    impl Outgoing for PipeOutgoing {
        fn send(&mut self, message: Vec<u8>) -> io::Result<()> {
            self.busy.store(true, Ordering::SeqCst);
            self.sender.send(message).map_err(|_| io::Error::new(io::ErrorKind::ConnectionAborted, "the relay connection ended"))?;
            self.wake.wake();
            Ok(())
        }

        fn busy(&self) -> bool {
            self.busy.load(Ordering::SeqCst)
        }
    }

    impl Drop for PipeOutgoing {
        fn drop(&mut self) {
            // Its thread sees the sender gone, and closes.
            self.wake.wake();
        }
    }

    fn would_block(error: &tungstenite::Error) -> bool {
        matches!(error, tungstenite::Error::Io(error) if error.kind() == io::ErrorKind::WouldBlock)
    }

    /// Runs `socket` (already past its handshake, its descriptor `fd` set not to block) on a thread
    /// of its own: the two halves a client or a stand-in relay talks through. `pings`: whether this
    /// side checks the other is there (a client does; a relay answers).
    pub fn serve<S: Read + Write + Send + 'static>(mut socket: WebSocket<S>, fd: RawFd, pings: bool) -> io::Result<(Box<dyn Outgoing>, Box<dyn Incoming>)> {
        let wake = Arc::new(Wake::new()?);
        let (to_send, sending): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = channel();
        let (received, receiving) = channel();
        let woken = wake.clone();
        let busy = Arc::new(AtomicBool::new(false));
        let going = busy.clone();
        std::thread::Builder::new().name("kuvo-relay".into()).spawn(move || {
            let mut unflushed = false;
            let mut clock = PingClock::new(Instant::now(), PING_EVERY, PONG_WITHIN, STALL_WITHIN);
            'run: loop {
                woken.drain();
                loop {
                    match sending.try_recv() {
                        Ok(message) => match socket.write(Message::Binary(message.into())) {
                            Ok(()) => {
                                unflushed = true;
                                going.store(true, Ordering::SeqCst);
                            }
                            Err(error) if would_block(&error) => {
                                unflushed = true;
                                going.store(true, Ordering::SeqCst);
                            }
                            Err(_) => break 'run,
                        },
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => break 'run,
                    }
                }
                if pings && clock.ping_due(Instant::now()) {
                    match socket.write(Message::Ping(Vec::new().into())) {
                        Ok(()) => unflushed = true,
                        Err(error) if would_block(&error) => unflushed = true,
                        Err(_) => break 'run,
                    }
                }
                if unflushed {
                    match socket.flush() {
                        Ok(()) => {
                            unflushed = false;
                            clock.flushed(Instant::now());
                            // (Handed more meanwhile: it says so again as it's handed over.)
                            going.store(false, Ordering::SeqCst);
                        }
                        Err(error) if would_block(&error) => {}
                        Err(_) => break 'run,
                    }
                }
                if pings && clock.dead(Instant::now()) {
                    break 'run;
                }
                loop {
                    match socket.read() {
                        Ok(Message::Binary(bytes)) => {
                            if received.send(bytes.to_vec()).is_err() {
                                break 'run;
                            }
                        }
                        Ok(Message::Pong(_)) => clock.pong(),
                        // (A ping's pong is queued by the socket itself, and goes with the next flush.)
                        Ok(Message::Ping(_)) => unflushed = true,
                        Ok(Message::Close(_)) => break 'run,
                        Ok(_) => {}
                        Err(error) if would_block(&error) => break,
                        Err(_) => break 'run,
                    }
                }
                let until = clock.next_at();
                let wait = if pings { until.saturating_duration_since(Instant::now()).as_millis().min(60_000) as libc::c_int + 1 } else { -1 };
                let mut waits = [
                    libc::pollfd { fd, events: libc::POLLIN | if unflushed { libc::POLLOUT } else { 0 }, revents: 0 },
                    libc::pollfd { fd: woken.read, events: libc::POLLIN, revents: 0 },
                ];
                // SAFETY: two entries, both descriptors open for as long as this thread runs.
                unsafe { libc::poll(waits.as_mut_ptr(), 2, wait) };
            }
            let _ = socket.close(None);
            let _ = socket.flush();
        })?;
        Ok((Box::new(PipeOutgoing { sender: to_send, wake, busy }), Box::new(ChannelIncoming(receiving))))
    }

    fn tls() -> Arc<rustls::ClientConfig> {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .expect("ring has the default protocol versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
        Arc::new(config)
    }

    impl Dial for WebSocketDial {
        fn dial(&self, url: &str) -> io::Result<(Box<dyn Outgoing>, Box<dyn Incoming>)> {
            let bad = |why: &str| io::Error::new(io::ErrorKind::InvalidInput, why.to_owned());
            let (secure, rest) = match (url.strip_prefix("wss://"), url.strip_prefix("ws://")) {
                (Some(rest), _) => (true, rest),
                (_, Some(rest)) => (false, rest),
                _ => return Err(bad("not a relay's address")),
            };
            let authority = rest.split(['/', '?']).next().unwrap_or(rest);
            // Never the relay secret in the clear, but to this computer itself (a test's relay).
            if !secure && !(authority.starts_with("127.0.0.1:") || authority.starts_with("localhost:")) {
                return Err(bad("a relay is reached over wss://"));
            }
            let with_port = if authority.rsplit_once(':').is_some_and(|(_, port)| port.parse::<u16>().is_ok()) { authority.to_owned() } else { format!("{authority}:{}", if secure { 443 } else { 80 }) };
            let mut last = bad("no address for the relay");
            let mut stream = None;
            for address in with_port.to_socket_addrs()? {
                match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
                    Ok(connected) => {
                        stream = Some(connected);
                        break;
                    }
                    Err(error) => last = error,
                }
            }
            let stream = stream.ok_or(last)?;
            let _ = stream.set_nodelay(true);
            stream.set_read_timeout(Some(CONNECT_TIMEOUT))?;
            stream.set_write_timeout(Some(CONNECT_TIMEOUT))?;
            let fd = stream.as_raw_fd();
            let connector = if secure { Connector::Rustls(tls()) } else { Connector::Plain };
            let (socket, _) = tungstenite::client_tls_with_config(url, stream, None, Some(connector)).map_err(|error| io::Error::new(io::ErrorKind::ConnectionRefused, error.to_string()))?;
            match socket.get_ref() {
                MaybeTlsStream::Plain(stream) => stream.set_nonblocking(true)?,
                MaybeTlsStream::Rustls(stream) => stream.get_ref().set_nonblocking(true)?,
                _ => return Err(bad("an unknown kind of connection")),
            }
            serve(socket, fd, true)
        }
    }
}

#[cfg(not(unix))]
impl Dial for WebSocketDial {
    fn dial(&self, _url: &str) -> io::Result<(Box<dyn Outgoing>, Box<dyn Incoming>)> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "reaching your other computers isn't built for this system yet"))
    }
}

/// A connection in this process, for what stands in for a relay: the device's two halves and the
/// relay's. Dropping either side's sending half ends the other's reading.
pub fn memory_connection() -> (Halves, Halves) {
    struct ChannelOutgoing(Sender<Vec<u8>>);
    impl Outgoing for ChannelOutgoing {
        fn send(&mut self, message: Vec<u8>) -> io::Result<()> {
            self.0.send(message).map_err(|_| io::Error::new(io::ErrorKind::ConnectionAborted, "the relay connection ended"))
        }
    }
    let (to_device, from_relay) = channel();
    let (to_relay, from_device) = channel();
    ((Box::new(ChannelOutgoing(to_relay)), Box::new(ChannelIncoming(from_relay))), (Box::new(ChannelOutgoing(to_device)), Box::new(ChannelIncoming(from_device))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock(at: Instant) -> PingClock {
        PingClock::new(at, Duration::from_secs(25), Duration::from_secs(10), Duration::from_secs(120))
    }

    #[test]
    fn a_ping_stuck_behind_a_long_upload_doesnt_end_the_connection_its_pong_counts_from_when_it_went_out() {
        let start = Instant::now();
        let mut clock = clock(start);
        let at = |secs: u64| start + Duration::from_secs(secs);
        assert!(!clock.ping_due(at(10)));
        assert!(clock.ping_due(at(25)), "pinged every 25 s");
        // A picture still going out, 40 s on: not dead (it was 10 s before).
        assert!(!clock.dead(at(65)));
        assert!(!clock.ping_due(at(55)), "one ping at a time");
        clock.flushed(at(70));
        assert!(!clock.dead(at(79)));
        clock.pong();
        assert!(!clock.dead(at(200)));
    }

    #[test]
    fn a_pong_late_from_when_the_ping_went_out_or_a_ping_that_never_goes_ends_it() {
        let start = Instant::now();
        let at = |secs: u64| start + Duration::from_secs(secs);
        let mut late = clock(start);
        assert!(late.ping_due(at(25)));
        late.flushed(at(25));
        assert!(!late.dead(at(35)));
        assert!(late.dead(at(36)));
        // Nothing going out at all (the network gone under a send): given up on after the stall.
        let mut stuck = clock(start);
        assert!(stuck.ping_due(at(25)));
        assert!(!stuck.dead(at(145)));
        assert!(stuck.dead(at(146)));
    }
}
