//! A session's latest turn as its starter hears of it: past the summaries you asked for (which end
//! turns of their own, with nothing done to tell of), what began it, what it said, and whether
//! that's news to whoever started the session. Conversations here are newest first, as the
//! engine's `order=desc` pages give them.

use std::time::Duration;

use http::Method;
use kuvo_engine::BotEngine;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::told::SessionTold;

const WAIT: Duration = Duration::from_secs(10);
/// What a turn nobody asked for must say, once a session's finish was told, to be told too (its
/// length in characters): a timer's "nothing new yet" isn't news.
pub const SUBSTANCE: usize = 200;

/// What's written between turns: notes, a model or mode picked, a command you ran.
fn between(row: &Value) -> bool {
    let kind = row["type"].as_str().unwrap_or_default();
    [
        "synthetic",
        "system",
        "model-switched",
        "agent-switched",
        "shell",
    ]
    .contains(&kind)
}

fn asked_summary(row: &Value) -> bool {
    row["type"] == "compaction" && row["reason"] == "manual"
}

/// Whether the summary at `index` ran on its own: nothing before it was still going.
fn on_its_own(rows: &[Value], index: usize) -> bool {
    let earlier = rows[index + 1..].iter().find(|row| !between(row));
    earlier.is_none_or(|row| row["type"] == "idle")
}

/// The summary a session did last, if that's all it did since its last turn.
pub fn summary_row_at_end(rows: &[Value]) -> Option<&Value> {
    let mut at = 0;
    // Past what's been written since (a message waiting its turn, a note), and its end.
    while rows
        .get(at)
        .is_some_and(|row| row["type"] == "user" || between(row))
    {
        at += 1;
    }
    while rows.get(at).is_some_and(|row| row["type"] == "idle") {
        at += 1;
    }
    while rows.get(at).is_some_and(between) {
        at += 1;
    }
    let row = rows.get(at)?;
    (asked_summary(row) && row["status"].is_string() && on_its_own(rows, at)).then_some(row)
}

/// A conversation without the summaries that ran on their own, nor their ends: its turns of work.
pub fn past_summaries(rows: &[Value]) -> Vec<Value> {
    let mut work: Vec<Value> = Vec::new();
    for (at, row) in rows.iter().enumerate() {
        if !asked_summary(row) || !on_its_own(rows, at) {
            work.push(row.clone());
            continue;
        }
        // Its end is the newest `idle` just above it, past any notes.
        let end = work.iter().rposition(|row| !between(row));
        if let Some(end) = end.filter(|end| work[*end]["type"] == "idle") {
            work.remove(end);
        }
    }
    work
}

/// How a session's work went: its own word, unless a summary came after that work, whose run
/// changed it.
pub fn work_outcome(rows: &[Value], outcome: &Value) -> Value {
    let work = past_summaries(rows);
    if work.len() == rows.len() {
        return outcome.clone();
    }
    let end = work.iter().find(|row| row["type"] == "idle");
    end.map(|row| row["outcome"].clone())
        .filter(|said| !said.is_null())
        .unwrap_or(outcome.clone())
}

/// What's part of a turn: what began it, and what its agent said.
pub fn of_turn(message: &Value) -> bool {
    ["user", "synthetic", "assistant"].contains(&message["type"].as_str().unwrap_or_default())
}

/// Where a session's latest turn lies in its messages (read as it runs too, not ended yet): from
/// `from` up to, not including, `before` (none for to the end of what was read).
pub fn turn_in(messages: &[Value]) -> (usize, Option<usize>) {
    // (Commands you ran after it, above where it ended, are no part of it.)
    let last_end = messages
        .iter()
        .position(|message| message["type"] == "idle");
    let from = match last_end {
        Some(end) if !messages[..end].iter().any(of_turn) => end + 1,
        _ => 0,
    };
    let before = messages
        .iter()
        .skip(from)
        .position(|message| message["type"] == "idle");
    (from, before.map(|at| at + from))
}

/// Where a session's last turn of work ended.
pub fn last_turn_end(rows: &[Value]) -> Option<Value> {
    past_summaries(rows)
        .into_iter()
        .find(|row| row["type"] == "idle")
}

