use crate::domain::types::{
    AppInfo, BuildInfo, ContainerDetails, EnvVar, ImageStatus, LogLines, ProcessState,
    ProcessStatus, PsReport, ResourceReport, ScaleEntry, ServiceInfo,
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("invalid JSON: {0}")]
    InvalidJson(String),
    #[error("missing key `{0}`")]
    MissingKey(&'static str),
    #[error("invalid boolean value for `{0}`: `{1}`")]
    InvalidBool(&'static str, String),
}

pub fn parse_apps_list(output: &str) -> Vec<String> {
    output
        .lines()
        .map(strip_ansi)
        .map(|line| line.trim().to_owned())
        .filter(|line| !line.is_empty() && !line.starts_with("====="))
        .collect()
}

pub fn parse_apps_report(json: &str, name: &str) -> Option<AppInfo> {
    let map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(json).ok()?;

    let str_of = |keys: [&str; 2]| -> Option<String> {
        keys.iter().find_map(|key| {
            map.get(*key)
                .and_then(|value| value.as_str())
                .map(str::to_owned)
        })
    };

    Some(AppInfo {
        name: name.to_owned(),
        created_at: format_created_at(
            &str_of(["app-created-at", "app created at"]).unwrap_or_default(),
        ),
        locked: str_of(["app-locked", "app locked"]).as_deref() == Some("true"),
        image_status: None,
        last_build: None,
        links: None,
        domains: Vec::new(),
        dns_record_exists: None,
    })
}

fn format_created_at(raw: &str) -> String {
    let Ok(secs) = raw.parse::<i64>() else {
        return raw.to_owned();
    };
    let Ok(datetime) = time::OffsetDateTime::from_unix_timestamp(secs) else {
        return raw.to_owned();
    };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02} UTC",
        datetime.year(),
        u8::from(datetime.month()),
        datetime.day(),
        datetime.hour(),
        datetime.minute()
    )
}

/// `dokku builds:report <app> --format json` as a typed last-build summary.
pub fn parse_build_info(json: &str) -> Option<BuildInfo> {
    let map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(json).ok()?;
    let field = |key: &str| {
        map.get(key)
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_owned()
    };
    Some(BuildInfo {
        status: field("build-status"),
        exit_code: field("build-exit-code"),
        kind: field("build-kind"),
        started_at: field("build-started-at"),
        finished_at: field("build-finished-at"),
    })
}

/// `dokku builds:report <app> --format json`. A never-built app returns every
/// `build-*` key as an empty string; an invalid response yields `None` (unknown).
pub fn parse_builds_report(json: &str) -> Option<ImageStatus> {
    parse_build_info(json).map(|build| build.image_status())
}

/// `dokku domains:report <app> --format json` -> the app's vhost hostnames.
pub fn parse_domains_report(json: &str) -> Vec<String> {
    let Ok(map) = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(json) else {
        return Vec::new();
    };
    map.get("app-vhosts")
        .and_then(|value| value.as_str())
        .map(|vhosts| vhosts.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default()
}

/// `dokku plugin:list` -> names of enabled plugins whose description ends in
/// `service plugin` (the datastore plugins that can be linked to apps).
pub fn parse_service_plugins(output: &str) -> Vec<String> {
    output
        .lines()
        .map(strip_ansi)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 4 || fields[2] != "enabled" {
                return None;
            }
            let description = fields[3..].join(" ");
            description
                .ends_with("service plugin")
                .then(|| fields[0].to_owned())
        })
        .collect()
}

/// `<plugin>:app-links <app>` -> names of services linked to the app (one per line).
pub fn parse_app_links(output: &str) -> Vec<String> {
    parse_apps_list(output)
}

