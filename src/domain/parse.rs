use crate::domain::mount_spec::MountSpec;
use crate::domain::types::{
    AppInfo, AppMounts, BuildInfo, ContainerDetails, EnvVar, ImageStatus, LogLines, Mount,
    ProcessState, ProcessStatus, PsReport, ResourceReport, ScaleEntry, ServiceInfo, ServiceStats,
    StorageEntry, VolumeUsage,
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

/// `<plugin>:list` -> service names. The installed plugin generation prints
/// only a `=====> <Name> services` banner and one name per line; an empty
/// listing prints a `!` warning instead. Lines that do not look like service
/// names (headers, warnings, prose) are skipped so future format additions do
/// not break the parser.
pub fn parse_service_list(output: &str) -> Vec<String> {
    output
        .lines()
        .map(strip_ansi)
        .map(|line| line.trim().to_owned())
        .filter(|line| {
            !line.is_empty()
                && !line.starts_with("=====")
                && !line.starts_with('!')
                && line
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
        .collect()
}

/// `storage:report` (no app argument) plain text -> per-app bind mounts. Each
/// app gets a `=====> <app> storage information` section with
/// `Storage build/deploy/run mounts:` lines holding `-v host:container[:opts]`
/// entries. The same mount reported under several phases collapses into one
/// [`Mount`] carrying every phase, in build/deploy/run order.
pub fn parse_storage_report(output: &str) -> Vec<AppMounts> {
    let mut apps: Vec<AppMounts> = Vec::new();
    for line in output.lines() {
        let line = strip_ansi(line);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(header) = line.strip_prefix("=====> ") {
            let app = header
                .strip_suffix(" storage information")
                .unwrap_or(header)
                .trim()
                .to_owned();
            apps.push(AppMounts {
                app,
                mounts: Vec::new(),
            });
            continue;
        }
        let Some(current) = apps.last_mut() else {
            continue;
        };
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let phase = match key.trim().to_lowercase().as_str() {
            "storage build mounts" => "build",
            "storage deploy mounts" => "deploy",
            "storage run mounts" => "run",
            _ => continue,
        };
        for spec in mount_flags(value) {
            merge_mount(current, &spec, phase);
        }
    }
    apps
}

/// Extracts the `host:container[:opts]` operands of the `-v` flags in a mount
/// list value like `-v /a:/b -v /c:/d:ro`.
fn mount_flags(value: &str) -> Vec<String> {
    let mut specs = Vec::new();
    let mut words = value.split_whitespace();
    while let Some(word) = words.next() {
        if word == "-v" {
            if let Some(spec) = words.next() {
                specs.push(spec.to_owned());
            }
        }
    }
    specs
}

fn merge_mount(app: &mut AppMounts, spec: &str, phase: &str) {
    let Ok(parsed) = MountSpec::try_from(spec) else {
        return;
    };
    let host = parsed.host().to_owned();
    let container = parsed.container().to_owned();
    let options = parsed.options().unwrap_or_default().to_owned();
    let locator = format!("{host}:{container}");

    if let Some(existing) = app
        .mounts
        .iter_mut()
        .find(|mount| mount.locator() == locator)
    {
        if !existing.phases.iter().any(|seen| seen == phase) {
            existing.phases.push(phase.to_owned());
            existing
                .phases
                .sort_by_key(|phase| phase_rank(phase.as_str()));
        }
        if existing.options.is_empty() {
            existing.options = options;
        }
        return;
    }
    app.mounts.push(Mount {
        host,
        container,
        options,
        phases: vec![phase.to_owned()],
    });
}

fn phase_rank(phase: &str) -> u8 {
    match phase {
        "build" => 0,
        "deploy" => 1,
        "run" => 2,
        _ => 3,
    }
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

/// `dokku ps:scale <app>` plain text -> the desired formation. The report is:
///
/// ```text
/// -----> Scaling for myapp
/// proctype: qty
/// --------: ---
/// web:  1
/// worker: 2
/// ```
///
/// The `----->` banner and the `proctype: qty` / `--------: ---` column header
/// are skipped. Malformed input yields an empty list so the caller degrades.
pub fn parse_ps_scale(output: &str) -> Vec<ScaleEntry> {
    output
        .lines()
        .map(strip_ansi)
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('-') {
                return None;
            }
            let (process_type, quantity) = line.split_once(':')?;
            let process_type = process_type.trim();
            if process_type.is_empty() {
                return None;
            }
            let quantity = quantity.trim().parse::<u32>().ok()?;
            Some(ScaleEntry::new(process_type, quantity))
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

/// `<plugin>:info <service>` plain text -> service details. Reads `Key: value`
/// lines under a `=====> <service> <plugin> service information` header; an
/// empty or `-` value means unset. The DSN is intentionally never parsed into
/// the returned struct, and an unrecognisable report yields `None`.
pub fn parse_service_info(output: &str, plugin: &str, service: &str) -> Option<ServiceInfo> {
    let mut info = ServiceInfo::unknown(plugin, service);
    let mut saw_header = false;
    for line in output.lines() {
        let line = strip_ansi(line);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("=====>") {
            saw_header = true;
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = normalize_report_value(value);
        match key.trim().to_lowercase().as_str() {
            "status" => info.status = value,
            "version" => info.version = value,
            "exposed ports" => info.exposed_ports = value,
            "internal ip" => info.internal_ip = value,
            "id" => info.id_short = value.chars().take(12).collect(),
            "links" => {
                info.linked_apps = value.split_whitespace().map(str::to_owned).collect();
            }
            // The Dsn contains credentials; it is read and discarded.
            "dsn" => {}
            _ => {}
        }
    }
    (saw_header && !info.status.is_empty()).then_some(info)
}

fn normalize_report_value(raw: &str) -> String {
    let value = raw.trim();
    if value == "-" {
        String::new()
    } else {
        value.to_owned()
    }
}

/// The fixed stats scripts emit `key=value` lines. Service plugins print a
/// `-----> Filesystem changes…` banner first and `storage:exec` may carry
/// docker pull noise on stderr; anything that is not a bare `key=value` line
/// is skipped.
fn parse_key_values(output: &str) -> std::collections::HashMap<String, String> {
    output
        .lines()
        .map(strip_ansi)
        .filter_map(|line| {
            let line = line.trim();
            let (key, value) = line.split_once('=')?;
            let key = key.trim();
            if key.is_empty() || key.contains(char::is_whitespace) {
                return None;
            }
            Some((key.to_owned(), value.trim().to_owned()))
        })
        .collect()
}

/// `<plugin>:enter <service> sh -c '<stats script>'` -> live container
/// stats. `None` when nothing recognisable was emitted (e.g. an error
/// banner only); missing individual keys stay `None`.
pub fn parse_service_stats(output: &str) -> Option<ServiceStats> {
    let kv = parse_key_values(output);
    let num = |key: &str| kv.get(key).and_then(|value| value.parse::<u64>().ok());
    let float = |key: &str| kv.get(key).and_then(|value| value.parse::<f64>().ok());

    let stats = ServiceStats {
        host_memory_total_kb: num("host_mem_total_kb"),
        host_memory_available_kb: num("host_mem_avail_kb"),
        memory_current_bytes: num("mem_current"),
        // `memory.max` prints `max` for an unlimited container.
        memory_limit_bytes: num("mem_limit"),
        inactive_file_bytes: num("inactive_file"),
        cpu_delta_usec: num("cpu_delta_usec"),
        cpu_total_usec: num("cpu_total_usec"),
        elapsed_secs: float("elapsed_s"),
        cpus: num("cpus"),
        data_kb: num("data_kb"),
        fs_total_kb: num("fs_total_kb"),
        fs_used_kb: num("fs_used_kb"),
        fs_avail_kb: num("fs_avail_kb"),
    };
    (!stats.is_empty()).then_some(stats)
}

/// `storage:exec <entry> -- sh -c '<usage script>'` -> disk usage for the
/// entry mounted at `/data`.
pub fn parse_volume_usage(output: &str) -> Option<VolumeUsage> {
    let kv = parse_key_values(output);
    let num = |key: &str| kv.get(key).and_then(|value| value.parse::<u64>().ok());

    let usage = VolumeUsage {
        used_kb: num("used_kb"),
        fs_total_kb: num("fs_total_kb"),
        fs_used_kb: num("fs_used_kb"),
        fs_avail_kb: num("fs_avail_kb"),
    };
    (!usage.is_empty()).then_some(usage)
}

/// `storage:list-entries --format json` -> registered entries, used to map a
/// mount's host path onto the entry name `storage:exec` needs. Unknown fields
/// are ignored; malformed JSON yields an empty list.
pub fn parse_storage_entries(json: &str) -> Vec<StorageEntry> {
    #[derive(serde::Deserialize)]
    struct RawEntry {
        name: String,
        host_path: String,
    }

    serde_json::from_str::<Vec<RawEntry>>(json)
        .map(|entries| {
            entries
                .into_iter()
                .map(|entry| StorageEntry {
                    name: entry.name,
                    host_path: entry.host_path,
                })
                .collect()
        })
        .unwrap_or_default()
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
    const PS_SCALE: &str = include_str!("../../tests/fixtures/ps_scale.txt");
    const PS_INSPECT: &str = include_str!("../../tests/fixtures/ps_inspect.json");
    const RESOURCE_REPORT: &str = include_str!("../../tests/fixtures/resource_report.txt");
    const RESOURCE_REPORT_EMPTY: &str =
        include_str!("../../tests/fixtures/resource_report_empty.txt");
    const REDIS_INFO: &str = include_str!("../../tests/fixtures/redis_info.txt");
    const POSTGRES_INFO: &str = include_str!("../../tests/fixtures/postgres_info.txt");
    const POSTGRES_LIST: &str = include_str!("../../tests/fixtures/postgres_list.txt");
    const REDIS_LIST: &str = include_str!("../../tests/fixtures/redis_list.txt");
    const SERVICE_LIST_EMPTY: &str = include_str!("../../tests/fixtures/service_list_empty.txt");
    const STORAGE_REPORT: &str = include_str!("../../tests/fixtures/storage_report.txt");
    const STORAGE_REPORT_EMPTY: &str =
        include_str!("../../tests/fixtures/storage_report_empty.txt");
    const REDIS_STATS: &str = include_str!("../../tests/fixtures/redis_stats.txt");
    const VOLUME_USAGE: &str = include_str!("../../tests/fixtures/volume_usage.txt");
    const LIST_ENTRIES: &str = include_str!("../../tests/fixtures/list_entries.json");

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
            vec![
                ScaleEntry::new("release", 0),
                ScaleEntry::new("web", 1),
                ScaleEntry::new("worker", 2),
            ]
        );
    }

    #[test]
    fn parses_empty_ps_scale() {
        let header_only = "-----> Scaling for myapp\nproctype: qty\n--------: ---\n";
        assert_eq!(parse_ps_scale(header_only), Vec::<ScaleEntry>::new());
        assert_eq!(parse_ps_scale(""), Vec::<ScaleEntry>::new());
    }

    #[test]
    fn ps_scale_ignores_malformed_rows() {
        let output = "-----> Scaling for myapp\nproctype: qty\n--------: ---\nweb:  1\nnot a row\nworker: lots\ncron: 0\n";
        assert_eq!(
            parse_ps_scale(output),
            vec![ScaleEntry::new("web", 1), ScaleEntry::new("cron", 0)]
        );
    }

    #[test]
    fn ps_scale_strips_ansi_and_trims() {
        let output = "\u{1b}[32m-----> Scaling for myapp\u{1b}[0m\n  web:   3  \n";
        assert_eq!(parse_ps_scale(output), vec![ScaleEntry::new("web", 3)]);
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
        assert_eq!(
            parse_resource_report(RESOURCE_REPORT_EMPTY),
            Vec::<ResourceReport>::new()
        );
    }

    #[test]
    fn parses_redis_service_info_without_the_dsn() {
        let info = parse_service_info(REDIS_INFO, "redis", "candid").expect("info");
        assert_eq!(info.plugin, "redis");
        assert_eq!(info.service, "candid");
        assert_eq!(info.status, "running");
        assert_eq!(info.version, "redis:7.2.4");
        assert_eq!(info.exposed_ports, "");
        assert_eq!(info.internal_ip, "");
        assert_eq!(info.id_short, "7a1aa2430d7a");
        assert_eq!(info.linked_apps, vec!["candid"]);
    }

    #[test]
    fn parses_postgres_service_info_with_exposed_ports() {
        let info = parse_service_info(POSTGRES_INFO, "postgres", "roboswarm-db").expect("info");
        assert_eq!(info.service, "roboswarm-db");
        assert_eq!(info.status, "running");
        assert_eq!(info.version, "postgres:16.2");
        assert_eq!(info.exposed_ports, "5432->15432");
        assert_eq!(info.linked_apps, vec!["roboswarm-server"]);
        assert_eq!(info.id_short, "ca71688c254d");
    }

    #[test]
    fn service_info_never_parses_the_dsn() {
        let output = "=====> x redis service information\n       Dsn:                 redis://:secret@host:6379\n       Status:              running\n";
        let info = parse_service_info(output, "redis", "x").expect("info");
        assert_eq!(info.status, "running");
        assert!(!format!("{info:?}").contains("secret"));
    }

    #[test]
    fn service_info_ignores_dashes_and_unknown_keys() {
        let output = "=====> x postgres service information\n       Exposed ports:       -\n       Internal ip:\n       Service root:        /var/lib/dokku/services/postgres/x\n       Status:              stopped\n";
        let info = parse_service_info(output, "postgres", "x").expect("info");
        assert_eq!(info.exposed_ports, "");
        assert_eq!(info.internal_ip, "");
        assert!(info.linked_apps.is_empty());
        assert_eq!(info.status, "stopped");
    }

    #[test]
    fn service_info_requires_a_recognisable_report() {
        assert_eq!(parse_service_info("nope", "redis", "x"), None);
        // A header but no status is treated as unrecognisable.
        assert_eq!(
            parse_service_info(
                "=====> x redis service information\n       Version: redis:7.2.4\n",
                "redis",
                "x"
            ),
            None
        );
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

    #[test]
    fn parses_service_list_fixtures() {
        assert_eq!(parse_service_list(POSTGRES_LIST), vec!["roboswarm-db"]);
        assert_eq!(parse_service_list(REDIS_LIST), vec!["candid"]);
    }

    #[test]
    fn service_list_empty_warning_yields_no_names() {
        assert_eq!(parse_service_list(SERVICE_LIST_EMPTY), Vec::<String>::new());
        assert_eq!(parse_service_list(""), Vec::<String>::new());
    }

    #[test]
    fn service_list_strips_ansi_and_skips_headers_and_prose() {
        let output = "\u{1b}[36m=====> Redis services\u{1b}[0m\n\n  candid \n !     something happened\nnot a service name\nWithCaps_ok\n";
        assert_eq!(parse_service_list(output), vec!["candid", "WithCaps_ok"]);
    }

    #[test]
    fn parses_storage_report_fixture() {
        let apps = parse_storage_report(STORAGE_REPORT);
        assert_eq!(apps.len(), 14);

        let dokku_ui = apps
            .iter()
            .find(|app| app.app == "dokku-ui")
            .expect("dokku-ui");
        assert_eq!(dokku_ui.mount_count(), 1);
        let mount = &dokku_ui.mounts[0];
        assert_eq!(mount.host, "/var/lib/dokku/data/services/dokku-ui");
        assert_eq!(mount.container, "/app/data");
        assert_eq!(mount.options, "");
        assert_eq!(mount.phases, vec!["deploy", "run"]);

        let starwars = apps
            .iter()
            .find(|app| app.app == "starwars")
            .expect("starwars");
        assert!(starwars.is_empty());
    }

    #[test]
    fn storage_report_empty_fixture_yields_app_without_mounts() {
        let apps = parse_storage_report(STORAGE_REPORT_EMPTY);
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].app, "starwars");
        assert!(apps[0].is_empty());
    }

    #[test]
    fn storage_report_merges_phases_into_one_mount() {
        let output = "=====> alpha storage information\n       Storage build mounts: -v /same:/data -v /only-build:/build\n       Storage deploy mounts: -v /same:/data:ro -v /only-deploy:/deploy\n       Storage run mounts:  -v /same:/data\n";
        let apps = parse_storage_report(output);
        assert_eq!(apps.len(), 1);
        let mounts = &apps[0].mounts;
        assert_eq!(mounts.len(), 3);

        let same = &mounts[0];
        assert_eq!(same.phases, vec!["build", "deploy", "run"]);
        assert_eq!(same.options, "ro");
        assert_eq!(same.arg(), "/same:/data:ro");

        assert_eq!(mounts[1].locator(), "/only-build:/build");
        assert_eq!(mounts[1].phases, vec!["build"]);
        assert_eq!(mounts[2].locator(), "/only-deploy:/deploy");
        assert_eq!(mounts[2].phases, vec!["deploy"]);
    }

    #[test]
    fn storage_report_ignores_malformed_entries() {
        let output = "=====> alpha storage information\n       Storage deploy mounts: -v broke -v /ok:/yes -v /host:relative\nnoise\n";
        let apps = parse_storage_report(output);
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].mount_count(), 1);
        assert_eq!(apps[0].mounts[0].locator(), "/ok:/yes");
    }

    #[test]
    fn storage_report_without_sections_is_empty() {
        assert_eq!(parse_storage_report(""), Vec::<AppMounts>::new());
        assert_eq!(
            parse_storage_report("Storage deploy mounts: -v /a:/b\n"),
            Vec::<AppMounts>::new()
        );
    }

    #[test]
    fn parses_service_stats_fixture() {
        let stats = parse_service_stats(REDIS_STATS).expect("stats");
        assert_eq!(stats.host_memory_total_kb, Some(8_131_800));
        assert_eq!(stats.memory_current_bytes, Some(9_748_480));
        assert_eq!(stats.memory_limit_bytes, None, "`max` means unlimited");
        assert_eq!(stats.inactive_file_bytes, Some(2_715_648));
        assert_eq!(stats.cpu_delta_usec, Some(5_122));
        assert_eq!(stats.cpu_total_usec, Some(23_949_388_167));
        assert_eq!(stats.elapsed_secs, Some(1.01));
        assert_eq!(stats.cpus, Some(4));
        assert_eq!(stats.data_kb, Some(12));
        assert_eq!(stats.fs_total_kb, Some(162_406_320));
        assert_eq!(stats.memory_used_label(), "6.7 MiB");
    }

    #[test]
    fn service_stats_skips_banner_and_missing_keys() {
        let output = "-----> Filesystem changes may not persist after container restarts\nnot a kv line\nmem_current=1024\nmem_limit=2048\n";
        let stats = parse_service_stats(output).expect("stats");
        assert_eq!(stats.memory_current_bytes, Some(1024));
        assert_eq!(stats.memory_limit_bytes, Some(2048));
        assert_eq!(stats.cpus, None);
        assert_eq!(stats.data_kb, None);
    }

    #[test]
    fn service_stats_empty_values_degrade_to_none() {
        // A cgroup v1 host: the cgroup files are absent, so the script's
        // command substitutions come back empty rather than 0.
        let output = "-----> Filesystem changes may not persist after container restarts\nhost_mem_total_kb=8131800\ncpu_delta_usec=\ncpu_total_usec=\n";
        let stats = parse_service_stats(output).expect("stats");
        assert_eq!(stats.host_memory_total_kb, Some(8_131_800));
        assert_eq!(stats.cpu_delta_usec, None, "not a misleading Some(0)");
        assert_eq!(stats.cpu_total_usec, None);
        assert_eq!(stats.cpu_percent_label(), "—");
        assert_eq!(stats.cpu_total_label(), "—");
    }

    #[test]
    fn service_stats_garbage_is_none() {
        assert_eq!(parse_service_stats(""), None);
        assert_eq!(
            parse_service_stats("-----> Service container is not running\n"),
            None
        );
        assert_eq!(parse_service_stats("mem_current=not-a-number\n"), None);
    }

    #[test]
    fn parses_volume_usage_fixture() {
        let usage = parse_volume_usage(VOLUME_USAGE).expect("usage");
        assert_eq!(usage.used_kb, Some(164));
        assert_eq!(usage.fs_total_kb, Some(162_406_320));
        assert_eq!(usage.fs_used_kb, Some(132_287_668));
        assert_eq!(usage.fs_avail_kb, Some(30_102_164));
    }

    #[test]
    fn volume_usage_skips_docker_pull_noise() {
        let output = "Unable to find image 'alpine:3' locally\n3: Pulling from library/alpine\nDigest: sha256:abc\nStatus: Downloaded newer image for alpine:3\nused_kb=10\nfs_total_kb=100\n";
        let usage = parse_volume_usage(output).expect("usage");
        assert_eq!(usage.used_kb, Some(10));
        assert_eq!(usage.fs_total_kb, Some(100));
        assert_eq!(usage.fs_used_kb, None);
    }

    #[test]
    fn volume_usage_garbage_is_none() {
        assert_eq!(parse_volume_usage(""), None);
        assert_eq!(
            parse_volume_usage("docker: Error response from daemon\n"),
            None
        );
    }

    #[test]
    fn parses_storage_entries_fixture() {
        let entries = parse_storage_entries(LIST_ENTRIES);
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].name, "legacy-151e4f1a23");
        assert_eq!(
            entries[0].host_path,
            "/var/lib/dokku/data/storage/candid/uploads"
        );
        assert_eq!(entries[2].name, "legacy-90db719326");
        assert_eq!(
            entries[2].host_path,
            "/var/lib/dokku/data/services/dokku-ui"
        );
    }

    #[test]
    fn storage_entries_ignores_unknown_fields_and_malformed_json() {
        let entries = parse_storage_entries(
            r#"[{"name":"e1","host_path":"/h","scheduler":"docker-local","schema_version":1}]"#,
        );
        assert_eq!(
            entries,
            vec![StorageEntry {
                name: "e1".into(),
                host_path: "/h".into()
            }]
        );
        assert_eq!(
            parse_storage_entries("not json"),
            Vec::<StorageEntry>::new()
        );
        assert_eq!(parse_storage_entries(""), Vec::<StorageEntry>::new());
        // A row missing a required field fails the whole parse (serde), so
        // the caller falls back to "no entries".
        assert!(parse_storage_entries(r#"[{"name":"e1"}]"#).is_empty());
    }
}
