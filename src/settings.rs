use std::collections::HashMap;
use std::path::PathBuf;

use crate::auth::proxy::{Cidr, parse_trusted_cidrs};
use crate::auth::rbac::Role;

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
    pub secret_key: String,
    pub session_ttl_secs: u64,
    pub cookie_secure: bool,
    pub snapshot_refresh_secs: u64,
    pub activity_ttl_secs: u64,
    pub run_log_ttl_secs: u64,
    pub reauth_ttl_secs: u64,
    /// How often the metrics poller samples cheap service stats; `0` disables
    /// metrics collection entirely.
    pub metrics_refresh_secs: u64,
    /// How often the metrics poller samples expensive data-dir sizes; `0`
    /// disables data-size samples while still collecting cgroup/df stats.
    pub metrics_du_every_secs: u64,
    /// How long metric rows are retained; `0` disables pruning.
    pub metrics_ttl_secs: u64,
    /// Reverse-proxy header auth is enabled only when this list is non-empty;
    /// the header is honored only when the direct peer falls inside it.
    pub trusted_proxy_cidrs: Vec<Cidr>,
    /// Header carrying the authenticated user's email (lowercased).
    pub proxy_auth_header: String,
    /// Role granted to auto-registered proxy-auth users.
    pub proxy_auth_default_role: Role,
    /// How long a cached Docker Hub tag list stays fresh; `0` disables remote
    /// image-tag fetching.
    pub image_tags_ttl_secs: u64,
    /// Maximum lifetime of a viewer-driven live-log tail; `0` disables the
    /// cap (the SSE heartbeat still runs).
    pub log_stream_max_secs: u64,
}

