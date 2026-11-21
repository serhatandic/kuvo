//! The client chooses a route to each computer before sending its request.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::transport::{Closer, Request, Response, Socket, Stream, Transport, sockets_not_here};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConnectionMethod {
    #[default]
    Automatic,
    Ssh,
    Relay,
}

/// A device's choice for one computer. Its SSH host may be an alias from this device's config.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionPreference {
    pub method: ConnectionMethod,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
}

pub(crate) fn valid_destination(address: &str) -> bool {
    !address.is_empty() && address.len() <= 320 && !address.starts_with(['-', '@']) && !address.ends_with('@')
        && address.bytes().filter(|byte| *byte == b'@').count() <= 1
        && !address.eq_ignore_ascii_case("this-mac.localhost")
        && address.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._@%+=,~-".contains(&byte))
}

pub(crate) fn destinations(preference: &ConnectionPreference, advertised: &[String]) -> Vec<String> {
    if preference.method == ConnectionMethod::Relay {
        return Vec::new();
    }
    let mut addresses = Vec::new();
    for address in preference.destination.iter().chain(advertised) {
        if valid_destination(address) && !addresses.contains(address) {
            addresses.push(address.clone());
        }
    }
    addresses.truncate(4);
    addresses
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Route {
    Ssh(usize),
    Relay,
    Unpaired,
    Unavailable,
}

#[derive(Default)]
struct Selection {
    checked: Option<(Instant, Route)>,
    epoch: u64,
}

pub(crate) struct ComputerTransport {
    name: String,
    method: ConnectionMethod,
    ssh: Vec<Arc<dyn Transport>>,
    relay: Option<Arc<dyn Transport>>,
    selected: Mutex<Selection>,
}

impl ComputerTransport {
    pub(crate) fn new(name: String, method: ConnectionMethod, ssh: Vec<Arc<dyn Transport>>, relay: Option<Arc<dyn Transport>>) -> Self {
        let ssh = if method == ConnectionMethod::Relay { Vec::new() } else { ssh };
        let relay = if method == ConnectionMethod::Ssh { None } else { relay };
        Self { name, method, ssh, relay, selected: Mutex::default() }
    }

    pub(crate) fn active(&self) -> Option<ConnectionMethod> {
        let selected = self.selected.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match selected.checked?.1 {
            Route::Relay => Some(ConnectionMethod::Relay),
            Route::Ssh(_) => Some(ConnectionMethod::Ssh),
            Route::Unpaired | Route::Unavailable => None,
        }
    }

    fn remember(&self, route: Route) -> Route {
        let mut selected = self.selected.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if selected.checked.is_none_or(|(_, current)| current != route) {
            selected.epoch = selected.epoch.wrapping_add(1);
        }
        selected.checked = Some((Instant::now(), route));
        route
    }

    fn select(&self, hold: &dyn Fn(Closer) -> bool) -> Route {
        let checked = self.selected.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).checked;
        if let Some((at, route)) = checked {
            if route != Route::Unpaired && at.elapsed() < Duration::from_secs(30) {
                return route;
            }
        }
        let mut unpaired = false;
        for (index, ssh) in self.ssh.iter().enumerate() {
            if !hold(Closer::nothing()) {
                return Route::Unavailable;
            }
            match ssh.probe(hold) {
                Ok(()) => return self.remember(Route::Ssh(index)),
                Err(error) => unpaired |= crate::link::is_unpaired(&error),
            }
        }
        // Pairing can finish while the same connector remains selected. Check again on its next request.
        self.remember(if self.relay.is_some() { Route::Relay } else if unpaired { Route::Unpaired } else { Route::Unavailable })
    }

    fn transport(&self, route: Route) -> Result<&dyn Transport> {
        match route {
            Route::Ssh(index) => Ok(self.ssh[index].as_ref()),
            Route::Relay => self.relay.as_deref().ok_or_else(|| self.unavailable()),
            Route::Unpaired => Err(crate::link::client::unpaired()),
            Route::Unavailable => Err(self.unavailable()),
        }
    }

    fn unavailable(&self) -> Error {
        let through = match self.method {
            ConnectionMethod::Automatic => "",
            ConnectionMethod::Ssh => " over SSH",
            ConnectionMethod::Relay => " over the relay",
        };
        Error::Unreachable(format!("Couldn't reach {}{through}.", self.name))
    }

    fn failed_ssh(&self, route: Route) {
        if matches!(route, Route::Ssh(_)) {
            let failed = if self.relay.is_some() { Route::Relay } else { Route::Unavailable };
            self.remember(failed);
        }
    }

    fn can_read_again(&self, route: Route, request: &Request) -> bool {
        self.method == ConnectionMethod::Automatic && self.relay.is_some() && matches!(route, Route::Ssh(_))
            && (request.method.eq_ignore_ascii_case("GET") || request.method.eq_ignore_ascii_case("HEAD"))
    }

    fn revoked_ssh(&self, route: Route) {
        if matches!(route, Route::Ssh(_)) {
            self.remember(if self.relay.is_some() { Route::Relay } else { Route::Unpaired });
        }
    }
}

