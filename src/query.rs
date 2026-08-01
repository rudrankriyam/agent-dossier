use std::collections::HashSet;
use std::sync::OnceLock;

use regex::Regex;

pub const MAX_QUERY_CHARS: usize = 2_048;
pub const MAX_FTS_TERMS: usize = 32;
pub const MAX_TERM_CHARS: usize = 64;
pub const MAX_HINTS_PER_KIND: usize = 16;
pub const MAX_REPO_HINTS: usize = 8;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Intent {
    Why,
    How,
    Unfinished,
    Chronology,
    Model,
    Attachment,
    #[default]
    General,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExactHints {
    /// UUIDs and identifiers explicitly introduced as session, task, turn, or IDs.
    pub ids: Vec<String>,
    pub prs: Vec<u64>,
    /// Versions are normalized by removing a leading `v`.
    pub versions: Vec<String>,
    pub commits: Vec<String>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RepoHint {
    pub owner: Option<String>,
    pub name: String,
}

impl RepoHint {
    pub fn canonical(&self) -> String {
        match &self.owner {
            Some(owner) => format!("{owner}/{}", self.name),
            None => self.name.clone(),
        }
    }
}

/// Normalizes a repository name for identity comparison.
///
/// Checkout directory names and spoken repository names disagree on case and
/// on `_` versus `-` far more often than they disagree on anything else, so
/// both collapse to one key before comparison.
pub fn repo_key(value: &str) -> String {
    value.to_ascii_lowercase().replace('_', "-")
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryAnalysis {
    /// The bounded input used for all analysis.
    pub query: String,
    pub normalized: String,
    pub terms: Vec<String>,
    pub fts_query: String,
    pub exact: ExactHints,
    pub repos: Vec<RepoHint>,
    pub intent: Intent,
}

pub fn analyze(query: &str) -> QueryAnalysis {
    let query = truncate_chars(query.trim(), MAX_QUERY_CHARS);
    let raw_tokens = tokenize(&query);
    let normalized = raw_tokens.join(" ");
    let terms = select_terms(&raw_tokens);
    let exact = extract_exact_hints(&query);
    let repos = extract_repo_hints(&query);
    let intent = classify_intent(&raw_tokens, &normalized, &exact);
    let fts_query = build_fts_or_query(&terms);

    QueryAnalysis {
        query,
        normalized,
        terms,
        fts_query,
        exact,
        repos,
        intent,
    }
}

/// Builds an FTS5 expression in which every term is a quoted literal.
///
/// Quoting each term prevents query punctuation and operators from changing the
/// expression's shape. Empty terms are ignored and the same public cap used by
/// `analyze` is applied defensively.
pub fn build_fts_or_query<T: AsRef<str>>(terms: &[T]) -> String {
    terms
        .iter()
        .map(AsRef::as_ref)
        .filter(|term| !term.is_empty())
        .take(MAX_FTS_TERMS)
        .map(quote_fts_literal)
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn quote_fts_literal(term: &str) -> String {
    format!("\"{}\"", term.replace('"', "\"\""))
}

fn truncate_chars(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        value.to_owned()
    } else {
        value.chars().take(limit).collect()
    }
}

fn tokenize(value: &str) -> Vec<String> {
    let chars: Vec<char> = value.chars().collect();
    let mut tokens = Vec::new();
    let mut current = String::new();

    for (index, &ch) in chars.iter().enumerate() {
        if !ch.is_alphanumeric() {
            push_token(&mut tokens, &mut current);
            continue;
        }

        let previous = index.checked_sub(1).and_then(|i| chars.get(i)).copied();
        let next = chars.get(index + 1).copied();
        let camel_boundary = !current.is_empty()
            && ch.is_uppercase()
            && (previous.is_some_and(|prev| prev.is_lowercase())
                || (previous.is_some_and(|prev| prev.is_uppercase())
                    && next.is_some_and(|next| next.is_lowercase())));
        let alpha_numeric_boundary = !current.is_empty()
            && previous.is_some_and(|prev| {
                (prev.is_alphabetic() && ch.is_numeric())
                    || (prev.is_numeric() && ch.is_alphabetic())
            });

        if camel_boundary || alpha_numeric_boundary {
            push_token(&mut tokens, &mut current);
        }

        for lower in ch.to_lowercase() {
            if current.chars().count() < MAX_TERM_CHARS {
                current.push(lower);
            }
        }
    }

    push_token(&mut tokens, &mut current);
    tokens
}

fn push_token(tokens: &mut Vec<String>, current: &mut String) {
    if !current.is_empty() {
        tokens.push(std::mem::take(current));
    }
}

fn select_terms(tokens: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut selected = Vec::new();

    for token in tokens {
        if selected.len() == MAX_FTS_TERMS {
            break;
        }
        if token.chars().count() < 2 && !token.chars().all(char::is_numeric) {
            continue;
        }
        if is_stopword(token) || !seen.insert(token.clone()) {
            continue;
        }
        selected.push(token.clone());
    }

    selected
}

fn is_stopword(token: &str) -> bool {
    matches!(
        token,
        "a" | "an"
            | "and"
            | "are"
            | "as"
            | "at"
            | "be"
            | "been"
            | "but"
            | "by"
            | "can"
            | "could"
            | "did"
            | "do"
            | "does"
            | "for"
            | "from"
            | "had"
            | "has"
            | "have"
            | "i"
            | "if"
            | "in"
            | "into"
            | "is"
            | "it"
            | "me"
            | "my"
            | "of"
            | "on"
            | "or"
            | "our"
            | "please"
            | "that"
            | "the"
            | "their"
            | "then"
            | "there"
            | "these"
            | "they"
            | "this"
            | "to"
            | "us"
            | "was"
            | "we"
            | "were"
            | "what"
            | "when"
            | "where"
            | "which"
            | "who"
            | "with"
            | "would"
            | "you"
            | "your"
    )
}

fn extract_exact_hints(query: &str) -> ExactHints {
    let mut hints = ExactHints::default();
    let mut uuid_ranges = Vec::new();

    for matched in uuid_regex().find_iter(query) {
        uuid_ranges.push(matched.range());
        push_unique(&mut hints.ids, matched.as_str().to_ascii_lowercase());
    }

    for captures in labelled_id_regex().captures_iter(query) {
        if let Some(value) = captures.get(1).or_else(|| captures.get(2)) {
            push_unique(&mut hints.ids, value.as_str().to_ascii_lowercase());
        }
    }

    for captures in loose_session_id_regex().captures_iter(query) {
        if let Some(value) = captures.get(1) {
            let value = value.as_str();
            if value.len() >= 8 && value.chars().any(|ch| ch.is_ascii_digit()) {
                push_unique(&mut hints.ids, value.to_ascii_lowercase());
            }
        }
    }

    for captures in pr_regex().captures_iter(query) {
        if let Some(value) = captures
            .get(1)
            .or_else(|| captures.get(2))
            .and_then(|m| m.as_str().parse().ok())
        {
            push_unique(&mut hints.prs, value);
        }
    }

    for captures in version_regex().captures_iter(query) {
        if let Some(value) = captures.get(1) {
            push_unique(&mut hints.versions, value.as_str().to_ascii_lowercase());
        }
    }

    for matched in commit_regex().find_iter(query) {
        if uuid_ranges
            .iter()
            .any(|range| matched.start() < range.end && range.start < matched.end())
        {
            continue;
        }
        let value = matched.as_str();
        if value
            .bytes()
            .any(|byte| matches!(byte, b'a'..=b'f' | b'A'..=b'F'))
        {
            push_unique(&mut hints.commits, value.to_ascii_lowercase());
        }
    }

    hints.ids.truncate(MAX_HINTS_PER_KIND);
    hints.prs.truncate(MAX_HINTS_PER_KIND);
    hints.versions.truncate(MAX_HINTS_PER_KIND);
    hints.commits.truncate(MAX_HINTS_PER_KIND);
    hints
}

fn extract_repo_hints(query: &str) -> Vec<RepoHint> {
    let mut hints = Vec::new();
    let mut seen = HashSet::new();
    let mut github_ranges = Vec::new();

    for captures in github_repo_regex().captures_iter(query) {
        let Some(whole) = captures.get(0) else {
            continue;
        };
        let Some(owner) = captures.get(1) else {
            continue;
        };
        let Some(name) = captures.get(2) else {
            continue;
        };
        github_ranges.push(whole.range());
        add_repo_hint(
            &mut hints,
            &mut seen,
            Some(owner.as_str()),
            trim_git_suffix(name.as_str()),
        );
    }

    for captures in owner_repo_regex().captures_iter(query) {
        let Some(whole) = captures.get(0) else {
            continue;
        };
        if github_ranges
            .iter()
            .any(|range| whole.start() < range.end && range.start < whole.end())
        {
            continue;
        }
        let Some(owner) = captures.get(1) else {
            continue;
        };
        let Some(name) = captures.get(2) else {
            continue;
        };
        if is_path_prefix(owner.as_str()) {
            continue;
        }
        add_repo_hint(
            &mut hints,
            &mut seen,
            Some(owner.as_str()),
            trim_git_suffix(name.as_str()),
        );
    }

    for captures in named_repo_regex().captures_iter(query) {
        if let Some(name) = captures.get(1) {
            add_repo_hint(&mut hints, &mut seen, None, trim_git_suffix(name.as_str()));
        }
    }

    // A multi-part slug is often the only repository clue in a natural-language
    // query. It remains a soft hint; callers should use owner-qualified hints as
    // hard filters when available.
    for matched in slug_regex().find_iter(query) {
        let slug = trim_git_suffix(matched.as_str());
        if slug.matches('-').count() >= 2 && !version_regex().is_match(slug) {
            add_repo_hint(&mut hints, &mut seen, None, slug);
        }
    }

    hints.truncate(MAX_REPO_HINTS);
    hints
}

fn add_repo_hint(
    hints: &mut Vec<RepoHint>,
    seen: &mut HashSet<String>,
    owner: Option<&str>,
    name: &str,
) {
    let name = name
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-' && ch != '_')
        .to_ascii_lowercase();
    if name.len() < 2 || is_stopword(&name) {
        return;
    }
    let owner = owner.map(|value| value.to_ascii_lowercase());
    let key = match &owner {
        Some(owner) => format!("{owner}/{name}"),
        None => name.clone(),
    };
    if seen.insert(key) {
        hints.push(RepoHint { owner, name });
    }
}

fn trim_git_suffix(value: &str) -> &str {
    value.strip_suffix(".git").unwrap_or(value)
}

fn is_path_prefix(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "bin"
            | "dev"
            | "etc"
            | "home"
            | "opt"
            | "private"
            | "src"
            | "tmp"
            | "users"
            | "usr"
            | "var"
            | "volumes"
    )
}

fn classify_intent(tokens: &[String], normalized: &str, exact: &ExactHints) -> Intent {
    let has = |needle: &str| tokens.iter().any(|token| token == needle);
    let has_any = |needles: &[&str]| needles.iter().any(|needle| has(needle));

    let why = if has("why") {
        12
    } else if has_any(&["rationale", "reason", "reasons"]) {
        7
    } else if normalized.contains("decision") {
        4
    } else {
        0
    };
    let how = if has("how") {
        10
    } else if has_any(&["implementation", "implemented", "approach"]) {
        5
    } else {
        0
    };
    let unfinished = if has_any(&[
        "unfinished",
        "incomplete",
        "remaining",
        "remains",
        "pending",
        "todo",
    ]) || normalized.contains("left to do")
        || normalized.contains("still needs")
    {
        10
    } else {
        0
    };
    let chronology = if has_any(&["chronology", "timeline", "sequence"]) {
        11
    } else if has_any(&["before", "after", "between"])
        || (exact.versions.len() >= 2 && has_any(&["from", "through", "until", "to"]))
        || normalized.contains("over time")
    {
        7
    } else {
        0
    };
    let model = if has_any(&["model", "models"]) {
        9
    } else if normalized.contains("reasoning effort") || has_any(&["handoff", "handoffs"]) {
        6
    } else {
        0
    };
    let attachment = if has_any(&[
        "attachment",
        "attachments",
        "screenshot",
        "screenshots",
        "image",
        "images",
        "photo",
        "photos",
    ]) {
        9
    } else if normalized.contains("file lineage") || normalized.contains("visual input") {
        6
    } else {
        0
    };

    [
        (why, Intent::Why),
        (unfinished, Intent::Unfinished),
        (chronology, Intent::Chronology),
        (model, Intent::Model),
        (attachment, Intent::Attachment),
        (how, Intent::How),
    ]
    .into_iter()
    .max_by_key(|(score, _)| *score)
    .filter(|(score, _)| *score > 0)
    .map_or(Intent::General, |(_, intent)| intent)
}

fn push_unique<T: Eq + std::hash::Hash + Clone>(values: &mut Vec<T>, value: T) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn uuid_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?i)\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b")
            .expect("valid UUID regex")
    })
}