/// Messages the box writes of its own accord, or passes on: none of them written by you.
const NOT_YOURS: &[&str] = &[
    "[Message from ",
    "[Wake-up]",
    "[Work from ",
    "[Session ",
    "[Goal started]",
    "[Goal changed]",
    "[Goal's end]",
    "[Goal resumed]",
    "[Goal] Nothing of the goal is at work now.",
    "[Not now]",
    "[Limit reset]",
    "[The user answered]",
];
// What the box passes on from someone: a letter or work from an agent, or what you answered,
// approved or picked.
const FROM_SOMEONE: &[&str] = &[
    "[Message from ",
    "[Work from ",
    "[The user ",
    "[Decision changed]",
];

pub(crate) fn written_by_you(message: &Value) -> bool {
    let text = message["text"].as_str().unwrap_or_default();
    !message["metadata"]["kuvo"]["note"].is_object()
        && !NOT_YOURS.iter().any(|start| text.starts_with(start))
}

fn boxs_own_word(message: &Value) -> bool {
    let text = message["text"].as_str().unwrap_or_default();
    !written_by_you(message) && !FROM_SOMEONE.iter().any(|start| text.starts_with(start))
}

/// What began a turn: someone's message (`You` when every one in it is the user's own, so its
/// answer is theirs, not a report), what the session left running ending or its agent carrying on
/// by itself (`Left`), the box's own word, its own wake-up, or another message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeganBy {
    Asked,
    You,
    Left,
    Word,
    Timer,
    Other,
}

#[derive(Clone, Debug)]
pub struct Began {
    pub kind: BeganBy,
    pub at: Option<u64>,
    /// Where in its run it was written (`time.order`), where the engine says.
    pub order: Option<super::background::WorkOrder>,
}

fn order_of(message: &Value) -> Option<super::background::WorkOrder> {
    let order = &message["time"]["order"];
    Some((order[0].as_str()?.to_owned(), order[1].as_u64()?))
}

fn time_of(message: Option<&Value>) -> Option<u64> {
    message
        .and_then(|message| message["time"]["created"].as_f64())
        .map(|at| at as u64)
}

// From a turn's messages, newest first; a turn with no starter in reach counts as asked for.
fn began_by(turn: &[Value]) -> Began {
    let starter = |message: &&Value| message["type"] == "user" || message["type"] == "synthetic";
    let starters: Vec<&Value> = turn.iter().filter(starter).collect();
    let askers: Vec<&&Value> = starters
        .iter()
        .filter(|message| message["type"] == "user" && !boxs_own_word(message))
        .collect();
    if let Some(asked) = askers.first() {
        let yours = askers.iter().all(|message| written_by_you(message));
        let kind = if yours { BeganBy::You } else { BeganBy::Asked };
        let (at, order) = (time_of(Some(asked)), order_of(asked));
        return Began { kind, at, order };
    }
    let Some(first) = starters.last() else {
        return Began {
            kind: BeganBy::Asked,
            at: None,
            order: None,
        };
    };
    let at = time_of(Some(first));
    let kind = if first["type"] == "user" {
        let timer = first["text"]
            .as_str()
            .is_some_and(|text| text.starts_with(kuvo_engine::WAKE_MARKER));
        if timer { BeganBy::Timer } else { BeganBy::Word }
    } else {
        let source = first["metadata"]["source"].as_str().unwrap_or_default();
        if ["shell", "subagent", "agent"].contains(&source) {
            BeganBy::Left
        } else {
            BeganBy::Other
        }
    };
    Began {
        kind,
        at,
        order: None,
    }
}

/// A session's last reply (its id, and when it was written), the end of a turn started after it
/// that wrote none (a turn that failed before it replied), what its latest turn said and what
/// began it, and whether something's written since its last turn ended.
#[derive(Clone, Debug)]
pub struct LastReply {
    pub id: Option<String>,
    pub at: Option<u64>,
    pub turn: Option<String>,
    pub turn_at: Option<u64>,
    pub text: String,
    pub began: Began,
    pub under_way: bool,
    pub whole: Vec<Value>,
}

pub(crate) async fn page(engine: &BotEngine, session: &str, query: &str) -> Option<Value> {
    let session =
        percent_encoding::utf8_percent_encode(session, percent_encoding::NON_ALPHANUMERIC);
    let path = format!("api/session/{session}/message?{query}");
    let read = engine.call_whole(Method::GET, &path, WAIT).await.ok()?;
    Some(read)
}

