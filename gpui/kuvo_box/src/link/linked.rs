//! A box reached over the link: the same [`Transport`] a box on this computer gives, so a
//! [`crate::BoxClient`] on top asks both the same way.

use std::io::BufReader;

use super::client::{LinkClient, words};
use super::keys::Host;
use crate::error::{Error, Result};
use crate::transport::{Closer, Request, Response, Stream, Transport};

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

    fn whole(&self, request: &Request) -> Result<Response> {
        let mut answer = self.client.request(&self.host, request)?;
        let (status, content_type) = answer.head()?;
        Ok(Response { status, content_type, body: answer.body()? })
    }
}

impl Transport for LinkBox {
    fn send(&self, request: &Request) -> Result<Response> {
        match self.whole(request) {
            // A read whose session went under it (the box started again, the relay connection
            // dropped) is read again, once, not shown as a failure (PROTOCOL.md, "When a client
            // says a computer isn't connected"). Anything that changes something isn't sent twice.
            // So is one sent under its own id (a message): its box takes an id once, so sent again
            // it goes once, answered as the first went (PROTOCOL.md, "Sending again"). The answer to
            // one that reached the box before the connection dropped isn't lost for it.
            Err(Error::Unreachable(why)) if (request.method.eq_ignore_ascii_case("GET") || request.headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("idempotency-key"))) && (why == words::CLOSED || why == words::RELAY) => self.whole(request),
            answered => answered,
        }
    }

    fn open(&self, request: &Request, hold: &dyn Fn(Closer) -> bool) -> Result<Stream> {
        let mut answer = self.client.request(&self.host, request)?;
        let handle = answer.handle();
        let closer = Closer::new(move || handle.cancel());
        if !hold(closer.clone()) {
            closer.close();
            return Err(Error::Unreachable("Stopped.".into()));
        }
        let (status, content_type) = answer.head()?;
        Ok(Stream { status, content_type, reader: Box::new(BufReader::new(answer.reader())), closer })
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
}
