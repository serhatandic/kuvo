//! Bounded multipart snapshots, in the existing peer mail format.

use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use kuvo_wire::{
    json::ordinary,
    keys::{decode, encode},
    mail::{Envelope, MAX_PLAINTEXT},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const PART_BYTES: usize = 40_000;
const MAX_PARTS: usize = 200;

#[derive(Deserialize, Serialize)]
pub struct Part {
    whole: String,
    i: usize,
    of: usize,
    d: String,
    s: String,
}

#[derive(Default)]
pub struct Parts(VecDeque<Assembly>);

struct Assembly {
    digest: String,
    pieces: Vec<Option<String>>,
    remaining: usize,
}

impl Parts {
    pub fn add(&mut self, part: Part) -> Option<Vec<u8>> {
        if !(2..=MAX_PARTS).contains(&part.of)
            || part.i >= part.of
            || part.d.len() > PART_BYTES
            || decode(&part.whole)?.len() != 32
        {
            return None;
        }
        let before = self
            .0
            .iter()
            .position(|current| current.digest == part.whole);
        let index = match before {
            Some(index) if self.0[index].pieces.len() == part.of => index,
            _ => {
                if let Some(index) = before {
                    self.0.remove(index);
                }
                if self.0.len() == 4 {
                    self.0.pop_front();
                }
                self.0.push_back(Assembly {
                    digest: part.whole,
                    pieces: vec![None; part.of],
                    remaining: part.of,
                });
                self.0.len() - 1
            }
        };
        let assembly = &mut self.0[index];
        if assembly.pieces[part.i].replace(part.d).is_none() {
            assembly.remaining -= 1;
        }
        if assembly.remaining != 0 {
            return None;
        }
        let assembly = self.0.remove(index)?;
        let combined: String = assembly.pieces.into_iter().flatten().collect();
        (encode(&Sha256::digest(&combined)) == assembly.digest)
            .then(|| decode(&combined))
            .flatten()
    }
}

pub fn split(bytes: &[u8]) -> Option<Arc<[Part]>> {
    let encoded = encode(bytes);
    let of = encoded.len().div_ceil(PART_BYTES);
    if !(2..=MAX_PARTS).contains(&of) {
        return None;
    }
    let whole = encode(&Sha256::digest(&encoded));
    Some(
        encoded
            .as_bytes()
            .chunks(PART_BYTES)
            .enumerate()
            .map(|(i, d)| Part {
                whole: whole.clone(),
                i,
                of,
                d: String::from_utf8_lossy(d).into_owned(),
                s: String::new(),
            })
            .collect(),
    )
}

/// Work Queue mail uses the same part spelling and digest as documents. The
/// whole is the compact JSON before the enclosing mail's version and clock.
pub fn work_queue_mails(payload: Value, at: i64) -> Option<Vec<Envelope>> {
    let mail = |work_queue| {
        Envelope(
            [("workQueue".into(), work_queue), ("at".into(), at.into())]
                .into_iter()
                .collect(),
        )
    };
    let one = mail(payload);
    if one.encode().len() <= MAX_PLAINTEXT {
        return Some(vec![one]);
    }
    let payload = ordinary(&one.0["workQueue"]);
    let parts = split(payload.as_bytes())?;
    Some(
        parts
            .iter()
            .map(|part| mail(json!({"part": part})))
            .collect(),
    )
}

#[derive(Default)]
pub(crate) struct WorkQueueParts(HashMap<([u8; 32], bool), Parts>);

impl WorkQueueParts {
    /// Assemble inside the relay task, before publishing to lossy subscribers.
    /// Memory-only reads and tracked mail have independent part sets.
    pub(crate) fn receive(&mut self, sender: [u8; 32], mail: &mut Envelope) -> bool {
        let Some(queue) = mail.0.get_mut("workQueue") else {
            return true;
        };
        let Some(part) = queue.get("part").filter(|part| part.is_object()) else {
            return true;
        };
        let memory = queue.get("memory").and_then(Value::as_bool) == Some(true);
        let mut first = true;
        let complete = serde_json::from_value::<Part>(part.clone())
            .ok()
            .and_then(|part| {
                let parts = self.0.entry((sender, memory)).or_default();
                first = !parts.0.iter().any(|pending| pending.digest == part.whole);
                parts.add(part)
            });
        let Some(Value::Object(mut whole)) = complete
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .filter(|whole: &Value| !whole.get("part").is_some_and(Value::is_object))
        else {
            if memory {
                return false;
            }
            mail.0.shift_remove("workQueue");
            // A partial delivery still answers reach, as one ordinary mail does.
            return first || mail.0.keys().any(|field| field != "at");
        };
        if memory {
            whole.insert("memory".into(), true.into());
        }
        *queue = Value::Object(whole);
        true
    }

    pub(crate) fn retain(&mut self, mut member: impl FnMut(&[u8; 32]) -> bool) {
        self.0.retain(|(sender, _), _| member(sender));
    }
}
