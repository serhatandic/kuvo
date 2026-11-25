//! Find Work Queue references in client Markdown and keep their original bindings.

use super::store::{Item, item_id};
use markdown::{ParseOptions, mdast::Node, to_mdast};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
};

pub(super) struct Mentions {
    number: Regex,
    outside: Regex,
    excluded: Regex,
    preceding_word: Regex,
    following_word: Regex,
    word_char: Regex,
    punctuation: Regex,
    legacy_image: Regex,
}

impl Mentions {
    pub fn new() -> Self {
        let compile = |pattern| Regex::new(pattern).expect("Work Queue reference pattern");
        Self {
            number: compile(r"#([0-9]+)"),
            outside: compile(
                r"(?i)(?:https?://|www\.)[^\s<]+|gh:[a-z0-9][a-z0-9_-]{0,38}/[a-z0-9_.-]{1,100}#[0-9]{1,10}",
            ),
            excluded: compile(r"(?i)(?:^|[^\p{L}\p{N}])(?:pr|mr|pull request|issue|step)$"),
            preceding_word: compile(r"\p{L}+$"),
            following_word: compile(r"^\p{L}+"),
            word_char: compile(r"^[\p{L}\p{N}_]"),
            punctuation: compile(r"^\p{P}+|\p{P}+$"),
            legacy_image: compile(r"\[Image ([0-9]{1,3})\]"),
        }
    }

    pub fn find(&self, text: &str) -> Vec<Mention> {
        if !text.contains('#') {
            return Vec::new();
        }
        let mut excluded = Vec::new();
        if let Ok(tree) = to_mdast(text, &ParseOptions::default()) {
            ranges(&tree, protected, &mut excluded);
        }
        excluded.extend(self.outside.find_iter(text).map(|matched| matched.range()));
        excluded.sort_by_key(|range| range.start);
        let mut protected = excluded.into_iter().peekable();
        let mut prior_offset = 0;
        let mut prior_index = 0;
        let mut mentions = Vec::new();
        for capture in self.number.captures_iter(text) {
            let Some(matched) = capture.get(0) else {
                continue;
            };
            let digits = &capture[1];
            let span = matched.range();
            if text[..span.start]
                .chars()
                .next_back()
                .is_some_and(|before| {
                    !before.is_whitespace()
                        && !matches!(
                            before,
                            '(' | '[' | '{' | '“' | '‘' | '"' | '\'' | '/' | ',' | '–' | '-'
                        )
                })
                || digits.len() > 6
                || self.word_char.is_match(&text[span.end..])
            {
                continue;
            }
            let Some(number) = digits.parse::<u32>().ok().filter(|number| *number > 0) else {
                continue;
            };
            while protected
                .peek()
                .is_some_and(|range| range.end <= span.start)
            {
                protected.next();
            }
            if protected.peek().is_some_and(|range| range.start < span.end) {
                continue;
            }
            let prefix = utf16_tail(&text[..span.start], 80).trim_end();
            if self.excluded.is_match(prefix)
                || (number <= 10 && self.rank(prefix, &text[span.end..]))
            {
                continue;
            }
            let start = prior_index + text[prior_offset..span.start].encode_utf16().count();
            let end = start + matched.as_str().len();
            mentions.push(Mention {
                start,
                end,
                number,
                item: None,
            });
            prior_index = end;
            prior_offset = span.end;
        }
        mentions
    }

    pub fn without_pictures(&self, mut text: String, urls: &HashSet<&str>) -> String {
        if urls.is_empty() {
            return text;
        }
        let Ok(tree) = to_mdast(&text, &ParseOptions::default()) else {
            return text;
        };
        let mut remove = Vec::new();
        ranges(
            &tree,
            |node| matches!(node, Node::Image(image) if urls.contains(image.url.as_str())),
            &mut remove,
        );
        if text.contains("[Image ")
            && urls
                .iter()
                .any(|url| url.starts_with("kuvo-attachment:legacy/"))
        {
            let mut excluded = Vec::new();
            ranges(&tree, protected, &mut excluded);
            for capture in self.legacy_image.captures_iter(&text) {
                let Some(found) = capture.get(0) else {
                    continue;
                };
                let span = found.range();
                if urls.contains(format!("kuvo-attachment:legacy/{}", &capture[1]).as_str())
                    && !text[..span.start].ends_with('\\')
                    && !excluded
                        .iter()
                        .any(|range| range.start < span.end && span.start < range.end)
                {
                    remove.push(span);
                }
            }
        }
        remove.sort_by_key(|range| std::cmp::Reverse(range.start));
        for range in remove {
            text.replace_range(range, "");
        }
        text
    }

