//! What changed between two versions: word-level prose changes, unified diffs for code,
//! and strict reverse patch application (`unapply_exactly`).

use serde::Serialize;

use super::diff::{OpKind, edits, lines, unified};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Run { pub t: &'static str, pub text: String }

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum DiffLine {
    Same { text: String },
    Added { text: String },
    Removed { text: String },
    Changed { runs: Vec<Run> },
    Folded { lines: usize },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "style", rename_all = "lowercase")]
pub enum Changes {
    Words { lines: Vec<DiffLine>, added: usize, removed: usize },
    Lines { patch: String, added: usize, removed: usize },
}

const AROUND: usize = 3;
const REWRITTEN: f64 = 0.6;
const LEAST_CONTEXT: usize = 3;

pub fn changes(before: &str, after: &str, prose: bool) -> Changes {
    if !prose {
        let diff = unified(before, after, 3);
        return Changes::Lines { patch: diff.patch, added: diff.additions, removed: diff.deletions };
    }
    let (a, b) = (lines(before), lines(after));
    let ops = edits(&a, &b);
    let (mut out, mut added, mut removed, mut i) = (Vec::new(), 0, 0, 0);
    while i < ops.len() {
        if ops[i].kind == OpKind::Same {
            out.push(DiffLine::Same { text: ops[i].line.to_owned() });
            i += 1;
            continue;
        }
        let (mut gone, mut come) = (Vec::new(), Vec::new());
        while i < ops.len() && ops[i].kind != OpKind::Same {
            if ops[i].kind == OpKind::Removed { gone.push(ops[i].line); } else { come.push(ops[i].line); }
            i += 1;
        }
        let pairs = gone.len().min(come.len());
        for p in 0..pairs {
            let runs = words(gone[p], come[p]);
            let kept: usize = runs.iter().filter(|r| r.t == " ").map(|r| r.text.trim().encode_utf16().count()).sum();
            let size = gone[p].trim().encode_utf16().count().min(come[p].trim().encode_utf16().count()).max(1);
            if (kept as f64) / (size as f64) < 1.0 - REWRITTEN {
                out.push(DiffLine::Removed { text: gone[p].to_owned() });
                out.push(DiffLine::Added { text: come[p].to_owned() });
            } else {
                out.push(DiffLine::Changed { runs });
            }
            added += 1;
            removed += 1;
        }
        for &text in &gone[pairs..] { out.push(DiffLine::Removed { text: text.to_owned() }); removed += 1; }
        for &text in &come[pairs..] { out.push(DiffLine::Added { text: text.to_owned() }); added += 1; }
    }
    Changes::Words { lines: fold(out), added, removed }
}

fn is_word_char(c: char) -> bool {
    c.is_alphabetic() || c.is_numeric() || matches!(c, '_' | '\'' | '\u{2019}' | '-')
}

/// Words and the spaces between them, matching `/\s+|[\p{L}\p{N}_'’-]+|[^\s\p{L}\p{N}_'’-]/gu`.
fn tokens(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        let mut end = start + c.len_utf8();
        let pred: Option<fn(char) -> bool> = if c.is_whitespace() {
            Some(char::is_whitespace)
        } else if is_word_char(c) {
            Some(is_word_char)
        } else {
            None
        };
        if let Some(pred) = pred {
            while let Some(&(next_idx, next_c)) = chars.peek() {
                if !pred(next_c) { break; }
                end = next_idx + next_c.len_utf8();
                chars.next();
            }
        }
        out.push(&text[start..end]);
    }
    out
}

pub fn words(before: &str, after: &str) -> Vec<Run> {
    let (a, b) = (tokens(before), tokens(after));
    let ops = edits(&a, &b);
    let mut runs: Vec<Run> = Vec::new();
    let push = |runs: &mut Vec<Run>, t: &'static str, text: &str| {
        if text.is_empty() { return; }
        if let Some(last) = runs.last_mut() && last.t == t { last.text.push_str(text); }
        else { runs.push(Run { t, text: text.to_owned() }); }
    };
    let mut i = 0;
    while i < ops.len() {
        if ops[i].kind == OpKind::Same {
            push(&mut runs, " ", ops[i].line);
            i += 1;
            continue;
        }
        let (mut gone, mut come) = (String::new(), String::new());
        loop {
            while i < ops.len() && ops[i].kind != OpKind::Same {
                if ops[i].kind == OpKind::Removed { gone.push_str(ops[i].line); } else { come.push_str(ops[i].line); }
                i += 1;
            }
            if i + 1 < ops.len()
                && !ops[i].line.is_empty()
                && ops[i].line.chars().all(char::is_whitespace)
                && ops[i + 1].kind != OpKind::Same
            {
                gone.push_str(ops[i].line);
                come.push_str(ops[i].line);
                i += 1;
                continue;
            }
            break;
        }
        push(&mut runs, "-", &gone);
        push(&mut runs, "+", &come);
    }
    runs
}

