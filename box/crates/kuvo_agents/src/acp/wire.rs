//! What ACP carries, read at the edge into Kuvo's own types: a session's updates, what an agent
//! said as it started (`initialize`), and what a session offers (`session/new`'s answer).

use serde_json::{Map, Value, json};

use super::tools::Call;

/// What Kuvo says first: the protocol it speaks and, with `files`, that it reads and writes files
/// for the agent.
pub(crate) fn initialize(files: bool, meta: Option<Value>, more: Map<String, Value>) -> Value {
    let mut capabilities = more;
    capabilities.insert("fs".into(), json!({"readTextFile": files, "writeTextFile": files}));
    capabilities.insert("terminal".into(), false.into());
    json!({
        "protocolVersion": 1,
        "clientCapabilities": capabilities,
        "clientInfo": {"name": "kuvo-code", "title": "Kuvo Code", "version": "1"},
        "_meta": meta,
    })
}

/// A `session/update`: whose, whether a load replays it, and what it says.
#[derive(Clone, Debug)]
pub(crate) struct SessionUpdate {
    pub session: Option<String>,
    pub replay: bool,
    pub update: Update,
}

impl SessionUpdate {
    pub fn read(params: &Value) -> Self {
        Self {
            session: params["sessionId"].as_str().map(str::to_owned),
            replay: params["_meta"]["isReplay"] == true,
            update: Update::read(&params["update"]),
        }
    }
}

/// A to-do entry of the agent's plan.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Todo {
    pub content: Value,
    pub status: Value,
}

#[derive(Clone, Debug)]
pub(crate) enum Update {
    /// Its words, or thinking (`thought`), as content.
    Chunk {
        thought: bool,
        content: Value,
    },
    /// Its words to a subagent's session, as the subagent's task.
    UserChunk(Value),
    /// A tool call, or more of one: its fields as they came.
    Tool(Call),
    /// How full its context is (`used`), how much its model holds (`size`), what it has cost the
    /// conversation so far in dollars.
    Usage {
        size: Option<u64>,
        used: Option<u64>,
        cost: Option<f64>,
    },
    Mode(String),
    ConfigOptions(Value),
    Notice(String),
    /// Summarized: said only once it's done.
    Compacted,
    /// Grok summarizing by itself, from its start to how it went.
    AutoCompactStarted {
        window: Option<u64>,
        used: Option<u64>,
    },
    AutoCompactCompleted {
        after: Option<u64>,
    },
    AutoCompactEnded,
    Plan(Vec<Todo>),
    /// Nothing to show (its commands, the session's title).
    Ignored,
    /// A kind Kuvo doesn't know: its words, if any.
    Unknown {
        kind: String,
        words: String,
    },
}

/// A count of tokens: a whole number, never negative.
/// What a turn used, as a prompt's answer says it (`usage`).
pub fn prompt_usage(answer: &Value) -> Option<crate::session::Usage> {
    let usage = answer.get("usage").filter(|usage| usage.is_object())?;
    let count = |key: &str| tokens(&usage[key]).unwrap_or_default();
    Some(crate::session::Usage {
        input: tokens(&usage["inputTokens"])?,
        output: count("outputTokens"),
        reasoning: count("thoughtTokens"),
        cache_read: count("cachedReadTokens"),
        cache_write: count("cachedWriteTokens"),
        cost: None,
    })
}

/// The title an agent gave its session `id`, from `session/list`'s answer; none while it has only
/// its placeholder ("New session - …").
pub fn listed_title(answer: &Value, id: &str) -> Option<String> {
    let sessions = answer["sessions"].as_array()?;
    let title = sessions.iter().find(|session| session["sessionId"] == id)?["title"].as_str()?.trim();
    let placeholder = ["New session - ", "Child session - "].iter().any(|start| title.starts_with(start));
    (!title.is_empty() && !placeholder).then(|| title.to_owned())
}

fn tokens(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        let float = value.as_f64()?;
        (float >= 0.0 && float.fract() == 0.0 && float < u64::MAX as f64).then_some(float as u64)
    })
}