impl Transport for ComputerTransport {
    fn send(&self, request: &Request) -> Result<Response> {
        let route = self.select(&|_| true);
        let result = self.transport(route)?.send(request);
        if result.as_ref().is_err_and(crate::link::is_unpaired) {
            self.revoked_ssh(route);
            if matches!(route, Route::Ssh(_)) && self.relay.is_some() {
                // The paired handshake refused the device before sending this request.
                return self.transport(Route::Relay)?.send(request);
            }
        }
        if matches!(&result, Err(Error::Unreachable(_))) {
            self.failed_ssh(route);
            // A mutating request might have reached SSH before the connection dropped.
            if self.can_read_again(route, request) {
                return self.transport(Route::Relay)?.send(request);
            }
        }
        result
    }

    fn open(&self, request: &Request, hold: &dyn Fn(Closer) -> bool) -> Result<Stream> {
        let route = self.select(hold);
        if !hold(Closer::nothing()) {
            return Err(self.unavailable());
        }
        let result = self.transport(route)?.open(request, hold);
        if result.as_ref().is_err_and(crate::link::is_unpaired) {
            self.revoked_ssh(route);
            if matches!(route, Route::Ssh(_)) && self.relay.is_some() && hold(Closer::nothing()) {
                return self.transport(Route::Relay)?.open(request, hold);
            }
        }
        if matches!(&result, Err(Error::Unreachable(_))) {
            self.failed_ssh(route);
            if self.can_read_again(route, request) && hold(Closer::nothing()) {
                return self.transport(Route::Relay)?.open(request, hold);
            }
        }
        result
    }

    /// A socket goes over the relay, even where requests go over SSH, which can't carry one.
    fn socket(&self, request: &Request) -> Result<Socket> {
        match &self.relay {
            Some(relay) => relay.socket(request),
            None => Err(sockets_not_here(format!("Terminals on {} aren’t available over SSH yet.", self.name))),
        }
    }

    fn streams(&self) -> bool {
        true
    }

