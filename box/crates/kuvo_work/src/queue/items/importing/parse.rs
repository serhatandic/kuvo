//! Numbered items in a Markdown backlog, before they are admitted to the queue.

use crate::queue::{
    labels,
    store::{limit, plain_name},
};
use kuvo_setup::QueueLabel;
use regex::Regex;
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
};

pub(super) struct Parser {
    heading: Regex,
    sorted: Regex,
    bullet: Regex,
    done: Regex,
    stopped: Regex,
    backlog: Regex,
    last_word: Regex,
}

pub(super) struct Imported {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub status: &'static str,
    pub label: Option<String>,
}

pub(super) struct Backlog {
    pub items: Vec<Imported>,
    pub skipped: Vec<String>,
}

struct Found {
    lines: Vec<String>,
    title: String,
    status: &'static str,
    label: Option<String>,
}

impl Parser {
    pub fn new() -> Self {
        let compile = |pattern| Regex::new(pattern).expect("backlog pattern");
        Self {
            heading: compile(r"^(#{1,6})\s+(.*)$"),
            sorted: compile(
                r"^\s*(?:[-*]\s+)?(NOW|NEXT|LATER|PARKED|DONE)(?-u:\b)[^:]{0,80}:\s*(.*)$",
            ),
            bullet: compile(r"^\s*[-*]\s+(.*)$"),
            done: compile(r"(?-u:\b)(?:done|finished|completed)(?-u:\b)"),
            stopped: compile(r"(?-u:\b)(?:cancell?ed|dropped|won'?t do)(?-u:\b)"),
            backlog: compile(r"(?-u:\b)(?:shelved|parked|later|ideas?|someday|backlog)(?-u:\b)"),
            last_word: compile(r"\s+\S*$"),
        }
    }

    pub fn files(&self, given: &Value, library: &[QueueLabel]) -> Result<Backlog, String> {
        let files = given
            .as_array()
            .map_or(std::slice::from_ref(given), Vec::as_slice);
        let mut texts = Vec::new();
        for file in files
            .iter()
            .filter_map(Value::as_str)
            .filter(|name| !name.trim().is_empty())
            .take(10)
        {
            let unreadable = |_| format!("{file} can't be read.");
            let stat = fs::metadata(file).map_err(unreadable)?;
            if !stat.is_file() || stat.len() > 20_000_000 {
                return Err(format!("{file} isn't a Markdown file to import."));
            }
            let bytes = fs::read(file).map_err(unreadable)?;
            texts.push(String::from_utf8_lossy(&bytes).into_owned());
        }
        if texts.is_empty() {
            return Err("Name the files to import (files), as full paths.".into());
        }
        Ok(self.parse(&texts, library))
    }

    pub fn parse(&self, texts: &[String], library: &[QueueLabel]) -> Backlog {
        let mut found: BTreeMap<u64, Found> = BTreeMap::new();
        let mut places = HashMap::new();
        let mut skipped = Vec::new();
        for text in texts {
            let mut headings = Vec::<(usize, &str)>::new();
            for line in text.split('\n').map(str::trim_end) {
                if let Some(heading) = self.heading.captures(line) {
                    let level = heading[1].len();
                    while headings.last().is_some_and(|(before, _)| *before >= level) {
                        headings.pop();
                    }
                    headings.push((level, heading.get(2).expect("heading text").as_str()));
                    continue;
                }
                if let Some(sorted) = self.sorted.captures(line).filter(|sorted| {
                    sorted[2]
                        .as_bytes()
                        .windows(2)
                        .any(|pair| pair[0] == b'#' && pair[1].is_ascii_digit())
                }) {
                    let status = match &sorted[1] {
                        "NOW" | "NEXT" => "todo",
                        "DONE" => "done",
                        _ => "backlog",
                    };
                    places.extend(numbers(&sorted[2], ",(").map(|(_, number)| (number, status)));
                    continue;
                }
                let Some(listed) = self
                    .bullet
                    .captures(line)
                    .and_then(|line| line.get(1))
                    .map(|listed| listed.as_str())
                else {
                    continue;
                };
                let Some((digits, number, rest)) = number_at(listed) else {
                    continue;
                };
                let rest = self.additional(rest, number, &mut skipped).trim_start();
                if let Some(kept) = found.get_mut(&number) {
                    kept.lines.push(listed.into());
                    continue;
                }
                let (mark, text) = match rest.get(..3) {
                    Some(mark @ ("[ ]" | "[x]" | "[X]" | "[~]")) => (mark, rest[3..].trim()),
                    _ => ("", rest.trim()),
                };
                let (mut status, label) = headings
                    .iter()
                    .rev()
                    .find_map(|(_, heading)| self.place(heading, library))
                    .unwrap_or(("todo", None));
                if mark.eq_ignore_ascii_case("[x]") {
                    status = "done";
                }
                let title = self.title(text);
                let title = if title.is_empty() {
                    format!("Item {number}")
                } else {
                    title
                };
                let _ = digits;
                found.insert(
                    number,
                    Found {
                        lines: vec![listed.into()],
                        title,
                        status,
                        label,
                    },
                );
            }
        }
        for (number, status) in places {
            if let Some(kept) = found.get_mut(&number).filter(|item| item.status != "done") {
                kept.status = status;
            }
        }
        let mut named = HashSet::new();
        for line in texts
            .iter()
            .flat_map(|text| text.split('\n'))
            .filter(|line| self.bullet.is_match(line.trim_end()))
        {
            for (_, number) in numbers(line, ",(/–-") {
                if !found.contains_key(&number)
                    && named.insert(number)
                    && !skipped
                        .iter()
                        .any(|entry| entry.starts_with(&format!("#{number} ")))
                {
                    skipped.push(format!("#{number} (named, but no line starts with it)"));
                }
            }
        }
        let items = found
            .into_iter()
            .map(|(number, kept)| Imported {
                number,
                title: kept.title,
                body: limit(&kept.lines.join("\n\n"), 100_000),
                status: kept.status,
                label: kept.label,
            })
            .collect();
        let mut seen = HashSet::new();
        skipped.retain(|entry| seen.insert(entry.clone()));
        Backlog { items, skipped }
    }

