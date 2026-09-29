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
    fn log_lines_accessors() {
        let lines = LogLines::new(vec!["a".into(), "b".into()]);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines.as_slice(), &["a", "b"]);
        assert!(!lines.is_empty());
        assert!(LogLines::new(Vec::new()).is_empty());
    }
}
