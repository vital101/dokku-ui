use std::fmt;
use std::str::FromStr;

/// Maximum length of a dokku storage entry name (`MaxEntryNameLength` in
/// dokku 0.38.4 `plugins/storage/entry.go`).
pub const MAX_STORAGE_ENTRY_NAME_LEN: usize = 45;

const MAX_VOLUME_CONTAINER_PATH_LEN: usize = 1024;
const RESERVED_LEGACY_PREFIX: &str = "legacy-";

/// A dokku storage entry name, as accepted by `storage:create`,
/// `storage:destroy`, and the named `storage:mount` form. Mirrors dokku
/// 0.38.4's `ValidateEntryName`: a DNS-1123 label of at most 45 characters
/// (`^[a-z0-9]([-a-z0-9]*[a-z0-9])?$`). The reserved `legacy-` prefix is
/// rejected for new entries by [`StorageEntryName::try_from_create`] but
/// allowed here, since existing `legacy-*` entries are valid attach targets.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StorageEntryName(String);

impl StorageEntryName {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Validate a name for `storage:create`; rejects the reserved `legacy-` prefix.
    pub fn try_from_create(value: &str) -> Result<Self, StorageEntryNameError> {
        let name = Self::try_from(value)?;
        if name.is_reserved_legacy_name() {
            return Err(StorageEntryNameError::ReservedPrefix);
        }
        Ok(name)
    }

    /// True when the name is an existing reserved/legacy entry.
    pub fn is_reserved_legacy_name(&self) -> bool {
        self.0.starts_with(RESERVED_LEGACY_PREFIX)
    }
}

impl TryFrom<&str> for StorageEntryName {
    type Error = StorageEntryNameError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if value.is_empty() {
            return Err(StorageEntryNameError::Empty);
        }
        if value.chars().count() > MAX_STORAGE_ENTRY_NAME_LEN {
            return Err(StorageEntryNameError::TooLong);
        }
        let chars: Vec<char> = value.chars().collect();
        let last_index = chars.len() - 1;
        for (index, c) in chars.iter().enumerate() {
            let label_edge = index == 0 || index == last_index;
            let allowed = if label_edge {
                c.is_ascii_lowercase() || c.is_ascii_digit()
            } else {
                *c == '-' || c.is_ascii_lowercase() || c.is_ascii_digit()
            };
            if !allowed {
                return Err(StorageEntryNameError::InvalidCharacter(*c));
            }
        }
        Ok(Self(value.to_owned()))
    }
}

impl TryFrom<String> for StorageEntryName {
    type Error = StorageEntryNameError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl FromStr for StorageEntryName {
    type Err = StorageEntryNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s)
    }
}

impl fmt::Display for StorageEntryName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StorageEntryNameError {
    #[error("storage entry name must not be empty")]
    Empty,
    #[error("storage entry name must be at most {MAX_STORAGE_ENTRY_NAME_LEN} characters")]
    TooLong,
    #[error(
        "storage entry name may only contain lowercase letters, digits, and interior hyphens, got `{0}`"
    )]
    InvalidCharacter(char),
    #[error("storage entry names may not start with `legacy-`, which is reserved by dokku")]
    ReservedPrefix,
}

/// Validate a container path for the named `storage:mount ... --container-dir`
/// form. Mirrors the `MountSpec` constraints that matter for that flag path:
/// absolute, quote/whitespace/colon-free (dokku's SSH wrapper re-splits argv
/// with `xargs -n 1`), and length-bounded.
pub fn is_valid_volume_container_path(path: &str) -> bool {
    !path.is_empty()
        && path.chars().count() <= MAX_VOLUME_CONTAINER_PATH_LEN
        && path.starts_with('/')
        && !path
            .chars()
            .any(|c| c.is_whitespace() || c == ':' || c == '\'' || c == '"' || c.is_control())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_entry_names() {
        for name in ["uploads", "a", "my-volume", "a1-b2", &"a".repeat(45)] {
            assert_eq!(
                StorageEntryName::try_from(name).expect("valid").as_str(),
                name
            );
        }
    }

    #[test]
    fn rejects_empty_name() {
        assert_eq!(
            StorageEntryName::try_from(""),
            Err(StorageEntryNameError::Empty)
        );
    }

    #[test]
    fn rejects_invalid_names() {
        for (raw, invalid) in [
            ("Uploads", 'U'),
            ("my_volume", '_'),
            ("my.volume", '.'),
            ("-leading", '-'),
            ("trailing-", '-'),
        ] {
            assert_eq!(
                StorageEntryName::try_from(raw),
                Err(StorageEntryNameError::InvalidCharacter(invalid)),
                "{raw}"
            );
        }
    }

    #[test]
    fn rejects_name_over_max_length() {
        assert_eq!(
            StorageEntryName::try_from("a".repeat(MAX_STORAGE_ENTRY_NAME_LEN + 1)),
            Err(StorageEntryNameError::TooLong)
        );
    }

    #[test]
    fn allows_legacy_prefix_as_attach_target() {
        let name = StorageEntryName::try_from("legacy-151e4f1a23").expect("valid");
        assert_eq!(name.as_str(), "legacy-151e4f1a23");
        assert!(name.is_reserved_legacy_name());
    }

    #[test]
    fn create_rejects_legacy_prefix() {
        assert_eq!(
            StorageEntryName::try_from_create("legacy-151e4f1a23"),
            Err(StorageEntryNameError::ReservedPrefix)
        );
        assert_eq!(
            StorageEntryName::try_from_create("legacy-x"),
            Err(StorageEntryNameError::ReservedPrefix)
        );
    }

    #[test]
    fn create_accepts_normal_names() {
        assert_eq!(
            StorageEntryName::try_from_create("uploads")
                .expect("valid")
                .as_str(),
            "uploads"
        );
    }

    #[test]
    fn non_legacy_name_is_not_reserved() {
        let name = StorageEntryName::try_from("uploads").expect("valid");
        assert!(!name.is_reserved_legacy_name());
    }

    #[test]
    fn from_str_and_display_roundtrip() {
        let name: StorageEntryName = "my-volume".parse().expect("parse");
        assert_eq!(name.to_string(), "my-volume");
    }

    #[test]
    fn accepts_valid_container_paths() {
        for path in ["/data", "/app/uploads", "/var/lib/app"] {
            assert!(is_valid_volume_container_path(path), "{path}");
        }
        let at_limit = format!("/{}", "a".repeat(MAX_VOLUME_CONTAINER_PATH_LEN - 1));
        assert!(is_valid_volume_container_path(&at_limit));
    }

    #[test]
    fn rejects_invalid_container_paths() {
        for path in [
            "",
            "relative",
            "/bad path",
            "/bad:path",
            "/it's",
            "/line\nbreak",
            "/tab\there",
            "/quote\"d",
            &"a".repeat(MAX_VOLUME_CONTAINER_PATH_LEN + 1),
        ] {
            assert!(!is_valid_volume_container_path(path), "{path:?}");
        }
        let over_limit = format!("/{}", "a".repeat(MAX_VOLUME_CONTAINER_PATH_LEN));
        assert!(!is_valid_volume_container_path(&over_limit));
    }
}
