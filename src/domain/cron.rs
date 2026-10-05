/// Cron task ids are hashes from `cron:list` (ASCII alphanumerics plus `-`/`_`).
pub fn is_valid_cron_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_hashed_ids() {
        for value in ["a1b2c3", "0123456789abcdef", "cron-id_1"] {
            assert!(is_valid_cron_id(value), "{value}");
        }
    }

    #[test]
    fn rejects_unsafe_ids() {
        for value in ["", "bad id", "it's", "id;rm", "id/../x", &"a".repeat(129)] {
            assert!(!is_valid_cron_id(value), "{value:?}");
        }
    }
}
