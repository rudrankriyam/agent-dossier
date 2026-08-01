use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::index::CodexIndex;
use crate::query::{Intent, QueryAnalysis, analyze, repo_key};
use crate::redact::{redact_text, safe_display_path};

pub const OUTPUT_SCHEMA_VERSION: u32 = 1;
pub const RANKER_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct DossierRequest {
    pub query: String,
    pub max_sessions: usize,
    pub max_evidence: usize,
    pub context: usize,
}

impl DossierRequest {
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            max_sessions: 5,
            max_evidence: 12,
            context: 1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dossier {
    pub schema_version: u32,
    pub ranker_version: u32,
    pub query: String,
    pub intent: String,
    pub matched_sessions: Vec<MatchedSession>,
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchedSession {
    pub session_id: String,
    pub root_id: String,
    pub repo: Option<String>,
    pub provenance_status: String,
    pub score: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub session_id: String,
    pub root_id: String,
    pub turn_id: Option<String>,
    pub timestamp: Option<String>,
    pub role: String,
    pub kind: String,
    pub text: String,
    pub citation: String,
    pub source: String,
    pub line: u64,
    pub byte_offset: u64,
    pub score: i64,
}

#[derive(Debug)]
struct Candidate {
    event_id: i64,
    session_id: String,
    root_id: String,
    repo: Option<String>,
    provenance_status: String,
    seq: i64,
    line: u64,
    byte_offset: u64,
    timestamp: Option<String>,
    turn_id: Option<String>,
    role: String,
    kind: String,
    text: String,
    source_path: String,
    content_hash: Vec<u8>,
    score: i64,
}

impl CodexIndex {
    pub fn dossier(&self, request: DossierRequest) -> Result<Dossier> {
        let analysis = analyze(&request.query);
        if analysis.fts_query.is_empty() {
            bail!("query does not contain searchable Codex-history terms");
        }
        if !(1..=20).contains(&request.max_sessions) {
            bail!("max_sessions must be between 1 and 20");
        }
        if !(1..=100).contains(&request.max_evidence) {
            bail!("max_evidence must be between 1 and 100");
        }
        if request.context > 3 {
            bail!("context must be between 0 and 3");
        }

        let repo_scope = self.repo_scope(&analysis)?;
        let mut candidates =
            self.search_candidates(&analysis, request.max_evidence * 40, repo_scope.as_deref())?;
        candidates.sort_by(candidate_order);

        let mut selected_sessions = BTreeMap::<String, MatchedSession>::new();
        for candidate in &candidates {
            selected_sessions
                .entry(candidate.session_id.clone())
                .and_modify(|session| session.score = session.score.max(candidate.score))
                .or_insert_with(|| MatchedSession {
                    session_id: candidate.session_id.clone(),
                    root_id: candidate.root_id.clone(),
                    repo: candidate.repo.clone(),
                    provenance_status: candidate.provenance_status.clone(),
                    score: candidate.score,
                });
        }
        let mut matched_sessions: Vec<_> = selected_sessions.into_values().collect();
        matched_sessions.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.session_id.cmp(&right.session_id))
        });
        matched_sessions.truncate(request.max_sessions);
        let allowed_sessions: HashSet<_> = matched_sessions
            .iter()
            .map(|session| session.session_id.as_str())
            .collect();

