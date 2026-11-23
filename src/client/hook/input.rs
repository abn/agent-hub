//! The harness hook JSON on stdin, mapped to one neutral input.

use serde_json::Value;

/// The harnesses the CLI wires to the hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Harness {
    Claude,
    Antigravity,
}

impl Harness {
    /// The harness a `--harness` value names.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "claude" => Some(Self::Claude),
            "antigravity" => Some(Self::Antigravity),
            _ => None,
        }
    }

    /// The name used for the `--harness` flag and the state directory.
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Antigravity => "antigravity",
        }
    }

    /// The canonical agent name the hub knows this harness by, sent as
    /// `X-Agent-Id` so the hooks and the harness's own MCP connection act as
    /// one agent.
    pub fn agent(self) -> &'static str {
        match self {
            Self::Claude => "claude-code",
            Self::Antigravity => "antigravity",
        }
    }
}

/// The events a harness can raise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    SessionStart,
    Prompt,
    PreCompact,
    SubagentStart,
    SubagentStop,
    PreToolUse,
    SessionEnd,
}

impl Event {
    /// The event a command line names.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "session-start" => Some(Self::SessionStart),
            "prompt" => Some(Self::Prompt),
            "pre-compact" => Some(Self::PreCompact),
            "subagent-start" => Some(Self::SubagentStart),
            "subagent-stop" => Some(Self::SubagentStop),
            "pre-tool-use" => Some(Self::PreToolUse),
            "session-end" => Some(Self::SessionEnd),
            _ => None,
        }
    }
}

/// What every harness's hook JSON reduces to.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HookInput {
    pub harness_session_id: String,
    pub cwd: Option<String>,
    pub source: Option<String>,
    pub transcript_path: Option<String>,
    pub prompt: Option<String>,
    pub invocation_num: Option<u64>,
    pub agent_type: Option<String>,
    /// The subagent a Claude event belongs to. The main thread has none.
    pub agent_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_input: Option<Value>,
}

fn text(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(key).and_then(Value::as_str))
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// Map the JSON a harness wrote on stdin. Text that is not a JSON object maps
/// to an empty input, which the caller reports and answers with the no-op.
pub fn parse(harness: Harness, stdin: &str) -> HookInput {
    let Ok(value @ Value::Object(_)) = serde_json::from_str::<Value>(stdin) else {
        return HookInput::default();
    };
    match harness {
        Harness::Claude => HookInput {
            harness_session_id: text(&value, &["session_id"]).unwrap_or_default(),
            cwd: text(&value, &["cwd"]),
            source: text(&value, &["source"]),
            transcript_path: text(&value, &["transcript_path"]),
            prompt: text(&value, &["prompt"]),
            invocation_num: None,
            agent_type: text(&value, &["agent_type"]),
            agent_id: text(&value, &["agent_id"]),
            tool_name: text(&value, &["tool_name"]),
            tool_input: value.get("tool_input").filter(|v| v.is_object()).cloned(),
        },
        Harness::Antigravity => HookInput {
            harness_session_id: text(&value, &["conversationId", "conversation_id"])
                .unwrap_or_default(),
            cwd: value
                .get("workspacePaths")
                .and_then(Value::as_array)
                .and_then(|paths| paths.iter().find_map(Value::as_str))
                .map(str::to_string)
                .or_else(|| text(&value, &["cwd"])),
            source: None,
            transcript_path: text(&value, &["transcriptPath", "transcript_path"]),
            prompt: None,
            invocation_num: value
                .get("invocationNum")
                .or_else(|| value.get("invocation_num"))
                .and_then(Value::as_u64),
            agent_type: None,
            agent_id: None,
            tool_name: None,
            tool_input: None,
        },
    }
}
