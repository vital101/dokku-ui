//! Validation for the SSH-keys admin screen (`ssh-keys:list/add/remove`).
//!
//! Key *names* travel as SSH argv, so they must be single-line, quote-free,
//! and whitespace-free. The public key itself is written to the command's
//! stdin (never argv), but newlines are still rejected so the single-line
//! `read` in `dokku ssh-keys:add` receives the whole key.

/// Accepts the identifier styles dokku users pick (`laptop`, `deploy@ci`).
pub fn is_valid_ssh_key_name(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@' | '+'))
}

/// A single-line OpenSSH public key (`ssh-ed25519 AAAA… comment`).
pub fn is_valid_public_key(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() || value.len() > 16_384 {
        return false;
    }
    if value
        .chars()
        .any(|c| c == '\'' || c == '\n' || c == '\r' || c.is_control())
    {
        return false;
    }
    let Some((key_type, _)) = value.split_once(' ') else {
        return false;
    };
    key_type.starts_with("ssh-") || key_type.starts_with("ecdsa-") || key_type.starts_with("sk-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_key_names() {
        for value in ["laptop", "deploy@ci", "jack.example.com", "a_b-c+d"] {
            assert!(is_valid_ssh_key_name(value), "{value}");
        }
    }

    #[test]
    fn rejects_unsafe_key_names() {
        for value in ["", "my laptop", "it's", "a;rm", &"a".repeat(129)] {
            assert!(!is_valid_ssh_key_name(value), "{value:?}");
        }
    }

    #[test]
    fn accepts_public_keys() {
        assert!(is_valid_public_key(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIabc jack@laptop"
        ));
        assert!(is_valid_public_key(
            "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTY= key"
        ));
    }

    #[test]
    fn rejects_malformed_public_keys() {
        for value in [
            "",
            "not-a-key",
            "ssh-ed25519",
            "ssh-ed25519 AAAAC3\nsecond line",
            "ssh-ed25519 AAAAC3'; rm",
        ] {
            assert!(!is_valid_public_key(value), "{value:?}");
        }
    }
}
