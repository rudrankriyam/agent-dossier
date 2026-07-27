use regex::Regex;
use std::path::Path;
use std::sync::OnceLock;

const REDACTED_SECRET: &str = "[REDACTED:SECRET]";

/// Removes credentials and terminal-control characters before text is indexed
/// or displayed.
pub fn redact_text(input: &str) -> String {
    static PRIVATE_KEY: OnceLock<Regex> = OnceLock::new();
    static AUTHORIZATION: OnceLock<Regex> = OnceLock::new();
    static JWT: OnceLock<Regex> = OnceLock::new();
    static KNOWN_TOKEN: OnceLock<Regex> = OnceLock::new();
    static JSON_SECRET: OnceLock<Regex> = OnceLock::new();
    static DOTENV_SECRET: OnceLock<Regex> = OnceLock::new();
    static LABELED_SECRET: OnceLock<Regex> = OnceLock::new();

    let mut text = strip_terminal_controls(input);

    text = regex(
        &PRIVATE_KEY,
        r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?-----END [A-Z0-9 ]*PRIVATE KEY-----",
    )
    .replace_all(&text, "[REDACTED:PRIVATE_KEY]")
    .into_owned();

    text = regex(
        &AUTHORIZATION,
        r"(?-u:\b)((?:Bearer|bearer|BEARER)[ \t]+[A-Za-z0-9._~+/=-]{4,}|(?:Basic|basic|BASIC)[ \t]+[A-Za-z0-9+/]{4,}={0,2})",
    )
    .replace_all(&text, |captures: &regex::Captures<'_>| {
        if captures[1].to_ascii_lowercase().starts_with("basic") {
            "Basic [REDACTED:BASIC]"
        } else {
            "Bearer [REDACTED:BEARER]"
        }
    })
    .into_owned();

    text = regex(
        &JWT,
        r"(?-u:\b)eyJ[A-Za-z0-9_-]{4,}\.[A-Za-z0-9_-]{4,}\.[A-Za-z0-9_-]{4,}(?-u:\b)",
    )
    .replace_all(&text, "[REDACTED:JWT]")
    .into_owned();

    text = regex(
        &KNOWN_TOKEN,
        r"(?x)(?-u:\b)(?:
            sk-(?:proj-)?[A-Za-z0-9_-]{8,}
            | github_pat_[A-Za-z0-9_]{8,}
            | gh[pousr]_[A-Za-z0-9]{8,}
            | xox[baprs]-[A-Za-z0-9-]{8,}
            | npm_[A-Za-z0-9]{8,}
            | AIza[A-Za-z0-9_-]{12,}
            | AKIA[0-9A-Z]{16}
        )(?-u:\b)",
    )
    .replace_all(&text, "[REDACTED:TOKEN]")
    .into_owned();

    text = regex(
        &JSON_SECRET,
        r#"("(?:[^"]*_)?(?:api[_-]?key|access[_-]?token|refresh[_-]?token|token|secret|client[_-]?secret|password|passwd|private[_-]?key|authorization|cookie|API[_-]?KEY|ACCESS[_-]?TOKEN|REFRESH[_-]?TOKEN|TOKEN|SECRET|CLIENT[_-]?SECRET|PASSWORD|PASSWD|PRIVATE[_-]?KEY|AUTHORIZATION|COOKIE)"[ \t]*:[ \t]*")([^"\r\n]*)(")"#,
    )
    .replace_all(&text, |captures: &regex::Captures<'_>| {
        format!("{}{REDACTED_SECRET}{}", &captures[1], &captures[3])
    })
    .into_owned();

    text = regex(
        &DOTENV_SECRET,
        r#"(?m)^([ \t]*(?:(?:export|EXPORT)[ \t]+)?[A-Za-z][A-Za-z0-9_]*(?:API_?KEY|TOKEN|SECRET|PASSWORD|PASSWD|PRIVATE_?KEY|CLIENT_?SECRET|AUTHORIZATION|COOKIE|api_?key|token|secret|password|passwd|private_?key|client_?secret|authorization|cookie)[A-Za-z0-9_]*[ \t]*=[ \t]*)(?:"[^"\r\n]*"|'[^'\r\n]*'|[^\x20\t#\r\n]+)"#,
    )
    .replace_all(&text, |captures: &regex::Captures<'_>| {
        format!("{}{REDACTED_SECRET}", &captures[1])
    })
    .into_owned();

    text = regex(
        &LABELED_SECRET,
        r#"(?-u:\b)((?:api[_-]?key|access[_-]?token|refresh[_-]?token|token|secret|client[_-]?secret|password|passwd|private[_-]?key|authorization|cookie|API[_-]?KEY|ACCESS[_-]?TOKEN|REFRESH[_-]?TOKEN|TOKEN|SECRET|CLIENT[_-]?SECRET|PASSWORD|PASSWD|PRIVATE[_-]?KEY|AUTHORIZATION|COOKIE)[ \t]*[:=][ \t]*)(?:"[^"\r\n]*"|'[^'\r\n]*'|[^\x20\t,;\r\n]+)"#,
    )
    .replace_all(&text, |captures: &regex::Captures<'_>| {
        format!("{}{REDACTED_SECRET}", &captures[1])
    })
    .into_owned();

    elide_home(&text)
}

/// Returns a printable path without revealing the current user's home
/// directory or allowing terminal escape sequences through.
pub fn safe_display_path(path: impl AsRef<Path>) -> String {
    redact_text(&path.as_ref().to_string_lossy()).replace('\\', "/")
}

fn regex<'a>(slot: &'a OnceLock<Regex>, pattern: &str) -> &'a Regex {
    slot.get_or_init(|| {
        Regex::new(pattern)
            .unwrap_or_else(|error| panic!("invalid redaction regex {pattern:?}: {error}"))
    })
}

