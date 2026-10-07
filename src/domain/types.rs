use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PsReport {
    pub deployed: bool,
    pub running: bool,
    pub process_count: i64,
    pub processes: Vec<ProcessStatus>,
    pub can_scale: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessStatus {
    pub process_type: String,
    pub state: ProcessState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProcessState {
    Running,
    Stopped,
    Missing,
    Other(String),
}

impl ProcessState {
    pub fn parse(raw: &str) -> Self {
        let state = raw.split_whitespace().next().unwrap_or("");
        match state {
            "running" | "restarting" => ProcessState::Running,
            "exited" | "stopped" | "dead" | "created" | "paused" => ProcessState::Stopped,
            "missing" => ProcessState::Missing,
            other => ProcessState::Other(other.to_owned()),
        }
    }
}

/// `builder:report <app>` as a typed view (empty strings mean unset).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BuilderReport {
    pub selected: String,
    pub computed_selected: String,
    pub build_dir: String,
    pub detected: String,
}

/// `git:report <app>` as a typed view. `deploy_branch` is the explicitly-set
/// property (empty = unset); `computed_deploy_branch` is the effective branch
/// after the global fallback. `sha` is only populated for commit-hash-shaped
/// values — the report prints the literal `HEAD` on unborn refs. Empty strings
/// mean unavailable/unset.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GitReport {
    pub deploy_branch: String,
    pub computed_deploy_branch: String,
    pub sha: String,
    pub source_image: String,
    pub last_updated_at: String,
}

/// One row of `letsencrypt:list` (dokku-letsencrypt 0.20.4 fixed-width
/// table): app, absolute expiry, and the printed countdowns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LetsencryptEntry {
    pub app: String,
    /// Absolute expiry `YYYY-MM-DD HH:MM:SS` as reported by the host.
    pub expires_at: String,
    /// Time before expiry, e.g. `42d, 19h, 3m, 14s`.
    pub renews_in: String,
    /// Time before the auto-renewal window opens.
    pub renewal_in: String,
}

/// The `Ssl …` section of `certs:report` (plain text only on 0.38.4).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SslReport {
    pub enabled: bool,
    pub hostnames: String,
    pub issuer: String,
    pub expires_at: String,
    pub starts_at: String,
    pub subject: String,
    pub verified: String,
}

impl SslReport {
    /// Whether the certificate chain verified against a CA (the report prints
    /// prose rather than a boolean).
    pub fn verified_by_ca(&self) -> bool {
        self.verified.contains("verified by")
    }
}

/// One authorized key from `ssh-keys:list` (text format). The text output
/// carries the authorized_keys options (`SSHCOMMAND_ALLOWED_KEYS`), not the
/// key type — the type only exists inside the public key itself.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SshKey {
    pub fingerprint: String,
    pub name: String,
    /// Comma-separated sshd options the key is restricted to
    /// (`no-agent-forwarding,no-user-rc,…`).
    pub options: String,
}

/// The `Http auth …` section of `http-auth:report` (plain text).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HttpAuthReport {
    pub enabled: bool,
    pub allowed_ips: Vec<String>,
    pub domains: Vec<String>,
    pub users: Vec<String>,
}

/// The `Scheduler …` section of `scheduler:report` (plain text on 0.38.4).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SchedulerReport {
    pub selected: String,
    pub computed_selected: String,
    pub global_selected: String,
}

/// The `Ports map` / `Ports map detected` sections of `ports:report`
/// (plain text on 0.38.4).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PortsReport {
    pub configured: Vec<String>,
    pub detected: Vec<String>,
}

/// The proxy section of `proxy:report` (plain text on 0.38.4).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProxyReport {
    pub enabled: bool,
    pub computed_type: String,
    pub global_type: String,
    pub proxy_type: String,
}

impl ProxyReport {
    /// The type actually in effect: the app's selection or the global one.
    pub fn effective_type(&self) -> &str {
        if self.computed_type.is_empty() {
            if self.proxy_type.is_empty() {
                &self.global_type
            } else {
                &self.proxy_type
            }
        } else {
            &self.computed_type
        }
    }
}

/// One scheduled cron task from `cron:list --format json` (dokku v0.38.4
/// `CronTask` struct). `task_in_maintenance` is the suspend state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CronTask {
    pub id: String,
    pub schedule: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub concurrency_policy: String,
    #[serde(default)]
    pub maintenance: bool,
    #[serde(default, rename = "task-in-maintenance")]
    pub task_in_maintenance: bool,
}

