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
