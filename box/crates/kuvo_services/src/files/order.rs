//! Names as Finder orders them (`file3` before `file29`): the order JavaScript's
//! `localeCompare(…, { numeric: true, sensitivity: "base" })` gives with the root collation, for
//! names in Latin letters, digits, spaces and ASCII punctuation; case and accents don't count.

use std::cmp::Ordering;

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// ASCII punctuation and symbols, in the root collation's order (all before digits and letters).
const PUNCTUATION: &str = "_-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$";

/// One piece of a name, as the collation weighs it.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Piece {
    Space,
    Mark(usize),
    /// Another symbol, by its code point.
    Symbol(u32),
    /// A run of digits, by its value: its length without leading zeros, then its digits.
    Number(usize, String),
    Letter(char),
}

fn pieces(name: &str) -> Vec<Piece> {
    let folded: Vec<char> = name
        .nfd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .collect();
    let mut out = Vec::with_capacity(folded.len());
    let mut at = 0;
    while at < folded.len() {
        let c = folded[at];
        if c.is_ascii_digit() {
            let start = at;
            while at < folded.len() && folded[at].is_ascii_digit() {
                at += 1;
            }
            let digits: String = folded[start..at].iter().collect();
            let value = digits.trim_start_matches('0').to_owned();
            out.push(Piece::Number(value.len(), value));
            continue;
        }
        out.push(if c.is_whitespace() {
            Piece::Space
        } else if let Some(rank) = PUNCTUATION.find(c) {
            Piece::Mark(rank)
        } else if c.is_alphanumeric() {
            Piece::Letter(c)
        } else {
            Piece::Symbol(c as u32)
        });
        at += 1;
    }
    out
}

/// Two names in Finder's order; equal where only case or accents differ.
pub(crate) fn by_name(a: &str, b: &str) -> Ordering {
    pieces(a).cmp(&pieces(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_by_value_case_and_accents_alike() {
        let mut names = vec![
            "file29", "File3", "b", "É", "a b", "a_b", "a-b", "ab", "10", "9", "Zeta", "e",
        ];
        names.sort_by(|a, b| by_name(a, b));
        assert_eq!(
            names,
            [
                "9", "10", "a b", "a_b", "a-b", "ab", "b", "É", "e", "File3", "file29", "Zeta"
            ]
        );
        assert_eq!(by_name("01", "1"), Ordering::Equal);
    }
}
