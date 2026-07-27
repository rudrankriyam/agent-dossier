use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_agent-dossier")
}

fn run(arguments: &[&str]) -> Output {
    Command::new(binary())
        .args(arguments)
        .output()
        .expect("run agent-dossier")
}

fn fixture_root() -> &'static str {
    "tests/fixtures/codex"
}

#[test]
fn indexes_and_queries_synthetic_codex_history() {
    let temp = TempDir::new().unwrap();
    let database = temp.path().join("index.sqlite");
    let database_text = database.to_str().unwrap();

    let indexed = run(&[
        "index",
        "--source",
        fixture_root(),
        "--db",
        database_text,
        "--format",
        "json",
    ]);
    assert!(
        indexed.status.success(),
        "{}",
        String::from_utf8_lossy(&indexed.stderr)
    );
    let stats: Value = serde_json::from_slice(&indexed.stdout).unwrap();
    assert_eq!(stats["sessions"], 2);
    assert_eq!(stats["warnings"], 1);

    let dossier = run(&[
        "dossier",
        "What command passed?",
        "--db",
        database_text,
        "--format",
        "json",
    ]);
    assert!(
        dossier.status.success(),
        "{}",
        String::from_utf8_lossy(&dossier.stderr)
    );
    let result: Value = serde_json::from_slice(&dossier.stdout).unwrap();
    assert!(!result["evidence"].as_array().unwrap().is_empty());
    assert!(
        result["evidence"][0]["citation"]
            .as_str()
            .unwrap()
            .starts_with("codex://session/")
    );
    assert!(!String::from_utf8_lossy(&dossier.stdout).contains("/Users/"));
}

#[test]
fn second_index_run_is_incremental() {
    let temp = TempDir::new().unwrap();
    let database = temp.path().join("index.sqlite");
    let database_text = database.to_str().unwrap();
    let arguments = [
        "index",
        "--source",
        fixture_root(),
        "--db",
        database_text,
        "--format",
        "json",
    ];
    assert!(run(&arguments).status.success());
    let second = run(&arguments);
    assert!(second.status.success());
    let stats: Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(stats["files_indexed"], 0);
    assert_eq!(stats["files_unchanged"], 2);
}

#[test]
fn redacts_before_persisting_searchable_text() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("history");
    fs::create_dir(&source).unwrap();
    let rollout = source.join("rollout.jsonl");
    fs::write(
        &rollout,
        concat!(
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"secret-demo\",\"cwd\":\"/tmp/demo\"}}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"api_key=CANARY_SECRET_DO_NOT_EMIT\"}}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"Stopped safely.\"}}\n"
        ),
    )
    .unwrap();
    let database = temp.path().join("index.sqlite");
    let indexed = run(&[
        "index",
        "--source",
        source.to_str().unwrap(),
        "--db",
        database.to_str().unwrap(),
    ]);
    assert!(
        indexed.status.success(),
        "{}",
        String::from_utf8_lossy(&indexed.stderr)
    );

    for path in database_files(&database) {
        let bytes = fs::read(&path).unwrap();
        assert!(
            !bytes
                .windows(b"CANARY_SECRET_DO_NOT_EMIT".len())
                .any(|window| window == b"CANARY_SECRET_DO_NOT_EMIT"),
            "secret persisted in {}",
            path.display()
        );
    }
}

#[test]
fn refuses_an_unrecognized_sqlite_database_without_modifying_it() {
    let temp = TempDir::new().unwrap();
    let database = temp.path().join("foreign.sqlite");
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute("CREATE TABLE sentinel(value TEXT NOT NULL)", [])
        .unwrap();
    connection
        .execute("INSERT INTO sentinel VALUES ('keep-me')", [])
        .unwrap();
    drop(connection);

    let before = fs::read(&database).unwrap();
    let output = run(&[
        "index",
        "--source",
        fixture_root(),
        "--db",
        database.to_str().unwrap(),
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unrecognized SQLite database"));
    let after = fs::read(&database).unwrap();
    assert_eq!(before, after);
}

#[cfg(unix)]
#[test]
fn creates_private_index_permissions_even_under_a_permissive_umask() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new().unwrap();
    let database = temp.path().join("private/index.sqlite");
    let output = run(&[
        "index",
        "--source",
        fixture_root(),
        "--db",
        database.to_str().unwrap(),
    ]);
    assert!(output.status.success());
    assert_eq!(
        fs::metadata(database.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&database).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

fn database_files(database: &Path) -> Vec<std::path::PathBuf> {
    ["", "-wal", "-shm"]
        .into_iter()
        .map(|suffix| std::path::PathBuf::from(format!("{}{suffix}", database.display())))
        .filter(|path| path.exists())
        .collect()
}
