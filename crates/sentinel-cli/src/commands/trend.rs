//! Historical TTL trend analysis and threshold-crossing projection (`sentinel trend`).
//!
//! Ingests a local directory of saved JSON scan reports, orders them by
//! timestamp and ledger sequence, detects discontinuous positive jumps (TTL
//! bumps), fits ordinary least-squares linear regressions per storage entry,
//! and projects calendar dates (with 95% confidence intervals) when entries
//! will cross warning and critical thresholds.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use comfy_table::presets::ASCII_FULL;
use comfy_table::{Cell, Color, Table};
use sentinel_core::domain::compare::MAX_BASELINE_FILE_BYTES;
use sentinel_core::domain::ttl::{SECONDS_PER_DAY, TARGET_LEDGER_CLOSE_SECONDS};
use sentinel_core::domain::ThresholdConfig;
use serde::{Deserialize, Serialize};

use crate::args::TrendArgs;
use crate::commands::{resolve_threshold_config, EXIT_INVALID_ARGS, EXIT_OK};
use crate::formatters::text::format_with_commas;

/// Maximum projection horizon in days (~100 years) to guard calendar formatting against overflow.
const MAX_PROJECTION_DAYS: f64 = 36_525.0;

/// Confidence level used for crossing-date confidence intervals (95%).
const CONFIDENCE_LEVEL: f64 = 0.95;

/// Origin of the wall-clock timestamps used for regression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimestampSource {
    /// Explicit `timestamp` / `scanned_at` field inside the JSON reports.
    Report,
    /// Date/timestamp parsed from report filenames (e.g. `scan-2026-01-15.json`).
    Filename,
    /// Derived from `latest_ledger` and `ledger_velocity_seconds` anchored to the newest report.
    LedgerVelocity,
    /// Mix of explicit timestamps and ledger-derived timestamps.
    Mixed,
}

/// Deserialized JSON scan snapshot with optional explicit timestamp metadata.
#[derive(Debug, Clone, Deserialize)]
struct RawSnapshotReport {
    contract_id: String,
    network: String,
    latest_ledger: u32,
    #[serde(default = "default_ledger_velocity")]
    ledger_velocity_seconds: f64,
    #[serde(default, alias = "scanned_at", alias = "generated_at")]
    timestamp: Option<RawTimestamp>,
    #[serde(default)]
    entries: Vec<RawEntrySnapshot>,
}

fn default_ledger_velocity() -> f64 {
    TARGET_LEDGER_CLOSE_SECONDS
}

/// Flexible JSON timestamp supporting Unix epoch seconds or ISO-8601 UTC strings.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum RawTimestamp {
    UnixSeconds(i64),
    UnixFloat(f64),
    Iso8601(String),
}

/// Entry fields extracted from a JSON scan report.
#[derive(Debug, Clone, Deserialize)]
struct RawEntrySnapshot {
    identifier: String,
    #[serde(default)]
    size_bytes: usize,
    #[serde(default)]
    live_until_ledger: u32,
    remaining_ledgers: i64,
}

/// Normalized historical snapshot ready for chronological ordering and regression.
#[derive(Debug, Clone)]
pub struct IngestedSnapshot {
    pub file_name: String,
    pub contract_id: String,
    pub network: String,
    pub latest_ledger: u32,
    pub ledger_velocity_seconds: f64,
    pub timestamp_secs: i64,
    pub explicit_timestamp: bool,
    pub entries: Vec<SnapshotEntryPoint>,
}

/// Single storage entry measurement within an ingested snapshot.
#[derive(Debug, Clone)]
pub struct SnapshotEntryPoint {
    pub identifier: String,
    pub size_bytes: usize,
    pub live_until_ledger: u32,
    pub remaining_ledgers: i64,
}

/// Result of ingesting and ordering snapshots from an input directory.
#[derive(Debug, Clone)]
pub struct IngestedSeries {
    pub contract_id: String,
    pub network: String,
    pub timestamp_source: TimestampSource,
    pub snapshots: Vec<IngestedSnapshot>,
}

/// A detected discontinuous positive jump in an entry's TTL between two consecutive snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BumpEvent {
    /// Ledger sequence of the snapshot before the bump.
    pub from_ledger: u32,
    /// Ledger sequence of the snapshot where the bump was observed.
    pub to_ledger: u32,
    /// UTC timestamp of the snapshot where the bump was observed.
    pub detected_timestamp: String,
    /// Change in `remaining_ledgers` across the jump.
    pub remaining_ledgers_delta: i64,
    /// Change in `live_until_ledger` across the jump.
    pub live_until_ledger_delta: i64,
}

/// Classification of a threshold crossing projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionState {
    /// Crossing date was projected from a negative decay slope.
    Projected,
    /// Latest observation is already strictly below the threshold.
    AlreadyCrossed,
    /// Entry slope is non-negative (flat or increasing), so no crossing is projected.
    NotDecaying,
    /// Not enough contiguous points with distinct timestamps to fit a slope.
    InsufficientData,
}

/// 95% confidence interval for a projected threshold crossing date.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfidenceInterval {
    /// Nominal confidence level (0.95).
    pub level: f64,
    /// Earliest projected calendar date (`YYYY-MM-DD`, UTC).
    pub earliest_date: String,
    /// Latest projected calendar date (`YYYY-MM-DD`, UTC).
    pub latest_date: String,
    /// Earliest projected timestamp (`YYYY-MM-DDTHH:MM:SSZ`, UTC).
    pub earliest_timestamp: String,
    /// Latest projected timestamp (`YYYY-MM-DDTHH:MM:SSZ`, UTC).
    pub latest_timestamp: String,
    /// Half-width uncertainty in days (`(latest - earliest) / 2`).
    pub margin_days: f64,
}

/// Projected warning or critical threshold crossing for a storage entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThresholdProjection {
    /// Threshold value in remaining ledgers.
    pub threshold_ledgers: u32,
    /// Outcome classification for this projection.
    pub state: ProjectionState,
    /// Point-estimate calendar date (`YYYY-MM-DD`, UTC) when `state == Projected`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projected_date: Option<String>,
    /// Point-estimate UTC timestamp (`YYYY-MM-DDTHH:MM:SSZ`) when `state == Projected`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projected_timestamp: Option<String>,
    /// Point-estimate Unix epoch seconds when `state == Projected`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projected_unix_seconds: Option<i64>,
    /// Projected days remaining from the latest snapshot until crossing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub days_until_crossing: Option<f64>,
    /// 95% confidence interval around the crossing date, available when segment has >= 3 points.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence_interval: Option<ConfidenceInterval>,
}

/// Complete trend regression and threshold-crossing forecast for a single storage entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntryTrend {
    /// Storage entry identifier (e.g. `instance`, `code_wasm`, `persistent:Admin`).
    pub identifier: String,
    /// Total snapshots in which this entry appeared.
    pub total_observations: usize,
    /// Number of contiguous observations used to fit the linear regression slope.
    pub regression_observations: usize,
    /// Most recent observed `live_until_ledger`.
    pub latest_live_until_ledger: u32,
    /// Most recent observed `remaining_ledgers`.
    pub latest_remaining_ledgers: i64,
    /// Most recent observed entry byte size.
    pub latest_size_bytes: usize,
    /// Net change in entry byte size from earliest to latest observation.
    pub size_bytes_delta: i64,
    /// Linear regression decay slope `dL / dt` in ledgers per day (negative when decaying).
    pub slope_ledgers_per_day: Option<f64>,
    /// Linear regression decay slope `dL / dt` in ledgers per second (negative when decaying).
    pub slope_ledgers_per_second: Option<f64>,
    /// Regression intercept (remaining ledgers at the start of the regression segment).
    pub intercept_ledgers: Option<f64>,
    /// Coefficient of determination (`R^2`) for the fitted regression line.
    pub r_squared: Option<f64>,
    /// Number of discontinuous positive jumps (TTL bumps) detected across snapshots.
    pub bump_count: usize,
    /// Details of each detected TTL bump event.
    pub bumps: Vec<BumpEvent>,
    /// True if a bump occurred on the latest snapshot and pre-bump slope was carried forward.
    pub slope_carried_from_pre_bump: bool,
    /// Projected crossing of the warning threshold.
    pub warning_projection: ThresholdProjection,
    /// Projected crossing of the critical threshold.
    pub critical_projection: ThresholdProjection,
}

