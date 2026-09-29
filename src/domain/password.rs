use std::fmt;

pub const MIN_LENGTH: usize = 12;
pub const MAX_LENGTH: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Password(String);

impl Password {
    pub fn new(raw: &str) -> Result<Self, PasswordError> {
        let len = raw.chars().count();
        if len < MIN_LENGTH {
            return Err(PasswordError::TooShort(len));
        }
        if len > MAX_LENGTH {
            return Err(PasswordError::TooLong(len));
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("********")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PasswordError {
    #[error("password must be at least {MIN_LENGTH} characters, got {0}")]
    TooShort(usize),
    #[error("password must be at most {MAX_LENGTH} characters, got {0}")]
    TooLong(usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_minimum_length() {
        let password = "a".repeat(MIN_LENGTH);
        let parsed = Password::new(&password).expect("min length");
        assert_eq!(parsed.as_str(), password);
    }

    #[test]
    fn accepts_maximum_length() {
        let password = "a".repeat(MAX_LENGTH);
        assert!(Password::new(&password).is_ok());
    }

    #[test]
    fn rejects_short_passwords() {
        assert_eq!(
            Password::new(&"a".repeat(MIN_LENGTH - 1)),
            Err(PasswordError::TooShort(MIN_LENGTH - 1))
        );
    }

    #[test]
    fn rejects_long_passwords() {
        assert_eq!(
            Password::new(&"a".repeat(MAX_LENGTH + 1)),
            Err(PasswordError::TooLong(MAX_LENGTH + 1))
        );
    }

    #[test]
    fn display_never_leaks_the_password() {
        let password = Password::new(&"a".repeat(MIN_LENGTH)).expect("valid");
        assert_eq!(password.to_string(), "********");
        assert!(!password.to_string().contains(&password.as_str()[..4]));
    }
}
