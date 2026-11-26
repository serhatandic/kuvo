//! An ACP agent's tool calls as the app knows them: their name and input, what a permission ask is
//! for, and what a call brought back.

use std::collections::{BTreeSet, HashMap};

use serde_json::{Map, Value, json};

use crate::{pictures::picture_data, session::ToolOutput, tool_relay::TOOL_SERVER_NAME};

/// The most of a call's output kept: its end.
const OUTPUT_LIMIT: usize = 30_000;

/// Kuvo Code's own tools by name, each with the arguments it takes, once they're known. Until then
/// any name of the tool server's is taken as one of them, and a bare name as the agent's own.
#[derive(Clone, Debug, Default)]
pub(crate) struct KnownTools(Option<HashMap<String, BTreeSet<String>>>);

impl KnownTools {
    #[cfg(test)]
    pub fn of(tools: &[(&str, &[&str])]) -> Self {
        Self(Some(
            tools
                .iter()
                .map(|(name, args)| {
                    let args = args.iter().map(|arg| (*arg).to_owned()).collect();
                    ((*name).to_owned(), args)
                })
                .collect(),
        ))
    }

    /// Whether `name` is one of them: unknown (`None`) before they're listed.
    fn has(&self, name: &str) -> Option<bool> {
        self.0.as_ref().map(|tools| tools.contains_key(name))
    }

    /// The app's name for a call an agent names only by the tool's own name, when Kuvo Code has a
    /// tool by that name that takes these arguments (an agent's own tool can share its name).
    fn named(&self, name: &str, args: &Map<String, Value>) -> Option<String> {
        let known = self.0.as_ref()?.get(name)?;
        args.keys()
            .all(|key| known.contains(key))
            .then(|| format!("{TOOL_SERVER_NAME}_{name}"))
    }
}

/// A tool call as the agent last told it: its updates merged field by field.
pub(crate) type Call = Map<String, Value>;

/// Merges an update of a call into what was known of it (fields it leaves out or nulls stay).
pub(crate) fn merge(call: &mut Call, update: &Map<String, Value>) {
    for (key, value) in update {
        if !value.is_null() {
            call.insert(key.clone(), value.clone());
        }
    }
}

fn text<'a>(value: &'a Value) -> Option<&'a str> {
    value.as_str().filter(|text| !text.is_empty())
}

fn first<'a>(values: &[&'a Value]) -> Option<&'a Value> {
    values.iter().copied().find(|value| !value.is_null())
}

pub(crate) fn id(call: &Call) -> &str {
    call.get("toolCallId").and_then(Value::as_str).unwrap_or("")
}

pub(crate) fn kind(call: &Call) -> &str {
    call.get("kind").and_then(Value::as_str).unwrap_or("")
}

pub(crate) fn status(call: &Call) -> &str {
    call.get("status").and_then(Value::as_str).unwrap_or("")
}

pub(crate) fn title(call: &Call) -> &str {
    call.get("title").and_then(Value::as_str).unwrap_or("")
}

fn raw(call: &Call) -> &Value {
    call.get("rawInput").unwrap_or(&Value::Null)
}

/// Antigravity asks its questions as a permission ask on a call named `interaction_…`.
pub(crate) fn is_question(call: &Call) -> bool {
    id(call).starts_with("interaction_")
}

/// The path a call is about, as it names it.
fn path(call: &Call) -> Option<Value> {
    let raw = raw(call);
    first(&[
        &call
            .get("locations")
            .and_then(|locations| locations.get(0))
            .map_or(Value::Null, |location| location["path"].clone()),
        &raw["path"],
        &raw["file_path"],
        &raw["filePath"],
    ])
    .cloned()
}

/// What `title` names in backticks, as Grok names a command it runs.
fn backticked(title: &str) -> Option<&str> {
    let start = title.find('`')? + 1;
    let length = title[start..].find('`')?;
    (length > 0).then(|| &title[start..start + length])
}

fn command(call: &Call) -> Option<Value> {
    let raw = raw(call);
    if let Some(parts) = raw["command"].as_array() {
        let words: Vec<_> = parts
            .iter()
            .map(|part| part.as_str().unwrap_or(""))
            .collect();
        return Some(words.join(" ").into());
    }
    first(&[
        &raw["command"],
        &raw["CommandLine"],
        &raw["command_line"],
        &raw["commandLine"],
    ])
    .cloned()
}

