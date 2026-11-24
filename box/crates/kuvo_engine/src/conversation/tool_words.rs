//! Add the resident's tool-step words at the message API boundary.

mod catalog;

use std::{borrow::Cow, collections::BTreeMap};

use regex::{Regex, RegexSet};
use serde::Serialize;
use serde_json::Value;

use crate::{Engine, disk};

const OWN: &str = "kuvo-browser";

pub(crate) struct ToolWords {
    error_tag: Regex,
    error_prefix: Regex,
    reasons: RegexSet,
    exit: Regex,
}

#[derive(Serialize)]
struct Failure<'a> {
    line: String,
    reason: Cow<'a, str>,
    error: String,
}

impl ToolWords {
    pub(crate) fn new() -> Result<Self, regex::Error> {
        Ok(Self {
            error_tag: Regex::new(r"(?i)</?[a-z_]*error>")?,
            error_prefix: Regex::new(
                r"(?i)^(?:mcp error|tool error|error|failed|exception)\s*:\s*",
            )?,
            reasons: RegexSet::new([
                r"user doesn['’]t want to proceed|rejected|declined|(?:user|you) denied|denied by (?:the )?user",
                r"^(?:\[request interrupted|interrupted|stopped|aborted|cancell?ed)(?-u:\b)",
                r"(?-u:\b)blocked(?-u:\b)|not allowed|not permitted|permission denied|(?-u:\b)denied(?-u:\b)|sandbox",
                r"timed out|(?-u:\btimeout\b)",
                r"does not exist|doesn['’]t exist|no such file|(?-u:\bnot found\b)|enoent",
            ])?,
            exit: Regex::new(r"^exit code ([0-9]+)")?,
        })
    }

    pub(crate) async fn fill_page(&self, engine: &Engine, page: &mut Value) {
        let Some(data) = page.get_mut("data") else {
            return;
        };
        let messages = match data {
            Value::Array(rows) => rows.as_mut_slice(),
            one => std::slice::from_mut(one),
        };
        if !messages.iter().any(|row| row["type"] == "assistant") {
            return;
        }
        let relative = if engine.is_mac_box().await {
            "config/opencode/opencode.json"
        } else {
            "box/opencode.json"
        };
        let settings = disk::read::<Value>(&engine.home.join(relative))
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        let servers = Servers::new(&settings);
        for row in messages.iter_mut().filter(|row| row["type"] == "assistant") {
            let Some(content) = row.get_mut("content").and_then(Value::as_array_mut) else {
                continue;
            };
            for step in content.iter_mut().filter(|step| step["type"] == "tool") {
                let Some(name) = step["name"].as_str() else {
                    continue;
                };
                let state = &step["state"];
                let words = servers.words(name, &state["input"]);
                let failure =
                    self.failure(name, state, words.as_ref().map(|(said, _)| said.as_ref()));
                if let Some((said, doing)) = words {
                    step["title"] = Value::String(said.into_owned());
                    step["doing"] = Value::String(doing.into_owned());
                }
                if let Some(failure) = failure {
                    step["failure"] =
                        serde_json::to_value(failure).expect("a tool failure serializes");
                }
            }
        }
    }

    fn failure(&self, name: &str, state: &Value, said: Option<&str>) -> Option<Failure<'static>> {
        if !matches!(state["status"].as_str(), Some("error" | "failed")) {
            return None;
        }
        let raw = [state["error"].as_str(), state["error"]["message"].as_str()]
            .into_iter()
            .flatten()
            .find(|each| !each.trim().is_empty())
            .map(Cow::Borrowed)
            .unwrap_or_else(|| Cow::Owned(printed(state)));
        let unwrapped = self.error_tag.replace_all(&raw, "");
        let mut error = unwrapped.trim();
        while let Some(prefix) = self.error_prefix.find(error) {
            let after = &error[prefix.end()..];
            if after.is_empty() {
                break;
            }
            error = after;
        }
        let reason = self.reason(error, state["error"]["type"].as_str() == Some("aborted"));
        let did = said
            .filter(|said| !said.is_empty())
            .unwrap_or_else(|| catalog::did_once(name));
        Some(Failure {
            line: format!("{did} · {reason}"),
            reason,
            error: if error.is_empty() {
                "It failed without saying why."
            } else {
                error
            }
            .to_owned(),
        })
    }

    fn reason(&self, error: &str, aborted: bool) -> Cow<'static, str> {
        let lower = error.to_lowercase();
        let first = self.reasons.matches(&lower).iter().next();
        Cow::Borrowed(match (first, aborted) {
            (Some(0), _) => "declined",
            (Some(1), _) | (_, true) => "stopped",
            (Some(2), _) => "blocked",
            (Some(3), _) => "timed out",
            (Some(4), _) => "not found",
            _ => {
                return self
                    .exit
                    .find(&lower)
                    .map_or(Cow::Borrowed("failed"), |exit| {
                        Cow::Owned(exit.as_str().to_owned())
                    });
            }
        })
    }
}

