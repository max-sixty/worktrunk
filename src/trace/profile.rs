//! Aggregate `[wt-trace]` records into a performance profile.
//!
//! Where [`parse`](super::parse) turns trace lines into [`TraceEntry`] values and
//! [`chrome`](super::chrome) exports them for Perfetto, this module answers the
//! three questions [`benches/CLAUDE.md`] poses about a single `wt` invocation
//! without leaving the terminal:
//!
//! - **Where does time go?** — [`Profile::by_type`] groups subprocesses by command
//!   shape (`git status`, `git rev-list`, `gh pr list`) with count/total/max/avg,
//!   and [`Profile::slowest`] lists the most expensive individual jobs.
//! - **How parallel are we?** — [`Profile::parallelism`] is Σ(subprocess time) ÷
//!   their wall span; [`Profile::peak_concurrency`] is the most subprocesses in
//!   flight at once.
//! - **Where is work wasted?** — [`CacheReport`] flags commands re-run with the
//!   same context (a cache miss that should have been a hit). Commands that read
//!   stdin are excluded — their real input isn't in the command string, so
//!   identical command lines aren't necessarily identical work.
//!
//! The analysis ([`Profile::from_entries`], [`CacheReport::from_entries`]) is pure
//! data over `&[TraceEntry]` and carries no styling, so it compiles without the
//! `cli` feature and is shared by both `wt config state logs profile` (which
//! renders via [`Profile::render_text`] or serializes the struct for `--format=json`)
//! and the `wt-perf` helper (which reuses [`CacheReport`] for its `cache-check`
//! output). The struct's `Serialize` impl is the single canonical JSON source.
//!
//! [`benches/CLAUDE.md`]: ../../../benches/CLAUDE.md

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;

use serde::Serialize;

use crate::styling::format_heading;

use super::{TraceEntry, TraceEntryKind, TraceResult};

/// How many individual calls [`Profile::slowest`] retains.
const SLOWEST_LIMIT: usize = 8;
/// How many cache offenders the text summary lists before collapsing the rest.
const CACHE_OFFENDER_LIMIT: usize = 3;

// Milestone event strings emitted by the `wt list` / picker collect pipeline
// (`src/commands/list/collect/mod.rs`). These literals must match the emit
// sites; a rename there is caught by the integration test
// `test_logs_profile_real_capture_has_key_intervals`.
//
// `List collect started`, `Parallel execution started`, `All results drained`,
// and `List collect complete` always fire on a collect run, so `collect_total`
// and `parallel_phase` are present in any capture. `Skeleton rendered` fires
// only for a progressive consumer — a TTY `wt list` or the picker — and
// `First result received` only for the progressive `wt list` table; both are
// absent from a piped capture, so the intervals built from them are too.
const M_COLLECT_STARTED: &str = "List collect started";
const M_SKELETON: &str = "Skeleton rendered";
const M_PARALLEL_STARTED: &str = "Parallel execution started";
const M_FIRST_RESULT: &str = "First result received";
const M_DRAINED: &str = "All results drained";
const M_COLLECT_COMPLETE: &str = "List collect complete";

/// Serialize a `Duration` as integer microseconds (the `dur_us` wire convention).
fn ser_dur_us<S: serde::Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_u64(d.as_micros() as u64)
}

/// Serialize an optional `Duration` as nullable microseconds.
fn ser_opt_dur_us<S: serde::Serializer>(d: &Option<Duration>, s: S) -> Result<S::Ok, S::Error> {
    match d {
        Some(d) => s.serialize_u64(d.as_micros() as u64),
        None => s.serialize_none(),
    }
}

/// A performance profile derived from a set of `[wt-trace]` records.
///
/// The struct IS the canonical result: `--format=json` serializes it directly
/// (durations as `*_us` microseconds), and [`Profile::render_text`] renders the
/// same fields, so the two views can't drift.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Profile {
    /// First → last record, across commands, spans, and instant events.
    #[serde(rename = "traced_us", serialize_with = "ser_dur_us")]
    pub traced: Duration,
    /// Number of subprocess (command) records.
    pub command_count: usize,
    /// Σ of every subprocess duration.
    #[serde(rename = "command_total_us", serialize_with = "ser_dur_us")]
    pub command_total: Duration,
    /// Σ of in-process span durations (config load, repo open, …).
    #[serde(rename = "span_total_us", serialize_with = "ser_dur_us")]
    pub span_total: Duration,
    /// Σ(timed subprocess time) ÷ (their wall span). `None` when no command
    /// carries a timestamp (e.g. a log captured before `ts`/`tid` existed) or
    /// the span is zero.
    pub parallelism: Option<f64>,
    /// Most subprocesses in flight simultaneously. `None` without timestamps.
    pub peak_concurrency: Option<usize>,
    /// Distinct thread IDs that ran subprocesses.
    pub thread_count: usize,
    /// Derived `wt list` latencies from the collect milestones.
    pub key_intervals: KeyIntervals,
    /// Subprocess time grouped by command shape, busiest first.
    pub by_type: Vec<TypeStat>,
    /// The most expensive individual jobs (commands and spans), slowest first.
    pub slowest: Vec<Slow>,
    /// Redundant same-context commands.
    pub cache: CacheReport,
    /// Milestone events with the gap since the previous milestone.
    pub phases: Vec<Phase>,
    /// Whether this capture is a `wt list`/picker collect run (the start anchor
    /// milestone is present). Drives the skeleton/first-result "not recorded"
    /// note; not serialized (derivable from `phases`).
    #[serde(skip)]
    pub collect_run: bool,
}

