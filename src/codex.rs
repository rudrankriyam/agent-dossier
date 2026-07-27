use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;

use serde_json::{Map, Value};

use crate::model::{
    Event, EventKind, ParseWarning, ParseWarningKind, ParsedRollout, Role, SessionMeta,
};

/// One JSONL record may contain a large tool result or embedded image. Keep the
/// parser bounded while leaving enough room for ordinary Codex records.
pub const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_TEXT_BYTES: usize = 64 * 1024;

pub fn parse_path(path: impl AsRef<Path>) -> io::Result<ParsedRollout> {
    parse_reader(File::open(path)?)
}

pub fn parse_reader(reader: impl Read) -> io::Result<ParsedRollout> {
    let mut reader = BufReader::new(reader);
    let mut parsed = ParsedRollout::default();
    let mut current_turn_id: Option<String> = None;
    let mut sequence = 0_u64;

    loop {
        let line = parsed.lines_seen + 1;
        let byte_offset = parsed.bytes_seen;
        let Some(raw_line) = read_bounded_line(&mut reader)? else {
            break;
        };

        parsed.lines_seen = line;
        parsed.bytes_seen = parsed
            .bytes_seen
            .saturating_add(raw_line.bytes_consumed as u64);

        let Some(bytes) = raw_line.bytes else {
            parsed.warnings.push(ParseWarning {
                line,
                byte_offset,
                kind: ParseWarningKind::OversizedLine,
                message: format!("record exceeded the {MAX_LINE_BYTES}-byte line limit"),
            });
            continue;
        };
        if bytes.iter().all(|byte| byte.is_ascii_whitespace()) {
            continue;
        }

        let record: Value = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(error) => {
                parsed.warnings.push(ParseWarning {
                    line,
                    byte_offset,
                    kind: ParseWarningKind::MalformedJson,
                    message: error.to_string(),
                });
                continue;
            }
        };

        let Some(record_type) = record.get("type").and_then(Value::as_str) else {
            parsed.warnings.push(ParseWarning {
                line,
                byte_offset,
                kind: ParseWarningKind::InvalidRecord,
                message: "record has no string `type`".to_string(),
            });
            continue;
        };

        if record_type == "session_meta" {
            if parsed.session.is_none() {
                match parse_session_meta(record.get("payload")) {
                    Ok(meta) => parsed.session = Some(meta),
                    Err(message) => parsed.warnings.push(ParseWarning {
                        line,
                        byte_offset,
                        kind: ParseWarningKind::InvalidRecord,
                        message,
                    }),
                }
            }
            continue;
        }

        let timestamp = string_field(record.as_object(), "timestamp");
        let payload = record.get("payload").and_then(Value::as_object);
        let event = match record_type {
            "turn_context" => parse_turn_context(payload, &mut current_turn_id),
            "event_msg" => parse_event_message(payload, current_turn_id.as_deref()),
            "response_item" => parse_response_item(payload, current_turn_id.as_deref()),
            _ => None,
        };

        if let Some(mut event) = event {
            sequence += 1;
            event.sequence = sequence;
            event.line = line;
            event.byte_offset = byte_offset;
            event.timestamp = timestamp;
            parsed.events.push(event);
        }
    }

    Ok(parsed)
}

struct BoundedLine {
    bytes: Option<Vec<u8>>,
    bytes_consumed: usize,
}

/// Consume exactly one logical line without ever retaining more than
/// `MAX_LINE_BYTES`. Oversized records are discarded through their newline so
/// the next record remains parseable.
fn read_bounded_line(reader: &mut impl BufRead) -> io::Result<Option<BoundedLine>> {
    let mut output = Vec::new();
    let mut bytes_consumed = 0_usize;
    let mut oversized = false;
    let mut saw_bytes = false;

    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if !saw_bytes {
                return Ok(None);
            }
            break;
        }
        saw_bytes = true;

        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(available.len(), |index| index + 1);
        let content_len = newline.unwrap_or(available.len());
        bytes_consumed = bytes_consumed.saturating_add(take);

        if !oversized {
            if output.len().saturating_add(content_len) <= MAX_LINE_BYTES {
                output.extend_from_slice(&available[..content_len]);
            } else {
                oversized = true;
                output.clear();
            }
        }

        reader.consume(take);
        if newline.is_some() {
            break;
        }
    }

    Ok(Some(BoundedLine {
        bytes: (!oversized).then_some(output),
        bytes_consumed,
    }))
}

