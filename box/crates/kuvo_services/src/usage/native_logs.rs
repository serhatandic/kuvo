//! Incremental native JSONL usage observations. The index owns the checkpoint
//! transaction, so neither a partial read nor a crash advances the file cursor.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::Path,
};

const HEAD: usize = 4096;
// The existing readers skip larger individual records but continue after their newline.
const LONGEST: usize = 64 << 20;
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Claude,
    Codex,
    Grok,
    Antigravity,
}
impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Grok => "grok",
            Self::Antigravity => "antigravity",
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub reasoning: u64,
    pub total: Option<u64>,
    pub fast: bool,
    pub ultrafast: bool,
}
impl Counts {
    pub fn processed(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write_5m)
            .saturating_add(self.cache_write_1h)
    }
    pub fn price_tokens(&self) -> super::pricing::Tokens {
        super::pricing::Tokens {
            input: self.input,
            output: self.output,
            cache_read: self.cache_read,
            cache_write_5m: self.cache_write_5m,
            cache_write_1h: self.cache_write_1h,
            fast: self.fast,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Observation {
    pub at: i64,
    pub session: Option<String>,
    pub model: Option<String>,
    pub pricing_model: Option<String>,
    pub counts: Counts,
    pub billed: Option<f64>,
    /// The maker's stable call id, if it has one. Never transcript words.
    pub call: Option<String>,
    pub message: Option<String>,
    pub request: Option<String>,
    pub sidechain: bool,
    pub has_speed: bool,
}
impl Observation {
    /// Codex copies its parents' token counts to other logs; all identical turns are one.
    pub fn dedup_key(&self, kind: Kind) -> Option<String> {
        let model = self.model.as_deref();
        let value = match kind {
            Kind::Claude => json!([self.message.as_ref()?, self.request]),
            Kind::Grok => json!([self.call.as_ref()?, model]),
            Kind::Antigravity => json!([self.session.as_ref()?, self.call.as_ref()?]),
            Kind::Codex => json!([
                self.at,
                model,
                self.pricing_model,
                self.counts.input.saturating_add(self.counts.cache_read),
                self.counts.cache_read,
                self.counts.output,
                self.counts.reasoning,
                self.counts.total
            ]),
        };
        let key = if kind == Kind::Codex {
            json!([serde_json::to_string(&value).ok()?])
        } else {
            value
        };
        Some(hex::encode(Sha256::digest(serde_json::to_vec(&key).ok()?)))
    }
}
fn text(v: &Value) -> Option<&str> {
    v.as_str().filter(|s| !s.is_empty())
}
fn trim(v: &Value) -> Option<&str> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty())
}
fn number(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str()?.trim().parse().ok())
        .filter(|v| v.is_finite())
}
fn count(v: &Value) -> u64 {
    v.as_f64().filter(|n| n.is_finite()).unwrap_or(0.0).max(0.0) as u64
}
fn timestamp(raw: &str) -> Option<i64> {
    let raw = raw.trim();
    if let Ok(ts) = raw.parse::<jiff::Timestamp>() {
        return Some(ts.as_millisecond());
    }
    let raw = raw.strip_suffix(" UTC").unwrap_or(raw);
    if let Ok(civil) = raw.parse::<jiff::civil::DateTime>() {
        return civil
            .to_zoned(jiff::tz::TimeZone::UTC)
            .ok()
            .map(|z| z.timestamp().as_millisecond());
    }
    None
}
fn time(v: &Value) -> Option<i64> {
    timestamp(v.as_str()?)
}
fn claude_counts(usage: &Value) -> Option<Counts> {
    let input = usage["input_tokens"].as_f64().filter(|v| v.is_finite())?;
    let output = usage["output_tokens"].as_f64().filter(|v| v.is_finite())?;
    let speed = usage["speed"].as_str();
    if speed.is_some_and(|s| !matches!(s, "fast" | "standard")) {
        return None;
    }
    let (cache_write_5m, cache_write_1h) = if usage["cache_creation"].is_object() {
        (
            count(&usage["cache_creation"]["ephemeral_5m_input_tokens"]),
            count(&usage["cache_creation"]["ephemeral_1h_input_tokens"]),
        )
    } else {
        (count(&usage["cache_creation_input_tokens"]), 0)
    };
    Some(Counts {
        input: input.max(0.0) as u64,
        output: output.max(0.0) as u64,
        cache_read: count(&usage["cache_read_input_tokens"]),
        cache_write_5m,
        cache_write_1h,
        fast: speed == Some("fast"),
        ..Default::default()
    })
}
fn claude(value: &Value) -> Vec<Observation> {
    let Some(at) = time(&value["timestamp"]) else {
        return vec![];
    };
    let msg = &value["message"];
    let usage = &msg["usage"];
    for (container, fields) in [
        (
            value,
            &[
                "cwd",
                "costUSD",
                "version",
                "sessionId",
                "requestId",
                "isApiErrorMessage",
            ][..],
        ),
        (msg, &["id", "model"][..]),
        (
            usage,
            &[
                "speed",
                "cache_read_input_tokens",
                "cache_creation_input_tokens",
            ][..],
        ),
    ] {
        if fields
            .iter()
            .any(|f| container.get(*f) == Some(&Value::Null))
        {
            return vec![];
        }
    }
    if [
        &value["sessionId"],
        &value["requestId"],
        &msg["id"],
        &msg["model"],
    ]
    .iter()
    .any(|v| v.as_str() == Some(""))
    {
        return vec![];
    }
    if let Some(version) = value["version"].as_str() {
        let mut parts = version.split('.');
        if !parts
            .next()
            .is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
            || !parts
                .next()
                .is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
            || !parts
                .next()
                .is_some_and(|p| p.as_bytes().first().is_some_and(u8::is_ascii_digit))
        {
            return vec![];
        }
    }
    let Some(counts) = claude_counts(usage) else {
        return vec![];
    };
    let mut parent = Observation {
        at,
        session: text(&value["sessionId"]).map(str::to_owned),
        model: text(&msg["model"])
            .filter(|m| *m != "<synthetic>")
            .map(str::to_owned),
        pricing_model: None,
        counts,
        billed: value["costUSD"].as_f64().filter(|v| v.is_finite()),
        call: None,
        message: text(&msg["id"]).map(str::to_owned),
        request: text(&value["requestId"]).map(str::to_owned),
        sidechain: value["isSidechain"] == true,
        has_speed: usage["speed"].is_string(),
    };
    let mut events = vec![parent.clone()];
    if let Some(items) = usage["iterations"]
        .as_array()
        .filter(|a| a.iter().all(Value::is_object))
    {
        let mut advisor = 0;
        for item in items {
            if item["type"] != "advisor_message" {
                continue;
            }
            let Some(model) = text(&item["model"]) else {
                continue;
            };
            let Some(counts) = claude_counts(item) else {
                continue;
            };
            parent.model = Some(model.into());
            parent.counts = counts;
            parent.message = events[0]
                .message
                .as_ref()
                .map(|id| format!("{id}:advisor:{advisor}"));
            parent.billed = None;
            parent.has_speed = item["speed"].is_string();
            events.push(parent.clone());
            advisor += 1;
        }
    }
    events
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct RawCounts {
    input: u64,
    cached: u64,
    output: u64,
    reasoning: u64,
    total: u64,
}
impl RawCounts {
    fn of(v: &Value) -> Self {
        let n = |keys: &[&str]| {
            keys.iter()
                .find_map(|key| {
                    let v = &v[*key];
                    v.as_f64()
                        .filter(|n| n.is_finite())
                        .map(|n| n.max(0.0) as u64)
                        .or_else(|| v.as_bool().map(u64::from))
                })
                .unwrap_or(0)
        };
        let input = n(&["input_tokens", "prompt_tokens", "input"]);
        let cached = n(&[
            "cached_input_tokens",
            "cache_read_input_tokens",
            "cached_tokens",
        ]);
        let output = n(&["output_tokens", "completion_tokens", "output"]);
        let reasoning = n(&["reasoning_output_tokens", "reasoning_tokens"]);
        let mut total = n(&["total_tokens"]);
        if total == 0 {
            total = input.saturating_add(output).saturating_add(reasoning);
        }
        Self {
            input,
            cached,
            output,
            reasoning,
            total,
        }
    }
    fn difference(self, before: Self) -> Self {
        Self {
            input: self.input.saturating_sub(before.input),
            cached: self.cached.saturating_sub(before.cached),
            output: self.output.saturating_sub(before.output),
            reasoning: self.reasoning.saturating_sub(before.reasoning),
            total: self.total.saturating_sub(before.total),
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
enum Gate {
    Born(i64),
    SelfTimed,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Codex {
    session: Option<String>,
    previous: Option<RawCounts>,
    model: Option<String>,
    fast: bool,
    ultrafast: bool,
    saw_meta: bool,
    gate: Option<Gate>,
}
fn model(payload: &Value) -> Option<&str> {
    trim(&payload["model"])
        .or_else(|| trim(&payload["model_name"]))
        .or_else(|| trim(&payload["metadata"]["model"]))
}
fn given(v: &Value) -> bool {
    !v.is_null() && v.as_str().is_none_or(|s| !s.trim().is_empty())
}
impl Codex {
    fn wants(&self, bytes: &[u8]) -> bool {
        const TYPE: &[u8] = br#""type":""#;
        bytes.windows(TYPE.len()).enumerate().any(|(at, prefix)| {
            if prefix != TYPE {
                return false;
            }
            let name = &bytes[at + TYPE.len()..];
            name.starts_with(b"turn_context\"")
                || name.starts_with(b"token_count\"")
                || name.starts_with(b"thread_settings_applied\"")
                || (!self.saw_meta && name.starts_with(b"session_meta\""))
                || (self.gate.is_some() && name.starts_with(b"task_started\""))
        })
    }

    fn line(&mut self, value: &Value) -> Vec<Observation> {
        let p = &value["payload"];
        match value["type"].as_str() {
            Some("turn_context") => {
                if let Some(model) = model(p) {
                    self.model = Some(model.into());
                }
                return vec![];
            }
            Some("session_meta") if !self.saw_meta => {
                self.saw_meta = true;
                self.session = trim(&p["id"]).map(str::to_owned);
                if given(&p["forked_from_id"])
                    || given(&p["parent_thread_id"])
                    || p["thread_source"] == "subagent"
                    || given(&p["source"]["subagent"])
                {
                    self.gate = Some(
                        time(&value["timestamp"])
                            .map_or(Gate::SelfTimed, |at| Gate::Born(at.div_euclid(1000))),
                    );
                }
                return vec![];
            }
            Some("event_msg") => {}
            _ => return vec![],
        }
        match p["type"].as_str() {
            Some("thread_settings_applied") => {
                if let Some(tier) =
                    trim(&p["thread_settings"]["service_tier"]).or_else(|| trim(&p["service_tier"]))
                {
                    self.fast = matches!(tier, "fast" | "priority");
                    self.ultrafast = tier == "ultrafast";
                }
                return vec![];
            }
            Some("task_started") => {
                if let Some(gate) = self.gate {
                    let started = p["started_at"]
                        .as_f64()
                        .or_else(|| p["started_at"].as_bool().map(|b| u8::from(b) as f64));
                    let until = match gate {
                        Gate::Born(at) => Some(at),
                        Gate::SelfTimed => time(&value["timestamp"]).map(|at| at.div_euclid(1000)),
                    };
                    if started.zip(until).is_some_and(|(a, b)| a >= b as f64) {
                        self.gate = None;
                    }
                }
                return vec![];
            }
            Some("token_count") => {}
            _ => return vec![],
        }
        let Some(at) = time(&value["timestamp"]) else {
            return vec![];
        };
        let info = &p["info"];
        let totals = info["total_token_usage"]
            .is_object()
            .then(|| RawCounts::of(&info["total_token_usage"]));
        if self.gate.is_some() {
            if totals.is_some() {
                self.previous = totals;
            }
            return vec![];
        }
        if totals.is_some() && totals == self.previous {
            return vec![];
        }
        let counts = if info["last_token_usage"].is_object() {
            RawCounts::of(&info["last_token_usage"])
        } else if let Some(total) = totals {
            total.difference(self.previous.unwrap_or_default())
        } else {
            return vec![];
        };
        if totals.is_some() {
            self.previous = totals;
        }
        if counts.input == 0 && counts.cached == 0 && counts.output == 0 && counts.reasoning == 0 {
            return vec![];
        }
        if let Some(name) = model(p).or_else(|| model(info)) {
            self.model = Some(name.into());
        }
        let name = self.model.get_or_insert_with(|| "gpt-5".into()).clone();
        let cached = counts.cached.min(counts.input);
        vec![Observation {
            at,
            session: self.session.clone(),
            pricing_model: (name == "gpt-reserve").then(|| "gpt-5.6-luna".into()),
            model: Some(name),
            counts: Counts {
                input: counts.input - cached,
                cache_read: cached,
                output: counts.output,
                reasoning: counts.reasoning,
                total: Some(counts.total),
                fast: self.fast,
                ultrafast: self.ultrafast,
                ..Default::default()
            },
            billed: None,
            call: None,
            message: None,
            request: None,
            sidechain: false,
            has_speed: false,
        }]
    }
}
fn grok(value: &Value) -> Vec<Observation> {
    let p = &value["params"];
    let update = p
        .get("update")
        .filter(|v| v.is_object())
        .unwrap_or(&value["update"]);
    if update["sessionUpdate"] != "turn_completed" {
        return vec![];
    }
    let usage = &update["usage"];
    let Some(models) = usage["modelUsage"].as_object() else {
        return vec![];
    };
    let meta = p
        .get("_meta")
        .filter(|v| v.is_object())
        .unwrap_or(&value["_meta"]);
    let at = [&p["_meta"], &value["_meta"]]
        .iter()
        .find_map(|m| {
            number(&m["agentTimestampMs"])
                .filter(|t| *t > 0.0)
                .map(|t| t as i64)
        })
        .or_else(|| {
            number(&value["timestamp"])
                .filter(|t| *t > 0.0)
                .map(|t| (t * 1000.0) as i64)
        })
        .or_else(|| time(&value["timestamp"]));
    let Some(at) = at else { return vec![] };
    let bound = |v: &Value| number(v).unwrap_or(0.0).clamp(0.0, 1e12) as u64;
    let mut events = vec![];
    let mut names = models.keys().collect::<Vec<_>>();
    names.sort();
    for name in names {
        let v = &models[name];
        let name = name.trim();
        if name.is_empty() || number(&v["inputTokens"]).is_none_or(|n| n < 0.0) {
            continue;
        }
        let input = bound(&v["inputTokens"]);
        let cache_read = bound(&v["cachedReadTokens"]).min(input);
        let cache_write = bound(&v["cacheCreationTokens"]).min(input - cache_read);
        let billed = number(&v["costUsdTicks"])
            .or_else(|| {
                if models.len() == 1 {
                    number(&usage["costUsdTicks"])
                } else {
                    None
                }
            })
            .filter(|n| *n >= 0.0)
            .map(|n| n / 10_000_000_000.0);
        events.push(Observation {
            at,
            session: text(&p["sessionId"]).map(str::to_owned),
            model: Some(name.into()),
            pricing_model: None,
            counts: Counts {
                input: input - cache_read - cache_write,
                cache_read,
                cache_write_5m: cache_write,
                output: bound(&v["outputTokens"]),
                ..Default::default()
            },
            billed,
            call: trim(&meta["eventId"]).map(str::to_owned),
            message: None,
            request: None,
            sidechain: false,
            has_speed: false,
        });
    }
    events
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Cursor {
    pub size: u64,
    pub modified: Option<u128>,
    pub offset: u64,
    head: String,
    head_length: usize,
    /// A committed suffix anchor distinguishes replacement even when the prefix was copied.
    tail: String,
    tail_length: usize,
    codex: Codex,
    #[serde(default)]
    codex_format: u8,
}
fn hash_part(file: &mut File, start: u64, length: usize) -> io::Result<String> {
    file.seek(SeekFrom::Start(start))?;
    let mut buf = vec![0; length];
    file.read_exact(&mut buf)?;
    Ok(hex::encode(Sha256::digest(buf)))
}
/// Codex writes the app-server's quota snapshot along with token_count events.
fn codex_limits(row: &Value) -> Vec<super::readings::Window> {
    use super::readings::{Source, Window};
    if row["payload"]["type"] != "token_count" {
        return vec![];
    }
    let Some(at) = time(&row["timestamp"]).and_then(|at| u64::try_from(at).ok()) else {
        return vec![];
    };
    let reading = row["payload"]
        .get("rate_limits")
        .or_else(|| row["payload"].get("rateLimits"))
        .unwrap_or(&Value::Null);
    let limit = reading["limit_id"]
        .as_str()
        .or_else(|| reading["limitId"].as_str())
        .filter(|id| !id.is_empty())
        .unwrap_or("codex");
    let model = (limit != "codex").then(|| {
        reading["limit_name"]
            .as_str()
            .or_else(|| reading["limitName"].as_str())
            .unwrap_or(limit)
            .to_owned()
    });
    ["primary", "secondary"]
        .into_iter()
        .filter_map(|id| {
            let w = &reading[id];
            let used = w["used_percent"]
                .as_f64()
                .or_else(|| w["usedPercent"].as_f64())?;
            if !used.is_finite() {
                return None;
            }
            let minutes = w["window_minutes"]
                .as_u64()
                .or_else(|| w["windowDurationMins"].as_u64())
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| *n > 0);
            let resets_at = w["resets_at"]
                .as_u64()
                .or_else(|| w["resetsAt"].as_u64())
                .filter(|n| *n > 0)
                .and_then(|n| {
                    if n < 10_000_000_000 {
                        n.checked_mul(1000)
                    } else {
                        Some(n)
                    }
                });
            Some(Window {
                id: if limit == "codex" {
                    id.into()
                } else {
                    format!("{limit}.{id}")
                },
                used: used / 100.0,
                minutes,
                resets_at,
                model: model.clone(),
                name: None,
                amount: None,
                read: at,
                source: Source::Turn,
            })
        })
        .collect()
}
pub struct Changes {
    pub cursor: Cursor,
    pub rewritten: bool,
    pub events: Vec<(u64, Observation)>,
    pub limits: Vec<super::readings::Window>,
}
/// A file unchanged since the checkpoint incurs no open/read. For an append, only
/// the bounded anchors and complete new lines are read. A failed read leaves the
/// original cursor untouched; the caller commits observations and cursor together.
pub fn read(file: &Path, kind: Kind, previous: &Cursor, since: i64) -> io::Result<Option<Changes>> {
    if kind == Kind::Antigravity {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Antigravity uses its SQLite reader",
        ));
    }
    let meta = std::fs::symlink_metadata(file)?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Ok(None);
    }
    let size = meta.len();
    let modified = meta
        .modified()
        .ok()
        .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|at| at.as_nanos());
    // An older checkpoint admitted noncompact records. Rebuild that file
    // once, then resume its normal append-only cursor.
    let current_format = kind != Kind::Codex || previous.codex_format == 1;
    if current_format && previous.size == size && previous.modified == modified {
        return Ok(None);
    }
    let mut file = File::open(file)?;
    let append = current_format
        && previous.offset <= previous.size
        && size > previous.size
        && previous.head_length as u64 <= size
        && hash_part(&mut file, 0, previous.head_length)? == previous.head
        && previous.tail_length as u64 <= previous.offset
        && hash_part(
            &mut file,
            previous.offset - previous.tail_length as u64,
            previous.tail_length,
        )? == previous.tail;
    let mut cursor = if append {
        previous.clone()
    } else {
        Cursor::default()
    };
    if kind == Kind::Codex {
        cursor.codex_format = 1;
    }
    file.seek(SeekFrom::Start(cursor.offset))?;
    let mut reader = BufReader::with_capacity(1 << 20, &mut file);
    let mut buf = Vec::new();
    let mut skipping = false;
    let mut position = cursor.offset;
    let mut observed = vec![];
    let mut limits = vec![];
    while position < size {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            break;
        }
        let budget = (size - position).min(available.len() as u64) as usize;
        let bytes = &available[..budget];
        let newline = bytes.iter().position(|b| *b == b'\n');
        let n = newline.map_or(bytes.len(), |at| at + 1);
        let piece = &bytes[..n];
        if !skipping {
            if buf.len() + piece.len() > LONGEST {
                buf.clear();
                skipping = true;
            } else {
                buf.extend_from_slice(piece);
            }
        }
        if newline.is_some() {
            if !skipping && !buf.is_empty() {
                // UsageKit only reads Codex's compact record markers; retain
                // a separate match for quota snapshots, which have their own
                // provider-reader contract and need not be compact.
                let codex_usage = kind == Kind::Codex && cursor.codex.wants(&buf);
                let keep = match kind {
                    Kind::Claude => buf.windows(7).any(|w| w == br#""usage""#),
                    Kind::Grok => buf.windows(14).any(|w| w == b"turn_completed"),
                    Kind::Antigravity => false,
                    Kind::Codex => {
                        codex_usage
                            || [b"rate_limits".as_slice(), b"rateLimits"]
                                .iter()
                                .any(|mark| buf.windows(mark.len()).any(|w| w == *mark))
                    }
                };
                if keep {
                    if let Ok(row) = serde_json::from_slice::<Value>(&buf) {
                        if kind == Kind::Codex {
                            limits.extend(codex_limits(&row));
                        }
                        let events = match kind {
                            Kind::Claude => claude(&row),
                            Kind::Grok => grok(&row),
                            Kind::Antigravity => vec![],
                            Kind::Codex if codex_usage => cursor.codex.line(&row),
                            Kind::Codex => vec![],
                        };
                        let at = cursor.offset;
                        observed.extend(
                            events
                                .into_iter()
                                .filter(|e| e.at >= since)
                                .map(|e| (at, e)),
                        );
                    }
                }
            }
            cursor.offset = position + n as u64;
            buf.clear();
            skipping = false;
        }
        position += n as u64;
        reader.consume(n);
    }
    drop(reader);
    cursor.size = size;
    cursor.modified = modified;
    cursor.head_length = HEAD.min(size as usize);
    cursor.head = hash_part(&mut file, 0, cursor.head_length)?;
    cursor.tail_length = HEAD.min(cursor.offset as usize);
    cursor.tail = hash_part(
        &mut file,
        cursor.offset - cursor.tail_length as u64,
        cursor.tail_length,
    )?;
    Ok(Some(Changes {
        cursor,
        rewritten: !append,
        events: observed,
        limits,
    }))
}