/// Complete trend analysis report across all entries of a contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrendReport {
    pub contract_id: String,
    pub network: String,
    pub snapshot_count: usize,
    pub first_ledger: u32,
    pub latest_ledger: u32,
    pub first_timestamp: String,
    pub latest_timestamp: String,
    pub timestamp_source: TimestampSource,
    pub warning_threshold: u32,
    pub critical_threshold: u32,
    pub entries: Vec<EntryTrend>,
}

/// Entry point for `sentinel trend`.
pub fn run_trend(args: TrendArgs) -> i32 {
    let thresholds = match resolve_threshold_config(args.warning_threshold, args.critical_threshold)
    {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("Error: {err}");
            return EXIT_INVALID_ARGS;
        }
    };

    let series = match ingest_snapshots(&args.input_dir, args.contract.as_deref()) {
        Ok(series) => series,
        Err(err) => {
            eprintln!("Error: {err}");
            return EXIT_INVALID_ARGS;
        }
    };

    let report = analyze_trend(&series, thresholds);

    if args.format == "json" {
        match render_trend_json(&report) {
            Ok(json) => println!("{json}"),
            Err(err) => {
                eprintln!("Error: {err}");
                return EXIT_INVALID_ARGS;
            }
        }
    } else {
        println!("{}", render_trend_text(&report));
    }

    EXIT_OK
}

/// Renders a `TrendReport` as pretty-printed JSON.
pub fn render_trend_json(report: &TrendReport) -> Result<String, String> {
    serde_json::to_string_pretty(report)
        .map_err(|err| format!("failed to serialize trend report: {err}"))
}

/// Renders a `TrendReport` as an ergonomic ASCII table with bump notes and summary.
pub fn render_trend_text(report: &TrendReport) -> String {
    let header_line = format!(
        "Contract: {} | Network: {} | Snapshots: {} (Ledgers {} -> {})",
        report.contract_id,
        report.network,
        report.snapshot_count,
        format_with_commas(report.first_ledger),
        format_with_commas(report.latest_ledger),
    );
    let window_line = format!(
        "Window: {} -> {} | Thresholds: WARN < {}, CRITICAL < {}",
        report.first_timestamp,
        report.latest_timestamp,
        format_with_commas(report.warning_threshold),
        format_with_commas(report.critical_threshold),
    );

    let mut table = Table::new();
    table.load_preset(ASCII_FULL);
    table.set_header(vec![
        "Entry",
        "Latest Rem.",
        "Slope (L/day)",
        "R^2",
        "Bumps",
        "Warn Crossing (95% CI)",
        "Critical Crossing (95% CI)",
    ]);

    let mut bumped_entries = 0usize;
    let mut already_warn = 0usize;
    let mut already_critical = 0usize;

    for entry in &report.entries {
        if entry.bump_count > 0 {
            bumped_entries += 1;
        }
        if entry.critical_projection.state == ProjectionState::AlreadyCrossed {
            already_critical += 1;
        } else if entry.warning_projection.state == ProjectionState::AlreadyCrossed {
            already_warn += 1;
        }

        let slope_cell = match entry.slope_ledgers_per_day {
            Some(slope) => {
                let formatted = format_signed_decimal(slope);
                if entry.slope_carried_from_pre_bump {
                    Cell::new(format!("{formatted}*"))
                } else {
                    Cell::new(formatted)
                }
            }
            None => Cell::new("N/A"),
        };

        let r2_cell = match entry.r_squared {
            Some(r2) => Cell::new(format!("{r2:.4}")),
            None => Cell::new("N/A"),
        };

        let bump_cell = if entry.bump_count > 0 {
            Cell::new(entry.bump_count.to_string()).fg(Color::Cyan)
        } else {
            Cell::new("0")
        };

        let warn_cell = format_projection_cell(&entry.warning_projection, Color::Yellow);
        let crit_cell = format_projection_cell(&entry.critical_projection, Color::Red);

        table.add_row(vec![
            Cell::new(&entry.identifier),
            Cell::new(format_with_commas(entry.latest_remaining_ledgers)),
            slope_cell,
            r2_cell,
            bump_cell,
            warn_cell,
            crit_cell,
        ]);
    }

    let mut sections = vec![header_line, window_line, table.to_string()];

    let bump_notes: Vec<String> = report
        .entries
        .iter()
        .flat_map(|entry| {
            entry.bumps.iter().map(move |b| {
                format!(
                    "  - {}: ledger {} -> {} ({:+} rem, {:+} live_until) at {}",
                    entry.identifier,
                    format_with_commas(b.from_ledger),
                    format_with_commas(b.to_ledger),
                    b.remaining_ledgers_delta,
                    b.live_until_ledger_delta,
                    b.detected_timestamp
                )
            })
        })
        .collect();
    if !bump_notes.is_empty() {
        sections.push(format!("Detected TTL Bumps:\n{}", bump_notes.join("\n")));
    }

    if report
        .entries
        .iter()
        .any(|entry| entry.slope_carried_from_pre_bump)
    {
        sections.push(
            "Note: * indicates slope carried forward from the latest pre-bump segment because a bump occurred on the most recent snapshot."
                .to_string(),
        );
    }

    let summary_line = format!(
        "Summary: {} entries analyzed. {} bumped, {} already WARN, {} already CRITICAL.",
        report.entries.len(),
        bumped_entries,
        already_warn,
        already_critical
    );
    sections.push(summary_line);

    sections.join("\n")
}

fn format_projection_cell(projection: &ThresholdProjection, crossed_color: Color) -> Cell {
    match projection.state {
        ProjectionState::AlreadyCrossed => Cell::new("ALREADY CROSSED").fg(crossed_color),
        ProjectionState::NotDecaying => Cell::new("NOT DECAYING").fg(Color::Green),
        ProjectionState::InsufficientData => Cell::new("INSUFFICIENT DATA").fg(Color::DarkGrey),
        ProjectionState::Projected => {
            let date = projection.projected_date.as_deref().unwrap_or("N/A");
            let text = match &projection.confidence_interval {
                Some(ci) if ci.earliest_date != ci.latest_date => {
                    format!(
                        "{date} [{}..{}] (±{:.1}d)",
                        ci.earliest_date, ci.latest_date, ci.margin_days
                    )
                }
                Some(ci) => format!("{date} (±{:.1}d)", ci.margin_days),
                None => format!("{date} (no CI)"),
            };
            Cell::new(text)
        }
    }
}

fn format_signed_decimal(value: f64) -> String {
    let rounded = (value * 10.0).round() / 10.0;
    let formatted = format_with_commas(format!("{rounded:.1}"));
    if rounded > 0.0 {
        format!("+{formatted}")
    } else {
        formatted
    }
}

/// Internal per-entry time-series observation.
#[derive(Debug, Clone, Copy)]
struct EntryObservation {
    ledger: u32,
    timestamp_secs: i64,
    size_bytes: usize,
    live_until_ledger: u32,
    remaining_ledgers: i64,
}

/// Internal result of least-squares linear regression `y = slope * x + intercept`
/// where `x = (timestamp_secs - origin_secs) / 86400.0` (days) and `y = remaining_ledgers`.
#[derive(Debug, Clone, Copy)]
struct OlsFit {
    origin_secs: i64,
    n: usize,
    x_mean: f64,
    sxx_centered: f64,
    slope: f64,
    intercept: f64,
    r_squared: f64,
    /// Residual standard error `sqrt(SS_res / (n - 2))`, defined for `n >= 3`.
    residual_std_err: Option<f64>,
}