/// A session's newest messages (at most `limit`), or none when they can't be read.
pub async fn newest(engine: &BotEngine, session: &str, limit: usize) -> Option<Vec<Value>> {
    let read = page(engine, session, &format!("limit={limit}&order=desc")).await?;
    read["data"].as_array().or(read.as_array()).cloned()
}

/// A session's messages, newest first, back as far as its latest turn begins (a long one is many
/// steps, each a message); none when the first page can't be read.
async fn latest_messages(engine: &BotEngine, session: &str) -> Option<Vec<Value>> {
    let mut messages = Vec::new();
    let mut query = "limit=50&order=desc".to_owned();
    for pages in 0..20 {
        let Some(read) = page(engine, session, &query).await else {
            if pages == 0 {
                return None;
            }
            break;
        };
        let data = read["data"]
            .as_array()
            .or(read.as_array())
            .cloned()
            .unwrap_or_default();
        messages.extend(
            data.iter()
                .filter(|message| message["id"].is_string())
                .cloned(),
        );
        let cursor = &read["cursor"];
        let next = cursor["next"].as_str().or(cursor["previous"].as_str());
        if data.is_empty() || turn_in(&past_summaries(&messages)).1.is_some() {
            break;
        }
        let Some(next) = next else {
            break;
        };
        let next = percent_encoding::utf8_percent_encode(next, percent_encoding::NON_ALPHANUMERIC);
        query = format!("limit=100&cursor={next}");
    }
    Some(messages)
}