impl Update {
    pub fn read(update: &Value) -> Self {
        let kind = update["sessionUpdate"].as_str().unwrap_or("");
        match kind {
            "agent_message_chunk" | "agent_thought_chunk" => Self::Chunk {
                thought: kind == "agent_thought_chunk",
                content: update["content"].clone(),
            },
            "user_message_chunk" => Self::UserChunk(update["content"].clone()),
            "tool_call" | "tool_call_update" => {
                Self::Tool(update.as_object().cloned().unwrap_or_default())
            }
            "usage_update" => {
                let cost = &update["cost"];
                let dollars = cost["currency"]
                    .as_str()
                    .is_none_or(|currency| currency == "USD");
                Self::Usage {
                    size: tokens(&update["size"]).filter(|size| *size > 0),
                    used: tokens(&update["used"]),
                    cost: cost["amount"].as_f64().filter(|_| dollars),
                }
            }
            "current_mode_update" => update["currentModeId"]
                .as_str()
                .map_or(Self::Ignored, |mode| Self::Mode(mode.into())),
            "config_option_update" if update["configOptions"].is_array() => {
                Self::ConfigOptions(update["configOptions"].clone())
            }
            "notice" => {
                let said = update["message"]
                    .as_str()
                    .or_else(|| update["text"].as_str());
                said.map_or(Self::Ignored, |said| Self::Notice(said.into()))
            }
            "compaction_update" | "compaction_summary_chunk" => Self::Compacted,
            "auto_compact_started" => Self::AutoCompactStarted {
                window: tokens(&update["context_window"]).filter(|size| *size > 0),
                used: tokens(&update["tokens_used"]),
            },
            "auto_compact_completed" => Self::AutoCompactCompleted {
                after: tokens(&update["tokens_after"]),
            },
            "auto_compact_failed" | "auto_compact_cancelled" => Self::AutoCompactEnded,
            "plan" => Self::Plan(
                update["entries"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|entry| Todo {
                        content: entry["content"].clone(),
                        status: entry["status"].clone(),
                    })
                    .collect(),
            ),
            "available_commands_update" | "session_info_update" | "config_option_update" => {
                Self::Ignored
            }
            _ => {
                let words = super::tools::content_text(&update["content"]);
                let words = if words.trim().is_empty() {
                    update["message"].as_str().unwrap_or("").to_owned()
                } else {
                    words
                };
                Self::Unknown {
                    kind: kind.into(),
                    words: words.trim().into(),
                }
            }
        }
    }

    /// The start of a turn the agent began on its own.
    pub fn begins_work(&self) -> bool {
        matches!(self, Self::Chunk { .. })
            || matches!(self, Self::Tool(call) if call.get("sessionUpdate").and_then(Value::as_str) == Some("tool_call"))
    }
}

/// What an agent said of itself as it started.
#[derive(Clone, Debug, Default)]
pub(crate) struct Capabilities {
    pub auth_methods: Vec<String>,
    pub load: bool,
    pub resume: bool,
    pub close: bool,
    pub http_servers: bool,
    pub logout: bool,
}

impl Capabilities {
    pub fn read(init: &Value) -> Self {
        let agent = &init["agentCapabilities"];
        let session = &agent["sessionCapabilities"];
        Self {
            auth_methods: init["authMethods"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|method| method["id"].as_str().map(str::to_owned))
                .collect(),
            load: agent["loadSession"] == true,
            resume: !session["resume"].is_null(),
            close: !session["close"].is_null(),
            http_servers: agent["mcpCapabilities"]["http"] == true,
            logout: !matches!(agent["auth"]["logout"], Value::Null | Value::Bool(false)),
        }
    }

    /// As kept in a holder's record, for a resident taking the process up.
    pub fn to_value(&self) -> Value {
        serde_json::json!({
            "authMethods": self.auth_methods.iter().map(|id| serde_json::json!({"id": id})).collect::<Vec<_>>(),
            "agentCapabilities": {
                "loadSession": self.load,
                "sessionCapabilities": {
                    "resume": if self.resume { serde_json::json!({}) } else { Value::Null },
                    "close": if self.close { serde_json::json!({}) } else { Value::Null },
                },
                "mcpCapabilities": {"http": self.http_servers},
            },
        })
    }
}

/// What a session offers as it opens (its models, modes and options), and what Kuvo chose of
/// them since (its `kuvo…` fields, never handed to the next resident).
#[derive(Clone, Debug, Default)]
pub(crate) struct Setup(pub Map<String, Value>);

impl Setup {
    pub fn read(answer: &Value) -> Self {
        Self(answer.as_object().cloned().unwrap_or_default())
    }

