use std::{
    fs,
    path::{Path, PathBuf},
};

const INVALID: &str = "Give a host name, or user@host. For a port or an IPv6 address, add a Host for the machine to ~/.ssh/config and give its name.";

#[derive(Clone, Eq, PartialEq)]
pub(super) struct Address(String);

impl Address {
    pub(super) fn read(text: &str) -> Result<Self, &'static str> {
        let text = text.trim_matches([' ', '\t']);
        if text.is_empty() {
            return Err("Give a host name, or user@host.");
        }
        if text.encode_utf16().count() > 320 {
            return Err("That's too long for a host's name.");
        }
        if text.starts_with(['-', '@'])
            || text.ends_with('@')
            || text.bytes().filter(|b| *b == b'@').count() > 1
            || !text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._@%+=,~-".contains(&b))
        {
            return Err(INVALID);
        }
        if text.eq_ignore_ascii_case("this-mac.localhost") {
            return Err("That’s this computer.");
        }
        Ok(Self(text.into()))
    }

    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

pub(super) fn suggestions(home: &Path, taken: &[String]) -> Vec<String> {
    let mut hosts = Vec::new();
    read_hosts(&home.join(".ssh/config"), home, &mut hosts, 0);
    hosts
        .into_iter()
        .filter(|host| !taken.iter().any(|saved| saved.eq_ignore_ascii_case(host)))
        .take(50)
        .collect()
}

fn read_hosts(path: &Path, home: &Path, hosts: &mut Vec<String>, depth: usize) {
    if depth >= 4 || hosts.len() >= 500 || !fs::metadata(path).is_ok_and(|m| m.len() <= 1_000_000) {
        return;
    }
    let Ok(text) = fs::read(path) else { return };
    for line in String::from_utf8_lossy(&text).lines() {
        let mut words = line
            .trim()
            .split([' ', '\t', '='])
            .filter(|word| !word.is_empty());
        let Some(keyword) = words.next() else {
            continue;
        };
        let words = words.take_while(|word| !word.starts_with('#')).map(|word| {
            word.strip_prefix('"')
                .and_then(|word| word.strip_suffix('"'))
                .unwrap_or(word)
        });
        if keyword.eq_ignore_ascii_case("host") {
            for word in words {
                if Address::read(word).is_ok() && !hosts.iter().any(|host| host == word) {
                    hosts.push(word.into());
                }
            }
        } else if keyword.eq_ignore_ascii_case("include") {
            for word in words {
                for file in includes(word, home) {
                    read_hosts(&file, home, hosts, depth + 1);
                }
            }
        }
    }
}

fn includes(pattern: &str, home: &Path) -> Vec<PathBuf> {
    let path = if let Some(relative) = pattern.strip_prefix("~/") {
        home.join(relative)
    } else {
        home.join(".ssh").join(pattern)
    };
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Vec::new();
    };
    if !name.contains(['*', '?']) {
        return vec![path];
    }
    let Some(folder) = path
        .parent()
        .filter(|p| !p.to_string_lossy().contains(['*', '?']))
    else {
        return Vec::new();
    };
    let mut matched: Vec<_> = fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|word| matches(name, word))
        })
        .map(|entry| entry.path())
        .collect();
    matched.sort();
    matched.truncate(100);
    matched
}

fn matches(pattern: &str, text: &str) -> bool {
    let mut possibilities = vec![false; text.len() + 1];
    possibilities[0] = true;
    for part in pattern.bytes() {
        if part == b'*' {
            for index in 1..possibilities.len() {
                possibilities[index] |= possibilities[index - 1];
            }
        } else {
            for (index, byte) in text.bytes().enumerate().rev() {
                possibilities[index + 1] = possibilities[index] && (part == b'?' || part == byte);
            }
            possibilities[0] = false;
        }
    }
    possibilities[text.len()]
}
