use std::fs;

use agent_dossier::{CodexIndex, DossierRequest};
use tempfile::TempDir;

fn write_session(dir: &std::path::Path, name: &str, id: &str, cwd: &str, lines: &[&str]) {
    let mut body = format!(
        "{{\"timestamp\":\"2026-04-17T19:59:11.000Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{id}\",\"timestamp\":\"2026-04-17T19:59:11.000Z\",\"cwd\":\"{cwd}\",\"originator\":\"Codex Desktop\",\"cli_version\":\"0.120.0\",\"source\":\"vscode\",\"model_provider\":\"openai\"}}}}\n"
    );
    for (offset, line) in lines.iter().enumerate() {
        body.push_str(&format!(
            "{{\"timestamp\":\"2026-04-17T20:00:0{offset}.000Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"agent_message\",\"message\":\"{line}\",\"phase\":\"commentary\"}}}}\n"
        ));
    }
    fs::write(dir.join(name), body).unwrap();
}

fn build_two_repo_index(temp: &TempDir) -> CodexIndex {
    let sources = temp.path().join("sessions");
    fs::create_dir_all(&sources).unwrap();
    write_session(
        &sources,
        "asc.jsonl",
        "session-asc",
        "/tmp/checkout/app-store-connect-cli",
        &[
            "The release was paused because the checksum verification failed.",
            "PR #442 tracks the release checksum fix.",
        ],
    );
    write_session(
        &sources,
        "other.jsonl",
        "session-other",
        "/tmp/checkout/other-project",
        &[
            "The release was paused here too while we debated the checksum layout.",
            "PR #442 in this project is unrelated to that release checksum.",
            "More release checksum chatter to make this session loud.",
        ],
    );
    let mut index = CodexIndex::open(temp.path().join("index.sqlite")).unwrap();
    index.refresh(&[sources]).unwrap();
    index
}

#[test]
fn repo_scoped_query_excludes_other_repositories() {
    let temp = TempDir::new().unwrap();
    let index = build_two_repo_index(&temp);

    let dossier = index
        .dossier(DossierRequest::new(
            "Why was the release paused in app-store-connect-cli?",
        ))
        .unwrap();

    assert!(!dossier.evidence.is_empty());
    for item in &dossier.evidence {
        assert_eq!(
            item.session_id, "session-asc",
            "evidence leaked from another repository: {}",
            item.text
        );
    }
    for session in &dossier.matched_sessions {
        assert_eq!(session.repo.as_deref(), Some("app-store-connect-cli"));
    }
}

#[test]
fn repo_scope_falls_back_when_no_session_matches_the_hint() {
    let temp = TempDir::new().unwrap();
    let index = build_two_repo_index(&temp);

    let dossier = index
        .dossier(DossierRequest::new(
            "Why was the release paused in some-unknown-project-name?",
        ))
        .unwrap();

    assert!(
        !dossier.evidence.is_empty(),
        "an unmatched repository hint must not empty the dossier"
    );
}

#[test]
fn repo_hint_matches_despite_underscore_and_case_differences() {
    let temp = TempDir::new().unwrap();
    let sources = temp.path().join("sessions");
    fs::create_dir_all(&sources).unwrap();
    write_session(
        &sources,
        "underscore.jsonl",
        "session-underscore",
        "/tmp/checkout/Third_Repo",
        &["The migration finished after the second retry."],
    );
    write_session(
        &sources,
        "noise.jsonl",
        "session-noise",
        "/tmp/checkout/unrelated",
        &["The migration here is a different migration entirely."],
    );
    let mut index = CodexIndex::open(temp.path().join("index.sqlite")).unwrap();
    index.refresh(&[sources]).unwrap();

    let dossier = index
        .dossier(DossierRequest::new(
            "How did the migration finish in repository third-repo?",
        ))
        .unwrap();

    assert!(!dossier.evidence.is_empty());
    for item in &dossier.evidence {
        assert_eq!(item.session_id, "session-underscore");
    }
}