/// Derived latencies from the `wt list` collect milestones. Each is `None` when
/// an endpoint milestone is absent — `time_to_skeleton`/`time_to_first_result`
/// need a TTY/`--progressive` capture; `collect_total`/`parallel_phase` are
/// built from always-fire milestones and present in any collect capture.
// Field order matches the KEY INTERVALS render order so the text and JSON views
// agree (serde follows declaration order).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct KeyIntervals {
    /// `Skeleton rendered` − `List collect started`: the headline perceived latency.
    #[serde(rename = "time_to_skeleton_us", serialize_with = "ser_opt_dur_us")]
    pub time_to_skeleton: Option<Duration>,
    /// `First result received` − `List collect started`.
    #[serde(rename = "time_to_first_result_us", serialize_with = "ser_opt_dur_us")]
    pub time_to_first_result: Option<Duration>,
    /// `All results drained` − `Parallel execution started`: the parallel fan-out.
    #[serde(rename = "parallel_phase_us", serialize_with = "ser_opt_dur_us")]
    pub parallel_phase: Option<Duration>,
    /// `List collect complete` − `List collect started`: whole collection.
    #[serde(rename = "collect_total_us", serialize_with = "ser_opt_dur_us")]
    pub collect_total: Option<Duration>,
}

impl KeyIntervals {
    /// True when no interval could be computed (not a collect capture).
    fn is_empty(&self) -> bool {
        self.time_to_skeleton.is_none()
            && self.time_to_first_result.is_none()
            && self.collect_total.is_none()
            && self.parallel_phase.is_none()
    }
}

/// Aggregated timing for one command shape (e.g. `git status`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TypeStat {
    /// The command shape — program plus leading subcommand tokens.
    #[serde(rename = "command")]
    pub key: String,
    pub count: usize,
    #[serde(rename = "total_us", serialize_with = "ser_dur_us")]
    pub total: Duration,
    #[serde(rename = "max_us", serialize_with = "ser_dur_us")]
    pub max: Duration,
    #[serde(rename = "avg_us", serialize_with = "ser_dur_us")]
    pub avg: Duration,
}

/// One expensive job in [`Profile::slowest`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Slow {
    #[serde(rename = "dur_us", serialize_with = "ser_dur_us")]
    pub duration: Duration,
    /// Display label — `cmd [context]` (with a failure marker) or `span:name`.
    pub label: String,
}

/// A milestone event and the gap since the previous one.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Phase {
    pub name: String,
    /// Offset from the first record.
    #[serde(rename = "at_us", serialize_with = "ser_dur_us")]
    pub at: Duration,
    /// Gap since the previous milestone (`None` for the first).
    #[serde(rename = "delta_us", serialize_with = "ser_opt_dur_us")]
    pub delta: Option<Duration>,
}

/// Redundant-command analysis: the same `(command, context)` run more than once
/// in a single invocation is a cache that should have hit but didn't.
///
/// The duplicate fields (`unique_commands`, `duplicated_commands`,
/// `extra_calls`, `same_context_*`) only consider commands whose work is fully
/// determined by `(command, context)`. A command that reads stdin
/// (`stdin=true`: a `claude -p` prompt, a diff piped to `git patch-id`) carries
/// input the command string doesn't capture, so two runs with identical
/// `(command, context)` may be entirely different work — it's counted in
/// `total_commands`/`total_time`/`contexts` but never reported as a duplicate.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CacheReport {
    /// Every command record, including stdin-reading ones.
    pub total_commands: usize,
    /// Distinct dedupable command strings (stdin-reading commands excluded).
    pub unique_commands: usize,
    pub contexts: usize,
    /// Σ of every command duration, including stdin-reading ones.
    #[serde(rename = "total_time_us", serialize_with = "ser_dur_us")]
    pub total_time: Duration,
    /// Distinct commands run more than once (in any context).
    pub duplicated_commands: usize,
    /// Extra runs beyond the first, regardless of context.
    pub extra_calls: usize,
    /// Commands re-run within the same context, worst waste first.
    pub same_context_duplicates: Vec<DuplicateCommand>,
    /// Extra same-context runs beyond the first.
    pub same_context_extra_calls: usize,
    /// Time spent on those extra same-context runs (slowest run per context kept).
    #[serde(rename = "same_context_extra_us", serialize_with = "ser_dur_us")]
    pub same_context_extra: Duration,
}

/// One command re-run within the same context.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DuplicateCommand {
    pub command: String,
    /// Highest run-count across this command's contexts.
    pub max_per_context: usize,
    pub extra_calls: usize,
    #[serde(rename = "extra_us", serialize_with = "ser_dur_us")]
    pub extra: Duration,
    pub contexts: Vec<DuplicateContext>,
}

/// Per-context run counts for a [`DuplicateCommand`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DuplicateContext {
    pub context: String,
    pub count: usize,
    #[serde(rename = "total_us", serialize_with = "ser_dur_us")]
    pub total: Duration,
}