        let mut evidence = Vec::new();
        let mut seen = HashSet::<(String, Vec<u8>)>::new();
        for candidate in candidates {
            if evidence.len() == request.max_evidence {
                break;
            }
            if !allowed_sessions.contains(candidate.session_id.as_str()) {
                continue;
            }
            if !seen.insert((candidate.root_id.clone(), candidate.content_hash.clone())) {
                continue;
            }
            evidence.push(candidate.into_evidence());
        }
        evidence.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.timestamp.cmp(&right.timestamp))
                .then_with(|| left.session_id.cmp(&right.session_id))
                .then_with(|| left.line.cmp(&right.line))
        });

        Ok(Dossier {
            schema_version: OUTPUT_SCHEMA_VERSION,
            ranker_version: RANKER_VERSION,
            query: redact_text(&analysis.query),
            intent: intent_name(analysis.intent).to_string(),
            matched_sessions,
            evidence,
        })
    }

    /// Returns the normalized repository keys to restrict retrieval to, when
    /// the query names at least one repository that exists in the index.
    ///
    /// Repository hints become a hard filter only when they match indexed
    /// sessions; otherwise retrieval falls back to the unrestricted search so
    /// a mistyped or unindexed name cannot silently empty the dossier.
    fn repo_scope(&self, analysis: &QueryAnalysis) -> Result<Option<Vec<String>>> {
        if analysis.repos.is_empty() {
            return Ok(None);
        }
        let mut keys: Vec<String> = analysis
            .repos
            .iter()
            .map(|hint| repo_key(&hint.name))
            .collect();
        keys.sort();
        keys.dedup();

        let placeholders = (1..=keys.len())
            .map(|position| format!("?{position}"))
            .collect::<Vec<_>>()
            .join(", ");
        let matches: i64 = self.connection().query_row(
            &format!(
                "SELECT count(*) FROM sessions
                 WHERE repo IS NOT NULL AND replace(lower(repo), '_', '-') IN ({placeholders})"
            ),
            rusqlite::params_from_iter(keys.iter()),
            |row| row.get(0),
        )?;
        Ok((matches > 0).then_some(keys))
    }

    fn search_candidates(
        &self,
        analysis: &QueryAnalysis,
        limit: usize,
        repo_scope: Option<&[String]>,
    ) -> Result<Vec<Candidate>> {
        let mut parameters: Vec<rusqlite::types::Value> = vec![analysis.fts_query.clone().into()];
        let mut scope_clause = String::new();
        if let Some(keys) = repo_scope {
            let placeholders = keys
                .iter()
                .enumerate()
                .map(|(index, _)| format!("?{}", index + 2))
                .collect::<Vec<_>>()
                .join(", ");
            scope_clause = format!(" AND replace(lower(s.repo), '_', '-') IN ({placeholders})");
            parameters.extend(keys.iter().map(|key| key.clone().into()));
        }
        let limit_placeholder = format!("?{}", parameters.len() + 1);
        parameters.push((limit.min(4_000) as i64).into());

        let mut statement = self.connection().prepare(&format!(
            "
            SELECT
              e.event_id, e.session_id, s.root_id, s.repo,
              s.provenance_status, e.seq, e.line_no, e.byte_offset,
              e.timestamp, e.turn_id, e.role, e.kind, e.text, s.path,
              e.content_hash, bm25(event_fts, 20.0, 3.0, 1.0, 7.0)
            FROM event_fts
            JOIN events e ON e.event_id = event_fts.event_id
            JOIN sessions s ON s.session_id = e.session_id
            WHERE event_fts MATCH ?1{scope_clause}
            ORDER BY bm25(event_fts, 20.0, 3.0, 1.0, 7.0), e.event_id
            LIMIT {limit_placeholder}
            "
        ))?;
        let rows =
            statement.query_map(rusqlite::params_from_iter(parameters), |row| {
                let rank: f64 = row.get(15)?;
                Ok(Candidate {
                    event_id: row.get(0)?,
                    session_id: row.get(1)?,
                    root_id: row.get(2)?,
                    repo: row.get(3)?,
                    provenance_status: row.get(4)?,
                    seq: row.get(5)?,
                    line: row.get::<_, i64>(6)? as u64,
                    byte_offset: row.get::<_, i64>(7)? as u64,
                    timestamp: row.get(8)?,
                    turn_id: row.get(9)?,
                    role: row.get(10)?,
                    kind: row.get(11)?,
                    text: row.get(12)?,
                    source_path: row.get(13)?,
                    content_hash: row.get(14)?,
                    score: (-rank * 1_000_000.0).round() as i64,
                })
            })?;
        let mut candidates: Vec<Candidate> = rows.collect::<rusqlite::Result<_>>()?;
        for candidate in &mut candidates {
            candidate.score += structured_boost(candidate, analysis);
        }

        if analysis.intent == Intent::Chronology {
            candidates.sort_by(|left, right| {
                left.timestamp
                    .cmp(&right.timestamp)
                    .then_with(|| left.event_id.cmp(&right.event_id))
            });
            for (position, candidate) in candidates.iter_mut().enumerate() {
                candidate.score += (position.min(100) as i64) * 10;
            }
        }
        Ok(candidates)
    }
}

impl Candidate {
    fn into_evidence(self) -> Evidence {
        Evidence {
            citation: format!("codex://session/{}#event-{}", self.session_id, self.seq),
            source: format!("{}:{}", safe_display_path(self.source_path), self.line),
            text: redact_text(&self.text),
            session_id: self.session_id,
            root_id: self.root_id,
            turn_id: self.turn_id,
            timestamp: self.timestamp,
            role: self.role,
            kind: self.kind,
            line: self.line,
            byte_offset: self.byte_offset,
            score: self.score,
        }
    }
}