impl Settings {
    /// Whether reverse-proxy header authentication is configured.
    pub fn proxy_auth_enabled(&self) -> bool {
        !self.trusted_proxy_cidrs.is_empty() && !self.proxy_auth_header.is_empty()
    }
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
        let secret_key = vars
            .get("SECRET_KEY")
            .cloned()
            .unwrap_or_else(|| "dev-secret-key-change-me-0123456789abcdef".to_owned());
        let session_ttl_secs = match vars.get("SESSION_TTL_SECS") {
            Some(raw) => raw
                .parse::<u64>()
                .map_err(|_| SettingsError::InvalidSessionTtl(raw.clone()))?,
            None => 604_800,
        };
        let cookie_secure = match vars.get("COOKIE_SECURE") {
            Some(raw) => raw
                .parse::<bool>()
                .map_err(|_| SettingsError::InvalidCookieSecure(raw.clone()))?,
            None => false,
        };
        let snapshot_refresh_secs = match vars.get("SNAPSHOT_REFRESH_SECS") {
            Some(raw) => raw
                .parse::<u64>()
                .map_err(|_| SettingsError::InvalidSnapshotRefresh(raw.clone()))?,
            None => 1800,
        };
        if snapshot_refresh_secs == 0 {
            return Err(SettingsError::InvalidSnapshotRefresh("0".to_owned()));
        }
        let activity_ttl_secs = match vars.get("ACTIVITY_TTL_SECS") {
            Some(raw) => raw
                .parse::<u64>()
                .map_err(|_| SettingsError::InvalidActivityTtl(raw.clone()))?,
            None => 7_776_000,
        };
        let reauth_ttl_secs = match vars.get("REAUTH_TTL_SECS") {
            Some(raw) => raw
                .parse::<u64>()
                .map_err(|_| SettingsError::InvalidReauthTtl(raw.clone()))?,
            None => 300,
        };
        let run_log_ttl_secs = match vars.get("RUN_LOG_TTL_SECS") {
            Some(raw) => raw
                .parse::<u64>()
                .map_err(|_| SettingsError::InvalidRunLogTtl(raw.clone()))?,
            None => 604_800,
        };
        let trusted_proxy_cidrs = match vars.get("TRUSTED_PROXY_CIDRS") {
            Some(raw) => parse_trusted_cidrs(raw)
                .map_err(|_| SettingsError::InvalidTrustedProxyCidrs(raw.clone()))?,
            None => Vec::new(),
        };
        let proxy_auth_header = vars
            .get("PROXY_AUTH_HEADER")
            .map(|raw| raw.trim().to_ascii_lowercase())
            .filter(|raw| !raw.is_empty())
            .unwrap_or_else(|| "x-forwarded-user".to_owned());
        let proxy_auth_default_role = match vars.get("PROXY_AUTH_DEFAULT_ROLE") {
            Some(raw) => Role::try_from(raw.as_str())
                .ok_or_else(|| SettingsError::InvalidProxyAuthRole(raw.clone()))?,
            None => Role::Viewer,
        };
        let image_tags_ttl_secs = match vars.get("IMAGE_TAGS_TTL_SECS") {
            Some(raw) => raw
                .parse::<u64>()
                .map_err(|_| SettingsError::InvalidImageTagsTtl(raw.clone()))?,
            None => 21_600,
        };
        let metrics_refresh_secs = match vars.get("METRICS_REFRESH_SECS") {
            Some(raw) => raw
                .parse::<u64>()
                .map_err(|_| SettingsError::InvalidMetricsRefresh(raw.clone()))?,
            None => 60,
        };
        let metrics_du_every_secs = match vars.get("METRICS_DU_EVERY_SECS") {
            Some(raw) => raw
                .parse::<u64>()
                .map_err(|_| SettingsError::InvalidMetricsDuTtl(raw.clone()))?,
            None => 900,
        };
        let metrics_ttl_secs = match vars.get("METRICS_TTL_SECS") {
            Some(raw) => raw
                .parse::<u64>()
                .map_err(|_| SettingsError::InvalidMetricsTtl(raw.clone()))?,
            None => 7_776_000,
        };
        let log_stream_max_secs = match vars.get("LOG_STREAM_MAX_SECS") {
            Some(raw) => raw
                .parse::<u64>()
                .map_err(|_| SettingsError::InvalidLogStreamMax(raw.clone()))?,
            None => 3_600,
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
            secret_key,
            session_ttl_secs,
            cookie_secure,
            snapshot_refresh_secs,
            activity_ttl_secs,
            run_log_ttl_secs,
            reauth_ttl_secs,
            metrics_refresh_secs,
            metrics_du_every_secs,
            metrics_ttl_secs,
            trusted_proxy_cidrs,
            proxy_auth_header,
            proxy_auth_default_role,
            image_tags_ttl_secs,
            log_stream_max_secs,
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
    #[error("SESSION_TTL_SECS must be a valid u64, got `{0}`")]
    InvalidSessionTtl(String),
    #[error("COOKIE_SECURE must be a valid bool, got `{0}`")]
    InvalidCookieSecure(String),
    #[error("SNAPSHOT_REFRESH_SECS must be a positive integer of seconds, got `{0}`")]
    InvalidSnapshotRefresh(String),
    #[error("ACTIVITY_TTL_SECS must be a valid u64, got `{0}`")]
    InvalidActivityTtl(String),
    #[error("RUN_LOG_TTL_SECS must be a valid u64, got `{0}`")]
    InvalidRunLogTtl(String),
    #[error("REAUTH_TTL_SECS must be a valid u64, got `{0}`")]
    InvalidReauthTtl(String),
    #[error("TRUSTED_PROXY_CIDRS must be a comma-separated list of CIDRs, got `{0}`")]
    InvalidTrustedProxyCidrs(String),
    #[error("PROXY_AUTH_DEFAULT_ROLE must be admin, operator, or viewer, got `{0}`")]
    InvalidProxyAuthRole(String),
    #[error("IMAGE_TAGS_TTL_SECS must be a valid u64, got `{0}`")]
    InvalidImageTagsTtl(String),
    #[error("METRICS_REFRESH_SECS must be a valid u64, got `{0}`")]
    InvalidMetricsRefresh(String),
    #[error("METRICS_DU_EVERY_SECS must be a valid u64, got `{0}`")]
    InvalidMetricsDuTtl(String),
    #[error("METRICS_TTL_SECS must be a valid u64, got `{0}`")]
    InvalidMetricsTtl(String),
    #[error("LOG_STREAM_MAX_SECS must be a valid u64, got `{0}`")]
    InvalidLogStreamMax(String),
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
        assert_eq!(
            settings.secret_key,
            "dev-secret-key-change-me-0123456789abcdef"
        );
        assert_eq!(settings.session_ttl_secs, 604_800);
        assert!(!settings.cookie_secure);
        assert_eq!(settings.snapshot_refresh_secs, 1800);
        assert_eq!(settings.activity_ttl_secs, 7_776_000);
        assert_eq!(settings.run_log_ttl_secs, 604_800);
        assert_eq!(settings.reauth_ttl_secs, 300);
        assert_eq!(settings.metrics_refresh_secs, 60);
        assert_eq!(settings.metrics_du_every_secs, 900);
        assert_eq!(settings.metrics_ttl_secs, 7_776_000);
        assert!(settings.trusted_proxy_cidrs.is_empty());
        assert_eq!(settings.proxy_auth_header, "x-forwarded-user");
        assert_eq!(settings.proxy_auth_default_role, Role::Viewer);
        assert!(!settings.proxy_auth_enabled());
        assert_eq!(settings.image_tags_ttl_secs, 21_600);
        assert_eq!(settings.log_stream_max_secs, 3_600);
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
            ("SECRET_KEY", "a-very-long-prod-secret-key-0123456789"),
            ("SESSION_TTL_SECS", "86400"),
            ("COOKIE_SECURE", "true"),
            ("SNAPSHOT_REFRESH_SECS", "45"),
            ("ACTIVITY_TTL_SECS", "86400"),
            ("RUN_LOG_TTL_SECS", "3600"),
            ("REAUTH_TTL_SECS", "60"),
            ("TRUSTED_PROXY_CIDRS", "10.0.0.0/8, 192.168.1.5"),
            ("PROXY_AUTH_HEADER", "X-Auth-Request-Email"),
            ("PROXY_AUTH_DEFAULT_ROLE", "operator"),
            ("IMAGE_TAGS_TTL_SECS", "900"),
            ("METRICS_REFRESH_SECS", "30"),
            ("METRICS_DU_EVERY_SECS", "1800"),
            ("METRICS_TTL_SECS", "86400"),
            ("LOG_STREAM_MAX_SECS", "120"),
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
        assert_eq!(
            settings.secret_key,
            "a-very-long-prod-secret-key-0123456789"
        );
        assert_eq!(settings.session_ttl_secs, 86_400);
        assert!(settings.cookie_secure);
        assert_eq!(settings.snapshot_refresh_secs, 45);
        assert_eq!(settings.activity_ttl_secs, 86_400);
        assert_eq!(settings.run_log_ttl_secs, 3_600);
        assert_eq!(settings.reauth_ttl_secs, 60);
        assert_eq!(settings.trusted_proxy_cidrs.len(), 2);
        assert_eq!(settings.proxy_auth_header, "x-auth-request-email");
        assert_eq!(settings.proxy_auth_default_role, Role::Operator);
        assert!(settings.proxy_auth_enabled());
        assert_eq!(settings.image_tags_ttl_secs, 900);
        assert_eq!(settings.metrics_refresh_secs, 30);
        assert_eq!(settings.metrics_du_every_secs, 1_800);
        assert_eq!(settings.metrics_ttl_secs, 86_400);
        assert_eq!(settings.log_stream_max_secs, 120);
    }

