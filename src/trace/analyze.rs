//! Analyze trace entries to produce performance summaries.

use super::parse::TraceEntry;
use std::collections::HashMap;
use std::time::Duration;

/// Summary statistics for a group of commands.
#[derive(Debug, Clone)]
pub struct CommandStats {
    pub name: String,
    pub count: usize,
    pub total: Duration,
    pub p50: Duration,
    pub p90: Duration,
    pub max: Duration,
}

/// A bucket in the duration histogram.
#[derive(Debug, Clone)]
pub struct HistogramBucket {
    pub label: String,
    pub threshold_ms: Option<u64>, // None for the overflow bucket
    pub count: usize,
}

/// Impact of applying a timeout threshold.
#[derive(Debug, Clone)]
pub struct TimeoutImpact {
    pub threshold: Duration,
    pub killed_count: usize,
    pub saved_duration: Duration,
    pub percent_of_total: f64,
}

/// Complete analysis of trace entries.
#[derive(Debug)]
pub struct TraceAnalysis {
    /// Stats grouped by command type (git subcommand or program)
    pub command_stats: Vec<CommandStats>,
    /// Total duration across all commands
    pub total_duration: Duration,
    /// Duration histogram
    pub histogram: Vec<HistogramBucket>,
    /// Timeout impact analysis at various thresholds
    pub timeout_impacts: Vec<TimeoutImpact>,
    /// Slowest individual commands
    pub slowest_commands: Vec<(Duration, String)>,
}

/// Analyze trace entries and produce a complete summary.
pub fn analyze(entries: &[TraceEntry]) -> TraceAnalysis {
    let command_stats = compute_command_stats(entries);
    let total_duration = entries.iter().map(|e| e.duration).sum();
    let histogram = compute_histogram(entries);
    let timeout_impacts = compute_timeout_impacts(entries, total_duration);
    let slowest_commands = compute_slowest(entries, 10);

    TraceAnalysis {
        command_stats,
        total_duration,
        histogram,
        timeout_impacts,
        slowest_commands,
    }
}

/// Group commands and compute statistics for each group.
fn compute_command_stats(entries: &[TraceEntry]) -> Vec<CommandStats> {
    // Group by command type (git subcommand or program name)
    let mut groups: HashMap<String, Vec<Duration>> = HashMap::new();

    for entry in entries {
        let name = if let Some(subcmd) = entry.git_subcommand() {
            format!("git {}", subcmd)
        } else {
            entry.program().to_string()
        };

        groups.entry(name).or_default().push(entry.duration);
    }

    // Compute stats for each group
    let mut stats: Vec<CommandStats> = groups
        .into_iter()
        .map(|(name, mut durations)| {
            durations.sort();
            let count = durations.len();
            let total: Duration = durations.iter().sum();
            let p50 = percentile(&durations, 50);
            let p90 = percentile(&durations, 90);
            let max = durations.last().copied().unwrap_or(Duration::ZERO);

            CommandStats {
                name,
                count,
                total,
                p50,
                p90,
                max,
            }
        })
        .collect();

    // Sort by total time descending
    stats.sort_by(|a, b| b.total.cmp(&a.total));
    stats
}

/// Compute a percentile from sorted durations.
fn percentile(sorted: &[Duration], pct: usize) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let idx = (sorted.len() * pct / 100).min(sorted.len() - 1);
    sorted[idx]
}

/// Compute duration histogram with standard buckets.
fn compute_histogram(entries: &[TraceEntry]) -> Vec<HistogramBucket> {
    let thresholds_ms = [10, 50, 100, 200, 500, 1000, 2000, 5000];
    let mut counts = vec![0usize; thresholds_ms.len() + 1];

    for entry in entries {
        let ms = entry.duration.as_millis() as u64;
        let bucket = thresholds_ms
            .iter()
            .position(|&t| ms <= t)
            .unwrap_or(thresholds_ms.len());
        counts[bucket] += 1;
    }

    let mut buckets = Vec::with_capacity(thresholds_ms.len() + 1);
    for (i, &threshold) in thresholds_ms.iter().enumerate() {
        buckets.push(HistogramBucket {
            label: format!("<{}ms", threshold),
            threshold_ms: Some(threshold),
            count: counts[i],
        });
    }
    buckets.push(HistogramBucket {
        label: ">5s".to_string(),
        threshold_ms: None,
        count: counts[thresholds_ms.len()],
    });

    buckets
}

