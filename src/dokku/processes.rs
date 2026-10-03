use std::collections::HashMap;

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::domain::types::{ContainerDetails, ProcessState, PsReport, ScaleEntry};

/// Upper bound for a single process type's scale, a fat-finger guard rather than
/// a dokku limit.
pub const SCALE_MAX: u32 = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessRow {
    pub process_type: String,
    pub desired: Option<u32>,
    pub running: usize,
    pub stopped: usize,
}

impl ProcessRow {
    pub fn desired_label(&self) -> String {
        match self.desired {
            Some(quantity) => quantity.to_string(),
            None => "—".to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerRow {
    pub id_short: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub uptime: String,
    pub restart_count: i64,
    pub oom_killed: bool,
    pub exit_code: i64,
}

/// The process type without its instance index: `web.1` -> `web`.
pub fn base_process_type(process_type: &str) -> &str {
    process_type.split('.').next().unwrap_or(process_type)
}

/// Merges the desired formation (`ps:scale --format json`) with the observed
/// process states (`ps:report`) into one row per process type. Types that are
/// running but absent from the formation (e.g. managed via `app.json`) are
/// appended with an unknown desired count.
pub fn formation_rows(formation: &[ScaleEntry], ps_report: Option<&PsReport>) -> Vec<ProcessRow> {
    let mut rows: Vec<ProcessRow> = formation
        .iter()
        .map(|entry| ProcessRow {
            process_type: entry.process_type.clone(),
            desired: Some(entry.quantity),
            running: 0,
            stopped: 0,
        })
        .collect();

    let Some(report) = ps_report else {
        return rows;
    };

    for status in &report.processes {
        let base = base_process_type(&status.process_type);
        let row = match rows.iter_mut().find(|row| row.process_type == base) {
            Some(row) => row,
            None => {
                rows.push(ProcessRow {
                    process_type: base.to_owned(),
                    desired: None,
                    running: 0,
                    stopped: 0,
                });
                rows.last_mut().expect("just pushed")
            }
        };
        match status.state {
            ProcessState::Running => row.running += 1,
            ProcessState::Missing => {}
            _ => row.stopped += 1,
        }
    }
    rows
}

/// Converts fetched container details into display rows, computing uptime
/// against `now`.
pub fn container_rows(containers: &[ContainerDetails], now: OffsetDateTime) -> Vec<ContainerRow> {
    containers
        .iter()
        .map(|container| ContainerRow {
            id_short: container.id_short.clone(),
            name: container.name.clone(),
            image: container.image.clone(),
            state: container.state.clone(),
            uptime: format_uptime(&container.started_at, now),
            restart_count: container.restart_count,
            oom_killed: container.oom_killed,
            exit_code: container.exit_code,
        })
        .collect()
}

/// Human-friendly uptime for a container started at an RFC3339 timestamp.
pub fn format_uptime(started_at: &str, now: OffsetDateTime) -> String {
    if started_at.is_empty() || started_at.starts_with("0001-01-01") {
        return "—".to_owned();
    }
    let Ok(started) = OffsetDateTime::parse(started_at, &Rfc3339) else {
        return "—".to_owned();
    };
    let secs = (now - started).whole_seconds();
    if secs < 0 {
        return "—".to_owned();
    }
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ScaleFormError {
    #[error("scale value for `{0}` must be a whole number")]
    NotANumber(String),
    #[error("scale value for `{0}` must be between 0 and {max}", max = SCALE_MAX)]
    OutOfRange(String),
    #[error("no scale values submitted")]
    Empty,
}

/// Form field name for a process type's desired count.
pub fn scale_field_name(process_type: &str) -> String {
    format!("scale_{process_type}")
}

/// Validates a submitted scale form against the app's known process types.
/// Unknown fields (including the CSRF token) are ignored; only process types
/// present in `formation` are accepted, and a missing field leaves that type
/// unchanged.
pub fn parse_scale_form(
    formation: &[ScaleEntry],
    fields: &HashMap<String, String>,
) -> Result<Vec<ScaleEntry>, ScaleFormError> {
    let mut entries = Vec::new();
    for entry in formation {
        let Some(raw) = fields.get(&scale_field_name(&entry.process_type)) else {
            continue;
        };
        let raw = raw.trim();
        let Ok(quantity) = raw.parse::<u32>() else {
            return Err(ScaleFormError::NotANumber(entry.process_type.clone()));
        };
        if quantity > SCALE_MAX {
            return Err(ScaleFormError::OutOfRange(entry.process_type.clone()));
        }
        entries.push(ScaleEntry::new(entry.process_type.clone(), quantity));
    }
    if entries.is_empty() {
        return Err(ScaleFormError::Empty);
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::ProcessStatus;

    fn report(processes: Vec<(&str, ProcessState)>) -> PsReport {
        PsReport {
            deployed: true,
            running: true,
            process_count: processes.len() as i64,
            processes: processes
                .into_iter()
                .map(|(process_type, state)| ProcessStatus {
                    process_type: process_type.to_owned(),
                    state,
                })
                .collect(),
            can_scale: Some(true),
        }
    }

    #[test]
    fn base_process_type_strips_instance_index() {
        assert_eq!(base_process_type("web.1"), "web");
        assert_eq!(base_process_type("worker-2.10"), "worker-2");
        assert_eq!(base_process_type("web"), "web");
    }

    #[test]
    fn formation_rows_merge_desired_and_observed() {
        let formation = vec![ScaleEntry::new("web", 2), ScaleEntry::new("worker", 1)];
        let report = report(vec![
            ("web.1", ProcessState::Running),
            ("web.2", ProcessState::Running),
            ("worker.1", ProcessState::Stopped),
        ]);

        let rows = formation_rows(&formation, Some(&report));

        assert_eq!(
            rows,
            vec![
                ProcessRow {
                    process_type: "web".into(),
                    desired: Some(2),
                    running: 2,
                    stopped: 0,
                },
                ProcessRow {
                    process_type: "worker".into(),
                    desired: Some(1),
                    running: 0,
                    stopped: 1,
                },
            ]
        );
    }

    #[test]
    fn formation_rows_keep_zero_scaled_types() {
        let formation = vec![ScaleEntry::new("web", 1), ScaleEntry::new("worker", 0)];
        let rows = formation_rows(&formation, None);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].desired, Some(0));
        assert_eq!(rows[1].desired_label(), "0");
    }

    #[test]
    fn formation_rows_append_unlisted_running_types() {
        let report = report(vec![("cron.1", ProcessState::Running)]);
        let rows = formation_rows(&[], Some(&report));

        assert_eq!(
            rows,
            vec![ProcessRow {
                process_type: "cron".into(),
                desired: None,
                running: 1,
                stopped: 0,
            }]
        );
        assert_eq!(rows[0].desired_label(), "—");
    }

    #[test]
    fn formation_rows_ignore_missing_state() {
        let formation = vec![ScaleEntry::new("web", 1)];
        let report = report(vec![("web", ProcessState::Missing)]);
        let rows = formation_rows(&formation, Some(&report));

        assert_eq!(rows[0].running, 0);
        assert_eq!(rows[0].stopped, 0);
    }

    #[test]
    fn container_rows_compute_uptime() {
        let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("now");
        let started = (now - time::Duration::hours(2))
            .format(&Rfc3339)
            .expect("format");
        let containers = vec![ContainerDetails {
            id_short: "abc".into(),
            name: "alpha.web.1".into(),
            image: "img".into(),
            state: "running".into(),
            started_at: started,
            restart_count: 1,
            oom_killed: true,
            exit_code: 137,
        }];

        let rows = container_rows(&containers, now);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "alpha.web.1");
        assert_eq!(rows[0].uptime, "2h");
        assert!(rows[0].oom_killed);
        assert_eq!(rows[0].restart_count, 1);
        assert_eq!(rows[0].exit_code, 137);
    }

    #[test]
    fn format_uptime_buckets() {
        let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("now");
        let at = |secs: i64| {
            (now - time::Duration::seconds(secs))
                .format(&Rfc3339)
                .expect("format")
        };
        assert_eq!(format_uptime(&at(0), now), "0s");
        assert_eq!(format_uptime(&at(30), now), "30s");
        assert_eq!(format_uptime(&at(60), now), "1m");
        assert_eq!(format_uptime(&at(3599), now), "59m");
        assert_eq!(format_uptime(&at(3600), now), "1h");
        assert_eq!(format_uptime(&at(86_399), now), "23h");
        assert_eq!(format_uptime(&at(86_400), now), "1d");
    }

    #[test]
    fn format_uptime_handles_unusable_timestamps() {
        let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("now");
        assert_eq!(format_uptime("", now), "—");
        assert_eq!(format_uptime("0001-01-01T00:00:00Z", now), "—");
        assert_eq!(format_uptime("not-a-date", now), "—");
        assert_eq!(
            format_uptime("2028-01-01T00:00:00Z", now),
            "—",
            "future start"
        );
    }

    #[test]
    fn parse_scale_form_reads_known_types() {
        let formation = vec![ScaleEntry::new("web", 1), ScaleEntry::new("worker", 2)];
        let fields = HashMap::from([
            ("scale_web".to_owned(), "3".to_owned()),
            ("scale_worker".to_owned(), "0".to_owned()),
        ]);

        assert_eq!(
            parse_scale_form(&formation, &fields),
            Ok(vec![
                ScaleEntry::new("web", 3),
                ScaleEntry::new("worker", 0)
            ])
        );
    }

    #[test]
    fn parse_scale_form_ignores_unknown_and_missing_fields() {
        let formation = vec![ScaleEntry::new("web", 1), ScaleEntry::new("worker", 1)];
        let fields = HashMap::from([
            ("csrf_token".to_owned(), "abc".to_owned()),
            ("scale_web".to_owned(), "2".to_owned()),
            ("scale_evil".to_owned(), "9".to_owned()),
        ]);

        assert_eq!(
            parse_scale_form(&formation, &fields),
            Ok(vec![ScaleEntry::new("web", 2)])
        );
    }

    #[test]
    fn parse_scale_form_rejects_bad_values() {
        let formation = vec![ScaleEntry::new("web", 1)];
        let not_a_number = HashMap::from([("scale_web".to_owned(), "lots".to_owned())]);
        assert_eq!(
            parse_scale_form(&formation, &not_a_number),
            Err(ScaleFormError::NotANumber("web".into()))
        );

        let too_big = HashMap::from([("scale_web".to_owned(), "101".to_owned())]);
        assert_eq!(
            parse_scale_form(&formation, &too_big),
            Err(ScaleFormError::OutOfRange("web".into()))
        );
    }

    #[test]
    fn parse_scale_form_rejects_empty_submission() {
        let formation = vec![ScaleEntry::new("web", 1)];
        assert_eq!(
            parse_scale_form(&formation, &HashMap::new()),
            Err(ScaleFormError::Empty)
        );
        assert_eq!(
            parse_scale_form(&[], &HashMap::new()),
            Err(ScaleFormError::Empty)
        );
    }
}
