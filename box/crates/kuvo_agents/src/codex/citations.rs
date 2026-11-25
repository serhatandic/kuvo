//! Codex's directives in its words (`:codex-file-citation{path="…" purpose="output"}`), which a chat
//! that isn't Codex's own would show raw: a file it cites as what it made becomes a file shown in
//! the conversation, one it cites in its words its path, and any other directive goes. Its words
//! come in pieces, so a directive split between them is held back until it's whole.

use regex::Regex;
use std::sync::LazyLock;

/// What its words become: words, or a file it made, to show.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Said {
    Text(String),
    Made(String),
}

const OPENING: &str = ":codex-";
/// Most held back for a directive that doesn't close: past it, the words go as they are.
const LONGEST: usize = 4096;

#[derive(Default)]
pub(super) struct Citations {
    held: String,
}

impl Citations {
    /// What `words` (the next piece) say, with what's held back from before.
    pub fn read(&mut self, words: &str) -> Vec<Said> {
        let text = std::mem::take(&mut self.held) + words;
        let mut said = Vec::new();
        let mut from = 0;
        let mut at = text.find(':');
        while let Some(index) = at {
            let rest = &text[index..(index + OPENING.len()).min(text.len())];
            if rest.len() < OPENING.len() && OPENING.starts_with(rest) {
                // Perhaps a directive's start, cut off: held back for the next piece.
                self.held = text[index..].to_owned();
                push(&mut said, &text[from..index]);
                return said;
            }
            if rest == OPENING {
                match closing(&text, index) {
                    None if text.len() - index <= LONGEST => {
                        self.held = text[index..].to_owned();
                        push(&mut said, &text[from..index]);
                        return said;
                    }
                    Some(Some(end)) => {
                        push(&mut said, &text[from..index]);
                        said.extend(parse(&text[index..=end]));
                        from = end + 1;
                        at = text[from..].find(':').map(|next| next + from);
                        continue;
                    }
                    _ => {}
                }
            }
            at = text[index + 1..].find(':').map(|next| next + index + 1);
        }
        push(&mut said, &text[from..]);
        said
    }

    /// What's still held back as the words end, as it is.
    pub fn end(&mut self) -> Vec<Said> {
        let mut said = Vec::new();
        push(&mut said, &std::mem::take(&mut self.held));
        said
    }
}

fn push(said: &mut Vec<Said>, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(Said::Text(last)) = said.last_mut() {
        last.push_str(text);
    } else {
        said.push(Said::Text(text.to_owned()));
    }
}

/// Where the directive at `at` ends (its `}`, outside quotes): none while it may still be coming,
/// `Some(None)` when what's there isn't one.
fn closing(text: &str, at: usize) -> Option<Option<usize>> {
    let bytes = text.as_bytes();
    let mut index = at + OPENING.len();
    while index < bytes.len() && matches!(bytes[index], b'a'..=b'z' | b'0'..=b'9' | b'-') {
        index += 1;
    }
    if index == bytes.len() {
        return None;
    }
    if bytes[index] != b'{' {
        return Some(None);
    }
    let mut quoted = false;
    index += 1;
    while index < bytes.len() {
        match bytes[index] {
            // One is on one line: past its end, it's words.
            b'\n' => return Some(None),
            b'\\' if quoted => index += 1,
            b'"' => quoted = !quoted,
            b'}' if !quoted => return Some(Some(index)),
            _ => {}
        }
        index += 1;
    }
    None
}

static NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^:codex-([a-z0-9-]+)\{").expect("valid"));
static ATTRIBUTE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"([A-Za-z_][\w-]*)\s*=\s*"((?:[^"\\]|\\.)*)""#).expect("valid"));
static ESCAPE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\(.)").expect("valid"));

/// What a whole directive says: a file citation's file, or nothing for any other.
fn parse(directive: &str) -> Vec<Said> {
    if NAME
        .captures(directive)
        .and_then(|found| found.get(1))
        .map(|name| name.as_str())
        != Some("file-citation")
    {
        return Vec::new();
    }
    let attribute = |key: &str| {
        ATTRIBUTE
            .captures_iter(directive)
            .find(|found| &found[1] == key)
            .map(|found| ESCAPE.replace_all(&found[2], "$1").into_owned())
    };
    let Some(path) = attribute("path").filter(|path| !path.is_empty()) else {
        return Vec::new();
    };
    if attribute("purpose").as_deref() == Some("output") {
        return vec![Said::Made(path)];
    }
    // Its path as code, which the apps make a link to the file.
    let fence = if path.contains('`') { "``" } else { "`" };
    vec![Said::Text(format!("{fence}{path}{fence}"))]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(pieces: &[&str]) -> Vec<Said> {
        let mut reader = Citations::default();
        let mut said: Vec<Said> = Vec::new();
        let read: Vec<Said> = pieces.iter().flat_map(|piece| reader.read(piece)).collect();
        for each in read.into_iter().chain(reader.end()) {
            match (said.last_mut(), each) {
                (Some(Said::Text(last)), Said::Text(text)) => last.push_str(&text),
                (_, each) => said.push(each),
            }
        }
        said
    }

    #[test]
    fn a_file_it_made_shows_and_one_it_cites_is_its_path_however_its_words_come() {
        let words = r#"Made :codex-file-citation{path="/tmp/a.png" purpose="output"} and see :codex-file-citation{path="src/x.rs"}. :codex-other{a="b"}Done"#;
        let expected = vec![
            Said::Text("Made ".into()),
            Said::Made("/tmp/a.png".into()),
            Said::Text(" and see `src/x.rs`. Done".into()),
        ];
        assert_eq!(all(&[words]), expected);
        let pieces: Vec<String> = words.chars().map(String::from).collect();
        let pieces: Vec<&str> = pieces.iter().map(String::as_str).collect();
        assert_eq!(all(&pieces), expected);
    }

    #[test]
    fn what_isnt_a_directive_goes_as_it_is() {
        assert_eq!(
            all(&["time: 10:30 :codex-x\nmore"]),
            vec![Said::Text("time: 10:30 :codex-x\nmore".into())]
        );
        assert_eq!(all(&["ends :cod"]), vec![Said::Text("ends :cod".into())]);
    }
}
