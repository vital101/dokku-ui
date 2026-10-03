use crate::domain::AppName;
use crate::domain::command::DokkuCommand;
use crate::domain::parse::{parse_apps_list, parse_config_show, parse_logs};
use crate::domain::types::{EnvVar, LogLines};

use super::client::{DokkuClient, DokkuError};
use super::overview::OverviewError;

#[derive(Debug, thiserror::Error)]
pub enum AppPageError {
    #[error(transparent)]
    PreCheck(#[from] OverviewError),
    #[error("failed to fetch app data: {0}")]
    Fetch(#[from] DokkuError),
}

pub async fn ensure_app_exists(
    client: &dyn DokkuClient,
    name: &str,
) -> Result<AppName, OverviewError> {
    let names = parse_apps_list(&client.exec(&DokkuCommand::AppsList).await?.stdout);
    if !names.iter().any(|n| n == name) {
        return Err(OverviewError::AppNotFound(name.to_owned()));
    }
    AppName::try_from(name.to_owned()).map_err(|_| OverviewError::AppNotFound(name.to_owned()))
}

pub async fn app_config(client: &dyn DokkuClient, name: &str) -> Result<Vec<EnvVar>, AppPageError> {
    let app = ensure_app_exists(client, name).await?;
    let output = client.exec(&DokkuCommand::ConfigShow { app }).await?;
    Ok(parse_config_show(&output.stdout))
}

pub async fn app_logs(
    client: &dyn DokkuClient,
    name: &str,
    num_lines: u32,
) -> Result<LogLines, AppPageError> {
    let app = ensure_app_exists(client, name).await?;
    let output = client.exec(&DokkuCommand::Logs { app, num_lines }).await?;
    Ok(parse_logs(&output.stdout))
}

#[cfg(test)]
mod tests {
    use crate::domain::AppName;

    use super::*;
    use crate::dokku::{DokkuOutput, MockClient};

    fn app(name: &str) -> AppName {
        AppName::try_from(name).expect("valid app name")
    }

    const CONFIG_SHOW: &str = include_str!("../../tests/fixtures/config_show.txt");
    const LOGS: &str = include_str!("../../tests/fixtures/logs.txt");

    fn alpha_client() -> MockClient {
        MockClient::new().stub(
            DokkuCommand::AppsList,
            Ok(DokkuOutput::ok("=====> My Apps\nalpha")),
        )
    }

    #[tokio::test]
    async fn ensure_exists_returns_parsed_name() {
        let client = alpha_client();
        let name = ensure_app_exists(&client, "alpha").await.expect("exists");
        assert_eq!(name, app("alpha"));
    }

    #[tokio::test]
    async fn ensure_exists_rejects_unknown_app() {
        let client = alpha_client();
        assert!(matches!(
            ensure_app_exists(&client, "nope").await,
            Err(OverviewError::AppNotFound(_))
        ));
    }

    #[tokio::test]
    async fn app_config_parses_fixture_output() {
        let client = alpha_client().stub(
            DokkuCommand::ConfigShow { app: app("alpha") },
            Ok(DokkuOutput::ok(CONFIG_SHOW)),
        );

        let vars = app_config(&client, "alpha").await.expect("config");
        assert_eq!(vars.len(), 3);
        assert_eq!(vars[0].key, "DATABASE_URL");
        assert_eq!(vars[0].value, "postgres://user:pass@host/db");
    }

    #[tokio::test]
    async fn app_config_unknown_app_skips_config_call() {
        let client = alpha_client();
        assert!(matches!(
            app_config(&client, "nope").await,
            Err(AppPageError::PreCheck(OverviewError::AppNotFound(_)))
        ));
        assert!(
            client
                .calls()
                .iter()
                .all(|c| !matches!(c, DokkuCommand::ConfigShow { .. })),
            "no ConfigShow call for unknown app"
        );
    }

    #[tokio::test]
    async fn app_config_fetch_error_propagates() {
        let client = alpha_client().stub(
            DokkuCommand::ConfigShow { app: app("alpha") },
            Err(crate::dokku::DokkuError::Exit {
                code: 1,
                stderr: "boom".into(),
            }),
        );

        assert!(matches!(
            app_config(&client, "alpha").await,
            Err(AppPageError::Fetch(_))
        ));
    }

    #[tokio::test]
    async fn app_logs_parses_and_strips_ansi() {
        let client = alpha_client().stub(
            DokkuCommand::Logs {
                app: app("alpha"),
                num_lines: 200,
            },
            Ok(DokkuOutput::ok(LOGS)),
        );

        let logs = app_logs(&client, "alpha", 200).await.expect("logs");
        assert_eq!(logs.len(), 3);
        assert!(!logs.as_slice().iter().any(|line| line.contains('\x1b')));
    }

    #[tokio::test]
    async fn app_logs_unknown_app_skips_logs_call() {
        let client = alpha_client();
        assert!(matches!(
            app_logs(&client, "nope", 200).await,
            Err(AppPageError::PreCheck(OverviewError::AppNotFound(_)))
        ));
        assert!(
            client
                .calls()
                .iter()
                .all(|c| !matches!(c, DokkuCommand::Logs { .. })),
            "no Logs call for unknown app"
        );
    }

    #[tokio::test]
    async fn app_logs_passes_num_lines_through() {
        let client = alpha_client().stub(
            DokkuCommand::Logs {
                app: app("alpha"),
                num_lines: 50,
            },
            Ok(DokkuOutput::ok("")),
        );

        app_logs(&client, "alpha", 50).await.expect("logs");
        assert!(client.calls().contains(&DokkuCommand::Logs {
            app: app("alpha"),
            num_lines: 50
        }));
    }
}
