//! Compare word and punctuation edits while preserving the anchors of accepted insertions.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

const DIFF_CELLS: usize = 1_000_000;
const MAX_DIFF_HUNKS: usize = 1_000;

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Hunk {
    start: usize,
    end: usize,
    text: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pieces: Option<Vec<Piece>>,
}

#[derive(Clone, Deserialize, Serialize)]
struct Piece {
    text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    operation: Option<String>,
}

pub(super) struct Combined {
    pub accepted: String,
    pub proposed: Option<String>,
    pub edits: Vec<Hunk>,
}

pub(super) struct Passages(Regex);

impl Passages {
    pub fn new() -> Self {
        Self(Regex::new(r"[\p{L}\p{N}_]+|[^\p{L}\p{N}_]").expect("passage tokens"))
    }

    fn split<'a>(&self, text: &'a str) -> Vec<&'a str> {
        self.0.find_iter(text).map(|token| token.as_str()).collect()
    }

    pub fn merge(
        &self,
        base: &str,
        accepted: &str,
        proposed: &str,
        layouts: &[&[Hunk]],
        operation: Option<&str>,
    ) -> Combined {
        let source = self.split(base);
        let rendered = self.split(accepted);
        let valid: Vec<_> = layouts
            .iter()
            .copied()
            .filter(|edits| valid(edits, source.len()))
            .collect();
        let exact = valid
            .iter()
            .find(|edits| apply(&source, edits) == accepted)
            .copied();
        let remembered = exact.or_else(|| valid.first().copied());
        let refined = remembered.map(|edits| refine(&source, edits));
        let left = if exact.is_some() {
            refined.expect("layout is present")
        } else if let Some(extended) = refined
            .as_ref()
            .and_then(|edits| self.extend(&source, edits, &rendered, accepted))
        {
            extended
        } else {
            diff(&source, &rendered, &mut DIFF_CELLS.clone())
                .into_iter()
                .map(|edit| {
                    valid
                        .iter()
                        .flat_map(|layout| layout.iter())
                        .find(|prior| prior.pieces.is_some() && edit.same(prior))
                        .cloned()
                        .unwrap_or(edit)
                })
                .collect()
        };
        let right = diff(&source, &self.split(proposed), &mut DIFF_CELLS.clone());
        let mut shared = HashSet::new();
        let mut joined = BTreeMap::new();
        for (index, next) in right.iter().enumerate() {
            let Some(prior) = left.iter().find(|edit| edit.start == next.start) else {
                continue;
            };
            if prior.insertion() && next.insertion() {
                if prior.pieces.is_some() || !prior.same(next) {
                    let mut pieces = prior.pieces();
                    pieces.push(Piece {
                        text: next.wording(),
                        operation: operation.map(str::to_owned),
                    });
                    pieces.sort_by(|a, b| a.text.encode_utf16().cmp(b.text.encode_utf16()));
                    let text = pieces.iter().map(|piece| piece.text.clone()).collect();
                    joined.insert(
                        prior.start,
                        Hunk {
                            text,
                            pieces: Some(pieces),
                            ..prior.clone()
                        },
                    );
                }
                shared.insert(index);
            } else if prior.same(next) {
                shared.insert(index);
            }
        }
        let mut independent = Vec::new();
        let mut all_merged = true;
        for (index, edit) in right
            .iter()
            .enumerate()
            .filter(|(index, _)| !shared.contains(index))
        {
            let _ = index;
            if left.iter().any(|prior| prior.overlaps(edit)) {
                all_merged = false;
                continue;
            }
            let mut edit = edit.clone();
            if edit.insertion()
                && let Some(operation) = operation
            {
                edit.pieces = Some(vec![Piece {
                    text: edit.wording(),
                    operation: Some(operation.into()),
                }]);
            }
            independent.push(edit);
        }
        let mut kept: Vec<_> = left
            .iter()
            .map(|edit| joined.get(&edit.start).unwrap_or(edit).clone())
            .chain(independent)
            .collect();
        kept.sort_by_key(|edit| edit.start);
        let proposed = (!all_merged).then(|| {
            let mut alternate: Vec<_> = right
                .iter()
                .filter(|edit| !joined.contains_key(&edit.start))
                .chain(joined.values())
                .chain(left.iter().filter(|edit| {
                    !joined.contains_key(&edit.start)
                        && !right.iter().any(|other| edit.overlaps(other))
                }))
                .cloned()
                .collect();
            alternate.sort_by_key(|edit| edit.start);
            apply(&source, &alternate)
        });
        Combined {
            accepted: apply(&source, &kept),
            proposed,
            edits: kept,
        }
    }

    fn extend(
        &self,
        source: &[&str],
        prior: &[Hunk],
        rendered: &[&str],
        accepted: &str,
    ) -> Option<Vec<Hunk>> {
        let previous = apply(source, prior);
        let before = self.split(&previous);
        let changes = diff(&before, rendered, &mut DIFF_CELLS.clone());
        let source_offsets = offsets(source);
        let before_offsets = offsets(&before);
        let mut read = 0;
        let mut written = 0;
        let mut spans = Vec::with_capacity(prior.len() + 1);
        for edit in prior {
            let end = written + source_offsets[edit.start] - source_offsets[read];
            spans.push((written..=end, source_offsets[read] as i64 - written as i64));
            read = edit.end;
            written = end
                + edit
                    .text
                    .iter()
                    .map(|text| text.encode_utf16().count())
                    .sum::<usize>();
        }
        let end = written + source_offsets[source.len()] - source_offsets[read];
        spans.push((written..=end, source_offsets[read] as i64 - written as i64));
        let mut combined = prior.to_vec();
        for change in changes {
            let from = before_offsets[change.start];
            let to = before_offsets[change.end];
            let (_, shift) = spans
                .iter()
                .find(|(span, _)| span.contains(&from) && span.contains(&to))?;
            let start = source_offsets
                .binary_search(&((from as i64 + shift) as usize))
                .ok()?;
            let end = source_offsets
                .binary_search(&((to as i64 + shift) as usize))
                .ok()?;
            let edit = Hunk {
                start,
                end,
                ..change
            };
            if prior.iter().any(|other| other.overlaps(&edit)) {
                return None;
            }
            combined.push(edit);
        }
        combined.sort_by_key(|edit| edit.start);
        (valid(&combined, source.len()) && apply(source, &combined) == accepted).then_some(combined)
    }
}

