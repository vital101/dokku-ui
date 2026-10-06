use ring::digest;

/// Reset links are valid for 24 hours; env-free because the link is shared
/// out of band by an admin, not emailed.
pub const RESET_TTL_SECS: i64 = 86_400;

const TOKEN_LEN: usize = 64;

/// Bearer tokens are stored hashed so a database read alone cannot mint a
/// session; SHA-256 is enough because the token has 256 bits of entropy.
pub fn hash_token(token: &str) -> String {
    digest::digest(&digest::SHA256, token.as_bytes())
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Shape check before any database lookup: 64 lowercase hex characters.
pub fn is_plausible_token(token: &str) -> bool {
    token.len() == TOKEN_LEN
        && token
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// Absolute reset URL from the instance's public URL.
pub fn reset_url(public_url: &str, token: &str) -> String {
    format!("{}/reset/{token}", public_url.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_deterministic_hex() {
        let token = crate::auth::csrf::generate_token();
        let hash = hash_token(&token);
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(hash, hash_token(&token));
        assert_ne!(hash, hash_token(&crate::auth::csrf::generate_token()));
    }

    #[test]
    fn plausible_token_shape() {
        assert!(is_plausible_token(&crate::auth::csrf::generate_token()));
        for bad in ["", "abc", &"A".repeat(64), &"g".repeat(64), &"a".repeat(63)] {
            assert!(!is_plausible_token(bad), "{bad}");
        }
    }

    #[test]
    fn reset_url_joins_without_double_slashes() {
        assert_eq!(
            reset_url("https://ui.example.com", "abc"),
            "https://ui.example.com/reset/abc"
        );
        assert_eq!(
            reset_url("https://ui.example.com/", "abc"),
            "https://ui.example.com/reset/abc"
        );
    }
}
