//! What goes over the relay (PROTOCOL.md, "The relay"), the link frames inside it ("Link frames"),
//! and the messages inside a session ("Messages in a session"). Held to `protocol/vectors/link.json`.

use serde_json::Value;

use super::keys::Address;

/// What the relay sends a device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FromRelay {
    Welcome(Address),
    Deliver {
        from: Address,
        payload: Vec<u8>,
    },
    Unreachable(Address),
    Pushed {
        to: Address,
        status: u8,
    },
    /// The device there has a new connection (`replaced`) or none: sessions with it are over.
    Gone {
        address: Address,
        replaced: bool,
    },
}

pub fn relay_hello(secret: &[u8; 32]) -> Vec<u8> {
    [&[1u8][..], secret].concat()
}

pub fn relay_send(to: &Address, payload: &[u8]) -> Vec<u8> {
    [&[2u8][..], to, payload].concat()
}

pub fn relay_push(to: &Address, payload: &[u8]) -> Vec<u8> {
    [&[5u8][..], to, payload].concat()
}

pub const MAX_RELAY_PAYLOAD: usize = 65_600;

/// What the relay sent; none for a kind this client doesn't know (skipped, as the protocol says).
pub fn read_relay(bytes: &[u8]) -> Option<FromRelay> {
    let kind = *bytes.first()?;
    let address: Address = bytes.get(1..17)?.try_into().ok()?;
    match (kind, bytes.len()) {
        (1, 17) => Some(FromRelay::Welcome(address)),
        (2, _) => Some(FromRelay::Deliver {
            from: address,
            payload: bytes[17..].to_vec(),
        }),
        (3, 17) => Some(FromRelay::Unreachable(address)),
        (6, 18) => Some(FromRelay::Pushed {
            to: address,
            status: bytes[17],
        }),
        // A reason it doesn't know yet counts as left: the sessions are over either way.
        (7, 18) => Some(FromRelay::Gone {
            address,
            replaced: bytes[17] == 0,
        }),
        _ => None,
    }
}

/// A link frame's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Handshake1 = 1,
    Handshake2 = 2,
    Transport = 3,
    Close = 4,
    Mail = 5,
}

pub fn frame(kind: Kind, session: u32, body: &[u8]) -> Vec<u8> {
    [&[kind as u8][..], &session.to_be_bytes(), body].concat()
}

pub fn read_frame(bytes: &[u8]) -> Option<(Kind, u32, &[u8])> {
    let kind = match *bytes.first()? {
        1 => Kind::Handshake1,
        2 => Kind::Handshake2,
        3 => Kind::Transport,
        4 => Kind::Close,
        5 => Kind::Mail,
        _ => return None,
    };
    let session = u32::from_be_bytes(bytes.get(1..5)?.try_into().ok()?);
    Some((kind, session, &bytes[5..]))
}

/// The most one transport message's plaintext holds.
pub const MAX_MESSAGE: usize = 65535 - 16;

pub use crate::json::canonical as sorted_json;

/// One session message: its head's length (2 bytes), its head as JSON, then its body.
pub fn encode_message(head: &Value, body: &[u8]) -> Vec<u8> {
    let json = sorted_json(head);
    [
        &(json.len() as u16).to_be_bytes()[..],
        json.as_bytes(),
        body,
    ]
    .concat()
}

pub fn decode_message(bytes: &[u8]) -> Option<(Value, &[u8])> {
    let length = u16::from_be_bytes(bytes.get(..2)?.try_into().ok()?) as usize;
    let head = serde_json::from_slice(bytes.get(2..2 + length)?).ok()?;
    Some((head, &bytes[2 + length..]))
}

/// `head` and `body` as messages that each fit: the first with `head`, the rest as `data`, the
/// last saying whether that's the end. None for a head too long to send.
pub fn pieces(head: &Value, body: &[u8], end: bool) -> Option<Vec<Vec<u8>>> {
    if !head.is_object() || sorted_json(head).len() > 60_000 {
        return None;
    }
    let id = head.get("id").cloned();
    let mut heads: Vec<(Value, &[u8])> = Vec::new();
    let mut current = head.clone();
    let mut offset = 0;
    loop {
        current["end"] = Value::Bool(false);
        let room = MAX_MESSAGE - 2 - sorted_json(&current).len() - 8;
        let upto = body.len().min(offset + room);
        heads.push((current, &body[offset..upto]));
        offset = upto;
        if offset >= body.len() {
            break;
        }
        current = serde_json::json!({ "t": "data" });
        if let Some(id) = &id {
            current["id"] = id.clone();
        }
    }
    if let Some(last) = heads.last_mut() {
        last.0["end"] = Value::Bool(end);
    }
    Some(
        heads
            .iter()
            .map(|(head, body)| encode_message(head, body))
            .collect(),
    )
}
