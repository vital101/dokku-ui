use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub port: u16,
    pub database_url: String,
}

impl Settings {
    pub fn from_env() -> Result<Self, SettingsError> {
        Self::from_map(&std::env::vars().collect())
    }

    pub fn from_map(vars: &HashMap<String, String>) -> Result<Self, SettingsError> {
        let port = match vars.get("PORT") {
            Some(raw) => raw
                .parse::<u16>()
                .map_err(|_| SettingsError::InvalidPort(raw.clone()))?,
            None => 8080,
        };
        let database_url = vars
            .get("DATABASE_URL")
            .cloned()
            .unwrap_or_else(|| "sqlite://dev-data/dokku-ui.db?mode=rwc".to_owned());
        Ok(Self { port, database_url })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SettingsError {
    #[error("PORT must be a valid u16, got `{0}`")]
    InvalidPort(String),
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn defaults_when_env_is_empty() {
        let settings = Settings::from_map(&HashMap::new()).expect("defaults");
        assert_eq!(settings.port, 8080);
        assert_eq!(
            settings.database_url,
            "sqlite://dev-data/dokku-ui.db?mode=rwc"
        );
    }

    #[test]
    fn reads_port_and_database_url() {
        let settings = Settings::from_map(&map(&[
            ("PORT", "9000"),
            ("DATABASE_URL", "sqlite:///tmp/other.db"),
        ]))
        .expect("settings");
        assert_eq!(settings.port, 9000);
        assert_eq!(settings.database_url, "sqlite:///tmp/other.db");
    }

    #[test]
    fn rejects_non_numeric_port() {
        let err = Settings::from_map(&map(&[("PORT", "not-a-port")])).expect_err("invalid port");
        assert!(matches!(err, SettingsError::InvalidPort(_)));
    }

    #[test]
    fn rejects_out_of_range_port() {
        let err = Settings::from_map(&map(&[("PORT", "70000")])).expect_err("invalid port");
        assert!(matches!(err, SettingsError::InvalidPort(_)));
    }
}
