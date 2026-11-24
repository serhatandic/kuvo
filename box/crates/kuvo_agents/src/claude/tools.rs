//! Claude Code's tools as the app knows them: the engine's name for each kind, with its input as
//! the app reads it.

use serde_json::{Map, Value};

/// The app's name and input for a call to Claude Code's `name` with `input`.
pub(super) fn kuvo_tool(name: &str, input: &Value) -> (String, Value) {
    let pick = |fields: &[(&str, &str)]| {
        let mut out = Map::new();
        for (key, from) in fields {
            let value = &input[*from];
            if !value.is_null() {
                out.insert((*key).into(), value.clone());
            }
        }
        out
    };
    let (kind, input) = match name {
        "Bash" => {
            let mut out = pick(&[("command", "command"), ("description", "description")]);
            if input["run_in_background"] == true {
                out.insert("background".into(), true.into());
            }
            ("shell", out)
        }
        "Read" => (
            "read",
            pick(&[("path", "file_path"), ("offset", "offset"), ("limit", "limit")]),
        ),
        "Write" => ("write", pick(&[("path", "file_path"), ("content", "content")])),
        "Edit" => (
            "edit",
            pick(&[
                ("path", "file_path"),
                ("oldString", "old_string"),
                ("newString", "new_string"),
                ("replaceAll", "replace_all"),
            ]),
        ),
        "MultiEdit" => ("edit", pick(&[("path", "file_path"), ("edits", "edits")])),
        "NotebookEdit" => (
            "edit",
            pick(&[("path", "notebook_path"), ("newString", "new_source")]),
        ),
        "Glob" => ("glob", pick(&[("pattern", "pattern"), ("path", "path")])),
        "Grep" => (
            "grep",
            pick(&[("pattern", "pattern"), ("path", "path"), ("glob", "glob")]),
        ),
        "WebFetch" => ("webfetch", pick(&[("url", "url"), ("prompt", "prompt")])),
        "WebSearch" | "web_search" => ("websearch", pick(&[("query", "query")])),
        "web_fetch" => ("webfetch", pick(&[("url", "url")])),
        "code_execution" | "bash_code_execution" => {
            let mut out = Map::new();
            if let Some(command) = [&input["command"], &input["code"]]
                .into_iter()
                .find(|value| !value.is_null())
            {
                out.insert("command".into(), command.clone());
            }
            out.insert("description".into(), "Ran on Anthropic's servers".into());
            ("shell", out)
        }
        "Task" | "Agent" => (
            "subagent",
            pick(&[
                ("description", "description"),
                ("agent", "subagent_type"),
                ("prompt", "prompt"),
            ]),
        ),
        "TodoWrite" => ("todowrite", pick(&[("todos", "todos")])),
        "AskUserQuestion" => ("question", pick(&[("questions", "questions")])),
        "Skill" => {
            let mut out = Map::new();
            if let Some(skill) = [&input["skill"], &input["command"]]
                .into_iter()
                .find(|value| !value.is_null())
            {
                out.insert("name".into(), skill.clone());
            }
            ("skill", out)
        }
        "ExitPlanMode" => ("plan", pick(&[("plan", "plan")])),
        _ => {
            let kind = name
                .strip_prefix("mcp__")
                .and_then(|qualified| qualified.split_once("__"))
                .map_or_else(
                    || name.to_lowercase(),
                    |(server, tool)| format!("{server}_{tool}"),
                );
            return (kind, input.clone());
        }
    };
    (kind.into(), Value::Object(input))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn names_and_reads_each_kind_as_the_app_does() {
        assert_eq!(
            kuvo_tool("Bash", &json!({"command":"ls","run_in_background":true})),
            ("shell".into(), json!({"command":"ls","background":true}))
        );
        assert_eq!(
            kuvo_tool("Edit", &json!({"file_path":"/a","old_string":"x","new_string":"y"})),
            ("edit".into(), json!({"path":"/a","oldString":"x","newString":"y"}))
        );
        assert_eq!(
            kuvo_tool("mcp__files__read__all", &json!({"q":1})),
            ("files_read__all".into(), json!({"q":1}))
        );
        assert_eq!(kuvo_tool("LS", &json!({})).0, "ls");
    }
}
