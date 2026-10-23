use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::domain::ServiceName;
use crate::domain::ServicePlugin;
use crate::domain::types::{ServiceStats, format_bytes};

const SVG_WIDTH: f64 = 720.0;
const SVG_HEIGHT: f64 = 180.0;
const PLOT_LEFT: f64 = 4.0;
const PLOT_RIGHT: f64 = 4.0;
const PLOT_TOP: f64 = 4.0;
const PLOT_BOTTOM: f64 = 4.0;
const HEADROOM: f64 = 1.15;
const BYTE_FLOOR: f64 = 1024.0;

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

    /// Start of the x-axis, relative to now. Kept timezone-free because the
    /// server and the viewer can sit in different zones.
    pub fn start_caption(self) -> &'static str {
        match self {
            Self::OneHour => "-1h",
            Self::SixHours => "-6h",
            Self::TwentyFourHours => "-24h",
            Self::SevenDays => "-7d",
            Self::ThirtyDays => "-30d",
        }
    }

    pub fn mid_caption(self) -> &'static str {
        match self {
            Self::OneHour => "-30m",
            Self::SixHours => "-3h",
            Self::TwentyFourHours => "-12h",
            Self::SevenDays => "-3.5d",
            Self::ThirtyDays => "-15d",
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChartAccent {
    Cpu,
    Memory,
    Disk,
    Data,
}

impl ChartAccent {
    fn key(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Memory => "memory",
            Self::Disk => "disk",
            Self::Data => "data",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ChartSpec {
    title: &'static str,
    kind: ChartKind,
    accent: ChartAccent,
    /// Smallest axis ceiling, so a nearly idle service still shows movement
    /// without scaling every pixel of noise up to the full chart height.
    floor: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChartView {
    pub key: &'static str,
    pub title: String,
    pub latest_label: String,
    pub peak_label: String,
    pub average_label: String,
    pub updated_label: String,
    pub axis_max_label: String,
    pub axis_mid_label: String,
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
            ChartSpec {
                title: "CPU",
                kind: ChartKind::Percent,
                accent: ChartAccent::Cpu,
                floor: 2.0,
            },
            &percent_points(aggregates, |row| row.cpu_percent),
            range,
            now_ms,
        ),
        build_chart(
            ChartSpec {
                title: "Memory",
                kind: ChartKind::Percent,
                accent: ChartAccent::Memory,
                floor: 10.0,
            },
            &percent_points(aggregates, |row| row.memory_percent),
            range,
            now_ms,
        ),
        build_chart(
            ChartSpec {
                title: "Disk",
                kind: ChartKind::Percent,
                accent: ChartAccent::Disk,
                floor: 10.0,
            },
            &percent_points(aggregates, |row| row.disk_percent),
            range,
            now_ms,
        ),
        build_chart(
            ChartSpec {
                title: "Data",
                kind: ChartKind::Bytes,
                accent: ChartAccent::Data,
                floor: BYTE_FLOOR,
            },
            &data_points(data),
            range,
            now_ms,
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
    spec: ChartSpec,
    points: &[MetricPoint],
    range: MetricRange,
    now_ms: i64,
) -> ChartView {
    let latest_label = latest_label(points, spec.kind);
    let peak = points
        .iter()
        .map(|point| point.value)
        .fold(0.0_f64, f64::max);
    let average_label = if points.is_empty() {
        "—".to_owned()
    } else {
        let sum: f64 = points.iter().map(|point| point.value).sum();
        format_chart_value(sum / points.len() as f64, spec.kind)
    };
    let peak_label = if points.is_empty() {
        "—".to_owned()
    } else {
        format_chart_value(peak, spec.kind)
    };
    let updated_label = updated_label(now_ms, points.last().map(|point| point.ts_ms));

    if points.is_empty() {
        return ChartView {
            key: spec.accent.key(),
            title: spec.title.to_owned(),
            latest_label,
            peak_label,
            average_label,
            updated_label,
            axis_max_label: format_chart_value(spec.floor, spec.kind),
            axis_mid_label: format_chart_value(spec.floor / 2.0, spec.kind),
            empty: true,
            svg: String::new(),
        };
    }

    let axis_max = axis_max(peak, spec.kind, spec.floor);
    let axis_max_label = format_chart_value(axis_max, spec.kind);
    let axis_mid_label = format_chart_value(axis_max / 2.0, spec.kind);
    let start = range.start_ms(now_ms);
    let end = now_ms.max(start.saturating_add(1));
    let duration = (end - start) as f64;
    let x_left = PLOT_LEFT;
    let x_right = SVG_WIDTH - PLOT_RIGHT;
    let y_top = PLOT_TOP;
    let y_bottom = SVG_HEIGHT - PLOT_BOTTOM;
    let plot_height = y_bottom - y_top;
    let map_point = |point: &MetricPoint| {
        let x =
            x_left + ((point.ts_ms - start).max(0) as f64 / duration).min(1.0) * (x_right - x_left);
        let ratio = if axis_max > 0.0 {
            (point.value / axis_max).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let y = y_top + (1.0 - ratio) * plot_height;
        (x, y)
    };

    let gap_ms = (range.width_ms().saturating_mul(2)).max(1);
    let mut segments: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut current: Vec<(f64, f64)> = Vec::new();
    let mut previous_ts: Option<i64> = None;
    for point in points {
        if let Some(previous) = previous_ts {
            if point.ts_ms.saturating_sub(previous) > gap_ms && !current.is_empty() {
                segments.push(std::mem::take(&mut current));
            }
        }
        current.push(map_point(point));
        previous_ts = Some(point.ts_ms);
    }
    if !current.is_empty() {
        segments.push(current);
    }

    let mut svg = String::new();
    let _ = write!(
        svg,
        r#"<svg viewBox="0 0 {width:.0} {height:.0}" preserveAspectRatio="none" class="metric-chart-svg" role="img" aria-label="{title}: latest {latest}, peak {peak}, average {average}"><title>{title}: latest {latest}, peak {peak}, average {average}</title><defs><linearGradient id="metrics-fill-{key}" x1="0" y1="0" x2="0" y2="1"><stop offset="0%" stop-color="currentColor" stop-opacity="0.32" /><stop offset="100%" stop-color="currentColor" stop-opacity="0.02" /></linearGradient></defs>"#,
        width = SVG_WIDTH,
        height = SVG_HEIGHT,
        title = spec.title,
        latest = latest_label,
        peak = peak_label,
        average = average_label,
        key = spec.accent.key(),
    );

    for fraction in [0.0, 0.5, 1.0] {
        let y = y_bottom - fraction * plot_height;
        let _ = write!(
            svg,
            r#"<path class="metric-chart-grid" d="M {x_left:.2} {y:.2} H {x_right:.2}" />"#,
        );
    }

    for segment in &segments {
        if segment.len() < 2 {
            continue;
        }
        let line = polyline_path(segment);
        let area = area_path(segment, y_bottom);
        let _ = write!(
            svg,
            r#"<path class="metric-chart-area" fill="url(#metrics-fill-{key})" d="{area}" /><path class="metric-chart-line" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" vector-effect="non-scaling-stroke" d="{line}" />"#,
            key = spec.accent.key(),
        );
    }

    if let Some((x, y)) = points.last().map(map_point) {
        let _ = write!(
            svg,
            r#"<circle class="metric-chart-halo" cx="{x:.2}" cy="{y:.2}" r="6" fill="currentColor" opacity="0.18" /><circle class="metric-chart-point" cx="{x:.2}" cy="{y:.2}" r="3.5" fill="currentColor" />"#,
        );
    }
    svg.push_str("</svg>");

    ChartView {
        key: spec.accent.key(),
        title: spec.title.to_owned(),
        latest_label,
        peak_label,
        average_label,
        updated_label,
        axis_max_label,
        axis_mid_label,
        empty: false,
        svg,
    }
}

fn polyline_path(points: &[(f64, f64)]) -> String {
    let mut path = String::new();
    for (index, (x, y)) in points.iter().enumerate() {
        let _ = if index == 0 {
            write!(path, "M {x:.2} {y:.2}")
        } else {
            write!(path, " L {x:.2} {y:.2}")
        };
    }
    path
}

fn area_path(points: &[(f64, f64)], baseline: f64) -> String {
    let (first_x, _) = points[0];
    let (last_x, _) = points[points.len() - 1];
    let mut path = String::new();
    let _ = write!(path, "M {first_x:.2} {baseline:.2}");
    for (x, y) in points {
        let _ = write!(path, " L {x:.2} {y:.2}");
    }
    let _ = write!(path, " L {last_x:.2} {baseline:.2} Z");
    path
}

/// Rounds a positive value up to a familiar axis ceiling (1, 2, 2.5, 5, 7.5,
/// 10 × 10ⁿ) so labels read cleanly.
fn nice_number(value: f64) -> f64 {
    if !value.is_finite() || value <= 0.0 {
        return 1.0;
    }
    let exponent = value.log10().floor();
    let magnitude = 10f64.powf(exponent);
    let normalized = value / magnitude;
    let step = [1.0, 2.0, 2.5, 5.0, 7.5, 10.0]
        .iter()
        .copied()
        .find(|step| normalized <= *step + 1e-9)
        .unwrap_or(10.0);
    step * magnitude
}

fn axis_max(peak: f64, kind: ChartKind, floor: f64) -> f64 {
    let target = (peak * HEADROOM).max(floor);
    match kind {
        ChartKind::Percent => nice_number(target).clamp(floor.max(1.0), 100.0),
        ChartKind::Bytes => nice_bytes(target),
    }
}

fn nice_bytes(target: f64) -> f64 {
    const KIB: f64 = 1024.0;
    let units = [1.0, KIB, KIB * KIB, KIB * KIB * KIB, KIB * KIB * KIB * KIB];
    let unit = units
        .iter()
        .rev()
        .copied()
        .find(|unit| target >= *unit)
        .unwrap_or(1.0);
    (nice_number(target / unit) * unit).max(1.0)
}

fn latest_label(points: &[MetricPoint], kind: ChartKind) -> String {
    points
        .last()
        .map(|point| format_chart_value(point.value, kind))
        .unwrap_or_else(|| "—".to_owned())
}

fn updated_label(now_ms: i64, ts_ms: Option<i64>) -> String {
    let Some(ts_ms) = ts_ms else {
        return "—".to_owned();
    };
    let secs = now_ms.saturating_sub(ts_ms).max(0) / 1_000;
    if secs < 60 {
        "just now".to_owned()
    } else if secs < 3_600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3_600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
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

    fn aggregate(
        ts_ms: i64,
        cpu: Option<f64>,
        memory: Option<f64>,
        disk: Option<f64>,
    ) -> MetricAggregate {
        MetricAggregate {
            ts_ms,
            cpu_percent: cpu,
            memory_percent: memory,
            disk_percent: disk,
            fs_used_bytes: None,
            fs_total_bytes: None,
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
    fn range_captions_match_the_span() {
        assert_eq!(MetricRange::OneHour.start_caption(), "-1h");
        assert_eq!(MetricRange::OneHour.mid_caption(), "-30m");
        assert_eq!(MetricRange::SevenDays.mid_caption(), "-3.5d");
        assert_eq!(MetricRange::ThirtyDays.mid_caption(), "-15d");
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
    fn empty_metrics_view_has_floor_axis_labels() {
        let view = build_metrics_view(&[], &[], MetricRange::OneHour, 100_000);

        assert!(view.empty);
        assert_eq!(view.charts.len(), 4);
        assert!(view.charts.iter().all(|chart| chart.empty));
        assert_eq!(view.charts[0].axis_max_label, "2.0%");
        assert_eq!(view.charts[0].axis_mid_label, "1.0%");
        assert_eq!(view.charts[1].axis_max_label, "10.0%");
        assert_eq!(view.charts[3].axis_max_label, "1 KiB");
    }

    #[test]
    fn percent_chart_uses_dynamic_axis_and_labels() {
        let aggregates = [
            aggregate(3_300_000, Some(25.0), Some(50.0), None),
            aggregate(3_360_000, Some(100.0), Some(0.0), None),
        ];
        let view = build_metrics_view(&aggregates, &[], MetricRange::OneHour, 4_600_000);

        assert!(!view.charts[0].empty);
        assert_eq!(view.charts[0].latest_label, "100.0%");
        assert_eq!(view.charts[0].peak_label, "100.0%");
        assert_eq!(view.charts[0].average_label, "62.5%");
        assert_eq!(view.charts[0].updated_label, "20m ago");
        assert_eq!(view.charts[0].axis_max_label, "100.0%");
        assert_eq!(view.charts[0].axis_mid_label, "50.0%");
        assert!(view.charts[0].svg.contains("metric-chart-line"));
        assert!(view.charts[0].svg.contains("M 458.89 133.00 L 470.76 4.00"));
        assert!(view.charts[0].svg.contains("metric-chart-point"));
        assert!(view.charts[0].svg.contains("metrics-fill-cpu"));
        assert_eq!(view.charts[1].latest_label, "0.0%");
        assert!(view.charts[2].empty);
        assert!(view.charts[3].empty);
    }

    #[test]
    fn small_cpu_values_zoom_the_axis() {
        let aggregates = [aggregate(4_000_000, Some(0.9), None, None)];
        let view = build_metrics_view(&aggregates, &[], MetricRange::OneHour, 4_600_000);

        assert_eq!(view.charts[0].latest_label, "0.9%");
        assert_eq!(view.charts[0].axis_max_label, "2.0%");
        assert_eq!(view.charts[0].axis_mid_label, "1.0%");
        assert!(view.charts[0].svg.contains(r#"cy="98.60""#));
        assert!(!view.charts[0].svg.contains(r#"cy="176.00""#));
    }

    #[test]
    fn percent_axis_has_floors_and_a_100_percent_cap() {
        let aggregates = [aggregate(4_600_000, Some(99.0), Some(6.0), Some(61.3))];
        let view = build_metrics_view(&aggregates, &[], MetricRange::OneHour, 4_600_000);

        assert_eq!(view.charts[0].axis_max_label, "100.0%");
        assert_eq!(view.charts[1].axis_max_label, "10.0%");
        assert_eq!(view.charts[1].axis_mid_label, "5.0%");
        assert_eq!(view.charts[2].axis_max_label, "75.0%");
        assert_eq!(view.charts[2].axis_mid_label, "37.5%");
    }

    #[test]
    fn data_chart_uses_nice_byte_ceilings_and_single_point_marker() {
        let data = [DataSizeAggregate {
            ts_ms: 4_600_000,
            data_bytes: Some(12.0 * 1024.0),
        }];
        let view = build_metrics_view(&[], &data, MetricRange::OneHour, 4_600_000);

        assert_eq!(view.charts[3].latest_label, "12 KiB");
        assert_eq!(view.charts[3].axis_max_label, "20 KiB");
        assert_eq!(view.charts[3].axis_mid_label, "10 KiB");
        assert_eq!(view.charts[3].updated_label, "just now");
        assert!(!view.charts[3].empty);
        assert!(view.charts[3].svg.contains("metrics-fill-data"));
        assert!(view.charts[3].svg.contains(r#"cx="716.00" cy="72.80""#));
        assert!(!view.charts[3].svg.contains("metric-chart-line"));
    }

    #[test]
    fn data_chart_scales_large_values() {
        let data = [DataSizeAggregate {
            ts_ms: 4_600_000,
            data_bytes: Some(44_560_285_696.0),
        }];
        let view = build_metrics_view(&[], &data, MetricRange::OneHour, 4_600_000);

        assert_eq!(view.charts[3].latest_label, "41.5 GiB");
        assert_eq!(view.charts[3].axis_max_label, "50.0 GiB");
    }

    #[test]
    fn gap_breaks_the_line_into_segments() {
        let aggregates = [
            aggregate(1_000_000, Some(10.0), None, None),
            aggregate(1_060_000, Some(20.0), None, None),
            aggregate(1_500_000, Some(30.0), None, None),
            aggregate(1_560_000, Some(40.0), None, None),
        ];
        let view = build_metrics_view(&aggregates, &[], MetricRange::OneHour, 4_600_000);

        assert_eq!(view.charts[0].svg.matches("metric-chart-line").count(), 2);
    }

    #[test]
    fn updated_label_uses_relative_units() {
        assert_eq!(updated_label(100_000, None), "—");
        assert_eq!(updated_label(100_000, Some(99_500)), "just now");
        assert_eq!(updated_label(100_000, Some(100_000 - 120_000)), "2m ago");
        assert_eq!(updated_label(100_000, Some(100_000 - 7_200_000)), "2h ago");
        assert_eq!(
            updated_label(100_000, Some(100_000 - 172_800_000)),
            "2d ago"
        );
    }
}