pub fn parse_ps_report(json: &str) -> Result<PsReport, ParseError> {
    let map: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(json).map_err(|err| ParseError::InvalidJson(err.to_string()))?;

    let str_of = |key: &'static str| -> Result<String, ParseError> {
        map.get(key)
            .and_then(|value| value.as_str())
            .map(str::to_owned)
            .ok_or(ParseError::MissingKey(key))
    };
    let bool_of = |key: &'static str| -> Result<bool, ParseError> {
        let raw = str_of(key)?;
        match raw.as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            other => Err(ParseError::InvalidBool(key, other.to_owned())),
        }
    };

    let processes = map
        .iter()
        .filter(|(key, _)| key.starts_with("status-"))
        .map(|(key, value)| ProcessStatus {
            process_type: key.trim_start_matches("status-").to_owned(),
            state: ProcessState::parse(value.as_str().unwrap_or("missing")),
        })
        .collect();

    let can_scale = map
        .get("ps-can-scale")
        .and_then(|value| value.as_str())
        .and_then(|raw| match raw {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        });

    Ok(PsReport {
        deployed: bool_of("deployed")?,
        running: bool_of("running")?,
        process_count: str_of("processes")?.parse().unwrap_or(-1),
        processes,
        can_scale,
    })
}

/// `dokku ps:scale <app> --format json` -> the desired formation. An empty
/// array (never scaled) yields an empty list; malformed input also yields empty
/// so the caller degrades rather than erroring.
pub fn parse_ps_scale(json: &str) -> Vec<ScaleEntry> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let serde_json::Value::Array(entries) = value else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let process_type = entry.get("process_type")?.as_str()?.to_owned();
            let quantity = entry.get("quantity")?;
            let quantity = quantity
                .as_u64()
                .or_else(|| quantity.as_str()?.parse::<u64>().ok())?;
            Some(ScaleEntry {
                process_type,
                quantity: u32::try_from(quantity).unwrap_or(u32::MAX),
            })
        })
        .collect()
}

/// `dokku ps:inspect <app>` -> sanitized `docker inspect`. Accepts either a JSON
/// array (the usual shape) or a single object; unreadable entries are skipped.
pub fn parse_ps_inspect(json: &str) -> Vec<ContainerDetails> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let objects: Vec<&serde_json::Value> = match &value {
        serde_json::Value::Array(entries) => entries.iter().collect(),
        serde_json::Value::Object(_) => vec![&value],
        _ => Vec::new(),
    };
    objects
        .iter()
        .filter_map(|object| parse_one_container(object))
        .collect()
}

fn parse_one_container(object: &serde_json::Value) -> Option<ContainerDetails> {
    let id = object.get("Id")?.as_str()?.to_owned();
    let id_short: String = id.chars().take(12).collect();
    let name = object
        .get("Name")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim_start_matches('/')
        .to_owned();
    let image = object
        .pointer("/Config/Image")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_owned();
    let state = object
        .pointer("/State/Status")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown")
        .to_owned();
    let started_at = object
        .pointer("/State/StartedAt")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_owned();
    let restart_count = object
        .pointer("/State/RestartCount")
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    let oom_killed = object
        .pointer("/State/OOMKilled")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let exit_code = object
        .pointer("/State/ExitCode")
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    Some(ContainerDetails {
        id_short,
        name,
        image,
        state,
        started_at,
        restart_count,
        oom_killed,
        exit_code,
    })
}