fn structured_boost(candidate: &Candidate, analysis: &QueryAnalysis) -> i64 {
    let text = candidate.text.to_ascii_lowercase();
    let mut boost = 0_i64;

    for identifier in &analysis.exact.ids {
        if text.contains(&identifier.to_ascii_lowercase()) {
            boost += 4_000_000;
        }
    }
    for pr in &analysis.exact.prs {
        if mentions_pr_number(&text, *pr) {
            boost += 4_000_000;
        }
    }
    for version in &analysis.exact.versions {
        if mentions_version(&text, &version.to_ascii_lowercase()) {
            boost += 3_000_000;
        }
    }
    for commit in &analysis.exact.commits {
        if mentions_commit(&text, &commit.to_ascii_lowercase()) {
            boost += 4_000_000;
        }
    }

    if let Some(repo) = candidate.repo.as_deref() {
        let repo = repo_key(repo);
        for hint in &analysis.repos {
            if repo == repo_key(&hint.name) {
                boost += 6_000_000;
            }
        }
    }

    boost
        + match analysis.intent {
            Intent::Why if candidate.role == "user" => 1_500_000,
            Intent::Why if candidate.role == "assistant" => 900_000,
            Intent::How if candidate.kind == "task_complete" => 1_500_000,
            Intent::How if candidate.kind == "tool_call" => 800_000,
            Intent::Unfinished if candidate.kind == "task_complete" => 1_500_000,
            Intent::Model if candidate.kind == "turn_context" => 2_000_000,
            Intent::Attachment if text.contains("image") || text.contains("attachment") => {
                2_000_000
            }
            Intent::Chronology if candidate.timestamp.is_some() => 700_000,
            _ => 0,
        }
}

/// Finds `needle` in `text` where the surrounding characters keep the match a
/// whole identifier rather than a fragment of a longer one.
fn contains_bounded(
    text: &str,
    needle: &str,
    previous_allowed: impl Fn(Option<char>) -> bool,
    remainder_allowed: impl Fn(&str) -> bool,
) -> bool {
    if needle.is_empty() {
        return false;
    }
    let mut search_from = 0;
    while let Some(found) = text[search_from..].find(needle) {
        let begin = search_from + found;
        let end = begin + needle.len();
        if previous_allowed(text[..begin].chars().next_back()) && remainder_allowed(&text[end..]) {
            return true;
        }
        search_from = begin + 1;
    }
    false
}

/// `#44` and `pr 44` must not match `#442`; the digits end where the text's
/// digits end.
fn mentions_pr_number(text: &str, pr: u64) -> bool {
    let digits_continue = |rest: &str| rest.chars().next().is_some_and(|ch| ch.is_ascii_digit());
    contains_bounded(
        text,
        &format!("#{pr}"),
        |_| true,
        |rest| !digits_continue(rest),
    ) || contains_bounded(
        text,
        &format!("pr {pr}"),
        |_| true,
        |rest| !digits_continue(rest),
    )
}

/// `2.5.1` must not match inside `12.5.1`, `2.5.10`, `2.5.1.4`, or a
/// pre-release such as `2.5.1-beta`, which all name different versions.
fn mentions_version(text: &str, version: &str) -> bool {
    contains_bounded(
        text,
        version,
        |previous| !previous.is_some_and(|ch| ch.is_ascii_digit() || ch == '.'),
        |rest| {
            let mut rest_chars = rest.chars();
            match rest_chars.next() {
                Some(ch) if ch.is_ascii_alphanumeric() => false,
                Some('.') | Some('-') => !rest_chars
                    .next()
                    .is_some_and(|next| next.is_ascii_alphanumeric()),
                _ => true,
            }
        },
    )
}

/// A short hash must match a whole hex token, not the middle of a longer one.
fn mentions_commit(text: &str, commit: &str) -> bool {
    contains_bounded(
        text,
        commit,
        |previous| !previous.is_some_and(|ch| ch.is_ascii_alphanumeric()),
        |rest| {
            !rest
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_alphanumeric())
        },
    )
}

fn candidate_order(left: &Candidate, right: &Candidate) -> Ordering {
    right
        .score
        .cmp(&left.score)
        .then_with(|| left.session_id.cmp(&right.session_id))
        .then_with(|| left.seq.cmp(&right.seq))
}

