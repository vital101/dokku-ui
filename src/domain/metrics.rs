use serde::{Deserialize, Serialize};

use crate::domain::ServiceName;
use crate::domain::ServicePlugin;
use crate::domain::types::{ServiceStats, format_bytes};

const SVG_WIDTH: f64 = 720.0;
const SVG_HEIGHT: f64 = 160.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MetricRange {
    OneHour,
    SixHours,
    TwentyFourHours,
    SevenDays,
    ThirtyDays,
}

impl MetricRange {
    pub const ALL: [Self; 5] = [
        Self::OneHour,
        Self::SixHours,
        Self::TwentyFourHours,
        Self::SevenDays,
        Self::ThirtyDays,
    ];

    pub fn parse(raw: Option<&str>) -> Self {
        match raw.map(str::trim).filter(|raw| !raw.is_empty()) {
            Some("1h") => Self::OneHour,
            Some("6h") => Self::SixHours,
            Some("7d") => Self::SevenDays,
            Some("30d") => Self::ThirtyDays,
            _ => Self::TwentyFourHours,
        }
    }

    pub fn query(self) -> &'static str {
        match self {
            Self::OneHour => "1h",
            Self::SixHours => "6h",
            Self::TwentyFourHours => "24h",
            Self::SevenDays => "7d",
            Self::ThirtyDays => "30d",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::OneHour => "1 hour",
            Self::SixHours => "6 hours",
            Self::TwentyFourHours => "24 hours",
            Self::SevenDays => "7 days",
            Self::ThirtyDays => "30 days",
        }
    }

    pub fn secs(self) -> i64 {
        match self {
            Self::OneHour => 3_600,
            Self::SixHours => 21_600,
            Self::TwentyFourHours => 86_400,
            Self::SevenDays => 604_800,
            Self::ThirtyDays => 2_592_000,
        }
    }

    pub fn width_ms(self) -> i64 {
        match self {
            Self::OneHour => 60_000,
            Self::SixHours => 300_000,
            Self::TwentyFourHours => 900_000,
            Self::SevenDays => 3_600_000,
            Self::ThirtyDays => 10_800_000,
        }
    }

    pub fn start_ms(self, now_ms: i64) -> i64 {
        now_ms.saturating_sub(self.secs().saturating_mul(1_000))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MetricSample {
    pub plugin: String,
    pub service: String,
    pub ts_ms: i64,
    pub bucket_ms: i64,
    pub cpu_percent: Option<f64>,
    pub cpu_total_usec: Option<i64>,
    pub cpus: Option<i64>,
    pub mem_used_bytes: Option<i64>,
    pub mem_limit_bytes: Option<i64>,
    pub host_mem_total_kb: Option<i64>,
    pub fs_total_kb: Option<i64>,
    pub fs_used_kb: Option<i64>,
    pub fs_avail_kb: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DataSizeSample {
    pub plugin: String,
    pub service: String,
    pub ts_ms: i64,
    pub bucket_ms: i64,
    pub data_kb: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MetricAggregate {
    pub ts_ms: i64,
    pub cpu_percent: Option<f64>,
    pub memory_percent: Option<f64>,
    pub disk_percent: Option<f64>,
    pub fs_used_bytes: Option<f64>,
    pub fs_total_bytes: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DataSizeAggregate {
    pub ts_ms: i64,
    pub data_bytes: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MetricPoint {
    pub ts_ms: i64,
    pub value: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChartKind {
    Percent,
    Bytes,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChartView {
    pub title: String,
    pub latest_label: String,
    pub empty: bool,
    pub svg: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MetricsView {
    pub charts: Vec<ChartView>,
    pub empty: bool,
}

pub fn align_bucket(ts_ms: i64, interval_ms: i64) -> i64 {
    let interval = interval_ms.max(1);
    ts_ms - ts_ms.rem_euclid(interval)
}

pub fn sample_from_stats(
    plugin: ServicePlugin,
    service: &ServiceName,
    stats: &ServiceStats,
    ts_ms: i64,
    bucket_ms: i64,
) -> MetricSample {
    MetricSample {
        plugin: plugin.to_string(),
        service: service.to_string(),
        ts_ms,
        bucket_ms,
        cpu_percent: finite_percent(stats.cpu_percent()),
        cpu_total_usec: i64_opt(stats.cpu_total_usec),
        cpus: i64_opt(stats.cpus),
        mem_used_bytes: i64_opt(stats.memory_working_set_bytes()),
        mem_limit_bytes: i64_opt(stats.memory_limit_bytes),
        host_mem_total_kb: i64_opt(stats.host_memory_total_kb),
        fs_total_kb: i64_opt(stats.fs_total_kb),
        fs_used_kb: i64_opt(stats.fs_used_kb),
        fs_avail_kb: i64_opt(stats.fs_avail_kb),
    }
}

pub fn data_size_sample(
    plugin: ServicePlugin,
    service: &ServiceName,
    data_kb: u64,
    ts_ms: i64,
    bucket_ms: i64,
) -> DataSizeSample {
    DataSizeSample {
        plugin: plugin.to_string(),
        service: service.to_string(),
        ts_ms,
        bucket_ms,
        data_kb: i64_from_u64(data_kb),
    }
}

pub fn build_metrics_view(
    aggregates: &[MetricAggregate],
    data: &[DataSizeAggregate],
    range: MetricRange,
    now_ms: i64,
) -> MetricsView {
    let charts = vec![
        build_chart(
            "CPU",
            &percent_points(aggregates, |row| row.cpu_percent),
            range,
            now_ms,
            "stroke-emerald-500",
        ),
        build_chart(
            "Memory",
            &percent_points(aggregates, |row| row.memory_percent),
            range,
            now_ms,
            "stroke-sky-500",
        ),
        build_chart(
            "Disk",
            &percent_points(aggregates, |row| row.disk_percent),
            range,
            now_ms,
            "stroke-amber-500",
        ),
        build_chart(
            "Data",
            &data_points(data),
            range,
            now_ms,
            "stroke-violet-500",
        ),
    ];

    let empty = charts.iter().all(|chart| chart.empty);
    MetricsView { charts, empty }
}

fn percent_points(
    aggregates: &[MetricAggregate],
    pick: impl Fn(&MetricAggregate) -> Option<f64>,
) -> Vec<MetricPoint> {
    aggregates
        .iter()
        .filter_map(|row| {
            pick(row)
                .and_then(|value| finite_percent(Some(value)))
                .map(|value| MetricPoint {
                    ts_ms: row.ts_ms,
                    value,
                })
        })
        .collect()
}

fn data_points(data: &[DataSizeAggregate]) -> Vec<MetricPoint> {
    data.iter()
        .filter_map(|row| {
            row.data_bytes
                .filter(|value| value.is_finite())
                .map(|value| MetricPoint {
                    ts_ms: row.ts_ms,
                    value: value.max(0.0),
                })
        })
        .collect()
}

fn build_chart(
    title: &str,
    points: &[MetricPoint],
    range: MetricRange,
    now_ms: i64,
    stroke: &str,
) -> ChartView {
    let kind = if title == "Data" {
        ChartKind::Bytes
    } else {
        ChartKind::Percent
    };
    let latest_label = latest_label(points, kind);
    if points.is_empty() {
        return ChartView {
            title: title.to_owned(),
            latest_label,
            empty: true,
            svg: String::new(),
        };
    }

    let max = match kind {
        ChartKind::Percent => 100.0,
        ChartKind::Bytes => points
            .iter()
            .map(|point| point.value)
            .fold(0.0_f64, f64::max)
            .max(1.0),
    };
    let start = range.start_ms(now_ms);
    let end = now_ms.max(start.saturating_add(1));
    let duration = (end - start) as f64;
    let mut path = String::new();
    for (idx, point) in points.iter().enumerate() {
        let x = ((point.ts_ms - start).max(0) as f64 / duration).min(1.0) * SVG_WIDTH;
        let y = SVG_HEIGHT - (point.value / max).clamp(0.0, 1.0) * SVG_HEIGHT;
        if points.len() == 1 {
            let x_left = (x - 1.0).max(0.0);
            let x_right = (x + 1.0).min(SVG_WIDTH);
            path.push_str(&format!("M {x_left:.2} {y:.2} L {x_right:.2} {y:.2}"));
            break;
        }
        if idx == 0 {
            path.push_str(&format!("M {x:.2} {y:.2}"));
        } else {
            path.push_str(&format!(" L {x:.2} {y:.2}"));
        }
    }

    let max_label = format_chart_value(max, kind);
    let svg = format!(
        r#"<svg viewBox="0 0 {svg_width:.0} {svg_height:.0}" class="h-36 w-full" role="img" aria-label="{title} over {range_label}"><path class="fill-none stroke-slate-800" stroke-width="1" d="M 0 {svg_height:.2} H {svg_width:.2}" /><path class="fill-none {stroke} stroke-2" stroke-width="2" d="{path}" /><text class="fill-slate-500" x="8" y="14">{max_label}</text><text class="fill-slate-500" x="8" y="154">0</text></svg>"#,
        svg_width = SVG_WIDTH,
        svg_height = SVG_HEIGHT,
        range_label = range.label(),
    );

    ChartView {
        title: title.to_owned(),
        latest_label,
        empty: false,
        svg,
    }
}

fn latest_label(points: &[MetricPoint], kind: ChartKind) -> String {
    points
        .last()
        .map(|point| format_chart_value(point.value, kind))
        .unwrap_or_else(|| "—".to_owned())
}

fn format_chart_value(value: f64, kind: ChartKind) -> String {
    match kind {
        ChartKind::Percent => format!("{value:.1}%"),
        ChartKind::Bytes => format_bytes(value.max(0.0) as u64),
    }
}

fn finite_percent(value: Option<f64>) -> Option<f64> {
    value
        .filter(|value| value.is_finite())
        .map(|value| value.clamp(0.0, 100.0))
}

fn i64_opt(value: Option<u64>) -> Option<i64> {
    value.map(i64_from_u64)
}

fn i64_from_u64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::ServiceStats;

    fn stats() -> ServiceStats {
        ServiceStats {
            host_memory_total_kb: Some(8_388_608),
            memory_current_bytes: Some(1_048_576),
            inactive_file_bytes: Some(104_857),
            cpu_delta_usec: Some(10_000),
            elapsed_secs: Some(1.0),
            cpus: Some(2),
            cpu_total_usec: Some(1_000_000),
            fs_total_kb: Some(100),
            fs_used_kb: Some(25),
            fs_avail_kb: Some(75),
            ..ServiceStats::default()
        }
    }

    #[test]
    fn range_parse_defaults_bad_values_to_24h() {
        assert_eq!(MetricRange::parse(Some("1h")), MetricRange::OneHour);
        assert_eq!(MetricRange::parse(Some("7d")), MetricRange::SevenDays);
        assert_eq!(
            MetricRange::parse(Some("soon")),
            MetricRange::TwentyFourHours
        );
        assert_eq!(MetricRange::parse(None), MetricRange::TwentyFourHours);
        assert_eq!(MetricRange::parse(Some("")).query(), "24h");
    }

    #[test]
    fn range_start_and_width_match_labels() {
        let range = MetricRange::SevenDays;
        assert_eq!(range.secs(), 604_800);
        assert_eq!(range.width_ms(), 3_600_000);
        assert_eq!(range.start_ms(604_830_000), 30_000);
        assert_eq!(range.start_ms(700_000), -604_100_000);
    }

    #[test]
    fn align_bucket_snaps_to_interval() {
        assert_eq!(align_bucket(123_456, 60_000), 120_000);
        assert_eq!(align_bucket(-5, 60_000), -60_000);
        assert_eq!(align_bucket(10, 0), 10);
    }

    #[test]
    fn sample_from_stats_clamps_percent_and_converts_u64() {
        let plugin = ServicePlugin::try_from("redis").expect("redis plugin");
        let service = ServiceName::try_from("candid").expect("service");
        let sample = sample_from_stats(plugin, &service, &stats(), 1_234, 1_200);

        assert_eq!(sample.plugin, "redis");
        assert_eq!(sample.service, "candid");
        assert_eq!(sample.bucket_ms, 1_200);
        assert_eq!(sample.cpu_percent, Some(0.5));
        assert_eq!(sample.cpu_total_usec, Some(1_000_000));
        assert_eq!(sample.mem_used_bytes, Some(943_719));
        assert_eq!(sample.host_mem_total_kb, Some(8_388_608));
    }

    #[test]
    fn empty_metrics_view_has_no_charts_with_data() {
        let view = build_metrics_view(&[], &[], MetricRange::OneHour, 100_000);

        assert!(view.empty);
        assert_eq!(view.charts.len(), 4);
        assert!(view.charts.iter().all(|chart| chart.empty));
    }

    #[test]
    fn percent_chart_draws_scaled_path() {
        let aggregates = [
            MetricAggregate {
                ts_ms: 1_000_000,
                cpu_percent: Some(25.0),
                memory_percent: Some(50.0),
                disk_percent: None,
                fs_used_bytes: Some(1024.0),
                fs_total_bytes: Some(4096.0),
            },
            MetricAggregate {
                ts_ms: 3_400_000,
                cpu_percent: Some(100.0),
                memory_percent: Some(0.0),
                disk_percent: None,
                fs_used_bytes: None,
                fs_total_bytes: Some(4096.0),
            },
        ];
        let view = build_metrics_view(&aggregates, &[], MetricRange::OneHour, 4_600_000);

        assert!(!view.charts[0].empty);
        assert_eq!(view.charts[0].latest_label, "100.0%");
        assert!(view.charts[0].svg.contains("stroke-emerald-500"));
        assert!(view.charts[0].svg.contains("M 0.00 120.00 L 480.00 0.00"));
        assert_eq!(view.charts[1].latest_label, "0.0%");
        assert!(view.charts[2].empty);
        assert!(view.charts[3].empty);
    }

    #[test]
    fn data_chart_uses_byte_labels_and_single_point_segment() {
        let data = [DataSizeAggregate {
            ts_ms: 4_600_000,
            data_bytes: Some(12.0 * 1024.0),
        }];
        let view = build_metrics_view(&[], &data, MetricRange::OneHour, 4_600_000);

        assert_eq!(view.charts[3].latest_label, "12 KiB");
        assert!(!view.charts[3].empty);
        assert!(view.charts[3].svg.contains("stroke-violet-500"));
        assert!(view.charts[3].svg.contains("M 719.00 0.00 L 720.00 0.00"));
    }
}
