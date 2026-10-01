use rand::RngCore;
use subtle::ConstantTimeEq;

pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn random_session_key() -> String {
    generate_token()
}

pub fn tokens_match(expected: &str, provided: &str) -> bool {
    bool::from(expected.as_bytes().ct_eq(provided.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_64_hex_characters() {
        let token = generate_token();
        assert_eq!(token.len(), 64);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn tokens_are_unique() {
        let tokens: Vec<String> = (0..100).map(|_| generate_token()).collect();
        let unique: std::collections::HashSet<_> = tokens.iter().collect();
        assert_eq!(unique.len(), tokens.len());
    }

    #[test]
    fn matching_tokens_are_equal() {
        let token = generate_token();
        assert!(tokens_match(&token, &token));
    }

    #[test]
    fn differing_tokens_are_not_equal() {
        assert!(!tokens_match("abc", "abd"));
        assert!(!tokens_match("abc", ""));
        assert!(!tokens_match("", "abc"));
        assert!(tokens_match("", ""));
    }
}
