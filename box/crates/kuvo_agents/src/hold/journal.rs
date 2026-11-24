use super::protocol::{HoldKind, Output};
use serde_json::Value;
use std::{
    collections::{BTreeMap, VecDeque},
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

struct Entry {
    offset: u64,
    bytes: usize,
    live_only: bool,
}

struct Ask {
    id: Value,
    method: String,
    line: String,
}

pub(super) struct Journal {
    path: PathBuf,
    file: File,
    entries: VecDeque<Entry>,
    pending: BTreeMap<u64, Ask>,
    pub seq: u64,
    bytes: u64,
    limit: u64,
    pub pinned: bool,
    kind: HoldKind,
}

impl Journal {
    pub fn open(path: PathBuf, kind: HoldKind, limit: u64) -> io::Result<Self> {
        kuvo_store::files::write_new(&path, b"")?;
        let file = File::options().read(true).write(true).open(&path)?;
        Ok(Self {
            path,
            file,
            entries: VecDeque::new(),
            pending: BTreeMap::new(),
            seq: 0,
            bytes: 0,
            limit,
            pinned: false,
            kind,
        })
    }

    pub fn first(&self) -> u64 {
        self.seq + 1 - self.entries.len() as u64
    }

    pub fn range_available(&self, from: u64) -> bool {
        from.saturating_add(1) >= self.first()
    }

    pub fn paused(&self) -> bool {
        self.pinned && self.bytes > self.limit
    }

    pub fn append(&mut self, line: &str) -> io::Result<()> {
        self.seq += 1;
        let live_only = match self.kind {
            HoldKind::JsonRpc => {
                self.note_ask(line);
                false
            }
            HoldKind::ControlJson => {
                line.contains("\"control_request\"")
                    && serde_json::from_str::<Value>(line)
                        .is_ok_and(|message| message["type"] == "control_request")
            }
            HoldKind::Raw => false,
        };
        self.file.seek(SeekFrom::Start(self.bytes))?;
        self.file.write_all(line.as_bytes())?;
        self.file.write_all(b"\n")?;
        let bytes = line.len() + 1;
        self.entries.push_back(Entry {
            offset: self.bytes,
            bytes,
            live_only,
        });
        self.bytes += bytes as u64;
        if !self.pinned && self.bytes > self.limit {
            self.prune()?;
        }
        Ok(())
    }

    pub fn unpin(&mut self) -> io::Result<()> {
        self.pinned = false;
        if self.bytes > self.limit {
            self.prune()?;
        }
        Ok(())
    }

    fn prune(&mut self) -> io::Result<()> {
        let mut newest = self.entries.iter().rev();
        let Some(last) = newest.next() else {
            return Ok(());
        };
        let mut remaining = last.bytes as u64;
        let mut keep = 1;
        for entry in newest {
            if remaining + entry.bytes as u64 > self.limit / 2 {
                break;
            }
            remaining += entry.bytes as u64;
            keep += 1;
        }
        let start = self.bytes - remaining;
        if start == 0 {
            return Ok(());
        }
        self.file.seek(SeekFrom::Start(start))?;
        replace_from(&self.path, (&mut self.file).take(remaining))?;
        self.file = File::options().read(true).write(true).open(&self.path)?;
        self.entries.drain(..self.entries.len() - keep);
        for entry in &mut self.entries {
            entry.offset -= start;
        }
        self.bytes = remaining;
        Ok(())
    }

    pub fn replay(&mut self, seq: u64, through: u64) -> io::Result<Option<Output>> {
        let Some(entry) = seq
            .checked_sub(self.first())
            .and_then(|index| self.entries.get(index as usize))
        else {
            return Err(super::invalid(
                "Holder journal no longer covers the replay cursor",
            ));
        };
        if entry.live_only && seq <= through {
            return Ok(None);
        }
        self.file.seek(SeekFrom::Start(entry.offset))?;
        let mut bytes = vec![0; entry.bytes - 1];
        self.file.read_exact(&mut bytes)?;
        let line = String::from_utf8(bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        Ok(Some(Output::Line {
            sequence: seq,
            line,
            again: false,
        }))
    }

    pub fn open_asks(&self, through: u64) -> impl Iterator<Item = u64> {
        self.pending.range(..=through).map(|(&seq, _)| seq)
    }

    pub fn ask(&self, sequence: u64) -> Option<Output> {
        let ask = self.pending.get(&sequence)?;
        Some(Output::Line {
            sequence,
            line: ask.line.clone(),
            again: true,
        })
    }

    pub fn asks_are_portable(&self, methods: &[String]) -> bool {
        self.pending
            .values()
            .all(|ask| methods.contains(&ask.method))
    }

    fn note_ask(&mut self, line: &str) {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            return;
        };
        let Some(method) = message["method"].as_str() else {
            return;
        };
        if !message["id"].is_null() {
            if let Some(previous) = self.find_id(&message["id"]) {
                self.settle(previous);
            }
            self.pending.insert(
                self.seq,
                Ask {
                    id: message["id"].clone(),
                    method: method.into(),
                    line: line.into(),
                },
            );
            if self.pending.len() > 1000 {
                self.pending.pop_first();
            }
        } else if matches!(method, "serverRequest/resolved" | "$/cancelRequest") {
            let params = &message["params"];
            let id = params
                .get("requestId")
                .filter(|id| !id.is_null())
                .unwrap_or(&params["id"]);
            let matches = self
                .pending
                .iter()
                .filter_map(|(&seq, ask)| match_id(&ask.id, id).then_some(seq))
                .collect::<Vec<_>>();
            for seq in matches {
                self.settle(seq);
            }
        }
    }

    pub fn acknowledge_answer(&mut self, line: &str) {
        if self.kind == HoldKind::JsonRpc
            && let Ok(message) = serde_json::from_str::<Value>(line)
            && message.get("method").is_none()
            && let Some(seq) = self.find_id(&message["id"])
        {
            self.settle(seq);
        }
    }

    fn find_id(&self, id: &Value) -> Option<u64> {
        self.pending
            .iter()
            .find_map(|(&seq, ask)| (&ask.id == id).then_some(seq))
    }

    fn settle(&mut self, seq: u64) {
        self.pending.remove(&seq);
        if let Some(entry) = seq
            .checked_sub(self.first())
            .and_then(|index| self.entries.get_mut(index as usize))
        {
            entry.live_only = true;
        }
    }
}

fn match_id(a: &Value, b: &Value) -> bool {
    if a == b {
        return true;
    }
    match (a, b) {
        (Value::String(s), Value::Number(n)) | (Value::Number(n), Value::String(s)) => {
            n.to_string() == *s
        }
        _ => false,
    }
}

fn replace_from(path: &Path, mut from: impl Read) -> io::Result<()> {
    kuvo_store::files::write_generated(path, |file| {
        io::copy(&mut from, file)?;
        Ok(())
    })
}
