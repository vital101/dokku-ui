/// Process types are app formation keys: lowercase letters, digits, `_`, `-`.
pub fn is_valid_process_type(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Resource values are single-line, quote-free, and numeric-ish (`1`, `1.5`,
/// `128`, `0`). Dokku validates the semantics; this only keeps argv safe.
pub fn is_valid_resource_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value.chars().any(|c| c.is_ascii_digit())
        && value.chars().all(|c| c.is_ascii_digit() || c == '.')
        && value.matches('.').count() <= 1
        && !value.starts_with('.')
        && !value.ends_with('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_types_follow_formation_keys() {
        for value in ["web", "worker", "web_1", "release", "my-type"] {
            assert!(is_valid_process_type(value), "{value}");
        }
        for value in ["", "web.1", "web 1", "web/1", "it's"] {
            assert!(!is_valid_process_type(value), "{value}");
        }
    }

    #[test]
    fn resource_values_are_numeric_and_safe() {
        for value in ["0", "1", "1.5", "128", "1024"] {
            assert!(is_valid_resource_value(value), "{value}");
        }
        for value in ["", ".", ".5", "5.", "1.2.3", "128m", "it's", "1 2", "-1"] {
            assert!(!is_valid_resource_value(value), "{value}");
        }
    }
}
