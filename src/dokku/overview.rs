use crate::domain::AppName;
use crate::domain::command::DokkuCommand;
use crate::domain::parse::{ParseError, parse_apps_list, parse_apps_report, parse_ps_report};
use crate::domain::types::{AppHealth, AppInfo, AppOverview};

use super::client::{DokkuClient, DokkuError};

#[derive(Debug, thiserror::Error)]
pub enum OverviewError {
    #[error("failed to list apps: {0}")]
    List(#[from] DokkuError),
    #[error("failed to parse apps:list output: {0}")]
    ParseList(#[from] ParseError),
    #[error("app `{0}` was not found")]
    AppNotFound(String),
    #[error("failed to fetch app report: {0}")]
    Report(#[from] DokkuError),
}

pub async fn app_overview(
    client: &dyn DokkuClient,
    name: &str,
) -> Result<AppOverview, OverviewError> {
    let names = parse_apps_list(&client.exec(&DokkuCommand::AppsList).await?.stdout)?;
    if !names.iter().any(|n| n == name) {
        return Err(OverviewError::AppNotFound(name.to_owned()));
    }

    let app = AppName::try_from(name.to_owned())
        .map_err(|_| OverviewError::AppNotFound(name.to_owned()))?;

    let app_info = match client.exec(&DokkuCommand::AppsReport { app: app.clone() }).await {
        Ok(output) => parse_apps_report(&output.stdout, name),
        Err(_) => None,
    };

    let ps_report = match client.exec(&DokkuCommand::PsReport { app }).await {
        Ok(output) => parse_ps_report(&output.stdout).ok(),
        Err(err) => return Err(OverviewError::Report(err)),
    };

    Ok(AppOverview {
        name: name.to_owned(),
        app_info,
        ps_report,
        health: AppHealth::from_report(ps_report.as_ref()),
        process_count: ps_report.as_ref().map(|r| r.process_count).unwrap_or(-1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dokku::{DokkuOutput, MockClient};

    fn app(name: &str) -> AppName {
        AppName::try_from(name).expect("valid app name")
    }

    fn report(running: bool, deployed: bool, process_count: i64) -> DokkuOutput {
        DokkuOutput::ok(format!(
            r#"{{"deployed": "{deployed}", "running": "{running}", "processes": "{process_count}"}}"#
        ))
    }

    fn apps_report() -> DokkuOutput {
        DokkuOutput::ok(
            r#"{"app created at": "2026-01-01T00:00:00Z", "app locked": "false"}"#.to_owned(),
        )
    }

    #[tokio::test]
    async fn happy_path_assembles_overview() {
        let client = MockClient::new()
            .stub(DokkuCommand::AppsList, Ok(DokkuOutput::ok(r#"["alpha","beta"]"#)))
            .stub(
                DokkuCommand::AppsReport { app: app("alpha") },
                Ok(apps_report()),
            )
            .stub(
                DokkuCommand::PsReport { app: app("alpha") },
                Ok(report(true, true, 2)),
            );

        let overview = app_overview(&client, "alpha").await.expect("overview");

        assert_eq!(overview.name, "alpha");
        assert_eq!(overview.health, AppHealth::Running);
        assert_eq!(overview.process_count, 2);
        let info = overview.app_info.expect("app_info");
        assert!(!info.locked);
        assert_eq!(info.created_at, "2026-01-01T00:00:00Z");
    }

    #[tokio::test]
    async fn unknown_app_yields_not_found_without_extra_calls() {
        let client = MockClient::new().stub(
            DokkuCommand::AppsList,
            Ok(DokkuOutput::ok(r#"["alpha"]"#)),
        );

        assert!(matches!(
            app_overview(&client, "nope").await,
            Err(OverviewError::AppNotFound(_))
        ));
        assert!(
            client
                .calls()
                .iter()
                .all(|c| !matches!(c, DokkuCommand::PsReport { .. } | DokkuCommand::AppsReport { .. })),
            "no report calls for unknown app"
        );
    }

    #[tokio::test]
    async fn ps_report_error_propagates() {
        let client = MockClient::new()
            .stub(DokkuCommand::AppsList, Ok(DokkuOutput::ok(r#"["alpha"]"#)))
            .stub(
                DokkuCommand::AppsReport { app: app("alpha") },
                Ok(apps_report()),
            )
            .stub(
                DokkuCommand::PsReport { app: app("alpha") },
                Err(crate::dokku::DokkuError::Exit {
                    code: 1,
                    stderr: "boom".into(),
                }),
            );

        assert!(matches!(
            app_overview(&client, "alpha").await,
            Err(OverviewError::Report(_))
        ));
    }

    #[tokio::test]
    async fn apps_list_error_propagates() {
        let client = MockClient::with_default(Err(crate::dokku::DokkuError::Connect(
            "refused".into(),
        )));

        assert!(matches!(
            app_overview(&client, "alpha").await,
            Err(OverviewError::List(_))
        ));
    }

    #[tokio::test]
    async fn apps_list_parse_error_propagates() {
        let client =
            MockClient::new().stub(DokkuCommand::AppsList, Ok(DokkuOutput::ok("not json")));

        assert!(matches!(
            app_overview(&client, "alpha").await,
            Err(OverviewError::ParseList(_))
        ));
    }

    #[tokio::test]
    async fn apps_report_degrades_on_invalid_json() {
        let client = MockClient::new()
            .stub(DokkuCommand::AppsList, Ok(DokkuOutput::ok(r#"["alpha"]"#)))
            .stub(
                DokkuCommand::AppsReport { app: app("alpha") },
                Ok(DokkuOutput::ok("not json")),
            )
            .stub(
                DokkuCommand::PsReport { app: app("alpha") },
                Ok(report(true, true, 1)),
            );

        let overview = app_overview(&client, "alpha").await.expect("overview");
        assert!(overview.app_info.is_none());
    }

    #[tokio::test]
    async fn apps_report_exec_error_degrades() {
        let client = MockClient::new()
            .stub(DokkuCommand::AppsList, Ok(DokkuOutput::ok(r#"["alpha"]"#)))
            .stub(
                DokkuCommand::AppsReport { app: app("alpha") },
                Err(crate::dokku::DokkuError::Exit {
                    code: 1,
                    stderr: "boom".into(),
                }),
            )
            .stub(
                DokkuCommand::PsReport { app: app("alpha") },
                Ok(report(false, true, 0)),
            );

        let overview = app_overview(&client, "alpha").await.expect("overview");
        assert!(overview.app_info.is_none());
        assert_eq!(overview.health, AppHealth::Stopped);
    }
}