/// `domains:report --format json` as a typed view: whether vhost support is
/// enabled and the app's own vs. global vhost lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainsReport {
    pub enabled: bool,
    pub vhosts: Vec<String>,
    pub global_enabled: bool,
    pub global_vhosts: Vec<String>,
}

/// One process type's desired instance count, from `ps:scale --format json`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ScaleEntry {
    pub process_type: String,
    pub quantity: u32,
}

impl ScaleEntry {
    pub fn new(process_type: impl Into<String>, quantity: u32) -> Self {
        Self {
            process_type: process_type.into(),
            quantity,
        }
    }

    /// `<process_type>=<quantity>`, the argument `ps:scale` accepts.
    pub fn arg(&self) -> String {
        format!("{}={}", self.process_type, self.quantity)
    }
}

/// Per-container runtime state, distilled from `ps:inspect` (sanitized docker inspect).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContainerDetails {
    pub id_short: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub started_at: String,
    pub restart_count: i64,
    pub oom_killed: bool,
    pub exit_code: i64,
}

/// Per-process-type resource limits/reservations from `resource:report`. Empty
/// strings mean "not set" and render as an em dash.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResourceReport {
    pub process_type: String,
    pub limit_cpu: String,
    pub limit_memory: String,
    pub limit_memory_swap: String,
    pub reserve_cpu: String,
    pub reserve_memory: String,
}

/// A service (datastore) linked to an app.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ServiceLink {
    pub plugin: String,
    pub service: String,
}

/// Details for a single linked service, parsed from the plain-text
/// `<plugin>:info <service>` report. The DSN is parsed but never rendered
/// unmasked except through the re-authenticated reveal flow. `Debug` redacts
/// the DSN and serialization omits it entirely, so neither a stray log line
/// nor a serialized copy can leak credentials.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ServiceInfo {
    pub plugin: String,
    pub service: String,
    pub status: String,
    pub version: String,
    pub exposed_ports: String,
    pub internal_ip: String,
    pub id_short: String,
    pub linked_apps: Vec<String>,
    /// The connection string including credentials; mask before display.
    #[serde(default, skip_serializing)]
    pub dsn: Option<String>,
}

impl std::fmt::Debug for ServiceInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceInfo")
            .field("plugin", &self.plugin)
            .field("service", &self.service)
            .field("status", &self.status)
            .field("version", &self.version)
            .field("exposed_ports", &self.exposed_ports)
            .field("internal_ip", &self.internal_ip)
            .field("id_short", &self.id_short)
            .field("linked_apps", &self.linked_apps)
            .field("dsn", &self.masked_dsn())
            .finish()
    }
}

impl ServiceInfo {
    pub fn unknown(plugin: impl Into<String>, service: impl Into<String>) -> Self {
        Self {
            plugin: plugin.into(),
            service: service.into(),
            status: String::new(),
            version: String::new(),
            exposed_ports: String::new(),
            internal_ip: String::new(),
            id_short: String::new(),
            linked_apps: Vec::new(),
            dsn: None,
        }
    }

    /// The connection string with credentials replaced by the fixed mask
    /// (`redis://••••••••@host:6379`), safe for default rendering.
    pub fn masked_dsn(&self) -> Option<String> {
        self.dsn
            .as_deref()
            .map(|dsn| crate::domain::redact::redact_line(dsn, &[]))
    }

    pub fn status_label(&self) -> &str {
        if self.status.is_empty() {
            "unknown"
        } else {
            &self.status
        }
    }

    pub fn status_badge_css(&self) -> &'static str {
        match self.status.as_str() {
            "running" => "bg-emerald-500/10 text-emerald-400",
            "" => "bg-slate-500/10 text-slate-400",
            _ => "bg-red-500/10 text-red-400",
        }
    }

    pub fn display_or_dash(value: &str) -> &str {
        if value.is_empty() { "—" } else { value }
    }

    pub fn linked_apps_label(&self) -> String {
        if self.linked_apps.is_empty() {
            "—".to_owned()
        } else {
            self.linked_apps.join(", ")
        }
    }
}

/// One bind mount from `storage:report`, keyed by its `host:container`
/// locator so the same mount reported under several phases collapses into a
/// single row. Empty options render as an em dash.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Mount {
    pub host: String,
    pub container: String,
    pub options: String,
    pub phases: Vec<String>,
}