/// The command shape used to group subprocesses: the program plus up to two
/// leading subcommand tokens.
///
/// Leading global flags before the subcommand are skipped (`git
/// --no-optional-locks status` → `git status`; `-c`/`-C` also consume their
/// value). A subcommand token is then lowercase letters and dashes only
/// (`status`, `for-each-ref`, `rev-list`, `pr`, `list`), so `git status
/// --porcelain` and `gh pr list --json …` collapse to `git status` and `gh pr
/// list`. The walk stops at the first flag, ref, range, path, or SHA.
///
/// The two-token allowance keeps real second-level subcommands distinct (`git
/// worktree list`, `gh pr list`), at the cost of a bare lowercase positional
/// operand also being absorbed — `git checkout main` and `git checkout feature`
/// land in separate buckets. worktrunk's hot commands (`status`, `rev-list`,
/// `merge-base`, …) take flags/refs/SHAs as their first operand, so this only
/// bites branch-name operands in a single invocation, which `wt list` doesn't
/// emit.
fn command_type(command: &str) -> String {
    let mut tokens = command.split_whitespace();
    let Some(program) = tokens.next() else {
        return String::new();
    };
    let mut parts = vec![program];

    let mut tokens = tokens.peekable();
    while let Some(tok) = tokens.peek() {
        if tok.starts_with('-') {
            let consumes_value = matches!(*tok, "-c" | "-C");
            tokens.next();
            if consumes_value {
                tokens.next();
            }
        } else {
            break;
        }
    }

    for tok in tokens {
        if parts.len() < 3
            && tok.starts_with(|c: char| c.is_ascii_lowercase())
            && tok.chars().all(|c| c.is_ascii_lowercase() || c == '-')
        {
            parts.push(tok);
        } else {
            break;
        }
    }
    parts.join(" ")
}

/// Display label for one slowest-list command.
fn command_label(command: &str, context: Option<&str>, result: &TraceResult) -> String {
    let mut label = match context {
        Some(c) => format!("{command} [{c}]"),
        None => command.to_string(),
    };
    match result {
        TraceResult::Completed { success: false } => label.push_str(" (ok=false)"),
        TraceResult::Error { message } => label.push_str(&format!(" (err: {message})")),
        TraceResult::Completed { success: true } => {}
    }
    label
}

/// Microseconds of an entry's duration (zero for instant events).
fn entry_dur_us(entry: &TraceEntry) -> u64 {
    match &entry.kind {
        TraceEntryKind::Command { duration, .. } | TraceEntryKind::Span { duration, .. } => {
            duration.as_micros() as u64
        }
        TraceEntryKind::Instant { .. } => 0,
    }
}

impl Profile {
    /// Build a profile from parsed trace entries.
    pub fn from_entries(entries: &[TraceEntry]) -> Self {
        let min_start = entries.iter().filter_map(|e| e.start_time_us).min();
        let max_end = entries
            .iter()
            .filter_map(|e| e.start_time_us.map(|s| s + entry_dur_us(e)))
            .max();
        let traced = match (min_start, max_end) {
            (Some(a), Some(b)) => Duration::from_micros(b.saturating_sub(a)),
            _ => Duration::ZERO,
        };

        let mut command_count = 0;
        let mut command_total_us = 0u64;
        let mut span_total_us = 0u64;
        let mut by_type_map: BTreeMap<String, (usize, u64, u64)> = BTreeMap::new();
        let mut threads: HashSet<u64> = HashSet::new();
        // (start, end) for timed subprocesses — drives parallelism & peak concurrency.
        let mut intervals: Vec<(u64, u64)> = Vec::new();
        let mut slowest: Vec<Slow> = Vec::new();

        for entry in entries {
            match &entry.kind {
                TraceEntryKind::Command {
                    command,
                    duration,
                    result,
                    ..
                } => {
                    let dur_us = duration.as_micros() as u64;
                    command_count += 1;
                    command_total_us += dur_us;
                    let stat = by_type_map.entry(command_type(command)).or_default();
                    stat.0 += 1;
                    stat.1 += dur_us;
                    stat.2 = stat.2.max(dur_us);
                    if let Some(tid) = entry.thread_id {
                        threads.insert(tid);
                    }
                    if let Some(start) = entry.start_time_us {
                        intervals.push((start, start + dur_us));
                    }
                    slowest.push(Slow {
                        duration: *duration,
                        label: command_label(command, entry.context.as_deref(), result),
                    });
                }
                TraceEntryKind::Span { name, duration } => {
                    span_total_us += duration.as_micros() as u64;
                    slowest.push(Slow {
                        duration: *duration,
                        label: format!("span:{name}"),
                    });
                }
                TraceEntryKind::Instant { .. } => {}
            }
        }

        let mut by_type: Vec<TypeStat> = by_type_map
            .into_iter()
            .map(|(key, (count, total, max))| TypeStat {
                key,
                count,
                total: Duration::from_micros(total),
                max: Duration::from_micros(max),
                avg: Duration::from_micros(total / count as u64),
            })
            .collect();
        by_type.sort_by(|a, b| b.total.cmp(&a.total).then_with(|| a.key.cmp(&b.key)));

        slowest.sort_by_key(|s| std::cmp::Reverse(s.duration));
        slowest.truncate(SLOWEST_LIMIT);

        let (parallelism, peak_concurrency) = concurrency(&intervals);

        let base = min_start.unwrap_or(0);
        let mut milestones: Vec<(&str, u64)> = entries
            .iter()
            .filter_map(|e| match &e.kind {
                TraceEntryKind::Instant { name } => e.start_time_us.map(|s| (name.as_str(), s)),
                _ => None,
            })
            .collect();
        milestones.sort_by_key(|(_, start)| *start);

        // Absolute timestamp of each milestone (first occurrence) for the
        // derived intervals. `Skeleton rendered` appears at most once per run.
        let mut milestone_at: BTreeMap<&str, u64> = BTreeMap::new();
        for (name, start) in &milestones {
            milestone_at.entry(name).or_insert(*start);
        }
        let interval = |from: &str, to: &str| match (milestone_at.get(from), milestone_at.get(to)) {
            (Some(&a), Some(&b)) if b >= a => Some(Duration::from_micros(b - a)),
            _ => None,
        };
        let key_intervals = KeyIntervals {
            time_to_skeleton: interval(M_COLLECT_STARTED, M_SKELETON),
            time_to_first_result: interval(M_COLLECT_STARTED, M_FIRST_RESULT),
            collect_total: interval(M_COLLECT_STARTED, M_COLLECT_COMPLETE),
            parallel_phase: interval(M_PARALLEL_STARTED, M_DRAINED),
        };
        let collect_run = milestone_at.contains_key(M_COLLECT_STARTED);

        let mut phases = Vec::with_capacity(milestones.len());
        let mut prev: Option<u64> = None;
        for (name, start) in milestones {
            let at = start.saturating_sub(base);
            phases.push(Phase {
                name: name.to_string(),
                at: Duration::from_micros(at),
                delta: prev.map(|p| Duration::from_micros(at.saturating_sub(p))),
            });
            prev = Some(at);
        }

        Profile {
            traced,
            command_count,
            command_total: Duration::from_micros(command_total_us),
            span_total: Duration::from_micros(span_total_us),
            parallelism,
            peak_concurrency,
            thread_count: threads.len(),
            key_intervals,
            by_type,
            slowest,
            cache: CacheReport::from_entries(entries),
            phases,
            collect_run,
        }
    }