fn with(raw: &Value, extra: impl IntoIterator<Item = (&'static str, Option<Value>)>) -> Value {
    let mut input = raw.as_object().cloned().unwrap_or_default();
    for (key, value) in extra {
        input.insert(key.into(), value.unwrap_or(Value::Null));
    }
    input.retain(|_, value| !value.is_null());
    Value::Object(input)
}

/// The app's name for a call, and its input.
pub(crate) fn kuvo_tool(call: &Call, known: &KnownTools) -> (String, Value) {
    let title = title(call);
    if is_question(call) {
        return (
            "question".into(),
            json!({"questions": [{"question": title}]}),
        );
    }
    let raw = raw(call);
    let described = Some(Value::from(title)).filter(|_| call.contains_key("title"));
    // Grok looking through the tools it has (its SearchTool): the app's tool search.
    if raw["variant"] == "SearchTool" {
        return (
            "tool_search".into(),
            with(&Value::Null, [("query", Some(raw["query"].clone()))]),
        );
    }
    match kind(call) {
        "execute" => {
            let command = command(call)
                .or_else(|| backticked(title).map(Value::from))
                .or_else(|| described.clone());
            let description = Some(raw["description"].clone());
            return (
                "shell".into(),
                with(
                    &Value::Null,
                    [("command", command), ("description", description)],
                ),
            );
        }
        "read" => {
            return (
                "read".into(),
                with(raw, [("path", path(call)), ("description", described)]),
            );
        }
        "edit" | "delete" | "move" => {
            return (
                "edit".into(),
                with(raw, [("path", path(call)), ("description", described)]),
            );
        }
        "search" => {
            let pattern = first(&[&raw["pattern"], &raw["query"]])
                .cloned()
                .or(described.clone());
            let fields = [
                ("pattern", pattern),
                ("path", path(call)),
                ("description", described),
            ];
            return ("grep".into(), with(raw, fields));
        }
        "fetch" => {
            let name = if raw["url"].is_null() {
                "websearch"
            } else {
                "webfetch"
            };
            let query = Some(raw["query"].clone())
                .filter(|query| !query.is_null())
                .or(described);
            return (
                name.into(),
                with(
                    &Value::Null,
                    [("url", Some(raw["url"].clone())), ("query", query)],
                ),
            );
        }
        // (OpenCode names its tool beside the kind: its subagents are `subagent`, with their task.)
        "think" => {
            return match call.get("name").and_then(Value::as_str) {
                Some(name) if !name.is_empty() && name != "think" => (name.into(), raw.clone()),
                _ => ("think".into(), json!({})),
            };
        }
        _ => {}
    }
    // One of Kuvo Code's, named with its server, or with server and tool in its input; its own
    // arguments where the agent wraps them in its call (Grok's `UseTool`).
    let server = first(&[&raw["server"], &raw["server_name"], &raw["serverName"]]);
    let tool = first(&[&raw["tool"], &raw["tool_name"], &raw["toolName"]]).and_then(Value::as_str);
    let wrapped = first(&[
        &raw["arguments"],
        &raw["args"],
        &raw["input"],
        &raw["tool_input"],
        &raw["toolInput"],
    ]);
    let args = wrapped
        .filter(|value| value.is_object())
        .unwrap_or(raw)
        .clone();
    let plain = |name: &str| {
        !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    };
    if server.and_then(Value::as_str) == Some(TOOL_SERVER_NAME)
        && let Some(tool) = tool.filter(|tool| plain(tool))
        && known.has(tool) != Some(false)
    {
        return (format!("{TOOL_SERVER_NAME}_{tool}"), args);
    }
    if let Some(name) = [Some(title), tool]
        .into_iter()
        .flatten()
        .find_map(server_prefixed)
        && known.has(&name) != Some(false)
    {
        return (format!("{TOOL_SERVER_NAME}_{name}"), args);
    }
    // Antigravity names a tool only in its title ("Running show_widget").
    let titled = titled(title);
    if let Some(titled) = titled
        && let Some(raw) = raw.as_object()
        && let Some(own) = known.named(titled, raw)
    {
        return (own, Value::Object(raw.clone()));
    }
    let name = titled.unwrap_or(title).to_lowercase();
    let mut name: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    while name.contains("__") {
        name = name.replace("__", "_");
    }
    let name = name.trim_matches('_');
    let name = if name.is_empty() { "tool" } else { name };
    (name.into(), raw.clone())
}

/// A tool of Kuvo Code's server named with it: `kuvo-browser/show_widget`, `mcp__kuvo-browser__…`,
/// `Tool: kuvo-browser.show_widget`.
fn server_prefixed(text: &str) -> Option<String> {
    let text = text.trim();
    let lower = text.to_ascii_lowercase();
    let at = lower.rfind(TOOL_SERVER_NAME)?;
    let before = &lower[..at];
    let before = before.strip_suffix("mcp__").unwrap_or(before);
    if !(before.is_empty() || before.ends_with([' ', ':', '\t'])) {
        return None;
    }
    let rest = &lower[at + TOOL_SERVER_NAME.len()..];
    let rest = ["__", "/", ".", ":", "_"]
        .iter()
        .find_map(|sep| rest.strip_prefix(sep))?;
    let name = rest.trim_end();
    (!name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        .then(|| name.into())
}

/// A title that is only a tool's name, after "Running", "Tool:" or "Calling".
fn titled(title: &str) -> Option<&str> {
    let title = title.trim();
    let lower = title.to_ascii_lowercase();
    let rest = ["running ", "tool:", "calling "]
        .iter()
        .find_map(|prefix| {
            lower
                .starts_with(prefix)
                .then(|| title[prefix.len()..].trim_start())
        })
        .unwrap_or(title);
    let plain = !rest.is_empty()
        && rest
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'));
    plain.then_some(rest)
}

/// The kinds of access Kuvo Code knows an ask by.
const CLASSIFIED: [&str; 12] = [
    "shell",
    "edit",
    "write",
    "patch",
    "webfetch",
    "websearch",
    "read",
    "grep",
    "glob",
    "list",
    "external_directory",
    "think",
];

/// What an ask is for: by the call's kind, else by its tool.
pub(crate) fn permission_action(call: &Call, name: &str) -> String {
    match kind(call) {
        "execute" => "shell",
        "edit" | "delete" | "move" => "edit",
        "fetch" => "webfetch",
        "read" | "search" => "read",
        _ => name,
    }
    .into()
}

/// Whether Kuvo Code knows the kind of access an ask is for.
pub(crate) fn classified(action: &str) -> bool {
    CLASSIFIED.contains(&action) || action.starts_with(&format!("{TOOL_SERVER_NAME}_"))
}

/// What an ask is about: a command, the paths, or what its input names.
pub(crate) fn resources(call: &Call) -> Vec<String> {
    let raw = raw(call);
    if kind(call) == "execute" {
        let command = command(call)
            .and_then(|value| value.as_str().map(str::to_owned))
            .or_else(|| backticked(title(call)).map(str::to_owned))
            .unwrap_or_else(|| text(&Value::from(title(call))).unwrap_or("*").to_owned());
        return vec![command];
    }
    let paths: Vec<String> = call
        .get("locations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|location| text(&location["path"]).map(str::to_owned))
        .collect();
    if !paths.is_empty() {
        return paths;
    }
    let named = first(&[&raw["path"], &raw["url"], &raw["file_path"]]).and_then(Value::as_str);
    vec![named.unwrap_or("*").to_owned()]
}

/// What was refused, in a few words for the agent.
pub(crate) fn refused_what(action: &str, resources: &[String]) -> String {
    let what = resources
        .iter()
        .filter(|resource| *resource != "*")
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    match (action, what.is_empty()) {
        ("shell", false) => format!("running `{what}`"),
        ("shell", true) => "running that command".into(),
        ("edit", false) => format!("editing {what}"),
        ("edit", true) => "editing files".into(),
        ("webfetch", false) => format!("opening {what}"),
        ("webfetch", true) => "opening that page".into(),
        ("read", false) => format!("reading {what}"),
        ("read", true) => "reading that".into(),
        (_, false) => format!("{action} on {what}"),
        (_, true) => format!("that ({action})"),
    }
}

/// A read Kuvo Code does for the agent (Antigravity's `client_view_file`), not its own
/// `view_file`; one named only by what the model says it does could be either.
pub(crate) fn client_read(call: &Call) -> bool {
    let named = format!(
        "{} {}",
        title(call),
        raw(call)["name"].as_str().unwrap_or("")
    )
    .to_ascii_lowercase();
    named.contains("client_view_file") || !named.contains("view_file")
}

/// A read that hasn't ended.
pub(crate) fn open_read(call: &Call) -> bool {
    kind(call) == "read" && !matches!(status(call), "completed" | "failed")
}

/// The file a read is of, as it names it.
pub(crate) fn read_path(call: &Call) -> Option<String> {
    let raw = raw(call);
    let location = call
        .get("locations")
        .and_then(|locations| locations.get(0))
        .map_or(&Value::Null, |location| &location["path"]);
    first(&[
        location,
        &raw["absolute_path"],
        &raw["path"],
        &raw["file_path"],
        &raw["filePath"],
    ])
    .and_then(text)
    .map(str::to_owned)
}

/// What content says in words.
pub(crate) fn content_text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.into();
    }
    let value = match content["type"].as_str() {
        Some("text") => &content["text"],
        Some("resource_link") => {
            first(&[&content["name"], &content["uri"]]).unwrap_or(&Value::Null)
        }
        Some("resource") => &content["resource"]["text"],
        _ => &Value::Null,
    };
    value.as_str().unwrap_or("").into()
}

/// A picture content carries (one it sends, or a resource's blob).
pub(crate) fn content_picture(content: &Value) -> Option<ToolOutput> {
    match content["type"].as_str()? {
        "image" => picture_data(content["data"].as_str()?, content["mimeType"].as_str()?),
        "resource" => {
            let resource = &content["resource"];
            picture_data(resource["blob"].as_str()?, resource["mimeType"].as_str()?)
        }
        _ => None,
    }
}

/// What a call brought back: its words (the end of them), and pictures.
pub(crate) fn tool_output(call: &Call) -> (String, Vec<ToolOutput>) {
    let mut parts: Vec<String> = Vec::new();
    let mut pictures = Vec::new();
    for part in call
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if part["type"] != "content" {
            continue;
        }
        let content = &part["content"];
        if let Some(picture) = content_picture(content) {
            pictures.push(picture);
        } else if content["type"] == "resource_link" {
            let name = first(&[&content["name"], &content["title"]])
                .and_then(Value::as_str)
                .unwrap_or("Link");
            parts.push(format!("{name}: {}", content["uri"].as_str().unwrap_or("")));
        } else {
            parts.push(content_text(content));
        }
    }
    let raw = call.get("rawOutput").unwrap_or(&Value::Null);
    if parts.iter().all(String::is_empty) && !raw.is_null() {
        parts.clear();
        if let Some(text) = raw.as_str() {
            parts.push(text.into());
        } else {
            let keys = [
                "output_for_prompt",
                "combinedOutput",
                "combined_output",
                "output",
                "stdout",
                "content",
            ];
            if let Some(output) = keys.iter().find_map(|key| text(&raw[*key])) {
                parts.push(output.into());
            }
            for key in ["stderr", "error", "message"] {
                if let Some(said) = text(&raw[key])
                    && !parts.iter().any(|part| part == said)
                {
                    parts.push(said.into());
                }
            }
            if parts.is_empty() && raw.as_object().is_some_and(|raw| !raw.is_empty()) {
                parts.push(serde_json::to_string_pretty(raw).unwrap_or_default());
            }
        }
    }
    let text = parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    (tail(text, OUTPUT_LIMIT), pictures)
}

