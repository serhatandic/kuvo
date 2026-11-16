//! Against the real relay, run by hand only (never in a suite: it reaches the network).
//!
//! - `KUVO_REAL_RELAY=wss://… cargo test -p kuvo_box --test real_relay -- --ignored --nocapture
//!   at_full_speed`: how a big body fares on the relay itself, two throwaway devices talking.
//! - `pictures_sent_the_desktop_apps_way_reach_the_box`: a desktop app's send of real-size
//!   screenshots, end to end, to a box in a temp home. Make the pictures with
//!   `python3 -I tests/real_relay/noise_png.py <out.png> 5000000` (a 2560×1640 PNG of about 5 MB),
//!   start `python3 -I tests/real_relay/engine.py <port> <log>` (a stand-in engine) and write
//!   `{"port": <port>, "password": "p"}` to the temp home's `run/engine.json`; then, with
//!   `BUN_BE_BUN=1 HOME=<temp> KUVO_HOME=<temp>/.kuvo-code` and the engine's binary as the runtime
//!   (`opencode --config=/dev/null --no-env-file kuvo-link.js …`, as a real box runs), `pair wss://…`
//!   (it prints the link and starts the box) and run this with `KUVO_PAIR=<link>`,
//!   `KUVO_PICTURES=<a.png>:<b.png>`, `KUVO_SESSION=ses_test`, `KUVO_TRIES=10`. Any box version's
//!   `kuvo-link.js` can be put there (an older one, as a user's computer still runs). Stop what you
//!   started by its PID, and remove the temp home.

use std::time::{Duration, Instant};

use kuvo_box::link::relay::Dial;
use kuvo_box::link::{Identity, WebSocketDial, frames};

fn relay() -> String {
    std::env::var("KUVO_REAL_RELAY").expect("KUVO_REAL_RELAY")
}

/// Sends `total` bytes in relay messages of `piece` bytes from one device to another, `pause`
/// between each `every` messages: what arrives, and whether the sender's connection held.
fn blast(total: usize, piece: usize, pause: Duration, every: usize) -> (usize, bool, Duration) {
    let url = relay();
    let (a, b) = (Identity::new(), Identity::new());
    let (mut a_out, mut a_in) = WebSocketDial.dial(&url).expect("a connects");
    a_out.send(frames::relay_hello(&a.secret_on(&url))).unwrap();
    a_in.next().expect("a welcomed");
    let (mut b_out, mut b_in) = WebSocketDial.dial(&url).expect("b connects");
    b_out.send(frames::relay_hello(&b.secret_on(&url))).unwrap();
    b_in.next().expect("b welcomed");
    let to = b.address_on(&url);
    let count = total.div_ceil(piece);
    let reader = std::thread::spawn(move || {
        let mut got = 0usize;
        let started = Instant::now();
        while got < count {
            match b_in.next() {
                Ok(bytes) if bytes.first() == Some(&2) => got += 1,
                Ok(_) => {}
                Err(_) => break,
            }
            if started.elapsed() > Duration::from_secs(180) {
                break;
            }
        }
        (got, started.elapsed())
    });
    let a_watch = std::thread::spawn(move || {
        // The sender's connection: whether the relay ends it (anything it sends back read meanwhile).
        let started = Instant::now();
        loop {
            match a_in.next() {
                Ok(_) => {}
                Err(_) => return Some(started.elapsed()),
            }
            if started.elapsed() > Duration::from_secs(200) {
                return None;
            }
        }
    });
    let payload = vec![7u8; piece];
    for n in 0..count {
        if a_out.send(frames::relay_send(&to, &payload)).is_err() {
            break;
        }
        if every > 0 && n % every == every - 1 {
            std::thread::sleep(pause);
        }
    }
    let (got, took) = reader.join().unwrap();
    drop(a_out);
    let a_ended = a_watch.join().unwrap();
    eprintln!("sent {count} × {piece} B: {got} arrived in {took:?}; sender's connection ended at {a_ended:?}");
    (got * piece, got == count, took)
}

#[test]
#[ignore]
fn thirteen_megabytes_at_full_speed() {
    blast(13_500_000, 65_000, Duration::ZERO, 0);
}

#[test]
#[ignore]
fn one_megabyte_at_full_speed() {
    blast(1_000_000, 65_000, Duration::ZERO, 0);
}