/// Runs linear regression, bump detection, and threshold-crossing projection across an ordered series.
pub fn analyze_trend(series: &IngestedSeries, thresholds: ThresholdConfig) -> TrendReport {
    let mut by_entry: BTreeMap<String, Vec<EntryObservation>> = BTreeMap::new();
    for snap in &series.snapshots {
        for entry in &snap.entries {
            by_entry
                .entry(entry.identifier.clone())
                .or_default()
                .push(EntryObservation {
                    ledger: snap.latest_ledger,
                    timestamp_secs: snap.timestamp_secs,
                    size_bytes: entry.size_bytes,
                    live_until_ledger: entry.live_until_ledger,
                    remaining_ledgers: entry.remaining_ledgers,
                });
        }
    }

    let entries = by_entry
        .into_iter()
        .map(|(identifier, observations)| analyze_entry(identifier, &observations, thresholds))
        .collect();

    let first = series.snapshots.first().expect("series has >= 2 snapshots");
    let latest = series.snapshots.last().expect("series has >= 2 snapshots");

    TrendReport {
        contract_id: series.contract_id.clone(),
        network: series.network.clone(),
        snapshot_count: series.snapshots.len(),
        first_ledger: first.latest_ledger,
        latest_ledger: latest.latest_ledger,
        first_timestamp: format_timestamp_utc(first.timestamp_secs),
        latest_timestamp: format_timestamp_utc(latest.timestamp_secs),
        timestamp_source: series.timestamp_source,
        warning_threshold: thresholds.warning_ledgers,
        critical_threshold: thresholds.critical_ledgers,
        entries,
    }
}

/// Analyzes one storage entry's chronological observations, splitting on TTL bumps.
fn analyze_entry(
    identifier: String,
    observations: &[EntryObservation],
    thresholds: ThresholdConfig,
) -> EntryTrend {
    let first = observations[0];
    let latest = observations[observations.len() - 1];
    let size_bytes_delta = (latest.size_bytes as i64) - (first.size_bytes as i64);

    // Split observations into contiguous segments separated by discontinuous positive jumps (bumps).
    let mut segments: Vec<Vec<EntryObservation>> = vec![vec![observations[0]]];
    let mut bumps: Vec<BumpEvent> = Vec::new();

    for pair in observations.windows(2) {
        let prev = pair[0];
        let curr = pair[1];
        if is_discontinuous_bump(prev, curr) {
            bumps.push(BumpEvent {
                from_ledger: prev.ledger,
                to_ledger: curr.ledger,
                detected_timestamp: format_timestamp_utc(curr.timestamp_secs),
                remaining_ledgers_delta: curr.remaining_ledgers - prev.remaining_ledgers,
                live_until_ledger_delta: i64::from(curr.live_until_ledger)
                    - i64::from(prev.live_until_ledger),
            });
            segments.push(vec![curr]);
        } else {
            segments
                .last_mut()
                .expect("at least one segment exists")
                .push(curr);
        }
    }

    let latest_segment = segments.last().expect("at least one segment exists");
    let (fit, slope_carried_from_pre_bump) = if latest_segment.len() >= 2 {
        (fit_ols(latest_segment), false)
    } else {
        // Entry was bumped right before the most recent snapshot: carry forward the slope
        // from the most recent multi-point pre-bump segment if available.
        let pre_fit = segments[..segments.len().saturating_sub(1)]
            .iter()
            .rev()
            .find_map(|seg| if seg.len() >= 2 { fit_ols(seg) } else { None });
        (pre_fit, pre_fit.is_some())
    };

    let regression_observations = fit.map(|f| f.n).unwrap_or(latest_segment.len());
    let slope_ledgers_per_day = fit.map(|f| round_4(f.slope));
    let slope_ledgers_per_second = fit.map(|f| round_6(f.slope / SECONDS_PER_DAY));
    let intercept_ledgers = fit.map(|f| {
        if slope_carried_from_pre_bump {
            round_2(latest.remaining_ledgers as f64)
        } else {
            round_2(f.intercept)
        }
    });
    let r_squared = fit.map(|f| round_6(f.r_squared));

    let warning_projection = project_threshold(
        thresholds.warning_ledgers,
        latest,
        fit,
        slope_carried_from_pre_bump,
    );
    let critical_projection = project_threshold(
        thresholds.critical_ledgers,
        latest,
        fit,
        slope_carried_from_pre_bump,
    );

    EntryTrend {
        identifier,
        total_observations: observations.len(),
        regression_observations,
        latest_live_until_ledger: latest.live_until_ledger,
        latest_remaining_ledgers: latest.remaining_ledgers,
        latest_size_bytes: latest.size_bytes,
        size_bytes_delta,
        slope_ledgers_per_day,
        slope_ledgers_per_second,
        intercept_ledgers,
        r_squared,
        bump_count: bumps.len(),
        bumps,
        slope_carried_from_pre_bump,
        warning_projection,
        critical_projection,
    }
}

/// Detects whether storage TTL was bumped (extended) between two consecutive observations.
fn is_discontinuous_bump(prev: EntryObservation, curr: EntryObservation) -> bool {
    let live_until_jumped = prev.live_until_ledger > 0
        && curr.live_until_ledger > 0
        && curr.live_until_ledger > prev.live_until_ledger;
    let remaining_jumped = curr.remaining_ledgers > prev.remaining_ledgers;
    live_until_jumped || remaining_jumped
}

/// Performs ordinary least-squares linear regression using:
/// `slope m = (N * sum(xy) - sum(x) * sum(y)) / (N * sum(x^2) - (sum(x))^2)`
/// with `x` shifted to elapsed days since the first point in the segment to prevent
/// floating-point cancellation on Unix epoch timestamps.
fn fit_ols(points: &[EntryObservation]) -> Option<OlsFit> {
    let n = points.len();
    if n < 2 {
        return None;
    }

    let origin_secs = points[0].timestamp_secs;
    let n_f = n as f64;

    let mut sum_x = 0.0;
    let mut sum_y = 0.0;
    let mut sum_xy = 0.0;
    let mut sum_xx = 0.0;

    for pt in points {
        let x = (pt.timestamp_secs - origin_secs) as f64 / SECONDS_PER_DAY;
        let y = pt.remaining_ledgers as f64;
        sum_x += x;
        sum_y += y;
        sum_xy += x * y;
        sum_xx += x * x;
    }

    let denom = n_f * sum_xx - sum_x * sum_x;
    if !denom.is_finite() || denom <= 1e-12 {
        return None;
    }

    let slope = (n_f * sum_xy - sum_x * sum_y) / denom;
    if !slope.is_finite() {
        return None;
    }
    let intercept = (sum_y - slope * sum_x) / n_f;
    let x_mean = sum_x / n_f;
    let y_mean = sum_y / n_f;

    let mut ss_res = 0.0;
    let mut ss_tot = 0.0;
    let mut sxx_centered = 0.0;
    for pt in points {
        let x = (pt.timestamp_secs - origin_secs) as f64 / SECONDS_PER_DAY;
        let y = pt.remaining_ledgers as f64;
        let predicted = slope * x + intercept;
        let resid = y - predicted;
        ss_res += resid * resid;
        let dy = y - y_mean;
        ss_tot += dy * dy;
        let dx = x - x_mean;
        sxx_centered += dx * dx;
    }

    let r_squared = if ss_tot <= 1e-12 {
        if ss_res <= 1e-12 {
            1.0
        } else {
            0.0
        }
    } else {
        (1.0 - ss_res / ss_tot).clamp(0.0, 1.0)
    };

    let residual_std_err = if n >= 3 {
        Some((ss_res / (n - 2) as f64).max(0.0).sqrt())
    } else {
        None
    };

    Some(OlsFit {
        origin_secs,
        n,
        x_mean,
        sxx_centered,
        slope,
        intercept,
        r_squared,
        residual_std_err,
    })
}

