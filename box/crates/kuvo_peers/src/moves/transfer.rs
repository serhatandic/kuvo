//! The bundle and progress journal shared by both residents. A receipt for a
//! part says only that its bytes reached staging. Activation needs separate proof.

use std::{
    collections::{BTreeSet, HashMap},
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::Duration,
};

use kuvo_store::files;
use kuvo_wire::keys::{decode, decode_exact, encode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::time::Instant;

use super::{Journal, Role, move_id, receipt::Bundle};
use crate::reach::Millis;

pub const PART_BYTES: u64 = 30_000;
pub const MAX_BUNDLE_BYTES: u64 = 1_000_000_000;
const IN_FLIGHT: usize = 16;
const PART_AGAIN: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Part {
    #[serde(rename = "i")]
    pub index: u32,
    #[serde(rename = "of")]
    pub count: u32,
    #[serde(rename = "d")]
    pub encoded: String,
}

#[derive(Clone)]
pub struct BundleSize {
    bytes: u64,
    hash: [u8; 32],
}

impl BundleSize {
    pub fn new(bytes: u64, hash: &str) -> io::Result<Self> {
        let hash = decode_exact::<32>(hash)
            .filter(|digest| encode(digest) == hash)
            .ok_or_else(|| invalid("Invalid move bundle hash"))?;
        if !(1..=MAX_BUNDLE_BYTES).contains(&bytes) {
            return Err(invalid("Invalid move bundle size"));
        }
        Ok(Self { bytes, hash })
    }

    pub fn count(&self) -> u32 {
        self.bytes.div_ceil(PART_BYTES) as u32
    }

    pub(super) fn measure(path: &Path) -> io::Result<(u64, String)> {
        let file = File::open(path)?;
        let mut digest = Sha256::new();
        let len = io::copy(&mut file.take(MAX_BUNDLE_BYTES + 1), &mut digest)?;
        if len > MAX_BUNDLE_BYTES {
            return Err(io::ErrorKind::FileTooLarge.into());
        }
        let hash = encode(&digest.finalize());
        Self::new(len, &hash)?;
        Ok((len, hash))
    }

    fn offset_and_size(&self, index: u32) -> io::Result<(u64, usize)> {
        if index >= self.count() {
            return Err(invalid("Invalid move part index"));
        }
        let offset = u64::from(index) * PART_BYTES;
        Ok((offset, (self.bytes - offset).min(PART_BYTES) as usize))
    }
}

/// The peer actor owns open staging journals. It acknowledges a part only
/// for a matching admission, never on behalf of an arrival or a switch.
#[derive(Default)]
pub(crate) struct Receiving {
    moves: HashMap<String, Copying>,
}

struct Copying {
    identity: Bundle,
    receiving: IncomingBundle,
    heard: Millis,
}

impl Receiving {
    pub(crate) fn take(
        &mut self,
        run: &Path,
        peer: &str,
        given: &Value,
        now: Millis,
    ) -> io::Result<Option<kuvo_wire::mail::Envelope>> {
        if given["op"].as_str() != Some("part") {
            return Ok(None);
        }
        let Some(id) = given["id"]
            .as_str()
            .filter(|id| move_id(id).is_some_and(|tail| tail.len() <= 40))
        else {
            return Ok(None);
        };
        let root = run.join("moves");
        let Some(mut record) = Journal::read(run, id)? else {
            return Ok(None);
        };
        if record.role != Role::To
            || record.dropping
            || record.peer != peer
            || !record.bundle.matches(given)
            || record.bundle.hash.as_deref() != given["hash"].as_str()
        {
            return Ok(None);
        }
        let Some(bytes) = record.size else {
            return Ok(None);
        };
        let Some(size) = record
            .bundle
            .hash
            .as_deref()
            .and_then(|hash| BundleSize::new(bytes, hash).ok())
        else {
            return Ok(None);
        };
        let Some(part) = parse_part(given, &size) else {
            return Ok(None);
        };
        if record.complete {
            self.moves.remove(id);
        } else {
            // Quiet transfers need no resident memory. Their got file keeps
            // the same resume point when another part eventually comes.
            self.moves
                .retain(|_, copy| now.saturating_sub(copy.heard) < 60_000);
            if self
                .moves
                .get(id)
                .is_none_or(|copy| copy.identity != record.bundle)
            {
                self.moves.insert(
                    id.into(),
                    Copying {
                        identity: record.bundle.clone(),
                        receiving: IncomingBundle::open(&root.join(id), size)?,
                        heard: now,
                    },
                );
            }
            let copy = self.moves.get_mut(id).expect("opened move staging");
            copy.heard = now;
            copy.receiving.receive(&part)?;
            let count = copy.receiving.got.len() as u64;
            if count.is_multiple_of(32) || copy.receiving.complete() {
                let sent = (count * PART_BYTES).min(bytes);
                if record.sent != Some(sent) {
                    record.sent = Some(sent);
                    record.save(run)?;
                }
            }
        }
        let mut reply = serde_json::to_value(record.bundle)?;
        reply["op"] = json!("got");
        reply["id"] = json!(id);
        reply["i"] = json!(part.index);
        Ok(Some(kuvo_wire::mail::Envelope(serde_json::Map::from_iter(
            [("at".into(), now.into()), ("move".into(), reply)],
        ))))
    }
}

fn parse_part(given: &Value, size: &BundleSize) -> Option<Part> {
    // JSON numbers written as 0.0 or 1e0 are integers to the TS reader.
    let index = given["i"].as_f64()?;
    let count = size.count();
    let encoded = given["d"].as_str()?;
    (index >= 0.0
        && index < f64::from(count)
        && index.fract() == 0.0
        && given["of"].as_f64() == Some(f64::from(count))
        && encoded.len() <= 40_000)
        .then(|| Part {
            index: index as u32,
            count,
            encoded: encoded.into(),
        })
}

pub struct OutgoingBundle {
    file: File,
    size: BundleSize,
}

/// Send at most one window. The caller records a part as sent after the
/// relay has accepted it for sending. All times in this window are monotonic.
pub struct Flight {
    count: u32,
    window: usize,
    got: BTreeSet<u32>,
    flying: HashMap<u32, Instant>,
    first_missing: u32,
}

impl Flight {
    pub fn new(size: &BundleSize) -> Self {
        Self {
            count: size.count(),
            window: IN_FLIGHT,
            got: BTreeSet::new(),
            flying: HashMap::new(),
            first_missing: 0,
        }
    }

    pub fn due(&mut self, now: Instant) -> impl Iterator<Item = u32> + '_ {
        let before = self.flying.len();
        self.flying
            .retain(|_, sent| now.saturating_duration_since(*sent) < PART_AGAIN);
        if self.flying.len() < before {
            self.window = (self.window / 2).max(1);
        }
        (self.first_missing..self.count)
            .filter(|index| !self.got.contains(index) && !self.flying.contains_key(index))
            .take(self.window.saturating_sub(self.flying.len()))
    }

    pub fn sent(&mut self, index: u32, now: Instant) {
        if index < self.count && !self.got.contains(&index) {
            self.flying.insert(index, now);
        }
    }

    pub fn acknowledge(&mut self, index: u32) -> bool {
        if index >= self.count || !self.got.insert(index) {
            return false;
        }
        self.flying.remove(&index);
        self.window = (self.window + 1).min(IN_FLIGHT);
        while self.got.contains(&self.first_missing) {
            self.first_missing += 1;
        }
        true
    }

    pub fn complete(&self) -> bool {
        self.first_missing == self.count
    }

    pub(super) fn acknowledged(&self) -> usize {
        self.got.len()
    }
}