    #[test]
    fn rejects_malformed_proxy_auth_settings() {
        let err = Settings::from_map(&map(&[("TRUSTED_PROXY_CIDRS", "10.0.0.0/8,bogus")]))
            .expect_err("invalid cidr");
        assert!(matches!(err, SettingsError::InvalidTrustedProxyCidrs(_)));

        let err = Settings::from_map(&map(&[("PROXY_AUTH_DEFAULT_ROLE", "root")]))
            .expect_err("invalid role");
        assert!(matches!(err, SettingsError::InvalidProxyAuthRole(_)));

        let err =
            Settings::from_map(&map(&[("IMAGE_TAGS_TTL_SECS", "soon")])).expect_err("invalid ttl");
        assert!(matches!(err, SettingsError::InvalidImageTagsTtl(_)));
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

    #[test]
    fn rejects_invalid_session_ttl() {
        let err =
            Settings::from_map(&map(&[("SESSION_TTL_SECS", "never")])).expect_err("invalid ttl");
        assert!(matches!(err, SettingsError::InvalidSessionTtl(_)));
    }

    #[test]
    fn rejects_invalid_cookie_secure() {
        let err =
            Settings::from_map(&map(&[("COOKIE_SECURE", "maybe")])).expect_err("invalid bool");
        assert!(matches!(err, SettingsError::InvalidCookieSecure(_)));
    }

    #[test]
    fn rejects_invalid_snapshot_refresh() {
        let err = Settings::from_map(&map(&[("SNAPSHOT_REFRESH_SECS", "soon")]))
            .expect_err("invalid refresh");
        assert!(matches!(err, SettingsError::InvalidSnapshotRefresh(_)));
    }

    #[test]
    fn rejects_zero_snapshot_refresh() {
        let err =
            Settings::from_map(&map(&[("SNAPSHOT_REFRESH_SECS", "0")])).expect_err("zero refresh");
        assert!(matches!(err, SettingsError::InvalidSnapshotRefresh(_)));
    }

    #[test]
    fn rejects_invalid_log_stream_max() {
        let err = Settings::from_map(&map(&[("LOG_STREAM_MAX_SECS", "forever")]))
            .expect_err("invalid max");
        assert!(matches!(err, SettingsError::InvalidLogStreamMax(_)));
    }

    #[test]
    fn accepts_zero_log_stream_max_to_disable_the_cap() {
        let settings =
            Settings::from_map(&map(&[("LOG_STREAM_MAX_SECS", "0")])).expect("disabled cap");
        assert_eq!(settings.log_stream_max_secs, 0);
    }

    #[test]
    fn accepts_zero_metrics_toggles() {
        let settings = Settings::from_map(&map(&[
            ("METRICS_REFRESH_SECS", "0"),
            ("METRICS_DU_EVERY_SECS", "0"),
            ("METRICS_TTL_SECS", "0"),
        ]))
        .expect("metrics disabled");
        assert_eq!(settings.metrics_refresh_secs, 0);
        assert_eq!(settings.metrics_du_every_secs, 0);
        assert_eq!(settings.metrics_ttl_secs, 0);
    }

    #[test]
    fn rejects_invalid_metrics_settings() {
        let err =
            Settings::from_map(&map(&[("METRICS_REFRESH_SECS", "soon")])).expect_err("invalid");
        assert!(matches!(err, SettingsError::InvalidMetricsRefresh(_)));
        let err =
            Settings::from_map(&map(&[("METRICS_DU_EVERY_SECS", "soon")])).expect_err("invalid");
        assert!(matches!(err, SettingsError::InvalidMetricsDuTtl(_)));
        let err = Settings::from_map(&map(&[("METRICS_TTL_SECS", "soon")])).expect_err("invalid");
        assert!(matches!(err, SettingsError::InvalidMetricsTtl(_)));
    }

    #[test]
    fn rejects_invalid_ttl_settings() {
        let err = Settings::from_map(&map(&[("ACTIVITY_TTL_SECS", "soon")]))
            .expect_err("invalid activity ttl");
        assert!(matches!(err, SettingsError::InvalidActivityTtl(_)));
        let err =
            Settings::from_map(&map(&[("RUN_LOG_TTL_SECS", "soon")])).expect_err("invalid log ttl");
        assert!(matches!(err, SettingsError::InvalidRunLogTtl(_)));
    }
}
