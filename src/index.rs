use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, anyhow, bail};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, params};
use serde::Serialize;
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::codex::parse_path;
use crate::model::{EventKind, Role};
use crate::redact::redact_text;

const APPLICATION_ID: i64 = 0x4144_4f53; // ADOS
const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone, Default, Serialize)]
pub struct IndexStats {
    pub files_scanned: usize,
    pub files_indexed: usize,
    pub files_unchanged: usize,
    pub files_deleted: usize,
    pub sessions: usize,
    pub events: usize,
    pub warnings: usize,
}

#[derive(Debug, Clone)]
struct FileStamp {
    size: u64,
    mtime_ns: u128,
    fingerprint: Vec<u8>,
}

pub struct CodexIndex {
    connection: Connection,
    path: PathBuf,
}

impl CodexIndex {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create index directory {}", parent.display()))?;
            set_private_dir(parent)?;
        }

        if path
            .symlink_metadata()
            .is_ok_and(|meta| meta.file_type().is_symlink())
        {
            bail!("refusing symlinked index path: {}", path.display());
        }

        let connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| format!("open index {}", path.display()))?;
        set_private_file(&path)?;

        let application_id: i64 =
            connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
        let user_version: i64 =
            connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let object_count: i64 = connection.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )?;

        if application_id == 0 && object_count == 0 {
            connection.pragma_update(None, "application_id", APPLICATION_ID)?;
        } else if application_id != APPLICATION_ID {
            bail!(
                "refusing unrecognized SQLite database at {}",
                path.display()
            );
        }
        if user_version != 0 && user_version != SCHEMA_VERSION {
            bail!("unsupported Agent Dossier index schema {user_version}");
        }

        connection.execute_batch(
            "
            PRAGMA foreign_keys = ON;
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA secure_delete = ON;
            PRAGMA temp_store = MEMORY;
            CREATE TABLE IF NOT EXISTS files (
                path TEXT PRIMARY KEY,
                size_bytes INTEGER NOT NULL,
                mtime_ns TEXT NOT NULL,
                fingerprint BLOB NOT NULL,
                session_id TEXT
            );
            CREATE TABLE IF NOT EXISTS sessions (
                session_id TEXT PRIMARY KEY,
                path TEXT NOT NULL UNIQUE,
                timestamp TEXT,
                cwd TEXT,
                repo TEXT,
                parent_id TEXT,
                root_id TEXT NOT NULL,
                provenance_status TEXT NOT NULL,
                thread_source TEXT NOT NULL,
                models TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS sessions_root_idx ON sessions(root_id);
            CREATE TABLE IF NOT EXISTS events (
                event_id INTEGER PRIMARY KEY,
                session_id TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
                seq INTEGER NOT NULL,
                line_no INTEGER NOT NULL,
                byte_offset INTEGER NOT NULL,
                timestamp TEXT,
                turn_id TEXT,
                role TEXT NOT NULL,
                kind TEXT NOT NULL,
                text TEXT NOT NULL,
                phase TEXT,
                model TEXT,
                status TEXT,
                duration_ms INTEGER,
                content_hash BLOB NOT NULL,
                UNIQUE(session_id, seq)
            );
            CREATE INDEX IF NOT EXISTS events_session_seq_idx
              ON events(session_id, seq);
            CREATE VIRTUAL TABLE IF NOT EXISTS event_fts USING fts5(
                user_text,
                assistant_text,
                tool_text,
                metadata_text,
                event_id UNINDEXED,
                session_id UNINDEXED,
                tokenize='porter unicode61'
            );
            PRAGMA user_version = 1;
            ",
        )?;

        Ok(Self { connection, path })
    }

    pub fn rebuild(&mut self, sources: &[PathBuf]) -> Result<IndexStats> {
        self.connection.execute_batch(
            "
            DELETE FROM event_fts;
            DELETE FROM events;
            DELETE FROM sessions;
            DELETE FROM files;
            ",
        )?;
        self.refresh(sources)
    }

    pub fn refresh(&mut self, sources: &[PathBuf]) -> Result<IndexStats> {
        let paths = discover_codex_rollouts(sources)?;
        let mut stats = IndexStats {
            files_scanned: paths.len(),
            ..IndexStats::default()
        };
        let known: BTreeMap<String, (u64, String, Vec<u8>)> = {
            let mut statement = self
                .connection
                .prepare("SELECT path, size_bytes, mtime_ns, fingerprint FROM files")?;
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        (row.get::<_, i64>(1)? as u64, row.get(2)?, row.get(3)?),
                    ))
                })?
                .collect::<rusqlite::Result<_>>()?
        };
        let current: HashSet<String> = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();

        let deleted: Vec<String> = known
            .keys()
            .filter(|path| !current.contains(path.as_str()))
            .cloned()
            .collect();
        for path in &deleted {
            delete_file(&self.connection, path)?;
        }
        stats.files_deleted = deleted.len();

        for path in paths {
            let stamp = file_stamp(&path)?;
            let path_text = path.to_string_lossy().into_owned();
            let unchanged = known.get(&path_text).is_some_and(|known| {
                known.0 == stamp.size
                    && known.1 == stamp.mtime_ns.to_string()
                    && known.2 == stamp.fingerprint
            });
            if unchanged {
                stats.files_unchanged += 1;
                continue;
            }
            let parsed = parse_path(&path)
                .with_context(|| format!("parse Codex rollout {}", path.display()))?;
            stats.warnings += parsed.warnings.len();
            let Some(session) = parsed.session else {
                stats.warnings += 1;
                continue;
            };
            let transaction = self.connection.transaction()?;
            delete_file_tx(&transaction, &path_text)?;
            let cwd = session.cwd.as_deref().map(redact_text);
            let repo = cwd.as_deref().and_then(repo_name);
            let mut models = BTreeSet::new();
            for event in &parsed.events {
                if let Some(model) = &event.model {
                    models.insert(redact_text(model));
                }
            }
            transaction.execute(
                "INSERT INTO sessions (
                    session_id, path, timestamp, cwd, repo, parent_id, root_id,
                    provenance_status, thread_source, models
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', ?8, ?9)",
                params![
                    session.id,
                    path_text,
                    session.timestamp,
                    cwd,
                    repo,
                    session.parent_id,
                    session.root_id,
                    session.thread_source,
                    models.into_iter().collect::<Vec<_>>().join(","),
                ],
            )?;

            for event in &parsed.events {
                let text = redact_text(&event.text);
                let role = role_name(event.role);
                let kind = kind_name(event.kind);
                let hash = content_hash(role, kind, &text);
                transaction.execute(
                    "INSERT INTO events (
                        session_id, seq, line_no, byte_offset, timestamp, turn_id,
                        role, kind, text, phase, model, status, duration_ms, content_hash
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                    params![
                        session.id,
                        event.sequence as i64,
                        event.line as i64,
                        event.byte_offset as i64,
                        event.timestamp,
                        event.turn_id,
                        role,
                        kind,
                        text,
                        event.phase,
                        event.model.as_deref().map(redact_text),
                        event.status,
                        event.duration_ms.map(|value| value as i64),
                        hash,
                    ],
                )?;
                let event_id = transaction.last_insert_rowid();
                let (user, assistant, tool, metadata) = match event.role {
                    Role::User => (text.as_str(), "", "", ""),
                    Role::Assistant => ("", text.as_str(), "", ""),
                    Role::Tool => ("", "", text.as_str(), ""),
                    _ => ("", "", "", text.as_str()),
                };
                transaction.execute(
                    "INSERT INTO event_fts (
                        user_text, assistant_text, tool_text, metadata_text,
                        event_id, session_id
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![user, assistant, tool, metadata, event_id, session.id],
                )?;
            }
            transaction.execute(
                "INSERT INTO files (path, size_bytes, mtime_ns, fingerprint, session_id)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    path_text,
                    stamp.size as i64,
                    stamp.mtime_ns.to_string(),
                    stamp.fingerprint,
                    session.id,
                ],
            )?;
            stats.events += parsed.events.len();
            stats.files_indexed += 1;
            transaction.commit()?;
        }

        update_provenance(&self.connection)?;
        stats.sessions = self
            .connection
            .query_row("SELECT count(*) FROM sessions", [], |row| {
                row.get::<_, i64>(0)
            })? as usize;
        if stats.events == 0 {
            stats.events = self
                .connection
                .query_row("SELECT count(*) FROM events", [], |row| {
                    row.get::<_, i64>(0)
                })? as usize;
        }
        checkpoint_private_files(&self.connection, &self.path)?;
        Ok(stats)
    }

    pub(crate) fn connection(&self) -> &Connection {
        &self.connection
    }
}

