//! Skill names from completed tool metadata only. Never open the named files or
//! retain their contents, arguments unrelated to skills, or the tool result.
use serde_json::Value;

fn first<'a>(input: &'a Value, names: &[&str]) -> &'a Value {
    names
        .iter()
        .find_map(|name| input.get(*name).filter(|value| !value.is_null()))
        .unwrap_or(&Value::Null)
}
fn skill_path(path: &str) -> Option<&str> {
    let mut parts = path.rsplit(['/', '\\']);
    if !parts.next()?.eq_ignore_ascii_case("SKILL.md") {
        return None;
    }
    let name = parts.next().filter(|name| !name.is_empty())?;
    parts.next()?.eq_ignore_ascii_case("skills").then_some(name)
}
fn named<'a>(tools: impl IntoIterator<Item = (&'a Value, &'a Value)>) -> Vec<String> {
    let mut names = Vec::new();
    for (tool, input) in tools {
        let Some(tool) = tool.as_str() else {
            continue;
        };
        if !input.is_object() {
            continue;
        }
        let name = if tool.eq_ignore_ascii_case("skill") {
            first(input, &["skill", "name"]).as_str()
        } else if tool.eq_ignore_ascii_case("read") || tool.eq_ignore_ascii_case("read_file") {
            first(input, &["file_path", "filePath", "path"])
                .as_str()
                .and_then(skill_path)
        } else {
            None
        };
        if let Some(name) = name.filter(|name| name.encode_utf16().count() <= 200) {
            names.push(name.to_owned());
        }
    }
    names.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    names.dedup();
    names.truncate(32);
    names
}
pub(super) fn portable(message: &Value) -> Vec<String> {
    named(
        message["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|part| part["type"] == "tool" && part["state"]["status"] == "completed")
            .map(|part| (&part["name"], &part["state"]["input"])),
    )
}
pub(super) fn engine(parts: &Value) -> Option<Vec<String>> {
    Some(named(parts.as_array()?.iter().filter_map(|pair| {
        let pair = pair.as_array()?;
        Some((pair.first()?, pair.get(1)?))
    })))
}
