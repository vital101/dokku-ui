/// Builders selectable via `builder:set <app> selected <value>` on dokku
/// 0.38.4 (the installed builder-* plugins).
pub const BUILDERS: [&str; 7] = [
    "dockerfile",
    "herokuish",
    "lambda",
    "nixpacks",
    "null",
    "pack",
    "railpack",
];

/// Builder properties the UI can set (`detected` is read-only).
pub const BUILDER_PROPERTIES: [&str; 2] = ["selected", "build-dir"];

/// A buildpack URL/path: single-line, quote-free, no control characters.
pub fn is_valid_buildpack(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && !value.contains('\n')
        && !value.contains('\r')
        && !value.contains('\'')
        && !value.chars().any(char::is_control)
}

pub fn is_valid_buildpack_index(index: u32) -> bool {
    (1..=1000).contains(&index)
}

pub fn is_valid_builder_property(property: &str) -> bool {
    BUILDER_PROPERTIES.contains(&property)
}

/// `selected` must be a known builder (or empty to clear); `build-dir` is a
/// relative or absolute path without quotes or control characters.
pub fn is_valid_builder_value(property: &str, value: &str) -> bool {
    if value.is_empty() {
        return true;
    }
    match property {
        "selected" => BUILDERS.contains(&value),
        "build-dir" => {
            value.len() <= 256
                && value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._/-".contains(c))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buildpacks_must_be_argv_safe() {
        for value in [
            "https://github.com/heroku/heroku-buildpack-nodejs",
            "https://example.com/bp#branch",
            "git@github.com:org/bp.git",
            ".",
        ] {
            assert!(is_valid_buildpack(value), "{value}");
        }
        for value in ["", "it's", "line\nbreak", "a".repeat(513).as_str()] {
            assert!(!is_valid_buildpack(value), "{value:?}");
        }
    }

    #[test]
    fn buildpack_index_is_bounded() {
        assert!(!is_valid_buildpack_index(0));
        assert!(is_valid_buildpack_index(1));
        assert!(is_valid_buildpack_index(1000));
        assert!(!is_valid_buildpack_index(1001));
    }

    #[test]
    fn builder_properties_are_whitelisted() {
        for property in ["selected", "build-dir"] {
            assert!(is_valid_builder_property(property));
        }
        for property in ["detected", "skip-cleanup", "global selected", ""] {
            assert!(!is_valid_builder_property(property), "{property}");
        }
    }

    #[test]
    fn builder_values_are_whitelisted_per_property() {
        assert!(is_valid_builder_value("selected", "dockerfile"));
        assert!(is_valid_builder_value("selected", ""));
        assert!(!is_valid_builder_value("selected", "podman"));
        assert!(is_valid_builder_value("build-dir", "/app/sub_dir"));
        assert!(is_valid_builder_value("build-dir", ""));
        assert!(!is_valid_builder_value("build-dir", "../it's"));
        assert!(!is_valid_builder_value("unknown", "x"));
    }
}