impl Mount {
    pub fn locator(&self) -> String {
        format!("{}:{}", self.host, self.container)
    }

    pub fn arg(&self) -> String {
        if self.options.is_empty() {
            self.locator()
        } else {
            format!("{}:{}", self.locator(), self.options)
        }
    }

    pub fn phases_label(&self) -> String {
        if self.phases.is_empty() {
            "—".to_owned()
        } else {
            self.phases.join(", ")
        }
    }
}

/// Every mount an app declares, from the all-apps `storage:report` output.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AppMounts {
    pub app: String,
    pub mounts: Vec<Mount>,
}

impl AppMounts {
    pub fn is_empty(&self) -> bool {
        self.mounts.is_empty()
    }

    pub fn mount_count(&self) -> usize {
        self.mounts.len()
    }
}

/// Human-readable binary byte size for stat labels.
pub fn format_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    const TIB: u64 = GIB * 1024;
    if bytes >= TIB {
        format!("{:.1} TiB", bytes as f64 / TIB as f64)
    } else if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{} KiB", bytes / KIB)
    } else {
        format!("{bytes} B")
    }
}

/// Compact duration for cumulative CPU-time labels: `6h 39m`, `5m 12s`, `12s`.
pub fn format_duration(secs: u64) -> String {
    const DAY: u64 = 86_400;
    const HOUR: u64 = 3_600;
    const MINUTE: u64 = 60;
    if secs >= DAY {
        format!("{}d {}h", secs / DAY, (secs % DAY) / HOUR)
    } else if secs >= HOUR {
        format!("{}h {}m", secs / HOUR, (secs % HOUR) / MINUTE)
    } else if secs >= MINUTE {
        format!("{}m {}s", secs / MINUTE, secs % MINUTE)
    } else {
        format!("{secs}s")
    }
}

/// Live resource usage for a service container, parsed from the fixed
/// `<plugin>:enter` stats script. Every field is optional: missing cgroup
/// values (older kernels, stopped containers) render as an em dash.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ServiceStats {
    pub host_memory_total_kb: Option<u64>,
    pub host_memory_available_kb: Option<u64>,
    pub memory_current_bytes: Option<u64>,
    /// `None` means the container has no memory limit (`memory.max` = `max`).
    pub memory_limit_bytes: Option<u64>,
    pub inactive_file_bytes: Option<u64>,
    pub cpu_delta_usec: Option<u64>,
    pub cpu_total_usec: Option<u64>,
    pub elapsed_secs: Option<f64>,
    pub cpus: Option<u64>,
    pub data_kb: Option<u64>,
    pub fs_total_kb: Option<u64>,
    pub fs_used_kb: Option<u64>,
    pub fs_avail_kb: Option<u64>,
}

impl ServiceStats {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// docker-stats parity: current memory minus reclaimable page cache.
    pub fn memory_working_set_bytes(&self) -> Option<u64> {
        let current = self.memory_current_bytes?;
        Some(current.saturating_sub(self.inactive_file_bytes.unwrap_or(0)))
    }

    /// The percentage base: the container limit, or the host total when the
    /// container is unlimited.
    pub fn memory_base_bytes(&self) -> Option<u64> {
        self.memory_limit_bytes
            .or_else(|| self.host_memory_total_kb.map(|kb| kb * 1024))
    }

    pub fn memory_percent(&self) -> Option<f64> {
        let used = self.memory_working_set_bytes()?;
        let base = self.memory_base_bytes()?;
        (base > 0).then(|| used as f64 / base as f64 * 100.0)
    }

    pub fn memory_used_label(&self) -> String {
        self.memory_working_set_bytes()
            .map(format_bytes)
            .unwrap_or_else(|| "—".to_owned())
    }

    /// "of 2.0 GiB" for a limited container; "of 7.8 GiB host (3.7 GiB
    /// available)" when unlimited — the parenthetical shows how much memory
    /// the host still has free.
    pub fn memory_detail_label(&self) -> String {
        match self.memory_limit_bytes {
            Some(bytes) => format!("of {}", format_bytes(bytes)),
            None => self
                .host_memory_total_kb
                .map(|kb| {
                    let available = self
                        .host_memory_available_kb
                        .map(|avail| format!(" ({} available)", format_bytes(avail * 1024)))
                        .unwrap_or_default();
                    format!("of {} host{available}", format_bytes(kb * 1024))
                })
                .unwrap_or_else(|| "—".to_owned()),
        }
    }

