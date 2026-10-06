use std::fmt;
use std::str::FromStr;

/// Service plugins the UI knows how to drive. Dokku names services with a
/// plugin prefix (`postgres:info`, `redis:link`, …); keeping the set closed
/// means every `{plugin}` URL segment can be validated before any SSH command
/// is built. The sidebar renders only the plugins the host actually has
/// installed (via the capabilities probe), so this list can name the wider
/// official catalog.
pub const SERVICE_PLUGINS: [&str; 8] = [
    "postgres",
    "mysql",
    "redis",
    "mongo",
    "mariadb",
    "memcached",
    "rabbitmq",
    "clickhouse",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ServicePlugin(&'static str);

impl ServicePlugin {
    pub fn all() -> impl Iterator<Item = ServicePlugin> {
        SERVICE_PLUGINS.into_iter().map(ServicePlugin)
    }

    pub fn as_str(&self) -> &'static str {
        self.0
    }

    /// Human-facing label (sidebar, headings, badges).
    pub fn display_name(&self) -> &'static str {
        match self.0 {
            "postgres" => "PostgreSQL",
            "mysql" => "MySQL",
            "redis" => "Redis",
            "mongo" => "MongoDB",
            "mariadb" => "MariaDB",
            "memcached" => "Memcached",
            "rabbitmq" => "RabbitMQ",
            "clickhouse" => "ClickHouse",
            _ => self.0,
        }
    }

    /// Where the service's data directory is mounted inside its container.
    /// Used by the stats script's `du`/`df` calls; verified against the
    /// installed plugin generations' `/proc/mounts` output (the added plugins
    /// follow their official images' data directories; memcached keeps no
    /// persistent volume, so usage measures the container root).
    pub fn data_dir(&self) -> &'static str {
        match self.0 {
            "postgres" => "/var/lib/postgresql/data",
            "mysql" | "mariadb" => "/var/lib/mysql",
            "redis" => "/data",
            "mongo" => "/data/db",
            "memcached" => "/",
            "rabbitmq" => "/var/lib/rabbitmq",
            "clickhouse" => "/var/lib/clickhouse",
            _ => "/data",
        }
    }
}

impl TryFrom<&str> for ServicePlugin {
    type Error = ServicePluginError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        SERVICE_PLUGINS
            .into_iter()
            .find(|plugin| *plugin == value)
            .map(ServicePlugin)
            .ok_or_else(|| ServicePluginError::Unsupported(value.to_owned()))
    }
}

impl TryFrom<String> for ServicePlugin {
    type Error = ServicePluginError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl FromStr for ServicePlugin {
    type Err = ServicePluginError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s)
    }
}

impl fmt::Display for ServicePlugin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServicePluginError {
    #[error("unsupported service plugin `{0}`")]
    Unsupported(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_supported_plugins() {
        for (raw, display) in [
            ("postgres", "PostgreSQL"),
            ("mysql", "MySQL"),
            ("redis", "Redis"),
            ("mongo", "MongoDB"),
            ("mariadb", "MariaDB"),
            ("memcached", "Memcached"),
            ("rabbitmq", "RabbitMQ"),
            ("clickhouse", "ClickHouse"),
        ] {
            let plugin = ServicePlugin::try_from(raw).expect("supported");
            assert_eq!(plugin.as_str(), raw);
            assert_eq!(plugin.display_name(), display);
        }
    }

    #[test]
    fn rejects_unknown_plugins() {
        for raw in ["", "maria", "postgres2", "POSTGRES", "core", "valkey"] {
            assert_eq!(
                ServicePlugin::try_from(raw),
                Err(ServicePluginError::Unsupported(raw.to_owned()))
            );
        }
    }

    #[test]
    fn all_lists_every_supported_plugin() {
        let names: Vec<&str> = ServicePlugin::all().map(|p| p.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "postgres",
                "mysql",
                "redis",
                "mongo",
                "mariadb",
                "memcached",
                "rabbitmq",
                "clickhouse",
            ]
        );
    }

    #[test]
    fn data_dirs_match_the_installed_plugins() {
        for (raw, dir) in [
            ("postgres", "/var/lib/postgresql/data"),
            ("mysql", "/var/lib/mysql"),
            ("redis", "/data"),
            ("mongo", "/data/db"),
            ("mariadb", "/var/lib/mysql"),
            ("memcached", "/"),
            ("rabbitmq", "/var/lib/rabbitmq"),
            ("clickhouse", "/var/lib/clickhouse"),
        ] {
            assert_eq!(
                ServicePlugin::try_from(raw).expect("supported").data_dir(),
                dir,
                "{raw}"
            );
        }
    }

    #[test]
    fn from_str_and_display_roundtrip() {
        let plugin: ServicePlugin = "redis".parse().expect("parse");
        assert_eq!(plugin.to_string(), "redis");
    }
}
