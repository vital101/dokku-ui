use crate::domain::AppName;
use crate::domain::command::DokkuCommand;
use crate::domain::parse::{parse_config_show, parse_logs};
use crate::domain::types::{EnvVar, LogLines};

use super::client::{DokkuClient, DokkuError};

pub async fn app_config(client: &dyn DokkuClient, app: AppName) -> Result<Vec<EnvVar>, DokkuError> {
    let output = client.exec(&DokkuCommand::ConfigShow { app }).await?;
    Ok(parse_config_show(&output.stdout))
}

pub async fn app_logs(
    client: &dyn DokkuClient,
    app: AppName,
    num_lines: u32,
) -> Result<LogLines, DokkuError> {
    let output = client.exec(&DokkuCommand::Logs { app, num_lines }).await?;
    Ok(parse_logs(&output.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dokku::{DokkuOutput, MockClient};

    fn app(name: &str) -> AppName {
        AppName::try_from(name).expect("valid app name")
    }

    const CONFIG_SHOW: &str = include_str!("../../tests/fixtures/config_show.txt");
    const LOGS: &str = include_str!("../../tests/fixtures/logs.txt");

    #[tokio::test]
    async fn app_config_parses_fixture_output() {
        let client = MockClient::new().stub(
            DokkuCommand::ConfigShow { app: app("alpha") },
            Ok(DokkuOutput::ok(CONFIG_SHOW)),
        );

        let vars = app_config(&client, app("alpha")).await.expect("config");
        assert_eq!(vars.len(), 3);
        assert_eq!(vars[0].key, "DATABASE_URL");
        assert_eq!(vars[0].value, "postgres://user:pass@host/db");
    }

    #[tokio::test]
    async fn app_config_fetch_error_propagates() {
        let client = MockClient::new().stub(
            DokkuCommand::ConfigShow { app: app("alpha") },
            Err(DokkuError::Exit {
                code: 1,
                stderr: "boom".into(),
            }),
        );

        assert!(matches!(
            app_config(&client, app("alpha")).await,
            Err(DokkuError::Exit { .. })
        ));
    }

    #[tokio::test]
    async fn app_logs_parses_and_strips_ansi() {
        let client = MockClient::new().stub(
            DokkuCommand::Logs {
                app: app("alpha"),
                num_lines: 200,
            },
            Ok(DokkuOutput::ok(LOGS)),
        );

        let logs = app_logs(&client, app("alpha"), 200).await.expect("logs");
        assert_eq!(logs.len(), 3);
        assert!(!logs.as_slice().iter().any(|line| line.contains('\x1b')));
    }

    #[tokio::test]
    async fn app_logs_passes_num_lines_through() {
        let client = MockClient::new().stub(
            DokkuCommand::Logs {
                app: app("alpha"),
                num_lines: 50,
            },
            Ok(DokkuOutput::ok("")),
        );

        app_logs(&client, app("alpha"), 50).await.expect("logs");
        assert!(client.calls().contains(&DokkuCommand::Logs {
            app: app("alpha"),
            num_lines: 50
        }));
    }
}