    fn rank(&self, prefix: &str, suffix: &str) -> bool {
        let ranked = |word: &str| {
            matches!(
                word,
                "ranked"
                    | "rank"
                    | "number"
                    | "result"
                    | "results"
                    | "spot"
                    | "place"
                    | "position"
                    | "priority"
                    | "seller"
            )
        };
        let left = self
            .preceding_word
            .find(prefix)
            .map(|word| word.as_str().to_lowercase())
            .unwrap_or_default();
        let right = utf16_head(suffix, 100).trim_start().to_lowercase();
        let next = self
            .following_word
            .find(&right)
            .map_or("", |word| word.as_str());
        let second = right
            .split_whitespace()
            .nth(1)
            .map(|word| self.punctuation.replace_all(word, ""));
        ranked(&left)
            || ranked(next)
            || (matches!(next, "on" | "in")
                && second
                    .as_ref()
                    .is_some_and(|word| word.chars().any(char::is_alphabetic)))
            || (next == "google"
                && second
                    .as_deref()
                    .is_some_and(|word| matches!(word, "result" | "results")))
    }
}

fn protected(node: &Node) -> bool {
    matches!(
        node,
        Node::Code(_)
            | Node::InlineCode(_)
            | Node::Link(_)
            | Node::LinkReference(_)
            | Node::Image(_)
            | Node::ImageReference(_)
            | Node::Definition(_)
    )
}

fn ranges(tree: &Node, select: impl Fn(&Node) -> bool, out: &mut Vec<Range<usize>>) {
    let mut pending = vec![tree];
    while let Some(node) = pending.pop() {
        if select(node) {
            if let Some(position) = node.position() {
                out.push(position.start.offset..position.end.offset);
            }
        } else if let Some(children) = node.children() {
            pending.extend(children.iter().rev());
        }
    }
}

fn utf16_head(text: &str, units: usize) -> &str {
    let mut used = 0;
    let end = text
        .char_indices()
        .find_map(|(at, ch)| {
            used += ch.len_utf16();
            (used > units).then_some(at)
        })
        .unwrap_or(text.len());
    &text[..end]
}