fn parse_session_meta(payload: Option<&Value>) -> Result<SessionMeta, String> {
    let payload = payload
        .and_then(Value::as_object)
        .ok_or_else(|| "session_meta payload is not an object".to_string())?;
    let id = string_field(Some(payload), "id")
        .or_else(|| string_field(Some(payload), "session_id"))
        .ok_or_else(|| "session_meta has neither `id` nor `session_id`".to_string())?;
    let session_id = string_field(Some(payload), "session_id").unwrap_or_else(|| id.clone());
    let parent_thread_id = string_field(Some(payload), "parent_thread_id").or_else(|| {
        nested_string(
            payload,
            &["source", "subagent", "thread_spawn", "parent_thread_id"],
        )
    });
    let forked_from_id = string_field(Some(payload), "forked_from_id");
    let parent_id = parent_thread_id.clone().or_else(|| forked_from_id.clone());
    let thread_source = string_field(Some(payload), "thread_source").unwrap_or_else(|| {
        if parent_id.is_some() || payload.get("source").is_some_and(is_subagent_source) {
            "subagent".to_string()
        } else {
            "user".to_string()
        }
    });

    Ok(SessionMeta {
        id,
        root_id: session_id.clone(),
        session_id,
        parent_thread_id,
        forked_from_id,
        parent_id,
        timestamp: string_field(Some(payload), "timestamp"),
        cwd: string_field(Some(payload), "cwd"),
        originator: string_field(Some(payload), "originator"),
        cli_version: string_field(Some(payload), "cli_version"),
        source: payload.get("source").cloned(),
        thread_source,
        model_provider: string_field(Some(payload), "model_provider"),
        history_mode: string_field(Some(payload), "history_mode"),
    })
}

fn is_subagent_source(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|source| source.contains_key("subagent"))
}