    pub fn get(&self, key: &str) -> &Value {
        self.0.get(key).unwrap_or(&Value::Null)
    }

    pub fn set(&mut self, key: &str, value: impl Into<Value>) {
        self.0.insert(key.into(), value.into());
    }

    /// The option with this id or category.
    pub fn option(&self, id: &str, category: &str) -> Option<&Value> {
        self.get("configOptions")
            .as_array()?
            .iter()
            .find(|option| option["id"] == id || option["category"] == category)
    }

    /// What the next resident reads of it: what the agent offered, without Kuvo's choices.
    pub fn portable(&self) -> Value {
        let mut offered = self.0.clone();
        offered.retain(|key, _| !key.starts_with("kuvo"));
        Value::Object(offered)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn reads_each_kind_of_update() {
        let read = |value: Value| Update::read(&value);
        assert!(matches!(
            read(
                json!({"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"h"}})
            ),
            Update::Chunk { thought: true, .. }
        ));
        let usage = read(
            json!({"sessionUpdate":"usage_update","size":200000,"used":1500.0,"cost":{"amount":0.5,"currency":"USD"}}),
        );
        assert!(
            matches!(usage, Update::Usage { size: Some(200000), used: Some(1500), cost: Some(c) } if c == 0.5)
        );
        let euros = read(
            json!({"sessionUpdate":"usage_update","used":-1,"cost":{"amount":1,"currency":"EUR"}}),
        );
        assert!(matches!(
            euros,
            Update::Usage {
                used: None,
                cost: None,
                ..
            }
        ));
        assert!(matches!(
            read(json!({"sessionUpdate":"available_commands_update"})),
            Update::Ignored
        ));
        let unknown = read(json!({"sessionUpdate":"weather","message":" sunny "}));
        assert!(
            matches!(unknown, Update::Unknown { ref kind, ref words } if kind == "weather" && words == "sunny")
        );
        assert!(read(json!({"sessionUpdate":"tool_call","toolCallId":"t"})).begins_work());
        assert!(!read(json!({"sessionUpdate":"tool_call_update","toolCallId":"t"})).begins_work());
    }

    #[test]
    fn a_setup_hands_on_only_what_the_agent_offered() {
        let mut setup = Setup::read(
            &json!({"sessionId":"s","configOptions":[{"id":"mode","currentValue":"default"}]}),
        );
        setup.set("kuvoMode", "yolo");
        assert_eq!(
            setup.option("mode", "mode").unwrap()["currentValue"],
            "default"
        );
        assert_eq!(
            setup.portable(),
            json!({"sessionId":"s","configOptions":[{"id":"mode","currentValue":"default"}]})
        );
        let init = json!({"authMethods":[{"id":"oauth-personal"}],"agentCapabilities":{"loadSession":true,"sessionCapabilities":{"resume":{}},"mcpCapabilities":{"http":true}}});
        let capabilities = Capabilities::read(&init);
        assert!(
            capabilities.load
                && capabilities.resume
                && !capabilities.close
                && capabilities.http_servers
        );
        let again = Capabilities::read(&capabilities.to_value());
        assert_eq!(again.auth_methods, ["oauth-personal"]);
        assert!(again.resume && again.load);
    }

    #[test]
    fn a_prompts_usage_and_a_listed_title_are_read_as_said_and_a_placeholder_is_no_title() {
        let usage = prompt_usage(&json!({"stopReason": "end_turn", "usage": {"inputTokens": 100, "outputTokens": 10, "totalTokens": 140, "thoughtTokens": 5, "cachedReadTokens": 25}})).unwrap();
        assert_eq!((usage.input, usage.output, usage.reasoning, usage.cache_read, usage.cache_write), (100, 10, 5, 25, 0));
        assert!(prompt_usage(&json!({"stopReason": "end_turn"})).is_none());
        let listed = json!({"sessions": [
            {"sessionId": "ses_a", "cwd": "/w", "title": "New session - 2026-11-26T05:00:00.000Z"},
            {"sessionId": "ses_b", "cwd": "/w", "title": " Fix the login "}]});
        assert_eq!(listed_title(&listed, "ses_a"), None);
        assert_eq!(listed_title(&listed, "ses_b").as_deref(), Some("Fix the login"));
        assert_eq!(listed_title(&listed, "ses_c"), None);
    }
}
