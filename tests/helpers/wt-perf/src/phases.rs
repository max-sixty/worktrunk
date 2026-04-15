//! Phase-timing benchmark for `wt list` and `wt switch`.
//!
//! Drives the wt binary N times against a reproducible repo, parses
//! `[wt-trace]` entries emitted via `worktrunk::shell_exec::trace_instant`,
//! and aggregates per-phase durations (median / p95 / min / max).
//!
//! Phase duration = delta between consecutive `Instant` events in a run.
//! Commands whose start time falls in `[t_i, t_{i+1})` are attributed to
//! phase *i*.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

use worktrunk::trace::{TraceEntry, TraceEntryKind, parse_lines};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BenchCommand {
    List,
    Switch,
}

impl BenchCommand {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "list" => Some(Self::List),
            "switch" => Some(Self::Switch),
            _ => None,
        }
    }

    fn wt_args(self) -> &'static [&'static str] {
        match self {
            // Force progressive mode so `"Skeleton rendered"` and
            // `"First result received"` instants fire even with stdout
            // redirected to a pipe (non-TTY).
            Self::List => &["list", "--progressive"],
            Self::Switch => &["switch"],
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Switch => "switch",
        }
    }
}

pub struct RunTrace {
    pub entries: Vec<TraceEntry>,
    pub wall_us: u64,
}

/// Run `wt <command>` once against `repo` with `RUST_LOG=debug` and capture
/// `[wt-trace]` lines from stderr. `switch` is driven with
/// `WORKTRUNK_PICKER_DRY_RUN=1` so no TTY is required.
pub fn run_once(wt: &Path, repo: &Path, command: BenchCommand) -> std::io::Result<RunTrace> {
    let mut cmd = Command::new(wt);
    cmd.arg("-C").arg(repo);
    cmd.args(command.wt_args());
    cmd.env("RUST_LOG", "debug");
    if command == BenchCommand::Switch {
        cmd.env("WORKTRUNK_PICKER_DRY_RUN", "1");
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let start = Instant::now();
    let output = cmd.output()?;
    let wall_us = start.elapsed().as_micros() as u64;

    let stderr = String::from_utf8_lossy(&output.stderr);
    let entries = parse_lines(&stderr);
    Ok(RunTrace { entries, wall_us })
}

#[derive(Clone, Debug)]
pub struct PhaseSample {
    pub label: String,
    pub dur_us: u64,
    pub cmd_count: usize,
}

/// Extract per-phase samples from a single run.
///
/// Returns `(instants.len() - 1)` samples; labels are `"A → B"` from each
/// consecutive pair of instant events. Runs with fewer than two instants
/// return an empty vector.
pub fn phases(run: &RunTrace) -> Vec<PhaseSample> {
    let instants: Vec<(u64, &str)> = run
        .entries
        .iter()
        .filter_map(|e| match &e.kind {
            TraceEntryKind::Instant { name } => e.start_time_us.map(|t| (t, name.as_str())),
            _ => None,
        })
        .collect();

    if instants.len() < 2 {
        return Vec::new();
    }

    let commands: Vec<u64> = run
        .entries
        .iter()
        .filter_map(|e| match e.kind {
            TraceEntryKind::Command { .. } => e.start_time_us,
            _ => None,
        })
        .collect();

    instants
        .windows(2)
        .map(|pair| {
            let (t0, a) = pair[0];
            let (t1, b) = pair[1];
            PhaseSample {
                label: format!("{a} → {b}"),
                dur_us: t1.saturating_sub(t0),
                cmd_count: commands.iter().filter(|&&ts| ts >= t0 && ts < t1).count(),
            }
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct PhaseStats {
    pub label: String,
    pub median_us: u64,
    pub p95_us: u64,
    pub min_us: u64,
    pub max_us: u64,
    pub cmds_median: usize,
}

#[derive(Clone, Debug)]
pub struct Report {
    pub command: String,
    pub repo: String,
    pub runs: usize,
    pub warmup: usize,
    pub phases: Vec<PhaseStats>,
    pub wall_median_us: u64,
    pub wall_p95_us: u64,
}

pub fn aggregate(command: BenchCommand, repo: &str, runs: &[RunTrace], warmup: usize) -> Report {
    let timed_runs = &runs[warmup..];
    let per_run: Vec<Vec<PhaseSample>> = timed_runs.iter().map(phases).collect();
    let walls: Vec<u64> = timed_runs.iter().map(|r| r.wall_us).collect();

    let n_phases = per_run.first().map(|p| p.len()).unwrap_or(0);
    let mut phases_out = Vec::with_capacity(n_phases);
    for i in 0..n_phases {
        let label = per_run[0][i].label.clone();
        let mut durs: Vec<u64> = per_run
            .iter()
            .filter_map(|r| r.get(i).map(|s| s.dur_us))
            .collect();
        let mut cmds: Vec<usize> = per_run
            .iter()
            .filter_map(|r| r.get(i).map(|s| s.cmd_count))
            .collect();
        durs.sort_unstable();
        cmds.sort_unstable();
        phases_out.push(PhaseStats {
            label,
            median_us: pct_u64(&durs, 50),
            p95_us: pct_u64(&durs, 95),
            min_us: *durs.first().unwrap_or(&0),
            max_us: *durs.last().unwrap_or(&0),
            cmds_median: pct_usize(&cmds, 50),
        });
    }

    let mut walls_sorted = walls;
    walls_sorted.sort_unstable();

    Report {
        command: command.name().to_string(),
        repo: repo.to_string(),
        runs: timed_runs.len(),
        warmup,
        phases: phases_out,
        wall_median_us: pct_u64(&walls_sorted, 50),
        wall_p95_us: pct_u64(&walls_sorted, 95),
    }
}

fn pct_u64(sorted: &[u64], p: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() * p) / 100).min(sorted.len() - 1);
    sorted[idx]
}

fn pct_usize(sorted: &[usize], p: usize) -> usize {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() * p) / 100).min(sorted.len() - 1);
    sorted[idx]
}