fn labelled_id_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:session|thread|task|turn|run|workflow|build)(?:\s+id)?\s*[:=#]\s*([a-z0-9][a-z0-9_.:-]{2,127})\b|\bid\s*[:=#]\s*([a-z0-9][a-z0-9_.:-]{2,127})\b",
        )
        .expect("valid labelled ID regex")
    })
}

fn loose_session_id_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:session|thread|task|turn|run|workflow|build)\s+([a-z0-9][a-z0-9_.:-]{7,127})\b",
        )
        .expect("valid loose session ID regex")
    })
}

fn pr_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?i)\b(?:pr|pull[\s-]+request)\s*#?\s*(\d{1,12})\b|#(\d{1,12})\b")
            .expect("valid PR regex")
    })
}

fn version_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?i)\bv?([0-9]{1,4}(?:\.[0-9a-z]+){2,}(?:[-+][0-9a-z.-]+)?)\b")
            .expect("valid version regex")
    })
}

fn commit_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"(?i)\b[0-9a-f]{7,40}\b").expect("valid commit regex"))
}

fn github_repo_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"(?i)(?:https?://)?github\.com/([a-z0-9_.-]+)/([a-z0-9_.-]+)(?:/[\w./?#=&%-]*)?",
        )
        .expect("valid GitHub repository regex")
    })
}