    /// Render the profile as a human-readable report (ANSI styling auto-strips
    /// on a pipe). `source` labels where the records came from.
    pub fn render_text(&self, source: &str) -> String {
        let mut out = String::new();

        out.push_str(&format_heading(
            "PERFORMANCE PROFILE",
            Some(&format!("@ {source}")),
        ));
        out.push('\n');
        let parallelism = match self.parallelism {
            Some(p) => format!("parallelism {p:.1}×"),
            None => "parallelism n/a".to_string(),
        };
        let peak = match self.peak_concurrency {
            Some(p) => format!("peak {p}"),
            None => "peak n/a".to_string(),
        };
        // Build the summary from segments so the timestamp-derived fields
        // (thread count, traced wall span) drop out for a log without `ts`/`tid`
        // rather than rendering a confusing `0 threads · 0.00ms traced`.
        // Every magnitude leads with its value (`32.00ms subprocess time`,
        // `5 subprocesses`); the derived metrics (`parallelism`, `peak`) lead
        // with their label since they aren't measured quantities.
        let mut segments = vec![
            format!(
                "{} subprocess{}",
                self.command_count,
                plural(self.command_count, "es")
            ),
            format!("{} subprocess time", fmt_dur(self.command_total)),
            parallelism,
            peak,
        ];
        if self.thread_count > 0 {
            segments.push(format!(
                "{} thread{}",
                self.thread_count,
                plural(self.thread_count, "s")
            ));
        }
        if !self.traced.is_zero() {
            segments.push(format!("{} traced", fmt_dur(self.traced)));
        }
        if !self.span_total.is_zero() {
            segments.push(format!("{} in-process", fmt_dur(self.span_total)));
        }
        out.push_str("  ");
        out.push_str(&segments.join(" · "));
        out.push('\n');

        if !self.by_type.is_empty() {
            out.push('\n');
            out.push_str(&format_heading("BY COMMAND TYPE", None));
            out.push('\n');
            let mut rows = vec![vec![
                "command".to_string(),
                "count".to_string(),
                "total".to_string(),
                "max".to_string(),
                "avg".to_string(),
            ]];
            for stat in &self.by_type {
                rows.push(vec![
                    stat.key.clone(),
                    stat.count.to_string(),
                    fmt_dur(stat.total),
                    fmt_dur(stat.max),
                    fmt_dur(stat.avg),
                ]);
            }
            out.push_str(&render_table(
                &rows,
                &[
                    Align::Left,
                    Align::Right,
                    Align::Right,
                    Align::Right,
                    Align::Right,
                ],
            ));
        }

        if !self.slowest.is_empty() {
            out.push('\n');
            out.push_str(&format_heading("SLOWEST CALLS", None));
            out.push('\n');
            let rows: Vec<Vec<String>> = self
                .slowest
                .iter()
                .map(|s| vec![fmt_dur(s.duration), s.label.clone()])
                .collect();
            out.push_str(&render_table(&rows, &[Align::Right, Align::Left]));
        }

        let cache = &self.cache;
        out.push('\n');
        out.push_str(&format_heading("CACHE", None));
        out.push('\n');
        if cache.same_context_extra_calls == 0 {
            out.push_str("  no commands re-run with the same context\n");
        } else {
            out.push_str(&format!(
                "  {} duplicate call{} wasting ~{} (same context, slowest run kept)\n",
                cache.same_context_extra_calls,
                plural(cache.same_context_extra_calls, "s"),
                fmt_dur(cache.same_context_extra),
            ));
            let rows: Vec<Vec<String>> = cache
                .same_context_duplicates
                .iter()
                .take(CACHE_OFFENDER_LIMIT)
                .map(|dup| {
                    vec![
                        fmt_dur(dup.extra),
                        format!("{} (×{})", dup.command, dup.max_per_context),
                    ]
                })
                .collect();
            out.push_str(&render_table(&rows, &[Align::Right, Align::Left]));
            let remaining = cache
                .same_context_duplicates
                .len()
                .saturating_sub(CACHE_OFFENDER_LIMIT);
            if remaining > 0 {
                out.push_str(&format!(
                    "  … and {remaining} more command{}\n",
                    plural(remaining, "s")
                ));
            }
        }

        let ki = &self.key_intervals;
        if !ki.is_empty() {
            out.push('\n');
            out.push_str(&format_heading("KEY INTERVALS", None));
            out.push('\n');
            let items = [
                ("time to skeleton", ki.time_to_skeleton),
                ("time to first result", ki.time_to_first_result),
                ("parallel phase", ki.parallel_phase),
                ("collect total", ki.collect_total),
            ];
            let rows: Vec<Vec<String>> = items
                .iter()
                .filter_map(|(label, d)| d.map(|d| vec![label.to_string(), fmt_dur(d)]))
                .collect();
            out.push_str(&render_table(&rows, &[Align::Left, Align::Right]));
        }
        if self.collect_run && ki.time_to_skeleton.is_none() {
            out.push_str(&color_print::cformat!(
                "  <dim>skeleton/first-result not recorded — these need an interactive terminal; re-run in a terminal, or use wt-perf timeline -- list --progressive</>\n"
            ));
        }

        if !self.phases.is_empty() {
            out.push('\n');
            out.push_str(&format_heading("PHASES", None));
            out.push('\n');
            let rows: Vec<Vec<String>> = self
                .phases
                .iter()
                .map(|p| {
                    let delta = match p.delta {
                        Some(d) => format!("+{}", fmt_dur(d)),
                        None => String::new(),
                    };
                    vec![fmt_dur(p.at), delta, p.name.clone()]
                })
                .collect();
            out.push_str(&render_table(
                &rows,
                &[Align::Right, Align::Right, Align::Left],
            ));
        }

        out
    }
}

