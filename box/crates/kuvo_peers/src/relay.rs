//! The Tokio owner of one reconnecting relay socket. An accepted send has been written to the
//! socket. Peer mail still needs an authenticated application receipt before it can be retired.

use std::{collections::HashMap, time::Duration};

use futures_util::{SinkExt, StreamExt};
use kuvo_wire::{
    frames::{self, FromRelay, Kind, relay_hello, relay_send},
    keys::{Address, Host, address_of},
    mail::Envelope,
};
use tokio::{
    net::TcpStream,
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::{self, Instant},
};
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream, connect_async_tls_with_config,
    tungstenite::{Message as Ws, protocol::WebSocketConfig},
};

use crate::session::{self, BoxHello, ClientHello, Id, Output, Sessions};

const WELCOME_WITHIN: Duration = Duration::from_secs(15);
const HEARTBEAT_EVERY: Duration = Duration::from_secs(30);
const ALIVE_FOR: Duration = Duration::from_secs(90);
const WRITE_WITHIN: Duration = Duration::from_secs(15);

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
type Reply<T> = oneshot::Sender<Result<T, Error>>;

pub struct Config {
    pub url: String,
    pub secret: [u8; 32],
    pub key: [u8; 32],
    pub hello: BoxHello,
    /// Device public keys and the identifiers of their pairing grants.
    pub grants: HashMap<[u8; 32], String>,
    /// Ordinary mail is authenticated against these keys; addresses only prioritize lookup.
    pub peers: Vec<Host>,
}

#[derive(Debug)]
pub enum Event {
    Connected(Address),
    Disconnected(Error),
    Unreachable(Address),
    Gone {
        address: Address,
        replaced: bool,
    },
    Session(session::Event),
    Mail {
        from: Address,
        key: [u8; 32],
        envelope: Envelope,
    },
    Removal {
        from: Address,
        hinted_key: [u8; 32],
        envelope: Envelope,
    },
}

/// The subset of authenticated relay events consumed by setup authority.
pub enum SetupEvent {
    Connected,
    Mail { key: [u8; 32], envelope: Envelope },
}

impl Event {
    pub fn for_setup(&self) -> Option<SetupEvent> {
        match self {
            Self::Connected(_) => Some(SetupEvent::Connected),
            Self::Mail { key, envelope, .. }
                if [
                    "docs",
                    "setupAuthority",
                    "scopedOfflineEdit",
                    "offlineEditReceipt",
                ]
                .iter()
                .any(|field| envelope.0.contains_key(*field)) =>
            {
                Some(SetupEvent::Mail {
                    key: *key,
                    envelope: envelope.clone(),
                })
            }
            _ => None,
        }
    }
}

