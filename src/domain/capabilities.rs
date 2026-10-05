use std::fmt;
use std::str::FromStr;

use crate::domain::command::DokkuCommand;

/// A dokku release, parsed from `dokku --version` (e.g. `dokku version 0.38.4`).
/// Compared as numeric segments so 0.38.26 sorts after 0.38.4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DokkuVersion {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl fmt::Display for DokkuVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&format!("{}.{}.{}", self.major, self.minor, self.patch))
    }
}

/// `dokku --version` reports the release as a `major.minor.patch` triplet
/// somewhere in the output (`dokku version 0.38.4`). Unrecognisable output
/// yields `None` so probing degrades instead of failing.
pub fn parse_dokku_version(output: &str) -> Option<DokkuVersion> {
    for token in output.split_whitespace() {
        let mut parts = token.split('.');
        let (Some(major), Some(minor), Some(patch)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if parts.next().is_some() {
            continue;
        }
        if let (Ok(major), Ok(minor), Ok(patch)) = (
            major.parse::<u64>(),
            minor.parse::<u64>(),
            patch.parse::<u64>(),
        ) {
            return Some(DokkuVersion {
                major,
                minor,
                patch,
            });
        }
    }
    None
}

/// Command families whose live-tail support is probed with `<cmd> --help`
/// (see [`DokkuCommand::Help`]). A family is supported when the help text
/// advertises the follow flag (`--tail`/`-t`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CapabilityFamily {
    Logs,
    NginxAccessLogs,
    NginxErrorLogs,
}

impl CapabilityFamily {
    pub fn all() -> [Self; 3] {
        [Self::Logs, Self::NginxAccessLogs, Self::NginxErrorLogs]
    }

    /// The dokku command whose `--help` output is probed.
    pub fn help_command(&self) -> &'static str {
        match self {
            Self::Logs => "logs",
            Self::NginxAccessLogs => "nginx:access-logs",
            Self::NginxErrorLogs => "nginx:error-logs",
        }
    }

    /// Human-facing label.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Logs => "app logs",
            Self::NginxAccessLogs => "nginx access logs",
            Self::NginxErrorLogs => "nginx error logs",
        }
    }
}

impl TryFrom<&str> for CapabilityFamily {
    type Error = CapabilityFamilyError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::all()
            .into_iter()
            .find(|family| family.help_command() == value)
            .ok_or_else(|| CapabilityFamilyError::Unsupported(value.to_owned()))
    }
}

impl FromStr for CapabilityFamily {
    type Err = CapabilityFamilyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CapabilityFamilyError {
    #[error("unsupported capability family `{0}`")]
    Unsupported(String),
}

/// A family's `--help` output advertises live tail when it names the follow
/// flag. Anything else (missing family command, bare usage line) is `false`.
pub fn parse_help_supports_tail(output: &str) -> bool {
    output
        .split_whitespace()
        .any(|token| token == "--tail" || token == "-t")
}

/// `dokku plugin:list` -> names of all *enabled* plugins (core, service, and
/// community alike). Malformed lines are skipped.
pub fn parse_plugin_names(output: &str) -> Vec<String> {
    output
        .lines()
        .map(|line| line.trim().to_owned())
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 3 || fields[2] != "enabled" {
                return None;
            }
            Some(fields[0].to_owned())
        })
        .collect()
}

/// What a command or family needs from the host, declared once next to the
/// argv definition in [`DokkuCommand::requirement`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Requirement {
    /// Part of dokku core; always present.
    Core,
    /// Only available on dokku releases at least `min` (numeric compare).
    Version { min: DokkuVersion },
    /// Only available when the named plugin is enabled.
    Plugin { name: String },
}

/// The verdict of a capability check, consulted by handlers to register tabs
/// or render an explanatory state — never to fail at command time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Support {
    Supported,
    /// No probe data yet (cold start without a row); treat as "check later".
    Unknown,
    VersionTooOld {
        required: DokkuVersion,
        found: Option<DokkuVersion>,
    },
    PluginMissing {
        plugin: String,
    },
    FamilyUnsupported {
        family: CapabilityFamily,
    },
}

