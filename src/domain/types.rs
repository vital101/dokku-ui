#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PsReport {
    pub deployed: bool,
    pub running: bool,
    pub process_count: i64,
    pub processes: Vec<ProcessStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessStatus {
    pub process_type: String,
    pub state: ProcessState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessState {
    Running,
    Stopped,
    Missing,
    Other(String),
}

impl ProcessState {
    pub fn parse(raw: &str) -> Self {
        let state = raw.split_whitespace().next().unwrap_or("");
        match state {
            "running" | "restarting" => ProcessState::Running,
            "exited" | "stopped" | "dead" | "created" | "paused" => ProcessState::Stopped,
            "missing" => ProcessState::Missing,
            other => ProcessState::Other(other.to_owned()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvVar {
    pub key: String,
    pub value: String,
}

impl EnvVar {
    /// Fixed-length mask for display; leaks nothing about the value or its length.
    pub fn masked_value(&self) -> &'static str {
        if self.value.is_empty() {
            ""
        } else {
            "••••••••"
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AppStats {
    pub total: usize,
    pub running: usize,
    pub stopped: usize,
}

impl AppStats {
    pub fn from_reports(reports: &[PsReport]) -> Self {
        reports.iter().fold(Self::default(), |mut stats, report| {
            stats.total += 1;
            if report.running {
                stats.running += 1;
            } else {
                stats.stopped += 1;
            }
            stats
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppHealth {
    Running,
    Stopped,
    NotDeployed,
    Unknown,
}

impl AppHealth {
    pub fn from_report(report: Option<&PsReport>) -> Self {
        match report {
            None => AppHealth::Unknown,
            Some(report) if report.running => AppHealth::Running,
            Some(report) if report.deployed => AppHealth::Stopped,
            Some(_) => AppHealth::NotDeployed,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AppHealth::Running => "Running",
            AppHealth::Stopped => "Stopped",
            AppHealth::NotDeployed => "Not deployed",
            AppHealth::Unknown => "Unknown",
        }
    }

    pub fn badge_css(self) -> &'static str {
        match self {
            AppHealth::Running => "bg-emerald-500/10 text-emerald-400",
            AppHealth::Stopped => "bg-red-500/10 text-red-400",
            AppHealth::NotDeployed => "bg-slate-500/10 text-slate-400",
            AppHealth::Unknown => "bg-slate-500/10 text-slate-400",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInfo {
    pub name: String,
    pub created_at: String,
    pub locked: bool,
    pub image_status: Option<ImageStatus>,
    pub link_exists: Option<bool>,
    pub dns_record_exists: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageStatus {
    None,
    Built,
    Error(String),
}

impl AppInfo {
    pub fn image_status_label(&self) -> &'static str {
        match &self.image_status {
            None => "unknown",
            Some(ImageStatus::None) => "none",
            Some(ImageStatus::Built) => "built",
            Some(ImageStatus::Error(_)) => "error",
        }
    }

    pub fn link_exists_label(&self) -> &'static str {
        match self.link_exists {
            None => "unknown",
            Some(true) => "yes",
            Some(false) => "no",
        }
    }

    pub fn dns_record_exists_label(&self) -> &'static str {
        match self.dns_record_exists {
            None => "unknown",
            Some(true) => "yes",
            Some(false) => "no",
        }
    }

    pub fn locked_label(&self) -> &'static str {
        match self.locked {
            true => "yes",
            false => "no",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppOverview {
    pub name: String,
    pub app_info: Option<AppInfo>,
    pub ps_report: Option<PsReport>,
    pub health: AppHealth,
    pub process_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLines(Vec<String>);

impl LogLines {
    pub fn new(lines: Vec<String>) -> Self {
        Self(lines)
    }

    pub fn as_slice(&self) -> &[String] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(running: bool, deployed: bool) -> PsReport {
        PsReport {
            deployed,
            running,
            process_count: 1,
            processes: Vec::new(),
        }
    }

    #[test]
    fn process_state_parse_maps_known_states() {
        assert_eq!(
            ProcessState::parse("running (CID: abc)"),
            ProcessState::Running
        );
        assert_eq!(ProcessState::parse("restarting"), ProcessState::Running);
        assert_eq!(
            ProcessState::parse("exited (CID: abc)"),
            ProcessState::Stopped
        );
        assert_eq!(ProcessState::parse("stopped"), ProcessState::Stopped);
        assert_eq!(ProcessState::parse("dead"), ProcessState::Stopped);
        assert_eq!(ProcessState::parse("created"), ProcessState::Stopped);
        assert_eq!(ProcessState::parse("paused"), ProcessState::Stopped);
        assert_eq!(ProcessState::parse("missing"), ProcessState::Missing);
        assert_eq!(
            ProcessState::parse("impossible-state"),
            ProcessState::Other("impossible-state".into())
        );
        assert_eq!(ProcessState::parse(""), ProcessState::Other(String::new()));
    }

    #[test]
    fn app_stats_folds_reports() {
        let reports = vec![
            report(true, true),
            report(true, true),
            report(false, true),
            report(false, false),
        ];
        assert_eq!(
            AppStats::from_reports(&reports),
            AppStats {
                total: 4,
                running: 2,
                stopped: 2,
            }
        );
    }

    #[test]
    fn app_stats_is_empty_for_no_reports() {
        assert_eq!(AppStats::from_reports(&[]), AppStats::default());
    }

    #[test]
    fn app_health_classifies_running_reports() {
        assert_eq!(
            AppHealth::from_report(Some(&report(true, true))),
            AppHealth::Running
        );
        assert_eq!(
            AppHealth::from_report(Some(&report(true, false))),
            AppHealth::Running
        );
    }

    #[test]
    fn app_health_classifies_stopped_and_not_deployed() {
        assert_eq!(
            AppHealth::from_report(Some(&report(false, true))),
            AppHealth::Stopped
        );
        assert_eq!(
            AppHealth::from_report(Some(&report(false, false))),
            AppHealth::NotDeployed
        );
    }

    #[test]
    fn app_health_missing_report_is_unknown() {
        assert_eq!(AppHealth::from_report(None), AppHealth::Unknown);
    }

    #[test]
    fn log_lines_accessors() {
        let lines = LogLines::new(vec!["a".into(), "b".into()]);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines.as_slice(), &["a", "b"]);
        assert!(!lines.is_empty());
        assert!(LogLines::new(Vec::new()).is_empty());
    }

    #[test]
    fn env_var_masked_value_is_fixed_length() {
        assert_eq!(
            EnvVar {
                key: "K".into(),
                value: "short".into()
            }
            .masked_value(),
            "••••••••"
        );
        assert_eq!(
            EnvVar {
                key: "K".into(),
                value: "a-much-longer-secret-value".into()
            }
            .masked_value(),
            "••••••••"
        );
    }

    #[test]
    fn env_var_masked_value_empty_stays_empty() {
        assert_eq!(
            EnvVar {
                key: "K".into(),
                value: String::new()
            }
            .masked_value(),
            ""
        );
    }
}
