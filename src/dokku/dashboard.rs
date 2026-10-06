use crate::domain::types::{AppHealth, AppStats, PsReport};

use super::snapshot::Snapshot;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppRow {
    pub name: String,
    pub health: AppHealth,
    pub process_count: i64,
    /// An unfinished destroy run targets this app.
    pub deleting: bool,
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

/// Pure assembly of the dashboard from an already-warm snapshot. No IO.
pub fn dashboard_from_snapshot(snapshot: &Snapshot) -> DashboardData {
    dashboard_from_snapshot_filtered(snapshot, |_| false)
}

/// Same assembly, minus apps the instance filter hides.
pub fn dashboard_from_snapshot_filtered(
    snapshot: &Snapshot,
    hide: impl Fn(&str) -> bool,
) -> DashboardData {
    let rows: Vec<AppRow> = snapshot
        .apps
        .iter()
        .filter(|name| !hide(name))
        .map(|name| {
            let report = snapshot.ps_report(name);
            AppRow {
                name: name.clone(),
                process_count: report.map(|r| r.process_count).unwrap_or(-1),
                health: AppHealth::from_report(report),
                deleting: false,
            }
        })
        .collect();

    let reports: Vec<PsReport> = snapshot
        .apps
        .iter()
        .filter(|name| !hide(name))
        .filter_map(|name| snapshot.ps_report(name).cloned())
        .collect();
    let stats = AppStats::from_reports(&reports);

    DashboardData { rows, stats }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::domain::types::PsReport;

    fn ps_report(running: bool, deployed: bool, process_count: i64) -> PsReport {
        PsReport {
            deployed,
            running,
            process_count,
            processes: Vec::new(),
            can_scale: None,
        }
    }

    fn snapshot(apps: &[&str], reports: &[(&str, Option<PsReport>)]) -> Snapshot {
        Snapshot {
            fetched_at: Instant::now(),
            apps: apps.iter().map(|name| (*name).to_owned()).collect(),
            ps_reports: reports
                .iter()
                .map(|(name, report)| ((*name).to_owned(), report.clone()))
                .collect(),
            apps_reports: Default::default(),
        }
    }

    #[test]
    fn happy_path_assembles_rows_and_stats() {
        let snapshot = snapshot(
            &["alpha", "beta", "gamma"],
            &[
                ("alpha", Some(ps_report(true, true, 2))),
                ("beta", Some(ps_report(false, true, 1))),
                ("gamma", Some(ps_report(false, false, 0))),
            ],
        );

        let data = dashboard_from_snapshot(&snapshot);

        assert_eq!(
            data.rows,
            vec![
                AppRow {
                    name: "alpha".into(),
                    health: AppHealth::Running,
                    process_count: 2,
                    deleting: false,
                },
                AppRow {
                    name: "beta".into(),
                    health: AppHealth::Stopped,
                    process_count: 1,
                    deleting: false,
                },
                AppRow {
                    name: "gamma".into(),
                    health: AppHealth::NotDeployed,
                    process_count: 0,
                    deleting: false,
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
    }

    #[test]
    fn missing_report_degrades_to_unknown_row() {
        let snapshot = snapshot(
            &["bad", "good"],
            &[("bad", None), ("good", Some(ps_report(true, true, 1)))],
        );

        let data = dashboard_from_snapshot(&snapshot);

        assert_eq!(
            data.rows,
            vec![
                AppRow {
                    name: "bad".into(),
                    health: AppHealth::Unknown,
                    process_count: -1,
                    deleting: false,
                },
                AppRow {
                    name: "good".into(),
                    health: AppHealth::Running,
                    process_count: 1,
                    deleting: false,
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

    #[test]
    fn unparseable_and_invalid_names_degrade_without_panicking() {
        let snapshot = snapshot(
            &["weird", "Bad_Name"],
            &[("weird", None), ("Bad_Name", None)],
        );

        let data = dashboard_from_snapshot(&snapshot);

        assert!(data.rows.iter().all(|row| row.health == AppHealth::Unknown));
        assert_eq!(data.stats, AppStats::default());
    }

    #[test]
    fn empty_snapshot_yields_empty_data() {
        let data = dashboard_from_snapshot(&snapshot(&[], &[]));

        assert!(data.rows.is_empty());
        assert_eq!(data.stats, AppStats::default());
    }

    #[test]
    fn app_filter_hides_rows_and_recomputes_stats() {
        let snapshot = snapshot(
            &["alpha", "dokku-ui"],
            &[
                ("alpha", Some(ps_report(true, true, 1))),
                ("dokku-ui", Some(ps_report(true, true, 2))),
            ],
        );

        let data = dashboard_from_snapshot_filtered(&snapshot, |name| name == "dokku-ui");

        assert_eq!(data.rows.len(), 1);
        assert_eq!(data.rows[0].name, "alpha");
        assert_eq!(
            data.stats,
            AppStats {
                total: 1,
                running: 1,
                stopped: 0,
            }
        );
    }

    #[test]
    fn many_apps_are_all_included() {
        let names: Vec<String> = (0..20).map(|i| format!("app{i:02}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let reports: Vec<(&str, Option<PsReport>)> = refs
            .iter()
            .map(|name| (*name, Some(ps_report(true, true, 1))))
            .collect();
        let snapshot = snapshot(&refs, &reports);

        let data = dashboard_from_snapshot(&snapshot);

        assert_eq!(data.rows.len(), 20);
        assert_eq!(data.stats.total, 20);
        assert!(data.rows.iter().all(|row| row.health == AppHealth::Running));
    }
}
