/// Fixed-length mask used for redacted secrets — the same glyph as the config
/// UI, so a masked value leaks nothing about the original (length included).
pub const MASK: &str = "••••••••";

/// Key suffixes treated as secret whenever they appear in a `KEY: value` or
/// `KEY=value` line. Lowercased for comparison.
const SECRET_KEY_SUFFIXES: [&str; 6] = ["_key", "_secret", "_token", "_password", "_pass", "_dsn"];

/// Whole key names treated as secret regardless of suffix (they often carry
/// credentials after `scheme://`). Lowercased for comparison.
const SECRET_KEY_NAMES: [&str; 2] = ["database_url", "redis_url"];

/// Masks secrets in a run line before it is persisted:
///
/// 1. every literal from `secrets` (values the UI knows are sensitive, e.g.
///    config values being set) is replaced with the fixed-length mask;
/// 2. `KEY: value` / `KEY=value` pairs whose key names a secret are masked —
///    this covers dokku's `config:set` echo without knowing the value;
/// 3. credentials inside a URL (`scheme://user:pass@host/…`) are masked, so a
///    leaked connection string never persists with its password.
///
/// Pure and table-tested; the run line task applies it unconditionally.
pub fn redact_line(line: &str, secrets: &[String]) -> String {
    let mut out = line.to_owned();
    for secret in secrets {
        if !secret.is_empty() {
            out = out.replace(secret, MASK);
        }
    }
    out = mask_url_credentials(&out);
    out = mask_key_value(&out);
    out
}

/// Masks `scheme://user:pass@host` down to `scheme://••••••••@host` (only the
/// first credential segment is expected in practice). Lines without a URL or
/// with no credentials pass through untouched.
fn mask_url_credentials(line: &str) -> String {
    let Some(scheme) = line.find("://") else {
        return line.to_owned();
    };
    let credentials_start = scheme + 3;
    let Some(at_offset) = line[credentials_start..].find('@') else {
        return line.to_owned();
    };
    let at = credentials_start + at_offset;
    if at == credentials_start {
        return line.to_owned();
    }
    format!("{}{}{}", &line[..credentials_start], MASK, &line[at..])
}

/// Masks the value of `KEY: value` / `KEY=value` pairs whose key is secret.
fn mask_key_value(line: &str) -> String {
    let Some((left, right, separator)) = split_key_value(line) else {
        return line.to_owned();
    };
    let key = left.trim().to_lowercase();
    if key.is_empty() {
        return line.to_owned();
    }
    let secret = SECRET_KEY_SUFFIXES
        .iter()
        .any(|suffix| key.ends_with(*suffix))
        || SECRET_KEY_NAMES.iter().any(|name| *name == key);
    if !secret || right.trim().is_empty() {
        return line.to_owned();
    }
    format!("{}{}{}", left.trim_end(), separator, MASK)
}

/// Splits the first `KEY: value` or `KEY=value` pair in `line`.
fn split_key_value(line: &str) -> Option<(String, String, &'static str)> {
    if let Some((left, right)) = line.split_once(':') {
        return Some((left.to_owned(), right.to_owned(), ": "));
    }
    if let Some((left, right)) = line.split_once('=') {
        return Some((left.to_owned(), right.to_owned(), "="));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_literal_secrets_anywhere_in_the_line() {
        assert_eq!(
            redact_line("Setting FOO to hunter2 for the app", &["hunter2".into()]),
            format!("Setting FOO to {MASK} for the app")
        );
    }

    #[test]
    fn empty_and_missing_literals_change_nothing() {
        assert_eq!(redact_line("plain line", &[]), "plain line");
        assert_eq!(redact_line("plain line", &["".into()]), "plain line");
        assert_eq!(
            redact_line("no match here", &["absent".into()]),
            "no match here"
        );
    }

    #[test]
    fn masks_secret_key_value_pairs() {
        assert_eq!(
            redact_line("DATABASE_URL: postgres://user:pass@host/db", &[]),
            format!("DATABASE_URL: {MASK}")
        );
        assert_eq!(
            redact_line("SECRET_KEY: s3cr3t", &[]),
            format!("SECRET_KEY: {MASK}")
        );
        assert_eq!(
            redact_line("REDIS_PASSWORD=hunter2", &[]),
            format!("REDIS_PASSWORD={MASK}")
        );
    }

    #[test]
    fn leaves_innocuous_key_value_pairs_alone() {
        assert_eq!(redact_line("PORT: 8080", &[]), "PORT: 8080");
        assert_eq!(
            redact_line("DOKKU_PROXY_PORT=80", &[]),
            "DOKKU_PROXY_PORT=80"
        );
        assert_eq!(
            redact_line("2026-01-01T10:00:00Z app[web.1]: GET /healthz 200", &[]),
            "2026-01-01T10:00:00Z app[web.1]: GET /healthz 200"
        );
    }

    #[test]
    fn masks_url_credentials_without_touching_the_rest() {
        assert_eq!(
            redact_line(
                "Connecting to postgres://alice:hunter2@db.example:5432/app",
                &[]
            ),
            format!("Connecting to postgres://{MASK}@db.example:5432/app")
        );
        assert_eq!(
            redact_line("https://user@host/path", &[]),
            format!("https://{MASK}@host/path")
        );
    }

    #[test]
    fn url_mask_requires_an_at_sign_after_the_scheme() {
        assert_eq!(
            redact_line("https://example.com/path", &[]),
            "https://example.com/path"
        );
        assert_eq!(redact_line("no scheme here", &[]), "no scheme here");
        assert_eq!(
            redact_line("plain@host but no scheme", &[]),
            "plain@host but no scheme"
        );
    }

    #[test]
    fn masking_is_an_idempotent_fixed_length_replacement() {
        let once = redact_line("MY_TOKEN: abc123", &[]);
        let twice = redact_line(&once, &[]);
        assert_eq!(once, twice);
        assert_eq!(once, format!("MY_TOKEN: {MASK}"));
    }
}
