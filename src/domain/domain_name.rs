use std::fmt;
use std::str::FromStr;

/// A vhost hostname accepted by `domains:add/remove/set`. Optional leading
/// `*.` wildcard, otherwise dot-separated labels of ASCII alphanumerics and
/// hyphens (no leading/trailing hyphen, no empty labels).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DomainName(String);

impl DomainName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<&str> for DomainName {
    type Error = DomainNameError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let raw = value.trim();
        if raw.is_empty() {
            return Err(DomainNameError::Empty);
        }
        if raw.len() > 253 {
            return Err(DomainNameError::TooLong);
        }
        let host = raw.strip_prefix("*.").unwrap_or(raw);
        if host.is_empty() {
            return Err(DomainNameError::Empty);
        }
        for label in host.split('.') {
            if label.is_empty() {
                return Err(DomainNameError::EmptyLabel(raw.to_owned()));
            }
            if label.len() > 63 {
                return Err(DomainNameError::LabelTooLong(label.to_owned()));
            }
            if label.starts_with('-') || label.ends_with('-') {
                return Err(DomainNameError::InvalidLabel(label.to_owned()));
            }
            if !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                return Err(DomainNameError::InvalidLabel(label.to_owned()));
            }
        }
        Ok(Self(raw.to_owned()))
    }
}

impl TryFrom<String> for DomainName {
    type Error = DomainNameError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl FromStr for DomainName {
    type Err = DomainNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s)
    }
}

impl fmt::Display for DomainName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DomainNameError {
    #[error("domain must not be empty")]
    Empty,
    #[error("domain is too long (max 253 characters)")]
    TooLong,
    #[error("domain `{0}` contains an empty label")]
    EmptyLabel(String),
    #[error("domain label `{0}` is too long (max 63 characters)")]
    LabelTooLong(String),
    #[error("domain label `{0}` must contain only letters, digits, and hyphens")]
    InvalidLabel(String),
}

/// Splits a form field into domains (whitespace and commas both separate) and
/// validates each. Empty input yields an empty list; the caller decides
/// whether that is an error.
pub fn parse_domain_list(input: &str) -> Result<Vec<DomainName>, DomainNameError> {
    input
        .split(|c: char| c.is_whitespace() || c == ',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(DomainName::try_from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_hostnames_and_wildcards() {
        for raw in [
            "example.com",
            "sub.example.com",
            "my-app.example.co.uk",
            "a.b.c.d",
            "*.example.com",
            "localhost",
            "1st.example",
        ] {
            assert_eq!(DomainName::try_from(raw).expect("valid").as_str(), raw);
        }
    }

    #[test]
    fn rejects_malformed_hostnames() {
        for raw in [
            "",
            " ",
            ".example.com",
            "example..com",
            "example.com.",
            "-example.com",
            "example-.com",
            "exa_mple.com",
            "example com",
            "http://example.com",
            "*. ",
        ] {
            assert!(DomainName::try_from(raw).is_err(), "{raw:?}");
        }
        assert!(DomainName::try_from("a".repeat(254)).is_err());
        assert!(DomainName::try_from(format!("{}.com", "a".repeat(64))).is_err());
    }

    #[test]
    fn parse_domain_list_splits_and_validates() {
        assert_eq!(
            parse_domain_list("one.example.com two.example.com,three.example.com").expect("list"),
            vec![
                DomainName::try_from("one.example.com").expect("d"),
                DomainName::try_from("two.example.com").expect("d"),
                DomainName::try_from("three.example.com").expect("d"),
            ]
        );
        assert!(parse_domain_list("").expect("empty").is_empty());
        assert!(parse_domain_list(" , ").expect("empty").is_empty());
        assert!(parse_domain_list("ok.example.com bad_domain").is_err());
    }
}