impl Hunk {
    fn insertion(&self) -> bool {
        self.start == self.end
    }

    fn overlaps(&self, other: &Self) -> bool {
        self.start == other.start || (self.start < other.end && other.start < self.end)
    }

    fn wording(&self) -> String {
        self.text.concat()
    }

    fn same(&self, other: &Self) -> bool {
        self.start == other.start && self.end == other.end && self.wording() == other.wording()
    }

    fn pieces(&self) -> Vec<Piece> {
        self.pieces.clone().unwrap_or_else(|| {
            vec![Piece {
                text: self.wording(),
                operation: None,
            }]
        })
    }
}

fn valid(edits: &[Hunk], length: usize) -> bool {
    edits
        .iter()
        .all(|edit| edit.start <= edit.end && edit.end <= length)
        && edits.windows(2).all(|pair| pair[0].end <= pair[1].start)
}

fn apply(source: &[&str], edits: &[Hunk]) -> String {
    let mut text = String::new();
    let mut from = 0;
    for edit in edits {
        for word in &source[from..edit.start] {
            text.push_str(word);
        }
        for word in &edit.text {
            text.push_str(word);
        }
        from = edit.end;
    }
    for word in &source[from..] {
        text.push_str(word);
    }
    text
}

fn offsets(tokens: &[&str]) -> Vec<usize> {
    std::iter::once(0)
        .chain(tokens.iter().scan(0, |sum, word| {
            *sum += word.encode_utf16().count();
            Some(*sum)
        }))
        .collect()
}

#[derive(Clone, Copy)]
struct Span {
    start: usize,
    end: usize,
    next_start: usize,
    next_end: usize,
}

impl Span {
    fn trim(mut self, base: &[&str], next: &[&str]) -> Self {
        while self.start < self.end
            && self.next_start < self.next_end
            && base[self.start] == next[self.next_start]
        {
            self.start += 1;
            self.next_start += 1;
        }
        while self.start < self.end
            && self.next_start < self.next_end
            && base[self.end - 1] == next[self.next_end - 1]
        {
            self.end -= 1;
            self.next_end -= 1;
        }
        self
    }

    fn cells(self) -> usize {
        (self.end - self.start + 1).saturating_mul(self.next_end - self.next_start + 1)
    }

    fn coarse(self, next: &[&str]) -> Hunk {
        Hunk {
            start: self.start,
            end: self.end,
            text: next[self.next_start..self.next_end]
                .iter()
                .map(|s| (*s).into())
                .collect(),
            pieces: None,
        }
    }
}

fn unique<'a>(words: &[&'a str], start: usize, end: usize) -> HashMap<&'a str, Option<usize>> {
    let mut positions = HashMap::new();
    for (i, token) in words.iter().enumerate().take(end).skip(start) {
        positions
            .entry(*token)
            .and_modify(|prior| *prior = None)
            .or_insert(Some(i));
    }
    positions
}