/// A desktop app's send, end to end over the real relay, to a box paired by `KUVO_PAIR` (a
/// `kuvo-link://pair?…` link): `KUVO_PICTURES` (paths, `:`-separated) attached the way the composer
/// attaches them, to `KUVO_SESSION`, `KUVO_TRIES` times. Each must be answered by the box (any
/// status: the body arrived whole), never "Couldn't connect".
#[test]
#[ignore]
fn pictures_sent_the_desktop_apps_way_reach_the_box() {
    use kuvo_box::link::{Host, LinkBox, LinkClient, Timing};
    use kuvo_box::{BoxClient, Draft};
    let link = std::env::var("KUVO_PAIR").expect("KUVO_PAIR");
    let query: std::collections::HashMap<String, String> = link.split_once('?').unwrap().1.split('&').map(|pair| {
        let (name, value) = pair.split_once('=').unwrap();
        (name.to_owned(), value.replace("%3A", ":").replace("%2F", "/").replace('+', " "))
    }).collect();
    let url = query["relay"].clone();
    let host = Host::from_text(&query["key"], &query["host"]).expect("the box's key and address");
    let client = LinkClient::new(url, Identity::new(), "Kuvo test on this Mac", "0.8.0", std::sync::Arc::new(WebSocketDial), Timing::default());
    client.pair(&host, &query["token"]).expect("paired");
    let boxed = BoxClient::new(LinkBox::new(client, host, "test box"));
    let pictures: Vec<std::path::PathBuf> = std::env::var("KUVO_PICTURES").unwrap().split(':').map(Into::into).collect();
    let session = std::env::var("KUVO_SESSION").unwrap_or_else(|_| "ses_test".into());
    let tries: usize = std::env::var("KUVO_TRIES").ok().and_then(|n| n.parse().ok()).unwrap_or(1);
    let mut passed = 0;
    for n in 0..tries {
        let mut draft = Draft::new(BoxClient::new_id(), format!("[Image 1] [Image 2] try {n}"));
        draft.files = kuvo_box::parts::carried(&pictures, false, &|_| false).expect("read");
        let bytes: usize = draft.files.iter().map(|file| file.uri.len()).sum();
        let started = Instant::now();
        let sent = boxed.send_draft(&session, &draft, None);
        eprintln!("try {n}: {bytes} bytes of pictures, {:?} → {sent:?}", started.elapsed());
        if !matches!(sent, Err(kuvo_box::Error::Unreachable(_))) {
            passed += 1;
        }
    }
    eprintln!("{passed}/{tries} reached the box");
    assert_eq!(passed, tries);
}

// MARK: A slow uplink, a dropped connection, a list refreshed: the desktop app's way, end to end

/// A computer's slow uplink to the relay, on this computer: a proxy that passes what's sent on at
/// `rate` bytes a second, what waits staying in the sender's own buffers, and that can drop every
/// connection through it or turn new ones away for a while (the network gone).
struct Uplink {
    port: u16,
    state: std::sync::Arc<std::sync::Mutex<UplinkState>>,
}

struct UplinkState {
    rate: usize,
    down_until: Option<Instant>,
    live: Vec<std::net::TcpStream>,
}

