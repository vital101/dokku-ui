use crate::domain::AppName;
use crate::domain::types::ScaleEntry;

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
            DokkuCommand::PsScaleGet { app } => vec![
                "ps:scale".into(),
                app.as_str().into(),
                "--format".into(),
                "json".into(),
            ],
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
            DokkuCommand::ServiceInfo { plugin, service } => vec![
                format!("{plugin}:info"),
                service.clone(),
                "--format".into(),
                "json".into(),
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

    /// Commands that trigger a deploy/rebuild can outlast the global command
    /// timeout; this returns a longer per-command ceiling where needed.
    pub fn timeout_override_secs(&self) -> Option<u64> {
        match self {
            DokkuCommand::PsScaleSet { .. } => Some(120),
            DokkuCommand::PsRebuild { .. } => Some(300),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str) -> AppName {
        AppName::try_from(name).expect("valid app name")
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
            vec!["ps:scale", "myapp", "--format", "json"]
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
            vec!["redis:info", "cache", "--format", "json"]
        );
    }

    #[test]
    fn timeout_override_only_for_long_commands() {
        assert_eq!(
            DokkuCommand::PsRebuild { app: app("myapp") }.timeout_override_secs(),
            Some(300)
        );
        assert_eq!(
            DokkuCommand::PsScaleSet {
                app: app("myapp"),
                scales: vec![ScaleEntry::new("web", 1)],
            }
            .timeout_override_secs(),
            Some(120)
        );
        for command in [
            DokkuCommand::AppsList,
            DokkuCommand::PsReport { app: app("myapp") },
            DokkuCommand::PsScaleGet { app: app("myapp") },
            DokkuCommand::PsInspect { app: app("myapp") },
        ] {
            assert_eq!(command.timeout_override_secs(), None, "{command:?}");
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
}
