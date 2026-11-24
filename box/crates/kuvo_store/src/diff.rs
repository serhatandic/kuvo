//! Minimal Myers diff and unified diff helpers matching `box/src/agents/diff.ts`, shared by
//! the artifacts and the coding agents.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpKind {
    Same,
    Removed,
    Added,
}

impl OpKind {
    pub fn prefix(self) -> char {
        match self {
            Self::Same => ' ',
            Self::Removed => '-',
            Self::Added => '+',
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Op<'a> {
    pub kind: OpKind,
    pub line: &'a str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnifiedDiff {
    pub patch: String,
    pub additions: usize,
    pub deletions: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatchCounts {
    pub additions: usize,
    pub deletions: usize,
}

/// Past this many changed lines, what's between the unchanged start and end shows as rewritten.
const SEARCH_LIMIT: usize = 2_000;

/// Shortest edit script from `a` to `b` (Myers), trimming common prefix and suffix first.
pub fn edits<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<Op<'a>> {
    let mut start = 0;
    while start < a.len() && start < b.len() && a[start] == b[start] {
        start += 1;
    }
    let mut end_a = a.len();
    let mut end_b = b.len();
    while end_a > start && end_b > start && a[end_a - 1] == b[end_b - 1] {
        end_a -= 1;
        end_b -= 1;
    }
    let mut ops = Vec::with_capacity(a.len().max(b.len()));
    for &item in &a[..start] {
        ops.push(Op {
            kind: OpKind::Same,
            line: item,
        });
    }
    ops.extend(middle(&a[start..end_a], &b[start..end_b]));
    for &item in &a[end_a..] {
        ops.push(Op {
            kind: OpKind::Same,
            line: item,
        });
    }
    ops
}

fn middle<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<Op<'a>> {
    let n = a.len();
    let m = b.len();
    let all_removed = || {
        a.iter()
            .map(|&line| Op {
                kind: OpKind::Removed,
                line,
            })
            .collect::<Vec<_>>()
    };
    let all_added = || {
        b.iter()
            .map(|&line| Op {
                kind: OpKind::Added,
                line,
            })
            .collect::<Vec<_>>()
    };
    if n == 0 {
        return all_added();
    }
    if m == 0 {
        return all_removed();
    }
    let max = n + m;
    let limit = max.min(SEARCH_LIMIT);
    let offset = max as isize;
    let mut v = vec![0isize; 2 * max + 2];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut found = false;

    'outer: for d in 0..=limit {
        let d_isize = d as isize;
        let lo = (offset - d_isize) as usize;
        let hi = (offset + d_isize + 1) as usize;
        trace.push(v[lo..hi].to_vec());
        let mut k = -d_isize;
        while k <= d_isize {
            let idx = (offset + k) as usize;
            let mut x = if k == -d_isize || (k != d_isize && v[idx - 1] < v[idx + 1]) {
                v[idx + 1]
            } else {
                v[idx - 1] + 1
            };
            let mut y = x - k;
            while (x as usize) < n && (y as usize) < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[idx] = x;
            if x as usize >= n && y as usize >= m {
                found = true;
                break 'outer;
            }
            k += 2;
        }
    }

    if !found {
        let mut out = all_removed();
        out.extend(all_added());
        return out;
    }

    let mut ops = Vec::new();
    let mut x = n as isize;
    let mut y = m as isize;
    for d in (0..trace.len()).rev() {
        let d_isize = d as isize;
        let t = &trace[d];
        let at = |k: isize| t[(k + d_isize) as usize];
        let k = x - y;
        let prev_k = if k == -d_isize || (k != d_isize && at(k - 1) < at(k + 1)) {
            k + 1
        } else {
            k - 1
        };
        let prev_x = if d == 0 { 0 } else { at(prev_k) };
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            x -= 1;
            y -= 1;
            ops.push(Op {
                kind: OpKind::Same,
                line: a[x as usize],
            });
        }
        if d > 0 {
            if x == prev_x {
                y -= 1;
                ops.push(Op {
                    kind: OpKind::Added,
                    line: b[y as usize],
                });
            } else {
                x -= 1;
                ops.push(Op {
                    kind: OpKind::Removed,
                    line: a[x as usize],
                });
            }
        }
    }
    ops.reverse();
    ops
}

pub fn lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut split: Vec<&str> = text.split('\n').collect();
    if split.last() == Some(&"") {
        split.pop();
    }
    split
}

/// Unified diff hunks from `before` to `after`, with `context` lines around each change.
pub fn unified(before: &str, after: &str, context: usize) -> UnifiedDiff {
    let a = lines(before);
    let b = lines(after);
    let ops = edits(&a, &b);
    let mut additions = 0;
    let mut deletions = 0;
    for op in &ops {
        match op.kind {
            OpKind::Added => additions += 1,
            OpKind::Removed => deletions += 1,
            OpKind::Same => {}
        }
    }
    let mut old_line = 1usize;
    let mut new_line = 1usize;
    let mut starts = Vec::with_capacity(ops.len());
    for op in &ops {
        starts.push((old_line, new_line));
        if op.kind != OpKind::Added {
            old_line += 1;
        }
        if op.kind != OpKind::Removed {
            new_line += 1;
        }
    }

    let mut patch = String::new();
    let mut i = 0;
    while i < ops.len() {
        if ops[i].kind == OpKind::Same {
            i += 1;
            continue;
        }
        let start = i.saturating_sub(context);
        let mut end = i;
        while end < ops.len() {
            if ops[end].kind != OpKind::Same {
                end += 1;
                continue;
            }
            let mut next = end;
            while next < ops.len() && ops[next].kind == OpKind::Same {
                next += 1;
            }
            if next < ops.len() && next - end <= context * 2 {
                end = next;
            } else {
                break;
            }
        }
        end = (end + context).min(ops.len());
        let slice = &ops[start..end];
        let old_count = slice.iter().filter(|op| op.kind != OpKind::Added).count();
        let new_count = slice.iter().filter(|op| op.kind != OpKind::Removed).count();
        let old_start = if old_count == 0 {
            starts[start].0.saturating_sub(1)
        } else {
            starts[start].0
        };
        let new_start = if new_count == 0 {
            starts[start].1.saturating_sub(1)
        } else {
            starts[start].1
        };
        use std::fmt::Write;
        let _ = writeln!(
            patch,
            "@@ -{old_start},{old_count} +{new_start},{new_count} @@"
        );
        for op in slice {
            patch.push(op.kind.prefix());
            patch.push_str(op.line);
            patch.push('\n');
        }
        i = end;
    }

    UnifiedDiff {
        patch,
        additions,
        deletions,
    }
}

/// Count added and removed lines in a unified diff patch.
pub fn from_patch(patch: &str) -> PatchCounts {
    let mut additions = 0;
    let mut deletions = 0;
    let mut in_hunk = false;
    for line in patch.split('\n') {
        if line.starts_with("@@") {
            in_hunk = true;
        } else if !in_hunk {
            continue;
        } else if line.starts_with('+') {
            additions += 1;
        } else if line.starts_with('-') {
            deletions += 1;
        }
    }
    PatchCounts {
        additions,
        deletions,
    }
}