/// Projects when remaining ledgers will cross `threshold`, along with a 95% confidence interval.
fn project_threshold(
    threshold: u32,
    latest: EntryObservation,
    fit: Option<OlsFit>,
    slope_carried_from_pre_bump: bool,
) -> ThresholdProjection {
    if latest.remaining_ledgers < i64::from(threshold) {
        return ThresholdProjection {
            threshold_ledgers: threshold,
            state: ProjectionState::AlreadyCrossed,
            projected_date: None,
            projected_timestamp: None,
            projected_unix_seconds: None,
            days_until_crossing: None,
            confidence_interval: None,
        };
    }

    let Some(fit) = fit else {
        return ThresholdProjection {
            threshold_ledgers: threshold,
            state: ProjectionState::InsufficientData,
            projected_date: None,
            projected_timestamp: None,
            projected_unix_seconds: None,
            days_until_crossing: None,
            confidence_interval: None,
        };
    };

    if fit.slope >= -1e-9 {
        return ThresholdProjection {
            threshold_ledgers: threshold,
            state: ProjectionState::NotDecaying,
            projected_date: None,
            projected_timestamp: None,
            projected_unix_seconds: None,
            days_until_crossing: None,
            confidence_interval: None,
        };
    }

    let threshold_f = f64::from(threshold);
    let latest_ts = latest.timestamp_secs;

    let (projected_ts, ci) = if slope_carried_from_pre_bump {
        // Line is anchored at the post-bump observation `(latest_ts, latest.remaining_ledgers)`
        // with slope carried forward from the pre-bump OLS fit.
        let delta_days_raw = (threshold_f - latest.remaining_ledgers as f64) / fit.slope;
        let delta_days = delta_days_raw.clamp(0.0, MAX_PROJECTION_DAYS);
        let projected_ts = latest_ts.saturating_add((delta_days * SECONDS_PER_DAY).round() as i64);

        let ci = fit.residual_std_err.map(|s| {
            let df = fit.n - 2;
            let t_crit = student_t_95(df);
            let se_slope = if fit.sxx_centered > 1e-12 {
                s / fit.sxx_centered.sqrt()
            } else {
                0.0
            };
            let (lo_days, hi_days) = if se_slope <= 1e-12 {
                (delta_days, delta_days)
            } else {
                let fast_slope = fit.slope - t_crit * se_slope; // more negative -> earlier crossing
                let slow_slope = fit.slope + t_crit * se_slope; // less negative -> later crossing
                if slow_slope < -1e-9 {
                    let lo = ((threshold_f - latest.remaining_ledgers as f64) / fast_slope)
                        .clamp(0.0, delta_days);
                    let hi = ((threshold_f - latest.remaining_ledgers as f64) / slow_slope)
                        .clamp(delta_days, MAX_PROJECTION_DAYS);
                    (lo, hi)
                } else {
                    let margin =
                        (t_crit * se_slope / fit.slope.abs() * delta_days).min(MAX_PROJECTION_DAYS);
                    (
                        (delta_days - margin).max(0.0),
                        (delta_days + margin).min(MAX_PROJECTION_DAYS),
                    )
                }
            };
            let earliest_ts = latest_ts.saturating_add((lo_days * SECONDS_PER_DAY).round() as i64);
            let latest_ci_ts = latest_ts.saturating_add((hi_days * SECONDS_PER_DAY).round() as i64);
            build_confidence_interval(earliest_ts, latest_ci_ts)
        });
        (projected_ts, ci)
    } else {
        let x_last = (latest_ts - fit.origin_secs) as f64 / SECONDS_PER_DAY;
        let x_cross_raw = (threshold_f - fit.intercept) / fit.slope;
        let x_cross = x_cross_raw.clamp(x_last, x_last + MAX_PROJECTION_DAYS);
        let projected_ts = fit
            .origin_secs
            .saturating_add((x_cross * SECONDS_PER_DAY).round() as i64);

        let ci = fit.residual_std_err.map(|s| {
            let df = fit.n - 2;
            let t_crit = student_t_95(df);
            let (x_lo, x_hi) = if s <= 1e-12 || fit.sxx_centered <= 1e-12 {
                (x_cross, x_cross)
            } else {
                // Fieller's exact inverse-regression confidence interval when slope is
                // statistically significant at 95% (g < 1), falling back to delta method otherwise.
                let g = (t_crit * s).powi(2) / (fit.slope * fit.slope * fit.sxx_centered);
                let dx = x_cross - fit.x_mean;
                if g < 1.0 {
                    let radicand = ((1.0 - g) / fit.n as f64 + (dx * dx) / fit.sxx_centered)
                        .max(0.0)
                        .sqrt();
                    let center = fit.x_mean + dx / (1.0 - g);
                    let half = (t_crit * s / fit.slope.abs()) * radicand / (1.0 - g);
                    let lo = (center - half).clamp(x_last, x_cross);
                    let hi = (center + half).clamp(x_cross, x_last + MAX_PROJECTION_DAYS);
                    (lo, hi)
                } else {
                    let se_y = s * (1.0 / fit.n as f64 + (dx * dx) / fit.sxx_centered).sqrt();
                    let half = (t_crit * se_y / fit.slope.abs()).min(MAX_PROJECTION_DAYS);
                    let lo = (x_cross - half).clamp(x_last, x_cross);
                    let hi = (x_cross + half).clamp(x_cross, x_last + MAX_PROJECTION_DAYS);
                    (lo, hi)
                }
            };
            let earliest_ts = fit
                .origin_secs
                .saturating_add((x_lo * SECONDS_PER_DAY).round() as i64);
            let latest_ci_ts = fit
                .origin_secs
                .saturating_add((x_hi * SECONDS_PER_DAY).round() as i64);
            build_confidence_interval(earliest_ts, latest_ci_ts)
        });
        (projected_ts, ci)
    };

    let days_until = round_2(((projected_ts - latest_ts) as f64 / SECONDS_PER_DAY).max(0.0));

    ThresholdProjection {
        threshold_ledgers: threshold,
        state: ProjectionState::Projected,
        projected_date: Some(format_date_utc(projected_ts)),
        projected_timestamp: Some(format_timestamp_utc(projected_ts)),
        projected_unix_seconds: Some(projected_ts),
        days_until_crossing: Some(days_until),
        confidence_interval: ci,
    }
}

fn build_confidence_interval(earliest_ts: i64, latest_ts: i64) -> ConfidenceInterval {
    let earliest_ts = earliest_ts.min(latest_ts);
    let latest_ts = latest_ts.max(earliest_ts);
    let margin_days = round_2(((latest_ts - earliest_ts) as f64 / SECONDS_PER_DAY) / 2.0);
    ConfidenceInterval {
        level: CONFIDENCE_LEVEL,
        earliest_date: format_date_utc(earliest_ts),
        latest_date: format_date_utc(latest_ts),
        earliest_timestamp: format_timestamp_utc(earliest_ts),
        latest_timestamp: format_timestamp_utc(latest_ts),
        margin_days,
    }
}

