use crate::domain::types::{AppHealth, AppOverview};

use super::snapshot::Snapshot;

#[derive(Debug, thiserror::Error)]
pub enum OverviewError {
    #[error("app `{0}` was not found")]
    AppNotFound(String),
}

/// Pure assembly of an app overview from an already-warm snapshot. No IO.
pub fn overview_from_snapshot(
    snapshot: &Snapshot,
    name: &str,
) -> Result<AppOverview, OverviewError> {
    if !snapshot.contains(name) {
        return Err(OverviewError::AppNotFound(name.to_owned()));
    }

    let ps_report = snapshot.ps_report(name).cloned();
    let app_info = snapshot.app_info(name).cloned();
    let health = AppHealth::from_report(ps_report.as_ref());
    let process_count = ps_report.as_ref().map(|r| r.process_count).unwrap_or(-1);

    Ok(AppOverview {
        name: name.to_owned(),
        app_info,
        ps_report,
        health,
        process_count,
    })
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::domain::types::{AppInfo, PsReport};

    fn ps_report(running: bool, deployed: bool, process_count: i64) -> PsReport {
        PsReport {
            deployed,
            running,
            process_count,
            processes: Vec::new(),
        }
    }

    fn app_info() -> AppInfo {
        AppInfo {
            name: "alpha".into(),
            created_at: "2026-10-03 10:36 UTC".into(),
            locked: false,
            image_status: None,
            link_exists: None,
            dns_record_exists: None,
        }
    }

    fn snapshot(
        names: &[&str],
        reports: Vec<(&str, Option<PsReport>)>,
        infos: Vec<(&str, Option<AppInfo>)>,
    ) -> Snapshot {
        Snapshot {
            fetched_at: Instant::now(),
            apps: names.iter().map(|name| (*name).to_owned()).collect(),
            ps_reports: reports
                .into_iter()
                .map(|(name, report)| (name.to_owned(), report))
                .collect(),
            apps_reports: infos
                .into_iter()
                .map(|(name, info)| (name.to_owned(), info))
                .collect(),
        }
    }

    #[test]
    fn happy_path_assembles_overview() {
        let snapshot = snapshot(
            &["alpha"],
            vec![("alpha", Some(ps_report(true, true, 2)))],
            vec![("alpha", Some(app_info()))],
        );

        let overview = overview_from_snapshot(&snapshot, "alpha").expect("overview");

        assert_eq!(overview.name, "alpha");
        assert_eq!(overview.health, AppHealth::Running);
        assert_eq!(overview.process_count, 2);
        let info = overview.app_info.expect("app_info");
        assert!(!info.locked);
        assert_eq!(info.created_at, "2026-10-03 10:36 UTC");
    }

    #[test]
    fn unknown_app_yields_not_found() {
        let snapshot = snapshot(&["alpha"], vec![], vec![]);

        assert!(matches!(
            overview_from_snapshot(&snapshot, "nope"),
            Err(OverviewError::AppNotFound(_))
        ));
    }

    #[test]
    fn missing_reports_degrade() {
        let snapshot = snapshot(&["alpha"], vec![("alpha", None)], vec![("alpha", None)]);

        let overview = overview_from_snapshot(&snapshot, "alpha").expect("overview");

        assert!(overview.app_info.is_none());
        assert_eq!(overview.health, AppHealth::Unknown);
        assert_eq!(overview.process_count, -1);
    }

    #[test]
    fn stopped_app_is_classified() {
        let snapshot = snapshot(
            &["alpha"],
            vec![("alpha", Some(ps_report(false, true, 0)))],
            vec![],
        );

        let overview = overview_from_snapshot(&snapshot, "alpha").expect("overview");

        assert_eq!(overview.health, AppHealth::Stopped);
    }
}