fn parse_turn_context(
    payload: Option<&Map<String, Value>>,
    current_turn_id: &mut Option<String>,
) -> Option<Event> {
    let turn_id = string_field(payload, "turn_id");
    if turn_id.is_some() {
        *current_turn_id = turn_id.clone();
    }
    let model = string_field(payload, "model")
        .or_else(|| nested_string_from_map(payload?, &["collaboration_mode", "settings", "model"]));
    let cwd = string_field(payload, "cwd");
    let (text, text_truncated) = bound_text(
        [
            model.as_ref().map(|value| format!("model {value}")),
            cwd.as_ref().map(|value| format!("cwd {value}")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n"),
    );
    if text.is_empty() && turn_id.is_none() {
        return None;
    }

    Some(
        empty_event(
            turn_id,
            Role::Metadata,
            EventKind::TurnContext,
            text,
            text_truncated,
        )
        .with_model(model),
    )
}

fn parse_event_message(
    payload: Option<&Map<String, Value>>,
    current_turn_id: Option<&str>,
) -> Option<Event> {
    let payload_type = string_field(payload, "type")?;
    match payload_type.as_str() {
        "user_message" => {
            let mut text = string_field(payload, "message").unwrap_or_default();
            let attachments = attachment_strings(payload);
            if !attachments.is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str("[attachments]\n");
                text.push_str(&attachments.join("\n"));
            }
            let (text, text_truncated) = bound_text(text);
            (!text.is_empty()).then(|| {
                empty_event(
                    current_turn_id.map(str::to_string),
                    Role::User,
                    EventKind::UserMessage,
                    text,
                    text_truncated,
                )
            })
        }
        "agent_message" => {
            let (text, text_truncated) =
                bound_text(string_field(payload, "message").unwrap_or_default());
            if text.is_empty() {
                return None;
            }
            let phase = string_field(payload, "phase");
            let kind = if phase.as_deref() == Some("final_answer") {
                EventKind::FinalAnswer
            } else {
                EventKind::AgentMessage
            };
            Some(
                empty_event(
                    current_turn_id.map(str::to_string),
                    Role::Assistant,
                    kind,
                    text,
                    text_truncated,
                )
                .with_phase(phase),
            )
        }
        "task_complete" | "turn_complete" => {
            let (text, text_truncated) =
                bound_text(string_field(payload, "last_agent_message").unwrap_or_default());
            let turn_id =
                string_field(payload, "turn_id").or_else(|| current_turn_id.map(str::to_string));
            let status = string_field(payload, "status").or_else(|| Some("completed".to_string()));
            Some(
                empty_event(
                    turn_id,
                    Role::Metadata,
                    EventKind::TaskComplete,
                    text,
                    text_truncated,
                )
                .with_status(status)
                .with_duration(number_field(payload, "duration_ms")),
            )
        }
        _ => None,
    }
}

fn parse_response_item(
    payload: Option<&Map<String, Value>>,
    current_turn_id: Option<&str>,
) -> Option<Event> {
    let payload = payload?;
    let payload_type = string_field(Some(payload), "type")?;
    match payload_type.as_str() {
        "message" => {
            let text = content_text(payload.get("content"));
            let (text, text_truncated) = bound_text(text);
            if text.is_empty() {
                return None;
            }
            let role = Role::from_wire(payload.get("role").and_then(Value::as_str));
            Some(
                empty_event(
                    current_turn_id.map(str::to_string),
                    role,
                    EventKind::ResponseMessage,
                    text,
                    text_truncated,
                )
                .with_phase(string_field(Some(payload), "phase")),
            )
        }
        "function_call" | "custom_tool_call" | "tool_search_call" => {
            let name = string_field(Some(payload), "name").unwrap_or_else(|| {
                if payload_type == "tool_search_call" {
                    "tool_search".to_string()
                } else {
                    "tool".to_string()
                }
            });
            let arguments = payload
                .get("arguments")
                .or_else(|| payload.get("input"))
                .or_else(|| payload.get("args"))
                .map(compact_value)
                .unwrap_or_default();
            let (text, text_truncated) = bound_text(if arguments.is_empty() {
                name.clone()
            } else {
                format!("{name}\n{arguments}")
            });
            Some(
                empty_event(
                    current_turn_id.map(str::to_string),
                    Role::Tool,
                    EventKind::ToolCall,
                    text,
                    text_truncated,
                )
                .with_status(string_field(Some(payload), "status"))
                .with_tool(
                    Some(name),
                    string_field(Some(payload), "call_id")
                        .or_else(|| string_field(Some(payload), "id")),
                ),
            )
        }
        _ => None,
    }
}

fn empty_event(
    turn_id: Option<String>,
    role: Role,
    kind: EventKind,
    text: String,
    text_truncated: bool,
) -> Event {
    Event {
        sequence: 0,
        line: 0,
        byte_offset: 0,
        timestamp: None,
        turn_id,
        role,
        kind,
        text,
        text_truncated,
        phase: None,
        model: None,
        status: None,
        duration_ms: None,
        tool_name: None,
        call_id: None,
    }
}

trait EventBuilder {
    fn with_phase(self, phase: Option<String>) -> Self;
    fn with_model(self, model: Option<String>) -> Self;
    fn with_status(self, status: Option<String>) -> Self;
    fn with_duration(self, duration_ms: Option<u64>) -> Self;
    fn with_tool(self, tool_name: Option<String>, call_id: Option<String>) -> Self;
}

impl EventBuilder for Event {
    fn with_phase(mut self, phase: Option<String>) -> Self {
        self.phase = phase;
        self
    }

    fn with_model(mut self, model: Option<String>) -> Self {
        self.model = model;
        self
    }

    fn with_status(mut self, status: Option<String>) -> Self {
        self.status = status;
        self
    }

    fn with_duration(mut self, duration_ms: Option<u64>) -> Self {
        self.duration_ms = duration_ms;
        self
    }

    fn with_tool(mut self, tool_name: Option<String>, call_id: Option<String>) -> Self {
        self.tool_name = tool_name;
        self.call_id = call_id;
        self
    }
}

fn attachment_strings(payload: Option<&Map<String, Value>>) -> Vec<String> {
    let mut attachments = Vec::new();
    let Some(payload) = payload else {
        return attachments;
    };
    for key in ["images", "local_images", "audio", "local_audio"] {
        let Some(values) = payload.get(key).and_then(Value::as_array) else {
            continue;
        };
        attachments.extend(values.iter().filter_map(Value::as_str).map(str::to_string));
    }
    attachments
}

fn content_text(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|part| {
            part.get("text")
                .and_then(Value::as_str)
                .or_else(|| part.get("input_text").and_then(Value::as_str))
                .or_else(|| part.get("output_text").and_then(Value::as_str))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn string_field(map: Option<&Map<String, Value>>, key: &str) -> Option<String> {
    map?.get(key)?.as_str().map(str::to_string)
}

fn number_field(map: Option<&Map<String, Value>>, key: &str) -> Option<u64> {
    map?.get(key)?.as_u64()
}

fn nested_string(map: &Map<String, Value>, path: &[&str]) -> Option<String> {
    nested_string_from_map(map, path)
}

fn nested_string_from_map(map: &Map<String, Value>, path: &[&str]) -> Option<String> {
    let mut value = map.get(*path.first()?)?;
    for key in &path[1..] {
        value = value.get(*key)?;
    }
    value.as_str().map(str::to_string)
}

fn compact_value(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}

fn bound_text(text: String) -> (String, bool) {
    if text.len() <= MAX_TEXT_BYTES {
        return (text, false);
    }

    const MARKER: &str = "\n[…truncated…]";
    let mut end = MAX_TEXT_BYTES.saturating_sub(MARKER.len());
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    let mut bounded = text[..end].to_string();
    bounded.push_str(MARKER);
    (bounded, true)
}