impl Uplink {
    fn start(relay: &str, rate: usize) -> Uplink {
        use std::io::{Read, Write};
        use std::net::{TcpListener, TcpStream};
        let authority = relay.trim_start_matches("wss://").split(['/', '?']).next().unwrap().to_owned();
        let upstream = if authority.contains(':') { authority } else { format!("{authority}:443") };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = std::sync::Arc::new(std::sync::Mutex::new(UplinkState { rate, down_until: None, live: Vec::new() }));
        let shared = state.clone();
        std::thread::spawn(move || {
            for client in listener.incoming().flatten() {
                if shared.lock().unwrap().down_until.is_some_and(|until| Instant::now() < until) {
                    let _ = client.shutdown(std::net::Shutdown::Both);
                    continue;
                }
                let Ok(server) = TcpStream::connect(&upstream) else { continue };
                {
                    let mut state = shared.lock().unwrap();
                    state.live.push(client.try_clone().unwrap());
                    state.live.push(server.try_clone().unwrap());
                }
                // Up: read from this computer's socket at the rate, so what waits stays in its own
                // buffers (which a Mac grows to 4 MB), as on a slow uplink.
                let size: libc::c_int = 64 * 1024;
                // SAFETY: an int option, its size given, on a socket this owns.
                unsafe { libc::setsockopt(std::os::fd::AsRawFd::as_raw_fd(&client), libc::SOL_SOCKET, libc::SO_RCVBUF, (&size as *const libc::c_int).cast(), std::mem::size_of::<libc::c_int>() as libc::socklen_t) };
                let mut from_client = client.try_clone().unwrap();
                let mut to_server = server.try_clone().unwrap();
                let rated = shared.clone();
                std::thread::spawn(move || {
                    let mut buffer = vec![0u8; 8 * 1024];
                    let (started, mut passed) = (Instant::now(), 0usize);
                    while let Ok(count) = from_client.read(&mut buffer) {
                        if count == 0 || to_server.write_all(&buffer[..count]).is_err() {
                            break;
                        }
                        passed += count;
                        let rate = rated.lock().unwrap().rate;
                        // (A by-hand check over the network: the uplink's pace, kept to the clock, not
                        // a test's wait.)
                        if rate > 0 {
                            let due = started + Duration::from_secs_f64(passed as f64 / rate as f64);
                            std::thread::sleep(due.saturating_duration_since(Instant::now()));
                        }
                    }
                    let _ = to_server.shutdown(std::net::Shutdown::Both);
                });
                // Down: as it comes.
                let (mut from_server, mut to_client) = (server, client);
                std::thread::spawn(move || {
                    let _ = std::io::copy(&mut from_server, &mut to_client);
                    let _ = to_client.shutdown(std::net::Shutdown::Both);
                });
            }
        });
        Uplink { port, state }
    }

    /// Every connection through it drops, and none is let through for `down` (the network gone).
    fn cut(&self, down: Duration) {
        let mut state = self.state.lock().unwrap();
        state.down_until = Some(Instant::now() + down);
        for stream in state.live.drain(..) {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }

    fn dial(&self) -> std::sync::Arc<dyn Dial> {
        std::sync::Arc::new(Through(self.port))
    }
}

/// The app's own WebSocket to the relay, made through the uplink.
struct Through(u16);

impl Dial for Through {
    fn dial(&self, url: &str) -> std::io::Result<kuvo_box::link::relay::Halves> {
        use std::os::fd::AsRawFd;
        let stream = std::net::TcpStream::connect(("127.0.0.1", self.0))?;
        let _ = stream.set_nodelay(true);
        stream.set_read_timeout(Some(Duration::from_secs(15)))?;
        stream.set_write_timeout(Some(Duration::from_secs(15)))?;
        let fd = stream.as_raw_fd();
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let config = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(rustls::crypto::ring::default_provider())).with_safe_default_protocol_versions().unwrap().with_root_certificates(roots).with_no_client_auth();
        let (socket, _) = tungstenite::client_tls_with_config(url, stream, None, Some(tungstenite::Connector::Rustls(std::sync::Arc::new(config)))).map_err(|error| std::io::Error::other(error.to_string()))?;
        match socket.get_ref() {
            tungstenite::stream::MaybeTlsStream::Rustls(stream) => stream.get_ref().set_nonblocking(true)?,
            _ => return Err(std::io::Error::other("not TLS")),
        }
        kuvo_box::link::relay::pump(socket, fd, true)
    }
}

