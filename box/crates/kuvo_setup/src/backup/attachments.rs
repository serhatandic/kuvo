//! Rewrite only the attachment fields used by the saved transcript formats.
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};

use serde_json::Value;

use super::{
    Error, Result,
    inventory::stat,
    recovery::{Unavailable, output, record_key},
};

pub(super) struct Addresses {
    roots: BTreeMap<&'static str, PathBuf>,
    placed: BTreeMap<PathBuf, PathBuf>,
    missing: HashSet<String>,
    recovery: PathBuf,
}

impl Addresses {
    pub fn new(saved: Option<&Value>, recovery: &Path) -> Self {
        let mut roots = BTreeMap::new();
        for name in ["attachments", "macAttachments"] {
            if let Some(path) = saved
                .and_then(|saved| saved.get(name))
                .and_then(Value::as_str)
            {
                let path = Path::new(path);
                if path.is_absolute() {
                    roots.insert(name, path.to_owned());
                }
            }
        }
        Self {
            roots,
            placed: BTreeMap::new(),
            missing: HashSet::new(),
            recovery: recovery.into(),
        }
    }

    pub fn add(&mut self, root: &str, name: &str, path: &Path) {
        if let Some(base) = self.roots.get(root) {
            self.placed.insert(base.join(name), path.to_owned());
        }
    }

    pub fn rewrite(
        &mut self,
        value: &mut Value,
        format: Format,
        unavailable: &mut Vec<Unavailable>,
    ) -> Result<bool> {
        let original = value.clone();
        let mut resolve = |path: &Path| -> Result<Option<PathBuf>> {
            if let Some(placed) = self.placed.get(path) {
                return Ok(Some(placed.clone()));
            }
            for (root, base) in &self.roots {
                let Ok(relative) = path.strip_prefix(base) else {
                    continue;
                };
                if relative.as_os_str().is_empty() {
                    continue;
                }
                let entry = format!(
                    "{root}/{}",
                    relative
                        .to_string_lossy()
                        .replace(std::path::MAIN_SEPARATOR, "/")
                );
                if self.missing.insert(entry.clone())
                    && !unavailable.iter().any(|problem| problem.entry == entry)
                {
                    unavailable.push(Unavailable {
                        entry: entry.clone(),
                        reason: "referenced session attachment has no restored copy (missing, expired or moved)".into(),
                    });
                }
                let leaf = relative.file_name().unwrap_or_default().to_string_lossy();
                let missing = output(
                    &self.recovery,
                    &format!("unavailable-attachments/{}/{leaf}", record_key(&[&entry])),
                )?;
                if stat(&missing)?.is_some() {
                    return Err(Error::Message(
                        "An unavailable attachment address is occupied.",
                    ));
                }
                return Ok(Some(missing));
            }
            Ok(None)
        };
        match format {
            Format::Part => part(value, false, &mut BTreeMap::new(), None, &mut resolve)?,
            Format::Prompt => message(value, true, &mut resolve)?,
            Format::Transcript => transcript(value, &mut resolve)?,
        }
        Ok(*value != original)
    }
}

#[derive(Clone, Copy)]
pub(super) enum Format {
    Part,
    Prompt,
    Transcript,
}

type Resolve<'a> = dyn FnMut(&Path) -> Result<Option<PathBuf>> + 'a;