fn owner_repo_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?i)\b([a-z0-9_.-]{1,64})/([a-z0-9_.-]{2,100})\b")
            .expect("valid owner/repository regex")
    })
}

fn named_repo_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r#"(?i)\b(?:repo|repository|project)\s+(?:named\s+|called\s+)?[`"']?([a-z0-9][a-z0-9_.-]{1,99})"#,
        )
        .expect("valid named repository regex")
    })
}

fn slug_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?i)\b[a-z0-9]+(?:-[a-z0-9]+){2,}(?:\.git)?\b")
            .expect("valid repository slug regex")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_punctuation_hyphens_camel_case_and_stopwords() {
        let analysis = analyze("What did App-Store ConnectCLI do for HTTPServer2?");

        assert_eq!(
            analysis.normalized,
            "what did app store connect cli do for http server 2"
        );
        assert_eq!(
            analysis.terms,
            ["app", "store", "connect", "cli", "http", "server", "2"]
        );
        assert_eq!(
            analysis.fts_query,
            r#""app" OR "store" OR "connect" OR "cli" OR "http" OR "server" OR "2""#
        );
    }

    #[test]
    fn extracts_exact_hints_without_treating_uuid_as_commit() {
        let analysis = analyze(
            "session 019f7194-e1ca-7690-8224-60757e3cd234, PR #442, \
             versions v2.5.1 and 2.6.0, commits a49e613 and c15f940.",
        );

        assert_eq!(analysis.exact.ids, ["019f7194-e1ca-7690-8224-60757e3cd234"]);
        assert_eq!(analysis.exact.prs, [442]);
        assert_eq!(analysis.exact.versions, ["2.5.1", "2.6.0"]);
        assert_eq!(analysis.exact.commits, ["a49e613", "c15f940"]);
    }

    #[test]
    fn extracts_qualified_named_and_slug_repo_hints() {
        let analysis = analyze(
            "Compare https://github.com/Owner/First.git with acme/second and \
             repository Third_Repo plus app-store-connect-cli.",
        );
        let canonical: Vec<_> = analysis.repos.iter().map(RepoHint::canonical).collect();

        assert_eq!(
            canonical,
            [
                "owner/first",
                "acme/second",
                "third_repo",
                "app-store-connect-cli"
            ]
        );
    }

    #[test]
    fn classifies_supported_intents() {
        let cases = [
            ("Why did we choose that dependency?", Intent::Why),
            ("How was the cache implemented?", Intent::How),
            ("What remains unfinished and pending?", Intent::Unfinished),
            (
                "Give me the chronology from 2.5.1 to 2.6.0",
                Intent::Chronology,
            ),
            (
                "Compare the model handoffs and reasoning effort",
                Intent::Model,
            ),
            (
                "Trace the screenshot attachment lineage",
                Intent::Attachment,
            ),
            ("Find the previous cache task", Intent::General),
        ];

        for (query, expected) in cases {
            assert_eq!(analyze(query).intent, expected, "{query}");
        }
    }

    #[test]
    fn why_takes_precedence_when_subject_is_a_model_or_attachment() {
        assert_eq!(
            analyze("Why did the model ignore the attachment?").intent,
            Intent::Why
        );
    }

    #[test]
    fn safely_quotes_fts_operators_and_quotes() {
        let terms = ["alpha OR beta", r#"quote"term"#, "-exclude"];

        assert_eq!(
            build_fts_or_query(&terms),
            r#""alpha OR beta" OR "quote""term" OR "-exclude""#
        );
    }

    #[test]
    fn caps_query_terms_and_hints_deterministically() {
        let long = (0..100)
            .map(|index| format!("term{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        let first = analyze(&long);
        let second = analyze(&long);

        assert_eq!(first, second);
        assert_eq!(first.terms.len(), MAX_FTS_TERMS);
        assert!(first.query.chars().count() <= MAX_QUERY_CHARS);
    }

    #[test]
    fn does_not_turn_a_local_path_prefix_into_an_owner() {
        let analysis = analyze("Inspect Users/rudrank and real-owner/real-repo.");
        let canonical: Vec<_> = analysis.repos.iter().map(RepoHint::canonical).collect();

        assert_eq!(canonical, ["real-owner/real-repo"]);
    }
}