    pub fn memory_percent_label(&self) -> String {
        percent_label(self.memory_percent())
    }

    /// Rounded percentage for `<progress>` values.
    pub fn memory_percent_rounded(&self) -> Option<u32> {
        percent_rounded(self.memory_percent())
    }

    /// Sampled CPU use as a percentage of all cores.
    pub fn cpu_percent(&self) -> Option<f64> {
        let delta = self.cpu_delta_usec? as f64;
        let elapsed = self.elapsed_secs?;
        let cpus = self.cpus? as f64;
        (elapsed > 0.0 && cpus > 0.0).then(|| delta / (elapsed * 1_000_000.0 * cpus) * 100.0)
    }

    pub fn cpu_percent_label(&self) -> String {
        percent_label(self.cpu_percent())
    }

    /// Cumulative CPU time consumed since the container started.
    pub fn cpu_total_secs(&self) -> Option<u64> {
        self.cpu_total_usec.map(|usec| usec / 1_000_000)
    }

    pub fn cpu_total_label(&self) -> String {
        self.cpu_total_secs()
            .map(format_duration)
            .unwrap_or_else(|| "—".to_owned())
    }

    pub fn data_label(&self) -> String {
        self.data_kb
            .map(|kb| format_bytes(kb * 1024))
            .unwrap_or_else(|| "—".to_owned())
    }

    pub fn fs_percent(&self) -> Option<f64> {
        let used = self.fs_used_kb? as f64;
        let total = self.fs_total_kb? as f64;
        (total > 0.0).then(|| used / total * 100.0)
    }

    pub fn fs_used_label(&self) -> String {
        self.fs_used_kb
            .map(|kb| format_bytes(kb * 1024))
            .unwrap_or_else(|| "—".to_owned())
    }

    pub fn fs_total_label(&self) -> String {
        self.fs_total_kb
            .map(|kb| format_bytes(kb * 1024))
            .unwrap_or_else(|| "—".to_owned())
    }

    pub fn fs_avail_label(&self) -> String {
        self.fs_avail_kb
            .map(|kb| format_bytes(kb * 1024))
            .unwrap_or_else(|| "—".to_owned())
    }

    pub fn fs_percent_label(&self) -> String {
        percent_label(self.fs_percent())
    }

    /// Rounded percentage for `<progress>` values.
    pub fn fs_percent_rounded(&self) -> Option<u32> {
        percent_rounded(self.fs_percent())
    }
}

/// Disk usage for one storage entry, from the fixed `storage:exec` script
/// (`/data` is the entry's mount point inside the throwaway container).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct VolumeUsage {
    pub used_kb: Option<u64>,
    pub fs_total_kb: Option<u64>,
    pub fs_used_kb: Option<u64>,
    pub fs_avail_kb: Option<u64>,
}

impl VolumeUsage {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn used_label(&self) -> String {
        self.used_kb
            .map(|kb| format_bytes(kb * 1024))
            .unwrap_or_else(|| "—".to_owned())
    }

    pub fn fs_percent(&self) -> Option<f64> {
        let used = self.fs_used_kb? as f64;
        let total = self.fs_total_kb? as f64;
        (total > 0.0).then(|| used / total * 100.0)
    }

    pub fn fs_percent_label(&self) -> String {
        percent_label(self.fs_percent())
    }

    /// Rounded percentage for `<progress>` values.
    pub fn fs_percent_rounded(&self) -> Option<u32> {
        percent_rounded(self.fs_percent())
    }

    /// "82% of 154.9 GiB used" — the tooltip for a row's usage figure.
    pub fn fs_label(&self) -> String {
        match (self.fs_percent(), self.fs_total_kb) {
            (Some(percent), Some(total)) => {
                format!("{percent:.0}% of {} used", format_bytes(total * 1024))
            }
            _ => "—".to_owned(),
        }
    }

    pub fn fs_used_label(&self) -> String {
        self.fs_used_kb
            .map(|kb| format_bytes(kb * 1024))
            .unwrap_or_else(|| "—".to_owned())
    }

    pub fn fs_total_label(&self) -> String {
        self.fs_total_kb
            .map(|kb| format_bytes(kb * 1024))
            .unwrap_or_else(|| "—".to_owned())
    }

