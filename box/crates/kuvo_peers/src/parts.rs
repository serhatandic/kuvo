//! Bounded multipart snapshots, in the existing peer mail format.

use std::{collections::VecDeque, sync::Arc};

use kuvo_wire::keys::{decode, encode};
use serde::{Deserialize, Serialize};
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
