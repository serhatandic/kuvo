//! Paired sessions, addressed by relay sender and handshake number. The generation distinguishes
//! successive handshakes for the same address and number at the Rust server boundary.

use std::collections::HashMap;

use kuvo_wire::{
    frames::{self, Kind},
    keys::{Address, Host},
    noise::{self, Initiator, Responder, Transport},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Id {
    pub address: Address,
    pub number: u32,
    pub(crate) generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientHello {
    pub v: u8,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub app: String,
    #[serde(default)]
    pub client: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pair: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComputerKind {
    Mac,
    Machine,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoxHello {
    pub v: u8,
    pub kind: ComputerKind,
    pub name: String,
    pub app: String,
    pub engine: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Device {
    pub key: [u8; 32],
    pub hello: ClientHello,
    pub grant: String,
}

/// A message whose JSON head has been checked to be an object. The dispatcher handles its
/// request IDs, routes, streaming bodies, and WebSocket upgrades.
#[derive(Debug, Clone)]
pub struct Message {
    pub head: Value,
    pub body: Vec<u8>,
}

#[derive(Debug)]
pub enum Event {
    Accepted(Id, Device),
    Confirmed(Id),
    Opened(Id, BoxHello),
    Reopening { before: Id, after: Id },
    Message(Id, Message),
    Ended(Id),
}

pub(crate) struct Output {
    pub frame: Option<Vec<u8>>,
    pub events: Vec<Event>,
}

impl Output {
    fn empty() -> Self {
        Self {
            frame: None,
            events: Vec::new(),
        }
    }

    fn frame(kind: Kind, number: u32, body: &[u8]) -> Self {
        Self {
            frame: Some(frames::frame(kind, number, body)),
            ..Self::empty()
        }
    }
}

enum Phase {
    Opening {
        key: [u8; 32],
        hello: ClientHello,
        ik: Initiator,
    },
    Serving {
        transport: Transport,
        device: Device,
        confirmed: bool,
    },
    Open(Transport),
}

struct Session {
    id: Id,
    phase: Phase,
}

#[derive(Default)]
pub(crate) struct Sessions {
    by_address: HashMap<Address, Session>,
    generation: u64,
}

impl Sessions {
    fn id(&mut self, address: Address, number: u32) -> Id {
        self.generation += 1;
        Id {
            address,
            number,
            generation: self.generation,
        }
    }

    pub(crate) fn initiate(
        &mut self,
        local: [u8; 32],
        host: Host,
        hello: ClientHello,
    ) -> Result<(Id, Output), noise::Failed> {
        let mut random = [0; 4];
        getrandom::fill(&mut random).map_err(|_| noise::Failed)?;
        let mut number = u32::from_be_bytes(random);
        if self
            .by_address
            .get(&host.address)
            .is_some_and(|previous| previous.id.number == number)
        {
            number = number.wrapping_add(1);
        }
        let id = self.id(host.address, number);
        let mut ik = Initiator::new(local, host.key, noise::PROLOGUE);
        let message =
            ik.write_message_1(&serde_json::to_vec(&hello).expect("client hello serializes"))?;
        let mut output = Output::frame(Kind::Handshake1, id.number, &message);
        if let Some(old) = self.by_address.insert(
            host.address,
            Session {
                id,
                phase: Phase::Opening {
                    key: host.key,
                    hello,
                    ik,
                },
            },
        ) {
            output.events.push(Event::Ended(old.id));
        }
        Ok((id, output))
    }

    /// A replacement may race an outgoing handshake. Start again with fresh Noise and a new number.
    pub(crate) fn gone(&mut self, local: [u8; 32], address: Address) -> Output {
        if let Some(Session {
            id: before,
            phase: Phase::Opening { key, hello, .. },
        }) = self.by_address.get(&address)
        {
            let before = *before;
            if let Ok((after, mut output)) =
                self.initiate(local, Host { key: *key, address }, hello.clone())
            {
                output.events.clear();
                output.events.push(Event::Reopening { before, after });
                return output;
            }
        }
        Output {
            frame: None,
            events: self.forget(address).into_iter().collect(),
        }
    }

    pub(crate) fn reset(&mut self) -> impl Iterator<Item = Event> {
        self.by_address
            .drain()
            .map(|(_, session)| Event::Ended(session.id))
    }

    pub(crate) fn forget(&mut self, address: Address) -> Option<Event> {
        self.by_address
            .remove(&address)
            .map(|session| Event::Ended(session.id))
    }

    pub(crate) fn close(&mut self, id: Id) -> Option<Output> {
        if self.by_address.get(&id.address)?.id != id {
            return None;
        }
        let mut output = Output::frame(Kind::Close, id.number, &[]);
        output.events.extend(self.forget(id.address));
        Some(output)
    }

    pub(crate) fn revoke(&self, grants: &HashMap<[u8; 32], String>) -> Vec<Id> {
        self.by_address
            .values()
            .filter_map(|session| match &session.phase {
                Phase::Serving { device, .. } if grants.get(&device.key) != Some(&device.grant) => {
                    Some(session.id)
                }
                _ => None,
            })
            .collect()
    }

    pub(crate) fn send(&mut self, id: Id, message: &[u8]) -> Option<Output> {
        let session = self.by_address.get_mut(&id.address)?;
        if session.id != id {
            return None;
        }
        let transport = match &mut session.phase {
            Phase::Opening { .. } => return None,
            Phase::Open(transport) | Phase::Serving { transport, .. } => transport,
        };
        Some(Output::frame(
            Kind::Transport,
            id.number,
            &transport.seal(message),
        ))
    }

    pub(crate) fn receive(
        &mut self,
        local: [u8; 32],
        address: Address,
        frame: (Kind, u32, &[u8]),
        hello: &BoxHello,
        grants: &HashMap<[u8; 32], String>,
    ) -> Output {
        let (kind, number, bytes) = frame;
        if kind == Kind::Handshake1 {
            return self.answer(local, address, number, bytes, hello, grants);
        }
        let Some(session) = self
            .by_address
            .get_mut(&address)
            .filter(|session| session.id.number == number)
        else {
            return if kind == Kind::Transport {
                Output::frame(Kind::Close, number, &[])
            } else {
                Output::empty()
            };
        };
        let id = session.id;
        match kind {
            Kind::Handshake2 if matches!(session.phase, Phase::Opening { .. }) => {
                self.finish(address, bytes)
            }
            Kind::Transport => self.transport(id, bytes),
            Kind::Close => Output {
                frame: None,
                events: self.forget(address).into_iter().collect(),
            },
            _ => Output::empty(),
        }
    }

    fn answer(
        &mut self,
        local: [u8; 32],
        address: Address,
        number: u32,
        bytes: &[u8],
        hello: &BoxHello,
        grants: &HashMap<[u8; 32], String>,
    ) -> Output {
        let refuse = || Output::frame(Kind::Close, number, &[]);
        let mut ik = Responder::new(local, noise::PROLOGUE);
        let Ok(payload) = ik.read_message_1(bytes) else {
            return refuse();
        };
        let Ok(client) = serde_json::from_slice::<ClientHello>(&payload) else {
            return refuse();
        };
        let Some(grant) = grants.get(&ik.rs).filter(|_| client.v == 1) else {
            return refuse();
        };
        let device = Device {
            key: ik.rs,
            hello: client,
            grant: grant.clone(),
        };
        let Ok((reply, transport)) =
            ik.write_message_2(&serde_json::to_vec(hello).expect("box hello serializes"))
        else {
            return refuse();
        };
        let id = self.id(address, number);
        let mut output = Output::frame(Kind::Handshake2, number, &reply);
        output.events.extend(self.forget(address));
        self.by_address.insert(
            address,
            Session {
                id,
                phase: Phase::Serving {
                    transport,
                    device: device.clone(),
                    confirmed: false,
                },
            },
        );
        output.events.push(Event::Accepted(id, device));
        output
    }

    fn finish(&mut self, address: Address, bytes: &[u8]) -> Output {
        let Some(Session {
            id,
            phase: Phase::Opening { ik, .. },
        }) = self.by_address.remove(&address)
        else {
            return Output::empty();
        };
        let opened = ik
            .read_message_2(bytes)
            .ok()
            .and_then(|(plain, transport)| {
                let hello: BoxHello = serde_json::from_slice(&plain).ok()?;
                (hello.v == 1).then_some((hello, transport))
            });
        match opened {
            Some((hello, transport)) => {
                self.by_address.insert(
                    address,
                    Session {
                        id,
                        phase: Phase::Open(transport),
                    },
                );
                Output {
                    frame: None,
                    events: vec![Event::Opened(id, hello)],
                }
            }
            None => {
                let mut output = Output::frame(Kind::Close, id.number, &[]);
                output.events.push(Event::Ended(id));
                output
            }
        }
    }

    fn transport(&mut self, id: Id, bytes: &[u8]) -> Output {
        let session = self
            .by_address
            .get_mut(&id.address)
            .expect("matching session");
        let mut output = Output::empty();
        let (transport, confirmed) = match &mut session.phase {
            Phase::Opening { .. } => return self.close(id).expect("matching session"),
            Phase::Open(transport) => (transport, None),
            Phase::Serving {
                transport,
                confirmed,
                ..
            } => (transport, Some(confirmed)),
        };
        let Ok(plain) = transport.open(bytes) else {
            return self.close(id).expect("matching session");
        };
        let Some((head, body)) = frames::decode_message(&plain)
            .filter(|(head, _)| head.is_object() && plain.len() <= frames::MAX_MESSAGE)
        else {
            return self.close(id).expect("matching session");
        };
        if let Some(confirmed) = confirmed
            && !*confirmed
        {
            *confirmed = true;
            output.events.push(Event::Confirmed(id));
        }
        if head["t"] == "ping" {
            let pong = frames::encode_message(&json!({ "t": "pong" }), &[]);
            output.frame = Some(frames::frame(
                Kind::Transport,
                id.number,
                &transport.seal(&pong),
            ));
        } else {
            output.events.push(Event::Message(
                id,
                Message {
                    head,
                    body: body.to_vec(),
                },
            ));
        }
        output
    }
}
