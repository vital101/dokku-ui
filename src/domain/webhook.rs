use ring::hmac;
use subtle::ConstantTimeEq;

/// GitHub's HMAC header, `X-Hub-Signature-256`.
pub const SIGNATURE_HEADER: &str = "X-Hub-Signature-256";

/// Verifies a GitHub webhook signature in constant time. The header carries
/// `sha256=<64 hex chars>`; malformed headers are rejected without comparing.
pub fn verify_github_signature(secret: &str, body: &[u8], header: &str) -> bool {
    let Some(hex) = header.strip_prefix("sha256=") else {
        return false;
    };
    let Some(expected) = decode_hex(hex) else {
        return false;
    };
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    let tag = hmac::sign(&key, body);
    tag.as_ref().ct_eq(&expected).into()
}

fn decode_hex(raw: &str) -> Option<Vec<u8>> {
    if raw.len() != 64 {
        return None;
    }
    (0..raw.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&raw[index..index + 2], 16).ok())
        .collect()
}

/// Whether a push payload comes from the configured repository. GitHub sends
/// `repository.full_name` (`org/repo`) plus clone/ssh/https URLs; any of them
/// may match the configured remote after normalization.
pub fn repo_matches(configured: &str, payload: &serde_json::Value) -> bool {
    let Some(configured) = normalize_repo_url(configured) else {
        return false;
    };
    let repository = payload.get("repository");
    // `full_name` has no host, so it is only meaningful for a github.com
    // remote; otherwise a GitLab repo with the same owner/name would match.
    if configured.starts_with("github.com/") {
        if let Some(full_name) = repository
            .and_then(|repo| repo.get("full_name"))
            .and_then(|value| value.as_str())
        {
            let tail = configured.split_once('/').map(|(_, tail)| tail);
            if tail.is_some_and(|tail| full_name.eq_ignore_ascii_case(tail)) {
                return true;
            }
        }
    }
    ["clone_url", "ssh_url", "git_url", "html_url"]
        .iter()
        .any(|key| {
            repository
                .and_then(|repo| repo.get(key))
                .and_then(|value| value.as_str())
                .and_then(normalize_repo_url)
                .is_some_and(|candidate| candidate == configured)
        })
}

/// `host/owner/repo`, lowercased, from any of the common remote URL shapes
/// (`https://`, `ssh://`, scp-style `git@host:owner/repo`, bare `owner/repo`).
fn normalize_repo_url(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    if url.is_empty() {
        return None;
    }
    let rest = match url.split_once("://") {
        Some((_, rest)) => rest,
        None => url,
    };
    // Strip any userinfo (`git@`, `user:token@`) from both URL forms.
    let rest = match rest.split_once('@') {
        Some((user, rest)) if !user.contains('/') => rest,
        _ => rest,
    };
    let rest = rest.trim_start_matches('/');
    if rest.is_empty() {
        return None;
    }
    let (host, path) = match rest.split_once(':') {
        Some((host, path)) => (host, path),
        None => rest.split_once('/')?,
    };
    let path = path.trim_start_matches('/');
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("{host}/{path}").to_ascii_lowercase())
}

/// Whether the push targets `refs/heads/<configured_branch>`.
pub fn push_branch_matches(configured_branch: &str, payload: &serde_json::Value) -> bool {
    payload
        .get("ref")
        .and_then(|value| value.as_str())
        .is_some_and(|reference| reference == format!("refs/heads/{configured_branch}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sign(secret: &str, body: &[u8]) -> String {
        let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
        let tag = hmac::sign(&key, body);
        tag.as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    }

    #[test]
    fn valid_signatures_accept_and_tampered_ones_reject() {
        let body = br#"{"ref":"refs/heads/main"}"#;
        let header = format!("sha256={}", sign("s3cr3t", body));
        assert!(verify_github_signature("s3cr3t", body, &header));
        assert!(!verify_github_signature("wrong", body, &header));
        assert!(!verify_github_signature(
            "s3cr3t",
            br#"{"ref":"refs/heads/other"}"#,
            &header
        ));
    }

    #[test]
    fn malformed_signature_headers_are_rejected() {
        let body = b"payload";
        for header in [
            "",
            "sha256=",
            "sha1=abcd",
            "sha256=zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
            "sha256=abc123",
        ] {
            assert!(!verify_github_signature("s3cr3t", body, header), "{header}");
        }
        let valid = format!("sha256={}", sign("s3cr3t", body));
        assert!(verify_github_signature("s3cr3t", body, &valid));
    }

    #[test]
    fn repo_matching_handles_every_url_shape() {
        let payload = serde_json::json!({
            "repository": {
                "full_name": "Org/Repo",
                "clone_url": "https://github.com/Org/Repo.git",
                "ssh_url": "git@github.com:Org/Repo.git",
                "html_url": "https://github.com/Org/Repo",
            }
        });
        for configured in [
            "https://github.com/org/repo.git",
            "ssh://git@github.com/org/repo.git",
            "git@github.com:org/repo.git",
            "https://github.com/org/repo",
        ] {
            assert!(repo_matches(configured, &payload), "{configured}");
        }
        assert!(!repo_matches("https://github.com/other/repo.git", &payload));
        assert!(!repo_matches("https://gitlab.com/org/repo.git", &payload));
        let empty = serde_json::json!({});
        assert!(!repo_matches("https://github.com/org/repo.git", &empty));
    }

    #[test]
    fn branch_matching_requires_the_exact_ref() {
        let payload = serde_json::json!({"ref": "refs/heads/main"});
        assert!(push_branch_matches("main", &payload));
        assert!(!push_branch_matches("develop", &payload));
        assert!(!push_branch_matches("", &payload));
        assert!(!push_branch_matches("main", &serde_json::json!({})));
    }
}