    pub fn fs_avail_label(&self) -> String {
        self.fs_avail_kb
            .map(|kb| format_bytes(kb * 1024))
            .unwrap_or_else(|| "—".to_owned())
    }
}

fn percent_label(percent: Option<f64>) -> String {
    percent
        .map(|value| format!("{value:.1}%"))
        .unwrap_or_else(|| "—".to_owned())
}

fn percent_rounded(percent: Option<f64>) -> Option<u32> {
    percent.map(|value| value.round().clamp(0.0, 100.0) as u32)
}

/// A registered storage entry, from `storage:list-entries --format json`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StorageEntry {
    pub name: String,
    pub host_path: String,
}

/// Summary of the most recent build for an app, from `builds:report --format json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildInfo {
    pub status: String,
    pub exit_code: String,
    pub kind: String,
    pub started_at: String,
    pub finished_at: String,
}

impl BuildInfo {
    pub fn image_status(&self) -> ImageStatus {
        match self.status.as_str() {
            "" => ImageStatus::None,
            "failed" => {
                let detail = if self.exit_code.is_empty() {
                    "build failed".to_owned()
                } else {
                    format!("build failed (exit {})", self.exit_code)
                };
                ImageStatus::Error(detail)
            }
            _ => ImageStatus::Built,
        }
    }

    pub fn is_failed(&self) -> bool {
        self.status == "failed"
    }