fn anchored(base: &[&str], next: &[&str], span: Span) -> Vec<Span> {
    let a = unique(base, span.start, span.end);
    let b = unique(next, span.next_start, span.next_end);
    let mut spans = Vec::new();
    let mut start = span.start;
    let mut next_start = span.next_start;
    for end in span.start..span.end {
        let token = base[end];
        if !a.get(token).is_some_and(|pos| pos.is_some()) {
            continue;
        }
        let Some(Some(next_end)) = b.get(token).copied() else {
            continue;
        };
        if next_end < next_start {
            return vec![span];
        }
        spans.push(Span {
            start,
            end,
            next_start,
            next_end,
        });
        start = end + 1;
        next_start = next_end + 1;
    }
    spans.push(Span {
        start,
        next_start,
        ..span
    });
    spans
}

fn diff(base: &[&str], next: &[&str], budget: &mut usize) -> Vec<Hunk> {
    let whole = (Span {
        start: 0,
        end: base.len(),
        next_start: 0,
        next_end: next.len(),
    })
    .trim(base, next);
    let spans = if whole.cells() <= DIFF_CELLS
        || whole.start == whole.end
        || whole.next_start == whole.next_end
    {
        vec![whole]
    } else {
        anchored(base, next, whole)
    };
    let mut edits = Vec::new();
    for span in spans {
        let span = span.trim(base, next);
        if span.start == span.end && span.next_start == span.next_end {
            continue;
        }
        let cells = span.cells();
        if span.start == span.end || span.next_start == span.next_end || cells > *budget {
            edits.push(span.coarse(next));
        } else {
            *budget -= cells;
            exact(base, next, span, cells, &mut edits);
        }
        if edits.len() > MAX_DIFF_HUNKS {
            return vec![whole.coarse(next)];
        }
    }
    edits
}

fn exact(base: &[&str], next: &[&str], span: Span, cells: usize, edits: &mut Vec<Hunk>) {
    let Span {
        start,
        end,
        next_start,
        next_end,
    } = span;
    let width = next_end - next_start + 1;
    let mut lcs = vec![0_u32; cells];
    let at = |i, j| (i - start) * width + j - next_start;
    for i in (start..end).rev() {
        for j in (next_start..next_end).rev() {
            lcs[at(i, j)] = if base[i] == next[j] {
                1 + lcs[at(i + 1, j + 1)]
            } else {
                lcs[at(i + 1, j)].max(lcs[at(i, j + 1)])
            };
        }
    }
    let (mut i, mut j) = (start, next_start);
    let mut run = None;
    while i < end || j < next_end {
        if i < end && j < next_end && base[i] == next[j] {
            i += 1;
            j += 1;
            run = None;
            continue;
        }
        let index = *run.get_or_insert_with(|| {
            edits.push(Hunk {
                start: i,
                end: i,
                text: Vec::new(),
                pieces: None,
            });
            edits.len() - 1
        });
        if j < next_end && (i == end || lcs[at(i, j + 1)] >= lcs[at(i + 1, j)]) {
            edits[index].text.push(next[j].into());
            j += 1;
        } else {
            i += 1;
            edits[index].end = i;
        }
    }
}

fn refine(source: &[&str], edits: &[Hunk]) -> Vec<Hunk> {
    let mut budget = DIFF_CELLS;
    let mut refined = Vec::new();
    for edit in edits {
        let width = edit.end - edit.start;
        if width == 0
            || edit.text.is_empty()
            || (width + 1).saturating_mul(edit.text.len() + 1) <= DIFF_CELLS
        {
            refined.push(edit.clone());
        } else {
            let replacement: Vec<_> = edit.text.iter().map(String::as_str).collect();
            refined.extend(
                diff(&source[edit.start..edit.end], &replacement, &mut budget)
                    .into_iter()
                    .map(|hunk| Hunk {
                        start: hunk.start + edit.start,
                        end: hunk.end + edit.start,
                        ..hunk
                    }),
            );
        }
        if refined.len() > MAX_DIFF_HUNKS {
            return edits.to_vec();
        }
    }
    let mut joined: Vec<Hunk> = Vec::new();
    for next in refined {
        if let Some(prior) = joined.last_mut().filter(|prior| prior.overlaps(&next)) {
            if !prior.insertion() || !next.insertion() {
                return edits.to_vec();
            }
            let pieces = prior.pieces().into_iter().chain(next.pieces()).collect();
            prior.text.extend(next.text);
            prior.pieces = Some(pieces);
        } else {
            joined.push(next);
        }
    }
    joined
}
