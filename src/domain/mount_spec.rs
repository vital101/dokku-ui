use std::fmt;
use std::str::FromStr;

/// A dokku storage mount: `host-path-or-volume:container-path[:options]`.
///
/// Used both as the argument to `storage:mount` / `storage:unmount` and as the
/// parsed form of the `-v` entries in `storage:report`. Host paths and docker
/// volume names never contain `:` or whitespace, so the split is unambiguous.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MountSpec {
    host: String,
    container: String,
    options: Option<String>,
}

impl MountSpec {
    pub fn new(
        host: impl Into<String>,
        container: impl Into<String>,
        options: Option<String>,
    ) -> Self {
        Self {
            host: host.into(),
            container: container.into(),
            options,
        }
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn container(&self) -> &str {
        &self.container
    }

    pub fn options(&self) -> Option<&str> {
        self.options.as_deref()
    }

    /// The full `host:container[:options]` argument `storage:mount` accepts.
    pub fn arg(&self) -> String {
        match &self.options {
            Some(options) => format!("{}:{}:{}", self.host, self.container, options),
            None => format!("{}:{}", self.host, self.container),
        }
    }

    /// The `host:container` form `storage:unmount` accepts (options ignored).
    pub fn locator(&self) -> String {
        format!("{}:{}", self.host, self.container)
    }
}

impl TryFrom<&str> for MountSpec {
    type Error = MountSpecError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if value.chars().any(char::is_whitespace) {
            return Err(MountSpecError::Whitespace(value.to_owned()));
        }
        let mut parts = value.splitn(3, ':');
        let (Some(host), Some(container)) = (parts.next(), parts.next()) else {
            return Err(MountSpecError::MissingSeparator(value.to_owned()));
        };
        if host.is_empty() {
            return Err(MountSpecError::EmptyHost);
        }
        if container.is_empty() {
            return Err(MountSpecError::EmptyContainer);
        }
        if !container.starts_with('/') {
            return Err(MountSpecError::RelativeContainer(container.to_owned()));
        }
        let options = match parts.next() {
            None | Some("") => None,
            Some(options) if options.contains(':') => {
                return Err(MountSpecError::InvalidOptions(options.to_owned()));
            }
            Some(options) => Some(options.to_owned()),
        };
        Ok(Self {
            host: host.to_owned(),
            container: container.to_owned(),
            options,
        })
    }
}

impl TryFrom<String> for MountSpec {
    type Error = MountSpecError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl FromStr for MountSpec {
    type Err = MountSpecError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s)
    }
}

impl fmt::Display for MountSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.arg())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MountSpecError {
    #[error("mount must be in `host-path:container-path[:options]` form, got `{0}`")]
    MissingSeparator(String),
    #[error("mount host path or volume name must not be empty")]
    EmptyHost,
    #[error("mount container path must not be empty")]
    EmptyContainer,
    #[error("container path must be absolute (start with `/`), got `{0}`")]
    RelativeContainer(String),
    #[error("mount paths and options must not contain whitespace: `{0}`")]
    Whitespace(String),
    #[error("invalid mount options `{0}`")]
    InvalidOptions(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_host_path_without_options() {
        let mount =
            MountSpec::try_from("/var/lib/dokku/data/storage/app:/app/storage").expect("ok");
        assert_eq!(mount.host(), "/var/lib/dokku/data/storage/app");
        assert_eq!(mount.container(), "/app/storage");
        assert_eq!(mount.options(), None);
        assert_eq!(mount.arg(), "/var/lib/dokku/data/storage/app:/app/storage");
        assert_eq!(
            mount.locator(),
            "/var/lib/dokku/data/storage/app:/app/storage"
        );
    }

    #[test]
    fn parses_docker_volume_and_options() {
        let mount = MountSpec::try_from("my-volume:/data:ro,Z").expect("ok");
        assert_eq!(mount.host(), "my-volume");
        assert_eq!(mount.container(), "/data");
        assert_eq!(mount.options(), Some("ro,Z"));
        assert_eq!(mount.arg(), "my-volume:/data:ro,Z");
        assert_eq!(mount.locator(), "my-volume:/data");
    }

    #[test]
    fn trailing_colon_means_no_options() {
        let mount = MountSpec::try_from("/host:/container:").expect("ok");
        assert_eq!(mount.options(), None);
        assert_eq!(mount.arg(), "/host:/container");
    }

    #[test]
    fn rejects_missing_separator() {
        assert_eq!(
            MountSpec::try_from("/host"),
            Err(MountSpecError::MissingSeparator("/host".into()))
        );
    }

    #[test]
    fn rejects_empty_halves() {
        assert_eq!(
            MountSpec::try_from(":/container"),
            Err(MountSpecError::EmptyHost)
        );
        assert_eq!(
            MountSpec::try_from("/host:"),
            Err(MountSpecError::EmptyContainer)
        );
    }

    #[test]
    fn rejects_relative_container_path() {
        assert_eq!(
            MountSpec::try_from("/host:relative"),
            Err(MountSpecError::RelativeContainer("relative".into()))
        );
    }

    #[test]
    fn rejects_whitespace_and_colon_options() {
        assert_eq!(
            MountSpec::try_from("/host:/container:ro, z"),
            Err(MountSpecError::Whitespace("/host:/container:ro, z".into()))
        );
        assert_eq!(
            MountSpec::try_from("/host:/container:ro:z"),
            Err(MountSpecError::InvalidOptions("ro:z".into()))
        );
    }

    #[test]
    fn from_str_and_display_roundtrip() {
        let mount: MountSpec = "/host:/container:ro".parse().expect("parse");
        assert_eq!(mount.to_string(), "/host:/container:ro");
    }
}