fn printed(state: &Value) -> String {
    state["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

type Words = (Cow<'static, str>, Cow<'static, str>);

struct Servers(BTreeMap<String, &'static str>);

impl Servers {
    fn new(settings: &Value) -> Self {
        let mut servers = catalog::MCP
            .iter()
            .map(|&(id, title)| (id.to_owned(), title))
            .collect::<BTreeMap<_, _>>();
        if let Some(given) = settings.pointer("/mcp/servers").and_then(Value::as_object) {
            for id in given
                .keys()
                .filter(|id| !catalog::MCP.iter().any(|(known, _)| *known == id.as_str()))
            {
                let id = id
                    .encode_utf16()
                    .map(|unit| match u8::try_from(unit) {
                        Ok(byte) if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') => {
                            char::from(byte)
                        }
                        _ => '_',
                    })
                    .collect::<String>();
                servers.insert(id, "");
            }
        }
        servers.insert(OWN.to_owned(), "");
        Self(servers)
    }

    fn words(&self, name: &str, input: &Value) -> Option<Words> {
        if name.is_empty() || catalog::BUILT_IN.contains(&name) {
            return None;
        }
        if let Some((said, doing)) = catalog::BRIDGE
            .iter()
            .find_map(|&(id, said, doing)| (id == name).then_some((said, doing)))
        {
            return Some((said.into(), doing.into()));
        }
        let known = self
            .0
            .iter()
            .filter_map(|(server, &title)| {
                let tool = name.strip_prefix(server.as_str())?.strip_prefix('_')?;
                (!tool.is_empty()).then_some((server.as_str(), title, tool))
            })
            .max_by_key(|(server, _, _)| server.len());
        let words = match known {
            Some((OWN, _, tool)) => return Some(own_words(tool, input)),
            Some((server, title, tool)) => {
                let name = if title.is_empty() {
                    humanized(server).into()
                } else {
                    Cow::Borrowed(title)
                };
                format!("{name} · {}", humanized(strip_server(tool, server)))
            }
            None => humanized(name),
        };
        Some((words.clone().into(), words.into()))
    }
}

fn strip_server<'a>(tool: &'a str, server: &str) -> &'a str {
    if let Some((prefix, tail)) = tool.split_at_checked(server.len())
        && prefix.bytes().zip(server.bytes()).all(|(a, b)| {
            a.eq_ignore_ascii_case(&b) || (matches!(a, b'-' | b'_') && matches!(b, b'-' | b'_'))
        })
        && let Some(after) = tail
            .strip_prefix(['-', '_'])
            .filter(|after| !after.is_empty())
    {
        return after;
    }
    tool
}

fn own_words(name: &str, input: &Value) -> Words {
    if let Some(target) = catalog::target(name)
        && let Some(value) = input[target.field].as_str().and_then(target_name)
    {
        let value = if target.quoted {
            format!("“{value}”")
        } else {
            value
        };
        return (
            target.said.replace("{}", &value).into(),
            target.doing.replace("{}", &value).into(),
        );
    }
    if let Some((said, doing)) = catalog::OWN
        .iter()
        .find_map(|&(id, said, doing)| (id == name).then_some((said, doing)))
    {
        return (said.into(), doing.into());
    }
    let plain = humanized(name);
    (plain.clone().into(), plain.into())
}

fn target_name(value: &str) -> Option<String> {
    let value = value.strip_prefix('&').unwrap_or(value);
    let line = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let address = line.strip_prefix("ses_").is_some_and(|suffix| {
        (1..=60).contains(&suffix.len()) && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
    });
    if line.is_empty() || address {
        return None;
    }
    if line.encode_utf16().count() <= 40 {
        return Some(line);
    }
    let mut remaining = 39usize;
    let end = line
        .char_indices()
        .find_map(|(index, ch)| {
            let width = ch.len_utf16();
            if remaining < width {
                Some(index)
            } else {
                remaining -= width;
                None
            }
        })
        .unwrap_or(line.len());
    Some(format!("{}…", line[..end].trim_end()))
}

fn humanized(name: &str) -> String {
    let mut separated = String::with_capacity(name.len());
    let mut can_split = false;
    for ch in name.chars() {
        if can_split && ch.is_ascii_uppercase() {
            separated.push(' ');
        }
        separated.push(ch);
        can_split = ch.is_ascii_lowercase() || ch.is_ascii_digit();
    }
    let words = separated
        .split(|ch: char| ch.is_whitespace() || matches!(ch, '_' | '-' | '.' | ':' | '/'))
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let mut chars = words.chars();
    let Some(first) = chars.next() else {
        return name.to_owned();
    };
    first.to_uppercase().chain(chars).collect()
}
