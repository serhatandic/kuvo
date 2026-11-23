//! Transfer only the selection sealed into the invitation for this applicant.

use kuvo_wire::{
    documents::{self, Envelope, PART_SIZE, Part},
    keys::{Identity, decode, encode},
    noise::{open_one_way, seal_one_way},
};
use serde_json::Value;

use super::hash;
use crate::{Error, seed::source::MAX_BYTES};

const CONSENT: &[u8] = b"kuvo installer credential consent v1\0";
const MAX_PARTS: usize = (MAX_BYTES * 4).div_ceil(3).div_ceil(PART_SIZE) + 1;

pub(crate) const COPY_FAILED: &str = "The saved copy is unavailable. Open the checklist again.";

pub(crate) fn seal(seed: &[u8], key: &[u8; 32], invite: &str) -> Result<String, Error> {
    let (sealed, _) = seal_one_way(
        seed,
        key,
        &Identity::from_bytes(*key, [0; 32]).public(),
        &prologue(invite),
    )
    .map_err(|_| Error::Invalid("credential consent"))?;
    Ok(encode(&sealed))
}

pub(crate) fn open(sealed: &str, key: &[u8; 32], invite: &str) -> Option<Vec<u8>> {
    if sealed.len() > ((MAX_BYTES + 48) * 4).div_ceil(3) {
        return None;
    }
    let (seed, _) = open_one_way(
        &decode(sealed)?,
        key,
        &Identity::from_bytes(*key, [0; 32]).public(),
        &prologue(invite),
    )
    .ok()?;
    Some(seed)
}

fn prologue(invite: &str) -> Vec<u8> {
    [CONSENT, invite.as_bytes()].concat()
}

pub(crate) fn split(bytes: &[u8]) -> Result<Vec<Part>, Error> {
    Ok(documents::split(&Envelope {
        d: encode(bytes),
        s: String::new(),
    })?)
}

#[derive(Default)]
pub(crate) struct Parts {
    whole: String,
    pieces: Vec<Option<String>>,
}

impl Parts {
    pub(crate) fn receive(&mut self, part: Part) -> Option<Value> {
        if !(2..=MAX_PARTS).contains(&part.of)
            || part.i >= part.of
            || part.d.len() > PART_SIZE
            || !part.d.is_ascii()
            || !part.s.is_empty()
        {
            return None;
        }
        if self.whole != part.whole || self.pieces.len() != part.of {
            self.whole = part.whole;
            self.pieces = vec![None; part.of];
        }
        self.pieces[part.i] = Some(part.d);
        let packed: String = self
            .pieces
            .iter()
            .map(Option::as_deref)
            .collect::<Option<_>>()?;
        if hash(&packed) != self.whole {
            return None;
        }
        let decoded = decode(&packed)?;
        if decoded.len() > MAX_BYTES {
            return None;
        }
        serde_json::from_slice(&decoded).ok()
    }
}