fn fold(all: Vec<DiffLine>) -> Vec<DiffLine> {
    let (mut out, mut i) = (Vec::new(), 0);
    while i < all.len() {
        if !matches!(all[i], DiffLine::Same { .. }) {
            out.push(all[i].clone());
            i += 1;
            continue;
        }
        let mut end = i;
        while end < all.len() && matches!(all[end], DiffLine::Same { .. }) { end += 1; }
        let (keep_start, keep_end) = (if i == 0 { 0 } else { AROUND }, if end == all.len() { 0 } else { AROUND });
        if end - i > keep_start + keep_end + 1 {
            out.extend_from_slice(&all[i..i + keep_start]);
            out.push(DiffLine::Folded { lines: end - i - keep_start - keep_end });
            out.extend_from_slice(&all[end - keep_end..end]);
        } else {
            out.extend_from_slice(&all[i..end]);
        }
        i = end;
    }
    out
}

#[derive(Default)]
struct Hunk {
    new_start: usize,
    new_count: usize,
    before: Vec<String>,
    after: Vec<String>,
    leading: usize,
    trailing: usize,
    changes: bool,
}

fn parse_hunk_header(line: &str) -> Option<(usize, usize)> {
    let (new_part, _) = line.strip_prefix("@@ -")?.split_once(" +")?.1.split_once(" @@")?;
    match new_part.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((new_part.parse().ok()?, 1)),
    }
}

fn parse(patch: &str) -> Vec<Hunk> {
    let mut hunks: Vec<Hunk> = Vec::new();
    for line in patch.split('\n') {
        if let Some((new_start, new_count)) = parse_hunk_header(line) {
            hunks.push(Hunk { new_start, new_count, ..Default::default() });
            continue;
        }
        let (Some(hunk), Some(first)) = (hunks.last_mut(), line.chars().next()) else { continue };
        let text = line[first.len_utf8()..].to_owned();
        match first {
            ' ' => {
                hunk.before.push(text.clone());
                hunk.after.push(text);
                if hunk.changes { hunk.trailing += 1; } else { hunk.leading += 1; }
            }
            '-' => { hunk.before.push(text); hunk.changes = true; hunk.trailing = 0; }
            '+' => { hunk.after.push(text); hunk.changes = true; hunk.trailing = 0; }
            _ => {}
        }
    }
    hunks
}

/// Reverse-apply `patch` to `text` only when every hunk fits at its exact offset and boundary.
pub fn unapply_exactly(patch: &str, text: &str) -> Option<String> {
    let hunks = parse(patch);
    if hunks.is_empty() { return None; }
    let mut result: Vec<String> = lines(text).into_iter().map(str::to_owned).collect();
    let length = result.len();
    let offset_of = |h: &Hunk| if h.new_count == 0 { Some(h.new_start) } else { h.new_start.checked_sub(1) };
    for hunk in &hunks {
        let at = offset_of(hunk)?;
        if at + hunk.after.len() > length
            || hunk.after.iter().enumerate().any(|(j, line)| result[at + j] != *line)
            || (hunk.leading < LEAST_CONTEXT && at != 0)
            || (hunk.trailing < LEAST_CONTEXT && at + hunk.after.len() != length)
        {
            return None;
        }
    }
    for hunk in hunks.iter().rev() {
        let at = offset_of(hunk)?;
        result.splice(at..at + hunk.after.len(), hunk.before.iter().cloned());
    }
    let ending = if text.ends_with('\n') || text.is_empty() { "\n" } else { "" };
    Some(if result.is_empty() { String::new() } else { format!("{}{}", result.join("\n"), ending) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_diff_marks_changed_words_and_preserves_growth() {
        let diff = changes(
            "# Ada's memory\n\nThe user likes tea in the morning.\nShips on Fridays.\n",
            "# Ada's memory\n\nThe user likes coffee in the morning.\nShips on Fridays.\n",
            true,
        );
        let Changes::Words { lines, .. } = diff else { panic!("expected words") };
        let changed = lines.into_iter().find(|l| matches!(l, DiffLine::Changed { .. })).unwrap();
        assert_eq!(changed, DiffLine::Changed {
            runs: vec![
                Run { t: " ", text: "The user likes ".into() },
                Run { t: "-", text: "tea".into() },
                Run { t: "+", text: "coffee".into() },
                Run { t: " ", text: " in the morning.".into() },
            ]
        });

        let grown = changes(
            "Ships on Fridays.\n",
            "Ships on Fridays. The 0.8 launch date is 12 November; Ben owns the press kit.\n",
            true,
        );
        let Changes::Words { lines, .. } = grown else { panic!("expected words") };
        assert_eq!(lines, vec![DiffLine::Changed {
            runs: vec![
                Run { t: " ", text: "Ships on Fridays.".into() },
                Run { t: "+", text: " The 0.8 launch date is 12 November; Ben owns the press kit.".into() },
            ]
        }]);
    }

    #[test]
    fn unapply_exactly_reconstructs_exact_and_rejects_drifted() {
        let before = "one\ntwo\n";
        let after = "one\ntwo\nthree\n";
        let patch = unified(before, after, 3).patch;
        assert_eq!(unapply_exactly(&patch, after).as_deref(), Some(before));
        assert_eq!(unapply_exactly(&patch, "one\ntwo\nthree\nfour\n"), None);
    }
}
