//! The attachment-bearing fields of OpenCode exports and held prompts.

use std::{
    collections::{BTreeMap, HashMap},
    io,
    path::{Path, PathBuf},
};

use serde_json::Value;
use url::Url;

const KEPT: &str =
    "The attached files are on disk here (what's in them is in this message already):";
const OPEN: &str = "Open these attached files with your tools if you need them:";
const STAYED: &str = "These attached files stayed on the user's Mac (they couldn't be read, or copied to this machine):";
const UNSENT: &str = " (on the user's Mac: it couldn't be copied to this machine)";
const PHONE: &str = "The user sent this from Kuvo Code on their iPhone.";

type Map<'a> = dyn FnMut(&str, bool) -> io::Result<Option<PathBuf>> + 'a;

pub(super) fn visit(
    exports: &mut [Value],
    queues: &mut BTreeMap<String, Value>,
    map: &mut Map<'_>,
) -> io::Result<()> {
    for entry in exports {
        message(entry, false, map)?;
    }
    for queue in queues.values_mut() {
        if let Some(waiting) = queue.get_mut("waiting").and_then(Value::as_array_mut) {
            for entry in waiting {
                if let Some(prompt) = entry.get_mut("message") {
                    message(prompt, true, map)?;
                }
            }
        }
    }
    Ok(())
}

