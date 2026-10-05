/// Process types are app formation keys: lowercase letters, digits, `_`, `-`.
pub fn is_valid_process_type(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Resource values are single-line and argv-safe: a number with an optional
/// docker-style unit suffix (`512m`, `1g`, `KB/MB/GB`) for memory, a plain
/// number/decimal for cpu, or `-1` (dokku's "swap off"). Dokku validates the
/// semantics; this only keeps argv safe and rejects obvious garbage early.
pub fn is_valid_resource_value(value: &str) -> bool {
    if value.is_empty() || value.len() > 32 {
        return false;
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    {
        return false;
    }
    let body = value.strip_prefix('-').unwrap_or(value);
    if body.is_empty() {
        return false;
    }
    let mut split = body.len();
    for (index, c) in body.char_indices() {
        if c.is_ascii_alphabetic() {
            split = index;
            break;
        }
    }
    let (numeric, suffix) = (&body[..split], &body[split..]);
    if numeric.is_empty() || !numeric.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return false;
    }
    if numeric.matches('.').count() > 1 || numeric.starts_with('.') || numeric.ends_with('.') {
        return false;
    }
    if !numeric.chars().any(|c| c.is_ascii_digit()) {
        return false;
    }
    if suffix.is_empty() {
        return true;
    }
    matches!(
        suffix.to_lowercase().as_str(),
        "b" | "k" | "kb" | "m" | "mb" | "g" | "gb" | "t" | "tb"
    )
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
        for value in [
            "0", "1", "1.5", "128", "1024", "512m", "1g", "1GB", "512MB", "-1",
        ] {
            assert!(is_valid_resource_value(value), "{value}");
        }
        for value in [
            "", ".", ".5", "5.", "1.2.3", "12x", "--1", "1-", "m", "1_000", "it's", "1 2",
        ] {
            assert!(!is_valid_resource_value(value), "{value}");
        }
    }
}