/// Two-tailed 95% Student's t critical value (`t_{0.025, df}`) for `df >= 1`.
fn student_t_95(df: usize) -> f64 {
    const TABLE: [f64; 30] = [
        12.706205, // df = 1
        4.302653,  // df = 2
        3.182446,  // df = 3
        2.776445,  // df = 4
        2.570582,  // df = 5
        2.446912,  // df = 6
        2.364624,  // df = 7
        2.306004,  // df = 8
        2.262157,  // df = 9
        2.228139,  // df = 10
        2.200985,  // df = 11
        2.178813,  // df = 12
        2.160369,  // df = 13
        2.144787,  // df = 14
        2.131450,  // df = 15
        2.119905,  // df = 16
        2.109816,  // df = 17
        2.100922,  // df = 18
        2.093024,  // df = 19
        2.085963,  // df = 20
        2.079614,  // df = 21
        2.073873,  // df = 22
        2.068658,  // df = 23
        2.063899,  // df = 24
        2.059539,  // df = 25
        2.055529,  // df = 26
        2.051831,  // df = 27
        2.048407,  // df = 28
        2.045230,  // df = 29
        2.042272,  // df = 30
    ];
    if df == 0 {
        return TABLE[0];
    }
    if df <= TABLE.len() {
        TABLE[df - 1]
    } else {
        // Cornish-Fisher expansion for 95% two-tailed normal quantile z = 1.959964.
        let v = df as f64;
        1.959964 + 2.37227 / v + 2.8225 / (v * v)
    }
}

fn round_2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn round_4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

fn round_6(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

struct PendingSnapshot {
    file_name: String,
    report: RawSnapshotReport,
    report_timestamp: Option<i64>,
    filename_timestamp: Option<i64>,
    mtime_secs: Option<i64>,
}

/// Reads all `.json` scan reports in `input_dir`, optionally filters by `contract_filter`,
/// resolves timestamps, and returns snapshots ordered by timestamp/ledger sequence.
pub fn ingest_snapshots(
    input_dir: &Path,
    contract_filter: Option<&str>,
) -> Result<IngestedSeries, String> {
    if !input_dir.exists() {
        return Err(format!(
            "input directory '{}' does not exist",
            input_dir.display()
        ));
    }
    if !input_dir.is_dir() {
        return Err(format!(
            "input path '{}' is not a directory",
            input_dir.display()
        ));
    }

    let mut json_paths: Vec<PathBuf> = Vec::new();
    let entries = fs::read_dir(input_dir)
        .map_err(|e| format!("failed to read directory '{}': {e}", input_dir.display()))?;
    for entry in entries {
        let entry =
            entry.map_err(|e| format!("failed to read entry in '{}': {e}", input_dir.display()))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let is_json = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"));
        if is_json {
            json_paths.push(path);
        }
    }

    // Deterministic filesystem traversal order before semantic sorting.
    json_paths.sort();

    if json_paths.is_empty() {
        return Err(format!(
            "no JSON scan reports found in '{}'",
            input_dir.display()
        ));
    }

    let mut pending: Vec<PendingSnapshot> = Vec::with_capacity(json_paths.len());
    for path in json_paths {
        let metadata =
            fs::metadata(&path).map_err(|e| format!("failed to stat '{}': {e}", path.display()))?;
        if metadata.len() > MAX_BASELINE_FILE_BYTES {
            return Err(format!(
                "report '{}' exceeds maximum allowed size of {} bytes",
                path.display(),
                MAX_BASELINE_FILE_BYTES
            ));
        }

        let contents = fs::read_to_string(&path)
            .map_err(|e| format!("failed to read '{}': {e}", path.display()))?;
        let report: RawSnapshotReport = serde_json::from_str(&contents)
            .map_err(|e| format!("invalid JSON scan report '{}': {e}", path.display()))?;

        let report_timestamp = match &report.timestamp {
            Some(raw) => Some(
                parse_raw_timestamp(raw)
                    .map_err(|e| format!("invalid timestamp in '{}': {e}", path.display()))?,
            ),
            None => None,
        };

        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let filename_timestamp = extract_timestamp_from_filename(&file_name);
        let mtime_secs = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64);

        pending.push(PendingSnapshot {
            file_name,
            report,
            report_timestamp,
            filename_timestamp,
            mtime_secs,
        });
    }

    if let Some(target_contract) = contract_filter {
        pending.retain(|item| item.report.contract_id == target_contract);
        if pending.is_empty() {
            return Err(format!(
                "no scan reports matching contract '{}' found in '{}'",
                target_contract,
                input_dir.display()
            ));
        }
    } else {
        let contracts: BTreeSet<&str> = pending
            .iter()
            .map(|item| item.report.contract_id.as_str())
            .collect();
        if contracts.len() > 1 {
            let list = contracts.into_iter().collect::<Vec<_>>().join(", ");
            return Err(format!(
                "multiple contracts found in '{}': {list}; specify --contract <ID> to select one",
                input_dir.display()
            ));
        }
    }

    if pending.len() < 2 {
        return Err(format!(
            "at least 2 historical scan reports are required for trend analysis (found {})",
            pending.len()
        ));
    }

    let contract_id = pending[0].report.contract_id.clone();
    let network = pending[0].report.network.clone();
    for item in &pending[1..] {
        if !item.report.network.eq_ignore_ascii_case(&network) {
            return Err(format!(
                "reports for contract '{}' span multiple networks: '{}' vs '{}'",
                contract_id, network, item.report.network
            ));
        }
    }

    // Resolve wall-clock timestamps.
    let all_have_report_ts = pending.iter().all(|p| p.report_timestamp.is_some());
    let all_have_filename_ts = pending
        .iter()
        .all(|p| p.report_timestamp.is_none() && p.filename_timestamp.is_some());
    let any_explicit_ts = pending
        .iter()
        .any(|p| p.report_timestamp.is_some() || p.filename_timestamp.is_some());

    // Anchor timestamp and ledger for ledger-velocity-derived timestamps.
    let anchor = pending
        .iter()
        .max_by_key(|p| {
            (
                p.report_timestamp.or(p.filename_timestamp).is_some(),
                p.report.latest_ledger,
            )
        })
        .expect("pending has at least 2 items");
    let anchor_ledger = anchor.report.latest_ledger;
    let anchor_ts = anchor
        .report_timestamp
        .or(anchor.filename_timestamp)
        .or(anchor.mtime_secs)
        .unwrap_or_else(unix_timestamp_now);

    let mut velocity_sum = 0.0;
    let mut velocity_count = 0usize;
    for item in &pending {
        let v = item.report.ledger_velocity_seconds;
        if v.is_finite() && (1.0..=60.0).contains(&v) {
            velocity_sum += v;
            velocity_count += 1;
        }
    }
    let avg_velocity = if velocity_count > 0 {
        velocity_sum / velocity_count as f64
    } else {
        TARGET_LEDGER_CLOSE_SECONDS
    };

    let timestamp_source = if all_have_report_ts {
        TimestampSource::Report
    } else if all_have_filename_ts {
        TimestampSource::Filename
    } else if any_explicit_ts {
        TimestampSource::Mixed
    } else {
        TimestampSource::LedgerVelocity
    };

    let mut snapshots: Vec<IngestedSnapshot> = pending
        .into_iter()
        .map(|item| {
            let explicit = item.report_timestamp.or(item.filename_timestamp);
            let (timestamp_secs, explicit_timestamp) = match explicit {
                Some(ts) => (ts, true),
                None => {
                    let ledger_delta =
                        i64::from(item.report.latest_ledger) - i64::from(anchor_ledger);
                    let offset_secs = (ledger_delta as f64 * avg_velocity).round() as i64;
                    (anchor_ts.saturating_add(offset_secs), false)
                }
            };
            let velocity = if item.report.ledger_velocity_seconds.is_finite()
                && (1.0..=60.0).contains(&item.report.ledger_velocity_seconds)
            {
                item.report.ledger_velocity_seconds
            } else {
                avg_velocity
            };
            IngestedSnapshot {
                file_name: item.file_name,
                contract_id: item.report.contract_id,
                network: item.report.network,
                latest_ledger: item.report.latest_ledger,
                ledger_velocity_seconds: velocity,
                timestamp_secs,
                explicit_timestamp,
                entries: item
                    .report
                    .entries
                    .into_iter()
                    .map(|e| SnapshotEntryPoint {
                        identifier: e.identifier,
                        size_bytes: e.size_bytes,
                        live_until_ledger: e.live_until_ledger,
                        remaining_ledgers: e.remaining_ledgers,
                    })
                    .collect(),
            }
        })
        .collect();

    // Order chronologically by timestamp, breaking ties by ledger sequence and filename.
    snapshots.sort_by(|a, b| {
        a.timestamp_secs
            .cmp(&b.timestamp_secs)
            .then_with(|| a.latest_ledger.cmp(&b.latest_ledger))
            .then_with(|| a.file_name.cmp(&b.file_name))
    });

    Ok(IngestedSeries {
        contract_id,
        network,
        timestamp_source,
        snapshots,
    })
}

