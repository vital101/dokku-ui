use crate::domain::types::{AppInfo, EnvVar, LogLines, ProcessState, ProcessStatus, PsReport};

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
        link_exists: None,
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

    Ok(PsReport {
        deployed: bool_of("deployed")?,
        running: bool_of("running")?,
        process_count: str_of("processes")?.parse().unwrap_or(-1),
        processes,
    })
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
        assert_eq!(info.link_exists, None);
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
}
