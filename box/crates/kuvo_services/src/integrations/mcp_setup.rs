//! MCP server setup snippet parsing and shell command-line splitting/quoting.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerSetup {
    #[serde(skip_serializing_if = "Option::is_none")] pub name: Option<String>,
    pub command: Vec<String>,
    pub environment: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub url: Option<String>,
    pub headers: BTreeMap<String, String>,
}

pub fn command_words(line: &str) -> Vec<String> {
    let (mut words, mut cur, mut quote, mut has, mut esc, mut had) = (Vec::new(), String::new(), None, false, false, false);
    for ch in line.chars() {
        if esc {
            esc = false;
            if matches!(ch, '\n' | '\r') { has = had; continue; }
            if quote == Some('"') && !matches!(ch, '"' | '\\' | '$' | '`') { cur.push('\\'); }
            cur.push(ch);
        } else if ch == '\\' && quote != Some('\'') { (had, esc, has) = (has, true, true); }
        else if let Some(q) = quote { if ch == q { quote = None; } else { cur.push(ch); } }
        else if matches!(ch, '"' | '\'') { (quote, has) = (Some(ch), true); }
        else if ch.is_whitespace() { if has { words.push(std::mem::take(&mut cur)); } has = false; }
        else { cur.push(ch); has = true; }
    }
    if has { words.push(cur); }
    words
}

pub fn quoted_word(word: &str) -> String {
    if !word.is_empty() && !word.chars().any(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '\\')) { word.into() }
    else { format!("\"{}\"", word.replace('\\', "\\\\").replace('"', "\\\"")) }
}
pub fn command_line(command: &[String]) -> String { command.iter().map(|w| quoted_word(w)).collect::<Vec<_>>().join(" ") }

fn lenient_json(text: &str) -> Option<Value> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let (mut stripped, mut in_str, mut i) = (String::with_capacity(text.len()), false, 0);
    while i < chars.len() {
        let (pos, ch) = chars[i];
        if in_str {
            stripped.push(ch);
            if ch == '\\' && i + 1 < chars.len() { i += 1; stripped.push(chars[i].1); } else if ch == '"' { in_str = false; }
            i += 1;
        } else if ch == '"' { in_str = true; stripped.push(ch); i += 1; }
        else if ch == '/' && chars.get(i + 1).is_some_and(|c| c.1 == '/') { while i < chars.len() && chars[i].1 != '\n' { i += 1; } }
        else if ch == '/' && chars.get(i + 1).is_some_and(|c| c.1 == '*') { let end = text[pos + 2..].find("*/")? + pos + 4; while i < chars.len() && chars[i].0 < end { i += 1; } }
        else { stripped.push(ch); i += 1; }
    }
    let chs: Vec<char> = stripped.chars().collect();
    let (mut out, mut in_str, mut j) = (String::with_capacity(stripped.len()), false, 0);
    while j < chs.len() {
        let ch = chs[j];
        if in_str {
            out.push(ch);
            if ch == '\\' && j + 1 < chs.len() { j += 1; out.push(chs[j]); } else if ch == '"' { in_str = false; }
        } else if ch == '"' { in_str = true; out.push(ch); }
        else if ch == ',' {
            let mut k = j + 1;
            while k < chs.len() && chs[k].is_whitespace() { k += 1; }
            if !chs.get(k).is_some_and(|&c| matches!(c, '}' | ']')) { out.push(ch); }
        } else { out.push(ch); }
        j += 1;
    }
    serde_json::from_str(&out).ok()
}

fn json_object(text: &str) -> Option<Map<String, Value>> {
    let wrapped = if text.starts_with('{') { text.to_owned() } else if text.starts_with('"') { format!("{{{}}}", text.strip_suffix(',').unwrap_or(text)) } else { return None };
    match lenient_json(&wrapped)? { Value::Object(m) => Some(m), _ => None }
}
fn as_text(v: &Value) -> Option<String> { match v { Value::String(s) => Some(s.clone()), Value::Number(n) => Some(n.to_string()), Value::Bool(b) => Some(b.to_string()), _ => None } }
fn strings(v: Option<&Value>) -> BTreeMap<String, String> {
    v.and_then(Value::as_object).map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), as_text(v)?))).collect()).unwrap_or_default()
}

