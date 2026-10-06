use crate::domain::AppName;
use crate::domain::capabilities::{CapabilityFamily, Requirement};
use crate::domain::domain_name::DomainName;
use crate::domain::git::GitBuildMode;
use crate::domain::mount_spec::MountSpec;
use crate::domain::service_create::ServiceCreateOptions;
use crate::domain::service_name::ServiceName;
use crate::domain::service_plugin::ServicePlugin;
use crate::domain::types::{EnvVar, ScaleEntry};

/// Fixed, read-only stats script executed inside a service container via
/// `<plugin>:enter <service> sh -c <script>`. Emits `key=value` lines only;
/// `__DATA_DIR__` is replaced with the plugin's in-container data directory.
/// Validated against the live host (cgroup v2, host kernel via /proc).
///
/// Must stay single-line and free of `'`: dokku's SSH wrapper re-splits
/// `$SSH_ORIGINAL_COMMAND` with `xargs -n 1` + `readarray`, which mangles
/// shell-style `'\''` escapes (xargs doesn't understand them) and splits
/// multi-line arguments on the per-token `echo` output.
const SERVICE_STATS_SCRIPT: &str = r#"DD=__DATA_DIR__; echo "host_mem_total_kb=$(awk "/^MemTotal:/{print \$2}" /proc/meminfo)"; echo "host_mem_avail_kb=$(awk "/^MemAvailable:/{print \$2}" /proc/meminfo)"; echo "mem_current=$(cat /sys/fs/cgroup/memory.current 2>/dev/null)"; echo "mem_limit=$(cat /sys/fs/cgroup/memory.max 2>/dev/null)"; echo "inactive_file=$(awk "/^inactive_file /{print \$2}" /sys/fs/cgroup/memory.stat 2>/dev/null)"; read up1 _ < /proc/uptime; c1=$(awk "/^usage_usec/{print \$2}" /sys/fs/cgroup/cpu.stat 2>/dev/null); sleep 1; read up2 _ < /proc/uptime; c2=$(awk "/^usage_usec/{print \$2}" /sys/fs/cgroup/cpu.stat 2>/dev/null); echo "elapsed_s=$(awk "BEGIN{print $up2-$up1}")"; echo "cpu_delta_usec=$( [ -n "$c2" ] && [ -n "$c1" ] && echo $((c2-c1)) )"; echo "cpu_total_usec=$c2"; echo "cpus=$(nproc 2>/dev/null)"; echo "data_kb=$(du -sk $DD 2>/dev/null | cut -f1)"; echo "fs_total_kb=$(df -Pk $DD | awk "NR==2{print \$2}")"; echo "fs_used_kb=$(df -Pk $DD | awk "NR==2{print \$3}")"; echo "fs_avail_kb=$(df -Pk $DD | awk "NR==2{print \$4}")""#;

/// Fixed disk-usage script run via `storage:exec <entry> -- sh -c <script>`
/// in a throwaway container with the entry mounted at `/data`.
///
/// Must stay single-line and free of `'` for the same reason as
/// [`SERVICE_STATS_SCRIPT`].
const VOLUME_USAGE_SCRIPT: &str = r#"echo "used_kb=$(du -sk /data 2>/dev/null | cut -f1)"; echo "fs_total_kb=$(df -Pk /data | awk "NR==2{print \$2}")"; echo "fs_used_kb=$(df -Pk /data | awk "NR==2{print \$3}")"; echo "fs_avail_kb=$(df -Pk /data | awk "NR==2{print \$4}")""#;

fn service_stats_script(data_dir: &str) -> String {
    SERVICE_STATS_SCRIPT.replace("__DATA_DIR__", data_dir)
}

/// How long a command may run. See [`DokkuCommand::timeout`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandTimeout {
    /// Bounded by the client default (`COMMAND_TIMEOUT_SECS`).
    Default,
    /// No timeout: the run ends when the command (or the connection) does.
    Indefinite,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DokkuCommand {
    AppsList,
    AppsCreate {
        app: AppName,
    },
    AppsDestroy {
        app: AppName,
        force: bool,
    },
    AppsLock {
        app: AppName,
    },
    AppsUnlock {
        app: AppName,
    },
    AppsRename {
        app: AppName,
        new_name: AppName,
    },
    PsReport {
        app: AppName,
    },
    PsStart {
        app: AppName,
    },
    PsStop {
        app: AppName,
    },
    PsRestart {
        app: AppName,
    },
    PsRebuild {
        app: AppName,
    },
    PsScaleGet {
        app: AppName,
    },
    PsScaleSet {
        app: AppName,
        scales: Vec<ScaleEntry>,
    },
    PsInspect {
        app: AppName,
    },
    AppsReport {
        app: AppName,
    },
    BuildsReport {
        app: AppName,
    },
    DomainsReport {
        app: AppName,
    },
    DomainsAdd {
        app: AppName,
        domains: Vec<DomainName>,
    },
    DomainsRemove {
        app: AppName,
        domains: Vec<DomainName>,
    },
    DomainsSet {
        app: AppName,
        domains: Vec<DomainName>,
    },
    ResourceReport {
        app: AppName,
    },
    ResourceLimit {
        app: AppName,
        process_type: String,
        cpu: Option<String>,
        memory: Option<String>,
        memory_swap: Option<String>,
    },
    ResourceReserve {
        app: AppName,
        process_type: String,
        cpu: Option<String>,
        memory: Option<String>,
    },
    ResourceLimitClear {
        app: AppName,
        process_type: String,
    },
    ResourceReserveClear {
        app: AppName,
        process_type: String,
    },
    ServiceInfo {
        plugin: String,
        service: String,
    },
    ServiceList {
        plugin: ServicePlugin,
    },
    ServiceCreate {
        plugin: ServicePlugin,
        service: ServiceName,
        options: ServiceCreateOptions,
    },
    ServiceDestroy {
        plugin: ServicePlugin,
        service: ServiceName,
        force: bool,
    },
    ServiceStart {
        plugin: ServicePlugin,
        service: ServiceName,
    },
    ServiceStop {
        plugin: ServicePlugin,
        service: ServiceName,
    },
    ServiceRestart {
        plugin: ServicePlugin,
        service: ServiceName,
    },
    ServiceLinks {
        plugin: ServicePlugin,
        service: ServiceName,
    },
    ServiceLink {
        plugin: ServicePlugin,
        service: ServiceName,
        app: AppName,
    },
    ServiceUnlink {
        plugin: ServicePlugin,
        service: ServiceName,
        app: AppName,
    },
    ServiceLogs {
        plugin: ServicePlugin,
        service: ServiceName,
        num_lines: u32,
    },
    ServiceExpose {
        plugin: ServicePlugin,
        service: ServiceName,
        ports: String,
    },
    ServiceUnexpose {
        plugin: ServicePlugin,
        service: ServiceName,
    },
    ServiceStats {
        plugin: ServicePlugin,
        service: ServiceName,
    },
    BuildpacksList {
        app: AppName,
    },
    BuildpacksSet {
        app: AppName,
        buildpack: String,
        index: Option<u32>,
    },
    BuildpacksAdd {
        app: AppName,
        buildpack: String,
        index: Option<u32>,
    },
    BuildpacksRemove {
        app: AppName,
        buildpack: String,
    },
    BuildpacksClear {
        app: AppName,
    },
    BuilderReport {
        app: AppName,
    },
    BuilderSet {
        app: AppName,
        property: String,
        value: Option<String>,
    },
    StorageReport,
    StorageListEntries,
    StorageUsage {
        entry: String,
    },
    StorageMount {
        app: AppName,
        mount: MountSpec,
    },
    StorageUnmount {
        app: AppName,
        mount: MountSpec,
    },
    PluginList,
    AppLinks {
        plugin: String,
        app: AppName,
    },
    ConfigShow {
        app: AppName,
    },
    CronList {
        app: AppName,
    },
    CronRun {
        app: AppName,
        cron_id: String,
    },
    CronSuspend {
        app: AppName,
        cron_id: String,
    },
    CronResume {
        app: AppName,
        cron_id: String,
    },
    MaintenanceEnable {
        app: AppName,
    },
    MaintenanceDisable {
        app: AppName,
    },
    HttpAuthEnable {
        app: AppName,
    },
    HttpAuthDisable {
        app: AppName,
    },
    HttpAuthAddUser {
        app: AppName,
        username: String,
        password: String,
    },
    HttpAuthRemoveUser {
        app: AppName,
        username: String,
    },
    ConfigSet {
        app: AppName,
        vars: Vec<EnvVar>,
    },
    ConfigUnset {
        app: AppName,
        keys: Vec<String>,
    },
    GitReport {
        app: AppName,
    },
    GitPublicKey,
    GitSet {
        app: AppName,
        property: String,
        value: Option<String>,
    },
    GitSync {
        app: AppName,
        repo: String,
        git_ref: Option<String>,
        build_mode: GitBuildMode,
    },
    GitFromImage {
        app: AppName,
        image: String,
    },
    GitFromArchive {
        app: AppName,
        archive_url: String,
    },
    LetsencryptList,
    LetsencryptActive {
        app: AppName,
    },
    LetsencryptEnable {
        app: AppName,
    },
    LetsencryptDisable {
        app: AppName,
    },
    LetsencryptRevoke {
        app: AppName,
    },
    LetsencryptCleanup {
        app: AppName,
    },
    LetsencryptCronJob {
        add: bool,
    },
    LetsencryptSet {
        app: AppName,
        property: String,
        value: Option<String>,
    },
    CertsReport {
        app: Option<AppName>,
    },
    Logs {
        app: AppName,
        num_lines: u32,
        follow: bool,
        process: Option<String>,
    },
    LogsFailed {
        app: AppName,
    },
    NginxAccessLogs {
        app: AppName,
        follow: bool,
    },
    NginxErrorLogs {
        app: AppName,
        follow: bool,
    },
    DokkuVersion,
    Help {
        family: CapabilityFamily,
    },
}