fn parse_raw_timestamp(raw: &RawTimestamp) -> Result<i64, String> {
    match raw {
        RawTimestamp::UnixSeconds(s) => Ok(*s),
        RawTimestamp::UnixFloat(f) => {
            if !f.is_finite() {
                return Err("non-finite numeric timestamp".to_string());
            }
            Ok(f.round() as i64)
        }
        RawTimestamp::Iso8601(s) => parse_iso8601_utc(s),
    }
}

/// Parses an ISO-8601 date or UTC timestamp (`YYYY-MM-DD`, `YYYY-MM-DDTHH:MM:SSZ`,
/// optional fractional seconds, optional `±HH:MM` offset) into Unix epoch seconds.
pub(crate) fn parse_iso8601_utc(input: &str) -> Result<i64, String> {
    let s = input.trim();
    if s.len() < 10 {
        return Err(format!("unrecognized timestamp '{input}'"));
    }

    let bytes = s.as_bytes();
    if bytes[4] != b'-' || bytes[7] != b'-' {
        return Err(format!("expected YYYY-MM-DD prefix in '{input}'"));
    }

    let year: i64 = s[0..4]
        .parse()
        .map_err(|_| format!("invalid year in '{input}'"))?;
    let month: i64 = s[5..7]
        .parse()
        .map_err(|_| format!("invalid month in '{input}'"))?;
    let day: i64 = s[8..10]
        .parse()
        .map_err(|_| format!("invalid day in '{input}'"))?;

    validate_calendar_date(year, month, day)?;
    let base_days = days_from_civil(year, month, day);

    if s.len() == 10 {
        return Ok(base_days * SECONDS_PER_DAY as i64);
    }

    let sep = bytes[10];
    if sep != b'T' && sep != b't' && sep != b' ' {
        return Err(format!("expected 'T' separator in '{input}'"));
    }
    let time_part = &s[11..];
    if time_part.len() < 8 {
        return Err(format!("incomplete time component in '{input}'"));
    }
    let tbytes = time_part.as_bytes();
    if (tbytes[2] != b':' && tbytes[2] != b'-') || (tbytes[5] != b':' && tbytes[5] != b'-') {
        return Err(format!("invalid time format in '{input}'"));
    }
    let hour: i64 = time_part[0..2]
        .parse()
        .map_err(|_| format!("invalid hour in '{input}'"))?;
    let minute: i64 = time_part[3..5]
        .parse()
        .map_err(|_| format!("invalid minute in '{input}'"))?;
    let second: i64 = time_part[6..8]
        .parse()
        .map_err(|_| format!("invalid second in '{input}'"))?;

    if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) || !(0..=59).contains(&second) {
        return Err(format!("time out of range in '{input}'"));
    }

    let mut rest = &time_part[8..];
    // Skip optional fractional seconds (.123456)
    if let Some(after_dot) = rest.strip_prefix('.') {
        let digits = after_dot.bytes().take_while(|b| b.is_ascii_digit()).count();
        if digits == 0 {
            return Err(format!("invalid fractional seconds in '{input}'"));
        }
        rest = &after_dot[digits..];
    }

    let tz_offset_secs: i64 = if rest.is_empty() || rest == "Z" || rest == "z" {
        0
    } else if rest.len() == 6 && (rest.starts_with('+') || rest.starts_with('-')) {
        let sign = if rest.starts_with('+') { 1 } else { -1 };
        if rest.as_bytes()[3] != b':' {
            return Err(format!("invalid timezone offset in '{input}'"));
        }
        let tz_h: i64 = rest[1..3]
            .parse()
            .map_err(|_| format!("invalid timezone hour in '{input}'"))?;
        let tz_m: i64 = rest[4..6]
            .parse()
            .map_err(|_| format!("invalid timezone minute in '{input}'"))?;
        if !(0..=23).contains(&tz_h) || !(0..=59).contains(&tz_m) {
            return Err(format!("timezone offset out of range in '{input}'"));
        }
        sign * (tz_h * 3_600 + tz_m * 60)
    } else {
        return Err(format!("unsupported timezone suffix '{rest}' in '{input}'"));
    };

    Ok(base_days * 86_400 + hour * 3_600 + minute * 60 + second - tz_offset_secs)
}

/// Attempts to extract a `YYYY-MM-DD` or `YYYY-MM-DDTHH[-:]MM[-:]SSZ` date from a filename.
fn extract_timestamp_from_filename(file_name: &str) -> Option<i64> {
    let bytes = file_name.as_bytes();
    if bytes.len() < 10 {
        return None;
    }
    for start in 0..=bytes.len() - 10 {
        if bytes[start..start + 4].iter().all(|b| b.is_ascii_digit())
            && bytes[start + 4] == b'-'
            && bytes[start + 5..start + 7]
                .iter()
                .all(|b| b.is_ascii_digit())
            && bytes[start + 7] == b'-'
            && bytes[start + 8..start + 10]
                .iter()
                .all(|b| b.is_ascii_digit())
        {
            if start + 20 <= bytes.len()
                && (bytes[start + 10] == b'T' || bytes[start + 10] == b't')
                && (bytes[start + 19] == b'Z' || bytes[start + 19] == b'z')
            {
                if let Ok(ts) = parse_iso8601_utc(&file_name[start..start + 20]) {
                    return Some(ts);
                }
            }
            if let Ok(ts) = parse_iso8601_utc(&file_name[start..start + 10]) {
                return Some(ts);
            }
        }
    }
    None
}

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

fn validate_calendar_date(year: i64, month: i64, day: i64) -> Result<(), String> {
    if !(1..=12).contains(&month) {
        return Err(format!("month {month} out of range"));
    }
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => unreachable!(),
    };
    if !(1..=max_day).contains(&day) {
        return Err(format!("day {day} out of range for {year:04}-{month:02}"));
    }
    Ok(())
}

/// Howard Hinnant's civil-to-days algorithm (inverse of `civil_date_from_days`).
pub(crate) fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = year - if month <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Howard Hinnant's civil-from-days algorithm, valid for all Unix-era dates.
pub(crate) fn civil_date_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    (year + if month <= 2 { 1 } else { 0 }, month, day)
}

/// Formats Unix seconds as UTC ISO-8601 calendar date `YYYY-MM-DD`.
pub(crate) fn format_date_utc(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let (year, month, day) = civil_date_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Formats Unix seconds as UTC ISO-8601 timestamp `YYYY-MM-DDTHH:MM:SSZ`.
pub(crate) fn format_timestamp_utc(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let within_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_date_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        within_day / 3_600,
        (within_day % 3_600) / 60,
        within_day % 60
    )
}

