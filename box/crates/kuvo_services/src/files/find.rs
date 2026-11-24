//! Finding files (PROTOCOL.md, `kuvo/files/find`): what a Files tab's filter and a new tab's
//! "Search files or enter a URL" show as you type: the files under a folder whose path there
//! matches the text, best first, and the web address the text reads as, if it reads as one.

use serde::Serialize;

/// How many files an answer names unless asked for fewer or more, and at most.
pub(crate) const FOUND_DEFAULT: usize = 50;
pub(crate) const FOUND_MOST: usize = 200;

/// JavaScript's whitespace (`\s`, `trim`).
fn is_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

/// Whether `letters` come in `text` in that order, with anything between.
fn in_order(text: &str, letters: &[char]) -> bool {
    let mut at = 0;
    for letter in text.chars() {
        if letters.get(at) == Some(&letter) {
            at += 1;
            if at == letters.len() {
                return true;
            }
        }
    }
    letters.is_empty()
}

/// How well `typed` matches a path (lowercased), best first; none where it doesn't.
fn rank(at: &str, text: &str, letters: &[char]) -> Option<u8> {
    let name = at.rsplit('/').next().unwrap_or(at);
    [
        name == text,
        name.starts_with(text),
        name.contains(text),
        at.contains(text),
        in_order(name, letters),
        in_order(at, letters),
    ]
    .iter()
    .position(|matched| *matched)
    .map(|rank| rank as u8)
}

/// Those of `paths` that `typed` matches, best first, `limit` at most: with no regard for case,
/// the text in a file's name (the name, then a name starting with it, then one with it inside),
/// then in its path, then its letters in order in its name, then in its path (spaces left out of
/// those); each the shorter path first, then by path. Blank text matches nothing.
pub(crate) fn rank_files(paths: &[String], typed: &str, limit: usize) -> Vec<String> {
    let text = typed.trim_matches(is_space).to_lowercase();
    if text.is_empty() {
        return Vec::new();
    }
    let letters: Vec<char> = text.chars().filter(|c| !is_space(*c)).collect();
    let mut found: Vec<(u8, &String)> = paths
        .iter()
        .filter_map(|at| rank(&at.to_lowercase(), &text, &letters).map(|rank| (rank, at)))
        .collect();
    // (Lengths and order as JavaScript's strings have them: UTF-16 code units.)
    let units = |at: &str| at.encode_utf16().count();
    found.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| units(a.1).cmp(&units(b.1)))
            .then_with(|| a.1.encode_utf16().cmp(b.1.encode_utf16()))
    });
    found
        .into_iter()
        .take(limit)
        .map(|(_, at)| at.clone())
        .collect()
}

/// The last part of a name that's a file's kind, never a web address's.
const FILE_KINDS: &[&str] = &[
    "bash", "bat", "c", "cc", "cfg", "cjs", "cmd", "conf", "cpp", "cs", "css", "csv", "dart",
    "env", "ex", "exs", "gif", "go", "gql", "gradle", "graphql", "gz", "h", "hpp", "hs", "htm",
    "html", "ini", "ipynb", "java", "jpeg", "jpg", "js", "json", "jsx", "kt", "less", "lock",
    "log", "lua", "md", "mdx", "mjs", "ml", "mod", "nix", "pdf", "php", "pl", "plist", "png",
    "proto", "ps", "py", "rb", "rs", "sass", "scala", "scss", "sh", "sql", "sum", "svelte", "svg",
    "swift", "tar", "tf", "toml", "ts", "tsx", "txt", "vue", "xml", "yaml", "yml", "zig", "zip",
    "zsh",
];
/// The endings a bare name (no port, no path) reads as a web address with.
const WEB_ENDINGS: &[&str] = &[
    "ai", "app", "biz", "blog", "ca", "ch", "cloud", "co", "com", "de", "dev", "edu", "eu", "fm",
    "fr", "gg", "gov", "info", "io", "jp", "ly", "me", "net", "nl", "org", "page", "run", "site",
    "so", "tech", "to", "tv", "uk", "us", "xyz",
];

fn is_ipv4(host: &str) -> bool {
    let parts: Vec<&str> = host.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|part| {
            (1..=3).contains(&part.len())
                && part.bytes().all(|b| b.is_ascii_digit())
                && part.parse::<u16>().is_ok_and(|n| n <= 255)
        })
}

/// A host and an optional port, as `host[:port]` or `[v6][:port]`.
fn host_and_port(text: &str) -> Option<(&str, Option<&str>)> {
    let (host, after) = if text.starts_with('[') {
        let end = text.find(']')?;
        let inside = &text[1..end];
        if inside.is_empty()
            || !inside
                .bytes()
                .all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.')
        {
            return None;
        }
        text.split_at(end + 1)
    } else {
        let end = text.find(':').unwrap_or(text.len());
        let host = &text[..end];
        if host.is_empty() || host.contains(['[', ']']) {
            return None;
        }
        text.split_at(end)
    };
    if after.is_empty() {
        return Some((host, None));
    }
    let port = after.strip_prefix(':')?;
    ((1..=5).contains(&port.len()) && port.bytes().all(|b| b.is_ascii_digit()))
        .then_some((host, Some(port)))
}

