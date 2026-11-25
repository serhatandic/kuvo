//! Client-facing presentation from the accepted account windows, not a second reader.
use super::{Entry, Provider, ordered_trails};
use crate::usage::{
    plan_facts::{Extra, ProblemKind},
    readings::{Source, Window},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
const MINUTE: u64 = 60_000;
const CURRENT: u64 = 30 * MINUTE;
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Tone {
    Danger,
    Warning,
    Calm,
    Unknown,
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
enum WordTone {
    Danger,
    Warning,
    Muted,
    Faint,
}
#[derive(Clone, Serialize)]
struct Word {
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    at: Option<u64>,
    #[serde(rename = "as", skip_serializing_if = "Option::is_none")]
    format: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tone: Option<WordTone>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    strong: bool,
}
impl Word {
    fn text(text: impl Into<String>, tone: Option<WordTone>) -> Self {
        Self {
            text: text.into(),
            at: None,
            format: None,
            tone,
            strong: false,
        }
    }
    fn time(
        text: &'static str,
        at: u64,
        format: &'static str,
        tone: Option<WordTone>,
        strong: bool,
    ) -> Self {
        Self {
            text: text.into(),
            at: Some(at),
            format: Some(format),
            tone,
            strong,
        }
    }
    fn sep() -> Self {
        Self::text(" · ", Some(WordTone::Faint))
    }
}
#[derive(Serialize)]
struct Line {
    window: String,
    title: String,
    left: f64,
    tone: Tone,
    words: Vec<Word>,
}
#[derive(Serialize)]
struct Confirm {
    title: Vec<Word>,
    message: String,
    button: &'static str,
    cancel: &'static str,
}
#[derive(Serialize)]
struct Resets {
    words: Vec<Word>,
    action: &'static str,
    confirm: Confirm,
}
#[derive(Serialize)]
struct Card {
    title: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    dot: Option<Tone>,
    tone: Tone,
    #[serde(skip_serializing_if = "Option::is_none")]
    left: Option<f64>,
    lines: Vec<Line>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    #[serde(rename = "asOf", skip_serializing_if = "Option::is_none")]
    as_of: Option<Vec<Word>>,
    #[serde(rename = "inUse", skip_serializing_if = "std::ops::Not::not")]
    in_use: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    resets: Option<Resets>,
}
fn usage_title(window: &Window) -> String {
    if let Some(name) = window.name.as_ref().filter(|s| !s.is_empty()) {
        return name.clone();
    }
    let span = match window.minutes {
        Some(300) => "5-hour".into(),
        Some(10080) => "Weekly".into(),
        Some(40000..=45000) => "Monthly".into(),
        Some(n) if n >= 1440 => format!("{}-day", n / 1440),
        Some(n) if n >= 60 => format!("{}-hour", n / 60),
        _ => "Limit".into(),
    };
    window
        .model
        .as_ref()
        .filter(|s| !s.is_empty())
        .map_or(span.clone(), |m| {
            if span == "Limit" {
                m.clone()
            } else {
                format!("{m} · {span}")
            }
        })
}
#[derive(Clone, Serialize)]
struct Status {
    words: String,
    warning: bool,
}
fn status(kind: Option<ProblemKind>, has_meters: bool, checked: bool) -> Option<Status> {
    let (words, warning) = match kind {
        Some(ProblemKind::Ended) => ("Plan ended.", false),
        Some(ProblemKind::Expired) => ("Sign-in expired. Sign in again under Accounts.", true),
        Some(ProblemKind::Unreadable) => {
            ("Can’t read limits. Check the sign-in under Accounts.", true)
        }
        Some(ProblemKind::RateLimited) => {
            ("The provider paused readings. Kuvo will try again.", true)
        }
        Some(ProblemKind::Failed) => (
            "Couldn’t read limits. Check the connection, then try again.",
            true,
        ),
        _ if has_meters => return None,
        _ if checked => ("No limits reported.", false),
        _ => ("Waiting for the first reading.", false),
    };
    Some(Status {
        words: words.into(),
        warning,
    })
}
#[derive(Serialize)]
struct ResetLabel {
    at: u64,
    words: String,
}
fn labels(at: Option<u64>, now: u64, until: u64) -> Vec<ResetLabel> {
    let Some(at) = at else {
        return vec![ResetLabel {
            at: now,
            words: "Reset time unavailable".into(),
        }];
    };
    if at <= now {
        return vec![ResetLabel {
            at: now,
            words: "Reset due".into(),
        }];
    }
    let mut labels = vec![];
    let mut start = now.max(at.saturating_sub(24 * 60 * MINUTE - 1));
    while start <= until && start < at {
        let left = at - start;
        let unit = if left.div_ceil(MINUTE) >= 60 {
            60 * MINUTE
        } else {
            MINUTE
        };
        let count = left.div_ceil(unit);
        labels.push(ResetLabel {
            at: start,
            words: format!(
                "resets in {count} {}",
                if unit == MINUTE { "min" } else { "h" }
            ),
        });
        let next = at - (count - 1) * unit;
        start = if unit == 60 * MINUTE {
            next.min(at.saturating_sub(59 * MINUTE))
        } else {
            next
        };
    }
    labels
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Meter {
    title: String,
    share: f64,
    value: String,
    level: &'static str,
    read_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    resets_at: Option<u64>,
    fresh_for_ms: u64,
    reset_labels: Vec<ResetLabel>,
}
#[derive(Clone)]
pub(super) struct Member {
    pub entry: Entry,
    pub name: String,
    pub here: bool,
    pub away: bool,
    pub away_at: Option<u64>,
}
fn meter(
    member: &Member,
    title: String,
    used: f64,
    read: u64,
    reset: Option<u64>,
    now: u64,
) -> Meter {
    let share = used.clamp(0., 1.);
    let failed = member
        .entry
        .facts
        .problem
        .as_ref()
        .is_some_and(|p| !(p.unknown && member.entry.facts.checked == Some(read)));
    let fresh = if member.away || failed {
        0
    } else {
        read.saturating_add(CURRENT)
            .min(reset.unwrap_or(u64::MAX))
            .min(member.away_at.unwrap_or(u64::MAX))
            .saturating_sub(now)
    };
    Meter {
        title,
        share,
        value: format!("{}%", (share * 100.).round()),
        level: if share > 0.7 {
            super::level(share)
        } else {
            "calm"
        },
        read_at: read,
        resets_at: reset,
        fresh_for_ms: fresh,
        reset_labels: labels(reset, now, now.saturating_add(fresh)),
    }
}
pub(super) fn display(entry: &Entry, members: &[Member], now: u64) -> Value {
    let mut meters = vec![];
    let mut used_members = vec![];
    let mut add = |index: usize, title, used, read, reset| {
        if !used_members.contains(&index) {
            used_members.push(index);
        }
        meters.push(meter(&members[index], title, used, read, reset, now));
    };
    let ended = entry
        .facts
        .problem
        .as_ref()
        .is_some_and(|p| p.kind == ProblemKind::Ended);
    if !ended {
        for trail in ordered_trails(entry) {
            let w = &trail.current;
            let index = members
                .iter()
                .position(|m| {
                    m.entry
                        .trails
                        .get(&w.id)
                        .is_some_and(|t| t.current.read == w.read)
                })
                .unwrap_or(0);
            add(index, usage_title(w), w.used, w.read, w.resets_at);
        }
        if let Some(b) = &entry.facts.balance {
            if let Some(limit) = b.limit.filter(|n| *n > 0.) {
                if let Some(index) = members
                    .iter()
                    .position(|m| m.entry.facts.balance.as_ref() == Some(b))
                {
                    add(
                        index,
                        "Credits".into(),
                        1. - b.left / limit,
                        members[index].entry.facts.checked.unwrap_or(0),
                        None,
                    );
                }
            }
        }
        for e in &entry.facts.extras {
            if let Extra::Capped {
                name,
                amount,
                limit,
                resets_at,
                ..
            } = e
            {
                if *limit > 0. {
                    if let Some(index) = members
                        .iter()
                        .position(|m| m.entry.facts.extras.contains(e))
                    {
                        add(
                            index,
                            name.clone(),
                            amount / limit,
                            members[index].entry.facts.checked.unwrap_or(0),
                            *resets_at,
                        );
                    }
                }
            }
        }
    }
    if meters.is_empty() {
        used_members.extend(0..members.len());
    }
    let status = entry
        .facts
        .problem
        .as_ref()
        .filter(|p| p.unknown)
        .map(|p| Status {
            words: p.message.clone(),
            warning: true,
        })
        .or_else(|| {
            status(
                entry.facts.problem.as_ref().map(|p| p.kind),
                !meters.is_empty(),
                entry.facts.checked.is_some(),
            )
        });
    let updated = meters
        .iter()
        .map(|m| m.read_at)
        .chain(entry.facts.checked)
        .max()
        .filter(|at| *at > 0);
    let computers = used_members
        .into_iter()
        .map(|i| json!({"name":members[i].name,"here":members[i].here}))
        .collect::<Vec<_>>();
    let mut data = json!({"title":entry.account.title(),"computers":computers,"meters":meters});
    if let Some(updated) = updated {
        data["updatedAt"] = json!(updated);
    }
    if let Some(status) = status {
        data["status"] = json!(status);
    }
    data
}
fn base_title(row: &Value) -> String {
    let name = row["agent"]
        .as_str()
        .or_else(|| row["service"].as_str())
        .unwrap_or_default();
    match (row["agent"].is_string(), name) {
        (true, "claude") => "Claude".into(),
        (true, "codex") => "Codex".into(),
        (true, "grok") => "Grok".into(),
        (true, "cursor") => "Cursor".into(),
        (true, "antigravity") => "Antigravity".into(),
        (false, "codex") => "ChatGPT".into(),
        (false, "grok") => "SuperGrok".into(),
        (false, "copilot") => "Copilot".into(),
        (false, "openrouter") => "OpenRouter".into(),
        (false, "zai") => "Z.ai".into(),
        (false, "ollama") => "Ollama".into(),
        (false, "opencode") => "OpenCode Go".into(),
        _ => {
            let mut chars = name.chars();
            chars
                .next()
                .into_iter()
                .flat_map(char::to_uppercase)
                .chain(chars)
                .collect()
        }
    }
}
fn resets(row: &Value) -> Option<Resets> {
    let extra = row["extras"]
        .as_array()?
        .iter()
        .find(|e| e["kind"] == "resets")?;
    let count = extra["count"].as_u64()?;
    if count < 1 || extra["usable"] != true || !extra["on"].is_object() {
        return None;
    }
    let next = extra["expiries"][0].as_u64();
    let mut words = vec![Word::text(
        if count == 1 {
            "1 reset left".into()
        } else {
            format!("{count} resets left")
        },
        Some(WordTone::Muted),
    )];
    if let Some(next) = next {
        words.push(Word::sep());
        words.push(Word::time(
            if count == 1 {
                "expires "
            } else {
                "next expires "
            },
            next,
            "date",
            Some(WordTone::Muted),
            false,
        ));
    }
    let title = next.map_or_else(
        || vec![Word::text("Use a reset?", None)],
        |at| {
            vec![
                Word::time("Use the reset that expires ", at, "date", None, false),
                Word::text("?", None),
            ]
        },
    );
    Some(Resets {
        words,
        action: "Use a reset",
        confirm: Confirm {
            title,
            message: format!("{}’s usage limits start over now.", base_title(row)),
            button: "Use Reset",
            cancel: "Cancel",
        },
    })
}
pub(super) fn card(entry: &Entry, row: &Value, now: u64, siblings: usize) -> Value {
    use crate::usage::readings::CardPrediction;
    let mut trails = ordered_trails(entry);
    trails.sort_by_key(|t| {
        (
            if t.current.model.as_deref().is_some_and(|s| !s.is_empty()) {
                2
            } else if t.current.name.as_deref().is_some_and(|s| !s.is_empty()) {
                1
            } else {
                0
            },
            t.current.minutes.unwrap_or(u32::MAX),
        )
    });
    let mut lines = vec![];
    let mut warning = false;
    let mut danger = false;
    if row["ended"] != true {
        for trail in trails {
            let w = &trail.current;
            let title = usage_title(w);
            if w.expired(now) {
                lines.push(Line {
                    window: w.id.clone(),
                    title,
                    left: 1.,
                    tone: Tone::Calm,
                    words: vec![Word::text(
                        "Started over, not read since",
                        Some(WordTone::Muted),
                    )],
                });
                continue;
            }
            let left = (1. - w.used).clamp(0., 1.);
            let percent = format!("{}% left", (left * 100.).round());
            let (left, tone, words) = match trail.card_prediction(now) {
                Some(CardPrediction::UsedUp) => {
                    danger = true;
                    (
                        0.,
                        Tone::Danger,
                        vec![w.resets_at.map_or_else(
                            || {
                                let mut word = Word::text("Used up", Some(WordTone::Danger));
                                word.strong = true;
                                word
                            },
                            |at| Word::time("Back at ", at, "time", Some(WordTone::Danger), true),
                        )],
                    )
                }
                Some(CardPrediction::RunsOut { at }) => {
                    warning = true;
                    (
                        left,
                        Tone::Warning,
                        vec![
                            Word::text(percent, None),
                            Word::sep(),
                            Word::time("runs out ~", at, "time", Some(WordTone::Warning), true),
                            Word::time(
                                ", resets ",
                                w.resets_at.unwrap_or(at),
                                "time",
                                Some(WordTone::Muted),
                                false,
                            ),
                        ],
                    )
                }
                Some(CardPrediction::Lasts) => (
                    left,
                    Tone::Calm,
                    vec![
                        Word::text(percent, Some(WordTone::Muted)),
                        Word::sep(),
                        Word::text("lasts until reset", Some(WordTone::Muted)),
                    ],
                ),
                None => {
                    let mut words = vec![Word::text(percent, Some(WordTone::Muted))];
                    if let Some(at) = w.resets_at {
                        words.push(Word::sep());
                        words.push(Word::time(
                            "resets ",
                            at,
                            "time",
                            Some(WordTone::Muted),
                            false,
                        ));
                    }
                    (
                        left,
                        if w.used >= 0.9 {
                            Tone::Warning
                        } else {
                            Tone::Calm
                        },
                        words,
                    )
                }
            };
            lines.push(Line {
                window: w.id.clone(),
                title,
                left,
                tone,
                words,
            });
        }
    }
    let spans = row["windows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|w| w["span"].is_string())
        .filter_map(|w| w["id"].as_str())
        .collect::<BTreeSet<_>>();
    let left = lines
        .iter()
        .filter(|l| spans.is_empty() || spans.contains(l.window.as_str()))
        .map(|l| l.left)
        .reduce(f64::min);
    let tone = if danger {
        Tone::Danger
    } else if warning {
        Tone::Warning
    } else if left.is_none() {
        Tone::Unknown
    } else {
        Tone::Calm
    };
    let newest = entry
        .trails
        .values()
        .map(|t| t.current.read)
        .max()
        .unwrap_or_default();
    let as_of =
        (!lines.is_empty() && newest > 0 && now.saturating_sub(newest) > CURRENT).then(|| {
            vec![Word::time(
                "as of ",
                newest,
                "time",
                Some(WordTone::Faint),
                false,
            )]
        });
    let base = base_title(row);
    let given_title = row["title"].as_str().unwrap_or(&base);
    let name = row["label"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| given_title.strip_prefix(&format!("{base} · ")))
        .or_else(|| row["plan"].as_str())
        .unwrap_or(given_title)
        .to_owned();
    let title = if siblings > 1 && !given_title.contains(" · ") {
        format!("{base} · {name}")
    } else {
        given_title.to_owned()
    };
    let problem = entry.facts.problem.as_ref();
    let note = if let Some(problem) = problem.filter(|p| p.unknown) {
        Some(if entry.account.service == Provider::Antigravity {
            "Google isn’t counting this account’s use yet, so what’s left is unknown.".into()
        } else {
            problem.message.clone()
        })
    } else if !lines.is_empty() {
        None
    } else if let Some(problem) = problem {
        status(Some(problem.kind), false, entry.facts.checked.is_some()).map(|s| s.words)
    } else if entry.facts.checked.is_none() {
        Some("Fills in once a session runs on this account".into())
    } else {
        Some("No limits reported.".into())
    };
    let in_use = entry.trails.values().any(|t| {
        t.current.source == Source::Turn
            && t.current.read > 0
            && now.saturating_sub(t.current.read) <= CURRENT
    });
    serde_json::to_value(Card {
        title,
        name,
        dot: if danger {
            Some(Tone::Danger)
        } else if warning {
            Some(Tone::Warning)
        } else {
            None
        },
        tone,
        left,
        lines,
        note,
        as_of,
        in_use,
        resets: resets(row),
    })
    .unwrap_or_default()
}
pub(super) fn named(rows: &mut [Value]) {
    let mut names = rows
        .iter()
        .map(|row| {
            let base = base_title(row);
            row["label"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map_or(base.clone(), |label| format!("{base} · {label}"))
        })
        .collect::<Vec<_>>();
    let clashes = |names: &[String]| {
        let mut counts = BTreeMap::new();
        for n in names {
            *counts.entry(n.clone()).or_insert(0usize) += 1;
        }
        counts
            .into_iter()
            .filter_map(|(n, count)| (count > 1).then_some(n))
            .collect::<BTreeSet<_>>()
    };
    let same = clashes(&names);
    for (row, name) in rows.iter().zip(&mut names) {
        let local = row["boxes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|b| b["here"] == true)
            .collect::<Vec<_>>();
        if !same.contains(name) || local.is_empty() || local.iter().any(|b| b["agent"].is_string())
        {
            continue;
        }
        let base = base_title(&json!({"service":row["service"]}));
        let label = local[0]["label"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        *name = label.map_or(base.clone(), |label| format!("{base} · {label}"));
    }
    for here in [false, true] {
        let same = clashes(&names);
        for (row, name) in rows.iter().zip(&mut names) {
            let boxes = row["boxes"]
                .as_array()
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            if !same.contains(name) || boxes.iter().any(|b| b["here"] == true) != here {
                continue;
            }
            let mut seen = BTreeSet::new();
            let other = boxes
                .iter()
                .filter(|b| b["here"] != true)
                .filter_map(|b| b["name"].as_str())
                .filter(|name| seen.insert(*name))
                .collect::<Vec<_>>()
                .join(", ");
            if !other.is_empty() {
                *name = format!("{name} · {other}");
            }
        }
    }
    for (row, name) in rows.iter_mut().zip(names) {
        row["title"] = json!(name);
    }
}
pub(super) fn railed(row: &Value) -> bool {
    row["ended"] != true
        && row["windows"].as_array().is_some_and(|w| !w.is_empty())
        && row["boxes"]
            .as_array()
            .is_some_and(|b| b.iter().any(|b| b["here"] == true))
}
pub(super) fn rail_key(row: &Value) -> &str {
    row["agent"]
        .as_str()
        .or_else(|| row["service"].as_str())
        .unwrap_or_default()
}
fn newest_turn(row: &Value) -> u64 {
    row["windows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|w| w["source"] == "turn")
        .filter_map(|w| w["read"].as_u64())
        .max()
        .unwrap_or(0)
}
fn tone(row: &Value) -> Tone {
    serde_json::from_value(row["card"]["tone"].clone()).unwrap_or(Tone::Unknown)
}
pub(super) fn rail(rows: &[Value]) -> Vec<Value> {
    let shown = rows.iter().filter(|r| railed(r)).collect::<Vec<_>>();
    let mut groups = BTreeMap::<&str, Vec<&Value>>::new();
    for row in &shown {
        groups.entry(rail_key(row)).or_default().push(row);
    }
    let mut placed = BTreeSet::new();
    let mut items = vec![];
    for row in shown {
        let key = rail_key(row);
        let members = &groups[key];
        if members.len() < 3 {
            let mut item = json!({"key":key,"rows":[row["id"]],"ring":row["id"],"selected":row["id"],"title":row["card"]["title"]});
            if let Some(dot) = row["card"].get("dot") {
                item["dot"] = dot.clone();
            }
            items.push(item);
            continue;
        }
        if !placed.insert(key) {
            continue;
        }
        let mut strip = members.clone();
        strip.sort_by(
            |a, b| match (a["card"]["left"].as_f64(), b["card"]["left"].as_f64()) {
                (Some(a_left), Some(b_left)) => {
                    a_left.total_cmp(&b_left).then(tone(a).cmp(&tone(b)))
                }
                (None, None) => std::cmp::Ordering::Equal,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                _ => std::cmp::Ordering::Less,
            },
        );
        let mut in_use: Option<&Value> = None;
        for member in members {
            if member["card"]["inUse"] == true
                && in_use.is_none_or(|old| newest_turn(member) > newest_turn(old))
            {
                in_use = Some(member);
            }
        }
        let selected = in_use.unwrap_or(strip[0]);
        let ring = strip
            .iter()
            .copied()
            .find(|m| m["rings"]["short"].is_object() || m["rings"]["long"].is_object())
            .unwrap_or(strip[0]);
        let fine = members.iter().filter(|m| tone(m) == Tone::Calm).count();
        let mut item = json!({"key":key,"rows":strip.iter().map(|m|m["id"].clone()).collect::<Vec<_>>(),"ring":ring["id"],"selected":selected["id"],"title":base_title(row),"count":members.len(),"summary":format!("{fine} of {} fine",members.len()),"strip":strip.iter().map(|m|{let mut heading=vec![json!({"text":m["card"]["name"],"strong":true})];if in_use.is_some_and(|u|u["id"]==m["id"]){heading.push(json!({"text":" · ","tone":"faint"}));heading.push(json!({"text":"in use","tone":"muted"}));}let mut mark=json!({"row":m["id"],"name":m["card"]["name"],"tone":m["card"]["tone"],"heading":heading});if let Some(left)=m["card"].get("left"){mark["left"]=left.clone();}mark}).collect::<Vec<_>>()});
        if members.iter().any(|m| m["card"]["dot"] == "danger") {
            item["dot"] = json!("danger");
        } else if members.iter().any(|m| m["card"]["dot"] == "warning") {
            item["dot"] = json!("warning");
        }
        items.push(item);
    }
    items
}
pub(super) fn merged_facts(members: &[Member]) -> crate::usage::plan_facts::Facts {
    use crate::usage::plan_facts::Facts;
    if members.len() == 1 {
        return members[0].entry.facts.clone();
    }
    let newest = |m: &Member| {
        m.entry.facts.checked.unwrap_or_else(|| {
            m.entry
                .trails
                .values()
                .map(|t| t.current.read)
                .max()
                .unwrap_or(0)
        })
    };
    let mut order = members.iter().collect::<Vec<_>>();
    order.sort_by_key(|m| std::cmp::Reverse(newest(m)));
    let plan = order.iter().find_map(|m| m.entry.facts.plan.clone());
    let balance = order.iter().find_map(|m| m.entry.facts.balance.clone());
    let chosen = order
        .iter()
        .find(|m| m.entry.facts.checked.is_some() && m.entry.facts.problem.is_none())
        .or_else(|| {
            order.iter().find(|m| {
                m.entry
                    .facts
                    .extras
                    .iter()
                    .any(|e| !matches!(e, Extra::Unreadable { .. }))
            })
        })
        .or_else(|| order.iter().find(|m| !m.entry.facts.extras.is_empty()));
    fn extra_id(e: &Extra) -> &str {
        match e {
            Extra::Spent { id, .. }
            | Extra::Capped { id, .. }
            | Extra::Left { id, .. }
            | Extra::Resets { id, .. }
            | Extra::Unreadable { id, .. } => id,
        }
    }
    let extras = chosen
        .map(|m| {
            m.entry
                .facts
                .extras
                .iter()
                .map(|e| {
                    if matches!(e, Extra::Unreadable { .. }) {
                        order
                            .iter()
                            .flat_map(|m| m.entry.facts.extras.iter())
                            .find(|other| {
                                extra_id(other) == extra_id(e)
                                    && !matches!(other, Extra::Unreadable { .. })
                            })
                            .unwrap_or(e)
                            .clone()
                    } else {
                        e.clone()
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let checked = members.iter().filter_map(|m| m.entry.facts.checked).max();
    let next = members.iter().filter_map(|m| m.entry.facts.next).min();
    let problem = members
        .iter()
        .all(|m| m.entry.facts.problem.is_some())
        .then(|| order.first().and_then(|m| m.entry.facts.problem.clone()))
        .flatten();
    Facts {
        plan,
        balance,
        extras,
        checked,
        next,
        problem,
        partial: false,
    }
}