impl CacheReport {
    /// Build a redundant-command report from parsed trace entries.
    pub fn from_entries(entries: &[TraceEntry]) -> Self {
        let mut total_commands = 0;
        let mut total_time_us = 0u64;
        let mut cmd_counts: HashMap<&str, usize> = HashMap::new();
        let mut contexts: HashSet<&str> = HashSet::new();
        // (command, context) → durations of every run, in microseconds.
        let mut pair_durations: HashMap<(&str, &str), Vec<u64>> = HashMap::new();

        for entry in entries {
            if let TraceEntryKind::Command {
                command,
                duration,
                reads_stdin,
                ..
            } = &entry.kind
            {
                let ctx = entry.context.as_deref().unwrap_or("(none)");
                let dur_us = duration.as_micros() as u64;
                total_commands += 1;
                total_time_us += dur_us;
                contexts.insert(ctx);
                // Duplicate analysis assumes (command, context) fully determines
                // a command's work. A command that reads stdin carries input the
                // command string doesn't capture, so two runs with identical
                // (command, context) may be entirely different work (a different
                // prompt piped to `claude -p`, a different diff to `git
                // patch-id`). Count it in the totals above, but never let it
                // form — or join — a duplicate bucket.
                if *reads_stdin {
                    continue;
                }
                *cmd_counts.entry(command.as_str()).or_default() += 1;
                pair_durations
                    .entry((command.as_str(), ctx))
                    .or_default()
                    .push(dur_us);
            }
        }

        // Group same-context repeats by command.
        let mut by_command: BTreeMap<&str, Vec<(&str, &Vec<u64>)>> = BTreeMap::new();
        for ((cmd, ctx), durations) in &pair_durations {
            if durations.len() > 1 {
                by_command.entry(cmd).or_default().push((ctx, durations));
            }
        }

        let mut same_context_duplicates = Vec::new();
        let mut same_context_extra_calls = 0usize;
        let mut same_context_extra_us = 0u64;
        for (cmd, ctx_list) in &by_command {
            let max_per_context = ctx_list.iter().map(|(_, d)| d.len()).max().unwrap_or(0);
            let extra_calls: usize = ctx_list.iter().map(|(_, d)| d.len() - 1).sum();
            same_context_extra_calls += extra_calls;
            // Wasted time: keep the slowest run per context (the likely cold
            // call), sum the rest.
            let extra_us: u64 = ctx_list
                .iter()
                .map(|(_, durations)| {
                    let max = durations.iter().max().copied().unwrap_or(0);
                    durations.iter().sum::<u64>() - max
                })
                .sum();
            same_context_extra_us += extra_us;

            let mut context_stats: Vec<DuplicateContext> = ctx_list
                .iter()
                .map(|(ctx, durations)| DuplicateContext {
                    context: (*ctx).to_string(),
                    count: durations.len(),
                    total: Duration::from_micros(durations.iter().sum()),
                })
                .collect();
            context_stats.sort_by(|a, b| {
                b.total
                    .cmp(&a.total)
                    .then_with(|| a.context.cmp(&b.context))
            });

            same_context_duplicates.push(DuplicateCommand {
                command: (*cmd).to_string(),
                max_per_context,
                extra_calls,
                extra: Duration::from_micros(extra_us),
                contexts: context_stats,
            });
        }
        same_context_duplicates.sort_by(|a, b| {
            b.extra
                .cmp(&a.extra)
                .then_with(|| a.command.cmp(&b.command))
        });

        let duplicated_commands = cmd_counts.values().filter(|c| **c > 1).count();
        let extra_calls = cmd_counts.values().filter(|c| **c > 1).map(|c| c - 1).sum();

        CacheReport {
            total_commands,
            unique_commands: cmd_counts.len(),
            contexts: contexts.len(),
            total_time: Duration::from_micros(total_time_us),
            duplicated_commands,
            extra_calls,
            same_context_duplicates,
            same_context_extra_calls,
            same_context_extra: Duration::from_micros(same_context_extra_us),
        }
    }
}