fn elide_home(input: &str) -> String {
    let Some(home) = dirs::home_dir() else {
        return input.to_owned();
    };
    let home = home.to_string_lossy();
    if home.is_empty() {
        input.to_owned()
    } else {
        input.replace(home.as_ref(), "~")
    }
}

fn strip_terminal_controls(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(character) = chars.next() {
        match character {
            '\u{001b}' => match chars.next() {
                Some('[') => consume_csi(&mut chars),
                Some(']') => consume_osc(&mut chars),
                Some(_) | None => {}
            },
            '\u{009b}' => consume_csi(&mut chars),
            '\u{009d}' => consume_osc(&mut chars),
            '\n' | '\r' | '\t' => output.push(character),
            '\u{0000}'..='\u{001f}'
            | '\u{007f}'..='\u{009f}'
            | '\u{061c}'
            | '\u{200e}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}' => {}
            _ => output.push(character),
        }
    }

    output
}

fn consume_csi(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    for character in chars.by_ref() {
        if ('\u{0040}'..='\u{007e}').contains(&character) {
            break;
        }
    }
}

fn consume_osc(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(character) = chars.next() {
        match character {
            '\u{0007}' | '\u{009c}' => break,
            '\u{001b}' if chars.peek() == Some(&'\\') => {
                chars.next();
                break;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{redact_text, safe_display_path};

    #[test]
    fn redacts_bearer_and_basic_authorization() {
        let input = "Authorization: Bearer abc.def-123_456\nBasic dXNlcjpwYXNzd29yZA==";
        let redacted = redact_text(input);

        assert!(!redacted.contains("abc.def-123_456"));
        assert!(!redacted.contains("dXNlcjpwYXNzd29yZA=="));
        assert!(redacted.contains("[REDACTED:BEARER]"));
        assert!(redacted.contains("[REDACTED:BASIC]"));
    }

    #[test]
    fn redacts_jwts() {
        // Assemble scanner-shaped fixtures at runtime so repository secret
        // scanners do not mistake synthetic test data for a live credential.
        let jwt = [
            "ey",
            "JhbGciOiJIUzI1NiJ9.",
            "ey",
            "JzdWIiOiIxMjM0NTY3ODkwIn0.",
            "signature123",
        ]
        .concat();
        let redacted = redact_text(&format!("session={jwt}"));

        assert!(!redacted.contains(&jwt));
        assert!(redacted.contains("[REDACTED:JWT]"));
    }

    #[test]
    fn redacts_known_token_prefixes() {
        let tokens = vec![
            ["s", "k-proj-abcdefghijklmnop"].concat(),
            ["github_", "pat_1234567890abcdef"].concat(),
            ["gh", "p_1234567890abcdef"].concat(),
            ["xo", "xb-1234567890-abcdef"].concat(),
            ["np", "m_1234567890abcdef"].concat(),
            ["AI", "zaSyD1234567890abcdefghij"].concat(),
            ["AK", "IAIOSFODNN7EXAMPLE"].concat(),
        ];
        let redacted = redact_text(&tokens.join("\n"));

        for token in &tokens {
            assert!(!redacted.contains(token), "token leaked");
        }
        assert_eq!(redacted.matches("[REDACTED:TOKEN]").count(), tokens.len());
    }

    #[test]
    fn redacts_private_keys() {
        let input = format!(
            "before\n-----BEGIN {} PRIVATE KEY-----\nsecret material\n-----END {} PRIVATE KEY-----\nafter",
            "OPENSSH", "OPENSSH"
        );
        let redacted = redact_text(&input);

        assert_eq!(redacted, "before\n[REDACTED:PRIVATE_KEY]\nafter");
    }

    #[test]
    fn redacts_json_and_dotenv_secret_values() {
        let input = concat!(
            r#"{"api_key":"json-secret","name":"public","nested_access_token":"token-value"}"#,
            "\nOPENAI_API_KEY=dotenv-secret",
            "\nexport DATABASE_PASSWORD=\"database-secret\"",
            "\nTEAM_ID=YQZQG7N4WG",
        );
        let redacted = redact_text(input);

        for secret in [
            "json-secret",
            "token-value",
            "dotenv-secret",
            "database-secret",
        ] {
            assert!(!redacted.contains(secret), "secret leaked: {secret}");
        }
        assert!(redacted.contains(r#""name":"public""#));
        assert!(redacted.contains("TEAM_ID=YQZQG7N4WG"));
    }

    #[test]
    fn strips_ansi_osc_bidi_and_other_controls() {
        let input = "safe\x1b[31mred\x1b[0m \x1b]8;;https://evil.test\x07link\x1b]8;;\x1b\\ \u{202e}txt\u{2066}\0done";

        assert_eq!(redact_text(input), "safered link txtdone");
    }

    #[test]
    fn elides_home_paths() {
        let home = dirs::home_dir().expect("test requires a home directory");
        let nested = home.join("Documents").join("private-project");

        assert_eq!(
            safe_display_path(&nested),
            "~/Documents/private-project".to_owned()
        );
        assert_eq!(
            redact_text(&nested.to_string_lossy()),
            "~/Documents/private-project"
        );
    }

    #[test]
    fn preserves_public_ids_and_hashes() {
        let input = concat!(
            "APP_ID=6567933550\n",
            "TEAM_ID=YQZQG7N4WG\n",
            "ISSUER_ID=69a6de95-7d54-47e3-e053-5b8c7c11a4d1\n",
            "KEY_ID=ABC123DEFG\n",
            "commit 0123456789abcdef0123456789abcdef01234567\n",
            "model=gpt-5-codex",
        );

        assert_eq!(redact_text(input), input);
    }
}