impl DokkuCommand {
    pub fn argv(&self) -> Vec<String> {
        match self {
            DokkuCommand::AppsList => vec!["apps:list".into()],
            DokkuCommand::AppsCreate { app } => {
                vec!["apps:create".into(), app.as_str().into()]
            }
            DokkuCommand::AppsDestroy { app, force } => {
                let mut argv = vec!["apps:destroy".into(), app.as_str().into()];
                if *force {
                    argv.push("--force".into());
                }
                argv
            }
            DokkuCommand::AppsLock { app } => {
                vec!["apps:lock".into(), app.as_str().into()]
            }
            DokkuCommand::AppsUnlock { app } => {
                vec!["apps:unlock".into(), app.as_str().into()]
            }
            DokkuCommand::AppsRename { app, new_name } => vec![
                "apps:rename".into(),
                app.as_str().into(),
                new_name.as_str().into(),
            ],
            DokkuCommand::PsReport { app } => {
                vec![
                    "ps:report".into(),
                    app.as_str().into(),
                    "--format".into(),
                    "json".into(),
                ]
            }
            DokkuCommand::PsStart { app } => vec!["ps:start".into(), app.as_str().into()],
            DokkuCommand::PsStop { app } => vec!["ps:stop".into(), app.as_str().into()],
            DokkuCommand::PsRestart { app } => vec!["ps:restart".into(), app.as_str().into()],
            DokkuCommand::PsRebuild { app } => vec!["ps:rebuild".into(), app.as_str().into()],
            DokkuCommand::PsScaleGet { app } => {
                vec!["ps:scale".into(), app.as_str().into()]
            }
            DokkuCommand::PsScaleSet { app, scales } => {
                let mut argv = vec!["ps:scale".into(), app.as_str().into()];
                argv.extend(scales.iter().map(ScaleEntry::arg));
                argv
            }
            DokkuCommand::PsInspect { app } => vec!["ps:inspect".into(), app.as_str().into()],
            DokkuCommand::AppsReport { app } => {
                vec![
                    "apps:report".into(),
                    app.as_str().into(),
                    "--format".into(),
                    "json".into(),
                ]
            }
            DokkuCommand::BuildsReport { app } => {
                vec![
                    "builds:report".into(),
                    app.as_str().into(),
                    "--format".into(),
                    "json".into(),
                ]
            }
            DokkuCommand::DomainsReport { app } => {
                vec![
                    "domains:report".into(),
                    app.as_str().into(),
                    "--format".into(),
                    "json".into(),
                ]
            }
            DokkuCommand::DomainsAdd { app, domains } => {
                let mut argv = vec!["domains:add".into(), app.as_str().into()];
                argv.extend(domains.iter().map(|domain| domain.as_str().into()));
                argv
            }
            DokkuCommand::DomainsRemove { app, domains } => {
                let mut argv = vec!["domains:remove".into(), app.as_str().into()];
                argv.extend(domains.iter().map(|domain| domain.as_str().into()));
                argv
            }
            DokkuCommand::DomainsSet { app, domains } => {
                let mut argv = vec!["domains:set".into(), app.as_str().into()];
                argv.extend(domains.iter().map(|domain| domain.as_str().into()));
                argv
            }
            DokkuCommand::ResourceReport { app } => {
                vec!["resource:report".into(), app.as_str().into()]
            }
            DokkuCommand::ResourceLimit {
                app,
                process_type,
                cpu,
                memory,
                memory_swap,
            } => {
                let mut argv = vec![
                    "resource:limit".into(),
                    "--process-type".into(),
                    process_type.clone(),
                ];
                if let Some(cpu) = cpu {
                    argv.push("--cpu".into());
                    argv.push(cpu.clone());
                }
                if let Some(memory) = memory {
                    argv.push("--memory".into());
                    argv.push(memory.clone());
                }
                if let Some(memory_swap) = memory_swap {
                    argv.push("--memory-swap".into());
                    argv.push(memory_swap.clone());
                }
                argv.push(app.as_str().into());
                argv
            }
            DokkuCommand::ResourceReserve {
                app,
                process_type,
                cpu,
                memory,
            } => {
                let mut argv = vec![
                    "resource:reserve".into(),
                    "--process-type".into(),
                    process_type.clone(),
                ];
                if let Some(cpu) = cpu {
                    argv.push("--cpu".into());
                    argv.push(cpu.clone());
                }
                if let Some(memory) = memory {
                    argv.push("--memory".into());
                    argv.push(memory.clone());
                }
                argv.push(app.as_str().into());
                argv
            }
            DokkuCommand::ResourceLimitClear { app, process_type } => vec![
                "resource:limit-clear".into(),
                "--process-type".into(),
                process_type.clone(),
                app.as_str().into(),
            ],
            DokkuCommand::ResourceReserveClear { app, process_type } => vec![
                "resource:reserve-clear".into(),
                "--process-type".into(),
                process_type.clone(),
                app.as_str().into(),
            ],
            DokkuCommand::ServiceInfo { plugin, service } => {
                vec![format!("{plugin}:info"), service.clone()]
            }
            DokkuCommand::ServiceList { plugin } => vec![format!("{plugin}:list")],
            DokkuCommand::ServiceCreate {
                plugin,
                service,
                options,
            } => {
                // Flags follow the service name: the plugin's `service_create`
                // parses `"${@:2}"` (dokku-postgres 1.36.4 `functions`).
                let mut argv = vec![format!("{plugin}:create"), service.as_str().into()];
                if let Some(image) = &options.image {
                    argv.push("--image".into());
                    argv.push(image.clone());
                }
                if let Some(version) = &options.image_version {
                    argv.push("--image-version".into());
                    argv.push(version.clone());
                }
                if let Some(custom_env) = &options.custom_env {
                    argv.push("--custom-env".into());
                    argv.push(custom_env.clone());
                }
                if let Some(config_options) = &options.config_options {
                    argv.push("--config-options".into());
                    argv.push(config_options.clone());
                }
                argv
            }
            DokkuCommand::ServiceDestroy {
                plugin,
                service,
                force,
            } => {
                let mut argv = vec![format!("{plugin}:destroy"), service.as_str().into()];
                if *force {
                    argv.push("--force".into());
                }
                argv
            }
            DokkuCommand::ServiceStart { plugin, service } => {
                vec![format!("{plugin}:start"), service.as_str().into()]
            }
            DokkuCommand::ServiceStop { plugin, service } => {
                vec![format!("{plugin}:stop"), service.as_str().into()]
            }
            DokkuCommand::ServiceRestart { plugin, service } => {
                vec![format!("{plugin}:restart"), service.as_str().into()]
            }
            DokkuCommand::ServiceLinks { plugin, service } => {
                vec![format!("{plugin}:links"), service.as_str().into()]
            }
            DokkuCommand::ServiceLink {
                plugin,
                service,
                app,
            } => vec![
                format!("{plugin}:link"),
                service.as_str().into(),
                app.as_str().into(),
            ],
            DokkuCommand::ServiceUnlink {
                plugin,
                service,
                app,
            } => vec![
                format!("{plugin}:unlink"),
                service.as_str().into(),
                app.as_str().into(),
            ],
            DokkuCommand::ServiceLogs {
                plugin,
                service,
                num_lines,
            } => vec![
                format!("{plugin}:logs"),
                service.as_str().into(),
                // This plugin generation takes the tail count as the third
                // positional argument with the (optional) follow flag second,
                // so the flag slot must be an explicit empty string:
                // `dokku redis:logs svc '' 200` -> `docker logs --tail 200`.
                String::new(),
                num_lines.to_string(),
            ],
            DokkuCommand::ServiceExpose {
                plugin,
                service,
                ports,
            } => vec![
                format!("{plugin}:expose"),
                service.as_str().into(),
                ports.clone(),
            ],
            DokkuCommand::ServiceUnexpose { plugin, service } => {
                vec![format!("{plugin}:unexpose"), service.as_str().into()]
            }
            DokkuCommand::ServiceStats { plugin, service } => vec![
                format!("{plugin}:enter"),
                service.as_str().into(),
                "sh".into(),
                "-c".into(),
                service_stats_script(plugin.data_dir()),
            ],
            DokkuCommand::BuildpacksList { app } => {
                vec!["buildpacks:list".into(), app.as_str().into()]
            }
            DokkuCommand::BuildpacksSet {
                app,
                buildpack,
                index,
            } => {
                let mut argv = vec!["buildpacks:set".into()];
                if let Some(index) = index {
                    argv.push("--index".into());
                    argv.push(index.to_string());
                }
                argv.push(app.as_str().into());
                argv.push(buildpack.clone());
                argv
            }
            DokkuCommand::BuildpacksAdd {
                app,
                buildpack,
                index,
            } => {
                let mut argv = vec!["buildpacks:add".into()];
                if let Some(index) = index {
                    argv.push("--index".into());
                    argv.push(index.to_string());
                }
                argv.push(app.as_str().into());
                argv.push(buildpack.clone());
                argv
            }
            DokkuCommand::BuildpacksRemove { app, buildpack } => vec![
                "buildpacks:remove".into(),
                app.as_str().into(),
                buildpack.clone(),
            ],
            DokkuCommand::BuildpacksClear { app } => {
                vec!["buildpacks:clear".into(), app.as_str().into()]
            }
            DokkuCommand::BuilderReport { app } => {
                vec!["builder:report".into(), app.as_str().into()]
            }
            DokkuCommand::BuilderSet {
                app,
                property,
                value,
            } => {
                let mut argv = vec!["builder:set".into(), app.as_str().into(), property.clone()];
                if let Some(value) = value {
                    argv.push(value.clone());
                }
                argv
            }
            DokkuCommand::StorageReport => vec!["storage:report".into()],
            DokkuCommand::StorageListEntries => vec![
                "storage:list-entries".into(),
                "--format".into(),
                "json".into(),
            ],
            DokkuCommand::StorageUsage { entry } => vec![
                "storage:exec".into(),
                entry.clone(),
                "--".into(),
                "sh".into(),
                "-c".into(),
                VOLUME_USAGE_SCRIPT.into(),
            ],
            DokkuCommand::StorageMount { app, mount } => {
                vec!["storage:mount".into(), app.as_str().into(), mount.arg()]
            }
            DokkuCommand::StorageUnmount { app, mount } => vec![
                "storage:unmount".into(),
                app.as_str().into(),
                mount.locator(),
            ],
            DokkuCommand::PluginList => vec!["plugin:list".into()],
            DokkuCommand::AppLinks { plugin, app } => {
                vec![format!("{plugin}:app-links"), app.as_str().into()]
            }
            DokkuCommand::ConfigShow { app } => {
                vec!["config:show".into(), app.as_str().into()]
            }
            DokkuCommand::CronList { app } => vec![
                "cron:list".into(),
                app.as_str().into(),
                "--format".into(),
                "json".into(),
            ],
            DokkuCommand::CronRun { app, cron_id } => {
                vec!["cron:run".into(), app.as_str().into(), cron_id.clone()]
            }
            DokkuCommand::CronSuspend { app, cron_id } => {
                vec!["cron:suspend".into(), app.as_str().into(), cron_id.clone()]
            }
            DokkuCommand::CronResume { app, cron_id } => {
                vec!["cron:resume".into(), app.as_str().into(), cron_id.clone()]
            }
            DokkuCommand::MaintenanceEnable { app } => {
                vec!["maintenance:enable".into(), app.as_str().into()]
            }
            DokkuCommand::MaintenanceDisable { app } => {
                vec!["maintenance:disable".into(), app.as_str().into()]
            }
            DokkuCommand::HttpAuthEnable { app } => {
                vec!["http-auth:enable".into(), app.as_str().into()]
            }
            DokkuCommand::HttpAuthDisable { app } => {
                vec!["http-auth:disable".into(), app.as_str().into()]
            }
            DokkuCommand::HttpAuthAddUser {
                app,
                username,
                password,
            } => vec![
                "http-auth:add-user".into(),
                app.as_str().into(),
                username.clone(),
                password.clone(),
            ],
            DokkuCommand::HttpAuthRemoveUser { app, username } => vec![
                "http-auth:remove-user".into(),
                app.as_str().into(),
                username.clone(),
            ],
            DokkuCommand::ConfigSet { app, vars } => {
                let mut argv = vec!["config:set".into(), app.as_str().into()];
                argv.extend(vars.iter().map(|var| format!("{}={}", var.key, var.value)));
                argv
            }
            DokkuCommand::ConfigUnset { app, keys } => {
                let mut argv = vec!["config:unset".into(), app.as_str().into()];
                argv.extend(keys.iter().cloned());
                argv
            }
            DokkuCommand::GitReport { app } => {
                vec!["git:report".into(), app.as_str().into()]
            }
            DokkuCommand::GitPublicKey => vec!["git:public-key".into()],
            DokkuCommand::GitSet {
                app,
                property,
                value,
            } => {
                let mut argv = vec!["git:set".into(), app.as_str().into(), property.clone()];
                if let Some(value) = value {
                    argv.push(value.clone());
                }
                argv
            }
            DokkuCommand::GitSync {
                app,
                repo,
                git_ref,
                build_mode,
            } => {
                // `git:sync [--build|--build-if-changes] <app> <repo> [<ref>]`:
                // the build flag comes before the app name.
                let mut argv = vec!["git:sync".into()];
                if let Some(flag) = build_mode.flag() {
                    argv.push(flag.into());
                }
                argv.push(app.as_str().into());
                argv.push(repo.clone());
                if let Some(git_ref) = git_ref {
                    argv.push(git_ref.clone());
                }
                argv
            }
            DokkuCommand::GitFromImage { app, image } => {
                vec!["git:from-image".into(), app.as_str().into(), image.clone()]
            }
            DokkuCommand::GitFromArchive { app, archive_url } => vec![
                "git:from-archive".into(),
                app.as_str().into(),
                archive_url.clone(),
            ],
            DokkuCommand::LetsencryptList => vec!["letsencrypt:list".into()],
            DokkuCommand::LetsencryptActive { app } => {
                vec!["letsencrypt:active".into(), app.as_str().into()]
            }
            DokkuCommand::LetsencryptEnable { app } => {
                vec!["letsencrypt:enable".into(), app.as_str().into()]
            }
            DokkuCommand::LetsencryptDisable { app } => {
                vec!["letsencrypt:disable".into(), app.as_str().into()]
            }
            DokkuCommand::LetsencryptRevoke { app } => {
                vec!["letsencrypt:revoke".into(), app.as_str().into()]
            }
            DokkuCommand::LetsencryptCleanup { app } => {
                vec!["letsencrypt:cleanup".into(), app.as_str().into()]
            }
            DokkuCommand::LetsencryptCronJob { add } => {
                let flag = if *add { "--add" } else { "--remove" };
                vec!["letsencrypt:cron-job".into(), flag.into()]
            }
            DokkuCommand::LetsencryptSet {
                app,
                property,
                value,
            } => {
                let mut argv = vec![
                    "letsencrypt:set".into(),
                    app.as_str().into(),
                    property.clone(),
                ];
                if let Some(value) = value {
                    argv.push(value.clone());
                }
                argv
            }
            DokkuCommand::CertsReport { app } => {
                let mut argv = vec!["certs:report".into()];
                if let Some(app) = app {
                    argv.push(app.as_str().into());
                }
                argv
            }
            DokkuCommand::Logs {
                app,
                num_lines,
                follow,
                process,
            } => {
                let mut argv = vec!["logs".into(), app.as_str().into()];
                if *follow {
                    argv.push("--tail".into());
                }
                if let Some(process) = process {
                    argv.push("--ps".into());
                    argv.push(process.clone());
                }
                argv.push("--num".into());
                argv.push(num_lines.to_string());
                argv
            }
            DokkuCommand::LogsFailed { app } => {
                vec!["logs:failed".into(), app.as_str().into()]
            }
            DokkuCommand::NginxAccessLogs { app, follow } => {
                let mut argv = vec!["nginx:access-logs".into(), app.as_str().into()];
                if *follow {
                    argv.push("-t".into());
                }
                argv
            }
            DokkuCommand::NginxErrorLogs { app, follow } => {
                let mut argv = vec!["nginx:error-logs".into(), app.as_str().into()];
                if *follow {
                    argv.push("-t".into());
                }
                argv
            }
            // `dokku --version` does not survive the SSH wrapper (the host
            // rejected it with a coreutils banner as the command name); the
            // `version` subcommand does
            DokkuCommand::DokkuVersion => vec!["version".into()],
            // `<cmd>:help` is dokku's help convention over SSH; `<cmd> --help`
            // is parsed as an app name by some plugins (e.g. logs)
            DokkuCommand::Help { family } => vec![family.help_probe().into()],
        }
    }

