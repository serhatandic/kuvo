//! What a lead may allow its sessions (answer_ask, PROTOCOL.md "Work"): what its own conversation
//! could do without asking. Its rules are matched part by part as the engine matches each: a command
//! line read as a shell would run it, and a file outside the session's folders also as working
//! outside the project, where it's written and where it really is.

use std::path::{Component, Path, PathBuf};

/// The tools that work on files, whose paths are judged as files.
pub const FILES: &[&str] = &[
    "edit",
    "write",
    "patch",
    "multiedit",
    "read",
    "list",
    "glob",
    "grep",
];

/// A command line's parts as the engine's rules match each (separated by `;`, `&&`, `||`, `|` or a
/// new line): each part's own text; the whole line where it can't be read as plain programs.
pub fn parts_of(line: &str) -> Vec<String> {
    read_line(line).unwrap_or_else(|| vec![line.to_owned()])
}

/// The parts of a line read as plain programs, or none for one a shell would expand or run
/// something besides in (a variable, a group, a redirect, something in the background).
fn read_line(line: &str) -> Option<Vec<String>> {
    let chars: Vec<char> = line.chars().collect();
    let (mut parts, mut words, mut word, mut has) = (Vec::new(), 0usize, String::new(), false);
    let mut quote: Option<char> = None;
    let mut start = 0;
    let at = |i: usize| chars.get(i).copied();
    let text =
        |from: usize, to: usize| chars[from..to].iter().collect::<String>().trim().to_owned();
    // (A part with no words is only blank.)
    let end_part = |at: usize, words: &mut usize, start: usize, parts: &mut Vec<String>| {
        let part = text(start, at);
        if *words > 0 {
            parts.push(part);
        } else if !part.is_empty() {
            return false;
        }
        *words = 0;
        true
    };
    let mut i = 0;
    while i < chars.len() {
        let char = chars[i];
        match quote {
            Some('\'') => {
                if char == '\'' {
                    quote = None;
                } else {
                    word.push(char);
                }
            }
            Some(_) => {
                if char == '"' {
                    quote = None;
                } else if char == '$' || char == '`' {
                    return None;
                } else if char == '\\' && i + 1 < chars.len() {
                    i += 1;
                    word.push(chars[i]);
                } else {
                    word.push(char);
                }
            }
            None => match char {
                '\'' | '"' => {
                    quote = Some(char);
                    has = true;
                }
                '\\' if i + 1 < chars.len() => {
                    i += 1;
                    if chars[i] != '\n' {
                        word.push(chars[i]);
                        has = true;
                    }
                }
                '$' | '`' | '(' | ')' | '{' | '}' | '>' | '<' => return None,
                '~' if !has && !matches!(at(i + 1), Some('/' | ' ' | '\t' | '\n') | None) => {
                    return None;
                }
                '#' if !has => return None,
                '\n' | ';' => {
                    words += usize::from(std::mem::take(&mut has));
                    word.clear();
                    if !end_part(i, &mut words, start, &mut parts) {
                        return None;
                    }
                    start = i + 1;
                }
                '|' | '&' => {
                    let pair = at(i + 1) == Some(char);
                    if char == '&' && !pair {
                        return None;
                    }
                    if char == '|' && at(i + 1) == Some('&') {
                        return None;
                    }
                    words += usize::from(std::mem::take(&mut has));
                    word.clear();
                    if !end_part(i, &mut words, start, &mut parts) {
                        return None;
                    }
                    if pair {
                        i += 1;
                    }
                    start = i + 1;
                }
                char if char.is_whitespace() => {
                    words += usize::from(std::mem::take(&mut has));
                    word.clear();
                }
                char => {
                    word.push(char);
                    has = true;
                }
            },
        }
        i += 1;
    }
    if quote.is_some() {
        return None;
    }
    words += usize::from(has);
    end_part(chars.len(), &mut words, start, &mut parts).then_some(parts)
}

/// A path made whole from `from`, its `.` and `..` worked out, as a shell's would be.
pub fn resolved(from: &Path, given: &str) -> PathBuf {
    let mut whole = PathBuf::new();
    for part in from.join(given).components() {
        match part {
            Component::ParentDir => {
                whole.pop();
            }
            Component::CurDir => {}
            other => whole.push(other),
        }
    }
    whole
}

/// Where a path really is: its nearest folder that's there with the links in it followed, and the
/// rest as written (a file not made yet).
pub fn real_of(full: &Path) -> PathBuf {
    let mut at = full.to_path_buf();
    let mut rest = Vec::new();
    loop {
        if let Ok(real) = std::fs::canonicalize(&at) {
            return rest
                .iter()
                .rev()
                .fold(real, |path, part: &std::ffi::OsString| path.join(part));
        }
        match (at.file_name().map(ToOwned::to_owned), at.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                at = parent.to_path_buf();
            }
            _ => return full.to_path_buf(),
        }
    }
}

/// Whether `full` is `folder` or in it.
pub fn within(full: &Path, folder: &Path) -> bool {
    full.starts_with(folder)
}

#[cfg(test)]
mod tests {
    use super::parts_of;

    #[test]
    fn a_line_is_read_part_by_part_as_a_shell_would_run_it() {
        assert_eq!(
            parts_of("git status && npm test | tee x; ls"),
            ["git status", "npm test", "tee x", "ls"]
        );
        assert_eq!(parts_of("echo 'a; b' \"c\""), ["echo 'a; b' \"c\""]);
        assert_eq!(
            parts_of("echo $HOME"),
            ["echo $HOME"],
            "something the shell fills in: the line"
        );
        assert_eq!(
            parts_of("rm -rf build &"),
            ["rm -rf build &"],
            "in the background: the line"
        );
        assert_eq!(
            parts_of("cat ~bob/x"),
            ["cat ~bob/x"],
            "another user's folder: the line"
        );
        assert_eq!(
            parts_of("ls ~/x # look"),
            ["ls ~/x # look"],
            "a comment: the line"
        );
        assert_eq!(parts_of("ls\n\npwd"), ["ls", "pwd"]);
        assert_eq!(
            parts_of("echo 'open"),
            ["echo 'open"],
            "an unclosed quote: the line"
        );
    }
}