/// A desktop app's sends of `KUVO_PICTURES` to the box paired by `KUVO_PAIR`, through your
/// computers' list (`Boxes`, as the app reaches a computer) over a slow uplink (`KUVO_UPLINK` bytes
/// a second; none: at full speed), `KUVO_TRIES` times. With `KUVO_CUT_AFTER=<s>`, each send's
/// connection drops that far into it and the network stays down `KUVO_DOWN_FOR` seconds (a Wi-Fi
/// drop); with `KUVO_REFRESH=1`, the list is taken again while each goes (the same computer, its
/// other facts changed, as the list's follow brings it every few seconds). Each must reach the box:
/// never "Not sent". `KUVO_LINK_LOG=<file>`: the link's log, as the app keeps it.
#[test]
#[ignore]
fn sends_reach_the_box_over_a_slow_uplink_a_dropped_connection_and_a_list_refreshed() {
    use kuvo_box::areas::Computer;
    use kuvo_box::boxes::{Boxes, Options, Timing as BoxesTiming};
    use kuvo_box::link::{Host, Keeper, LinkClient, MemoryKeeper, Timing};
    use kuvo_box::Draft;
    let env = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let link = env("KUVO_PAIR").expect("KUVO_PAIR");
    let query: std::collections::HashMap<String, String> = link.split_once('?').unwrap().1.split('&').map(|pair| {
        let (name, value) = pair.split_once('=').unwrap();
        (name.to_owned(), value.replace("%3A", ":").replace("%2F", "/").replace('+', " "))
    }).collect();
    let url = query["relay"].clone();
    if let Some(path) = env("KUVO_LINK_LOG") {
        kuvo_box::link::log::to_file(path.into());
    }
    let rate: usize = env("KUVO_UPLINK").and_then(|n| n.parse().ok()).unwrap_or(0);
    let uplink = Uplink::start(&url, rate);
    let identity = Identity::new();
    let keeper = std::sync::Arc::new(MemoryKeeper::default());
    keeper.write(&identity.kept());
    // Paired once, as the app's nearest box pairs it.
    let host = Host::from_text(&query["key"], &query["host"]).expect("the box's key and address");
    LinkClient::new(url.clone(), identity.clone(), "Kuvo test on this Mac", "0.8.0", uplink.dial(), Timing::default()).pair(&host, &query["token"]).expect("paired");
    let boxes = Boxes::new(Options { nearest: None, keeper, dial: uplink.dial(), name: "Kuvo test on this Mac".into(), app: "0.8.0".into(), kept: None, timing: BoxesTiming::default() });
    let computer = Computer { key: query["key"].clone(), name: "test box".into(), kind: "machine".into(), reach: "connected".into(), address: Some(query["host"].clone()), relay: Some(url.clone()), ..Computer::default() };
    boxes.take(std::slice::from_ref(&computer), false);
    let boxed = boxes.client(&computer.key).expect("a client for it");
    let pictures: Vec<std::path::PathBuf> = env("KUVO_PICTURES").map(|paths| paths.split(':').map(Into::into).collect()).unwrap_or_default();
    let session = env("KUVO_SESSION").unwrap_or_else(|| "ses_test".into());
    let tries: usize = env("KUVO_TRIES").and_then(|n| n.parse().ok()).unwrap_or(1);
    let cut_after = env("KUVO_CUT_AFTER").and_then(|n| n.parse::<f64>().ok()).map(Duration::from_secs_f64);
    let down_for = env("KUVO_DOWN_FOR").and_then(|n| n.parse::<f64>().ok()).map_or(Duration::ZERO, Duration::from_secs_f64);
    let refresh = env("KUVO_REFRESH").is_some();
    let mut passed = 0;
    for n in 0..tries {
        let mut draft = Draft::new(kuvo_box::BoxClient::new_id(), format!("{} try {n}", (1..=pictures.len()).map(|i| format!("[Image {i}]")).collect::<Vec<_>>().join(" ")));
        draft.files = kuvo_box::parts::carried(&pictures, false, &|_| false).expect("read");
        let bytes: usize = draft.files.iter().map(|file| file.uri.len()).sum();
        let started = Instant::now();
        let going = {
            let (boxed, session) = (boxed.clone(), session.clone());
            std::thread::spawn(move || boxed.send_draft(&session, &draft, None))
        };
        let mut cut = cut_after;
        let mut refreshed = 0;
        while !going.is_finished() {
            std::thread::sleep(Duration::from_millis(250));
            if cut.is_some_and(|after| started.elapsed() >= after) {
                cut = None;
                eprintln!("  try {n}: the connection drops at {:?}, down for {down_for:?}", started.elapsed());
                uplink.cut(down_for);
            }
            if refresh && started.elapsed() >= Duration::from_secs(refreshed + 1) {
                refreshed += 1;
                let mut changed = computer.clone();
                changed.version = Some(format!("0.8.{refreshed}"));
                changed.working = Some(refreshed as usize % 3);
                changed.reach = if refreshed % 2 == 0 { "connected".into() } else { "connecting".into() };
                boxes.take(&[changed], false);
            }
        }
        let sent = going.join().unwrap();
        eprintln!("try {n}: {bytes} bytes of pictures, {:?} → {sent:?}", started.elapsed());
        if sent.is_ok() {
            passed += 1;
        }
        assert_eq!(boxes.client_made(&computer.key), Some(1), "the list's refreshes kept the same client");
    }
    eprintln!("{passed}/{tries} reached the box");
    assert_eq!(passed, tries);
}
