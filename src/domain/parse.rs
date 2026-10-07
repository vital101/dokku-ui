use crate::domain::mount_spec::MountSpec;
use crate::domain::types::{
    AppInfo, AppMounts, BuildInfo, BuilderReport, ContainerDetails, CronTask, DomainsReport,
    EnvVar, GitReport, HttpAuthReport, ImageStatus, LetsencryptEntry, LogLines, Mount, PortsReport,
    ProcessState, ProcessStatus, ProxyReport, PsReport, ResourceReport, ScaleEntry,
    SchedulerReport, ServiceInfo, ServiceStats, SshKey, SslReport, StorageEntry, VolumeUsage,
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

/// `dokku buildpacks:list <app>` -> buildpack URLs in order. The
/// `-----> <app> buildpack urls` banner and blank lines are skipped.
pub fn parse_buildpacks_list(output: &str) -> Vec<String> {
    output
        .lines()
        .map(strip_ansi)
        .map(|line| line.trim().to_owned())
        .filter(|line| !line.is_empty() && !line.starts_with("----->"))
        .collect()
}

/// `dokku builder:report <app>` -> the selected/computed builder and build
/// dir. Keys are `Builder <name>:` lines; unknown keys are ignored.
pub fn parse_builder_report(output: &str) -> BuilderReport {
    let mut report = BuilderReport::default();
    for line in output.lines() {
        let line = strip_ansi(line);
        let line = line.trim();
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_lowercase();
        let value = normalize_report_value(value);
        match key.as_str() {
            "builder selected" => report.selected = value,
            "builder computed selected" => report.computed_selected = value,
            "builder build dir" => report.build_dir = value,
            "builder detected" => report.detected = value,
            _ => {}
        }
    }
    report
}

/// `dokku git:report <app>` plain text -> typed view. Lines look like
/// `Git computed deploy branch:    main`; the `Git ` prefix and padding are
/// stripped. The sha value is only accepted when it looks like a hex commit
/// (the report prints the literal `HEAD` on unborn refs); `last updated at`
/// is the deploy-branch ref mtime in unix seconds, formatted to UTC.
pub fn parse_git_report(output: &str) -> GitReport {
    let mut report = GitReport::default();
    for line in output.lines() {
        let line = strip_ansi(line);
        let line = line.trim();
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_lowercase();
        let key = key.strip_prefix("git ").unwrap_or(&key).to_owned();
        let value = normalize_report_value(value);
        match key.as_str() {
            "deploy branch" => report.deploy_branch = value,
            "computed deploy branch" => report.computed_deploy_branch = value,
            "sha" => {
                if (7..=64).contains(&value.len()) && value.chars().all(|c| c.is_ascii_hexdigit()) {
                    report.sha = value;
                }
            }
            "source image" => report.source_image = value,
            "last updated at" => report.last_updated_at = format_created_at(&value),
            _ => {}
        }
    }
    report
}

/// `dokku git:public-key` -> the public key line. The host prints a warning
/// block and exits 1 when no deploy key exists, so anything that is not an
/// ssh key line yields `None` (the caller renders the guidance state).
pub fn parse_git_public_key(output: &str) -> Option<String> {
    output
        .lines()
        .map(strip_ansi)
        .map(|line| line.trim().to_owned())
        .find(|line| line.starts_with("ssh-") && line.contains(' '))
}

/// `letsencrypt:list` -> secured apps with expiry info. The output is a
/// fixed-width table under a `-----> App name …` banner; columns are separated
/// by runs of two or more spaces because the expiry and countdown cells
/// themselves contain single spaces.
pub fn parse_letsencrypt_list(output: &str) -> Vec<LetsencryptEntry> {
    output
        .lines()
        .map(strip_ansi)
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with("----->") || line.starts_with("=====>") {
                return None;
            }
            let columns = split_padded_columns(line);
            if columns.len() < 4 {
                return None;
            }
            Some(LetsencryptEntry {
                app: columns[0].to_owned(),
                expires_at: columns[1].to_owned(),
                renews_in: columns[2].to_owned(),
                renewal_in: columns[3].to_owned(),
            })
        })
        .collect()
}

fn split_padded_columns(line: &str) -> Vec<&str> {
    let bytes = line.as_bytes();
    let mut columns = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b' ' && i + 1 < bytes.len() && bytes[i + 1] == b' ' {
            let column = line[start..i].trim();
            if !column.is_empty() {
                columns.push(column);
            }
            while i < bytes.len() && bytes[i] == b' ' {
                i += 1;
            }
            start = i;
        } else {
            i += 1;
        }
    }
    let last = line[start..].trim();
    if !last.is_empty() {
        columns.push(last);
    }
    columns
}

