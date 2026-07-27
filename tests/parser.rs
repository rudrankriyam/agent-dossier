use std::io::Cursor;
use std::path::PathBuf;

use agent_dossier::codex::{MAX_LINE_BYTES, MAX_TEXT_BYTES, parse_path, parse_reader};
use agent_dossier::model::{EventKind, ParseWarningKind, Role};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("codex")
        .join(name)
}

#[test]
fn parses_current_codex_shapes_and_keeps_provenance() {
    let parsed = parse_path(fixture("current.jsonl")).expect("fixture should parse");
    let session = parsed.session.expect("session metadata");

    assert_eq!(session.id, "child-001");
    assert_eq!(session.session_id, "root-001");
    assert_eq!(session.root_id, "root-001");
    assert_eq!(session.parent_id.as_deref(), Some("root-001"));
    assert_eq!(session.parent_thread_id.as_deref(), Some("root-001"));
    assert_eq!(session.forked_from_id.as_deref(), Some("root-001"));
    assert_eq!(session.thread_source, "subagent");

    assert_eq!(parsed.events.len(), 7);
    assert_eq!(parsed.events[0].kind, EventKind::TurnContext);
    assert_eq!(parsed.events[0].model.as_deref(), Some("gpt-test"));
    assert_eq!(parsed.events[0].turn_id.as_deref(), Some("turn-001"));

    let user = parsed
        .events
        .iter()
        .find(|event| event.kind == EventKind::UserMessage)
        .expect("user event");
    assert_eq!(user.role, Role::User);
    assert!(user.text.contains("Find the last working command."));
    assert!(user.text.contains("https://example.invalid/image.png"));
    assert!(user.text.contains("/tmp/screenshot.png"));

    let response = parsed
        .events
        .iter()
        .find(|event| event.kind == EventKind::ResponseMessage)
        .expect("assistant response item");
    assert_eq!(response.phase.as_deref(), Some("commentary"));
    assert_eq!(response.turn_id.as_deref(), Some("turn-001"));

    let tools: Vec<_> = parsed
        .events
        .iter()
        .filter(|event| event.kind == EventKind::ToolCall)
        .collect();
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0].tool_name.as_deref(), Some("exec_command"));
    assert_eq!(tools[0].call_id.as_deref(), Some("call-001"));
    assert_eq!(tools[0].status.as_deref(), Some("completed"));
    assert_eq!(tools[1].tool_name.as_deref(), Some("exec"));

    let final_answer = parsed
        .events
        .iter()
        .find(|event| event.kind == EventKind::FinalAnswer)
        .expect("final answer");
    assert_eq!(final_answer.phase.as_deref(), Some("final_answer"));

    let completed = parsed
        .events
        .iter()
        .find(|event| event.kind == EventKind::TaskComplete)
        .expect("task completion");
    assert_eq!(completed.status.as_deref(), Some("completed"));
    assert_eq!(completed.duration_ms, Some(123_456));
    assert_eq!(completed.turn_id.as_deref(), Some("turn-001"));

    assert_eq!(parsed.warnings.len(), 1);
    assert_eq!(parsed.warnings[0].kind, ParseWarningKind::MalformedJson);
    assert_eq!(parsed.warnings[0].line, 8);
    assert!(parsed.events.iter().all(|event| event.line != 8));
    assert!(
        parsed
            .events
            .windows(2)
            .all(|events| events[0].byte_offset < events[1].byte_offset)
    );
}

#[test]
fn supports_legacy_meta_and_turns_without_turn_ids() {
    let parsed = parse_path(fixture("legacy.jsonl")).expect("fixture should parse");
    let session = parsed.session.expect("session metadata");

    assert_eq!(session.id, "fork-legacy");
    assert_eq!(session.session_id, "fork-legacy");
    assert_eq!(session.root_id, "fork-legacy");
    assert_eq!(session.parent_id.as_deref(), Some("root-legacy"));
    assert_eq!(session.thread_source, "subagent");

    assert_eq!(parsed.events[0].model.as_deref(), Some("gpt-legacy"));
    assert_eq!(parsed.events[0].turn_id, None);
    assert_eq!(parsed.events[1].turn_id, None);
    assert_eq!(parsed.events[1].kind, EventKind::AgentMessage);
}

#[test]
fn skips_an_oversized_record_and_resynchronizes_at_the_next_line() {
    let oversized = "x".repeat(MAX_LINE_BYTES + 1);
    let input = format!(
        "{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"{oversized}\"}}}}\n\
         {{\"type\":\"event_msg\",\"payload\":{{\"type\":\"agent_message\",\"message\":\"after\",\"phase\":\"commentary\"}}}}\n"
    );

    let parsed = parse_reader(Cursor::new(input)).expect("stream should parse");
    assert_eq!(parsed.warnings.len(), 1);
    assert_eq!(parsed.warnings[0].kind, ParseWarningKind::OversizedLine);
    assert_eq!(parsed.warnings[0].line, 1);
    assert_eq!(parsed.events.len(), 1);
    assert_eq!(parsed.events[0].line, 2);
    assert_eq!(parsed.events[0].text, "after");
    assert!(parsed.events[0].byte_offset > MAX_LINE_BYTES as u64);
}

#[test]
fn bounds_event_text_on_a_utf8_boundary() {
    let message = "é".repeat(MAX_TEXT_BYTES);
    let input = format!(
        "{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":{}}}}}\n",
        serde_json::to_string(&message).expect("serialize message")
    );

    let parsed = parse_reader(Cursor::new(input)).expect("stream should parse");
    let event = &parsed.events[0];
    assert!(event.text_truncated);
    assert!(event.text.len() <= MAX_TEXT_BYTES);
    assert!(event.text.ends_with("[…truncated…]"));
}

#[test]
fn first_session_meta_is_authoritative() {
    let input = concat!(
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"first\",\"session_id\":\"first\"}}\n",
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"second\",\"session_id\":\"second\"}}\n"
    );
    let parsed = parse_reader(Cursor::new(input)).expect("stream should parse");

    assert_eq!(parsed.session.expect("session").id, "first");
}
