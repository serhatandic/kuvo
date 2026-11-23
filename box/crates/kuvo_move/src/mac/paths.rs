use crate::{Error, entries};
use kuvo_store::files;
use serde_json::Value;
use std::{fs, path::Path};
use url::Url;

pub(super) struct PathMap(Vec<(String, String)>);

impl PathMap {
    pub(super) fn new<'a>(pairs: impl IntoIterator<Item = (&'a Path, &'a Path)>) -> Self {
        let mut prefixes: Vec<_> = pairs
            .into_iter()
            .filter_map(|(from, to)| Some((from.to_str()?.to_owned(), to.to_str()?.to_owned())))
            .collect();
        prefixes.sort_by_key(|(from, _)| std::cmp::Reverse(from.len()));
        Self(prefixes)
    }

    pub(super) fn absolute(&self, value: &str) -> Option<String> {
        self.0.iter().find_map(|(from, to)| {
            let rest = suffix(value, from)?;
            Some(format!("{to}{rest}"))
        })
    }

    pub(super) fn names_old(&self, value: &str) -> bool {
        let mentions = |prefix: &str| {
            value.match_indices(prefix).any(|(at, _)| {
                let tail = &value[at + prefix.len()..];
                tail.is_empty()
                    || tail.starts_with(['/', '"', '\'', '\n', '\r', '}', '\\', '?', '#'])
            })
        };
        self.0.iter().any(|(from, _)| {
            mentions(from)
                || Url::from_file_path(from)
                    .ok()
                    .and_then(|url| url.as_str().strip_prefix("file://").map(str::to_owned))
                    .is_some_and(|encoded| mentions(&encoded))
        })
    }

    pub(super) fn uri(&self, uri: &str) -> Option<String> {
        let rest = uri.strip_prefix("file://")?;
        self.0.iter().find_map(|(from, to)| {
            if !rest.starts_with('/') {
                return None;
            }
            let encoded = Url::from_file_path(from).ok()?;
            let encoded = encoded.as_str().strip_prefix("file://")?;
            if encoded != from
                && let Some(rest) = suffix_uri(rest, from)
            {
                return Some(format!("file://{to}{rest}"));
            }
            let rest = suffix_uri(rest, encoded)?;
            let encoded = Url::from_file_path(to).ok()?;
            Some(format!("{encoded}{rest}"))
        })
    }

    fn value(&self, value: &str) -> Option<String> {
        if value.contains('\n') {
            return None;
        }
        if let Some(path) = value
            .strip_prefix("{file:")
            .and_then(|path| path.strip_suffix('}'))
        {
            return self.absolute(path).map(|path| format!("{{file:{path}}}"));
        }
        self.uri(value).or_else(|| self.absolute(value))
    }

    pub(super) fn lines(&self, text: &str) -> Option<String> {
        let mut output = String::new();
        let mut copied = 0;
        for (at, _) in text.char_indices() {
            if at < copied {
                continue;
            }
            if at > 0
                && !text[..at]
                    .chars()
                    .next_back()
                    .is_some_and(|ch| ch.is_whitespace() || matches!(ch, '=' | ':' | '"' | '\''))
            {
                continue;
            }
            for (from, to) in &self.0 {
                let Some(rest) = text[at..].strip_prefix(from) else {
                    continue;
                };
                if rest.is_empty() || rest.starts_with(['/', '"', '\'', '\r', '\n']) {
                    output.push_str(&text[copied..at]);
                    output.push_str(to);
                    copied = at + from.len();
                    break;
                }
            }
        }
        if copied == 0 {
            return None;
        }
        output.push_str(&text[copied..]);
        Some(output)
    }

    pub(super) fn json(&self, value: &mut Value) -> bool {
        match value {
            Value::String(text) => self.value(text).is_some_and(|next| {
                *text = next;
                true
            }),
            Value::Array(items) => {
                let mut changed = false;
                for item in items {
                    changed |= self.json(item);
                }
                changed
            }
            Value::Object(fields) => {
                let mut changed = false;
                for (key, mut value) in std::mem::take(fields) {
                    changed |= self.json(&mut value);
                    if let Some(mapped) = self.value(&key) {
                        changed = true;
                        fields.insert(mapped, value);
                    } else {
                        fields.insert(key, value);
                    }
                }
                changed
            }
            _ => false,
        }
    }
}

fn suffix<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    text.strip_prefix(prefix)
        .filter(|rest| rest.is_empty() || rest.starts_with('/'))
}

fn suffix_uri<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    text.strip_prefix(prefix)
        .filter(|rest| rest.is_empty() || rest.starts_with(['/', '?', '#']))
}

/// Relative links and paths outside the move continue to point where they did.
pub(super) fn repoint_links(stage: &Path, map: &PathMap) -> Result<(), Error> {
    fn visit(folder: &Path, map: &PathMap, depth: u8) -> Result<(), Error> {
        for entry in entries(folder)? {
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                let path = entry.path();
                let target = fs::read_link(&path)?;
                if let Some(new) = target.to_str().and_then(|text| map.absolute(text)) {
                    files::repoint_mac_link(&path, Path::new(&new))?;
                }
            } else if kind.is_dir() && depth < 40 {
                visit(&entry.path(), map, depth + 1)?;
            }
        }
        Ok(())
    }

    for entry in entries(stage)? {
        if entry.file_type()?.is_dir()
            && !matches!(entry.file_name().to_str(), Some("bin" | "engine"))
        {
            visit(&entry.path(), map, 0)?;
        }
    }
    Ok(())
}

/// Keep the legacy JSON writer's indentation and terminating newline.
pub(super) fn serialize_json(value: &Value, before: &[u8]) -> serde_json::Result<Vec<u8>> {
    use serde::Serialize;

    let mut output = Vec::new();
    let mut lines = before.split(|byte| *byte == b'\n').skip(1);
    let indent = if lines.clone().any(|line| line.starts_with(b"  \"")) {
        Some(b"  ".as_slice())
    } else if lines.clone().any(|line| line.starts_with(b" \"")) {
        Some(b" ".as_slice())
    } else if lines.any(|line| line.starts_with(b"\t")) {
        Some(b"\t".as_slice())
    } else {
        None
    };
    if let Some(indent) = indent {
        let formatter = serde_json::ser::PrettyFormatter::with_indent(indent);
        value.serialize(&mut serde_json::Serializer::with_formatter(
            &mut output,
            formatter,
        ))?;
    } else {
        value.serialize(&mut serde_json::Serializer::new(&mut output))?;
    }
    if before.ends_with(b"\n") {
        output.push(b'\n');
    }
    Ok(output)
}
