use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Email(String);

impl Email {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<&str> for Email {
    type Error = EmailError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if !is_valid_email(value) {
            return Err(EmailError::Invalid(value.to_owned()));
        }
        Ok(Self(value.to_owned()))
    }
}

impl TryFrom<String> for Email {
    type Error = EmailError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl fmt::Display for Email {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn is_valid_email(value: &str) -> bool {
    if value.is_empty() || value.len() > 254 {
        return false;
    }
    if value.chars().any(char::is_whitespace) {
        return false;
    }
    let mut parts = value.split('@');
    let (Some(local), Some(domain)) = (parts.next(), parts.next()) else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    if local.is_empty() || local.len() > 64 {
        return false;
    }
    if domain.is_empty() || domain.len() > 253 {
        return false;
    }
    if !domain.contains('.') {
        return false;
    }
    domain
        .split('.')
        .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EmailError {
    #[error("invalid email address: `{0}`")]
    Invalid(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_emails() {
        for email in [
            "admin@example.com",
            "user+tag@example.co.uk",
            "a@b.co",
            "first.last@sub.domain.example",
        ] {
            assert_eq!(Email::try_from(email).expect("valid").as_str(), email);
        }
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(Email::try_from(""), Err(EmailError::Invalid(String::new())));
    }

    #[test]
    fn rejects_missing_at_sign() {
        assert!(Email::try_from("not-an-email").is_err());
    }

    #[test]
    fn rejects_multiple_at_signs() {
        assert!(Email::try_from("a@b@c.com").is_err());
    }

    #[test]
    fn rejects_empty_local_or_domain() {
        assert!(Email::try_from("@example.com").is_err());
        assert!(Email::try_from("user@").is_err());
    }

    #[test]
    fn rejects_domain_without_dot() {
        assert!(Email::try_from("user@example").is_err());
    }

    #[test]
    fn rejects_dot_boundaries_in_domain() {
        assert!(Email::try_from("user@example..com").is_err());
        assert!(Email::try_from("user@.example.com").is_err());
        assert!(Email::try_from("user@example.com.").is_err());
    }

    #[test]
    fn rejects_whitespace() {
        assert!(Email::try_from("user name@example.com").is_err());
        assert!(Email::try_from("user@exam ple.com").is_err());
    }

    #[test]
    fn rejects_oversized_inputs() {
        let local = "a".repeat(65);
        assert!(Email::try_from(format!("{local}@example.com").as_str()).is_err());
        let domain = format!("{}.com", "a".repeat(250));
        assert!(Email::try_from(format!("user@{domain}").as_str()).is_err());
    }
}