/// Compute impact of various timeout thresholds.
fn compute_timeout_impacts(entries: &[TraceEntry], total: Duration) -> Vec<TimeoutImpact> {
    let thresholds = [100, 200, 500, 1000];
    let total_ms = total.as_millis() as f64;

    thresholds
        .iter()
        .map(|&threshold_ms| {
            let threshold = Duration::from_millis(threshold_ms);
            let (killed_count, saved_duration) =
                entries.iter().fold((0, Duration::ZERO), |acc, e| {
                    if e.duration > threshold {
                        (acc.0 + 1, acc.1 + e.duration)
                    } else {
                        acc
                    }
                });

            let percent = if total_ms > 0.0 {
                saved_duration.as_millis() as f64 / total_ms * 100.0
            } else {
                0.0
            };

            TimeoutImpact {
                threshold,
                killed_count,
                saved_duration,
                percent_of_total: percent,
            }
        })
        .collect()
}

/// Get the N slowest commands.
fn compute_slowest(entries: &[TraceEntry], n: usize) -> Vec<(Duration, String)> {
    let mut sorted: Vec<_> = entries
        .iter()
        .map(|e| (e.duration, e.command.clone()))
        .collect();
    sorted.sort_by(|a, b| b.0.cmp(&a.0));
    sorted.truncate(n);
    sorted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::parse::parse_lines;

    #[test]
    fn test_percentile_empty() {
        // Edge case: percentile of empty slice returns ZERO
        assert_eq!(percentile(&[], 50), Duration::ZERO);
    }

    fn sample_trace() -> &'static str {
        r#"[wt-trace] cmd="git status" dur=10.0ms ok=true
[wt-trace] cmd="git status" dur=15.0ms ok=true
[wt-trace] cmd="git diff" dur=100.0ms ok=true
[wt-trace] cmd="git merge-base HEAD main" dur=500.0ms ok=true
[wt-trace] cmd="gh pr list" dur=200.0ms ok=true"#
    }

    #[test]
    fn test_command_stats() {
        let entries = parse_lines(sample_trace());
        let stats = compute_command_stats(&entries);

        // Should have 3 groups: git merge-base, gh, git diff, git status
        assert_eq!(stats.len(), 4);

        // First should be git merge-base (highest total)
        assert_eq!(stats[0].name, "git merge-base");
        assert_eq!(stats[0].count, 1);

        // git status should have count 2
        let status_stats = stats.iter().find(|s| s.name == "git status").unwrap();
        assert_eq!(status_stats.count, 2);
    }

    #[test]
    fn test_histogram() {
        let entries = parse_lines(sample_trace());
        let histogram = compute_histogram(&entries);

        // Buckets use <= threshold:
        // 10ms -> <10ms bucket (10 <= 10)
        // 15ms -> <50ms bucket
        // 100ms -> <100ms bucket (100 <= 100)
        // 200ms -> <200ms bucket (200 <= 200)
        // 500ms -> <500ms bucket (500 <= 500)
        let bucket_10 = histogram.iter().find(|b| b.label == "<10ms").unwrap();
        assert_eq!(bucket_10.count, 1); // 10ms

        let bucket_50 = histogram.iter().find(|b| b.label == "<50ms").unwrap();
        assert_eq!(bucket_50.count, 1); // 15ms
    }

    #[test]
    fn test_timeout_impact() {
        let entries = parse_lines(sample_trace());
        let total: Duration = entries.iter().map(|e| e.duration).sum();
        let impacts = compute_timeout_impacts(&entries, total);

        // Timeout kills commands > threshold (not >=)
        // At 100ms threshold: gh (200ms) and merge-base (500ms) would be killed
        // diff (100ms) is NOT killed since 100 is not > 100
        let impact_100 = impacts
            .iter()
            .find(|i| i.threshold.as_millis() == 100)
            .unwrap();
        assert_eq!(impact_100.killed_count, 2);
    }

    #[test]
    fn test_slowest() {
        let entries = parse_lines(sample_trace());
        let slowest = compute_slowest(&entries, 3);

        assert_eq!(slowest.len(), 3);
        assert_eq!(slowest[0].0, Duration::from_millis(500));
        assert!(slowest[0].1.contains("merge-base"));
    }

    #[test]
    fn test_empty_entries() {
        let entries: Vec<TraceEntry> = vec![];
        let analysis = super::analyze(&entries);

        assert!(analysis.command_stats.is_empty());
        assert_eq!(analysis.total_duration, Duration::ZERO);
        assert!(analysis.slowest_commands.is_empty());
        // Histogram buckets exist but all have 0 count
        assert!(analysis.histogram.iter().all(|b| b.count == 0));
        // Timeout impacts have 0% saved
        assert!(
            analysis
                .timeout_impacts
                .iter()
                .all(|i| i.percent_of_total == 0.0)
        );
    }
}
