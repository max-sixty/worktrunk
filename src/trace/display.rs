//! Display formatting for trace analysis output.

use super::analyze::TraceAnalysis;
use std::fmt::Write as _;

/// Render the complete analysis to a string.
pub fn render(analysis: &TraceAnalysis) -> String {
    let mut out = String::new();

    render_header(&mut out);
    render_command_breakdown(&mut out, analysis);
    render_histogram(&mut out, analysis);
    render_timeout_impact(&mut out, analysis);
    render_slowest(&mut out, analysis);

    out
}

fn render_header(out: &mut String) {
    out.push_str("============================================================\n");
    out.push_str("              TRACE PERFORMANCE ANALYSIS\n");
    out.push_str("============================================================\n");
}

fn render_command_breakdown(out: &mut String, analysis: &TraceAnalysis) {
    out.push_str("\nCOMMAND TYPE BREAKDOWN\n");
    out.push_str("----------------------\n");
    writeln!(
        out,
        "{:<15} {:>6} {:>9} {:>8} {:>8} {:>8}",
        "Command", "Count", "Total(s)", "p50(ms)", "p90(ms)", "Max(ms)"
    )
    .unwrap();
    writeln!(
        out,
        "{:<15} {:>6} {:>9} {:>8} {:>8} {:>8}",
        "---------------", "------", "---------", "--------", "--------", "--------"
    )
    .unwrap();

    for stat in &analysis.command_stats {
        writeln!(
            out,
            "{:<15} {:>6} {:>9.1} {:>8.0} {:>8.0} {:>8.0}",
            truncate(&stat.name, 15),
            stat.count,
            stat.total.as_secs_f64(),
            stat.p50.as_secs_f64() * 1000.0,
            stat.p90.as_secs_f64() * 1000.0,
            stat.max.as_secs_f64() * 1000.0,
        )
        .unwrap();
    }

    writeln!(
        out,
        "{:<15} {:>6} {:>9} {:>8} {:>8} {:>8}",
        "---------------", "------", "---------", "--------", "--------", "--------"
    )
    .unwrap();
    writeln!(
        out,
        "{:<15} {:>6} {:>9.1}",
        "TOTAL",
        "",
        analysis.total_duration.as_secs_f64()
    )
    .unwrap();
}

fn render_histogram(out: &mut String, analysis: &TraceAnalysis) {
    out.push_str("\nDURATION HISTOGRAM\n");
    out.push_str("------------------\n");

    let max_count = analysis
        .histogram
        .iter()
        .map(|b| b.count)
        .max()
        .unwrap_or(1)
        .max(1);

    for bucket in &analysis.histogram {
        let bar_len = (bucket.count as f64 / max_count as f64 * 30.0) as usize;
        let bar: String = "#".repeat(bar_len);

        let annotation = match bucket.label.as_str() {
            "<100ms" => " <-- 100ms",
            "<500ms" => " <-- 500ms",
            _ => "",
        };

        writeln!(
            out,
            "{:<10} |{:<30}| {:>4}{}",
            bucket.label, bar, bucket.count, annotation
        )
        .unwrap();
    }
}

fn render_timeout_impact(out: &mut String, analysis: &TraceAnalysis) {
    out.push_str("\nTIMEOUT IMPACT\n");
    out.push_str("--------------\n");
    writeln!(
        out,
        "{:<12} {:>8} {:>10} {:>10}",
        "Threshold", "Killed", "Saved(s)", "% Total"
    )
    .unwrap();
    writeln!(
        out,
        "{:<12} {:>8} {:>10} {:>10}",
        "------------", "--------", "----------", "----------"
    )
    .unwrap();

    for impact in &analysis.timeout_impacts {
        writeln!(
            out,
            "{:<12} {:>8} {:>9.1}s {:>9.0}%",
            format!(">{}ms", impact.threshold.as_millis()),
            impact.killed_count,
            impact.saved_duration.as_secs_f64(),
            impact.percent_of_total
        )
        .unwrap();
    }
}

fn render_slowest(out: &mut String, analysis: &TraceAnalysis) {
    out.push_str("\nTOP 10 SLOWEST COMMANDS\n");
    out.push_str("-----------------------\n");

    for (duration, command) in &analysis.slowest_commands {
        let cmd_display = truncate(command, 60);
        writeln!(
            out,
            "{:>7.0}ms  {}",
            duration.as_secs_f64() * 1000.0,
            cmd_display
        )
        .unwrap();
    }
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::{analyze::analyze, parse::parse_lines};

    #[test]
    fn test_render_sample_log() {
        let sample = include_str!("testdata/sample.log");
        let entries = parse_lines(sample);
        let analysis = analyze(&entries);
        let output = render(&analysis);

        insta::assert_snapshot!(output);
    }
}