pub fn default_codex_sources() -> Vec<PathBuf> {
    dirs::home_dir()
        .map(|home| {
            vec![
                home.join(".codex/sessions"),
                home.join(".codex/archived_sessions"),
            ]
        })
        .unwrap_or_default()
}

pub fn default_index_path() -> Result<PathBuf> {
    let root = dirs::cache_dir().ok_or_else(|| anyhow!("no OS cache directory available"))?;
    Ok(root.join("agent-dossier/index.sqlite"))
}

pub fn discover_codex_rollouts(sources: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for source in sources {
        if !source.exists() {
            continue;
        }
        if source.symlink_metadata()?.file_type().is_symlink() {
            bail!("refusing symlinked Codex source: {}", source.display());
        }
        for entry in WalkDir::new(source).follow_links(false).sort_by_file_name() {
            let entry = entry.with_context(|| format!("walk {}", source.display()))?;
            if entry.file_type().is_symlink() {
                continue;
            }
            if entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "jsonl")
            {
                paths.push(entry.path().to_path_buf());
            }
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn delete_file(connection: &Connection, path: &str) -> Result<()> {
    let session_id: Option<String> = connection
        .query_row(
            "SELECT session_id FROM files WHERE path = ?1",
            [path],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(session_id) = session_id {
        connection.execute("DELETE FROM event_fts WHERE session_id = ?1", [&session_id])?;
        connection.execute("DELETE FROM sessions WHERE session_id = ?1", [&session_id])?;
    }
    connection.execute("DELETE FROM files WHERE path = ?1", [path])?;
    Ok(())
}

fn delete_file_tx(transaction: &Transaction<'_>, path: &str) -> Result<()> {
    let session_id: Option<String> = transaction
        .query_row(
            "SELECT session_id FROM files WHERE path = ?1",
            [path],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(session_id) = session_id {
        transaction.execute("DELETE FROM event_fts WHERE session_id = ?1", [&session_id])?;
        transaction.execute("DELETE FROM sessions WHERE session_id = ?1", [&session_id])?;
    }
    transaction.execute("DELETE FROM files WHERE path = ?1", [path])?;
    Ok(())
}

fn update_provenance(connection: &Connection) -> Result<()> {
    let mut statement =
        connection.prepare("SELECT session_id, parent_id FROM sessions ORDER BY session_id")?;
    let rows: Vec<(String, Option<String>)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let parents: HashMap<String, Option<String>> = rows.iter().cloned().collect();

    for (session_id, _) in rows {
        let mut seen = HashSet::new();
        let mut cursor = session_id.clone();
        let (root, status) = loop {
            if !seen.insert(cursor.clone()) {
                break (session_id.clone(), "cycle");
            }
            match parents.get(&cursor) {
                Some(Some(parent)) if parents.contains_key(parent) => cursor = parent.clone(),
                Some(Some(parent)) => break (parent.clone(), "orphan"),
                _ => break (cursor, "complete"),
            }
        };
        connection.execute(
            "UPDATE sessions SET root_id = ?1, provenance_status = ?2
             WHERE session_id = ?3",
            params![root, status, session_id],
        )?;
    }
    Ok(())
}

fn file_stamp(path: &Path) -> Result<FileStamp> {
    let metadata = path.metadata()?;
    if !metadata.is_file() {
        bail!("not a regular file: {}", path.display());
    }
    let mtime_ns = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    hasher.update(metadata.len().to_le_bytes());
    let mut buffer = vec![0_u8; 64 * 1024];
    let head = file.read(&mut buffer)?;
    hasher.update(&buffer[..head]);
    if metadata.len() > buffer.len() as u64 {
        file.seek(SeekFrom::End(-(buffer.len() as i64)))?;
        let tail = file.read(&mut buffer)?;
        hasher.update(&buffer[..tail]);
    }
    Ok(FileStamp {
        size: metadata.len(),
        mtime_ns,
        fingerprint: hasher.finalize()[..16].to_vec(),
    })
}

fn content_hash(role: &str, kind: &str, text: &str) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(role.as_bytes());
    hasher.update([0]);
    hasher.update(kind.as_bytes());
    hasher.update([0]);
    hasher.update(text.as_bytes());
    hasher.finalize()[..16].to_vec()
}

fn role_name(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
        Role::Metadata => "metadata",
        Role::Developer => "developer",
        Role::System => "system",
        Role::Unknown => "unknown",
    }
}

fn kind_name(kind: EventKind) -> &'static str {
    match kind {
        EventKind::TurnContext => "turn_context",
        EventKind::UserMessage => "user_message",
        EventKind::AgentMessage => "agent_message",
        EventKind::FinalAnswer => "final_answer",
        EventKind::ResponseMessage => "response_message",
        EventKind::ToolCall => "tool_call",
        EventKind::TaskComplete => "task_complete",
    }
}

fn repo_name(cwd: &str) -> Option<String> {
    cwd.replace('\\', "/")
        .split('/')
        .filter(|part| !part.is_empty() && *part != "~")
        .next_back()
        .map(str::to_string)
}

fn checkpoint_private_files(connection: &Connection, path: &Path) -> Result<()> {
    connection.execute_batch("PRAGMA wal_checkpoint(PASSIVE);")?;
    set_private_file(path)?;
    set_private_file(&PathBuf::from(format!("{}-wal", path.display())))?;
    set_private_file(&PathBuf::from(format!("{}-shm", path.display())))?;
    Ok(())
}

#[cfg(unix)]
fn set_private_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_dir(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if path.exists() {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_private_file(_path: &Path) -> Result<()> {
    Ok(())
}