fn file(value: &mut Value, resolve: &mut Resolve<'_>) -> Result<()> {
    let has_source = value
        .get("source")
        .and_then(|source| source.get("uri"))
        .is_some_and(Value::is_string);
    let Some(fields) = (if has_source {
        value.get_mut("source").and_then(Value::as_object_mut)
    } else {
        value.as_object_mut()
    }) else {
        return Ok(());
    };
    let field = if fields.get("uri").is_some_and(Value::is_string) {
        "uri"
    } else {
        "url"
    };
    let Some(given) = fields.get(field).and_then(Value::as_str) else {
        return Ok(());
    };
    let parsed = if given
        .get(..5)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("file:"))
    {
        let Ok(url) = reqwest::Url::parse(given) else {
            return Ok(());
        };
        Some(url)
    } else {
        None
    };
    let path = parsed
        .as_ref()
        .map_or_else(|| Ok(PathBuf::from(given)), reqwest::Url::to_file_path);
    let Ok(path) = path else { return Ok(()) };
    if !path.is_absolute() || path.to_string_lossy().contains(['\0', '\r', '\n']) {
        return Ok(());
    }
    if let Some(target) = resolve(&path)?.filter(|target| *target != path) {
        let replacement = if let Some(parsed) = parsed {
            #[cfg(windows)]
            let target = kuvo_os::platform::windows_conventional_path(&target).unwrap_or(target);
            let Ok(mut url) = reqwest::Url::from_file_path(target) else {
                return Ok(());
            };
            url.set_query(parsed.query());
            url.set_fragment(parsed.fragment());
            url.to_string()
        } else {
            target.to_string_lossy().into_owned()
        };
        fields.insert(field.into(), Value::String(replacement));
    }
    Ok(())
}

const HEADINGS: [&str; 3] = [
    "The attached files are on disk here (what's in them is in this message already):",
    "Open these attached files with your tools if you need them:",
    "These attached files stayed on the user's Mac (they couldn't be read, or copied to this machine):",
];
const UNSENT: &str = " (on the user's Mac: it couldn't be copied to this machine)";

fn block<'a>(
    text: &'a str,
    start: &str,
    end: &str,
    headings: &[&str],
) -> Option<(usize, usize, &'a str)> {
    let body = text.trim_end();
    let body = body.strip_suffix(end)?.strip_suffix('\n')?;
    let opening = format!("{start}\n");
    for (at, _) in body.rmatch_indices(&opening) {
        if at > 0 && body.as_bytes()[at - 1] != b'\n' {
            continue;
        }
        let content = at + opening.len();
        let lines = &body[content..];
        if lines
            .lines()
            .next()
            .is_some_and(|heading| headings.contains(&heading))
        {
            return Some((at, content, lines));
        }
    }
    None
}

fn note(
    value: Option<&mut Value>,
    notes: &mut BTreeMap<String, String>,
    words: Option<&str>,
    resolve: &mut Resolve<'_>,
) -> Result<()> {
    let Some(Value::String(text)) = value else {
        return Ok(());
    };
    let phone = block(
        text,
        "<from-phone>",
        "</from-phone>",
        &["The user sent this from Kuvo Code on their iPhone."],
    )
    .map_or(text.len(), |(at, _, _)| at);
    let body = &text[..phone];
    if words.is_some_and(|words| words == text || body.trim() == words.trim()) {
        return Ok(());
    }
    let Some((_, at, old)) = block(body, "<attached-files>", "</attached-files>", &HEADINGS) else {
        return Ok(());
    };
    let mut stayed = false;
    let mut lines = Vec::new();
    for line in old.split('\n') {
        if HEADINGS.contains(&line) {
            stayed = line == HEADINGS[2];
        } else if !stayed
            && !line.ends_with(UNSENT)
            && Path::new(line).is_absolute()
            && let Some(next) = resolve(Path::new(line))?
        {
            let next = next.to_string_lossy().into_owned();
            notes.insert(line.into(), next.clone());
            lines.push(next);
            continue;
        }
        lines.push(line.to_owned());
    }
    let end = at + old.len();
    text.replace_range(at..end, &lines.join("\n"));
    Ok(())
}

fn part(
    value: &mut Value,
    user: bool,
    notes: &mut BTreeMap<String, String>,
    words: Option<&str>,
    resolve: &mut Resolve<'_>,
) -> Result<()> {
    match value.get("type").and_then(Value::as_str) {
        Some("file") => file(value, resolve),
        Some("tool") => {
            if let Some(content) = value
                .get_mut("state")
                .and_then(|state| state.get_mut("content"))
                .and_then(Value::as_array_mut)
            {
                for output in content {
                    if output.get("type").and_then(Value::as_str) == Some("file") {
                        file(output, resolve)?;
                    }
                }
            }
            Ok(())
        }
        Some("text") if user => note(value.get_mut("text"), notes, words, resolve),
        _ => Ok(()),
    }
}