fn unix_timestamp_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn temp_test_dir(prefix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sentinel-trend-unit-{prefix}-{}-{}",
            std::process::id(),
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    type TestEntrySpec<'a> = (&'a str, u32, i64, usize);
    type TestSnapshotSpec<'a> = (u32, &'a str, Vec<TestEntrySpec<'a>>);

    fn make_series(snaps: Vec<TestSnapshotSpec<'_>>) -> IngestedSeries {
        let snapshots = snaps
            .into_iter()
            .enumerate()
            .map(|(i, (ledger, ts, entries))| IngestedSnapshot {
                file_name: format!("snap-{i}.json"),
                contract_id: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM".to_string(),
                network: "testnet".to_string(),
                latest_ledger: ledger,
                ledger_velocity_seconds: 5.0,
                timestamp_secs: parse_iso8601_utc(ts).unwrap(),
                explicit_timestamp: true,
                entries: entries
                    .into_iter()
                    .map(|(id, live, rem, size)| SnapshotEntryPoint {
                        identifier: id.to_string(),
                        size_bytes: size,
                        live_until_ledger: live,
                        remaining_ledgers: rem,
                    })
                    .collect(),
            })
            .collect();
        IngestedSeries {
            contract_id: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM".to_string(),
            network: "testnet".to_string(),
            timestamp_source: TimestampSource::Report,
            snapshots,
        }
    }

    #[test]
    fn test_iso8601_roundtrip_and_calendar_math() {
        assert_eq!(parse_iso8601_utc("1970-01-01").unwrap(), 0);
        assert_eq!(parse_iso8601_utc("1970-01-01T00:00:00Z").unwrap(), 0);
        assert_eq!(
            parse_iso8601_utc("2026-01-15T12:30:45Z").unwrap(),
            days_from_civil(2026, 1, 15) * 86_400 + 12 * 3_600 + 30 * 60 + 45
        );
        assert_eq!(
            format_timestamp_utc(parse_iso8601_utc("2026-03-01T08:15:00Z").unwrap()),
            "2026-03-01T08:15:00Z"
        );
        assert_eq!(
            format_date_utc(parse_iso8601_utc("2024-02-29T23:59:59Z").unwrap()),
            "2024-02-29"
        );
        assert_eq!(
            parse_iso8601_utc("2026-01-15T14:00:00+02:00").unwrap(),
            parse_iso8601_utc("2026-01-15T12:00:00Z").unwrap()
        );
        assert!(parse_iso8601_utc("2025-02-29").is_err());
    }

    #[test]
    fn test_extract_timestamp_from_filename() {
        assert_eq!(
            extract_timestamp_from_filename("scan-2026-01-15.json"),
            Some(parse_iso8601_utc("2026-01-15").unwrap())
        );
        assert_eq!(
            extract_timestamp_from_filename("contract_2026-01-15T06-30-00Z.json"),
            Some(parse_iso8601_utc("2026-01-15T06:30:00Z").unwrap())
        );
        assert_eq!(extract_timestamp_from_filename("day1.json"), None);
    }

    #[test]
    fn test_ingest_orders_reports_by_timestamp_and_ledger() {
        let dir = temp_test_dir("order");
        let contract = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM";

        let r3 = json!({
            "contract_id": contract,
            "network": "testnet",
            "latest_ledger": 5_034_560,
            "ledger_velocity_seconds": 5.0,
            "timestamp": "2026-01-03T00:00:00Z",
            "entries": [{ "identifier": "instance", "live_until_ledger": 5_300_000, "remaining_ledgers": 265_440 }]
        });
        let r1 = json!({
            "contract_id": contract,
            "network": "testnet",
            "latest_ledger": 5_000_000,
            "ledger_velocity_seconds": 5.0,
            "timestamp": "2026-01-01T00:00:00Z",
            "entries": [{ "identifier": "instance", "live_until_ledger": 5_300_000, "remaining_ledgers": 300_000 }]
        });
        let r2 = json!({
            "contract_id": contract,
            "network": "testnet",
            "latest_ledger": 5_017_280,
            "ledger_velocity_seconds": 5.0,
            "timestamp": "2026-01-02T00:00:00Z",
            "entries": [{ "identifier": "instance", "live_until_ledger": 5_300_000, "remaining_ledgers": 282_720 }]
        });

        fs::write(dir.join("a_latest.json"), serde_json::to_vec(&r3).unwrap()).unwrap();
        fs::write(
            dir.join("z_earliest.json"),
            serde_json::to_vec(&r1).unwrap(),
        )
        .unwrap();
        fs::write(dir.join("m_middle.json"), serde_json::to_vec(&r2).unwrap()).unwrap();
        fs::write(dir.join("notes.txt"), "ignored").unwrap();

        let series = ingest_snapshots(&dir, None).unwrap();
        assert_eq!(series.timestamp_source, TimestampSource::Report);
        assert_eq!(series.snapshots.len(), 3);
        assert_eq!(series.snapshots[0].latest_ledger, 5_000_000);
        assert_eq!(series.snapshots[1].latest_ledger, 5_017_280);
        assert_eq!(series.snapshots[2].latest_ledger, 5_034_560);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_ingest_derives_timestamps_from_ledger_velocity_when_omitted() {
        let dir = temp_test_dir("derived-ts");
        let contract = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM";

        for (name, ledger, rem) in [
            ("snap1.json", 5_000_000u32, 300_000i64),
            ("snap2.json", 5_017_280u32, 282_720i64),
            ("snap3.json", 5_034_560u32, 265_440i64),
        ] {
            let report = json!({
                "contract_id": contract,
                "network": "testnet",
                "latest_ledger": ledger,
                "ledger_velocity_seconds": 5.0,
                "entries": [{ "identifier": "instance", "live_until_ledger": 5_300_000, "remaining_ledgers": rem }]
            });
            fs::write(dir.join(name), serde_json::to_vec(&report).unwrap()).unwrap();
        }

        let series = ingest_snapshots(&dir, None).unwrap();
        assert_eq!(series.timestamp_source, TimestampSource::LedgerVelocity);
        assert_eq!(
            series.snapshots[1].timestamp_secs - series.snapshots[0].timestamp_secs,
            86_400
        );
        assert_eq!(
            series.snapshots[2].timestamp_secs - series.snapshots[1].timestamp_secs,
            86_400
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_exact_linear_decay_and_crossing_dates() {
        // Exact -17,280 ledgers/day (-0.2 ledgers/sec) starting at 293,760 on 2026-01-01.
        // Warning threshold = 120,960 => crossed at (293,760 - 120,960) / 17,280 = 10 days => 2026-01-11.
        // Critical threshold = 34,560 => crossed at (293,760 - 34,560) / 17,280 = 15 days => 2026-01-16.
        let series = make_series(vec![
            (
                5_000_000,
                "2026-01-01T00:00:00Z",
                vec![("instance", 5_293_760, 293_760, 320)],
            ),
            (
                5_017_280,
                "2026-01-02T00:00:00Z",
                vec![("instance", 5_293_760, 276_480, 330)],
            ),
            (
                5_034_560,
                "2026-01-03T00:00:00Z",
                vec![("instance", 5_293_760, 259_200, 340)],
            ),
        ]);

        let report = analyze_trend(&series, ThresholdConfig::default());
        assert_eq!(report.entries.len(), 1);
        let entry = &report.entries[0];

        assert_eq!(entry.slope_ledgers_per_day, Some(-17_280.0));
        assert_eq!(entry.slope_ledgers_per_second, Some(-0.2));
        assert_eq!(entry.r_squared, Some(1.0));
        assert_eq!(entry.bump_count, 0);
        assert_eq!(entry.size_bytes_delta, 20);

        assert_eq!(entry.warning_projection.state, ProjectionState::Projected);
        assert_eq!(
            entry.warning_projection.projected_date.as_deref(),
            Some("2026-01-11")
        );
        let warn_ci = entry
            .warning_projection
            .confidence_interval
            .as_ref()
            .expect("3 points should yield CI");
        assert_eq!(warn_ci.earliest_date, "2026-01-11");
        assert_eq!(warn_ci.latest_date, "2026-01-11");
        assert_eq!(warn_ci.margin_days, 0.0);

        assert_eq!(entry.critical_projection.state, ProjectionState::Projected);
        assert_eq!(
            entry.critical_projection.projected_date.as_deref(),
            Some("2026-01-16")
        );
    }

    #[test]
    fn test_noisy_decay_produces_nonzero_confidence_interval() {
        // Realistic slight ledger-close jitter around ~17,300 ledgers/day across 4 days.
        let series = make_series(vec![
            (
                5_000_000,
                "2026-01-01T00:00:00Z",
                vec![("instance", 5_300_000, 300_000, 300)],
            ),
            (
                5_017_600,
                "2026-01-02T00:00:00Z",
                vec![("instance", 5_300_000, 282_400, 300)],
            ),
            (
                5_034_400,
                "2026-01-03T00:00:00Z",
                vec![("instance", 5_300_000, 265_600, 300)],
            ),
            (
                5_052_200,
                "2026-01-04T00:00:00Z",
                vec![("instance", 5_300_000, 247_800, 300)],
            ),
        ]);

        let report = analyze_trend(&series, ThresholdConfig::default());
        let entry = &report.entries[0];
        let slope = entry.slope_ledgers_per_day.unwrap();
        assert!(slope < -17_000.0 && slope > -17_600.0, "slope={slope}");
        assert!(entry.r_squared.unwrap() > 0.99);

        let warn_ci = entry
            .warning_projection
            .confidence_interval
            .as_ref()
            .expect("CI expected for N=4");
        assert!(
            warn_ci.margin_days > 0.0,
            "noisy fit should produce non-zero CI margin"
        );
        assert!(
            warn_ci.earliest_timestamp
                <= *entry
                    .warning_projection
                    .projected_timestamp
                    .as_ref()
                    .unwrap()
        );
        assert!(
            warn_ci.latest_timestamp
                >= *entry
                    .warning_projection
                    .projected_timestamp
                    .as_ref()
                    .unwrap()
        );
    }

    #[test]
    fn test_mid_series_storage_bump_is_detected_and_segmented() {
        // Day 1-2 decay, then bumped before Day 3, then Day 3-5 decay cleanly at -17,280 L/day.
        let series = make_series(vec![
            (
                5_000_000,
                "2026-01-01T00:00:00Z",
                vec![("instance", 5_200_000, 200_000, 256)],
            ),
            (
                5_017_280,
                "2026-01-02T00:00:00Z",
                vec![("instance", 5_200_000, 182_720, 256)],
            ),
            // TTL bumped to 5,500,000 before Day 3!
            (
                5_034_560,
                "2026-01-03T00:00:00Z",
                vec![("instance", 5_500_000, 465_440, 256)],
            ),
            (
                5_051_840,
                "2026-01-04T00:00:00Z",
                vec![("instance", 5_500_000, 448_160, 256)],
            ),
            (
                5_069_120,
                "2026-01-05T00:00:00Z",
                vec![("instance", 5_500_000, 430_880, 256)],
            ),
        ]);

        let report = analyze_trend(&series, ThresholdConfig::default());
        let entry = &report.entries[0];

        assert_eq!(entry.bump_count, 1);
        assert_eq!(entry.bumps[0].from_ledger, 5_017_280);
        assert_eq!(entry.bumps[0].to_ledger, 5_034_560);
        assert_eq!(entry.bumps[0].live_until_ledger_delta, 300_000);
        assert_eq!(entry.regression_observations, 3);
        assert!(!entry.slope_carried_from_pre_bump);
        assert_eq!(entry.slope_ledgers_per_day, Some(-17_280.0));
        assert_eq!(entry.warning_projection.state, ProjectionState::Projected);
        // 430,880 - 120,960 = 309,920 ledgers => 17.935 days after 2026-01-05 => 2026-01-22
        assert_eq!(
            entry.warning_projection.projected_date.as_deref(),
            Some("2026-01-22")
        );
    }

    #[test]
    fn test_bump_on_latest_snapshot_carries_pre_bump_slope() {
        // Day 1-3 decay at -17,280 L/day, then Day 4 is freshly bumped.
        let series = make_series(vec![
            (
                5_000_000,
                "2026-01-01T00:00:00Z",
                vec![("instance", 5_200_000, 200_000, 256)],
            ),
            (
                5_017_280,
                "2026-01-02T00:00:00Z",
                vec![("instance", 5_200_000, 182_720, 256)],
            ),
            (
                5_034_560,
                "2026-01-03T00:00:00Z",
                vec![("instance", 5_200_000, 165_440, 256)],
            ),
            (
                5_051_840,
                "2026-01-04T00:00:00Z",
                vec![("instance", 5_345_600, 293_760, 256)],
            ),
        ]);

        let report = analyze_trend(&series, ThresholdConfig::default());
        let entry = &report.entries[0];

        assert_eq!(entry.bump_count, 1);
        assert!(entry.slope_carried_from_pre_bump);
        assert_eq!(entry.slope_ledgers_per_day, Some(-17_280.0));
        // 293,760 - 120,960 = 172,800 => 10 days after 2026-01-04 => 2026-01-14
        assert_eq!(
            entry.warning_projection.projected_date.as_deref(),
            Some("2026-01-14")
        );
    }

    #[test]
    fn test_already_crossed_and_archived_entries() {
        let series = make_series(vec![
            (
                5_000_000,
                "2026-01-01T00:00:00Z",
                vec![
                    ("persistent:Warned", 5_100_000, 100_000, 64),
                    ("persistent:Dead", 0, 0, 0),
                ],
            ),
            (
                5_017_280,
                "2026-01-02T00:00:00Z",
                vec![
                    ("persistent:Warned", 5_100_000, 82_720, 64),
                    ("persistent:Dead", 0, 0, 0),
                ],
            ),
            (
                5_034_560,
                "2026-01-03T00:00:00Z",
                vec![
                    ("persistent:Warned", 5_100_000, 65_440, 64),
                    ("persistent:Dead", 0, 0, 0),
                ],
            ),
        ]);

        let report = analyze_trend(&series, ThresholdConfig::default());
        let dead = report
            .entries
            .iter()
            .find(|e| e.identifier == "persistent:Dead")
            .unwrap();
        assert_eq!(
            dead.warning_projection.state,
            ProjectionState::AlreadyCrossed
        );
        assert_eq!(
            dead.critical_projection.state,
            ProjectionState::AlreadyCrossed
        );

        let warned = report
            .entries
            .iter()
            .find(|e| e.identifier == "persistent:Warned")
            .unwrap();
        assert_eq!(
            warned.warning_projection.state,
            ProjectionState::AlreadyCrossed
        );
        assert_eq!(warned.critical_projection.state, ProjectionState::Projected);
    }

    #[test]
    fn test_render_trend_text_and_json() {
        let series = make_series(vec![
            (
                5_000_000,
                "2026-01-01T00:00:00Z",
                vec![("instance", 5_293_760, 293_760, 320)],
            ),
            (
                5_017_280,
                "2026-01-02T00:00:00Z",
                vec![("instance", 5_293_760, 276_480, 320)],
            ),
            (
                5_034_560,
                "2026-01-03T00:00:00Z",
                vec![("instance", 5_293_760, 259_200, 320)],
            ),
        ]);
        let report = analyze_trend(&series, ThresholdConfig::default());

        let text = render_trend_text(&report);
        assert!(text.contains("Entry"));
        assert!(text.contains("Slope (L/day)"));
        assert!(text.contains("Warn Crossing (95% CI)"));
        assert!(text.contains("Critical Crossing (95% CI)"));
        assert!(text.contains("instance"));
        assert!(text.contains("-17,280.0"));
        assert!(text.contains("2026-01-11"));
        assert!(text.contains("2026-01-16"));

        let json_str = render_trend_json(&report).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["snapshot_count"], 3);
        assert_eq!(parsed["entries"][0]["slope_ledgers_per_day"], -17280.0);
        assert_eq!(
            parsed["entries"][0]["warning_projection"]["projected_date"],
            "2026-01-11"
        );
        assert_eq!(
            parsed["entries"][0]["critical_projection"]["projected_date"],
            "2026-01-16"
        );
    }
}
