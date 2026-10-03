use std::collections::HashMap;

use crate::domain::AppName;
use crate::domain::command::DokkuCommand;
use crate::domain::parse::{parse_apps_list, parse_ps_report_all};
use crate::domain::types::{AppHealth, AppStats, PsReport};

use super::client::{DokkuClient, DokkuError};

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
}

pub async fn dashboard_data(client: &dyn DokkuClient) -> Result<DashboardData, DashboardError> {
    let names = parse_apps_list(&client.exec(&DokkuCommand::AppsList).await?.stdout);
    let apps: Vec<AppName> = names
        .iter()
        .filter_map(|name| AppName::try_from(name.clone()).ok())
        .collect();

    let reports: HashMap<String, PsReport> = if apps.is_empty() {
        HashMap::new()
    } else {
        match client.exec(&DokkuCommand::PsReportAll { apps }).await {
            Ok(output) => parse_ps_report_all(&output.stdout).into_iter().collect(),
            Err(_) => HashMap::new(),
        }
    };

    let mut rows: Vec<AppRow> = names
        .iter()
        .map(|name| {
            let report = reports.get(name);
            AppRow {
                name: name.clone(),
                health: AppHealth::from_report(report),
                process_count: report.map(|r| r.process_count).unwrap_or(-1),
            }
        })
        .collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name));

    let stats = AppStats::from_reports(&reports.values().cloned().collect::<Vec<_>>());

    Ok(DashboardData { rows, stats })
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::dokku::{DokkuOutput, MockClient};

    fn app(name: &str) -> crate::domain::AppName {
        crate::domain::AppName::try_from(name).expect("valid app name")
    }

    fn apps_list_output(names: &[&str]) -> DokkuOutput {
        let mut output = String::from("=====> My Apps");
        for name in names {
            output.push('\n');
            output.push_str(name);
        }
        DokkuOutput::ok(output)
    }

    fn section(
        name: &str,
        deployed: bool,
        running: bool,
        processes: i64,
        statuses: &[&str],
    ) -> String {
        let mut out = format!(
            "=====> {name} ps information\n       Deployed: {deployed}\n       Running: {running}\n       Processes: {processes}\n"
        );
        for status in statuses {
            out.push_str(&format!("       Status {status}\n"));
        }
        out
    }

    fn multi_report_output(sections: &[String]) -> DokkuOutput {
        DokkuOutput::ok(sections.join(""))
    }

    #[tokio::test]
    async fn happy_path_assembles_rows_and_stats() {
        let client = MockClient::new()
            .stub(
                DokkuCommand::AppsList,
                Ok(apps_list_output(&["alpha", "beta", "gamma"])),
            )
            .stub(
                DokkuCommand::PsReportAll {
                    apps: vec![app("alpha"), app("beta"), app("gamma")],
                },
                Ok(multi_report_output(&[
                    section(
                        "alpha",
                        true,
                        true,
                        2,
                        &["web.1: running (CID: a1)", "web.2: running (CID: a2)"],
                    ),
                    section("beta", true, false, 1, &["web.1: exited (CID: b1)"]),
                    section("gamma", false, false, 0, &[]),
                ])),
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
        assert_eq!(client.calls().len(), 2, "one list + one multi-app report");
    }

    #[tokio::test]
    async fn failing_multi_report_degrades_all_rows_to_unknown() {
        let client = MockClient::new()
            .stub(
                DokkuCommand::AppsList,
                Ok(apps_list_output(&["good", "bad"])),
            )
            .stub(
                DokkuCommand::PsReportAll {
                    apps: vec![app("good"), app("bad")],
                },
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
                    health: AppHealth::Unknown,
                    process_count: -1,
                },
            ]
        );
        assert_eq!(data.stats, AppStats::default());
    }

    #[tokio::test]
    async fn unparseable_multi_report_degrades_to_unknown_rows() {
        let client = MockClient::new()
            .stub(DokkuCommand::AppsList, Ok(apps_list_output(&["weird"])))
            .stub(
                DokkuCommand::PsReportAll {
                    apps: vec![app("weird")],
                },
                Ok(DokkuOutput::ok("not a report")),
            );

        let data = dashboard_data(&client).await.expect("dashboard data");

        assert_eq!(data.rows[0].health, AppHealth::Unknown);
        assert_eq!(data.stats.total, 0);
    }

    #[tokio::test]
    async fn partial_report_covers_only_reported_apps() {
        let client = MockClient::new()
            .stub(
                DokkuCommand::AppsList,
                Ok(apps_list_output(&["alpha", "beta"])),
            )
            .stub(
                DokkuCommand::PsReportAll {
                    apps: vec![app("alpha"), app("beta")],
                },
                Ok(multi_report_output(&[section(
                    "alpha",
                    true,
                    true,
                    1,
                    &["web.1: running (CID: a1)"],
                )])),
            );

        let data = dashboard_data(&client).await.expect("dashboard data");

        assert_eq!(data.rows[0].name, "alpha");
        assert_eq!(data.rows[0].health, AppHealth::Running);
        assert_eq!(data.rows[1].name, "beta");
        assert_eq!(data.rows[1].health, AppHealth::Unknown);
        assert_eq!(data.stats.total, 1);
    }

    #[tokio::test]
    async fn invalid_app_name_renders_unknown_without_report_call() {
        let client =
            MockClient::new().stub(DokkuCommand::AppsList, Ok(apps_list_output(&["Bad_Name"])));

        let data = dashboard_data(&client).await.expect("dashboard data");

        assert_eq!(data.rows.len(), 1);
        assert_eq!(data.rows[0].health, AppHealth::Unknown);
        assert_eq!(client.calls(), vec![DokkuCommand::AppsList]);
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
    async fn empty_apps_list_yields_empty_data_and_skips_report() {
        let client = MockClient::new().stub(
            DokkuCommand::AppsList,
            Ok(DokkuOutput::ok("=====> My Apps")),
        );

        let data = dashboard_data(&client).await.expect("dashboard data");

        assert!(data.rows.is_empty());
        assert_eq!(data.stats, AppStats::default());
        assert_eq!(client.calls(), vec![DokkuCommand::AppsList]);
    }

    #[tokio::test]
    async fn many_apps_fetch_in_one_command() {
        let names: Vec<String> = (0..20).map(|i| format!("app{i:02}")).collect();
        let list_output = format!("=====> My Apps\n{}", names.to_vec().join("\n"));
        let sections: Vec<String> = names
            .iter()
            .map(|name| section(name, true, true, 1, &["web.1: running (CID: c)"]))
            .collect();
        let apps: Vec<_> = names.iter().map(|n| app(n)).collect();
        let client = MockClient::new()
            .stub(DokkuCommand::AppsList, Ok(DokkuOutput::ok(list_output)))
            .stub(
                DokkuCommand::PsReportAll { apps },
                Ok(multi_report_output(&sections)),
            );

        let data = dashboard_data(&client).await.expect("dashboard data");

        assert_eq!(data.rows.len(), 20);
        assert_eq!(data.stats.total, 20);
        assert!(data.rows.iter().all(|row| row.health == AppHealth::Running));
        assert_eq!(client.calls().len(), 2);
    }
}