fn intent_name(intent: Intent) -> &'static str {
    match intent {
        Intent::Why => "why",
        Intent::How => "how",
        Intent::Unfinished => "unfinished",
        Intent::Chronology => "chronology",
        Intent::Model => "model",
        Intent::Attachment => "attachment",
        Intent::General => "general",
    }
}

pub fn render_markdown(dossier: &Dossier) -> String {
    let mut output = format!(
        "# Dossier: {}\n\nIntent: `{}` · {} sessions · {} evidence events\n",
        single_line(&dossier.query),
        dossier.intent,
        dossier.matched_sessions.len(),
        dossier.evidence.len()
    );
    if dossier.evidence.is_empty() {
        output.push_str("\nNo matching Codex evidence was found.\n");
        return output;
    }
    output.push_str("\n## Evidence\n");
    for item in &dossier.evidence {
        output.push_str(&format!("\n- **{} / {}**", item.role, item.kind));
        if let Some(timestamp) = &item.timestamp {
            output.push_str(&format!(" · `{}`", single_line(timestamp)));
        }
        output.push('\n');
        for line in item.text.lines().take(80) {
            output.push_str("    ");
            output.push_str(line);
            output.push('\n');
        }
        output.push_str(&format!(
            "    `{}` · `{}`\n",
            item.citation,
            single_line(&item.source)
        ));
    }
    output.push_str("\nThis is cited evidence, not a generated answer.\n");
    output
}

fn single_line(value: &str) -> String {
    value.replace(['\r', '\n', '`'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(text: &str, repo: Option<&str>) -> Candidate {
        Candidate {
            event_id: 1,
            session_id: "session".into(),
            root_id: "session".into(),
            repo: repo.map(str::to_owned),
            provenance_status: "complete".into(),
            seq: 1,
            line: 1,
            byte_offset: 0,
            timestamp: None,
            turn_id: None,
            role: "assistant".into(),
            kind: "agent_message".into(),
            text: text.into(),
            source_path: "/tmp/session.jsonl".into(),
            content_hash: vec![0; 16],
            score: 0,
        }
    }

    #[test]
    fn pr_hint_does_not_match_a_longer_pr_number() {
        let analysis = analyze("What happened in PR #442?");

        assert_eq!(
            structured_boost(&candidate("Merged PR #4420 yesterday.", None), &analysis),
            0
        );
        assert!(structured_boost(&candidate("Merged PR #442 today.", None), &analysis) > 0);
        assert!(structured_boost(&candidate("Merged #442.", None), &analysis) > 0);
    }

    #[test]
    fn version_hint_does_not_match_inside_a_longer_version() {
        let analysis = analyze("What changed in v2.5.1?");

        assert_eq!(
            structured_boost(
                &candidate("Shipped 12.5.1 for the desktop app.", None),
                &analysis
            ),
            0
        );
        assert_eq!(
            structured_boost(&candidate("Shipped 2.5.10 last week.", None), &analysis),
            0
        );
        assert!(structured_boost(&candidate("Shipped 2.5.1 last week.", None), &analysis) > 0);
    }

    #[test]
    fn commit_hint_does_not_match_inside_a_longer_hash() {
        let analysis = analyze("Which task produced commit a49e613?");

        assert_eq!(
            structured_boost(&candidate("Reverted 0a49e613bb earlier.", None), &analysis),
            0
        );
        assert!(
            structured_boost(
                &candidate("Cherry-picked a49e613 cleanly.", None),
                &analysis
            ) > 0
        );
    }

    #[test]
    fn repo_boost_tolerates_underscore_and_case_differences() {
        let analysis = analyze("What happened in repository third-repo?");

        assert!(structured_boost(&candidate("Migration done.", Some("Third_Repo")), &analysis) > 0);
    }

    #[test]
    fn markdown_states_the_evidence_boundary() {
        let dossier = Dossier {
            schema_version: 1,
            ranker_version: 1,
            query: "why paused?".into(),
            intent: "why".into(),
            matched_sessions: Vec::new(),
            evidence: vec![Evidence {
                session_id: "demo".into(),
                root_id: "demo".into(),
                turn_id: None,
                timestamp: None,
                role: "assistant".into(),
                kind: "agent_message".into(),
                text: "Checksum mismatch.".into(),
                citation: "codex://session/demo#event-1".into(),
                source: "~/.codex/demo.jsonl:2".into(),
                line: 2,
                byte_offset: 10,
                score: 1,
            }],
        };
        let rendered = render_markdown(&dossier);
        assert!(rendered.contains("codex://session/demo#event-1"));
        assert!(rendered.contains("not a generated answer"));
    }
}