/// `dokku resource:report <app>` (text) -> per-process-type limits/reservations.
/// Lines look like `web limit memory: 1024` / `web reservation cpu:`. Unknown
/// fields and malformed lines are ignored.
pub fn parse_resource_report(output: &str) -> Vec<ResourceReport> {
    use std::collections::BTreeMap;

    let mut by_type: BTreeMap<String, ResourceReport> = BTreeMap::new();
    for line in output.lines() {
        let line = strip_ansi(line);
        let line = line.trim();
        if line.is_empty() || line.starts_with("=====") {
            continue;
        }
        let Some((left, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().to_owned();
        let mut words = left.split_whitespace();
        let (Some(process_type), Some(kind)) = (words.next(), words.next()) else {
            continue;
        };
        let field = words.collect::<Vec<_>>().join(" ");
        let entry = by_type
            .entry(process_type.to_owned())
            .or_insert_with(|| ResourceReport {
                process_type: process_type.to_owned(),
                ..ResourceReport::default()
            });
        match (kind, field.as_str()) {
            ("limit", "cpu") => entry.limit_cpu = value,
            ("limit", "memory") => entry.limit_memory = value,
            ("limit", "memory swap") => entry.limit_memory_swap = value,
            ("reservation", "cpu") => entry.reserve_cpu = value,
            ("reservation", "memory") => entry.reserve_memory = value,
            _ => {}
        }
    }
    by_type.into_values().collect()
}

/// `<plugin>:info <service> --format json`. Accepts a single JSON object, a
/// JSON array, or JSON-lines; unknown keys are ignored and the DSN is dropped.
pub fn parse_service_info(json: &str, plugin: &str, service: &str) -> Option<ServiceInfo> {
    let value = parse_first_json_object(json)?;
    let field = |keys: &[&str]| -> String {
        keys.iter()
            .find_map(|key| value.get(*key).and_then(json_value_to_string))
            .unwrap_or_default()
    };
    let linked_apps = value
        .get("links")
        .map(json_value_to_list)
        .unwrap_or_default();
    Some(ServiceInfo {
        plugin: plugin.to_owned(),
        service: {
            let reported = field(&["service"]);
            if reported.is_empty() {
                service.to_owned()
            } else {
                reported
            }
        },
        status: field(&["status"]),
        version: field(&["version", "image-version"]),
        exposed_ports: field(&["exposed-ports", "exposed_ports"]),
        internal_ip: field(&["internal-ip", "internal_ip"]),
        memory: field(&["memory"]),
        created: field(&["created"]),
        linked_apps,
    })
}

fn parse_first_json_object(json: &str) -> Option<serde_json::Value> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(json) {
        match value {
            serde_json::Value::Object(_) => return Some(value),
            serde_json::Value::Array(entries) => {
                return entries.into_iter().find(|entry| entry.is_object());
            }
            _ => {}
        }
    }
    json.lines()
        .find_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(serde_json::Value::is_object)
}

fn json_value_to_string(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Number(number) => Some(number.to_string()),
        serde_json::Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn json_value_to_list(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::Array(entries) => entries
            .iter()
            .filter_map(json_value_to_string)
            .filter(|entry| !entry.is_empty() && entry != "-")
            .collect(),
        serde_json::Value::String(text) => text
            .split_whitespace()
            .filter(|entry| !entry.is_empty() && *entry != "-")
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

pub fn parse_config_show(output: &str) -> Vec<EnvVar> {
    output
        .lines()
        .filter(|line| !line.is_empty() && !line.trim_start().starts_with("====="))
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            let key = key.trim();
            if key.is_empty() {
                return None;
            }
            Some(EnvVar {
                key: key.to_owned(),
                value: value.trim().to_owned(),
            })
        })
        .collect()
}

pub fn parse_logs(output: &str) -> LogLines {
    LogLines::new(output.lines().map(strip_ansi).collect())
}

pub const LOG_LINES_DEFAULT: u32 = 200;
pub const LOG_LINES_MIN: u32 = 10;
pub const LOG_LINES_MAX: u32 = 1000;

pub fn clamp_log_lines(raw: Option<&str>) -> u32 {
    let parsed = raw
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(LOG_LINES_DEFAULT);
    parsed.clamp(LOG_LINES_MIN, LOG_LINES_MAX)
}

fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.next() == Some('[') {
                for esc in chars.by_ref() {
                    if ('@'..='~').contains(&esc) {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const APPS_LIST: &str = include_str!("../../tests/fixtures/apps_list.txt");
    const APPS_REPORT: &str = include_str!("../../tests/fixtures/apps_report.json");
    const PS_REPORT: &str = include_str!("../../tests/fixtures/ps_report.json");
    const PS_REPORT_NOT_DEPLOYED: &str =
        include_str!("../../tests/fixtures/ps_report_not_deployed.json");
    const PS_REPORT_MISSING: &str = include_str!("../../tests/fixtures/ps_report_missing.json");
    const CONFIG_SHOW: &str = include_str!("../../tests/fixtures/config_show.txt");
    const CONFIG_SHOW_EMPTY: &str = include_str!("../../tests/fixtures/config_show_empty.txt");
    const LOGS: &str = include_str!("../../tests/fixtures/logs.txt");
    const BUILDS_REPORT: &str = include_str!("../../tests/fixtures/builds_report.json");
    const BUILDS_REPORT_FAILED: &str =
        include_str!("../../tests/fixtures/builds_report_failed.json");
    const BUILDS_REPORT_EMPTY: &str = include_str!("../../tests/fixtures/builds_report_empty.json");
    const DOMAINS_REPORT: &str = include_str!("../../tests/fixtures/domains_report.json");
    const DOMAINS_REPORT_DEFAULT: &str =
        include_str!("../../tests/fixtures/domains_report_default.json");
    const PLUGIN_LIST: &str = include_str!("../../tests/fixtures/plugin_list.txt");
    const APP_LINKS: &str = include_str!("../../tests/fixtures/app_links.txt");
    const PS_SCALE: &str = include_str!("../../tests/fixtures/ps_scale.json");
    const PS_SCALE_EMPTY: &str = include_str!("../../tests/fixtures/ps_scale_empty.json");
    const PS_INSPECT: &str = include_str!("../../tests/fixtures/ps_inspect.json");
    const RESOURCE_REPORT: &str = include_str!("../../tests/fixtures/resource_report.txt");
    const REDIS_INFO: &str = include_str!("../../tests/fixtures/redis_info.json");
    const POSTGRES_INFO: &str = include_str!("../../tests/fixtures/postgres_info.json");

    #[test]
    fn parses_apps_list_fixture() {
        assert_eq!(
            parse_apps_list(APPS_LIST),
            vec!["myapp", "my-app", "api.internal", "1st-app"]
        );
    }

    #[test]
    fn parses_empty_apps_list() {
        assert_eq!(parse_apps_list("=====> My Apps\n"), Vec::<String>::new());
        assert_eq!(parse_apps_list(""), Vec::<String>::new());
    }

    #[test]
    fn apps_list_strips_ansi_and_skips_headers_and_blanks() {
        let output = "\u{1b}[36m=====> My Apps\u{1b}[0m\n\n  alpha  \n\u{1b}[32mbeta\u{1b}[0m\n\n";
        assert_eq!(parse_apps_list(output), vec!["alpha", "beta"]);
    }

    #[test]
    fn parses_apps_report_fixture() {
        let info = parse_apps_report(APPS_REPORT, "dokku-ui").expect("parse");
        assert_eq!(info.name, "dokku-ui");
        assert_eq!(info.created_at, "2026-10-03 10:36 UTC");
        assert!(!info.locked);
        assert_eq!(info.image_status, None);
        assert_eq!(info.last_build, None);
        assert_eq!(info.links, None);
        assert_eq!(info.dns_record_exists, None);
    }

    #[test]
    fn apps_report_locked_flag_parses() {
        let info = parse_apps_report(
            r#"{"app-created-at": "1791023796", "app-locked": "true"}"#,
            "myapp",
        )
        .expect("parse");
        assert!(info.locked);
    }

    #[test]
    fn apps_report_accepts_spaced_keys_from_older_dokku() {
        let info = parse_apps_report(
            r#"{"app created at": "2026-01-01T00:00:00Z", "app locked": "true"}"#,
            "myapp",
        )
        .expect("parse");
        assert_eq!(info.created_at, "2026-01-01T00:00:00Z");
        assert!(info.locked);
    }

    #[test]
    fn apps_report_missing_locked_defaults_to_false() {
        let info =
            parse_apps_report(r#"{"app-created-at": "1791023796"}"#, "myapp").expect("parse");
        assert!(!info.locked);
    }

    #[test]
    fn apps_report_missing_created_at_defaults_to_empty() {
        let info = parse_apps_report(r#"{"app-locked": "false"}"#, "myapp").expect("parse");
        assert_eq!(info.created_at, "");
    }

    #[test]
    fn created_at_epoch_formats_as_utc() {
        for (raw, expected) in [
            ("0", "1970-01-01 00:00 UTC"),
            ("1791023796", "2026-10-03 10:36 UTC"),
        ] {
            let info = parse_apps_report(&format!(r#"{{"app-created-at": "{raw}"}}"#), "myapp")
                .expect("parse");
            assert_eq!(info.created_at, expected, "{raw}");
        }
    }

    #[test]
    fn created_at_non_epoch_values_pass_through() {
        for raw in ["2026-01-01T00:00:00Z", "", "not-a-date"] {
            let info = parse_apps_report(&format!(r#"{{"app-created-at": "{raw}"}}"#), "myapp")
                .expect("parse");
            assert_eq!(info.created_at, raw);
        }
    }

    #[test]
    fn rejects_invalid_apps_report_json() {
        assert_eq!(parse_apps_report("not json", "myapp"), None);
    }

    #[test]
    fn labels_render_unknown_when_fields_absent() {
        let info = parse_apps_report(APPS_REPORT, "myapp").expect("parse");
        assert_eq!(info.image_status_label(), "unknown");
        assert_eq!(info.link_exists_label(), "unknown");
        assert_eq!(info.dns_record_exists_label(), "unknown");
        assert_eq!(info.locked_label(), "no");
    }

    #[test]
    fn parses_builds_report_fixture() {
        assert_eq!(parse_builds_report(BUILDS_REPORT), Some(ImageStatus::Built));
    }

    #[test]
    fn parses_failed_builds_report() {
        assert_eq!(
            parse_builds_report(BUILDS_REPORT_FAILED),
            Some(ImageStatus::Error("build failed (exit 1)".into()))
        );
    }

    #[test]
    fn never_built_builds_report_maps_to_none() {
        assert_eq!(
            parse_builds_report(BUILDS_REPORT_EMPTY),
            Some(ImageStatus::None)
        );
    }

    #[test]
    fn rejects_invalid_builds_report() {
        assert_eq!(parse_builds_report("not json"), None);
        assert_eq!(parse_builds_report(""), None);
    }

    #[test]
    fn parses_domains_report_fixture() {
        assert_eq!(
            parse_domains_report(DOMAINS_REPORT),
            vec!["dokku.re-cycledair.com"]
        );
    }

    #[test]
    fn parses_auto_assigned_vhost() {
        assert_eq!(
            parse_domains_report(DOMAINS_REPORT_DEFAULT),
            vec!["starwars.re-cycledair.com"]
        );
    }

    #[test]
    fn domains_report_without_vhosts_is_empty() {
        assert_eq!(
            parse_domains_report(r#"{"app-enabled":"true","app-vhosts":""}"#),
            Vec::<String>::new()
        );
        assert_eq!(parse_domains_report("not json"), Vec::<String>::new());
    }

    #[test]
    fn parses_service_plugins_from_fixture() {
        let plugins = parse_service_plugins(PLUGIN_LIST);
        for expected in ["mongo", "mysql", "postgres", "redis"] {
            assert!(plugins.contains(&expected.to_owned()), "missing {expected}");
        }
        assert!(
            !plugins.contains(&"apps".to_owned()),
            "apps is not a service plugin"
        );
        assert!(!plugins.contains(&"letsencrypt".to_owned()));
    }

    #[test]
    fn service_plugins_ignores_disabled_and_core_plugins() {
        let output = "\n  postgres  1.36.4 disabled  dokku postgres service plugin\n  apps 0.38.4 enabled dokku core apps plugin\n  redis 1.42.1 enabled dokku redis service plugin\n";
        assert_eq!(parse_service_plugins(output), vec!["redis"]);
    }

    #[test]
    fn parses_app_links_fixture() {
        assert_eq!(parse_app_links(APP_LINKS), vec!["roboswarm-db"]);
        assert_eq!(parse_app_links(""), Vec::<String>::new());
        assert_eq!(
            parse_app_links("=====> app links\n\n  link-a  \n"),
            vec!["link-a"]
        );
    }

    #[test]
    fn parses_ps_report_fixture() {
        let report = parse_ps_report(PS_REPORT).expect("parse");
        assert!(report.deployed);
        assert!(report.running);
        assert_eq!(report.process_count, 1);
        assert_eq!(
            report.processes,
            vec![ProcessStatus {
                process_type: "web.1".into(),
                state: ProcessState::Running,
            }]
        );
    }

    #[test]
    fn parses_not_deployed_report() {
        let report = parse_ps_report(PS_REPORT_NOT_DEPLOYED).expect("parse");
        assert!(!report.deployed);
        assert!(!report.running);
        assert_eq!(report.process_count, 0);
        assert!(report.processes.is_empty());
    }

    #[test]
    fn parses_missing_process_report() {
        let report = parse_ps_report(PS_REPORT_MISSING).expect("parse");
        assert!(!report.deployed);
        assert!(!report.running);
        assert_eq!(report.process_count, -1);
        assert_eq!(
            report.processes,
            vec![ProcessStatus {
                process_type: "web".into(),
                state: ProcessState::Missing,
            }]
        );
    }

    #[test]
    fn rejects_report_without_deployed_key() {
        assert!(matches!(
            parse_ps_report(r#"{"running": "true", "processes": "1"}"#),
            Err(ParseError::MissingKey("deployed"))
        ));
    }

    #[test]
    fn rejects_report_with_invalid_running_bool() {
        assert!(matches!(
            parse_ps_report(r#"{"deployed": "true", "running": "maybe", "processes": "1"}"#),
            Err(ParseError::InvalidBool("running", _))
        ));
    }

    #[test]
    fn rejects_report_that_is_not_json() {
        assert!(matches!(
            parse_ps_report("nope"),
            Err(ParseError::InvalidJson(_))
        ));
    }

    #[test]
    fn rejects_report_with_non_string_processes() {
        assert!(matches!(
            parse_ps_report(r#"{"deployed": "true", "running": "false", "processes": 2}"#),
            Err(ParseError::MissingKey("processes"))
        ));
    }

    #[test]
    fn parses_config_show_fixture() {
        assert_eq!(
            parse_config_show(CONFIG_SHOW),
            vec![
                EnvVar {
                    key: "DATABASE_URL".into(),
                    value: "postgres://user:pass@host/db".into(),
                },
                EnvVar {
                    key: "DOKKU_PROXY_PORT".into(),
                    value: "80".into(),
                },
                EnvVar {
                    key: "SECRET_KEY".into(),
                    value: "s3cr3t".into(),
                },
            ]
        );
    }

    #[test]
    fn parses_empty_config_show() {
        assert_eq!(parse_config_show(CONFIG_SHOW_EMPTY), Vec::<EnvVar>::new());
    }

    #[test]
    fn config_show_skips_header_and_blank_lines() {
        let output = "\n=====> app env vars\n\n\nKEY1:  value1\n";
        assert_eq!(
            parse_config_show(output),
            vec![EnvVar {
                key: "KEY1".into(),
                value: "value1".into(),
            }]
        );
    }

    #[test]
    fn config_show_skips_lines_without_colon() {
        let output = "=====> app env vars\nnot a kv line\nKEY1: v1\n";
        assert_eq!(parse_config_show(output).len(), 1);
    }

    #[test]
    fn config_show_trims_values_with_colons() {
        let output = "=====> app env vars\nURL: http://example.com:8080/path\n";
        assert_eq!(
            parse_config_show(output),
            vec![EnvVar {
                key: "URL".into(),
                value: "http://example.com:8080/path".into(),
            }]
        );
    }

    #[test]
    fn parses_logs_and_strips_ansi() {
        let lines = parse_logs(LOGS);
        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines.as_slice()[1],
            "2026-09-29T10:00:01.000000000Z app[web.1]: GET /healthz 200"
        );
        assert_eq!(
            lines.as_slice()[2],
            "2026-09-29T10:00:02.000000000Z app[web.1]: ERROR something failed"
        );
    }

    #[test]
    fn parses_empty_logs() {
        assert!(parse_logs("").is_empty());
    }

    #[test]
    fn clamp_defaults_to_200_for_missing_or_unparseable() {
        for raw in [None, Some(""), Some("abc"), Some("-5"), Some("12.5")] {
            assert_eq!(clamp_log_lines(raw), LOG_LINES_DEFAULT, "{raw:?}");
        }
    }

    #[test]
    fn clamp_enforces_minimum() {
        assert_eq!(clamp_log_lines(Some("0")), LOG_LINES_MIN);
        assert_eq!(clamp_log_lines(Some("5")), LOG_LINES_MIN);
        assert_eq!(clamp_log_lines(Some("10")), LOG_LINES_MIN);
    }

    #[test]
    fn clamp_enforces_maximum() {
        assert_eq!(clamp_log_lines(Some("1000")), LOG_LINES_MAX);
        assert_eq!(clamp_log_lines(Some("5000")), LOG_LINES_MAX);
    }

    #[test]
    fn clamp_passes_through_in_range_values() {
        assert_eq!(clamp_log_lines(Some("50")), 50);
        assert_eq!(clamp_log_lines(Some("200")), LOG_LINES_DEFAULT);
    }

    #[test]
    fn parses_ps_scale_fixture() {
        assert_eq!(
            parse_ps_scale(PS_SCALE),
            vec![ScaleEntry::new("web", 1), ScaleEntry::new("worker", 3)]
        );
    }

    #[test]
    fn parses_empty_ps_scale() {
        assert_eq!(parse_ps_scale(PS_SCALE_EMPTY), Vec::<ScaleEntry>::new());
        assert_eq!(parse_ps_scale("[]"), Vec::<ScaleEntry>::new());
    }

    #[test]
    fn ps_scale_accepts_string_quantities_and_skips_bad_entries() {
        let json = r#"[{"process_type":"web","quantity":"2"},{"process_type":"worker"},{"quantity":1},{"process_type":"cron","quantity":0}]"#;
        assert_eq!(
            parse_ps_scale(json),
            vec![ScaleEntry::new("web", 2), ScaleEntry::new("cron", 0)]
        );
    }

    #[test]
    fn ps_scale_malformed_is_empty() {
        assert_eq!(parse_ps_scale("not json"), Vec::<ScaleEntry>::new());
        assert_eq!(
            parse_ps_scale(r#"{"process_type":"web"}"#),
            Vec::<ScaleEntry>::new()
        );
    }

    #[test]
    fn parses_ps_inspect_fixture() {
        let containers = parse_ps_inspect(PS_INSPECT);
        assert_eq!(containers.len(), 2);
        let web = &containers[0];
        assert_eq!(web.name, "alpha.web.1");
        assert_eq!(web.image, "dokku/alpha:latest");
        assert_eq!(web.state, "running");
        assert_eq!(web.id_short, "b1a216b5b1d4");
        assert!(!web.oom_killed);
        assert_eq!(web.restart_count, 0);
        assert_eq!(containers[1].restart_count, 2);
        assert!(containers[1].oom_killed);
    }

    #[test]
    fn ps_inspect_accepts_single_object() {
        let json = r#"{"Id":"abcdef012345","Name":"/app.web.1","Config":{"Image":"img"},"State":{"Status":"running","StartedAt":"2026-01-01T00:00:00Z","RestartCount":1,"OOMKilled":false,"ExitCode":0}}"#;
        let containers = parse_ps_inspect(json);
        assert_eq!(containers.len(), 1);
        assert_eq!(containers[0].name, "app.web.1");
        assert_eq!(containers[0].restart_count, 1);
    }

    #[test]
    fn ps_inspect_malformed_is_empty() {
        assert_eq!(parse_ps_inspect("not json"), Vec::<ContainerDetails>::new());
        assert_eq!(parse_ps_inspect("[1,2,3]"), Vec::<ContainerDetails>::new());
    }

    #[test]
    fn parses_resource_report_fixture() {
        let reports = parse_resource_report(RESOURCE_REPORT);
        assert_eq!(reports.len(), 2);
        let web = reports
            .iter()
            .find(|report| report.process_type == "web")
            .expect("web report");
        assert_eq!(web.limit_memory, "1024");
        assert_eq!(web.limit_memory_swap, "0");
        assert_eq!(web.reserve_memory, "512");
        assert_eq!(web.limit_cpu, "");
        let worker = reports
            .iter()
            .find(|report| report.process_type == "worker")
            .expect("worker report");
        assert_eq!(worker.limit_cpu, "2");
        assert_eq!(worker.limit_memory, "");
    }

    #[test]
    fn resource_report_ignores_headers_blanks_and_unknown_fields() {
        let output = "=====> alpha resource information\n\n  web limit cpu: 1\n  web limit network: 10\n  web mystery field: x\nnot a line\n";
        let reports = parse_resource_report(output);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].limit_cpu, "1");
        assert_eq!(reports[0].limit_memory, "");
    }

    #[test]
    fn resource_report_empty_is_empty() {
        assert_eq!(parse_resource_report(""), Vec::<ResourceReport>::new());
    }

    #[test]
    fn parses_redis_service_info_without_the_dsn() {
        let info = parse_service_info(REDIS_INFO, "redis", "roboswarm-db").expect("info");
        assert_eq!(info.plugin, "redis");
        assert_eq!(info.service, "roboswarm-db");
        assert_eq!(info.status, "running");
        assert_eq!(info.version, "8.10.1");
        assert_eq!(info.internal_ip, "172.17.0.5");
        assert_eq!(info.linked_apps, vec!["alpha", "beta"]);
    }

    #[test]
    fn parses_postgres_service_info_array_shape() {
        let info = parse_service_info(POSTGRES_INFO, "postgres", "db").expect("info");
        assert_eq!(info.service, "db");
        assert_eq!(info.exposed_ports, "5432");
        assert!(info.linked_apps.is_empty());
    }

    #[test]
    fn service_info_falls_back_to_requested_service_and_ignores_dashes() {
        let json = r#"{"status":"running","links":"-","dsn":"redis://:secret@host"}"#;
        let info = parse_service_info(json, "redis", "cache").expect("info");
        assert_eq!(info.service, "cache");
        assert!(info.linked_apps.is_empty());
    }

    #[test]
    fn service_info_parses_json_lines_and_rejects_garbage() {
        let json = "{\"service\":\"one\",\"status\":\"running\"}\n{\"service\":\"two\",\"status\":\"stopped\"}";
        assert_eq!(
            parse_service_info(json, "redis", "x").map(|info| info.service),
            Some("one".to_owned())
        );
        assert_eq!(parse_service_info("nope", "redis", "x"), None);
    }

    #[test]
    fn parses_build_info_fixture() {
        let build = parse_build_info(BUILDS_REPORT).expect("build");
        assert_eq!(build.status, "succeeded");
        assert_eq!(build.kind, "build");
        assert_eq!(build.exit_code, "0");
        assert_eq!(build.finished_at, "2026-10-03T17:10:52Z");
        assert!(!build.is_failed());
        assert_eq!(build.status_label(), "succeeded");
    }

    #[test]
    fn build_info_labels() {
        let empty = parse_build_info(BUILDS_REPORT_EMPTY).expect("build");
        assert_eq!(empty.status_label(), "never built");
        assert_eq!(empty.image_status(), ImageStatus::None);
        let failed = parse_build_info(BUILDS_REPORT_FAILED).expect("build");
        assert!(failed.is_failed());
        assert_eq!(
            failed.image_status(),
            ImageStatus::Error("build failed (exit 1)".into())
        );
    }

    #[test]
    fn parses_can_scale_flag() {
        assert_eq!(
            parse_ps_report(PS_REPORT).expect("parse").can_scale,
            Some(true)
        );
        assert_eq!(
            parse_ps_report(
                r#"{"deployed":"true","running":"false","processes":"0","ps-can-scale":"false"}"#
            )
            .expect("parse")
            .can_scale,
            Some(false)
        );
        assert_eq!(
            parse_ps_report(r#"{"deployed":"true","running":"false","processes":"0"}"#)
                .expect("parse")
                .can_scale,
            None
        );
    }
}