fn is_label(label: &str) -> bool {
    let bytes = label.as_bytes();
    let plain = |b: &u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    !bytes.is_empty()
        && plain(&bytes[0])
        && plain(&bytes[bytes.len() - 1])
        && bytes.iter().all(|b| plain(b) || *b == b'-')
}

/// The web address `typed` reads as, with its scheme, or none: with `http://` or `https://` as
/// typed (one that isn't a whole address is none); `localhost` or an IP address, `http://` before
/// it; a name with a dot, `https://`, but never one ending in a file's kind, and a bare name (no
/// port or path) only with a common web ending. Text with a space in it is none.
pub(crate) fn web_address(typed: &str) -> Option<String> {
    let text = typed.trim_matches(is_space);
    if text.is_empty() || text.chars().any(is_space) {
        return None;
    }
    let lower = text.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        let url = url::Url::parse(text).ok()?;
        return url
            .host_str()
            .is_some_and(|host| !host.is_empty())
            .then(|| text.to_owned());
    }
    // (Another scheme, or a word and a colon, `mailto:`, `note:`: not a web page to open.)
    if let Some((scheme, after)) = text.split_once(':') {
        let mut letters = scheme.chars();
        let schemelike = letters.next().is_some_and(|c| c.is_ascii_alphabetic())
            && letters.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'));
        if schemelike && !after.starts_with(|c: char| c.is_ascii_digit()) {
            return None;
        }
    }
    let cut = text.find(['/', '?', '#']).unwrap_or(text.len());
    let (host_port, rest) = text.split_at(cut);
    let (host, port) = host_and_port(host_port)?;
    let host = host.to_lowercase();
    if let Some(port) = port
        && !(1..=65_535).contains(&port.parse::<u32>().unwrap_or(0))
    {
        return None;
    }
    if host == "localhost" || is_ipv4(&host) || host.starts_with('[') {
        return Some(format!("http://{text}"));
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() < 2 || !labels.iter().all(|label| is_label(label)) {
        return None;
    }
    let ending = labels[labels.len() - 1];
    if !(2..=63).contains(&ending.len())
        || !ending.bytes().all(|b| b.is_ascii_lowercase())
        || FILE_KINDS.contains(&ending)
    {
        return None;
    }
    let bare = port.is_none() && !rest.starts_with('/');
    (!bare || WEB_ENDINGS.contains(&ending)).then(|| format!("https://{text}"))
}

/// A file found, by its full path, its name and the folder it's in there ("" at the top).
#[derive(Serialize)]
pub(crate) struct Found {
    path: String,
    name: String,
    folder: String,
}

impl Found {
    pub(crate) fn at(top: &std::path::Path, at: &str) -> Self {
        let (folder, name) = at.rsplit_once('/').unwrap_or(("", at));
        Self {
            // (Part by part, as Node's `path.join` writes it: `\` between them on Windows.)
            path: at
                .split('/')
                .fold(top.to_path_buf(), |path, part| path.join(part))
                .to_string_lossy()
                .into_owned(),
            name: name.to_owned(),
            folder: folder.to_owned(),
        }
    }
}

/// The number of files asked for: a whole number above zero, at most [`FOUND_MOST`].
pub(crate) fn limit(asked: f64) -> usize {
    if asked.fract() == 0.0 && asked > 0.0 {
        asked.min(FOUND_MOST as f64) as usize
    } else {
        FOUND_DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_before_paths_shorter_first() {
        let paths: Vec<String> = [
            "src/sign.ts",
            "sign/a.ts",
            "x/signup.ts",
            "signup.ts",
            "docs/s-i-g-n.md",
            "other.txt",
        ]
        .map(String::from)
        .into();
        assert_eq!(
            rank_files(&paths, " SIGN ", 10),
            [
                "signup.ts",
                "src/sign.ts",
                "x/signup.ts",
                "sign/a.ts",
                "docs/s-i-g-n.md"
            ]
        );
        assert_eq!(rank_files(&paths, "  ", 10), Vec::<String>::new());
        assert_eq!(rank_files(&paths, "sign", 2).len(), 2);
    }

    #[test]
    fn web_addresses_only_where_text_is_one() {
        for (typed, url) in [
            ("example.com", Some("https://example.com")),
            (
                "docs.example.dev:8080/a",
                Some("https://docs.example.dev:8080/a"),
            ),
            ("localhost:3000", Some("http://localhost:3000")),
            ("127.0.0.1", Some("http://127.0.0.1")),
            ("[::1]:80/x", Some("http://[::1]:80/x")),
            ("HTTPS://Kuvo.run/x", Some("HTTPS://Kuvo.run/x")),
            ("http://", None),
            ("kuvo.run", Some("https://kuvo.run")),
            ("foo.bar", None),
            ("foo.bar/x", Some("https://foo.bar/x")),
            ("signup.ts", None),
            ("main.go:12", None),
            ("mailto:me", None),
            ("a b.com", None),
            ("example.com:0", None),
            ("example.com:70000", None),
            ("256.1.1.1", None),
        ] {
            assert_eq!(web_address(typed).as_deref(), url, "{typed}");
        }
    }
}
