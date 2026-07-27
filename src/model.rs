use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMeta {
    /// The rollout's own thread identifier.
    pub id: String,
    /// Codex's session/root input. For current subagent rollouts this can differ
    /// from `id`.
    pub session_id: String,
    pub parent_thread_id: Option<String>,
    pub forked_from_id: Option<String>,
    /// Best direct lineage input: explicit parent, then fork source.
    pub parent_id: Option<String>,
    /// Best root input available without opening another rollout.
    pub root_id: String,
    pub timestamp: Option<String>,
    pub cwd: Option<String>,
    pub originator: Option<String>,
    pub cli_version: Option<String>,
    pub source: Option<Value>,
    pub thread_source: String,
    pub model_provider: Option<String>,
    pub history_mode: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
    Tool,
    Metadata,
    Developer,
    System,
    Unknown,
}

impl Role {
    pub fn from_wire(value: Option<&str>) -> Self {
        match value {
            Some("user") => Self::User,
            Some("assistant") => Self::Assistant,
            Some("tool") => Self::Tool,
            Some("developer") => Self::Developer,
            Some("system") => Self::System,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    TurnContext,
    UserMessage,
    AgentMessage,
    FinalAnswer,
    ResponseMessage,
    ToolCall,
    TaskComplete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub sequence: u64,
    pub line: u64,
    pub byte_offset: u64,
    pub timestamp: Option<String>,
    pub turn_id: Option<String>,
    pub role: Role,
    pub kind: EventKind,
    pub text: String,
    pub text_truncated: bool,
    pub phase: Option<String>,
    pub model: Option<String>,
    pub status: Option<String>,
    pub duration_ms: Option<u64>,
    pub tool_name: Option<String>,
    pub call_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParseWarningKind {
    MalformedJson,
    OversizedLine,
    InvalidRecord,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParseWarning {
    pub line: u64,
    pub byte_offset: u64,
    pub kind: ParseWarningKind,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedRollout {
    /// Codex treats the first `session_meta` record as authoritative.
    pub session: Option<SessionMeta>,
    pub events: Vec<Event>,
    pub warnings: Vec<ParseWarning>,
    pub lines_seen: u64,
    pub bytes_seen: u64,
}