    /// Timeout policy for this command.
    ///
    /// Mutating actions the user watches stream in the run modal have **no
    /// timeout**: builds can take many minutes, and the SSH keepalive
    /// surfaces dead connections instead of a timer. Everything else (reports,
    /// config, logs, lists) is bounded by the client default
    /// (`COMMAND_TIMEOUT_SECS`) so panel fetches can never hang.
    pub fn timeout(&self) -> CommandTimeout {
        match self {
            DokkuCommand::PsStart { .. }
            | DokkuCommand::PsStop { .. }
            | DokkuCommand::PsRestart { .. }
            | DokkuCommand::PsRebuild { .. }
            | DokkuCommand::PsScaleSet { .. }
            | DokkuCommand::AppsDestroy { .. }
            | DokkuCommand::AppsRename { .. }
            | DokkuCommand::ServiceCreate { .. }
            | DokkuCommand::ServiceDestroy { .. }
            | DokkuCommand::ServiceStart { .. }
            | DokkuCommand::ServiceStop { .. }
            | DokkuCommand::ServiceRestart { .. }
            | DokkuCommand::ServiceLink { .. }
            | DokkuCommand::ServiceUnlink { .. }
            | DokkuCommand::ServiceExpose { .. }
            | DokkuCommand::ServiceUnexpose { .. }
            | DokkuCommand::StorageMount { .. }
            | DokkuCommand::StorageUnmount { .. }
            | DokkuCommand::DomainsAdd { .. }
            | DokkuCommand::DomainsRemove { .. }
            | DokkuCommand::DomainsSet { .. }
            | DokkuCommand::ResourceLimit { .. }
            | DokkuCommand::ResourceReserve { .. }
            | DokkuCommand::ResourceLimitClear { .. }
            | DokkuCommand::ResourceReserveClear { .. }
            | DokkuCommand::CronRun { .. }
            | DokkuCommand::CronSuspend { .. }
            | DokkuCommand::CronResume { .. }
            | DokkuCommand::MaintenanceEnable { .. }
            | DokkuCommand::MaintenanceDisable { .. }
            | DokkuCommand::HttpAuthEnable { .. }
            | DokkuCommand::HttpAuthDisable { .. }
            | DokkuCommand::HttpAuthAddUser { .. }
            | DokkuCommand::HttpAuthRemoveUser { .. }
            | DokkuCommand::BuildpacksSet { .. }
            | DokkuCommand::BuildpacksAdd { .. }
            | DokkuCommand::BuildpacksRemove { .. }
            | DokkuCommand::BuildpacksClear { .. }
            | DokkuCommand::BuilderSet { .. }
            | DokkuCommand::GitSync { .. }
            | DokkuCommand::GitFromImage { .. }
            | DokkuCommand::GitFromArchive { .. }
            | DokkuCommand::LetsencryptEnable { .. }
            | DokkuCommand::LetsencryptDisable { .. }
            | DokkuCommand::LetsencryptRevoke { .. }
            | DokkuCommand::LetsencryptCleanup { .. }
            | DokkuCommand::LetsencryptCronJob { .. }
            | DokkuCommand::LetsencryptSet { .. }
            | DokkuCommand::Logs { follow: true, .. }
            | DokkuCommand::NginxAccessLogs { follow: true, .. }
            | DokkuCommand::NginxErrorLogs { follow: true, .. } => CommandTimeout::Indefinite,
            _ => CommandTimeout::Default,
        }
    }

