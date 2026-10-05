/// Basic-auth usernames for the `http-auth` plugin: letters, digits, `.`, `_`,
/// `-` (no spaces or shell metacharacters, since they travel in argv).
pub fn is_valid_username(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_simple_usernames() {
        for value in ["alice", "a.b_c-d", "user123", "A"] {
            assert!(is_valid_username(value), "{value}");
        }
    }

    #[test]
    fn rejects_unsafe_usernames() {
        for value in ["", "a b", "it's", "a;rm", "a/b", "a:b", &"a".repeat(65)] {
            assert!(!is_valid_username(value), "{value:?}");
        }
    }
}
