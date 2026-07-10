//! Render trace entries as a start-time-sorted text timeline.
//!
//! The per-record view of a single `wt` invocation: one row per
//! command/span/instant in start order, then a summary of subprocess totals,
//! the traced span, and the externally measured wall. Complements
//! [`Profile`](super::Profile), the aggregate view over the same records;
//! `wt-perf timeline` is the consumer, and supplies `wall` from its own
//! spawn → wait measurement (the trace can't see the process prelude — argv
//! parsing, dyld, code before `init_logging` sets the trace epoch — or the
//! exit path, so the gap between `traced` and `wall` is the unobserved
//! overhead). Labels and tables come from the same helpers as
//! [`Profile::render_text`](super::Profile::render_text), so the two views
//! can't drift.

use std::time::Duration;

use super::parse::{TraceEntry, TraceEntryKind};
use super::profile::{Align, command_label, fmt_dur, render_table};

/// Duration of an entry (zero for instant events).
fn duration_of(e: &TraceEntry) -> Duration {
    match &e.kind {
        TraceEntryKind::Command { duration, .. } | TraceEntryKind::Span { duration, .. } => {
            *duration
        }
        TraceEntryKind::Instant { .. } => Duration::ZERO,
    }
}

/// Render parsed entries as a column-aligned, start-time-sorted timeline.
pub fn render_timeline(entries: &[TraceEntry], wall: Duration) -> String {
    let mut sorted: Vec<&TraceEntry> = entries.iter().collect();
    sorted.sort_by_key(|e| e.start_time_us.unwrap_or(0));

    let mut rows = vec![
        ["ts(ms)", "dur", "tid", "kind", "name"]
            .map(str::to_string)
            .to_vec(),
    ];
    for e in &sorted {
        let (kind, dur, name) = match &e.kind {
            TraceEntryKind::Command {
                command,
                duration,
                result,
                ..
            } => (
                "cmd",
                fmt_dur(*duration),
                command_label(command, e.context.as_deref(), result),
            ),
            TraceEntryKind::Span { name, duration } => ("span", fmt_dur(*duration), name.clone()),
            TraceEntryKind::Instant { name } => ("event", String::new(), name.clone()),
        };
        rows.push(vec![
            format!("{:.3}", e.start_time_us.unwrap_or(0) as f64 / 1_000.0),
            dur,
            e.thread_id
                .map(|t| t.to_string())
                .unwrap_or_else(|| "-".into()),
            kind.to_string(),
            name,
        ]);
    }
    let mut out = render_table(
        &rows,
        &[
            Align::Right,
            Align::Right,
            Align::Right,
            Align::Left,
            Align::Left,
        ],
    );

    // Summary: subprocess totals + traced span + true process wall.
    let cmds: Vec<(Duration, String)> = sorted
        .iter()
        .filter_map(|e| match &e.kind {
            TraceEntryKind::Command {
                command,
                duration,
                result,
                ..
            } => Some((
                *duration,
                command_label(command, e.context.as_deref(), result),
            )),
            _ => None,
        })
        .collect();
    let cmd_total: Duration = cmds.iter().map(|(d, _)| *d).sum();
    let slowest = cmds.iter().max_by_key(|(d, _)| *d);
    let traced = Duration::from_micros(
        sorted
            .iter()
            .map(|e| e.start_time_us.unwrap_or(0) + duration_of(e).as_micros() as u64)
            .max()
            .unwrap_or(0)
            .saturating_sub(
                sorted
                    .iter()
                    .map(|e| e.start_time_us.unwrap_or(0))
                    .min()
                    .unwrap_or(0),
            ),
    );
    let untraced = wall.saturating_sub(traced);

    out.push('\n');
    if let Some((dur, name)) = slowest {
        let plural = if cmds.len() == 1 { "" } else { "es" };
        out.push_str(&format!(
            "{} subprocess{plural} totaling {} (slowest: {} {name})\n",
            cmds.len(),
            fmt_dur(cmd_total),
            fmt_dur(*dur),
        ));
    } else {
        out.push_str("0 subprocesses\n");
    }
    out.push_str(&format!(
        "traced: {} (first → last record)\n",
        fmt_dur(traced)
    ));
    out.push_str(&format!(
        "wall:   {} (spawn → wait; +{} untraced prelude/epilogue)\n",
        fmt_dur(wall),
        fmt_dur(untraced),
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::TraceResult;

    fn span(name: &str, ts_us: u64, dur_us: u64, tid: u64) -> TraceEntry {
        TraceEntry {
            context: None,
            kind: TraceEntryKind::Span {
                name: name.to_string(),
                duration: Duration::from_micros(dur_us),
            },
            start_time_us: Some(ts_us),
            thread_id: Some(tid),
        }
    }

    fn cmd(
        cmd: &str,
        ctx: Option<&str>,
        ts_us: u64,
        dur_us: u64,
        tid: u64,
        ok: bool,
    ) -> TraceEntry {
        TraceEntry {
            context: ctx.map(|s| s.to_string()),
            kind: TraceEntryKind::Command {
                command: cmd.to_string(),
                duration: Duration::from_micros(dur_us),
                result: TraceResult::Completed { success: ok },
                reads_stdin: false,
            },
            start_time_us: Some(ts_us),
            thread_id: Some(tid),
        }
    }

    fn instant(name: &str, ts_us: u64) -> TraceEntry {
        TraceEntry {
            context: None,
            kind: TraceEntryKind::Instant {
                name: name.to_string(),
            },
            start_time_us: Some(ts_us),
            thread_id: Some(1),
        }
    }

    #[test]
    fn renders_sorted_timeline_with_summary() {
        // Emit order swaps span and child cmd (parent finishes after child),
        // so this exercises the sort-by-start-time guarantee. The instant
        // event renders as an `event` row with an empty duration cell, stays
        // out of the subprocess summary, and doesn't extend the traced span.
        let entries = vec![
            cmd("git rev-parse HEAD", Some("repo"), 50, 4_000, 1, true),
            span("prewarm", 30, 4_100, 1),
            span("init_logging", 0, 8, 1),
            instant("Skeleton rendered", 3_000),
            span("user_config_load", 4_200, 280, 38),
        ];
        // Wall = 6ms; traced = 4.48ms (4.2ms start → 4.48ms end);
        // untraced prelude/epilogue = 6 - 4.48 = 1.52ms.
        insta::assert_snapshot!(
            render_timeline(&entries, Duration::from_micros(6_000)),
            @"
          ts(ms)     dur  tid  kind   name
           0.000  0.01ms    1  span   init_logging
           0.030  4.10ms    1  span   prewarm
           0.050  4.00ms    1  cmd    git rev-parse HEAD [repo]
           3.000            1  event  Skeleton rendered
           4.200  0.28ms   38  span   user_config_load

        1 subprocess totaling 4.00ms (slowest: 4.00ms git rev-parse HEAD [repo])
        traced: 4.48ms (first → last record)
        wall:   6.00ms (spawn → wait; +1.52ms untraced prelude/epilogue)
        "
        );
    }

    #[test]
    fn cmd_failure_annotates_name() {
        let entries = vec![cmd("git foo", None, 0, 1_000, 1, false)];
        insta::assert_snapshot!(
            render_timeline(&entries, Duration::from_millis(2)),
            @"
          ts(ms)     dur  tid  kind  name
           0.000  1.00ms    1  cmd   git foo (ok=false)

        1 subprocess totaling 1.00ms (slowest: 1.00ms git foo (ok=false))
        traced: 1.00ms (first → last record)
        wall:   2.00ms (spawn → wait; +1.00ms untraced prelude/epilogue)
        "
        );
    }
}