fn message(entry: &mut Value, prompt: bool, map: &mut Map<'_>) -> io::Result<()> {
    if !prompt && matches!(entry["type"].as_str(), Some("file" | "tool")) {
        return part(entry, false, None, &mut HashMap::new(), map);
    }
    if entry["info"].is_object()
        && let Some(messages) = entry.get_mut("messages").and_then(Value::as_array_mut)
    {
        for each in messages {
            message(each, false, map)?;
        }
        return Ok(());
    }
    let role = entry
        .pointer("/info/role")
        .and_then(Value::as_str)
        .or_else(|| entry["role"].as_str())
        .or_else(|| entry["type"].as_str());
    let user = prompt
        || role == Some("user")
        || role.is_none()
            && entry["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("msg_"))
            && entry["text"].is_string();
    let words = entry
        .pointer("/metadata/kuvo/parts/words")
        .or_else(|| entry.pointer("/info/metadata/kuvo/parts/words"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut notes = HashMap::new();
    if user {
        if let Some(text) = entry.get_mut("text") {
            note(text, words.as_deref(), &mut notes, map)?;
        }
        if let Some(files) = entry.get_mut("files").and_then(Value::as_array_mut) {
            for each in files {
                file(each, map)?;
            }
        }
    }
    if !prompt {
        for field in ["parts", "content"] {
            if let Some(parts) = entry.get_mut(field).and_then(Value::as_array_mut) {
                for each in parts {
                    part(each, user, words.as_deref(), &mut notes, map)?;
                }
            }
        }
    }
    if user {
        for field in [
            "/metadata/kuvo/parts/paths",
            "/info/metadata/kuvo/parts/paths",
        ] {
            if let Some(paths) = entry.pointer_mut(field).and_then(Value::as_array_mut) {
                for value in paths {
                    if let Some(replacement) = value.as_str().and_then(|path| notes.get(path)) {
                        *value = Value::String(replacement.clone());
                    }
                }
            }
        }
    }
    Ok(())
}

fn part(
    entry: &mut Value,
    user: bool,
    words: Option<&str>,
    notes: &mut HashMap<String, String>,
    map: &mut Map<'_>,
) -> io::Result<()> {
    match entry["type"].as_str() {
        Some("file") => file(entry, map),
        Some("tool") => {
            if let Some(content) = entry
                .pointer_mut("/state/content")
                .and_then(Value::as_array_mut)
            {
                for each in content.iter_mut().filter(|each| each["type"] == "file") {
                    file(each, map)?;
                }
            }
            Ok(())
        }
        Some("text") if user => {
            if let Some(text) = entry.get_mut("text") {
                note(text, words, notes, map)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn file(entry: &mut Value, map: &mut Map<'_>) -> io::Result<()> {
    let address = if entry.pointer("/source/uri").is_some_and(Value::is_string) {
        entry.get_mut("source").expect("checked source")
    } else {
        entry
    };
    let key = if address["uri"].is_string() {
        "uri"
    } else {
        "url"
    };
    let Some(given) = address[key].as_str() else {
        return Ok(());
    };
    let is_uri = given
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file:"));
    let parsed = is_uri
        .then(|| Url::parse(given))
        .transpose()
        .map_err(|_| invalid())?;
    let path = match &parsed {
        Some(url) => {
            let encoded = url.path().to_ascii_lowercase();
            if encoded.contains("%2f") || cfg!(windows) && encoded.contains("%5c") {
                return Err(invalid());
            }
            if let Ok(local) = url.to_file_path() {
                local
            } else {
                // A received transcript can use a file URL from a different
                // platform. Match it against the certified source inventory.
                let path = percent_encoding::percent_decode_str(url.path())
                    .decode_utf8()
                    .map_err(|_| invalid())?;
                url.host_str().map_or_else(
                    || PathBuf::from(path.as_ref()),
                    |host| PathBuf::from(format!("//{host}{path}")),
                )
            }
        }
        None => PathBuf::from(given),
    };
    let Some(path) = path.to_str().filter(|path| absolute(path)) else {
        return Ok(());
    };
    if path.contains(['\0', '\r', '\n']) {
        return Err(invalid());
    }
    if let Some(target) = map(path, false)? {
        let replacement = if let Some(original) = parsed {
            let mut uri = Url::from_file_path(target).map_err(|_| invalid())?;
            uri.set_query(original.query());
            uri.set_fragment(original.fragment());
            uri.into()
        } else {
            target.to_str().ok_or_else(invalid)?.to_owned()
        };
        address[key] = Value::String(replacement);
    }
    Ok(())
}

fn note(
    value: &mut Value,
    words: Option<&str>,
    notes: &mut HashMap<String, String>,
    map: &mut Map<'_>,
) -> io::Result<()> {
    let Some(text) = value.as_str() else {
        return Ok(());
    };
    let end = block(text, "<from-phone>", "</from-phone>", &[PHONE])
        .map_or(text.len(), |(open, _, _)| open);
    if words.is_some_and(|words| text == words || text[..end].trim() == words.trim()) {
        return Ok(());
    }
    let Some((_, start, finish)) = block(
        &text[..end],
        "<attached-files>",
        "</attached-files>",
        &[KEPT, OPEN, STAYED],
    ) else {
        return Ok(());
    };
    let mut stayed = false;
    let mut lines = Vec::new();
    let mut changed = false;
    for row in text[start..finish].split('\n') {
        if matches!(row, KEPT | OPEN | STAYED) {
            stayed = row == STAYED;
            lines.push(row.to_owned());
            continue;
        }
        let path = row.strip_suffix(UNSENT).unwrap_or(row);
        if absolute(path) {
            if let Some(to) = map(path, stayed || path != row)?
                && !stayed
                && path == row
            {
                let to = to.to_str().ok_or_else(invalid)?.to_owned();
                notes.insert(path.into(), to.clone());
                lines.push(to);
                changed = true;
                continue;
            }
        }
        lines.push(row.to_owned());
    }
    if changed {
        *value = Value::String(format!(
            "{}{}{}",
            &text[..start],
            lines.join("\n"),
            &text[finish..]
        ));
    }
    Ok(())
}

fn block(text: &str, start: &str, end: &str, headings: &[&str]) -> Option<(usize, usize, usize)> {
    let opening = format!("{start}\n");
    let end = format!("\n{end}");
    let before = text.trim_end().strip_suffix(&end)?;
    for (open, _) in before.rmatch_indices(&opening) {
        if open != 0 && before.as_bytes().get(open - 1) != Some(&b'\n') {
            continue;
        }
        let content = open + opening.len();
        if before
            .get(content..)?
            .split('\n')
            .next()
            .is_some_and(|heading| headings.contains(&heading))
        {
            return Some((open, content, before.len()));
        }
    }
    None
}

pub(super) fn absolute(path: &str) -> bool {
    path.starts_with('/')
        || Path::new(path).is_absolute()
        || path.as_bytes().get(..3).is_some_and(|prefix| {
            prefix[0].is_ascii_alphabetic()
                && prefix[1] == b':'
                && matches!(prefix[2], b'/' | b'\\')
        })
        || path.starts_with("\\\\")
}

pub(super) fn portable_source(path: &str) -> String {
    if path.as_bytes().get(..3).is_some_and(|prefix| {
        prefix[0].is_ascii_alphabetic() && prefix[1] == b':' && matches!(prefix[2], b'/' | b'\\')
    }) {
        format!("/{}", path.replace('\\', "/"))
    } else if path.starts_with("\\\\") {
        path.replace('\\', "/")
    } else {
        path.to_owned()
    }
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "A file attachment has an invalid local address.",
    )
}
