use crate::domain::AppName;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DokkuCommand {
    AppsList,
    AppsCreate { app: AppName },
    AppsDestroy { app: AppName, force: bool },
    PsReport { app: AppName },
    PsStart { app: AppName },
    PsStop { app: AppName },
    PsRestart { app: AppName },
    ConfigShow { app: AppName },
    Logs { app: AppName, num_lines: u32 },
}

impl DokkuCommand {
    pub fn argv(&self) -> Vec<String> {
        match self {
            DokkuCommand::AppsList => {
                vec!["apps:list".into(), "--format".into(), "json".into()]
            }
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
            DokkuCommand::ConfigShow { app } => {
                vec!["config:show".into(), app.as_str().into()]
            }
            DokkuCommand::Logs { app, num_lines } => vec![
                "logs".into(),
                app.as_str().into(),
                "--num-lines".into(),
                num_lines.to_string(),
            ],
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
        assert_eq!(
            DokkuCommand::AppsList.argv(),
            vec!["apps:list", "--format", "json"]
        );
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
            vec!["logs", "myapp", "--num-lines", "200"]
        );
    }
}