    pub fn status_label(&self) -> &str {
        match self.status.as_str() {
            "" => "never built",
            other => other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EnvVar {
    pub key: String,
    pub value: String,
}

impl EnvVar {
    /// Fixed-length mask for display; leaks nothing about the value or its length.
    pub fn masked_value(&self) -> &'static str {
        if self.value.is_empty() {
            ""
        } else {
            "••••••••"
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AppStats {
    pub total: usize,
    pub running: usize,
    pub stopped: usize,
}

impl AppStats {
    pub fn from_reports(reports: &[PsReport]) -> Self {
        reports.iter().fold(Self::default(), |mut stats, report| {
            stats.total += 1;
            if report.running {
                stats.running += 1;
            } else {
                stats.stopped += 1;
            }
            stats
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppHealth {
    Running,
    Stopped,
    NotDeployed,
    Unknown,
}

impl AppHealth {
    pub fn from_report(report: Option<&PsReport>) -> Self {
        match report {
            None => AppHealth::Unknown,
            Some(report) if report.running => AppHealth::Running,
            Some(report) if report.deployed => AppHealth::Stopped,
            Some(_) => AppHealth::NotDeployed,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AppHealth::Running => "Running",
            AppHealth::Stopped => "Stopped",
            AppHealth::NotDeployed => "Not deployed",
            AppHealth::Unknown => "Unknown",
        }
    }

    pub fn badge_css(self) -> &'static str {
        match self {
            AppHealth::Running => "bg-emerald-500/10 text-emerald-400",
            AppHealth::Stopped => "bg-red-500/10 text-red-400",
            AppHealth::NotDeployed => "bg-slate-500/10 text-slate-400",
            AppHealth::Unknown => "bg-slate-500/10 text-slate-400",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppInfo {
    pub name: String,
    pub created_at: String,
    pub locked: bool,
    pub image_status: Option<ImageStatus>,
    pub last_build: Option<BuildInfo>,
    pub links: Option<Vec<ServiceLink>>,
    pub domains: Vec<String>,
    pub dns_record_exists: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageStatus {
    None,
    Built,
    Error(String),
}

impl AppInfo {
    pub fn image_status_label(&self) -> &'static str {
        match &self.image_status {
            None => "unknown",
            Some(ImageStatus::None) => "none",
            Some(ImageStatus::Built) => "built",
            Some(ImageStatus::Error(_)) => "error",
        }
    }

    pub fn link_exists_label(&self) -> &'static str {
        match &self.links {
            None => "unknown",
            Some(links) if links.is_empty() => "no",
            Some(_) => "yes",
        }
    }

    pub fn domains_label(&self) -> String {
        if self.domains.is_empty() {
            "—".to_owned()
        } else {
            self.domains.join(", ")
        }
    }

    pub fn last_build_label(&self) -> String {
        match &self.last_build {
            None => "unknown".to_owned(),
            Some(build) => build.status_label().to_owned(),
        }
    }

    pub fn dns_record_exists_label(&self) -> &'static str {
        match self.dns_record_exists {
            None => "unknown",
            Some(true) => "yes",
            Some(false) => "no",
        }
    }

    pub fn locked_label(&self) -> &'static str {
        match self.locked {
            true => "yes",
            false => "no",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppOverview {
    pub name: String,
    pub app_info: Option<AppInfo>,
    pub ps_report: Option<PsReport>,
    pub health: AppHealth,
    pub process_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LogLines(Vec<String>);

impl LogLines {
    pub fn new(lines: Vec<String>) -> Self {
        Self(lines)
    }

    pub fn as_slice(&self) -> &[String] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Keeps only the last `max` lines. Service logs have no line-count flag on
    /// the installed dokku plugins, so the display side re-bounds whatever the
    /// container produced.
    pub fn tail(&self, max: usize) -> Self {
        if self.0.len() <= max {
            return self.clone();
        }
        Self(self.0[self.0.len() - max..].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(running: bool, deployed: bool) -> PsReport {
        PsReport {
            deployed,
            running,
            process_count: 1,
            processes: Vec::new(),
            can_scale: None,
        }
    }

    #[test]
    fn process_state_parse_maps_known_states() {
        assert_eq!(
            ProcessState::parse("running (CID: abc)"),
            ProcessState::Running
        );
        assert_eq!(ProcessState::parse("restarting"), ProcessState::Running);
        assert_eq!(
            ProcessState::parse("exited (CID: abc)"),
            ProcessState::Stopped
        );
        assert_eq!(ProcessState::parse("stopped"), ProcessState::Stopped);
        assert_eq!(ProcessState::parse("dead"), ProcessState::Stopped);
        assert_eq!(ProcessState::parse("created"), ProcessState::Stopped);
        assert_eq!(ProcessState::parse("paused"), ProcessState::Stopped);
        assert_eq!(ProcessState::parse("missing"), ProcessState::Missing);
        assert_eq!(
            ProcessState::parse("impossible-state"),
            ProcessState::Other("impossible-state".into())
        );
        assert_eq!(ProcessState::parse(""), ProcessState::Other(String::new()));
    }

    #[test]
    fn app_stats_folds_reports() {
        let reports = vec![
            report(true, true),
            report(true, true),
            report(false, true),
            report(false, false),
        ];
        assert_eq!(
            AppStats::from_reports(&reports),
            AppStats {
                total: 4,
                running: 2,
                stopped: 2,
            }
        );
    }

    #[test]
    fn app_stats_is_empty_for_no_reports() {
        assert_eq!(AppStats::from_reports(&[]), AppStats::default());
    }

    #[test]
    fn app_health_classifies_running_reports() {
        assert_eq!(
            AppHealth::from_report(Some(&report(true, true))),
            AppHealth::Running
        );
        assert_eq!(
            AppHealth::from_report(Some(&report(true, false))),
            AppHealth::Running
        );
    }

    #[test]
    fn app_health_classifies_stopped_and_not_deployed() {
        assert_eq!(
            AppHealth::from_report(Some(&report(false, true))),
            AppHealth::Stopped
        );
        assert_eq!(
            AppHealth::from_report(Some(&report(false, false))),
            AppHealth::NotDeployed
        );
    }

    #[test]
    fn app_health_missing_report_is_unknown() {
        assert_eq!(AppHealth::from_report(None), AppHealth::Unknown);
    }

    #[test]
    fn log_lines_accessors() {
        let lines = LogLines::new(vec!["a".into(), "b".into()]);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines.as_slice(), &["a", "b"]);
        assert!(!lines.is_empty());
        assert!(LogLines::new(Vec::new()).is_empty());
    }

    #[test]
    fn env_var_masked_value_is_fixed_length() {
        assert_eq!(
            EnvVar {
                key: "K".into(),
                value: "short".into()
            }
            .masked_value(),
            "••••••••"
        );
        assert_eq!(
            EnvVar {
                key: "K".into(),
                value: "a-much-longer-secret-value".into()
            }
            .masked_value(),
            "••••••••"
        );
    }

    #[test]
    fn env_var_masked_value_empty_stays_empty() {
        assert_eq!(
            EnvVar {
                key: "K".into(),
                value: String::new()
            }
            .masked_value(),
            ""
        );
    }

    #[test]
    fn service_info_unknown_and_labels() {
        let unknown = ServiceInfo::unknown("redis", "cache");
        assert_eq!(unknown.status_label(), "unknown");
        assert!(unknown.status_badge_css().contains("slate"));
        assert!(unknown.linked_apps.is_empty());

        let running = ServiceInfo {
            status: "running".into(),
            ..ServiceInfo::unknown("redis", "cache")
        };
        assert_eq!(running.status_label(), "running");
        assert!(running.status_badge_css().contains("emerald"));

        let stopped = ServiceInfo {
            status: "stopped".into(),
            ..ServiceInfo::unknown("redis", "cache")
        };
        assert!(stopped.status_badge_css().contains("red"));
        assert_eq!(ServiceInfo::display_or_dash(""), "—");
        assert_eq!(ServiceInfo::display_or_dash("5432"), "5432");
    }

    #[test]
    fn mount_formats_locator_arg_and_phases() {
        let mount = Mount {
            host: "/var/lib/dokku/data/storage/alpha".into(),
            container: "/app/storage".into(),
            options: String::new(),
            phases: vec!["deploy".into(), "run".into()],
        };
        assert_eq!(
            mount.locator(),
            "/var/lib/dokku/data/storage/alpha:/app/storage"
        );
        assert_eq!(mount.arg(), mount.locator());
        assert_eq!(mount.phases_label(), "deploy, run");

        let with_options = Mount {
            options: "ro".into(),
            ..mount
        };
        assert_eq!(
            with_options.arg(),
            "/var/lib/dokku/data/storage/alpha:/app/storage:ro"
        );
    }

    #[test]
    fn mount_without_phases_labels_as_dash() {
        let mount = Mount {
            host: "vol".into(),
            container: "/data".into(),
            options: String::new(),
            phases: Vec::new(),
        };
        assert_eq!(mount.phases_label(), "—");
    }

    #[test]
    fn app_mounts_reports_counts() {
        let empty = AppMounts {
            app: "alpha".into(),
            mounts: Vec::new(),
        };
        assert!(empty.is_empty());
        assert_eq!(empty.mount_count(), 0);

        let populated = AppMounts {
            app: "alpha".into(),
            mounts: vec![Mount {
                host: "/host".into(),
                container: "/c".into(),
                options: String::new(),
                phases: vec!["deploy".into()],
            }],
        };
        assert!(!populated.is_empty());
        assert_eq!(populated.mount_count(), 1);
    }

    #[test]
    fn log_lines_tail_keeps_the_last_lines() {
        let lines = LogLines::new(vec!["a".into(), "b".into(), "c".into()]);
        assert_eq!(lines.tail(2).as_slice(), &["b", "c"]);
        assert_eq!(lines.tail(5).as_slice(), &["a", "b", "c"]);
        assert!(lines.tail(0).is_empty());
    }

    #[test]
    fn format_bytes_buckets() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1 KiB");
        assert_eq!(format_bytes(2048), "2 KiB");
        assert_eq!(format_bytes(1_572_864), "1.5 MiB");
        assert_eq!(format_bytes(6_455_296), "6.2 MiB");
        assert_eq!(format_bytes(8_326_963_200), "7.8 GiB");
        assert_eq!(format_bytes(1_099_511_627_776), "1.0 TiB");
    }

    /// Numbers from the real host capture in `tests/fixtures/redis_stats.txt`.
    fn captured_stats() -> ServiceStats {
        ServiceStats {
            host_memory_total_kb: Some(8_131_800),
            host_memory_available_kb: Some(3_872_192),
            memory_current_bytes: Some(9_748_480),
            memory_limit_bytes: None,
            inactive_file_bytes: Some(2_715_648),
            cpu_delta_usec: Some(5_122),
            cpu_total_usec: Some(23_949_388_167),
            elapsed_secs: Some(1.01),
            cpus: Some(4),
            data_kb: Some(12),
            fs_total_kb: Some(162_406_320),
            fs_used_kb: Some(132_162_720),
            fs_avail_kb: Some(30_227_216),
        }
    }

    #[test]
    fn service_stats_working_set_and_memory_labels() {
        let stats = captured_stats();
        assert_eq!(stats.memory_working_set_bytes(), Some(7_032_832));
        assert_eq!(stats.memory_used_label(), "6.7 MiB");
        assert_eq!(
            stats.memory_detail_label(),
            "of 7.8 GiB host (3.7 GiB available)"
        );
        assert_eq!(stats.memory_percent_label(), "0.1%");
        assert!(!stats.is_empty());
    }

    #[test]
    fn service_stats_unlimited_without_host_available_omits_parenthetical() {
        let stats = ServiceStats {
            host_memory_available_kb: None,
            ..captured_stats()
        };
        assert_eq!(stats.memory_detail_label(), "of 7.8 GiB host");
    }

    #[test]
    fn service_stats_memory_limit_label() {
        let stats = ServiceStats {
            memory_limit_bytes: Some(2_147_483_648),
            ..captured_stats()
        };
        assert_eq!(stats.memory_detail_label(), "of 2.0 GiB");
        // The limit takes precedence over the host total for the percentage.
        assert_eq!(stats.memory_base_bytes(), Some(2_147_483_648));
    }

    #[test]
    fn service_stats_cpu_sampling_math() {
        let stats = captured_stats();
        let percent = stats.cpu_percent().expect("cpu percent");
        assert!((percent - 0.1268).abs() < 0.01, "{percent}");
        assert_eq!(stats.cpu_percent_label(), "0.1%");
        assert_eq!(stats.cpu_total_secs(), Some(23_949));
        assert_eq!(stats.cpu_total_label(), "6h 39m");
    }

    #[test]
    fn format_duration_buckets() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(59), "59s");
        assert_eq!(format_duration(60), "1m 0s");
        assert_eq!(format_duration(312), "5m 12s");
        assert_eq!(format_duration(3_600), "1h 0m");
        assert_eq!(format_duration(23_949), "6h 39m");
        assert_eq!(format_duration(90_000), "1d 1h");
    }

    #[test]
    fn service_stats_disk_labels() {
        let stats = captured_stats();
        assert_eq!(stats.data_label(), "12 KiB");
        assert_eq!(stats.fs_used_label(), "126.0 GiB");
        assert_eq!(stats.fs_total_label(), "154.9 GiB");
        assert_eq!(stats.fs_avail_label(), "28.8 GiB");
        assert_eq!(stats.fs_percent_label(), "81.4%");
    }

    #[test]
    fn percent_rounding_clamps_for_progress_values() {
        assert_eq!(captured_stats().memory_percent_rounded(), Some(0));
        assert_eq!(captured_stats().fs_percent_rounded(), Some(81));
        let over = ServiceStats {
            memory_current_bytes: Some(2_000),
            inactive_file_bytes: None,
            memory_limit_bytes: Some(1_000),
            ..ServiceStats::default()
        };
        assert_eq!(over.memory_percent_rounded(), Some(100));
        assert_eq!(ServiceStats::default().memory_percent_rounded(), None);
    }

    #[test]
    fn empty_service_stats_render_dashes() {
        let stats = ServiceStats::default();
        assert!(stats.is_empty());
        assert_eq!(stats.memory_used_label(), "—");
        assert_eq!(stats.memory_detail_label(), "—");
        assert_eq!(stats.memory_percent_label(), "—");
        assert_eq!(stats.cpu_percent_label(), "—");
        assert_eq!(stats.cpu_total_secs(), None);
        assert_eq!(stats.data_label(), "—");
        assert_eq!(stats.fs_used_label(), "—");
        assert_eq!(stats.fs_percent(), None);
    }

    #[test]
    fn volume_usage_labels() {
        let usage = VolumeUsage {
            used_kb: Some(164),
            fs_total_kb: Some(162_406_320),
            fs_used_kb: Some(132_287_668),
            fs_avail_kb: Some(30_102_164),
        };
        assert!(!usage.is_empty());
        assert_eq!(usage.used_label(), "164 KiB");
        assert_eq!(usage.fs_percent_label(), "81.5%");
        assert_eq!(usage.fs_label(), "81% of 154.9 GiB used");
        assert_eq!(usage.fs_used_label(), "126.2 GiB");
        assert_eq!(usage.fs_total_label(), "154.9 GiB");
        assert_eq!(usage.fs_avail_label(), "28.7 GiB");
    }

    #[test]
    fn empty_volume_usage_renders_dashes() {
        let usage = VolumeUsage::default();
        assert!(usage.is_empty());
        assert_eq!(usage.used_label(), "—");
        assert_eq!(usage.fs_label(), "—");
        assert_eq!(usage.fs_percent(), None);
    }
}