fn said_in(message: &Value, separator: &str) -> String {
    let parts = message["content"].as_array().into_iter().flatten();
    let texts = parts.filter(|part| part["type"] == "text");
    texts
        .map(|part| part["text"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join(separator)
}

/// A session's last reply and latest turn, or none when it can't be read.
pub async fn last_reply(engine: &BotEngine, session: &str) -> Option<LastReply> {
    // (A summary asked for is no turn of its work: this is about what it did.)
    let whole = latest_messages(engine, session).await?;
    let messages = past_summaries(&whole);
    let reply = messages
        .iter()
        .find(|message| message["type"] == "assistant");
    // Its latest turn, newest first: back to where the one before it ended.
    let (from, before) = turn_in(&messages);
    let turn = &messages[from..before.unwrap_or(messages.len())];
    let silent =
        from > 0 && !turn.is_empty() && !turn.iter().any(|message| message["type"] == "assistant");
    let ended = silent.then(|| &messages[from - 1]);
    // What it said in it, the latest first (a step of tools alone says nothing).
    let assistant = turn.iter().filter(|message| message["type"] == "assistant");
    let said = assistant.map(|message| {
        said_in(message, " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    });
    let text = said
        .into_iter()
        .find(|text| !text.is_empty())
        .unwrap_or_default();
    let id = |message: Option<&Value>| {
        message.and_then(|message| message["id"].as_str().map(str::to_owned))
    };
    Some(LastReply {
        id: id(reply),
        at: time_of(reply),
        turn: id(ended),
        turn_at: time_of(ended),
        text,
        began: began_by(turn),
        under_way: from == 0 && turn.iter().any(of_turn),
        whole,
    })
}

// The marks a line of Markdown starts with: a heading, a quote, a list's.
static BLOCK_MARKS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"^(?:\s*(?:#{1,6}(?:\s|$)|>|[-*+]\s|\d{1,9}[.)]\s))+\s*").expect("a pattern")
});

/// A reply as one line: without Markdown's block marks, code fences or rules.
pub fn reply_line(text: &str) -> String {
    let fence = |line: &str| {
        let line = line.trim_start();
        line.starts_with("```") || line.starts_with("~~~")
    };
    let rule =
        |line: &str| !line.trim().is_empty() && line.chars().all(|ch| "-*_= \t".contains(ch));
    let lines = text
        .lines()
        .map(|line| BLOCK_MARKS.replace(line, "").into_owned());
    let kept: Vec<String> = lines.filter(|line| !fence(line) && !rule(line)).collect();
    kept.join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Text cut to `count` characters, with an ellipsis where it was cut.
pub fn cut(text: &str, count: usize) -> String {
    if text.chars().count() <= count {
        return text.to_owned();
    }
    text.chars().take(count - 1).chain(['…']).collect()
}

/// The first lines of what the latest of `messages` (newest first) to say something said.
pub fn reply_in(messages: &[Value]) -> Option<String> {
    let written = messages
        .iter()
        .filter(|message| message["type"] == "assistant")
        .map(|message| said_in(message, "\n"));
    let written = written.into_iter().find(|text| !text.trim().is_empty())?;
    // (Marks alone said nothing: not an earlier reply in its place.)
    let text = reply_line(&written);
    if text.is_empty() {
        return None;
    }
    if text.chars().count() <= 280 {
        return Some(text);
    }
    // Cut short: not inside code, whose words would read as Markdown.
    let start = cut(&text, 280);
    if start.matches('`').count().is_multiple_of(2) {
        return Some(start);
    }
    let open = start.rfind('`').unwrap_or(start.len());
    Some(format!("{}…", start[..open].trim_end()))
}

/// What said it, as a hash: whether a reply says the same again.
pub fn hashed(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))[..16].to_owned()
}

/// Whether a finished turn is news to whoever started the session. One the user started by writing
/// to it directly never is; the first it hears of it always is; what it says of what it left
/// running is, once told before that was over; after that a turn someone asked for is, and one
/// that began on its own only with something to say: not nothing, not the same again, not a line
/// or two. Told last while its reply couldn't be read, a reply it had written by then was told.
pub fn is_news(was: &SessionTold, last: &LastReply) -> bool {
    if last.id.is_some() && last.id == was.reply {
        return false;
    }
    if was.unread
        && was
            .upto
            .is_some_and(|upto| last.at.is_some_and(|at| at <= upto))
    {
        return false;
    }
    match last.began.kind {
        BeganBy::You => false,
        _ if was.at.is_none() && was.reply.is_none() => true,
        kind if was.early && kind != BeganBy::Timer => true,
        BeganBy::Asked | BeganBy::Word => true,
        _ => last.text.chars().count() >= SUBSTANCE && Some(hashed(&last.text)) != was.said,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_summary_asked_for_at_rest_is_no_turn_of_its_work() {
        let rows = [
            json!({"type": "idle", "outcome": "succeeded"}),
            json!({"type": "compaction", "reason": "manual", "status": "completed"}),
            json!({"type": "idle", "outcome": "failed"}),
            json!({"type": "assistant", "id": "msg_2"}),
            json!({"type": "user", "id": "msg_1"}),
        ];
        let work = past_summaries(&rows);
        assert_eq!(work.len(), 3, "the summary and its end gone");
        assert_eq!(work_outcome(&rows, &json!("succeeded")), json!("failed"));
        assert_eq!(
            summary_row_at_end(&rows).map(|row| row["status"].clone()),
            Some(json!("completed"))
        );
        assert_eq!(turn_in(&work), (1, None), "after the end that's left");
        assert_eq!(turn_in(&work[1..]), (0, None));
    }

    #[test]
    fn what_began_a_turn() {
        let you = json!({"type": "user", "text": "Fix it", "time": {"created": 5}});
        let lead = json!({"type": "user", "text": "[Message from Lead · ses_l] Fix it"});
        let word = json!({"type": "user", "text": "[Session finished] \"x\""});
        let timer = json!({"type": "user", "text": "[Wake-up] Look"});
        let left = json!({"type": "synthetic", "metadata": {"source": "shell"}});
        assert_eq!(began_by(std::slice::from_ref(&you)).kind, BeganBy::You);
        assert_eq!(began_by(&[you, lead.clone()]).kind, BeganBy::Asked);
        assert_eq!(began_by(&[word]).kind, BeganBy::Word);
        assert_eq!(began_by(&[timer]).kind, BeganBy::Timer);
        assert_eq!(began_by(&[left]).kind, BeganBy::Left);
        assert_eq!(began_by(&[]).kind, BeganBy::Asked);
    }

    #[test]
    fn a_reply_as_a_line_cut_outside_code() {
        let reply = json!({"type": "assistant", "content": [{"type": "text", "text": "## Done\n```\ncode\n```\n- one\n---"}]});
        assert_eq!(reply_in(&[reply]).as_deref(), Some("Done code one"));
        let long = format!("{} `{}`", "a".repeat(270), "b".repeat(20));
        let reply = json!({"type": "assistant", "content": [{"type": "text", "text": long}]});
        assert_eq!(reply_in(&[reply]).unwrap(), format!("{}…", "a".repeat(270)));
    }
}