/// Parallelism factor and peak concurrency from timed subprocess intervals.
///
/// Factor is Σ(durations) ÷ wall span: 1.0 is serial, 4.0 means four
/// subprocesses ran concurrently on average. Peak is the most intervals
/// overlapping at any instant (a half-open sweep, so back-to-back commands
/// don't count as concurrent).
fn concurrency(intervals: &[(u64, u64)]) -> (Option<f64>, Option<usize>) {
    if intervals.is_empty() {
        return (None, None);
    }
    let span = intervals.iter().map(|(_, e)| *e).max().unwrap()
        - intervals.iter().map(|(s, _)| *s).min().unwrap();
    let total: u64 = intervals.iter().map(|(s, e)| e - s).sum();
    let parallelism = (span > 0).then(|| total as f64 / span as f64);

    let mut events: Vec<(u64, i32)> = Vec::with_capacity(intervals.len() * 2);
    for &(start, end) in intervals {
        events.push((start, 1));
        events.push((end, -1));
    }
    // Closings before openings at the same instant so adjacency isn't overlap.
    events.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    let mut current = 0i32;
    let mut peak = 0i32;
    for (_, delta) in events {
        current += delta;
        peak = peak.max(current);
    }
    (parallelism, Some(peak as usize))
}

#[derive(Clone, Copy)]
enum Align {
    Left,
    Right,
}

