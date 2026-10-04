use std::fmt;
use std::str::FromStr;

/// A datastore service name, as accepted by the service plugins. Mirrors
/// dokku's `is_valid_service_name` check: `^[A-Za-z0-9_-]+$` (service names
/// are not app names — uppercase and underscores are legal).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ServiceName(String);

impl ServiceName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<&str> for ServiceName {
    type Error = ServiceNameError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if value.is_empty() {
            return Err(ServiceNameError::Empty);
        }
        if let Some(invalid) = value
            .chars()
            .find(|c| !c.is_ascii_alphanumeric() && *c != '_' && *c != '-')
        {
            return Err(ServiceNameError::InvalidCharacter(invalid));
        }
        Ok(Self(value.to_owned()))
    }
}

impl TryFrom<String> for ServiceName {
    type Error = ServiceNameError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl FromStr for ServiceName {
    type Err = ServiceNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s)
    }
}

impl fmt::Display for ServiceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServiceNameError {
    #[error("service name must not be empty")]
    Empty,
    #[error("service name may only contain letters, digits, underscores, and hyphens, got `{0}`")]
    InvalidCharacter(char),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_dokku_service_names() {
        for name in ["cache", "my-db", "my_db", "AppCache", "1st-db", "a"] {
            assert_eq!(ServiceName::try_from(name).expect("valid").as_str(), name);
        }
    }

    #[test]
    fn rejects_empty_name() {
        assert_eq!(ServiceName::try_from(""), Err(ServiceNameError::Empty));
    }

    #[test]
    fn rejects_invalid_characters() {
        for (raw, invalid) in [
            ("my db", ' '),
            ("my.db", '.'),
            ("my/db", '/'),
            ("my$db", '$'),
        ] {
            assert_eq!(
                ServiceName::try_from(raw),
                Err(ServiceNameError::InvalidCharacter(invalid)),
                "{raw}"
            );
        }
    }

    #[test]
    fn from_str_and_display_roundtrip() {
        let name: ServiceName = "my-db".parse().expect("parse");
        assert_eq!(name.to_string(), "my-db");
    }
}
