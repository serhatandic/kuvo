//! A box reached over the link: the same [`Transport`] a box on this computer gives, so a
//! [`crate::BoxClient`] on top asks both the same way.

use std::io::BufReader;
use std::time::{Duration, Instant, SystemTime};

use super::client::{LinkClient, words};
use super::keys::Host;
use super::log;
use crate::error::{Error, Result};
use crate::transport::{Closer, Request, Response, Stream, Transport};

/// What a message taken back by Stop before it went says.
pub const STOPPED: &str = "Stopped.";

/// One box, by its key and where it is on the relay.
pub struct LinkBox {
    client: LinkClient,
    host: Host,
    label: String,
}

impl LinkBox {
    /// `label`: the computer's name, for a refusal of the client's own.
    pub fn new(client: LinkClient, host: Host, label: impl Into<String>) -> LinkBox {
        LinkBox { client, host, label: label.into() }
    }

    pub fn host(&self) -> &Host {
        &self.host
    }

    /// Its answer, whole; failing, whether it went (was put on a session, so the box may have it).
    fn whole(&self, request: &Request, eager: bool) -> std::result::Result<Response, (Error, bool)> {
        let mut answer = self.client.request_now(&self.host, request, eager).map_err(|error| (error, false))?;
        let (status, content_type) = answer.head().map_err(|error| (error, true))?;
        Ok(Response { status, content_type, body: answer.body().map_err(|error| (error, true))? })
    }

    /// One sent under its own id (a message), sent again while its link comes back (PROTOCOL.md,
    /// "Sending again"): at once, then after 1, 2, 4 and 8 s (8 s from then on), or as soon as a
    /// relay connection is made, for up to 2 minutes from when it first failed; its box takes an id
    /// once, so however often it goes it goes once, answered as the first went. One that never
    /// went to a box that isn't on the relay, or one the box turned away, isn't sent again; nor is
    /// one Stop took back before it ever went.
    fn repeated(&self, request: &Request, key: &str) -> Result<Response> {
        self.client.repeating(key, true);
        let sent = self.repeating(request, key);
        self.client.repeating(key, false);
        sent
    }

    fn repeating(&self, request: &Request, key: &str) -> Result<Response> {
        let timing = self.client.timing();
        // Since it first failed, and how many tries: timed by the wall clock, which goes on while
        // the computer sleeps, so one whose two minutes passed meanwhile fails rather than going late.
        let mut failed: Option<(SystemTime, u32)> = None;
        let mut went = false;
        let past = |since: SystemTime| SystemTime::now().duration_since(since).unwrap_or_default();
        loop {
            // (A relay connection made after this, the relay's back: what waits goes then.)
            let connections = self.client.connections();
            match self.whole(request, failed.is_some()) {
                Ok(response) => {
                    if let Some((since, tries)) = failed {
                        log::note(format!("{}: {} {} got there on try {}, {:.1} s after it first failed", self.label, request.method, request.route(), tries + 1, past(since).as_secs_f64()));
                    }
                    return Ok(response);
                }
                Err((error, placed)) => {
                    went |= placed;
                    let again = match &error {
                        Error::Unreachable(why) => why == words::CLOSED || why == words::TIMED_OUT || why == words::RELAY || (why == words::OFFLINE && went),
                        _ => false,
                    };
                    if !again {
                        if failed.is_some() {
                            log::note(format!("{}: {} {} not sent: {error}", self.label, request.method, request.route()));
                        }
                        return Err(error);
                    }
                    let (since, tries) = failed.map_or((SystemTime::now(), 1), |(since, tries)| (since, tries + 1));
                    failed = Some((since, tries));
                    let wait = if tries == 1 { Duration::ZERO } else { timing.repeat_first.saturating_mul(1 << (tries - 2).min(8)).min(timing.repeat_most) };
                    let left = timing.repeat_for.saturating_sub(past(since));
                    if left.is_zero() {
                        log::note(format!("{}: {} {} not sent after {tries} tries in {:.0} s: {error}", self.label, request.method, request.route(), past(since).as_secs_f64()));
                        return Err(error);
                    }
                    log::note(format!("{}: {} {} didn’t get there ({error}); sent again {}", self.label, request.method, request.route(), if wait.is_zero() { "now".to_owned() } else { format!("in {wait:?}, or once the relay’s back") }));
                    if !wait.is_zero() {
                        self.client.wait_for_relay(connections, Instant::now() + wait.min(left), key);
                    }
                    // Taken back before it ever went (Stop), or its time's up meanwhile (the
                    // computer slept): not sent.
                    if !went && self.client.is_taken_back(key) {
                        log::note(format!("{}: {} {} taken back by Stop", self.label, request.method, request.route()));
                        return Err(Error::Unreachable(STOPPED.into()));
                    }
                    if past(since) >= timing.repeat_for {
                        log::note(format!("{}: {} {} not sent after {tries} tries in {:.0} s: {error}", self.label, request.method, request.route(), past(since).as_secs_f64()));
                        return Err(error);
                    }
                }
            }
        }
    }
}

impl Transport for LinkBox {
    fn send(&self, request: &Request) -> Result<Response> {
        if let Some((_, key)) = request.headers.iter().find(|(name, value)| name.eq_ignore_ascii_case("idempotency-key") && !value.is_empty()) {
            return self.repeated(request, key);
        }
        match self.whole(request, false) {
            // A read whose session went under it (the box started again, the relay connection
            // dropped) is read again, once, not shown as a failure (PROTOCOL.md, "When a client
            // says a computer isn't connected"). Anything else that changes something isn't sent
            // twice: only one under its own id (`repeated`).
            Err((Error::Unreachable(why), _)) if request.method.eq_ignore_ascii_case("GET") && (why == words::CLOSED || why == words::RELAY) => self.whole(request, false).map_err(|(error, _)| error),
            answered => answered.map_err(|(error, _)| error),
        }
    }

    fn open(&self, request: &Request, hold: &dyn Fn(Closer) -> bool) -> Result<Stream> {
        let mut answer = self.client.request(&self.host, request)?;
        let handle = answer.handle();
        let closer = Closer::new(move || handle.cancel());
        if !hold(closer.clone()) {
            closer.close();
            return Err(Error::Unreachable(STOPPED.into()));
        }
        let (status, content_type) = answer.head()?;
        Ok(Stream { status, content_type, reader: Box::new(BufReader::new(answer.reader().with_idle_timeout(request.event_timeout()))), closer })
    }

    fn refresh(&self) -> u64 {
        self.client.sessions_over(&self.host)
    }

    fn streams(&self) -> bool {
        true
    }

    fn describe(&self) -> String {
        self.label.clone()
    }

    fn take_back(&self, keys: &[String]) {
        self.client.take_back(keys);
    }
}