impl Support {
    /// Short, user-facing explanation rendered next to gated UI.
    pub fn label(&self) -> String {
        match self {
            Self::Supported => "available".to_owned(),
            Self::Unknown => "unknown (no probe data yet)".to_owned(),
            Self::VersionTooOld { required, found } => match found {
                Some(found) => format!("requires dokku ≥ {required}, host runs {found}"),
                None => format!("requires dokku ≥ {required}"),
            },
            Self::PluginMissing { plugin } => format!("requires the `{plugin}` plugin"),
            Self::FamilyUnsupported { family } => format!(
                "live tail is not available on this host (`{0}` has no follow flag)",
                family.help_command()
            ),
        }
    }
}

/// Parsed view of the host's capabilities, published to a shared SQLite row so
/// every process answers identically and cold starts cost zero SSH.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    pub dokku_version: Option<DokkuVersion>,
    pub enabled_plugins: Vec<String>,
    /// Help-command names of families whose `--help` advertises live tail.
    pub log_sources: Vec<String>,
}

impl Capabilities {
    pub fn supports_command(&self, command: &DokkuCommand) -> Support {
        self.supports_requirement(&command.requirement())
    }

    pub fn supports_requirement(&self, requirement: &Requirement) -> Support {
        match requirement {
            Requirement::Core => Support::Supported,
            Requirement::Version { min } => match self.dokku_version {
                None => Support::Unknown,
                Some(found) if found < *min => Support::VersionTooOld {
                    required: *min,
                    found: Some(found),
                },
                Some(_) => Support::Supported,
            },
            Requirement::Plugin { name } => {
                if self.enabled_plugins.is_empty() {
                    return Support::Unknown;
                }
                if self.enabled_plugins.iter().any(|plugin| plugin == name) {
                    Support::Supported
                } else {
                    Support::PluginMissing {
                        plugin: name.clone(),
                    }
                }
            }
        }
    }