    fn place(
        &self,
        heading: &str,
        library: &[QueueLabel],
    ) -> Option<(&'static str, Option<String>)> {
        let lower = heading.to_lowercase();
        if self.done.is_match(&lower) {
            return Some(("done", None));
        }
        if self.stopped.is_match(&lower) {
            return Some(("canceled", None));
        }
        if self.backlog.is_match(&lower) {
            return Some(("backlog", None));
        }
        let value = Value::String(heading.trim().to_owned());
        super::super::status_of(&value)
            .ok()
            .map(|status| (status, None))
            .or_else(|| {
                labels::named(library, &value)
                    .map(|index| ("todo", Some(library[index].id.clone())))
            })
    }

    fn additional<'a>(&self, mut rest: &'a str, number: u64, skipped: &mut Vec<String>) -> &'a str {
        loop {
            let after = rest.trim_start();
            let stripped = after.trim_start_matches(['/', '–', ',', '-']);
            if stripped == after {
                break;
            }
            let Some((digits, _, left)) = number_at(stripped.trim_start()) else {
                break;
            };
            skipped.push(format!("#{digits} (named with #{number} on one line)"));
            rest = left;
        }
        rest
    }

    fn title(&self, text: &str) -> String {
        let flat = plain_name(&text.replace("**", "").replace('`', ""));
        let mut units = 0;
        let mut end = flat.len();
        for (at, ch) in flat.char_indices() {
            if units >= 8
                && matches!(ch, '.' | '!' | '?')
                && flat[at + 1..]
                    .chars()
                    .next()
                    .is_none_or(char::is_whitespace)
            {
                end = at + 1;
                break;
            }
            units += ch.len_utf16();
        }
        let sentence = &flat[..end];
        if sentence.encode_utf16().count() <= 120 {
            return sentence.into();
        }
        let cut = limit(sentence, 119);
        format!("{}…", self.last_word.replace(&cut, ""))
    }
}

fn number_at(text: &str) -> Option<(&str, u64, &str)> {
    let text = text.strip_prefix('#')?;
    let end = text.bytes().take_while(u8::is_ascii_digit).count();
    if !(1..=6).contains(&end)
        || text
            .as_bytes()
            .get(end)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        return None;
    }
    let digits = &text[..end];
    Some((digits, digits.parse().ok()?, &text[end..]))
}

fn numbers<'a>(line: &'a str, separators: &'a str) -> impl Iterator<Item = (&'a str, u64)> {
    line.match_indices('#').filter_map(move |(at, _)| {
        if line[..at]
            .chars()
            .next_back()
            .is_some_and(|ch| !ch.is_whitespace() && !separators.contains(ch))
        {
            return None;
        }
        let (digits, number, _) = number_at(&line[at..])?;
        Some((digits, number))
    })
}