/// `letsencrypt:active <app>` -> whether the app has an active certificate.
/// The plugin prints the literal `true`/`false` and exits 0 either way.
pub fn parse_letsencrypt_active(output: &str) -> bool {
    output.lines().map(str::trim).any(|line| line == "true")
}

/// `certs:report [<app>]` -> the first `=====> <app> ssl information` section
/// (callers pass a single app). Values are prose/plain; malformed output
/// yields `None`.
pub fn parse_certs_report(output: &str) -> Option<SslReport> {
    let mut report: Option<SslReport> = None;
    for line in output.lines() {
        let line = strip_ansi(line);
        let line = line.trim();
        if line.starts_with("=====> ") {
            if report.is_some() {
                break;
            }
            report = Some(SslReport::default());
            continue;
        }
        let Some(current) = report.as_mut() else {
            continue;
        };
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_lowercase();
        let value = normalize_report_value(value);
        match key.strip_prefix("ssl ").unwrap_or("") {
            "enabled" => current.enabled = value == "true",
            "hostnames" => current.hostnames = value,
            "issuer" => current.issuer = value,
            "expires at" => current.expires_at = value,
            "starts at" => current.starts_at = value,
            "subject" => current.subject = value,
            "verified" => current.verified = value,
            _ => {}
        }
    }
    report
}

/// `dokku cron:list <app> --format json` -> scheduled tasks. A malformed
/// response yields an empty list (the caller renders an empty state).
pub fn parse_cron_tasks(json: &str) -> Vec<CronTask> {
    serde_json::from_str::<Vec<CronTask>>(json).unwrap_or_default()
}
/// `ports:report <app>` -> configured and detected mappings from the
/// `Ports map` / `Ports map detected` lines. Malformed output yields defaults.
pub fn parse_ports_report(output: &str) -> PortsReport {
    let mut report = PortsReport::default();
    let mut in_section = false;
    for line in output.lines() {
        let line = strip_ansi(line);
        let line = line.trim();
        if line.starts_with("=====> ") {
            in_section = true;
            continue;
        }
        if !in_section {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        match key.trim().to_lowercase().as_str() {
            "ports map" => report.configured = split_mappings(value),
            "ports map detected" => report.detected = split_mappings(value),
            _ => {}
        }
    }
    report
}

/// `proxy:report <app>` -> proxy status from the `Proxy …` lines.
pub fn parse_proxy_report(output: &str) -> ProxyReport {
    let mut report = ProxyReport::default();
    let mut in_section = false;
    for line in output.lines() {
        let line = strip_ansi(line);
        let line = line.trim();
        if line.starts_with("=====> ") {
            in_section = true;
            continue;
        }
        if !in_section {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = normalize_report_value(value);
        match key.trim().to_lowercase().as_str() {
            "proxy enabled" => report.enabled = value == "true",
            "proxy computed type" => report.computed_type = value,
            "proxy global type" => report.global_type = value,
            "proxy type" => report.proxy_type = value,
            _ => {}
        }
    }
    report
}

fn split_mappings(value: &str) -> Vec<String> {
    value.split_whitespace().map(str::to_owned).collect()
}

/// `ssh-keys:list` -> authorized keys. Text lines look like
/// `SHA256:… NAME="laptop" SSHCOMMAND_ALLOWED_KEYS="no-agent-forwarding,…"`
/// (the installed `sshcommand list` format); the final field is the
/// authorized_keys options list, not the key type.
pub fn parse_ssh_keys(output: &str) -> Vec<SshKey> {
    output
        .lines()
        .map(strip_ansi)
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let mut tokens = line.split_whitespace();
            let fingerprint = tokens.next()?.to_owned();
            let mut key = SshKey {
                fingerprint,
                ..SshKey::default()
            };
            for token in tokens {
                if let Some(value) = token.strip_prefix("NAME=") {
                    key.name = value.trim_matches('"').to_owned();
                } else if let Some(value) = token.strip_prefix("SSHCOMMAND_ALLOWED_KEYS=") {
                    key.options = value.trim_matches('"').to_owned();
                }
            }
            (!key.fingerprint.is_empty()).then_some(key)
        })
        .collect()
}

