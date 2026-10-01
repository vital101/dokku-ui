use std::sync::Arc;

use futures_util::future::join_all;
use tokio::sync::Semaphore;

use crate::domain::AppName;
use crate::domain::command::DokkuCommand;
use crate::domain::parse::{ParseError, parse_apps_list, parse_ps_report};
use crate::domain::types::{AppHealth, AppStats, PsReport};

use super::client::{DokkuClient, DokkuError};

const MAX_CONCURRENT_REPORTS: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppRow {
    pub name: String,
    pub health: AppHealth,
    pub process_count: i64,
}

impl AppRow {
    pub fn process_label(&self) -> String {
        match self.process_count {
            -1 => "—".to_owned(),
            count => count.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DashboardData {
    pub rows: Vec<AppRow>,
    pub stats: AppStats,
}

#[derive(Debug, thiserror::Error)]
pub enum DashboardError {
    #[error("failed to list apps: {0}")]
    List(#[from] DokkuError),
    #[error("failed to parse apps:list output: {0}")]
    Parse(#[from] ParseError),
}

pub async fn dashboard_data(client: &dyn DokkuClient) -> Result<DashboardData, DashboardError> {
    let names = parse_apps_list(&client.exec(&DokkuCommand::AppsList).await?.stdout)?;
    let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_REPORTS));

    let results: Vec<(String, Option<PsReport>)> = join_all(names.into_iter().map(|name| {
        let permits = permits.clone();
        async move {
            let report = match AppName::try_from(name.clone()) {
                Err(_) => None,
                Ok(app) => {
                    let _permit = permits.acquire().await.ok();
                    let output = client.exec(&DokkuCommand::PsReport { app }).await;
                    match output {
                        Ok(output) => parse_ps_report(&output.stdout).ok(),
                        Err(_) => None,
                    }
                }
            };
            (name, report)
        }
    }))
    .await;

    let mut rows: Vec<AppRow> = results
        .iter()
        .map(|(name, report)| AppRow {
            process_count: report.as_ref().map(|r| r.process_count).unwrap_or(-1),
            health: AppHealth::from_report(report.as_ref()),
            name: name.clone(),
        })
        .collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name));

    let reports: Vec<PsReport> = results
        .iter()
        .filter_map(|(_, report)| report.clone())
        .collect();
    let stats = AppStats::from_reports(&reports);

    Ok(DashboardData { rows, stats })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dokku::{DokkuOutput, MockClient};

    fn app(name: &str) -> crate::domain::AppName {
        crate::domain::AppName::try_from(name).expect("valid app name")
    }

    fn report(running: bool, deployed: bool, process_count: i64) -> DokkuOutput {
        DokkuOutput::ok(format!(
            r#"{{"deployed": "{deployed}", "running": "{running}", "processes": "{process_count}"}}"#
        ))
    }

    fn calls_for(client: &MockClient, command: &DokkuCommand) -> usize {
        client.calls().iter().filter(|c| c == &command).count()
    }

    #[tokio::test]
    async fn happy_path_assembles_rows_and_stats() {
        let client = MockClient::new()
            .stub(
                DokkuCommand::AppsList,
                Ok(DokkuOutput::ok(r#"["alpha","beta","gamma"]"#)),
            )
            .stub(
                DokkuCommand::PsReport { app: app("alpha") },
                Ok(report(true, true, 2)),
            )
            .stub(
                DokkuCommand::PsReport { app: app("beta") },
                Ok(report(false, true, 1)),
            )
            .stub(
                DokkuCommand::PsReport { app: app("gamma") },
                Ok(report(false, false, 0)),
            );

        let data = dashboard_data(&client).await.expect("dashboard data");

        assert_eq!(
            data.rows,
            vec![
                AppRow {
                    name: "alpha".into(),
                    health: AppHealth::Running,
                    process_count: 2,
                },
                AppRow {
                    name: "beta".into(),
                    health: AppHealth::Stopped,
                    process_count: 1,
                },
                AppRow {
                    name: "gamma".into(),
                    health: AppHealth::NotDeployed,
                    process_count: 0,
                },
            ]
        );
        assert_eq!(
            data.stats,
            AppStats {
                total: 3,
                running: 1,
                stopped: 2,
            }
        );
        for name in ["alpha", "beta", "gamma"] {
            assert_eq!(
                calls_for(&client, &DokkuCommand::PsReport { app: app(name) }),
                1,
                "{name} queried once"
            );
        }
    }

    #[tokio::test]
    async fn failing_report_degrades_to_unknown_row() {
        let client = MockClient::new()
            .stub(
                DokkuCommand::AppsList,
                Ok(DokkuOutput::ok(r#"["good","bad"]"#)),
            )
            .stub(
                DokkuCommand::PsReport { app: app("good") },
                Ok(report(true, true, 1)),
            )
            .stub(
                DokkuCommand::PsReport { app: app("bad") },
                Err(crate::dokku::DokkuError::Exit {
                    code: 1,
                    stderr: "boom".into(),
                }),
            );

        let data = dashboard_data(&client).await.expect("dashboard data");

        assert_eq!(
            data.rows,
            vec![
                AppRow {
                    name: "bad".into(),
                    health: AppHealth::Unknown,
                    process_count: -1,
                },
                AppRow {
                    name: "good".into(),
                    health: AppHealth::Running,
                    process_count: 1,
                },
            ]
        );
        assert_eq!(
            data.stats,
            AppStats {
                total: 1,
                running: 1,
                stopped: 0,
            }
        );
    }

    #[tokio::test]
    async fn unparseable_report_degrades_to_unknown_row() {
        let client = MockClient::new()
            .stub(DokkuCommand::AppsList, Ok(DokkuOutput::ok(r#"["weird"]"#)))
            .stub(
                DokkuCommand::PsReport { app: app("weird") },
                Ok(DokkuOutput::ok("not json")),
            );

        let data = dashboard_data(&client).await.expect("dashboard data");

        assert_eq!(data.rows[0].health, AppHealth::Unknown);
        assert_eq!(data.stats.total, 0);
    }

    #[tokio::test]
    async fn invalid_app_name_renders_unknown_without_exec() {
        let client = MockClient::new().stub(
            DokkuCommand::AppsList,
            Ok(DokkuOutput::ok(r#"["Bad_Name"]"#)),
        );

        let data = dashboard_data(&client).await.expect("dashboard data");

        assert_eq!(data.rows.len(), 1);
        assert_eq!(data.rows[0].health, AppHealth::Unknown);
        assert!(
            client
                .calls()
                .iter()
                .all(|c| !matches!(c, DokkuCommand::PsReport { .. })),
            "no ps:report executed for invalid name"
        );
    }

    #[tokio::test]
    async fn apps_list_error_propagates() {
        let client =
            MockClient::with_default(Err(crate::dokku::DokkuError::Connect("refused".into())));

        assert!(matches!(
            dashboard_data(&client).await,
            Err(DashboardError::List(DokkuError::Connect(_)))
        ));
    }

    #[tokio::test]
    async fn apps_list_parse_error_propagates() {
        let client =
            MockClient::new().stub(DokkuCommand::AppsList, Ok(DokkuOutput::ok("not json")));

        assert!(matches!(
            dashboard_data(&client).await,
            Err(DashboardError::Parse(_))
        ));
    }

    #[tokio::test]
    async fn empty_apps_list_yields_empty_data() {
        let client = MockClient::new().stub(DokkuCommand::AppsList, Ok(DokkuOutput::ok("[]")));

        let data = dashboard_data(&client).await.expect("dashboard data");

        assert!(data.rows.is_empty());
        assert_eq!(data.stats, AppStats::default());
    }

    #[tokio::test]
    async fn many_apps_all_receive_reports_without_deadlock() {
        let names: Vec<String> = (0..20).map(|i| format!("app{i:02}")).collect();
        let mut client = MockClient::new().stub(
            DokkuCommand::AppsList,
            Ok(DokkuOutput::ok(format!("{names:?}"))),
        );
        for name in &names {
            let app = app(name);
            client = client.stub(
                DokkuCommand::PsReport { app: app.clone() },
                Ok(report(true, true, 1)),
            );
        }

        let data = dashboard_data(&client).await.expect("dashboard data");

        assert_eq!(data.rows.len(), 20);
        assert_eq!(data.stats.total, 20);
        assert!(data.rows.iter().all(|row| row.health == AppHealth::Running));
    }
}