    fn refresh(&self) -> u64 {
        let epoch = self.selected.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).epoch;
        epoch.wrapping_add(self.relay.as_ref().map_or(0, |relay| relay.refresh()))
    }

    fn describe(&self) -> String {
        self.name.clone()
    }

    fn take_back(&self, keys: &[String]) {
        if let Some(relay) = &self.relay {
            relay.take_back(keys);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[derive(Default)]
    struct Relay(AtomicUsize);

    impl Transport for Relay {
        fn send(&self, _request: &Request) -> Result<Response> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(Response { status: 200, content_type: None, body: b"relay".to_vec() })
        }

        fn open(&self, request: &Request, _hold: &dyn Fn(Closer) -> bool) -> Result<Stream> {
            let response = self.send(request)?;
            Ok(Stream { status: response.status, content_type: None, reader: Box::new(std::io::Cursor::new(response.body)), closer: Closer::nothing() })
        }

        fn streams(&self) -> bool { true }
        fn describe(&self) -> String { "Relay".into() }
    }

    #[derive(Default)]
    struct Ssh {
        away: AtomicBool,
        unpaired: AtomicBool,
        requests: Mutex<Vec<Request>>,
    }

    impl Transport for Ssh {
        fn send(&self, request: &Request) -> Result<Response> {
            self.requests.lock().unwrap().push(request.clone());
            if self.unpaired.load(Ordering::Relaxed) {
                return Err(crate::link::client::unpaired());
            }
            if self.away.load(Ordering::Relaxed) || request.path == "change" {
                return Err(Error::Unreachable("SSH closed".into()));
            }
            Ok(Response { status: 200, content_type: None, body: b"ssh".to_vec() })
        }

        fn open(&self, _request: &Request, _hold: &dyn Fn(Closer) -> bool) -> Result<Stream> {
            Ok(Stream { status: 200, content_type: None, reader: Box::new(std::io::Cursor::new(b"{\"ssh\":true}\n")), closer: Closer::nothing() })
        }

        fn streams(&self) -> bool { true }
        fn describe(&self) -> String { "SSH".into() }
    }

    #[test]
    fn auto_uses_verified_ssh_and_never_resends_a_lost_write_on_the_relay() {
        let ssh = Arc::new(Ssh::default());
        let relay = Arc::new(Relay::default());
        let client = ComputerTransport::new("Lab".into(), ConnectionMethod::Automatic, vec![ssh.clone()], Some(relay.clone()));
        assert_eq!(client.send(&Request::get("read")).unwrap().body, b"ssh");
        let mut stream = client.open(&Request::get("events"), &|_| true).unwrap();
        let mut body = String::new();
        stream.reader.read_to_string(&mut body).unwrap();
        assert_eq!(body, "{\"ssh\":true}\n");
        assert_eq!(client.active(), Some(ConnectionMethod::Ssh));
        let change = Request::post("change", serde_json::json!({"choice": true})).header("idempotency-key", "one-save");
        assert!(client.send(&change).is_err());
        assert_eq!(relay.0.load(Ordering::Relaxed), 0);
        assert_eq!(client.send(&Request::get("read")).unwrap().body, b"relay");
        let requests = ssh.requests.lock().unwrap();
        assert_eq!(requests.iter().filter(|request| request.path == "change").collect::<Vec<_>>(), [&change]);
    }

    #[test]
    fn explicit_routes_do_not_probe_or_fall_back_to_the_other_route() {
        let ssh = Arc::new(Ssh::default());
        let relay = Arc::new(Relay::default());
        let forced = ComputerTransport::new("Lab".into(), ConnectionMethod::Relay, vec![ssh.clone()], Some(relay.clone()));
        assert_eq!(forced.send(&Request::get("read")).unwrap().body, b"relay");
        assert!(ssh.requests.lock().unwrap().is_empty());
        ssh.away.store(true, Ordering::Relaxed);
        let forced = ComputerTransport::new("Lab".into(), ConnectionMethod::Ssh, vec![ssh.clone()], Some(relay.clone()));
        assert!(forced.send(&Request::get("read")).is_err());
        assert_eq!(relay.0.load(Ordering::Relaxed), 1);
        let auto = ComputerTransport::new("Lab".into(), ConnectionMethod::Automatic, vec![ssh], Some(relay.clone()));
        assert_eq!(auto.send(&Request::get("read")).unwrap().body, b"relay");
    }

    #[test]
    fn forced_ssh_remembers_a_pairing_refusal_and_recovers_when_paired() {
        let ssh = Arc::new(Ssh::default());
        ssh.unpaired.store(true, Ordering::Relaxed);
        let relay = Arc::new(Relay::default());
        let client = ComputerTransport::new("Lab".into(), ConnectionMethod::Ssh, vec![ssh.clone()], Some(relay.clone()));
        assert!(client.send(&Request::get("read")).as_ref().is_err_and(crate::link::is_unpaired));
        ssh.unpaired.store(false, Ordering::Relaxed);
        assert_eq!(client.send(&Request::get("read")).unwrap().body, b"ssh");
        assert_eq!(relay.0.load(Ordering::Relaxed), 0);
    }
}
