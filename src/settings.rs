use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub port: u16,
    pub database_url: String,
    pub dokku_host: String,
    pub dokku_ssh_port: u16,
    pub dokku_ssh_user: String,
    pub dokku_ssh_key_path: PathBuf,
    pub dokku_ssh_known_hosts_path: Option<PathBuf>,
    pub command_timeout_secs: u64,
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
        let dokku_host = vars
            .get("DOKKU_HOST")
            .cloned()
            .unwrap_or_else(|| "host.docker.internal".to_owned());
        let dokku_ssh_port = match vars.get("DOKKU_SSH_PORT") {
            Some(raw) => raw
                .parse::<u16>()
                .map_err(|_| SettingsError::InvalidSshPort(raw.clone()))?,
            None => 22,
        };
        let dokku_ssh_user = vars
            .get("DOKKU_SSH_USER")
            .cloned()
            .unwrap_or_else(|| "dokku".to_owned());
        let dokku_ssh_key_path = PathBuf::from(
            vars.get("DOKKU_SSH_KEY_PATH")
                .cloned()
                .unwrap_or_else(|| "dev-data/ssh/id_ed25519".to_owned()),
        );
        let dokku_ssh_known_hosts_path = vars.get("DOKKU_SSH_HOST_KEYS_PATH").map(PathBuf::from);
        let command_timeout_secs = match vars.get("COMMAND_TIMEOUT_SECS") {
            Some(raw) => raw
                .parse::<u64>()
                .map_err(|_| SettingsError::InvalidTimeout(raw.clone()))?,
            None => 30,
        };
        Ok(Self {
            port,
            database_url,
            dokku_host,
            dokku_ssh_port,
            dokku_ssh_user,
            dokku_ssh_key_path,
            dokku_ssh_known_hosts_path,
            command_timeout_secs,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SettingsError {
    #[error("PORT must be a valid u16, got `{0}`")]
    InvalidPort(String),
    #[error("DOKKU_SSH_PORT must be a valid u16, got `{0}`")]
    InvalidSshPort(String),
    #[error("COMMAND_TIMEOUT_SECS must be a valid u64, got `{0}`")]
    InvalidTimeout(String),
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
        assert_eq!(settings.dokku_host, "host.docker.internal");
        assert_eq!(settings.dokku_ssh_port, 22);
        assert_eq!(settings.dokku_ssh_user, "dokku");
        assert_eq!(
            settings.dokku_ssh_key_path,
            PathBuf::from("dev-data/ssh/id_ed25519")
        );
        assert_eq!(settings.dokku_ssh_known_hosts_path, None);
        assert_eq!(settings.command_timeout_secs, 30);
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
    fn reads_dokku_settings() {
        let settings = Settings::from_map(&map(&[
            ("DOKKU_HOST", "dokku.example.com"),
            ("DOKKU_SSH_PORT", "2222"),
            ("DOKKU_SSH_USER", "dokku-admin"),
            ("DOKKU_SSH_KEY_PATH", "/run/secrets/dokku_key"),
            ("DOKKU_SSH_HOST_KEYS_PATH", "/app/data/ssh/known_hosts"),
            ("COMMAND_TIMEOUT_SECS", "60"),
        ]))
        .expect("settings");
        assert_eq!(settings.dokku_host, "dokku.example.com");
        assert_eq!(settings.dokku_ssh_port, 2222);
        assert_eq!(settings.dokku_ssh_user, "dokku-admin");
        assert_eq!(
            settings.dokku_ssh_key_path,
            PathBuf::from("/run/secrets/dokku_key")
        );
        assert_eq!(
            settings.dokku_ssh_known_hosts_path,
            Some(PathBuf::from("/app/data/ssh/known_hosts"))
        );
        assert_eq!(settings.command_timeout_secs, 60);
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

    #[test]
    fn rejects_invalid_ssh_port() {
        let err = Settings::from_map(&map(&[("DOKKU_SSH_PORT", "22x")])).expect_err("invalid port");
        assert!(matches!(err, SettingsError::InvalidSshPort(_)));
    }

    #[test]
    fn rejects_invalid_timeout() {
        let err = Settings::from_map(&map(&[("COMMAND_TIMEOUT_SECS", "soon")]))
            .expect_err("invalid timeout");
        assert!(matches!(err, SettingsError::InvalidTimeout(_)));
    }
}