/// Pad rows into aligned columns, two spaces between columns, two-space indent.
///
/// Widths use `char` count — trace command shapes and durations are ASCII;
/// only a `[context]` (a branch name) can carry wide characters, and it sits in
/// the final left-aligned column. Trailing whitespace is trimmed per line.
///
/// Durations all render in one unit ([`fmt_dur`]) with two decimals, so
/// right-aligning a duration column lines up both the decimal points and the
/// `ms` suffix — no decimal-specific alignment is needed.
fn render_table(rows: &[Vec<String>], aligns: &[Align]) -> String {
    let cols = aligns.len();
    let mut widths = vec![0usize; cols];
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }

    let mut out = String::new();
    for row in rows {
        let mut line = String::from("  ");
        for (i, cell) in row.iter().enumerate() {
            let last = i + 1 == row.len();
            let pad = widths[i].saturating_sub(cell.chars().count());
            match aligns[i] {
                Align::Left => {
                    line.push_str(cell);
                    if !last {
                        line.push_str(&" ".repeat(pad));
                    }
                }
                Align::Right => {
                    line.push_str(&" ".repeat(pad));
                    line.push_str(cell);
                }
            }
            if !last {
                line.push_str("  ");
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// Format a duration in milliseconds with two decimals — one unit everywhere so
/// every duration column aligns (decimals and the `ms` suffix line up under
/// right-alignment) regardless of magnitude.
fn fmt_dur(d: Duration) -> String {
    format!("{:.2}ms", d.as_micros() as f64 / 1_000.0)
}

fn plural(n: usize, suffix: &str) -> &str {
    if n == 1 { "" } else { suffix }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Strip ANSI so the heading colors don't clutter the snapshot.
    fn plain(s: String) -> String {
        anstream::adapter::strip_str(&s).to_string()
    }

    fn cmd(
        command: &str,
        ctx: Option<&str>,
        ts_us: u64,
        dur_us: u64,
        tid: u64,
        ok: bool,
    ) -> TraceEntry {
        TraceEntry {
            context: ctx.map(str::to_string),
            kind: TraceEntryKind::Command {
                command: command.to_string(),
                duration: Duration::from_micros(dur_us),
                result: TraceResult::Completed { success: ok },
                reads_stdin: false,
            },
            start_time_us: Some(ts_us),
            thread_id: Some(tid),
        }
    }

    /// A successful command that consumed stdin uncaptured by its command
    /// string (the `stdin=true` shape: `claude -p`, `git patch-id`).
    fn cmd_stdin(command: &str, ctx: Option<&str>, dur_us: u64) -> TraceEntry {
        TraceEntry {
            context: ctx.map(str::to_string),
            kind: TraceEntryKind::Command {
                command: command.to_string(),
                duration: Duration::from_micros(dur_us),
                result: TraceResult::Completed { success: true },
                reads_stdin: true,
            },
            start_time_us: None,
            thread_id: None,
        }
    }

    fn span(name: &str, ts_us: u64, dur_us: u64) -> TraceEntry {
        TraceEntry {
            context: None,
            kind: TraceEntryKind::Span {
                name: name.to_string(),
                duration: Duration::from_micros(dur_us),
            },
            start_time_us: Some(ts_us),
            thread_id: Some(1),
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
    fn command_type_keeps_program_and_subcommands() {
        // Stops at flags, refs, ranges, paths, and SHAs.
        assert_eq!(command_type("git status --porcelain"), "git status");
        assert_eq!(
            command_type("git rev-list --count HEAD...origin/main"),
            "git rev-list"
        );
        assert_eq!(
            command_type("git for-each-ref --format=%(refname) refs/heads/"),
            "git for-each-ref"
        );
        assert_eq!(command_type("gh pr list --json number"), "gh pr list");
        assert_eq!(
            command_type("git merge-base abc123 def456"),
            "git merge-base"
        );
        assert_eq!(command_type("git diff main...HEAD --shortstat"), "git diff");
        assert_eq!(command_type("claude -p 'summarize'"), "claude");
        // Leading global flags are skipped to reach the subcommand.
        assert_eq!(
            command_type("git --no-optional-locks status --porcelain"),
            "git status"
        );
        assert_eq!(command_type("git -c foo.bar=baz diff --stat"), "git diff");
        // Empty command string yields an empty key.
        assert_eq!(command_type(""), "");
    }

    #[test]
    fn fmt_dur_is_milliseconds() {
        // One unit everywhere, two decimals — sub-millisecond and multi-second alike.
        assert_eq!(fmt_dur(Duration::from_micros(53)), "0.05ms");
        assert_eq!(fmt_dur(Duration::from_micros(1_500)), "1.50ms");
        assert_eq!(fmt_dur(Duration::from_millis(826)), "826.00ms");
        assert_eq!(fmt_dur(Duration::from_secs(3)), "3000.00ms");
        assert_eq!(fmt_dur(Duration::from_micros(999_999)), "1000.00ms");
        assert_eq!(fmt_dur(Duration::ZERO), "0.00ms");
    }

    /// A collect run with only milestones (no subprocesses) and no skeleton:
    /// KEY INTERVALS shows the always-fire intervals, the note explains the
    /// absent skeleton/first-result milestones, and the empty subprocess sections
    /// are omitted.
    #[test]
    fn renders_collect_run_without_skeleton() {
        let entries = vec![
            instant("List collect started", 1_000),
            instant("Parallel execution started", 2_000),
            instant("All results drained", 8_000),
            instant("List collect complete", 9_000),
        ];
        let profile = Profile::from_entries(&entries);
        assert_eq!(profile.key_intervals.time_to_skeleton, None);
        insta::assert_snapshot!(plain(profile.render_text("trace.log")));
    }

    #[test]
    fn parallelism_and_peak_from_overlap() {
        // Three 10ms commands: two overlap on threads 1 & 2 (0–10ms), the third
        // runs alone after (10–20ms). Σ=30ms over a 20ms span → 1.5×; peak 2.
        let entries = vec![
            cmd("git status", Some("a"), 0, 10_000, 1, true),
            cmd("git status", Some("b"), 0, 10_000, 2, true),
            cmd("git diff", Some("a"), 10_000, 10_000, 1, true),
        ];
        let profile = Profile::from_entries(&entries);
        assert_eq!(profile.parallelism, Some(1.5));
        assert_eq!(profile.peak_concurrency, Some(2));
        assert_eq!(profile.thread_count, 2);
        assert_eq!(profile.command_total, Duration::from_millis(30));
    }

    #[test]
    fn cache_report_counts_same_context_repeats() {
        // `git config` runs 3× in context "a" (2 extra) and once in "b".
        let entries = vec![
            cmd("git config", Some("a"), 0, 5_000, 1, true),
            cmd("git config", Some("a"), 6_000, 4_000, 1, true),
            cmd("git config", Some("a"), 11_000, 3_000, 1, true),
            cmd("git config", Some("b"), 0, 2_000, 2, true),
        ];
        let cache = CacheReport::from_entries(&entries);
        assert_eq!(cache.same_context_extra_calls, 2);
        // Context "a": keep slowest (5ms), waste 4ms+3ms = 7ms.
        assert_eq!(cache.same_context_extra, Duration::from_millis(7));
        assert_eq!(cache.same_context_duplicates.len(), 1);
        assert_eq!(cache.same_context_duplicates[0].max_per_context, 3);
    }

    #[test]
    fn cache_report_excludes_stdin_reading_commands() {
        // Three identical `claude -p` lines in one context differ only by the
        // prompt piped to stdin (not in the command string), so they're real
        // distinct work, not a cache miss. A plain `git status` repeated in the
        // same context is the genuine duplicate the report should still catch.
        let claude = "sh -c claude -p";
        let entries = vec![
            cmd_stdin(claude, Some("main"), 5_000),
            cmd_stdin(claude, Some("main"), 4_000),
            cmd_stdin(claude, Some("main"), 3_000),
            cmd("git status", Some("main"), 0, 2_000, 1, true),
            cmd("git status", Some("main"), 6_000, 1_000, 1, true),
        ];
        let cache = CacheReport::from_entries(&entries);

        // The stdin-reading commands are not duplicates …
        assert_eq!(cache.same_context_duplicates.len(), 1);
        assert_eq!(cache.same_context_duplicates[0].command, "git status");
        assert_eq!(cache.same_context_extra_calls, 1);
        assert_eq!(cache.same_context_extra, Duration::from_millis(1));
        // … nor counted in the cross-context duplicate tallies.
        assert_eq!(cache.duplicated_commands, 1);
        assert_eq!(cache.extra_calls, 1);
        assert_eq!(cache.unique_commands, 1); // only `git status` is dedupable

        // … but they still count toward the totals (real commands, real time).
        assert_eq!(cache.total_commands, 5);
        assert_eq!(cache.total_time, Duration::from_millis(15));
        assert_eq!(cache.contexts, 1);
    }

    #[test]
    fn renders_full_report() {
        let entries = vec![
            instant("List collect started", 0),
            cmd(
                "git status --porcelain",
                Some("main"),
                1_000,
                12_000,
                1,
                true,
            ),
            cmd(
                "git status --porcelain",
                Some("feature"),
                1_000,
                8_000,
                2,
                true,
            ),
            span("user_config_load", 500, 2_000),
            instant("Skeleton rendered", 13_000),
            cmd(
                "git diff --shortstat",
                Some("main"),
                13_000,
                5_000,
                1,
                false,
            ),
            cmd("git config --list", Some("main"), 1_000, 4_000, 1, true),
            cmd("git config --list", Some("main"), 6_000, 3_000, 1, true),
            instant("All results drained", 18_000),
        ];
        let profile = Profile::from_entries(&entries);
        insta::assert_snapshot!(plain(profile.render_text("trace.log")));
    }

    /// The JSON form (direct serialization of the struct — the single canonical
    /// source) carries the same analysis with microsecond durations, including
    /// nullable key-interval keys.
    #[test]
    fn to_json_shape() {
        let entries = vec![
            instant("List collect started", 0),
            instant("Skeleton rendered", 3_000),
            cmd("git status", Some("main"), 0, 5_000, 1, true),
            cmd("git status", Some("main"), 6_000, 4_000, 1, true),
            instant("List collect complete", 12_000),
        ];
        let profile = Profile::from_entries(&entries);
        insta::assert_snapshot!(serde_json::to_string_pretty(&profile).unwrap(), @r#"
        {
          "traced_us": 12000,
          "command_count": 2,
          "command_total_us": 9000,
          "span_total_us": 0,
          "parallelism": 0.9,
          "peak_concurrency": 1,
          "thread_count": 1,
          "key_intervals": {
            "time_to_skeleton_us": 3000,
            "time_to_first_result_us": null,
            "parallel_phase_us": null,
            "collect_total_us": 12000
          },
          "by_type": [
            {
              "command": "git status",
              "count": 2,
              "total_us": 9000,
              "max_us": 5000,
              "avg_us": 4500
            }
          ],
          "slowest": [
            {
              "dur_us": 5000,
              "label": "git status [main]"
            },
            {
              "dur_us": 4000,
              "label": "git status [main]"
            }
          ],
          "cache": {
            "total_commands": 2,
            "unique_commands": 1,
            "contexts": 1,
            "total_time_us": 9000,
            "duplicated_commands": 1,
            "extra_calls": 1,
            "same_context_duplicates": [
              {
                "command": "git status",
                "max_per_context": 2,
                "extra_calls": 1,
                "extra_us": 4000,
                "contexts": [
                  {
                    "context": "main",
                    "count": 2,
                    "total_us": 9000
                  }
                ]
              }
            ],
            "same_context_extra_calls": 1,
            "same_context_extra_us": 4000
          },
          "phases": [
            {
              "name": "List collect started",
              "at_us": 0,
              "delta_us": null
            },
            {
              "name": "Skeleton rendered",
              "at_us": 3000,
              "delta_us": 3000
            },
            {
              "name": "List collect complete",
              "at_us": 12000,
              "delta_us": 9000
            }
          ]
        }
        "#);
    }

    /// A failed command, no timestamps, no repeats, no milestones: exercises the
    /// `(err: …)` label, the `n/a` parallelism/peak fallbacks, the
    /// no-duplicates cache line, and the omitted PHASES/in-process sections.
    #[test]
    fn renders_minimal_report_without_timestamps() {
        let err = |command: &str, dur_us: u64, message: &str| TraceEntry {
            context: None,
            kind: TraceEntryKind::Command {
                command: command.to_string(),
                duration: Duration::from_micros(dur_us),
                result: TraceResult::Error {
                    message: message.to_string(),
                },
                reads_stdin: false,
            },
            start_time_us: None,
            thread_id: None,
        };
        let ok = |command: &str, dur_us: u64| TraceEntry {
            context: None,
            kind: TraceEntryKind::Command {
                command: command.to_string(),
                duration: Duration::from_micros(dur_us),
                result: TraceResult::Completed { success: true },
                reads_stdin: false,
            },
            start_time_us: None,
            thread_id: None,
        };
        let entries = vec![
            ok("git status", 5_000),
            err("git rev-list", 3_000, "bad revision"),
        ];
        let profile = Profile::from_entries(&entries);
        assert_eq!(profile.parallelism, None);
        assert_eq!(profile.peak_concurrency, None);
        insta::assert_snapshot!(plain(profile.render_text("trace.log")));
    }

    /// Many slow calls and several duplicate commands (one across two contexts):
    /// exercises slowest-list truncation, the cache `… and N more` collapse, and
    /// the per-context sort.
    #[test]
    fn truncates_slowest_and_collapses_cache() {
        let dup = |command: &str, ctx: &str, dur_us: u64| TraceEntry {
            context: Some(ctx.to_string()),
            kind: TraceEntryKind::Command {
                command: command.to_string(),
                duration: Duration::from_micros(dur_us),
                result: TraceResult::Completed { success: true },
                reads_stdin: false,
            },
            start_time_us: None,
            thread_id: None,
        };
        // Four duplicated commands (waste aaa>bbb>ccc>ddd); ddd spans two
        // contexts. Ten calls total, so the eight-deep slowest list truncates.
        let entries = vec![
            dup("git aaa", "w1", 9_000),
            dup("git aaa", "w1", 8_000),
            dup("git bbb", "w1", 7_000),
            dup("git bbb", "w1", 6_000),
            dup("git ccc", "w1", 5_000),
            dup("git ccc", "w1", 4_000),
            dup("git ddd", "w1", 3_000),
            dup("git ddd", "w1", 2_000),
            dup("git ddd", "w2", 1_500),
            dup("git ddd", "w2", 1_000),
        ];
        let profile = Profile::from_entries(&entries);
        assert_eq!(profile.slowest.len(), SLOWEST_LIMIT);
        assert_eq!(profile.cache.same_context_duplicates.len(), 4);
        // ddd's contexts sort by total time: w1 (5ms) before w2 (2.5ms).
        let ddd = profile
            .cache
            .same_context_duplicates
            .iter()
            .find(|d| d.command == "git ddd")
            .unwrap();
        assert_eq!(
            ddd.contexts
                .iter()
                .map(|c| c.context.as_str())
                .collect::<Vec<_>>(),
            ["w1", "w2"]
        );
        insta::assert_snapshot!(plain(profile.render_text("trace.log")));
    }
}