fn utf16_tail(text: &str, units: usize) -> &str {
    let mut used = 0;
    let start = text
        .char_indices()
        .rev()
        .find_map(|(at, ch)| {
            used += ch.len_utf16();
            (used > units).then_some(at + ch.len_utf8())
        })
        .unwrap_or(0);
    &text[start..]
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Mention {
    start: usize,
    end: usize,
    number: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Index {
    pub body: Vec<Mention>,
    pub notes: Vec<Vec<Mention>>,
    pub targets: Vec<String>,
}

impl Index {
    pub fn read(item: &Item) -> Option<Self> {
        let index: Self = serde_json::from_value(item.saved.get("mentionIndex")?.clone()).ok()?;
        (index.notes.len() == item.notes.len()
            && index.targets.iter().all(|id| item_id(id))
            && live_ranges(&item.body, &index.body)
            && index.notes.iter().zip(&item.notes).all(|(ranges, note)| {
                live_ranges(note["text"].as_str().unwrap_or_default(), ranges)
            }))
        .then_some(index)
    }

    pub fn rebuild(parser: &Mentions, item: &Item, all: &[Item], prior: Option<&Self>) -> Self {
        let mut by_number = HashMap::<u32, &str>::new();
        for other in all.iter().filter(|other| other.removed.is_none()) {
            if let Some(number) = other.number.filter(|number| *number < 1_000_000.0) {
                by_number.entry(number as u32).or_insert(&other.id);
            }
        }
        for link in super::store::array(item.saved.get("mentionLinks")) {
            if link["kind"].as_str() != Some("mentions") {
                continue;
            }
            if let Some(other) = all
                .iter()
                .find(|other| link["item"].as_str() == Some(other.id.as_str()))
                && let Some(number) = other.number.filter(|number| *number < 1_000_000.0)
            {
                by_number.insert(number as u32, &other.id);
            }
        }
        if let Some(prior) = prior {
            for mention in std::iter::once(&prior.body).chain(&prior.notes).flatten() {
                if let Some(id) = &mention.item {
                    by_number.insert(mention.number, id);
                }
            }
        }
        let bind = |text: &str| {
            parser
                .find(text)
                .into_iter()
                .map(|mut range| {
                    if let Some(id) = by_number.get(&range.number).filter(|id| **id != item.id) {
                        range.item = Some((*id).to_owned());
                    }
                    range
                })
                .collect::<Vec<_>>()
        };
        let body = bind(&item.body);
        let notes: Vec<_> = item
            .notes
            .iter()
            .map(|note| bind(note["text"].as_str().unwrap_or_default()))
            .collect();
        Self::from_ranges(body, notes)
    }

    pub fn from_ranges(body: Vec<Mention>, notes: Vec<Vec<Mention>>) -> Self {
        let mut seen = HashSet::new();
        let targets = std::iter::once(&body)
            .chain(&notes)
            .flatten()
            .filter_map(|range| range.item.as_ref())
            .filter(|id| seen.insert(*id))
            .cloned()
            .collect();
        Self {
            body,
            notes,
            targets,
        }
    }
}

fn live_ranges(text: &str, ranges: &[Mention]) -> bool {
    let units: Vec<_> = text.encode_utf16().collect();
    let mut previous = 0;
    ranges.iter().all(|range| {
        let Some(chars) = units.get(range.start..range.end) else {
            return false;
        };
        if range.start < previous || !range.item.as_deref().is_none_or(item_id) {
            return false;
        }
        previous = range.end;
        let [35, digits @ ..] = chars else {
            return false;
        };
        (1..=6).contains(&digits.len())
            && digits.iter().all(|digit| (48..=57).contains(digit))
            && digits
                .iter()
                .fold(0, |n, digit| n * 10 + u32::from(digit - 48))
                == range.number
    })
}

/// A valid, already-bound range is never rebound by an unrelated write.
/// Missing indexes and as-yet-unbound ranges can gain their item when a number
/// arrives or an older queue is recovered.
pub(super) fn needs_sync(item: &Item) -> bool {
    Index::read(item).is_none_or(|index| {
        std::iter::once(&index.body)
            .chain(&index.notes)
            .flatten()
            .any(|range| range.item.is_none())
    })
}

pub(super) fn before_numbering(items: &[Item]) -> Option<HashMap<String, u32>> {
    items.iter().any(needs_sync).then(|| {
        items
            .iter()
            .filter(|item| item.removed.is_none())
            .filter_map(|item| {
                Some((
                    item.id.clone(),
                    item.number.filter(|number| *number < 1_000_000.0)? as u32,
                ))
            })
            .collect()
    })
}

pub(super) fn sync_indexes(
    parser: &Mentions,
    items: &[Item],
    before: &HashMap<String, u32>,
) -> Vec<(usize, Index)> {
    let mut by_number = HashMap::<u32, &str>::new();
    for item in items.iter().filter(|item| item.removed.is_none()) {
        if let Some(number) = item.number.filter(|number| *number < 1_000_000.0) {
            by_number.entry(number as u32).or_insert(&item.id);
        }
    }
    items
        .iter()
        .enumerate()
        .filter_map(|(at, item)| {
            let Some(mut index) = Index::read(item) else {
                return Some((at, Index::rebuild(parser, item, items, None)));
            };
            let mut changed = false;
            for range in std::iter::once(&mut index.body)
                .chain(&mut index.notes)
                .flatten()
            {
                if range.item.is_none()
                    && let Some(id) = by_number
                        .get(&range.number)
                        .filter(|id| **id != item.id && before.get(**id) != Some(&range.number))
                {
                    range.item = Some((*id).to_owned());
                    changed = true;
                }
            }
            changed.then(|| (at, Index::from_ranges(index.body, index.notes)))
        })
        .collect()
}

/// A recovered owner file may be missing an index. Rebuild it in memory until
/// the next write without touching a saved, valid binding.
pub(super) fn recover(parser: &Mentions, items: &mut [Item]) {
    let updates: Vec<_> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| Index::read(item).is_none())
        .map(|(index, item)| (index, Index::rebuild(parser, item, items, None)))
        .collect();
    for (index, rebuilt) in updates {
        let saved = &mut items[index].saved;
        saved.insert("mentionIndex".into(), serde_json::json!(rebuilt));
        saved.remove("mentionLinks");
    }
}