/// The last `limit` bytes of `text`, at a character's start.
fn tail(text: String, limit: usize) -> String {
    if text.len() <= limit {
        return text;
    }
    let mut at = text.len() - limit;
    while !text.is_char_boundary(at) {
        at += 1;
    }
    text[at..].to_owned()
}

/// A text's first line, short.
pub(crate) fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    if line.chars().count() > 80 {
        format!("{}…", line.chars().take(79).collect::<String>())
    } else {
        line.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(value: Value) -> Call {
        value.as_object().cloned().unwrap_or_default()
    }

    #[test]
    fn names_each_kind_as_the_app_does() {
        let none = KnownTools::default();
        let shell =
            call(json!({"toolCallId":"1","kind":"execute","title":"Run `ls -la`","rawInput":{}}));
        assert_eq!(
            kuvo_tool(&shell, &none),
            ("shell".into(), json!({"command":"ls -la"}))
        );
        let argv = call(
            json!({"kind":"execute","rawInput":{"command":["git","status"],"description":"d"}}),
        );
        assert_eq!(
            kuvo_tool(&argv, &none).1,
            json!({"command":"git status","description":"d"})
        );
        let read = call(
            json!({"kind":"read","title":"Read a","locations":[{"path":"/a"}],"rawInput":{"limit":3}}),
        );
        assert_eq!(
            kuvo_tool(&read, &none),
            (
                "read".into(),
                json!({"limit":3,"path":"/a","description":"Read a"})
            )
        );
        let fetch = call(json!({"kind":"fetch","title":"cats","rawInput":{}}));
        assert_eq!(
            kuvo_tool(&fetch, &none),
            ("websearch".into(), json!({"query":"cats"}))
        );
        let search =
            call(json!({"kind":"other","rawInput":{"variant":"SearchTool","query":"widgets"}}));
        assert_eq!(
            kuvo_tool(&search, &none),
            ("tool_search".into(), json!({"query":"widgets"}))
        );
        let question = call(json!({"toolCallId":"interaction_1","title":"Which?"}));
        assert_eq!(kuvo_tool(&question, &none).0, "question");
    }

    #[test]
    fn kuvo_tools_go_by_the_apps_name_with_their_own_arguments() {
        let none = KnownTools::default();
        let wrapped = call(
            json!({"kind":"other","title":"UseTool","rawInput":{"variant":"UseTool","server":"kuvo-browser","tool_name":"show_widget","tool_input":{"html":"<b>"}}}),
        );
        assert_eq!(
            kuvo_tool(&wrapped, &none),
            ("kuvo-browser_show_widget".into(), json!({"html":"<b>"}))
        );
        let prefixed = call(
            json!({"kind":"other","title":"mcp__kuvo-browser__notify_user","rawInput":{"text":"hi"}}),
        );
        assert_eq!(
            kuvo_tool(&prefixed, &none),
            ("kuvo-browser_notify_user".into(), json!({"text":"hi"}))
        );
        let known = KnownTools::of(&[("show_widget", &["html"])]);
        let titled =
            call(json!({"kind":"other","title":"Running show_widget","rawInput":{"html":"x"}}));
        assert_eq!(kuvo_tool(&titled, &known).0, "kuvo-browser_show_widget");
        // (An agent's own tool by the same name, with other arguments, stays its own.)
        let own =
            call(json!({"kind":"other","title":"Running show_widget","rawInput":{"other":1}}));
        assert_eq!(kuvo_tool(&own, &known).0, "show_widget");
        let odd = call(json!({"kind":"other","title":"Do: Some Thing!"}));
        assert_eq!(kuvo_tool(&odd, &none).0, "do_some_thing");
    }

    #[test]
    fn asks_say_what_they_are_for() {
        let shell = call(json!({"kind":"execute","title":"Run `rm -rf x`"}));
        assert_eq!(permission_action(&shell, "shell"), "shell");
        assert_eq!(resources(&shell), ["rm -rf x"]);
        let edit = call(json!({"kind":"edit","locations":[{"path":"/a"},{"path":"/b"}]}));
        assert_eq!(resources(&edit), ["/a", "/b"]);
        assert_eq!(refused_what("edit", &resources(&edit)), "editing /a, /b");
        assert_eq!(refused_what("shell", &["*".into()]), "running that command");
        assert!(classified("kuvo-browser_show_widget") && !classified("mystery"));
    }

    #[test]
    fn output_is_words_pictures_or_what_it_ran_said() {
        let said = call(json!({"content":[
            {"type":"content","content":{"type":"text","text":"one"}},
            {"type":"content","content":{"type":"image","data":"AAAA","mimeType":"image/png"}},
            {"type":"content","content":{"type":"resource_link","name":"Doc","uri":"file:///d"}},
            {"type":"terminal","terminalId":"t"}]}));
        let (text, pictures) = tool_output(&said);
        assert_eq!(text, "one\nDoc: file:///d");
        assert_eq!(pictures.len(), 1);
        let ran = call(json!({"rawOutput":{"stdout":"out","stderr":"err","exit_code":1}}));
        assert_eq!(tool_output(&ran).0, "out\nerr");
        let long = call(json!({"rawOutput":"é".repeat(20_000)}));
        assert!(tool_output(&long).0.len() <= OUTPUT_LIMIT);
    }

    #[test]
    fn reads_kuvo_code_does_and_the_agents_own() {
        assert!(client_read(&call(json!({"title":"client_view_file"}))));
        assert!(!client_read(&call(json!({"title":"view_file"}))));
        assert!(client_read(&call(json!({"title":"Reading a.txt"}))));
        assert_eq!(
            first_line(&format!("\n  {}\nmore", "x".repeat(90)))
                .chars()
                .count(),
            80
        );
    }
}
