//! Read existing Blocks from the engine settings files without rewriting them.

use super::{Effect, Rule};
use kuvo_store::files;
use serde_json::Value;
use std::{
    io,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy)]
pub enum ConfigLocation {
    Desktop,
    Machine,
}

/// Global settings have the same location the resident hands OpenCode.
/// Project layers are supplied in the engine's application order.
pub async fn read_blocks(
    home: &Path,
    location: ConfigLocation,
    project_layers: Vec<PathBuf>,
) -> io::Result<Vec<Rule>> {
    let home = home.to_owned();
    tokio::task::spawn_blocking(move || {
        let mut rules = Vec::new();
        match location {
            ConfigLocation::Desktop => {
                read(&home.join("config/opencode/opencode.json"), &mut rules)?;
                read(&home.join("config/opencode/opencode.jsonc"), &mut rules)?;
            }
            ConfigLocation::Machine => {
                if !read(&home.join("box/opencode.json"), &mut rules)? {
                    read(&home.join("keys/settings.json"), &mut rules)?;
                }
            }
        }
        for file in project_layers {
            read(&file, &mut rules)?;
        }
        Ok(rules)
    })
    .await?
}

fn read(path: &Path, rules: &mut Vec<Rule>) -> io::Result<bool> {
    let Some(bytes) = files::read(path)? else {
        return Ok(false);
    };
    let info = settings_object(&bytes).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Reading Blocks in {}: {error}", path.display()),
        )
    })?;
    rules.extend(
        info["policies"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|policy| {
                if policy["action"] != "tool.use" {
                    return None;
                }
                let value = policy["resource"].as_str()?;
                let (action, resource) = value.split_once(':').unwrap_or((value, "*"));
                let resource = if resource.is_empty() { "*" } else { resource };
                let effect = match policy["effect"].as_str()? {
                    "allow" => Effect::Allow,
                    "ask" => Effect::Ask,
                    _ => Effect::Deny,
                };
                Some(Rule::new(action, resource, effect))
            }),
    );
    Ok(true)
}

pub(crate) fn settings_object(bytes: &[u8]) -> Result<Value, &'static str> {
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let mut text = bytes.to_vec();
    let mut input = bytes.iter().copied().enumerate().peekable();
    let mut quoted = false;
    while let Some((at, byte)) = input.next() {
        match byte {
            b'"' => quoted = !quoted,
            b'\\' if quoted => {
                input.next();
            }
            b'/' if !quoted => match input.peek().map(|item| item.1) {
                Some(b'/') => {
                    text[at] = b' ';
                    while let Some((position, byte)) = input.next_if(|(_, byte)| *byte != b'\n') {
                        text[position] = if byte == b'\r' { b'\r' } else { b' ' };
                    }
                }
                Some(b'*') => {
                    text[at] = b' ';
                    text[at + 1] = b' ';
                    input.next();
                    let mut closed = false;
                    while let Some((position, byte)) = input.next() {
                        text[position] = if matches!(byte, b'\r' | b'\n') {
                            byte
                        } else {
                            b' '
                        };
                        if byte == b'*'
                            && let Some((last, _)) = input.next_if(|(_, byte)| *byte == b'/')
                        {
                            text[last] = b' ';
                            closed = true;
                            break;
                        }
                    }
                    if !closed {
                        return Err("An opening comment has no end.");
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    let mut previous = None;
    let mut quoted = false;
    let mut at = 0;
    while let Some(&byte) = text.get(at) {
        if byte == b'"' {
            quoted = !quoted;
        } else if quoted && byte == b'\\' {
            at += 2;
            continue;
        }
        if !quoted && !byte.is_ascii_whitespace() {
            if matches!(byte, b']' | b'}')
                && let Some(comma) = previous.filter(|&at| text[at] == b',')
            {
                text[comma] = b' ';
            }
            previous = Some(at);
        }
        at += 1;
    }
    let value: Value =
        serde_json::from_slice(&text).map_err(|_| "The settings file isn't valid JSON.")?;
    if !value.is_object() {
        return Err("The settings file isn't a JSON object.");
    }
    Ok(value)
}