impl OutgoingBundle {
    pub fn open(path: &Path, size: BundleSize) -> io::Result<Self> {
        let file = File::open(path)?;
        if file.metadata()?.len() != size.bytes {
            return Err(invalid("The staged move bundle has the wrong size"));
        }
        Ok(Self { file, size })
    }

    pub fn part(&mut self, index: u32) -> io::Result<Part> {
        let (offset, length) = self.size.offset_and_size(index)?;
        let mut bytes = vec![0; length];
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(&mut bytes)?;
        Ok(Part {
            index,
            count: self.size.count(),
            encoded: encode(&bytes),
        })
    }
}

pub struct IncomingBundle {
    size: BundleSize,
    file: PathBuf,
    journal: PathBuf,
    got: BTreeSet<u32>,
    journal_length: u64,
}

impl IncomingBundle {
    /// Open an already admitted move directory, exclusively owned by the
    /// transfer worker. The worker checks the peer and generation before use.
    pub fn open(directory: &Path, size: BundleSize) -> io::Result<Self> {
        let journal = directory.join("got");
        let file = directory.join("bundle.tgz");
        let bytes = files::read(&journal)?.unwrap_or_default();
        let journal_length = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |last| last + 1);
        let got = bytes[..journal_length]
            .split(|byte| *byte == b'\n')
            .filter_map(|row| std::str::from_utf8(row).ok()?.parse::<u32>().ok())
            .filter(|index| *index < size.count())
            .collect();
        Ok(Self {
            size,
            file,
            journal,
            got,
            journal_length: journal_length as u64,
        })
    }

    pub fn receive(&mut self, part: &Part) -> io::Result<()> {
        let (offset, length) = self.size.offset_and_size(part.index)?;
        if part.count != self.size.count() || part.encoded.len() > 40_000 {
            return Err(invalid("Invalid move part size"));
        }
        let bytes = decode(&part.encoded)
            .filter(|bytes| bytes.len() == length)
            .ok_or_else(|| invalid("Invalid move part bytes"))?;
        if self.complete() {
            return Ok(());
        }
        files::write_staged_range(&self.file, offset, &bytes)?;
        if !self.got.contains(&part.index) {
            let line = format!("{}\n", part.index);
            files::append_staged_line(&self.journal, self.journal_length, line.as_bytes())?;
            self.journal_length += line.len() as u64;
            self.got.insert(part.index);
        }
        Ok(())
    }

    pub fn complete(&self) -> bool {
        self.got.len() == self.size.count() as usize
    }

    /// Return the staged path only after every part and the whole SHA-256
    /// have been verified. The caller keeps the move on hold while unpacking.
    pub fn verify(&self) -> io::Result<&Path> {
        if !self.complete() {
            return Err(invalid("The move bundle is incomplete"));
        }
        let mut file = File::open(&self.file)?;
        let mut digest = Sha256::new();
        if io::copy(&mut (&mut file).take(self.size.bytes + 1), &mut digest)? != self.size.bytes
            || digest.finalize()[..] != self.size.hash[..]
        {
            return Err(invalid("What came wasn’t what was sent."));
        }
        Ok(&self.file)
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