fn remap_paths(metadata: &mut Value, notes: &BTreeMap<String, String>) {
    if let Some(paths) = metadata
        .pointer_mut("/kuvo/parts/paths")
        .and_then(Value::as_array_mut)
    {
        for path in paths {
            if let Some(new) = path.as_str().and_then(|path| notes.get(path)) {
                *path = Value::String(new.clone());
            }
        }
    }
}

fn message(value: &mut Value, prompt: bool, resolve: &mut Resolve<'_>) -> Result<()> {
    if !prompt
        && matches!(
            value.get("type").and_then(Value::as_str),
            Some("file" | "tool")
        )
    {
        return part(value, false, &mut BTreeMap::new(), None, resolve);
    }
    let info = !prompt && value.get("info").is_some_and(Value::is_object);
    if info && let Some(messages) = value.get_mut("messages").and_then(Value::as_array_mut) {
        for message_value in messages {
            message(message_value, false, resolve)?;
        }
        return Ok(());
    }
    let role = info
        .then(|| value.pointer("/info/role"))
        .flatten()
        .filter(|value| !value.is_null())
        .or_else(|| value.get("role").filter(|value| !value.is_null()))
        .or_else(|| value.get("type"));
    let user = prompt
        || role.and_then(Value::as_str) == Some("user")
        || (role.is_none()
            && value
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| id.starts_with("msg_"))
            && value.get("text").is_some_and(Value::is_string));
    let words = value
        .pointer("/metadata/kuvo/parts/words")
        .or_else(|| {
            info.then(|| value.pointer("/info/metadata/kuvo/parts/words"))
                .flatten()
        })
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut notes = BTreeMap::new();
    if user {
        note(value.get_mut("text"), &mut notes, words.as_deref(), resolve)?;
        if let Some(files) = value.get_mut("files").and_then(Value::as_array_mut) {
            for attachment in files {
                file(attachment, resolve)?;
            }
        }
    }
    if !prompt {
        for field in ["parts", "content"] {
            if let Some(parts) = value.get_mut(field).and_then(Value::as_array_mut) {
                for value in parts {
                    part(value, user, &mut notes, words.as_deref(), resolve)?;
                }
            }
        }
    }
    if user {
        if let Some(metadata) = value.get_mut("metadata") {
            remap_paths(metadata, &notes);
        }
        if info && let Some(metadata) = value.pointer_mut("/info/metadata") {
            remap_paths(metadata, &notes);
        }
    }
    Ok(())
}

fn transcript(value: &mut Value, resolve: &mut Resolve<'_>) -> Result<()> {
    if let Some(rows) = value.as_array_mut() {
        for value in rows {
            transcript(value, resolve)?;
        }
        Ok(())
    } else {
        message(value, false, resolve)
    }
}

pub(super) fn legacy_session(name: &str, value: &Value) -> Option<String> {
    let mut parts = name.strip_prefix("engine/storage/")?.split('/');
    let kind = parts.next()?;
    let parent = parts.next()?;
    let id = parts.next()?.strip_suffix(".json")?;
    if parts.next().is_some()
        || !value.is_object()
        || value
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|saved| saved != id)
    {
        return None;
    }
    let session = match kind {
        "session" => id,
        "message"
            if value
                .get("sessionID")
                .is_none_or(|id| id.as_str() == Some(parent)) =>
        {
            parent
        }
        "part" if value.get("messageID").and_then(Value::as_str) == Some(parent) => {
            value.get("sessionID")?.as_str()?
        }
        _ => return None,
    };
    session
        .strip_prefix("ses_")
        .filter(|id| (1..=95).contains(&id.len()) && super::url_token(id, id.len()))?;
    Some(session.to_owned())
}