fn server(entry: &Map<String, Value>, name: Option<String>) -> Option<ServerSetup> {
    let name = name.filter(|n| !n.is_empty());
    if let Some(u) = entry.get("url").or_else(|| entry.get("serverUrl")).and_then(Value::as_str).filter(|u| !u.is_empty()) {
        return Some(ServerSetup { name, command: Vec::new(), environment: BTreeMap::new(), url: Some(u.into()), headers: strings(entry.get("headers")) });
    }
    let mut command: Vec<String> = match entry.get("command") {
        Some(Value::Array(arr)) if arr.iter().all(Value::is_string) => arr.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect(),
        Some(Value::String(cmd)) if !cmd.is_empty() => {
            let mut list = vec![cmd.clone()];
            if let Some(Value::Array(args)) = entry.get("args") { list.extend(args.iter().filter_map(as_text)); }
            list
        }
        _ => return None,
    };
    if command.is_empty() { return None; }
    if command.len() == 1 && command[0].contains(' ') && !entry.contains_key("args") { command = command_words(&command[0]); }
    Some(ServerSetup { name, command, environment: strings(entry.get("env").or_else(|| entry.get("environment"))), url: None, headers: BTreeMap::new() })
}

fn claude_code_json(line: &str) -> Option<ServerSetup> {
    let mut words: Vec<String> = command_words(line).into_iter().skip(3).collect();
    while words.first().is_some_and(|w| w.starts_with('-')) {
        let drop = if words[0].contains('=') || words[0] == "--client-secret" { 1 } else { 2.min(words.len()) };
        words.drain(..drop);
    }
    (words.len() >= 2).then(|| server(&json_object(&words[1])?, Some(words[0].clone()))).flatten()
}

fn claude_code(line: &str) -> Option<ServerSetup> {
    let mut words = Vec::new();
    for w in command_words(line).into_iter().skip(3) {
        if let Some((k, v)) = w.starts_with("--").then(|| w.split_once('=')).flatten() { words.push(k.into()); words.push(v.into()); } else { words.push(w); }
    }
    let mut setup = ServerSetup { name: None, command: Vec::new(), environment: BTreeMap::new(), url: None, headers: BTreeMap::new() };
    let (mut remote, mut rest, mut i) = (false, Vec::new(), 0);
    while i < words.len() {
        match words[i].as_str() {
            "--" => { setup.command = words[i + 1..].to_vec(); break; }
            "-e" | "--env" => while words.get(i + 1).is_some_and(|w| w.find('=').is_some_and(|p| p > 0)) {
                i += 1;
                let (k, v) = words[i].split_once('=').unwrap();
                setup.environment.insert(k.into(), v.into());
            },
            "-H" | "--header" => {
                if let Some((k, v)) = words.get(i + 1).and_then(|v| v.split_once(':')).filter(|(k, _)| !k.is_empty()) { setup.headers.insert(k.trim().into(), v.trim().into()); }
                i += 1;
            }
            "-t" | "--transport" => { remote = matches!(words.get(i + 1).map(String::as_str), Some("http" | "sse")); i += 1; }
            "-s" | "--scope" => i += 1,
            _ => rest.push(words[i].clone()),
        }
        i += 1;
    }
    setup.name = Some(rest.first()?.clone());
    if remote { setup.url = Some(rest.get(1)?.clone()); } else if setup.command.is_empty() { setup.command = rest[1..].to_vec(); }
    (!setup.command.is_empty() || setup.url.is_some()).then_some(setup)
}

pub fn server_setups(text: &str) -> Vec<ServerSetup> {
    let trimmed = text.trim();
    if trimmed.starts_with("claude mcp add-json ") { return claude_code_json(trimmed).into_iter().collect(); }
    if trimmed.starts_with("claude mcp add ") { return claude_code(trimmed).into_iter().collect(); }
    let Some(object) = json_object(trimmed) else { return Vec::new() };
    let named = |servers: &Map<String, Value>| -> Vec<ServerSetup> {
        let mut keys: Vec<&String> = servers.keys().collect();
        keys.sort();
        keys.into_iter().filter_map(|k| server(servers[k].as_object()?, Some(k.clone()))).collect()
    };
    for key in ["mcpServers", "servers", "mcp", "context_servers"] {
        if let Some(Value::Object(servers)) = object.get(key) { return named(servers); }
    }
    server(&object, None).map_or_else(|| named(&object), |alone| vec![alone])
}

pub fn server_setup(text: &str) -> Option<ServerSetup> { let mut found = server_setups(text); (found.len() == 1).then(|| found.pop().unwrap()) }

pub fn web_address(typed: &str) -> Option<String> {
    let text = typed.trim();
    if text.is_empty() || text.chars().any(char::is_whitespace) { return None; }
    let schemed = text.contains("://");
    let lower = text.to_ascii_lowercase();
    let local = ["localhost", "127.0.0.1", "[::1]"].iter().any(|p| lower.starts_with(p));
    let full = if schemed { text.into() } else { format!("{}{text}", if local { "http://" } else { "https://" }) };
    let parsed = Url::parse(&full).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") { return None; }
    let host = parsed.host_str()?.to_ascii_lowercase();
    if host.is_empty() || (!schemed && !host.contains('.') && host != "localhost" && !host.contains(':') && !host.starts_with('[')) { return None; }
    Some(full)
}