/// `scheduler:report <app>` -> scheduler selection from the `Scheduler …`
/// lines. Malformed output yields defaults.
pub fn parse_scheduler_report(output: &str) -> SchedulerReport {
    let mut report = SchedulerReport::default();
    let mut in_section = false;
    for line in output.lines() {
        let line = strip_ansi(line);
        let line = line.trim();
        if line.starts_with("=====> ") {
            in_section = true;
            continue;
        }
        if !in_section {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = normalize_report_value(value);
        match key.trim().to_lowercase().as_str() {
            "scheduler selected" => report.selected = value,
            "scheduler computed selected" => report.computed_selected = value,
            "scheduler global selected" => report.global_selected = value,
            _ => {}
        }
    }
    report
}

/// `http-auth:report <app>` -> enabled flag, allowed-IP bypass list, scoped
/// domains, and basic-auth users. Malformed output yields defaults.
pub fn parse_http_auth_report(output: &str) -> HttpAuthReport {
    let mut report = HttpAuthReport::default();
    let mut in_section = false;
    for line in output.lines() {
        let line = strip_ansi(line);
        let line = line.trim();
        if line.starts_with("=====> ") {
            in_section = true;
            continue;
        }
        if !in_section {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = normalize_report_value(value);
        match key.trim().to_lowercase().as_str() {
            "http auth enabled" => report.enabled = value == "true",
            "http auth allowed ips" => report.allowed_ips = split_mappings(&value),
            "http auth domains" => report.domains = split_mappings(&value),
            "http auth users" => report.users = split_mappings(&value),
            _ => {}
        }
    }
    report
}

/// `dokku domains:report <app> --format json` -> the app's vhost hostnames.
pub fn parse_domains_report(json: &str) -> Vec<String> {
    parse_domains_detail(json)
        .map(|report| report.vhosts)
        .unwrap_or_default()
}

/// `dokku domains:report <app> --format json` -> the typed report. Tolerant:
/// a missing/odd value degrades to disabled/empty rather than failing.
pub fn parse_domains_detail(json: &str) -> Option<DomainsReport> {
    let map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(json).ok()?;
    let flag = |key: &str| map.get(key).and_then(|value| value.as_str()) == Some("true");
    let list = |key: &str| {
        map.get(key)
            .and_then(|value| value.as_str())
            .map(|vhosts| vhosts.split_whitespace().map(str::to_owned).collect())
            .unwrap_or_default()
    };
    Some(DomainsReport {
        enabled: flag("app-enabled"),
        vhosts: list("app-vhosts"),
        global_enabled: flag("global-enabled"),
        global_vhosts: list("global-vhosts"),
    })
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
/// empty or `-` value means unset. The DSN is parsed (for the re-auth-gated
/// reveal flow) and masked before any display. An unrecognisable report
/// yields `None`.
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
            "dsn" => info.dsn = (!value.is_empty()).then_some(value),
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

/// `logs:failed <app>` -> the last failed deploy log. The `=====> <app> failed
/// deploy logs` header and dokku `----->` banners are dropped; everything
/// else (including `remote: !` error lines) is kept. No failed deploy ⇒ empty.
pub fn parse_logs_failed(output: &str) -> LogLines {
    LogLines::new(
        output
            .lines()
            .map(strip_ansi)
            .filter(|line| {
                let line = line.trim();
                !line.starts_with("=====>") && !line.starts_with("----->")
            })
            .collect(),
    )
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
    const BUILDER_REPORT: &str = include_str!("../../tests/fixtures/builder_report.txt");
    const GIT_REPORT: &str = include_str!("../../tests/fixtures/git_report.txt");
    const GIT_REPORT_NOT_DEPLOYED: &str =
        include_str!("../../tests/fixtures/git_report_not_deployed.txt");
    const GIT_REPORT_FRESH: &str = include_str!("../../tests/fixtures/git_report_fresh.txt");
    const GIT_REPORT_SYNCED: &str = include_str!("../../tests/fixtures/git_report_synced.txt");
    const GIT_PUBLIC_KEY_MISSING: &str =
        include_str!("../../tests/fixtures/git_public_key_missing.txt");
    const LETSENCRYPT_LIST: &str = include_str!("../../tests/fixtures/letsencrypt_list.txt");
    const LETSENCRYPT_ACTIVE_TRUE: &str =
        include_str!("../../tests/fixtures/letsencrypt_active_true.txt");
    const LETSENCRYPT_ACTIVE_FALSE: &str =
        include_str!("../../tests/fixtures/letsencrypt_active_false.txt");
    const CERTS_REPORT: &str = include_str!("../../tests/fixtures/certs_report.txt");
    const CERTS_REPORT_APP: &str = include_str!("../../tests/fixtures/certs_report_app.txt");
    const CERTS_REPORT_DISABLED: &str =
        include_str!("../../tests/fixtures/certs_report_disabled.txt");
    const LOGS_FAILED: &str = include_str!("../../tests/fixtures/logs_failed.txt");
    const LOGS_FAILED_POPULATED: &str =
        include_str!("../../tests/fixtures/logs_failed_populated.txt");

    #[test]
    fn parses_git_report_fixture() {
        let report = parse_git_report(GIT_REPORT);
        assert_eq!(report.deploy_branch, "main");
        assert_eq!(report.computed_deploy_branch, "main");
        assert_eq!(report.sha, "", "null sha (literal HEAD) is not a commit");
        assert_eq!(report.source_image, "");
        assert_eq!(report.last_updated_at, "2026-10-05 21:18 UTC");
    }

    #[test]
    fn git_report_without_explicit_branch_falls_back_to_computed() {
        let report = parse_git_report(GIT_REPORT_NOT_DEPLOYED);
        assert_eq!(report.deploy_branch, "");
        assert_eq!(report.computed_deploy_branch, "master");
        assert_eq!(report.sha.len(), 40);
    }

    #[test]
    fn git_report_fresh_app_has_no_commit_or_timestamp() {
        let report = parse_git_report(GIT_REPORT_FRESH);
        assert_eq!(report.deploy_branch, "");
        assert_eq!(report.computed_deploy_branch, "master");
        assert_eq!(report.sha, "", "the report prints HEAD for unborn refs");
        assert_eq!(report.last_updated_at, "");
    }

    #[test]
    fn git_report_after_sync_has_real_sha() {
        let report = parse_git_report(GIT_REPORT_SYNCED);
        assert_eq!(report.deploy_branch, "master");
        assert_eq!(report.sha, "7fd1a60b01f91b314f59955a4e4d4e80d8edf11d");
        assert!(!report.last_updated_at.is_empty());
    }

    #[test]
    fn git_report_tolerates_missing_or_malformed_output() {
        let report = parse_git_report("");
        assert_eq!(report, GitReport::default());
        let report = parse_git_report("=====> alpha git information\nGit sha: deadbeef\n");
        assert_eq!(report.sha, "deadbeef", "short shas are kept");
        let report = parse_git_report("Git sha: not a sha\n");
        assert_eq!(report.sha, "");
    }

    #[test]
    fn git_public_key_parses_key_line_only() {
        assert_eq!(
            parse_git_public_key("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI test@host\n"),
            Some("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI test@host".to_owned())
        );
        assert_eq!(
            parse_git_public_key(GIT_PUBLIC_KEY_MISSING),
            None,
            "warning block is not a key"
        );
        assert_eq!(parse_git_public_key(""), None);
    }

    #[test]
    fn parses_letsencrypt_list_fixture() {
        let entries = parse_letsencrypt_list(LETSENCRYPT_LIST);
        assert_eq!(entries.len(), 10, "the banner is skipped, every row parsed");
        assert_eq!(entries[0].app, "oneiric");
        assert_eq!(entries[0].expires_at, "2026-11-18 05:25:41");
        assert_eq!(entries[0].renews_in, "42d, 19h, 3m, 14s");
        assert_eq!(entries[0].renewal_in, "12d, 19h, 3m, 14s");
        assert!(
            entries.iter().any(|entry| entry.app == "dokku-ui"),
            "dokku-ui row present"
        );
    }

    #[test]
    fn letsencrypt_list_ignores_banners_and_junk() {
        assert_eq!(parse_letsencrypt_list(""), Vec::new());
        assert_eq!(
            parse_letsencrypt_list("-----> App name           Certificate Expiry\n"),
            Vec::new()
        );
        assert_eq!(
            parse_letsencrypt_list("a-b  2026-01-01 00:00:00  1d, 0h, 0m, 0s  0d, 0h, 0m, 0s\n")
                .len(),
            1
        );
    }

    #[test]
    fn parses_letsencrypt_active_fixtures() {
        assert!(parse_letsencrypt_active(LETSENCRYPT_ACTIVE_TRUE));
        assert!(!parse_letsencrypt_active(LETSENCRYPT_ACTIVE_FALSE));
        assert!(!parse_letsencrypt_active(""));
        assert!(!parse_letsencrypt_active("not true\n"));
    }

    #[test]
    fn parses_certs_report_sections() {
        let report = parse_certs_report(CERTS_REPORT_APP).expect("section");
        assert!(report.enabled);
        assert_eq!(report.hostnames, "dokku.re-cycledair.com");
        assert_eq!(report.expires_at, "Jan  1 11:30:37 2027 GMT");
        assert!(report.issuer.contains("Let's Encrypt"));
        assert_eq!(report.subject, "subject=CN = dokku.re-cycledair.com");
        assert!(report.verified_by_ca());

        let first = parse_certs_report(CERTS_REPORT).expect("first section");
        assert_eq!(
            first.hostnames, "alethos.io www.alethos.io",
            "the all-apps report yields the first section"
        );
    }

    #[test]
    fn certs_report_disabled_app_has_empty_fields() {
        let report = parse_certs_report(CERTS_REPORT_DISABLED).expect("section");
        assert!(!report.enabled);
        assert!(report.hostnames.is_empty());
        assert!(report.expires_at.is_empty());
        assert!(!report.verified_by_ca());
        assert_eq!(parse_certs_report(""), None);
    }

    const PORTS_REPORT: &str = include_str!("../../tests/fixtures/ports_report.txt");
    const PROXY_REPORT: &str = include_str!("../../tests/fixtures/proxy_report.txt");
    const SCHEDULER_REPORT: &str = include_str!("../../tests/fixtures/scheduler_report.txt");

    #[test]
    fn parses_ports_and_proxy_reports() {
        let ports = parse_ports_report(PORTS_REPORT);
        assert_eq!(ports.configured, vec!["http:80:5000"]);
        assert_eq!(ports.detected, vec!["http:80:5000"]);
        assert_eq!(parse_ports_report(""), PortsReport::default());

        let proxy = parse_proxy_report(PROXY_REPORT);
        assert!(proxy.enabled);
        assert_eq!(proxy.computed_type, "nginx");
        assert_eq!(proxy.global_type, "nginx");
        assert!(proxy.proxy_type.is_empty());
        assert_eq!(proxy.effective_type(), "nginx");
        assert_eq!(parse_proxy_report(""), ProxyReport::default());
    }

    #[test]
    fn parses_scheduler_report() {
        let report = parse_scheduler_report(SCHEDULER_REPORT);
        assert!(report.selected.is_empty());
        assert_eq!(report.computed_selected, "docker-local");
        assert_eq!(report.global_selected, "docker-local");
        assert_eq!(parse_scheduler_report(""), SchedulerReport::default());
    }

    const HTTP_AUTH_REPORT: &str = include_str!("../../tests/fixtures/http_auth_report.txt");

    #[test]
    fn parses_http_auth_report() {
        let report = parse_http_auth_report(HTTP_AUTH_REPORT);
        assert!(report.enabled);
        assert_eq!(report.allowed_ips, vec!["10.0.0.0/8", "192.168.1.5"]);
        assert!(report.domains.is_empty());
        assert_eq!(report.users, vec!["admin"]);
        assert_eq!(parse_http_auth_report(""), HttpAuthReport::default());
    }

    #[test]
    fn parses_ssh_keys_text_format() {
        let output = "SHA256:abc NAME=\"laptop\" SSHCOMMAND_ALLOWED_KEYS=\"no-agent-forwarding,no-user-rc\"\nSHA256:def NAME=\"deploy@ci\" SSHCOMMAND_ALLOWED_KEYS=\"no-port-forwarding\"\n";
        let keys = parse_ssh_keys(output);
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0].fingerprint, "SHA256:abc");
        assert_eq!(keys[0].name, "laptop");
        assert_eq!(keys[0].options, "no-agent-forwarding,no-user-rc");
        assert_eq!(keys[1].name, "deploy@ci");
        assert_eq!(keys[1].options, "no-port-forwarding");
        assert!(parse_ssh_keys("").is_empty());
    }

    #[test]
    fn logs_failed_drops_the_header_and_keeps_the_log() {
        assert_eq!(
            parse_logs_failed(LOGS_FAILED),
            LogLines::new(Vec::new()),
            "the empty capture is header-only"
        );
        let lines = parse_logs_failed(LOGS_FAILED_POPULATED);
        assert!(
            lines
                .as_slice()
                .iter()
                .any(|line| line.contains("pre-receive hook declined")),
            "remote error lines survive"
        );
        assert!(
            lines
                .as_slice()
                .iter()
                .all(|line| !line.contains("failed deploy logs")),
            "the dokku header is dropped"
        );
        assert!(
            lines
                .as_slice()
                .iter()
                .any(|line| line.starts_with("remote:  !")),
            "remote warning lines are content, not banners"
        );
    }

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
    fn parses_buildpacks_list() {
        let output = "-----> alpha buildpack urls\nhttps://github.com/heroku/heroku-buildpack-nodejs\nhttps://github.com/heroku/heroku-buildpack-ruby\n";
        assert_eq!(
            parse_buildpacks_list(output),
            vec![
                "https://github.com/heroku/heroku-buildpack-nodejs",
                "https://github.com/heroku/heroku-buildpack-ruby",
            ]
        );
        assert_eq!(
            parse_buildpacks_list("-----> alpha buildpack urls\n"),
            Vec::<String>::new()
        );
        assert_eq!(parse_buildpacks_list(""), Vec::<String>::new());
    }

    #[test]
    fn parses_builder_report_fixture() {
        let report = parse_builder_report(BUILDER_REPORT);
        assert_eq!(report.selected, "");
        assert_eq!(report.build_dir, "");
        let populated = "=====> alpha builder information\n       Builder build dir:             /app\n       Builder computed selected:     dockerfile\n       Builder detected:              dockerfile\n       Builder selected:              dockerfile\n";
        let report = parse_builder_report(populated);
        assert_eq!(report.selected, "dockerfile");
        assert_eq!(report.computed_selected, "dockerfile");
        assert_eq!(report.build_dir, "/app");
        assert_eq!(report.detected, "dockerfile");
    }

    #[test]
    fn parses_cron_tasks_from_json() {
        let json = r#"[
            {"id":"a1b2c3","schedule":"0 * * * *","command":"echo hi","concurrency_policy":"allow","maintenance":false,"task-in-maintenance":false},
            {"id":"d4e5f6","schedule":"30 2 * * *","command":"backup","concurrency_policy":"forbid","maintenance":true,"task-in-maintenance":true}
        ]"#;
        let tasks = parse_cron_tasks(json);
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].id, "a1b2c3");
        assert_eq!(tasks[0].schedule, "0 * * * *");
        assert_eq!(tasks[0].command, "echo hi");
        assert_eq!(tasks[0].concurrency_policy, "allow");
        assert!(!tasks[0].task_in_maintenance);
        assert!(tasks[1].maintenance);
        assert!(tasks[1].task_in_maintenance);
    }

    #[test]
    fn cron_tasks_empty_and_malformed_are_empty() {
        assert!(parse_cron_tasks("[]").is_empty());
        assert!(parse_cron_tasks("not json").is_empty());
        assert!(parse_cron_tasks("").is_empty());
    }

    #[test]
    fn parses_domains_detail_fixture() {
        let report = parse_domains_detail(DOMAINS_REPORT).expect("report");
        assert!(report.enabled);
        assert_eq!(report.vhosts, vec!["dokku.re-cycledair.com"]);
        assert!(report.global_enabled);
        assert_eq!(report.global_vhosts, vec!["re-cycledair.com"]);
    }

    #[test]
    fn domains_detail_tolerates_missing_fields() {
        let report = parse_domains_detail(r#"{"app-vhosts":"one.example.com"}"#).expect("report");
        assert!(!report.enabled);
        assert_eq!(report.vhosts, vec!["one.example.com"]);
        assert!(report.global_vhosts.is_empty());
        assert_eq!(parse_domains_detail("not json"), None);
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
    fn service_info_parses_the_dsn_but_masks_it_by_default() {
        let output = "=====> x redis service information\n       Dsn:                 redis://:secret@host:6379\n       Status:              running\n";
        let info = parse_service_info(output, "redis", "x").expect("info");
        assert_eq!(info.status, "running");
        assert_eq!(
            info.dsn.as_deref(),
            Some("redis://:secret@host:6379"),
            "the DSN is parsed for the reveal flow"
        );
        let masked = info.masked_dsn().expect("masked");
        assert!(!masked.contains("secret"), "{masked}");
        assert!(masked.contains("@host:6379"), "{masked}");
        let debug = format!("{info:?}");
        assert!(!debug.contains("secret"), "Debug masks the DSN: {debug}");
        let json = serde_json::to_string(&info).expect("serialize");
        assert!(
            !json.contains("secret") && !json.contains("dsn"),
            "serialization omits the DSN: {json}"
        );
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