    /// Capability gate for this command, consulted by the capability framework
    /// so handlers render explanatory states instead of failing at command
    /// time. Exhaustive: adding a variant forces a requirement here.
    pub fn requirement(&self) -> Requirement {
        match self {
            DokkuCommand::ServiceInfo { plugin, .. } | DokkuCommand::AppLinks { plugin, .. } => {
                Requirement::Plugin {
                    name: plugin.clone(),
                }
            }
            DokkuCommand::ServiceList { plugin }
            | DokkuCommand::ServiceCreate { plugin, .. }
            | DokkuCommand::ServiceDestroy { plugin, .. }
            | DokkuCommand::ServiceStart { plugin, .. }
            | DokkuCommand::ServiceStop { plugin, .. }
            | DokkuCommand::ServiceRestart { plugin, .. }
            | DokkuCommand::ServiceLinks { plugin, .. }
            | DokkuCommand::ServiceLink { plugin, .. }
            | DokkuCommand::ServiceUnlink { plugin, .. }
            | DokkuCommand::ServiceLogs { plugin, .. }
            | DokkuCommand::ServiceExpose { plugin, .. }
            | DokkuCommand::ServiceUnexpose { plugin, .. }
            | DokkuCommand::ServiceStats { plugin, .. } => Requirement::Plugin {
                name: plugin.as_str().to_owned(),
            },
            DokkuCommand::MaintenanceEnable { .. } | DokkuCommand::MaintenanceDisable { .. } => {
                Requirement::Plugin {
                    name: "maintenance".to_owned(),
                }
            }
            DokkuCommand::HttpAuthEnable { .. }
            | DokkuCommand::HttpAuthDisable { .. }
            | DokkuCommand::HttpAuthAddUser { .. }
            | DokkuCommand::HttpAuthRemoveUser { .. } => Requirement::Plugin {
                name: "http-auth".to_owned(),
            },
            DokkuCommand::LetsencryptList
            | DokkuCommand::LetsencryptActive { .. }
            | DokkuCommand::LetsencryptEnable { .. }
            | DokkuCommand::LetsencryptDisable { .. }
            | DokkuCommand::LetsencryptRevoke { .. }
            | DokkuCommand::LetsencryptCleanup { .. }
            | DokkuCommand::LetsencryptCronJob { .. }
            | DokkuCommand::LetsencryptSet { .. } => Requirement::Plugin {
                name: "letsencrypt".to_owned(),
            },
            _ => Requirement::Core,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str) -> AppName {
        AppName::try_from(name).expect("valid app name")
    }

    fn plugin(name: &str) -> ServicePlugin {
        ServicePlugin::try_from(name).expect("supported plugin")
    }

    fn service(name: &str) -> ServiceName {
        ServiceName::try_from(name).expect("valid service name")
    }

    #[test]
    fn apps_list_argv() {
        assert_eq!(DokkuCommand::AppsList.argv(), vec!["apps:list"]);
    }

    #[test]
    fn apps_create_argv() {
        assert_eq!(
            DokkuCommand::AppsCreate { app: app("myapp") }.argv(),
            vec!["apps:create", "myapp"]
        );
    }

    #[test]
    fn apps_destroy_argv_without_force() {
        assert_eq!(
            DokkuCommand::AppsDestroy {
                app: app("myapp"),
                force: false
            }
            .argv(),
            vec!["apps:destroy", "myapp"]
        );
    }

    #[test]
    fn apps_destroy_argv_with_force() {
        assert_eq!(
            DokkuCommand::AppsDestroy {
                app: app("myapp"),
                force: true
            }
            .argv(),
            vec!["apps:destroy", "myapp", "--force"]
        );
    }

    #[test]
    fn ps_report_argv() {
        assert_eq!(
            DokkuCommand::PsReport { app: app("myapp") }.argv(),
            vec!["ps:report", "myapp", "--format", "json"]
        );
    }

    #[test]
    fn apps_report_argv() {
        assert_eq!(
            DokkuCommand::AppsReport { app: app("myapp") }.argv(),
            vec!["apps:report", "myapp", "--format", "json"]
        );
    }

    #[test]
    fn ps_actions_argv() {
        for (command, expected) in [
            (
                DokkuCommand::PsStart { app: app("myapp") },
                vec!["ps:start", "myapp"],
            ),
            (
                DokkuCommand::PsStop { app: app("myapp") },
                vec!["ps:stop", "myapp"],
            ),
            (
                DokkuCommand::PsRestart { app: app("myapp") },
                vec!["ps:restart", "myapp"],
            ),
        ] {
            assert_eq!(command.argv(), expected);
        }
    }

    #[test]
    fn builds_report_argv() {
        assert_eq!(
            DokkuCommand::BuildsReport { app: app("myapp") }.argv(),
            vec!["builds:report", "myapp", "--format", "json"]
        );
    }

    #[test]
    fn domains_report_argv() {
        assert_eq!(
            DokkuCommand::DomainsReport { app: app("myapp") }.argv(),
            vec!["domains:report", "myapp", "--format", "json"]
        );
    }

    #[test]
    fn domains_mutations_argv() {
        let domains = vec![
            DomainName::try_from("one.example.com").expect("d"),
            DomainName::try_from("two.example.com").expect("d"),
        ];
        assert_eq!(
            DokkuCommand::DomainsAdd {
                app: app("myapp"),
                domains: domains.clone(),
            }
            .argv(),
            vec!["domains:add", "myapp", "one.example.com", "two.example.com"]
        );
        assert_eq!(
            DokkuCommand::DomainsRemove {
                app: app("myapp"),
                domains: domains.clone(),
            }
            .argv(),
            vec![
                "domains:remove",
                "myapp",
                "one.example.com",
                "two.example.com"
            ]
        );
        assert_eq!(
            DokkuCommand::DomainsSet {
                app: app("myapp"),
                domains,
            }
            .argv(),
            vec!["domains:set", "myapp", "one.example.com", "two.example.com"]
        );
    }

    #[test]
    fn resource_mutations_argv() {
        assert_eq!(
            DokkuCommand::ResourceLimit {
                app: app("myapp"),
                process_type: "web".into(),
                cpu: Some("1".into()),
                memory: Some("128".into()),
                memory_swap: None,
            }
            .argv(),
            vec![
                "resource:limit",
                "--process-type",
                "web",
                "--cpu",
                "1",
                "--memory",
                "128",
                "myapp"
            ]
        );
        assert_eq!(
            DokkuCommand::ResourceReserve {
                app: app("myapp"),
                process_type: "worker".into(),
                cpu: None,
                memory: Some("512".into()),
            }
            .argv(),
            vec![
                "resource:reserve",
                "--process-type",
                "worker",
                "--memory",
                "512",
                "myapp"
            ]
        );
        assert_eq!(
            DokkuCommand::ResourceLimitClear {
                app: app("myapp"),
                process_type: "web".into(),
            }
            .argv(),
            vec!["resource:limit-clear", "--process-type", "web", "myapp"]
        );
        assert_eq!(
            DokkuCommand::ResourceReserveClear {
                app: app("myapp"),
                process_type: "web".into(),
            }
            .argv(),
            vec!["resource:reserve-clear", "--process-type", "web", "myapp"]
        );
    }

    #[test]
    fn cron_argv() {
        assert_eq!(
            DokkuCommand::CronList { app: app("myapp") }.argv(),
            vec!["cron:list", "myapp", "--format", "json"]
        );
        assert_eq!(
            DokkuCommand::CronRun {
                app: app("myapp"),
                cron_id: "a1b2c3".into(),
            }
            .argv(),
            vec!["cron:run", "myapp", "a1b2c3"]
        );
        assert_eq!(
            DokkuCommand::CronSuspend {
                app: app("myapp"),
                cron_id: "a1b2c3".into(),
            }
            .argv(),
            vec!["cron:suspend", "myapp", "a1b2c3"]
        );
        assert_eq!(
            DokkuCommand::CronResume {
                app: app("myapp"),
                cron_id: "a1b2c3".into(),
            }
            .argv(),
            vec!["cron:resume", "myapp", "a1b2c3"]
        );
    }

    #[test]
    fn maintenance_and_http_auth_argv() {
        assert_eq!(
            DokkuCommand::MaintenanceEnable { app: app("myapp") }.argv(),
            vec!["maintenance:enable", "myapp"]
        );
        assert_eq!(
            DokkuCommand::MaintenanceDisable { app: app("myapp") }.argv(),
            vec!["maintenance:disable", "myapp"]
        );
        assert_eq!(
            DokkuCommand::HttpAuthEnable { app: app("myapp") }.argv(),
            vec!["http-auth:enable", "myapp"]
        );
        assert_eq!(
            DokkuCommand::HttpAuthDisable { app: app("myapp") }.argv(),
            vec!["http-auth:disable", "myapp"]
        );
        assert_eq!(
            DokkuCommand::HttpAuthAddUser {
                app: app("myapp"),
                username: "alice".into(),
                password: "s3cr3t".into(),
            }
            .argv(),
            vec!["http-auth:add-user", "myapp", "alice", "s3cr3t"]
        );
        assert_eq!(
            DokkuCommand::HttpAuthRemoveUser {
                app: app("myapp"),
                username: "alice".into(),
            }
            .argv(),
            vec!["http-auth:remove-user", "myapp", "alice"]
        );
    }

    #[test]
    fn plugin_backed_commands_declare_their_plugins() {
        use crate::domain::capabilities::Requirement;
        assert_eq!(
            DokkuCommand::MaintenanceEnable { app: app("myapp") }.requirement(),
            Requirement::Plugin {
                name: "maintenance".into()
            }
        );
        assert_eq!(
            DokkuCommand::HttpAuthAddUser {
                app: app("myapp"),
                username: "alice".into(),
                password: "s3cr3t".into(),
            }
            .requirement(),
            Requirement::Plugin {
                name: "http-auth".into()
            }
        );
        for command in [
            DokkuCommand::LetsencryptList,
            DokkuCommand::LetsencryptActive { app: app("myapp") },
            DokkuCommand::LetsencryptEnable { app: app("myapp") },
            DokkuCommand::LetsencryptDisable { app: app("myapp") },
            DokkuCommand::LetsencryptRevoke { app: app("myapp") },
            DokkuCommand::LetsencryptCleanup { app: app("myapp") },
            DokkuCommand::LetsencryptCronJob { add: true },
            DokkuCommand::LetsencryptSet {
                app: app("myapp"),
                property: "email".into(),
                value: Some("ops@example.com".into()),
            },
        ] {
            assert_eq!(
                command.requirement(),
                Requirement::Plugin {
                    name: "letsencrypt".into()
                },
                "{command:?}"
            );
        }
    }

    #[test]
    fn tls_argv() {
        assert_eq!(
            DokkuCommand::LetsencryptList.argv(),
            vec!["letsencrypt:list"]
        );
        assert_eq!(
            DokkuCommand::LetsencryptActive { app: app("myapp") }.argv(),
            vec!["letsencrypt:active", "myapp"]
        );
        assert_eq!(
            DokkuCommand::LetsencryptEnable { app: app("myapp") }.argv(),
            vec!["letsencrypt:enable", "myapp"]
        );
        assert_eq!(
            DokkuCommand::LetsencryptRevoke { app: app("myapp") }.argv(),
            vec!["letsencrypt:revoke", "myapp"]
        );
        assert_eq!(
            DokkuCommand::LetsencryptCleanup { app: app("myapp") }.argv(),
            vec!["letsencrypt:cleanup", "myapp"]
        );
        assert_eq!(
            DokkuCommand::LetsencryptCronJob { add: true }.argv(),
            vec!["letsencrypt:cron-job", "--add"]
        );
        assert_eq!(
            DokkuCommand::LetsencryptCronJob { add: false }.argv(),
            vec!["letsencrypt:cron-job", "--remove"]
        );
        assert_eq!(
            DokkuCommand::LetsencryptSet {
                app: app("myapp"),
                property: "email".into(),
                value: Some("ops@example.com".into()),
            }
            .argv(),
            vec!["letsencrypt:set", "myapp", "email", "ops@example.com"]
        );
        assert_eq!(
            DokkuCommand::LetsencryptSet {
                app: app("myapp"),
                property: "staging".into(),
                value: Some("true".into()),
            }
            .argv(),
            vec!["letsencrypt:set", "myapp", "staging", "true"]
        );
        assert_eq!(
            DokkuCommand::CertsReport { app: None }.argv(),
            vec!["certs:report"]
        );
        assert_eq!(
            DokkuCommand::CertsReport {
                app: Some(app("myapp"))
            }
            .argv(),
            vec!["certs:report", "myapp"]
        );
    }

    #[test]
    fn build_config_argv() {
        assert_eq!(
            DokkuCommand::BuildpacksList { app: app("myapp") }.argv(),
            vec!["buildpacks:list", "myapp"]
        );
        assert_eq!(
            DokkuCommand::BuildpacksSet {
                app: app("myapp"),
                buildpack: "https://example.com/bp".into(),
                index: Some(2),
            }
            .argv(),
            vec![
                "buildpacks:set",
                "--index",
                "2",
                "myapp",
                "https://example.com/bp"
            ]
        );
        assert_eq!(
            DokkuCommand::BuildpacksAdd {
                app: app("myapp"),
                buildpack: "https://example.com/bp".into(),
                index: None,
            }
            .argv(),
            vec!["buildpacks:add", "myapp", "https://example.com/bp"]
        );
        assert_eq!(
            DokkuCommand::BuildpacksRemove {
                app: app("myapp"),
                buildpack: "https://example.com/bp".into(),
            }
            .argv(),
            vec!["buildpacks:remove", "myapp", "https://example.com/bp"]
        );
        assert_eq!(
            DokkuCommand::BuildpacksClear { app: app("myapp") }.argv(),
            vec!["buildpacks:clear", "myapp"]
        );
        assert_eq!(
            DokkuCommand::BuilderReport { app: app("myapp") }.argv(),
            vec!["builder:report", "myapp"]
        );
        assert_eq!(
            DokkuCommand::BuilderSet {
                app: app("myapp"),
                property: "selected".into(),
                value: Some("dockerfile".into()),
            }
            .argv(),
            vec!["builder:set", "myapp", "selected", "dockerfile"]
        );
        assert_eq!(
            DokkuCommand::BuilderSet {
                app: app("myapp"),
                property: "selected".into(),
                value: None,
            }
            .argv(),
            vec!["builder:set", "myapp", "selected"]
        );
    }

    #[test]
    fn plugin_list_argv() {
        assert_eq!(DokkuCommand::PluginList.argv(), vec!["plugin:list"]);
    }

    #[test]
    fn app_links_argv() {
        assert_eq!(
            DokkuCommand::AppLinks {
                plugin: "postgres".into(),
                app: app("myapp")
            }
            .argv(),
            vec!["postgres:app-links", "myapp"]
        );
    }

    #[test]
    fn ps_rebuild_argv() {
        assert_eq!(
            DokkuCommand::PsRebuild { app: app("myapp") }.argv(),
            vec!["ps:rebuild", "myapp"]
        );
    }

    #[test]
    fn ps_scale_get_argv() {
        assert_eq!(
            DokkuCommand::PsScaleGet { app: app("myapp") }.argv(),
            vec!["ps:scale", "myapp"]
        );
    }

    #[test]
    fn ps_scale_set_argv_formats_each_entry() {
        assert_eq!(
            DokkuCommand::PsScaleSet {
                app: app("myapp"),
                scales: vec![ScaleEntry::new("web", 2), ScaleEntry::new("worker", 3)],
            }
            .argv(),
            vec!["ps:scale", "myapp", "web=2", "worker=3"]
        );
    }

    #[test]
    fn ps_scale_set_argv_allows_zero() {
        assert_eq!(
            DokkuCommand::PsScaleSet {
                app: app("myapp"),
                scales: vec![ScaleEntry::new("worker", 0)],
            }
            .argv(),
            vec!["ps:scale", "myapp", "worker=0"]
        );
    }

    #[test]
    fn ps_inspect_argv() {
        assert_eq!(
            DokkuCommand::PsInspect { app: app("myapp") }.argv(),
            vec!["ps:inspect", "myapp"]
        );
    }

    #[test]
    fn resource_report_argv() {
        assert_eq!(
            DokkuCommand::ResourceReport { app: app("myapp") }.argv(),
            vec!["resource:report", "myapp"]
        );
    }

    #[test]
    fn service_info_argv() {
        assert_eq!(
            DokkuCommand::ServiceInfo {
                plugin: "redis".into(),
                service: "cache".into()
            }
            .argv(),
            vec!["redis:info", "cache"]
        );
    }

    #[test]
    fn mutating_actions_run_without_a_timeout() {
        for command in [
            DokkuCommand::PsStart { app: app("myapp") },
            DokkuCommand::PsStop { app: app("myapp") },
            DokkuCommand::PsRestart { app: app("myapp") },
            DokkuCommand::PsRebuild { app: app("myapp") },
            DokkuCommand::PsScaleSet {
                app: app("myapp"),
                scales: vec![ScaleEntry::new("web", 1)],
            },
            DokkuCommand::AppsDestroy {
                app: app("myapp"),
                force: true,
            },
            DokkuCommand::ServiceCreate {
                plugin: plugin("postgres"),
                service: service("db"),
                options: ServiceCreateOptions::default(),
            },
            DokkuCommand::ServiceDestroy {
                plugin: plugin("postgres"),
                service: service("db"),
                force: true,
            },
            DokkuCommand::ServiceStart {
                plugin: plugin("redis"),
                service: service("cache"),
            },
            DokkuCommand::ServiceStop {
                plugin: plugin("redis"),
                service: service("cache"),
            },
            DokkuCommand::ServiceRestart {
                plugin: plugin("redis"),
                service: service("cache"),
            },
            DokkuCommand::ServiceLink {
                plugin: plugin("postgres"),
                service: service("db"),
                app: app("myapp"),
            },
            DokkuCommand::ServiceUnlink {
                plugin: plugin("postgres"),
                service: service("db"),
                app: app("myapp"),
            },
            DokkuCommand::ServiceExpose {
                plugin: plugin("postgres"),
                service: service("db"),
                ports: "5432".into(),
            },
            DokkuCommand::ServiceUnexpose {
                plugin: plugin("postgres"),
                service: service("db"),
            },
            DokkuCommand::StorageMount {
                app: app("myapp"),
                mount: MountSpec::try_from("/host:/data").expect("mount"),
            },
            DokkuCommand::StorageUnmount {
                app: app("myapp"),
                mount: MountSpec::try_from("/host:/data").expect("mount"),
            },
        ] {
            assert_eq!(
                command.timeout(),
                CommandTimeout::Indefinite,
                "{command:?} streams in the run modal"
            );
        }
    }

    #[test]
    fn read_commands_use_the_default_timeout() {
        for command in [
            DokkuCommand::AppsList,
            DokkuCommand::AppsCreate { app: app("myapp") },
            DokkuCommand::AppsReport { app: app("myapp") },
            DokkuCommand::PsReport { app: app("myapp") },
            DokkuCommand::PsScaleGet { app: app("myapp") },
            DokkuCommand::PsInspect { app: app("myapp") },
            DokkuCommand::ConfigShow { app: app("myapp") },
            DokkuCommand::Logs {
                app: app("myapp"),
                num_lines: 200,
                follow: false,
                process: None,
            },
            DokkuCommand::BuildsReport { app: app("myapp") },
            DokkuCommand::DomainsReport { app: app("myapp") },
            DokkuCommand::ResourceReport { app: app("myapp") },
            DokkuCommand::ServiceInfo {
                plugin: "postgres".into(),
                service: "db".into(),
            },
            DokkuCommand::ServiceList {
                plugin: plugin("redis"),
            },
            DokkuCommand::ServiceLinks {
                plugin: plugin("redis"),
                service: service("cache"),
            },
            DokkuCommand::ServiceLogs {
                plugin: plugin("redis"),
                service: service("cache"),
                num_lines: 200,
            },
            DokkuCommand::ServiceStats {
                plugin: plugin("redis"),
                service: service("cache"),
            },
            DokkuCommand::StorageReport,
            DokkuCommand::StorageListEntries,
            DokkuCommand::StorageUsage {
                entry: "legacy-90db719326".into(),
            },
            DokkuCommand::PluginList,
            DokkuCommand::AppLinks {
                plugin: "postgres".into(),
                app: app("myapp"),
            },
        ] {
            assert_eq!(
                command.timeout(),
                CommandTimeout::Default,
                "{command:?} stays bounded"
            );
        }
    }

    #[test]
    fn config_show_argv() {
        assert_eq!(
            DokkuCommand::ConfigShow { app: app("myapp") }.argv(),
            vec!["config:show", "myapp"]
        );
    }

    #[test]
    fn logs_argv() {
        assert_eq!(
            DokkuCommand::Logs {
                app: app("myapp"),
                num_lines: 200,
                follow: false,
                process: None
            }
            .argv(),
            vec!["logs", "myapp", "--num", "200"]
        );
    }

    #[test]
    fn logs_argv_uses_the_ps_flag_when_filtering() {
        assert_eq!(
            DokkuCommand::Logs {
                app: app("myapp"),
                num_lines: 200,
                follow: true,
                process: Some("web".into()),
            }
            .argv(),
            vec!["logs", "myapp", "--tail", "--ps", "web", "--num", "200"]
        );
    }

    #[test]
    fn logs_failed_argv() {
        assert_eq!(
            DokkuCommand::LogsFailed { app: app("myapp") }.argv(),
            vec!["logs:failed", "myapp"]
        );
    }

    #[test]
    fn git_read_argv() {
        assert_eq!(
            DokkuCommand::GitReport { app: app("myapp") }.argv(),
            vec!["git:report", "myapp"]
        );
        assert_eq!(DokkuCommand::GitPublicKey.argv(), vec!["git:public-key"]);
    }

    #[test]
    fn git_set_argv_sets_and_clears() {
        assert_eq!(
            DokkuCommand::GitSet {
                app: app("myapp"),
                property: "deploy-branch".into(),
                value: Some("main".into()),
            }
            .argv(),
            vec!["git:set", "myapp", "deploy-branch", "main"]
        );
        assert_eq!(
            DokkuCommand::GitSet {
                app: app("myapp"),
                property: "deploy-branch".into(),
                value: None,
            }
            .argv(),
            vec!["git:set", "myapp", "deploy-branch"]
        );
    }

    #[test]
    fn git_sync_argv_puts_the_build_flag_first() {
        assert_eq!(
            DokkuCommand::GitSync {
                app: app("myapp"),
                repo: "https://github.com/org/repo.git".into(),
                git_ref: Some("main".into()),
                build_mode: GitBuildMode::Build,
            }
            .argv(),
            vec![
                "git:sync",
                "--build",
                "myapp",
                "https://github.com/org/repo.git",
                "main"
            ]
        );
        assert_eq!(
            DokkuCommand::GitSync {
                app: app("myapp"),
                repo: "git@github.com:org/repo.git".into(),
                git_ref: None,
                build_mode: GitBuildMode::BuildIfChanges,
            }
            .argv(),
            vec![
                "git:sync",
                "--build-if-changes",
                "myapp",
                "git@github.com:org/repo.git"
            ]
        );
        assert_eq!(
            DokkuCommand::GitSync {
                app: app("myapp"),
                repo: "https://github.com/org/repo.git".into(),
                git_ref: None,
                build_mode: GitBuildMode::NoBuild,
            }
            .argv(),
            vec!["git:sync", "myapp", "https://github.com/org/repo.git"]
        );
    }

    #[test]
    fn git_image_and_archive_argv() {
        assert_eq!(
            DokkuCommand::GitFromImage {
                app: app("myapp"),
                image: "ghcr.io/org/app:v1".into(),
            }
            .argv(),
            vec!["git:from-image", "myapp", "ghcr.io/org/app:v1"]
        );
        assert_eq!(
            DokkuCommand::GitFromArchive {
                app: app("myapp"),
                archive_url: "https://example.com/app.tar.gz".into(),
            }
            .argv(),
            vec![
                "git:from-archive",
                "myapp",
                "https://example.com/app.tar.gz"
            ]
        );
    }

    #[test]
    fn service_list_argv() {
        assert_eq!(
            DokkuCommand::ServiceList {
                plugin: plugin("redis")
            }
            .argv(),
            vec!["redis:list"]
        );
    }

    #[test]
    fn service_create_argv() {
        assert_eq!(
            DokkuCommand::ServiceCreate {
                plugin: plugin("postgres"),
                service: service("my-db"),
                options: ServiceCreateOptions::default(),
            }
            .argv(),
            vec!["postgres:create", "my-db"]
        );
    }

    #[test]
    fn service_create_argv_carries_advanced_options_after_the_name() {
        let options = ServiceCreateOptions::parse(
            Some("postgres"),
            Some("16.2"),
            Some("USER=alpha;HOST=beta"),
            Some("--shm-size=1g"),
        )
        .expect("valid options");
        assert_eq!(
            DokkuCommand::ServiceCreate {
                plugin: plugin("postgres"),
                service: service("my-db"),
                options,
            }
            .argv(),
            vec![
                "postgres:create",
                "my-db",
                "--image",
                "postgres",
                "--image-version",
                "16.2",
                "--custom-env",
                "USER=alpha;HOST=beta",
                "--config-options",
                "--shm-size=1g",
            ]
        );
    }

    #[test]
    fn service_destroy_argv_without_force() {
        assert_eq!(
            DokkuCommand::ServiceDestroy {
                plugin: plugin("postgres"),
                service: service("my-db"),
                force: false,
            }
            .argv(),
            vec!["postgres:destroy", "my-db"]
        );
    }

    #[test]
    fn service_destroy_argv_with_force() {
        assert_eq!(
            DokkuCommand::ServiceDestroy {
                plugin: plugin("postgres"),
                service: service("my-db"),
                force: true,
            }
            .argv(),
            vec!["postgres:destroy", "my-db", "--force"]
        );
    }

    #[test]
    fn service_lifecycle_argv() {
        for (command, expected) in [
            (
                DokkuCommand::ServiceStart {
                    plugin: plugin("redis"),
                    service: service("cache"),
                },
                vec!["redis:start", "cache"],
            ),
            (
                DokkuCommand::ServiceStop {
                    plugin: plugin("redis"),
                    service: service("cache"),
                },
                vec!["redis:stop", "cache"],
            ),
            (
                DokkuCommand::ServiceRestart {
                    plugin: plugin("redis"),
                    service: service("cache"),
                },
                vec!["redis:restart", "cache"],
            ),
        ] {
            assert_eq!(command.argv(), expected);
        }
    }

    #[test]
    fn service_links_argv() {
        assert_eq!(
            DokkuCommand::ServiceLinks {
                plugin: plugin("redis"),
                service: service("cache"),
            }
            .argv(),
            vec!["redis:links", "cache"]
        );
    }

    #[test]
    fn service_link_and_unlink_argv() {
        assert_eq!(
            DokkuCommand::ServiceLink {
                plugin: plugin("postgres"),
                service: service("my-db"),
                app: app("myapp"),
            }
            .argv(),
            vec!["postgres:link", "my-db", "myapp"]
        );
        assert_eq!(
            DokkuCommand::ServiceUnlink {
                plugin: plugin("postgres"),
                service: service("my-db"),
                app: app("myapp"),
            }
            .argv(),
            vec!["postgres:unlink", "my-db", "myapp"]
        );
    }

    #[test]
    fn service_logs_argv_uses_an_empty_follow_flag_slot() {
        assert_eq!(
            DokkuCommand::ServiceLogs {
                plugin: plugin("redis"),
                service: service("cache"),
                num_lines: 200,
            }
            .argv(),
            vec!["redis:logs", "cache", "", "200"]
        );
    }

    #[test]
    fn service_expose_and_unexpose_argv() {
        assert_eq!(
            DokkuCommand::ServiceExpose {
                plugin: plugin("postgres"),
                service: service("my-db"),
                ports: "5432".into(),
            }
            .argv(),
            vec!["postgres:expose", "my-db", "5432"]
        );
        assert_eq!(
            DokkuCommand::ServiceUnexpose {
                plugin: plugin("postgres"),
                service: service("my-db"),
            }
            .argv(),
            vec!["postgres:unexpose", "my-db"]
        );
    }

    #[test]
    fn storage_report_argv() {
        assert_eq!(DokkuCommand::StorageReport.argv(), vec!["storage:report"]);
    }

    #[test]
    fn storage_mount_argv_uses_the_full_spec() {
        assert_eq!(
            DokkuCommand::StorageMount {
                app: app("myapp"),
                mount: MountSpec::try_from("/host:/data:ro").expect("mount"),
            }
            .argv(),
            vec!["storage:mount", "myapp", "/host:/data:ro"]
        );
    }

    #[test]
    fn storage_unmount_argv_uses_the_locator() {
        assert_eq!(
            DokkuCommand::StorageUnmount {
                app: app("myapp"),
                mount: MountSpec::try_from("/host:/data:ro").expect("mount"),
            }
            .argv(),
            vec!["storage:unmount", "myapp", "/host:/data"]
        );
    }

    #[test]
    fn service_stats_argv_runs_the_fixed_script() {
        let argv = DokkuCommand::ServiceStats {
            plugin: plugin("postgres"),
            service: service("my-db"),
        }
        .argv();
        assert_eq!(argv[0], "postgres:enter");
        assert_eq!(argv[1], "my-db");
        assert_eq!(argv[2], "sh");
        assert_eq!(argv[3], "-c");
        assert!(
            argv[4].contains("DD=/var/lib/postgresql/data"),
            "{}",
            argv[4]
        );
        assert!(argv[4].contains("memory.current"));
        assert!(argv[4].contains("cpu.stat"));
        assert!(argv[4].contains("du -sk $DD"));
        assert!(argv[4].contains("df -Pk $DD"));
    }

    #[test]
    fn service_stats_script_uses_each_plugins_data_dir() {
        for (plugin_name, expected) in [
            ("postgres", "DD=/var/lib/postgresql/data"),
            ("mysql", "DD=/var/lib/mysql"),
            ("redis", "DD=/data"),
            ("mongo", "DD=/data/db"),
        ] {
            let argv = DokkuCommand::ServiceStats {
                plugin: plugin(plugin_name),
                service: service("svc"),
            }
            .argv();
            assert!(argv[4].contains(expected), "{plugin_name}: {}", argv[4]);
        }
    }

    #[test]
    fn service_stats_script_is_xargs_safe() {
        for plugin_name in ["postgres", "mysql", "redis", "mongo"] {
            let argv = DokkuCommand::ServiceStats {
                plugin: plugin(plugin_name),
                service: service("svc"),
            }
            .argv();
            assert!(
                !argv[4].contains('\''),
                "{plugin_name}: shell quoting breaks dokku's SSH xargs re-split: {}",
                argv[4]
            );
            assert!(
                !argv[4].contains('\n'),
                "{plugin_name}: xargs -n 1 plus readarray split multi-line scripts: {}",
                argv[4]
            );
        }
    }

    #[test]
    fn volume_usage_script_is_xargs_safe() {
        let argv = DokkuCommand::StorageUsage {
            entry: "legacy-90db719326".into(),
        }
        .argv();
        assert!(
            !argv[5].contains('\''),
            "single quote breaks dokku's SSH xargs re-split: {}",
            argv[5]
        );
        assert!(
            !argv[5].contains('\n'),
            "xargs -n 1 plus readarray split multi-line scripts: {}",
            argv[5]
        );
    }

    #[test]
    fn storage_list_entries_argv_requests_json() {
        assert_eq!(
            DokkuCommand::StorageListEntries.argv(),
            vec!["storage:list-entries", "--format", "json"]
        );
    }

    #[test]
    fn storage_usage_argv_runs_the_fixed_script_in_a_throwaway_container() {
        let argv = DokkuCommand::StorageUsage {
            entry: "legacy-90db719326".into(),
        }
        .argv();
        assert_eq!(argv[0], "storage:exec");
        assert_eq!(argv[1], "legacy-90db719326");
        assert_eq!(argv[2], "--");
        assert_eq!(argv[3], "sh");
        assert_eq!(argv[4], "-c");
        assert!(argv[5].contains("du -sk /data"));
        assert!(argv[5].contains("df -Pk /data"));
    }

    #[test]
    fn dokku_version_argv() {
        assert_eq!(DokkuCommand::DokkuVersion.argv(), vec!["version"]);
    }

    #[test]
    fn help_argv_uses_the_dokku_help_convention() {
        assert_eq!(
            DokkuCommand::Help {
                family: crate::domain::capabilities::CapabilityFamily::Logs,
            }
            .argv(),
            vec!["logs:help"]
        );
        assert_eq!(
            DokkuCommand::Help {
                family: crate::domain::capabilities::CapabilityFamily::NginxAccessLogs,
            }
            .argv(),
            vec!["nginx:help"]
        );
    }

    #[test]
    fn probe_commands_use_the_default_timeout() {
        for command in [
            DokkuCommand::DokkuVersion,
            DokkuCommand::Help {
                family: crate::domain::capabilities::CapabilityFamily::Logs,
            },
        ] {
            assert_eq!(command.timeout(), CommandTimeout::Default, "{command:?}");
        }
    }

    #[test]
    fn service_commands_require_their_plugin() {
        assert_eq!(
            DokkuCommand::ServiceList {
                plugin: plugin("redis")
            }
            .requirement(),
            Requirement::Plugin {
                name: "redis".into()
            }
        );
        assert_eq!(
            DokkuCommand::ServiceInfo {
                plugin: "postgres".into(),
                service: "db".into(),
            }
            .requirement(),
            Requirement::Plugin {
                name: "postgres".into()
            }
        );
        assert_eq!(
            DokkuCommand::AppLinks {
                plugin: "mongo".into(),
                app: app("myapp"),
            }
            .requirement(),
            Requirement::Plugin {
                name: "mongo".into()
            }
        );
        assert_eq!(
            DokkuCommand::ServiceStats {
                plugin: plugin("redis"),
                service: service("cache"),
            }
            .requirement(),
            Requirement::Plugin {
                name: "redis".into()
            }
        );
    }

    #[test]
    fn core_commands_are_core_requirements() {
        for command in [
            DokkuCommand::AppsList,
            DokkuCommand::AppsCreate { app: app("myapp") },
            DokkuCommand::ConfigShow { app: app("myapp") },
            DokkuCommand::Logs {
                app: app("myapp"),
                num_lines: 200,
                follow: false,
                process: None,
            },
            DokkuCommand::StorageReport,
            DokkuCommand::DokkuVersion,
            DokkuCommand::Help {
                family: crate::domain::capabilities::CapabilityFamily::Logs,
            },
        ] {
            assert_eq!(command.requirement(), Requirement::Core, "{command:?}");
        }
    }
}