pub struct SetupLink {
    pub sender: Sender,
    pub events: mpsc::Receiver<SetupEvent>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("relay connection is offline")]
    Offline,
    #[error("relay closed the connection{0}")]
    Closed(String),
    #[error("relay WebSocket: {0}")]
    WebSocket(#[from] Box<tokio_tungstenite::tungstenite::Error>),
    #[error("relay welcome timed out")]
    WelcomeTimeout,
    #[error("relay returned an unexpected welcome")]
    WrongWelcome,
    #[error("relay heartbeat timed out")]
    HeartbeatTimeout,
    #[error("relay write timed out")]
    WriteTimeout,
    #[error("relay event receiver stopped consuming events")]
    EventQueue,
    #[error("session has ended or is still opening")]
    NoSession,
    #[error("invalid session message size or head")]
    Message,
    #[error("starting session: {0}")]
    Noise(#[from] kuvo_wire::noise::Failed),
    #[error("sending peer mail: {0}")]
    Mail(#[from] kuvo_wire::mail::Error),
}

enum MailRoute {
    Member,
    Removal,
}

enum Command {
    Mail(Host, Envelope, MailRoute, Reply<()>),
    Start(Host, ClientHello, Reply<Id>),
    Send(Id, session::Message, Reply<()>),
    Close(Id, Reply<()>),
    Trust {
        grants: HashMap<[u8; 32], String>,
        peers: Vec<Host>,
    },
}

impl Command {
    fn offline(self, config: &mut Config) {
        match self {
            Self::Mail(_, _, _, reply) | Self::Send(_, _, reply) | Self::Close(_, reply) => {
                let _ = reply.send(Err(Error::Offline));
            }
            Self::Start(_, _, reply) => {
                let _ = reply.send(Err(Error::Offline));
            }
            Self::Trust { grants, peers } => {
                config.grants = grants;
                config.peers = peers;
            }
        }
    }
}

/// Commands can be shared by routing and peer workers. The `Relay` owns their lifetime.
#[derive(Clone)]
pub struct Sender(mpsc::Sender<Command>);

impl Sender {
    async fn request<T>(&self, command: impl FnOnce(Reply<T>) -> Command) -> Result<T, Error> {
        let (reply, result) = oneshot::channel();
        self.0
            .send(command(reply))
            .await
            .map_err(|_| Error::Offline)?;
        result.await.map_err(|_| Error::Offline)?
    }

    pub async fn mail(&self, to: Host, envelope: Envelope) -> Result<(), Error> {
        self.request(|reply| Command::Mail(to, envelope, MailRoute::Member, reply))
            .await
    }

    pub async fn removal(&self, to: Host, envelope: Envelope) -> Result<(), Error> {
        self.request(|reply| Command::Mail(to, envelope, MailRoute::Removal, reply))
            .await
    }

    pub async fn start(&self, host: Host, hello: ClientHello) -> Result<Id, Error> {
        self.request(|reply| Command::Start(host, hello, reply))
            .await
    }

    pub async fn send(&self, id: Id, message: session::Message) -> Result<(), Error> {
        self.request(|reply| Command::Send(id, message, reply))
            .await
    }

    pub async fn close(&self, id: Id) -> Result<(), Error> {
        self.request(|reply| Command::Close(id, reply)).await
    }

    pub async fn trust(
        &self,
        grants: HashMap<[u8; 32], String>,
        peers: Vec<Host>,
    ) -> Result<(), Error> {
        self.0
            .send(Command::Trust { grants, peers })
            .await
            .map_err(|_| Error::Offline)
    }
}

pub struct Relay {
    pub sender: Sender,
    pub events: mpsc::Receiver<Event>,
    task: JoinHandle<()>,
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Relay {
    pub fn start(config: Config) -> Self {
        let (commands, rx) = mpsc::channel(64);
        let (events, event_rx) = mpsc::channel(128);
        let task = tokio::spawn(
            Owner {
                config,
                rx,
                events,
                sessions: Sessions::default(),
            }
            .run(),
        );
        Self {
            sender: Sender(commands),
            events: event_rx,
            task,
        }
    }
}

struct Owner {
    config: Config,
    rx: mpsc::Receiver<Command>,
    events: mpsc::Sender<Event>,
    sessions: Sessions,
}

impl Owner {
    async fn run(mut self) {
        let mut attempts = 0;
        loop {
            let url = self.config.url.clone();
            let opening = time::timeout(WELCOME_WITHIN, connect(&url, self.config.secret));
            let mut since = None;
            let reason = match self.while_offline(opening).await {
                None => return,
                Some(Ok(Ok(mut socket))) => {
                    since = Some(Instant::now());
                    if self
                        .events
                        .send(Event::Connected(address_of(&self.config.secret)))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    match self.serve(&mut socket).await {
                        Err(reason) => reason,
                        Ok(()) => return,
                    }
                }
                Some(Ok(Err(error))) => error,
                Some(Err(_)) => Error::WelcomeTimeout,
            };
            if matches!(reason, Error::EventQueue) {
                // A consumer that fell behind cannot safely keep its sessions. Dropping the event
                // stream also wakes every waiter even if its queue has no room for a close.
                return;
            }
            for event in self.sessions.reset() {
                if self.events.send(Event::Session(event)).await.is_err() {
                    return;
                }
            }
            if self.events.send(Event::Disconnected(reason)).await.is_err() {
                return;
            }
            let healthy = since.is_some_and(|since| since.elapsed() >= Duration::from_secs(60));
            let wait = next_try(&mut attempts, healthy);
            if self.while_offline(time::sleep(wait)).await.is_none() {
                return;
            }
        }
    }

    async fn while_offline<T>(&mut self, future: impl Future<Output = T>) -> Option<T> {
        tokio::pin!(future);
        loop {
            tokio::select! {
                result = &mut future => return Some(result),
                command = self.rx.recv() => command?.offline(&mut self.config),
                _ = self.events.closed() => return None,
            }
        }
    }

    async fn serve(&mut self, socket: &mut Socket) -> Result<(), Error> {
        let address = address_of(&self.config.secret);
        let mut last_heard = Instant::now();
        let mut heartbeat = time::interval_at(last_heard + HEARTBEAT_EVERY, HEARTBEAT_EVERY);
        heartbeat.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = self.events.closed() => return Ok(()),
                next = socket.next() => {
                    let message = websocket_message(next)?;
                    match message {
                        Ws::Binary(bytes) => {
                            last_heard = Instant::now();
                            if let Some(relay) = frames::read_relay(&bytes) {
                                self.receive(socket, address, relay).await?;
                            }
                        }
                        Ws::Ping(_) => flush(socket).await?,
                        _ => {}
                    }
                }
                _ = heartbeat.tick() => {
                    if last_heard.elapsed() > ALIVE_FOR {
                        return Err(Error::HeartbeatTimeout);
                    }
                    write(socket, relay_send(&address, &[])).await?;
                }
                command = self.rx.recv() => {
                    let Some(command) = command else { return Ok(()) };
                    self.command(socket, command).await?;
                }
            }
        }
    }

    async fn receive(
        &mut self,
        socket: &mut Socket,
        own: Address,
        relay: FromRelay,
    ) -> Result<(), Error> {
        match relay {
            FromRelay::Deliver { from, payload } if from != own => {
                let Some((kind, number, body)) = frames::read_frame(&payload) else {
                    return Ok(());
                };
                if kind == Kind::Mail {
                    if number == 0 {
                        self.receive_mail(from, body)?;
                    }
                } else {
                    let output = self.sessions.receive(
                        self.config.key,
                        from,
                        (kind, number, body),
                        &self.config.hello,
                        &self.config.grants,
                    );
                    self.output(socket, from, output).await?;
                }
            }
            FromRelay::Unreachable(address) if address != own => {
                if let Some(event) = self.sessions.forget(address) {
                    self.emit(Event::Session(event))?;
                }
                self.emit(Event::Unreachable(address))?;
            }
            FromRelay::Gone { address, replaced } if address != own => {
                let output = self.sessions.gone(self.config.key, address);
                self.output(socket, address, output).await?;
                self.emit(Event::Gone { address, replaced })?;
            }
            _ => {}
        }
        Ok(())
    }

    fn receive_mail(&self, from: Address, body: &[u8]) -> Result<(), Error> {
        if let Some((hinted_key, envelope)) = Envelope::open_removal(body, &self.config.key) {
            return self.emit(Event::Removal {
                from,
                hinted_key,
                envelope,
            });
        }
        let peers = &self.config.peers;
        let candidates = peers
            .iter()
            .filter(|peer| peer.address == from)
            .chain(peers.iter().filter(|peer| peer.address != from));
        for peer in candidates {
            match Envelope::open(body, &self.config.key, &peer.key) {
                Ok(envelope) => {
                    return self.emit(Event::Mail {
                        from,
                        key: peer.key,
                        envelope,
                    });
                }
                Err(kuvo_wire::mail::Error::Noise) => {}
                Err(_) => break,
            }
        }
        Ok(())
    }

    async fn command(&mut self, socket: &mut Socket, command: Command) -> Result<(), Error> {
        match command {
            Command::Mail(host, envelope, route, reply) => {
                let sealed = match route {
                    MailRoute::Member => envelope.seal(&self.config.key, &host.key),
                    MailRoute::Removal => envelope.seal_removal(&self.config.key, &host.key),
                };
                match sealed {
                    Ok(payload) => write(socket, relay_send(&host.address, &payload)).await?,
                    Err(error) => {
                        let _ = reply.send(Err(error.into()));
                        return Ok(());
                    }
                }
                let _ = reply.send(Ok(()));
            }
            Command::Start(host, hello, reply) => {
                match self.sessions.initiate(self.config.key, host, hello) {
                    Ok((id, output)) => {
                        self.output(socket, id.address, output).await?;
                        let _ = reply.send(Ok(id));
                    }
                    Err(error) => {
                        let _ = reply.send(Err(error.into()));
                    }
                }
            }
            Command::Send(id, message, reply) => {
                let result = if !message.head.is_object()
                    || message
                        .body
                        .len()
                        .saturating_add(frames::sorted_json(&message.head).len() + 2)
                        > frames::MAX_MESSAGE
                {
                    Err(Error::Message)
                } else {
                    let plain = frames::encode_message(&message.head, &message.body);
                    let output = self.sessions.send(id, &plain);
                    self.send_output(socket, id, output).await?
                };
                let _ = reply.send(result);
            }
            Command::Close(id, reply) => {
                let output = self.sessions.close(id);
                let _ = reply.send(self.send_output(socket, id, output).await?);
            }
            Command::Trust { grants, peers } => {
                self.config.peers = peers;
                self.config.grants = grants;
                for id in self.sessions.revoke(&self.config.grants) {
                    let output = self.sessions.close(id);
                    self.send_output(socket, id, output).await?.ok();
                }
            }
        }
        Ok(())
    }

    async fn send_output(
        &self,
        socket: &mut Socket,
        id: Id,
        output: Option<Output>,
    ) -> Result<Result<(), Error>, Error> {
        if let Some(output) = output {
            self.output(socket, id.address, output).await?;
            Ok(Ok(()))
        } else {
            Ok(Err(Error::NoSession))
        }
    }

    async fn output(&self, socket: &mut Socket, to: Address, output: Output) -> Result<(), Error> {
        if let Some(frame) = output.frame {
            write(socket, relay_send(&to, &frame)).await?;
        }
        for event in output.events {
            self.emit(Event::Session(event))?;
        }
        Ok(())
    }

    fn emit(&self, event: Event) -> Result<(), Error> {
        self.events.try_send(event).map_err(|_| Error::EventQueue)
    }
}

fn next_try(attempts: &mut u32, healthy: bool) -> Duration {
    *attempts = if healthy {
        0
    } else {
        attempts.saturating_add(1)
    };
    Duration::from_secs((1 << (*attempts).min(6)).min(60))
}

async fn connect(url: &str, secret: [u8; 32]) -> Result<Socket, Error> {
    // Use an explicit TLS provider so this library works alongside another provider in the box.
    let tls = rustls::ClientConfig::builder_with_provider(
        rustls::crypto::ring::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .expect("ring supports the default TLS versions")
    .with_root_certificates(rustls::RootCertStore::from_iter(
        webpki_roots::TLS_SERVER_ROOTS.iter().cloned(),
    ))
    .with_no_client_auth();
    let max = frames::MAX_RELAY_PAYLOAD + 17;
    let settings = WebSocketConfig::default()
        .max_message_size(Some(max))
        .max_frame_size(Some(max));
    let (mut socket, _) = connect_async_tls_with_config(
        url,
        Some(settings),
        true,
        Some(Connector::Rustls(tls.into())),
    )
    .await
    .map_err(Box::new)?;
    write(&mut socket, relay_hello(&secret)).await?;
    let own = address_of(&secret);
    loop {
        match websocket_message(socket.next().await)? {
            Ws::Binary(bytes) => match frames::read_relay(&bytes) {
                Some(FromRelay::Welcome(address)) if address == own => return Ok(socket),
                Some(_) => return Err(Error::WrongWelcome),
                None => {}
            },
            Ws::Ping(_) => flush(&mut socket).await?,
            _ => {}
        }
    }
}

fn websocket_message(
    next: Option<Result<Ws, tokio_tungstenite::tungstenite::Error>>,
) -> Result<Ws, Error> {
    match next {
        Some(Ok(Ws::Close(frame))) => Err(Error::Closed(
            frame
                .map(|frame| format!(" ({})", frame.code))
                .unwrap_or_default(),
        )),
        Some(Ok(message)) => Ok(message),
        Some(Err(error)) => Err(Error::WebSocket(Box::new(error))),
        None => Err(Error::Closed(String::new())),
    }
}

async fn write(socket: &mut Socket, bytes: Vec<u8>) -> Result<(), Error> {
    time::timeout(WRITE_WITHIN, socket.send(Ws::Binary(bytes.into())))
        .await
        .map_err(|_| Error::WriteTimeout)?
        .map_err(Box::new)?;
    Ok(())
}

async fn flush(socket: &mut Socket) -> Result<(), Error> {
    time::timeout(WRITE_WITHIN, socket.flush())
        .await
        .map_err(|_| Error::WriteTimeout)?
        .map_err(Box::new)?;
    Ok(())
}
