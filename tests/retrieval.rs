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

fn write_session_with_replay(
    dir: &std::path::Path,
    name: &str,
    id: &str,
    cwd: &str,
    genuine: &str,
    replay: &str,
) {
    let body = format!(
        "{{\"timestamp\":\"2026-04-17T19:59:11.000Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{id}\",\"timestamp\":\"2026-04-17T19:59:11.000Z\",\"cwd\":\"{cwd}\",\"originator\":\"Codex Desktop\",\"cli_version\":\"0.120.0\",\"source\":\"vscode\",\"model_provider\":\"openai\"}}}}\n\
         {{\"timestamp\":\"2026-04-17T20:00:00.000Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"{genuine}\"}}}}\n\
         {{\"timestamp\":\"2026-04-17T20:00:01.000Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"{replay}\"}}]}}}}\n"
    );
    fs::write(dir.join(name), body).unwrap();
}

#[test]
fn bare_project_name_scopes_retrieval_to_that_repository() {
    let temp = TempDir::new().unwrap();
    let sources = temp.path().join("sessions");
    fs::create_dir_all(&sources).unwrap();
    write_session(
        &sources,
        "aural.jsonl",
        "session-aural",
        "/tmp/checkout/AuralKit",
        &["The maintenance recovery ended with a clean verified release."],
    );
    write_session(
        &sources,
        "noise.jsonl",
        "session-noise",
        "/tmp/checkout/unrelated",
        &[
            "A verified release and a clean maintenance recovery were discussed here at length.",
            "More release recovery chatter about a clean verified maintenance pass.",
        ],
    );
    let mut index = CodexIndex::open(temp.path().join("index.sqlite")).unwrap();
    index.refresh(&[sources]).unwrap();

    let dossier = index
        .dossier(DossierRequest::new(
            "What worked last time to take AuralKit from maintenance to a verified release?",
        ))
        .unwrap();

    assert!(!dossier.evidence.is_empty());
    for item in &dossier.evidence {
        assert_eq!(
            item.session_id, "session-aural",
            "evidence leaked outside the named project: {}",
            item.text
        );
    }
}

#[test]
fn generic_words_do_not_arm_the_repository_scope() {
    let temp = TempDir::new().unwrap();
    let sources = temp.path().join("sessions");
    fs::create_dir_all(&sources).unwrap();
    write_session(
        &sources,
        "release-repo.jsonl",
        "session-release-repo",
        "/tmp/checkout/release",
        &["Unrelated notes kept in a repository that happens to be called release."],
    );
    write_session(
        &sources,
        "real.jsonl",
        "session-real",
        "/tmp/checkout/some-app",
        &["The deploy failed because the release checklist was skipped."],
    );
    let mut index = CodexIndex::open(temp.path().join("index.sqlite")).unwrap();
    index.refresh(&[sources]).unwrap();

    let dossier = index
        .dossier(DossierRequest::new(
            "Why did the release checklist get skipped before the deploy?",
        ))
        .unwrap();

    assert!(
        dossier
            .evidence
            .iter()
            .any(|item| item.session_id == "session-real"),
        "a generic word must not lock retrieval to a same-named repository"
    );
}

#[test]
fn replayed_context_does_not_outrank_a_genuine_user_message() {
    let temp = TempDir::new().unwrap();
    let sources = temp.path().join("sessions");
    fs::create_dir_all(&sources).unwrap();
    let replay = "Replayed transcript: checksum checksum checksum checksum verification \
                  verification verification failed failed failed for the release release release.";
    write_session_with_replay(
        &sources,
        "mixed.jsonl",
        "session-mixed",
        "/tmp/checkout/some-app",
        "The release was paused because checksum verification failed.",
        replay,
    );
    let mut index = CodexIndex::open(temp.path().join("index.sqlite")).unwrap();
    index.refresh(&[sources]).unwrap();

    let dossier = index
        .dossier(DossierRequest::new(
            "Why did checksum verification fail for the release?",
        ))
        .unwrap();

    assert!(!dossier.evidence.is_empty());
    assert_eq!(
        dossier.evidence[0].kind, "user_message",
        "injected replay context outranked the genuine user message: {}",
        dossier.evidence[0].text
    );
}

#[test]
fn outdated_index_schema_is_wiped_and_rebuilt() {
    let temp = TempDir::new().unwrap();
    let sources = temp.path().join("sessions");
    fs::create_dir_all(&sources).unwrap();
    write_session(
        &sources,
        "one.jsonl",
        "session-one",
        "/tmp/checkout/some-app",
        &["The migration finished cleanly."],
    );
    let database = temp.path().join("index.sqlite");
    {
        let mut index = CodexIndex::open(&database).unwrap();
        index.refresh(&[sources.clone()]).unwrap();
    }
    {
        let connection = rusqlite::Connection::open(&database).unwrap();
        connection.pragma_update(None, "user_version", 1).unwrap();
    }

    let mut index = CodexIndex::open(&database).unwrap();
    let stats = index.refresh(&[sources]).unwrap();

    assert_eq!(stats.sessions, 1);
    let dossier = index
        .dossier(DossierRequest::new("How did the migration finish?"))
        .unwrap();
    assert!(!dossier.evidence.is_empty());
}
