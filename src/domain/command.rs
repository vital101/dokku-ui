use crate::domain::AppName;
use crate::domain::mount_spec::MountSpec;
use crate::domain::service_name::ServiceName;
use crate::domain::service_plugin::ServicePlugin;
use crate::domain::types::ScaleEntry;

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
    ResourceReport {
        app: AppName,
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
    StorageReport,
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
    Logs {
        app: AppName,
        num_lines: u32,
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
            DokkuCommand::ResourceReport { app } => {
                vec!["resource:report".into(), app.as_str().into()]
            }
            DokkuCommand::ServiceInfo { plugin, service } => {
                vec![format!("{plugin}:info"), service.clone()]
            }
            DokkuCommand::ServiceList { plugin } => vec![format!("{plugin}:list")],
            DokkuCommand::ServiceCreate { plugin, service } => {
                vec![format!("{plugin}:create"), service.as_str().into()]
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
            DokkuCommand::StorageReport => vec!["storage:report".into()],
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
            DokkuCommand::Logs { app, num_lines } => vec![
                "logs".into(),
                app.as_str().into(),
                "--num".into(),
                num_lines.to_string(),
            ],
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
            | DokkuCommand::StorageUnmount { .. } => CommandTimeout::Indefinite,
            _ => CommandTimeout::Default,
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
            DokkuCommand::StorageReport,
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
                num_lines: 200
            }
            .argv(),
            vec!["logs", "myapp", "--num", "200"]
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
            }
            .argv(),
            vec!["postgres:create", "my-db"]
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
}