    pub fn supports_family(&self, family: CapabilityFamily) -> Support {
        if self
            .log_sources
            .iter()
            .any(|source| source == family.help_command())
        {
            return Support::Supported;
        }
        match self.dokku_version {
            None => Support::Unknown,
            Some(_) => Support::FamilyUnsupported { family },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(major: u64, minor: u64, patch: u64) -> DokkuVersion {
        DokkuVersion {
            major,
            minor,
            patch,
        }
    }

    fn capabilities() -> Capabilities {
        Capabilities {
            dokku_version: Some(version(0, 38, 4)),
            enabled_plugins: vec!["apps".into(), "logs".into(), "nginx-vhosts".into()],
            log_sources: vec!["logs".into()],
        }
    }

    fn redis_plugin() -> crate::domain::ServicePlugin {
        crate::domain::ServicePlugin::try_from("redis").expect("redis plugin")
    }

    #[test]
    fn parses_dokku_version_from_output() {
        assert_eq!(
            parse_dokku_version("dokku version 0.38.4\n"),
            Some(version(0, 38, 4))
        );
        assert_eq!(
            parse_dokku_version("dokku version 0.38.26\n"),
            Some(version(0, 38, 26))
        );
        assert_eq!(parse_dokku_version("dokku 0.9.1\n"), Some(version(0, 9, 1)));
    }

    #[test]
    fn unparseable_version_output_yields_none() {
        for output in [
            "",
            "dokku version\n",
            "dokku version latest\n",
            "0.38\n",
            "a.b.c",
        ] {
            assert_eq!(parse_dokku_version(output), None, "{output:?}");
        }
    }

    #[test]
    fn versions_compare_numerically_per_segment() {
        assert!(version(0, 38, 26) > version(0, 38, 4));
        assert!(version(0, 38, 4) > version(0, 38, 0));
        assert!(version(1, 0, 0) > version(0, 99, 99));
        assert_eq!(version(0, 38, 4), version(0, 38, 4));
        assert!(version(0, 38, 4) < version(0, 38, 26));
    }

    #[test]
    fn version_display_roundtrips() {
        assert_eq!(version(0, 38, 4).to_string(), "0.38.4");
    }

    #[test]
    fn help_output_advertises_tail_when_the_follow_flag_is_named() {
        let usage = "Usage: dokku logs [OPTIONS] <app>\nOptions:\n  -n, --num <num>    number of lines to display\n  -t, --tail         continually stream logs\n";
        assert!(parse_help_supports_tail(usage));
        assert!(!parse_help_supports_tail("Usage: dokku logs <app>"));
        assert!(!parse_help_supports_tail(""));
    }

    #[test]
    fn plugin_names_keeps_enabled_plugins_only() {
        let output = "  apps 0.38.4 enabled dokku core apps plugin\n  redis 1.42.1 disabled dokku redis service plugin\n  letsencrypt 0.20.4 enabled Automated installation\n  malformed line\n";
        assert_eq!(parse_plugin_names(output), vec!["apps", "letsencrypt"]);
    }

    #[test]
    fn plugin_names_empty_and_header_only() {
        assert_eq!(parse_plugin_names(""), Vec::<String>::new());
        assert_eq!(parse_plugin_names("=====> Plugins"), Vec::<String>::new());
    }

    #[test]
    fn core_requirements_are_always_supported() {
        assert_eq!(
            capabilities().supports_command(&DokkuCommand::AppsList),
            Support::Supported
        );
        assert_eq!(
            capabilities().supports_command(&DokkuCommand::ConfigShow {
                app: crate::domain::AppName::try_from("alpha").expect("app"),
            }),
            Support::Supported
        );
    }

    #[test]
    fn plugin_requirements_check_the_enabled_list() {
        let caps = capabilities();
        assert_eq!(
            caps.supports_command(&DokkuCommand::ServiceList {
                plugin: redis_plugin()
            }),
            Support::PluginMissing {
                plugin: "redis".into()
            }
        );
        let caps = Capabilities {
            dokku_version: caps.dokku_version,
            enabled_plugins: vec!["apps".into(), "redis".into()],
            log_sources: caps.log_sources,
        };
        assert_eq!(
            caps.supports_command(&DokkuCommand::ServiceList {
                plugin: redis_plugin()
            }),
            Support::Supported
        );
    }

    #[test]
    fn plugin_check_is_unknown_without_probe_data() {
        let caps = Capabilities {
            dokku_version: None,
            enabled_plugins: Vec::new(),
            log_sources: Vec::new(),
        };
        assert_eq!(
            caps.supports_command(&DokkuCommand::ServiceList {
                plugin: redis_plugin()
            }),
            Support::Unknown
        );
    }

    #[test]
    fn version_requirements_gate_older_hosts() {
        let caps = capabilities();
        assert_eq!(
            caps.supports_requirement(&Requirement::Version {
                min: version(0, 38, 4)
            }),
            Support::Supported
        );
        assert!(matches!(
            caps.supports_requirement(&Requirement::Version {
                min: version(0, 39, 0)
            }),
            Support::VersionTooOld { .. }
        ));
        let unknown = Capabilities {
            dokku_version: None,
            enabled_plugins: Vec::new(),
            log_sources: Vec::new(),
        };
        assert_eq!(
            unknown.supports_requirement(&Requirement::Version {
                min: version(0, 39, 0)
            }),
            Support::Unknown
        );
    }

    #[test]
    fn family_support_comes_from_the_probed_help() {
        let caps = capabilities();
        assert_eq!(
            caps.supports_family(CapabilityFamily::Logs),
            Support::Supported
        );
        assert_eq!(
            caps.supports_family(CapabilityFamily::NginxAccessLogs),
            Support::FamilyUnsupported {
                family: CapabilityFamily::NginxAccessLogs
            }
        );
        let unknown = Capabilities {
            dokku_version: None,
            enabled_plugins: Vec::new(),
            log_sources: Vec::new(),
        };
        assert_eq!(
            unknown.supports_family(CapabilityFamily::Logs),
            Support::Unknown
        );
    }

    #[test]
    fn family_try_from_matches_help_commands() {
        for family in CapabilityFamily::all() {
            let parsed: CapabilityFamily = family.help_command().parse().expect("parse");
            assert_eq!(parsed, family);
        }
        assert_eq!(
            CapabilityFamily::try_from("redis"),
            Err(CapabilityFamilyError::Unsupported("redis".into()))
        );
    }

    #[test]
    fn support_labels_are_explanatory() {
        assert_eq!(Support::Supported.label(), "available");
        assert_eq!(Support::Unknown.label(), "unknown (no probe data yet)");
        assert_eq!(
            Support::PluginMissing {
                plugin: "redis".into()
            }
            .label(),
            "requires the `redis` plugin"
        );
        assert_eq!(
            Support::VersionTooOld {
                required: version(0, 39, 0),
                found: Some(version(0, 38, 4)),
            }
            .label(),
            "requires dokku ≥ 0.39.0, host runs 0.38.4"
        );
        let label = Support::FamilyUnsupported {
            family: CapabilityFamily::NginxAccessLogs,
        }
        .label();
        assert!(label.contains("nginx:access-logs"));
    }
}