pub fn format_human(r: &Report) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "wt {} @ {} — {} runs ({} warmup discarded)\n\n",
        r.command, r.repo, r.runs, r.warmup
    ));
    let header = format!(
        "{:<55}   {:>8} {:>8} {:>8} {:>8} {:>5}\n",
        "Phase", "median", "p95", "min", "max", "cmds"
    );
    out.push_str(&header);
    let rule_len = header.trim_end().chars().count();
    let rule: String = "─".repeat(rule_len);
    out.push_str(&rule);
    out.push('\n');
    for p in &r.phases {
        out.push_str(&format!(
            "{:<55}   {:>8} {:>8} {:>8} {:>8} {:>5}\n",
            truncate(&p.label, 55),
            fmt_us(p.median_us),
            fmt_us(p.p95_us),
            fmt_us(p.min_us),
            fmt_us(p.max_us),
            p.cmds_median,
        ));
    }
    out.push_str(&rule);
    out.push('\n');
    out.push_str(&format!(
        "{:<55}   {:>8} {:>8}\n",
        "Wall clock (child process)",
        fmt_us(r.wall_median_us),
        fmt_us(r.wall_p95_us),
    ));
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max - 1).collect();
        t.push('…');
        t
    }
}

fn fmt_us(us: u64) -> String {
    if us >= 1_000 {
        format!("{:.1}ms", us as f64 / 1_000.0)
    } else {
        format!("{us}µs")
    }
}

pub fn format_json(r: &Report) -> String {
    let phases: Vec<_> = r
        .phases
        .iter()
        .map(|p| {
            serde_json::json!({
                "label": p.label,
                "median_us": p.median_us,
                "p95_us": p.p95_us,
                "min_us": p.min_us,
                "max_us": p.max_us,
                "cmds_median": p.cmds_median,
            })
        })
        .collect();
    serde_json::to_string_pretty(&serde_json::json!({
        "command": r.command,
        "repo": r.repo,
        "runs": r.runs,
        "warmup": r.warmup,
        "phases": phases,
        "wall_median_us": r.wall_median_us,
        "wall_p95_us": r.wall_p95_us,
    }))
    .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_instant(name: &str, ts: u64) -> TraceEntry {
        TraceEntry {
            context: None,
            kind: TraceEntryKind::Instant {
                name: name.to_string(),
            },
            start_time_us: Some(ts),
            thread_id: Some(0),
        }
    }

    #[test]
    fn phases_from_instants() {
        let run = RunTrace {
            entries: vec![
                fake_instant("A", 1_000),
                fake_instant("B", 3_500),
                fake_instant("C", 4_000),
            ],
            wall_us: 4_200,
        };
        let samples = phases(&run);
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0].label, "A → B");
        assert_eq!(samples[0].dur_us, 2_500);
        assert_eq!(samples[1].label, "B → C");
        assert_eq!(samples[1].dur_us, 500);
    }

    #[test]
    fn aggregate_picks_median_and_p95() {
        let runs: Vec<RunTrace> = [100u64, 200, 300, 400, 500]
            .iter()
            .map(|&d| RunTrace {
                entries: vec![fake_instant("A", 0), fake_instant("B", d)],
                wall_us: d,
            })
            .collect();
        let r = aggregate(BenchCommand::List, "repo", &runs, 0);
        assert_eq!(r.phases.len(), 1);
        assert_eq!(r.phases[0].label, "A → B");
        assert_eq!(r.phases[0].min_us, 100);
        assert_eq!(r.phases[0].max_us, 500);
        // median at p=50 of 5 elements → index 2 → 300
        assert_eq!(r.phases[0].median_us, 300);
    }
}
