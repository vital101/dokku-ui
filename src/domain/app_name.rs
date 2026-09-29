use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AppName(String);

impl AppName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<&str> for AppName {
    type Error = AppNameError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let mut chars = value.chars();
        let Some(first) = chars.next() else {
            return Err(AppNameError::Empty);
        };
        if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
            return Err(AppNameError::InvalidStart(first));
        }
        if let Some(invalid) =
            chars.find(|c| !c.is_ascii_lowercase() && !c.is_ascii_digit() && *c != '.' && *c != '-')
        {
            return Err(AppNameError::InvalidCharacter(invalid));
        }
        Ok(Self(value.to_owned()))
    }
}

impl TryFrom<String> for AppName {
    type Error = AppNameError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl FromStr for AppName {
    type Err = AppNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s)
    }
}

impl fmt::Display for AppName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AppNameError {
    #[error("app name must not be empty")]
    Empty,
    #[error("app name must begin with a lowercase alphanumeric character, got `{0}`")]
    InvalidStart(char),
    #[error("app name may only contain lowercase alphanumerics, dots, and hyphens, got `{0}`")]
    InvalidCharacter(char),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_names() {
        for name in [
            "myapp",
            "my-app",
            "myapp1",
            "a",
            "api.internal",
            "1app",
            "a.b-c.d",
        ] {
            assert_eq!(AppName::try_from(name).expect("valid").as_str(), name);
        }
    }

    #[test]
    fn rejects_empty_name() {
        assert_eq!(AppName::try_from(""), Err(AppNameError::Empty));
    }

    #[test]
    fn rejects_uppercase_start() {
        assert_eq!(
            AppName::try_from("MyApp"),
            Err(AppNameError::InvalidStart('M'))
        );
    }

    #[test]
    fn rejects_invalid_start_character() {
        assert_eq!(
            AppName::try_from("-myapp"),
            Err(AppNameError::InvalidStart('-'))
        );
    }

    #[test]
    fn rejects_invalid_characters() {
        assert_eq!(
            AppName::try_from("my_app"),
            Err(AppNameError::InvalidCharacter('_'))
        );
        assert_eq!(
            AppName::try_from("my app"),
            Err(AppNameError::InvalidCharacter(' '))
        );
        assert_eq!(
            AppName::try_from("my$app"),
            Err(AppNameError::InvalidCharacter('$'))
        );
    }

    #[test]
    fn rejects_uppercase_in_body() {
        assert_eq!(
            AppName::try_from("myApp"),
            Err(AppNameError::InvalidCharacter('A'))
        );
    }

    #[test]
    fn rejects_longest_possible_string() {
        let name = "a".repeat(10_000);
        assert!(AppName::try_from(name.as_str()).is_ok());
    }

    #[test]
    fn from_str_parses() {
        assert_eq!("myapp".parse::<AppName>().expect("parse").as_str(), "myapp");
    }

    #[test]
    fn display_roundtrips() {
        let name = AppName::try_from("my-app").expect("valid");
        assert_eq!(name.to_string(), "my-app");
    }
}
