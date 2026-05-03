//! CLI for worktrunk performance testing and tracing.
//!
//! Run `wt-perf --help` (and `wt-perf <subcommand> --help`) for usage.

use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use worktrunk::trace::{TraceEntry, TraceEntryKind, TraceResult};
use wt_perf::{canonicalize, create_repo_at, invalidate_caches_auto, parse_config};

#[derive(Parser)]
#[command(name = "wt-perf")]
#[command(about = "Performance testing and tracing tools for worktrunk")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Set up a benchmark repository
    Setup {
        /// Config name: typical-N, branches-N, branches-N-M, divergent, picker-test
        config: String,

        /// Directory to create repo in (default: temp directory)
        #[arg(long)]
        path: Option<PathBuf>,

        /// Keep the repo (don't wait for cleanup)
        #[arg(long)]
        persist: bool,
    },

    /// Invalidate git caches for cold benchmarks
    Invalidate {
        /// Path to the repository
        repo: PathBuf,
    },

    /// Parse trace logs and output Chrome Trace Format JSON
    #[command(after_long_help = r#"EXAMPLES:
  # Generate trace from wt command
  # --progressive is required — without it, TTY-gated events (Skeleton
  # rendered, First result received) don't fire when stdout is a pipe.
  RUST_LOG=debug wt list --progressive 2>&1 | wt-perf trace > trace.json

  # Then either:
  #   - Open trace.json in chrome://tracing or https://ui.perfetto.dev
  #   - Query with: trace_processor trace.json -Q 'SELECT * FROM slice LIMIT 10'

  # Find milestone events (instant events have dur=0)
  trace_processor trace.json -Q 'SELECT name, ts/1e6 as ms FROM slice WHERE dur = 0'

  # Install trace_processor for SQL analysis:
  curl -LO https://get.perfetto.dev/trace_processor && chmod +x trace_processor
"#)]
    Trace {
        /// Path to trace log file (reads from stdin if omitted)
        file: Option<PathBuf>,
    },

    /// Analyze trace logs for duplicate commands (cache effectiveness)
    #[command(after_long_help = r#"EXAMPLES:
  # Check cache effectiveness for wt list
  RUST_LOG=debug wt list --progressive 2>&1 | wt-perf cache-check

  # From a file
  wt-perf cache-check trace.log
"#)]
    CacheCheck {
        /// Path to trace log file (reads from stdin if omitted)
        file: Option<PathBuf>,
    },

    /// Run a `wt` command with tracing on and render a timeline.
    ///
    /// Sets `WORKTRUNK_TRACE=1` so wt emits only `[wt-trace]` records on
    /// stderr (no other log noise), then sorts the records by start time
    /// and prints a column-aligned timeline to stdout. With `--chrome`,
    /// emits Chrome Trace Format JSON instead — pipe to a file and open in
    /// chrome://tracing or https://ui.perfetto.dev.
    #[command(after_long_help = r#"EXAMPLES:
  # Text timeline of `wt list` in the current repo
  wt-perf timeline -- list

  # Cold-cache run (invalidates ./ then runs)
  wt-perf timeline --cold -- list

  # Cold run against a specific repo
  wt-perf timeline --cold --repo /tmp/wt-perf-typical-1 -- -C /tmp/wt-perf-typical-1 list

  # Chrome Trace Format JSON for Perfetto
  wt-perf timeline --chrome -- list > trace.json
"#)]
    Timeline {
        /// Invalidate caches before running (cold measurement).
        #[arg(long)]
        cold: bool,

        /// Repo to invalidate (only used with --cold). Defaults to cwd.
        #[arg(long, value_name = "PATH")]
        repo: Option<PathBuf>,

        /// Output Chrome Trace Format JSON to stdout instead of a text timeline.
        #[arg(long)]
        chrome: bool,

        /// Args passed to `wt`. Use `--` to separate them from timeline flags.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        wt_args: Vec<String>,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Setup {
            config,
            path,
            persist,
        } => {
            let repo_config = parse_config(&config).unwrap_or_else(|| {
                eprintln!("Unknown config: {}", config);
                eprintln!();
                eprintln!("Available configs:");
                eprintln!(
                    "  typical-N       - Typical repo with N worktrees (500 commits, 100 files)"
                );
                eprintln!("  branches-N      - N branches with 1 commit each");
                eprintln!("  branches-N-M    - N branches with M commits each");
                eprintln!("  divergent       - 200 branches × 20 commits (GH #461 scenario)");
                eprintln!("  picker-test     - Config for wt switch interactive picker testing");
                std::process::exit(1);
            });

            let base_path = if let Some(p) = path {
                std::fs::create_dir_all(&p).unwrap();
                canonicalize(&p).unwrap()
            } else {
                let temp = std::env::temp_dir().join(format!("wt-perf-{}", config));
                if temp.exists() {
                    std::fs::remove_dir_all(&temp).unwrap();
                }
                std::fs::create_dir_all(&temp).unwrap();
                canonicalize(&temp).unwrap()
            };

            eprintln!("Creating {} repo...", config);
            create_repo_at(&repo_config, &base_path);

            let mut parts = vec![format!("main @ {}", base_path.display())];
            if repo_config.worktrees > 1 {
                parts.push(format!("{} worktrees", repo_config.worktrees));
            }
            if repo_config.branches > 0 {
                parts.push(format!("{} branches", repo_config.branches));
            }
            eprintln!("Created: {}", parts.join(", "));
            eprintln!();
            eprintln!(
                "  RUST_LOG=debug wt -C {} list --progressive 2>&1 | wt-perf trace > trace.json",
                base_path.display()
            );
            eprintln!("  wt-perf invalidate {}", base_path.display());

            if !persist {
                eprintln!();
                eprintln!("Press Enter to clean up (or Ctrl+C to keep)...");
                std::io::stdout().flush().unwrap();
                let mut input = String::new();
                std::io::stdin().read_line(&mut input).unwrap();

                eprintln!("Cleaning up...");
                if let Err(e) = std::fs::remove_dir_all(&base_path) {
                    eprintln!("Warning: Failed to clean up: {}", e);
                    eprintln!("You may need to manually remove: {}", base_path.display());
                }
            }
        }

        Commands::Invalidate { repo } => {
            let repo = canonicalize(&repo).unwrap_or_else(|e| {
                eprintln!("Invalid repo path {}: {}", repo.display(), e);
                std::process::exit(1);
            });

            if !repo.join(".git").exists() {
                eprintln!("Not a git repository: {}", repo.display());
                std::process::exit(1);
            }

            invalidate_caches_auto(&repo);
            eprintln!("Invalidated caches for {}", repo.display());
        }

        Commands::Trace { file } => {
            let entries = read_trace_entries(file.as_deref());
            println!("{}", worktrunk::trace::to_chrome_trace(&entries));
        }

        Commands::CacheCheck { file } => {
            let entries = read_trace_entries(file.as_deref());
            cache_check(&entries);
        }

        Commands::Timeline {
            cold,
            repo,
            chrome,
            wt_args,
        } => run_timeline(cold, repo, chrome, &wt_args),
    }
}

/// Resolve the `wt` binary as a sibling of the current executable
/// (`target/{debug,release}/wt-perf` → `target/{debug,release}/wt`).
fn resolve_wt_binary() -> PathBuf {
    let me = std::env::current_exe().unwrap_or_else(|e| {
        eprintln!("Failed to resolve current executable: {e}");
        std::process::exit(1);
    });
    let candidate = me.parent().map(|p| p.join("wt")).unwrap_or_default();
    if !candidate.is_file() {
        eprintln!(
            "wt binary not found at {} — run `cargo build --release --bin wt` (or `cargo build --bin wt`) first.",
            candidate.display()
        );
        std::process::exit(1);
    }
    candidate
}

/// Run a `wt` command with `WORKTRUNK_TRACE=1`, capture stderr, and render.
fn run_timeline(cold: bool, repo: Option<PathBuf>, chrome: bool, wt_args: &[String]) {
    let wt = resolve_wt_binary();

    if cold {
        let path = repo
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap());
        let path = canonicalize(&path).unwrap_or_else(|e| {
            eprintln!("Invalid repo path {}: {}", path.display(), e);
            std::process::exit(1);
        });
        if !path.join(".git").exists() {
            eprintln!("--cold target is not a git repository: {}", path.display());
            std::process::exit(1);
        }
        invalidate_caches_auto(&path);
    }

    // Measure spawn → wait wall externally. The trace can't see the
    // process prelude (argv parsing, dyld, the time before `init_logging`
    // registers the logger and the trace_epoch is set) or the epilogue
    // (drop, exit), so the externally-measured duration is the only honest
    // answer to "how long did the whole thing take".
    let started = Instant::now();
    let output = Command::new(&wt)
        .args(wt_args)
        .env("RUST_LOG", "debug")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .unwrap_or_else(|e| {
            eprintln!("Failed to spawn {}: {e}", wt.display());
            std::process::exit(1);
        });
    let wall = started.elapsed();

    let stderr = String::from_utf8_lossy(&output.stderr);
    let entries = worktrunk::trace::parse_lines(&stderr);

    if entries.is_empty() {
        eprintln!(
            "No [wt-trace] entries captured. wt exited with {}; check that the command runs past `init_logging` (e.g. avoid `--version`/`--help`).",
            output.status,
        );
        if !output.stderr.is_empty() {
            eprintln!("--- wt stderr ---\n{stderr}");
        }
        std::process::exit(1);
    }

    if chrome {
        println!("{}", worktrunk::trace::to_chrome_trace(&entries));
    } else {
        print!("{}", render_timeline(&entries, wall));
    }

    if !output.status.success() {
        eprintln!("note: wt exited with {}", output.status);
        std::process::exit(1);
    }
}

/// Render parsed entries as a column-aligned, start-time-sorted timeline.
///
/// `wall` is the externally-measured spawn → wait duration. The trace
/// can't see the prelude (argv parsing, dyld, time before `init_logging`
/// sets the trace epoch) or the exit path, so reporting `wall` lets
/// readers see how much of the process the trace actually accounts for —
/// the gap between `traced` and `wall` is the unobserved overhead.
fn render_timeline(entries: &[TraceEntry], wall: Duration) -> String {
    let mut rows: Vec<Row> = entries.iter().map(Row::from_entry).collect();
    rows.sort_by_key(|r| r.start_us);

    let mut out = String::new();
    out.push_str("   ts(ms)      dur   tid  kind   name\n");
    for row in &rows {
        let ts_ms = row.start_us as f64 / 1_000.0;
        out.push_str(&format!(
            "{:>9.3}  {:>7}  {:>4}  {:<5}  {}\n",
            ts_ms,
            format_duration(row.dur),
            row.tid.map(|t| t.to_string()).unwrap_or_else(|| "-".into()),
            row.kind,
            row.name,
        ));
    }

    // Summary: subprocess totals + traced span + true process wall.
    let cmd_rows: Vec<&Row> = rows.iter().filter(|r| r.kind == "cmd").collect();
    let cmd_total: u64 = cmd_rows.iter().map(|r| r.dur.as_micros() as u64).sum();
    let slowest_cmd = cmd_rows
        .iter()
        .max_by_key(|r| r.dur)
        .map(|r| (format_duration(r.dur), r.name.as_str()));
    let traced_us = rows
        .iter()
        .map(|r| r.start_us + r.dur.as_micros() as u64)
        .max()
        .unwrap_or(0)
        .saturating_sub(rows.iter().map(|r| r.start_us).min().unwrap_or(0));
    let traced = Duration::from_micros(traced_us);
    let untraced = wall.saturating_sub(traced);

    out.push('\n');
    if cmd_rows.is_empty() {
        out.push_str("0 subprocesses\n");
    } else if let Some((dur, cmd)) = slowest_cmd {
        out.push_str(&format!(
            "{} subprocess{} totaling {} (slowest: {} {})\n",
            cmd_rows.len(),
            if cmd_rows.len() == 1 { "" } else { "es" },
            format_duration(Duration::from_micros(cmd_total)),
            dur,
            cmd,
        ));
    }
    out.push_str(&format!(
        "traced: {} (first → last [wt-trace] record)\n",
        format_duration(traced)
    ));
    out.push_str(&format!(
        "wall:   {} (spawn → wait; +{} untraced prelude/epilogue)\n",
        format_duration(wall),
        format_duration(untraced),
    ));
    out
}

/// Internal flat-row representation for the renderer.
struct Row {
    start_us: u64,
    dur: Duration,
    tid: Option<u64>,
    kind: &'static str,
    name: String,
}

impl Row {
    fn from_entry(e: &TraceEntry) -> Self {
        let (kind, name, dur) = match &e.kind {
            TraceEntryKind::Command {
                command,
                duration,
                result,
            } => {
                let label = e
                    .context
                    .as_deref()
                    .map(|c| format!("{command} [{c}]"))
                    .unwrap_or_else(|| command.clone());
                let label = match result {
                    TraceResult::Completed { success: false } => format!("{label}  (ok=false)"),
                    TraceResult::Error { message } => format!("{label}  (err: {message})"),
                    TraceResult::Completed { success: true } => label,
                };
                ("cmd", label, *duration)
            }
            TraceEntryKind::Span { name, duration } => ("span", name.clone(), *duration),
            TraceEntryKind::Instant { name } => ("event", name.clone(), Duration::ZERO),
        };
        Self {
            start_us: e.start_time_us.unwrap_or(0),
            dur,
            tid: e.thread_id,
            kind,
            name,
        }
    }
}

/// Human-friendly duration: `<1ms` → `Nus`, `<1s` → `N.Nms`, else `N.Ns`.
fn format_duration(d: Duration) -> String {
    let us = d.as_micros();
    if us < 1_000 {
        format!("{us}us")
    } else if us < 1_000_000 {
        format!("{:.1}ms", us as f64 / 1_000.0)
    } else {
        format!("{:.2}s", us as f64 / 1_000_000.0)
    }
}

/// Read trace input from file or stdin, parse entries, and exit if empty.
fn read_trace_entries(file: Option<&std::path::Path>) -> Vec<worktrunk::trace::TraceEntry> {
    let input = match file {
        Some(path) if path.as_os_str() != "-" => match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(e) => {
                eprintln!("Error reading {}: {}", path.display(), e);
                std::process::exit(1);
            }
        },
        _ => {
            if std::io::stdin().is_terminal() {
                eprintln!(
                    "Reading from stdin... (pipe trace data or use Ctrl+D to end)\n\
                     See `wt-perf <subcommand> --help` for the capture pipeline."
                );
            }

            let mut content = String::new();
            std::io::stdin()
                .lock()
                .read_to_string(&mut content)
                .expect("Failed to read stdin");
            content
        }
    };

    let entries = worktrunk::trace::parse_lines(&input);

    if entries.is_empty() {
        eprintln!(
            "No [wt-trace] entries found in input.\n\
             Run the target command with RUST_LOG=debug to emit trace records.\n\
             See `wt-perf <subcommand> --help` for the capture pipeline."
        );
        std::process::exit(1);
    }

    entries
}

/// Analyze trace entries for cache effectiveness.
///
/// Outputs structured JSON to stdout, composable with jq.
///
/// For each (command, context) pair called N times, the first call is "necessary"
/// and the remaining N-1 are "extra". Wasted time is computed by keeping the
/// slowest call (likely a cache-miss/cold call) and summing the rest.
fn cache_check(entries: &[worktrunk::trace::TraceEntry]) {
    use std::collections::{BTreeMap, HashMap, HashSet};
    use worktrunk::trace::TraceEntryKind;

    let mut total_commands = 0;
    let mut cmd_counts: HashMap<&str, usize> = HashMap::new();
    let mut contexts: HashSet<&str> = HashSet::new();

    // Collect all durations per (command, context) pair
    let mut pair_durations: HashMap<(&str, &str), Vec<u64>> = HashMap::new();

    for entry in entries {
        if let TraceEntryKind::Command {
            command, duration, ..
        } = &entry.kind
        {
            let ctx = entry.context.as_deref().unwrap_or("(none)");
            *cmd_counts.entry(command.as_str()).or_default() += 1;
            pair_durations
                .entry((command.as_str(), ctx))
                .or_default()
                .push(duration.as_micros() as u64);
            contexts.insert(ctx);
            total_commands += 1;
        }
    }

    // Build structured duplicates list: group by command
    let mut cmd_ctx_info: BTreeMap<&str, Vec<(&str, &Vec<u64>)>> = BTreeMap::new();
    for ((cmd, ctx), durations) in &pair_durations {
        if durations.len() > 1 {
            cmd_ctx_info.entry(cmd).or_default().push((ctx, durations));
        }
    }

    let mut duplicates = Vec::new();
    let mut total_extra = 0usize;
    let mut total_extra_us = 0u64;
    for (cmd, ctx_list) in &cmd_ctx_info {
        let max_count = ctx_list.iter().map(|(_, d)| d.len()).max().unwrap();
        let extra: usize = ctx_list.iter().map(|(_, d)| d.len() - 1).sum();
        total_extra += extra;

        // Wasted time: for each context, keep the slowest call, sum the rest
        let extra_us: u64 = ctx_list
            .iter()
            .map(|(_, durations)| {
                let max = durations.iter().max().unwrap();
                durations.iter().sum::<u64>() - max
            })
            .sum();
        total_extra_us += extra_us;

        let contexts: Vec<_> = ctx_list
            .iter()
            .map(|(ctx, durations)| {
                let total_us: u64 = durations.iter().sum();
                serde_json::json!({
                    "context": ctx,
                    "count": durations.len(),
                    "total_us": total_us,
                })
            })
            .collect();
        duplicates.push(serde_json::json!({
            "command": cmd,
            "max_per_context": max_count,
            "extra_calls": extra,
            "extra_us": extra_us,
            "contexts": contexts,
        }));
    }
    duplicates.sort_by(|a, b| b["extra_us"].as_u64().cmp(&a["extra_us"].as_u64()));

    let total_time_us: u64 = pair_durations.values().flat_map(|d| d.iter()).sum();
    let dup_count = cmd_counts.values().filter(|c| **c > 1).count();
    let dup_total: usize = cmd_counts.values().filter(|c| **c > 1).map(|c| c - 1).sum();

    let output = serde_json::json!({
        "total_commands": total_commands,
        "unique_commands": cmd_counts.len(),
        "contexts": contexts.len(),
        "total_time_us": total_time_us,
        "duplicated_commands": dup_count,
        "extra_calls": dup_total,
        "same_context_duplicates": duplicates,
        "same_context_extra_calls": total_extra,
        "same_context_extra_us": total_extra_us,
    });
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
}

#[cfg(test)]
mod tests {
    use super::*;

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
            },
            start_time_us: Some(ts_us),
            thread_id: Some(tid),
        }
    }

    #[test]
    fn format_duration_buckets() {
        assert_eq!(format_duration(Duration::from_micros(0)), "0us");
        assert_eq!(format_duration(Duration::from_micros(999)), "999us");
        assert_eq!(format_duration(Duration::from_micros(1_000)), "1.0ms");
        assert_eq!(format_duration(Duration::from_micros(4_500)), "4.5ms");
        assert_eq!(format_duration(Duration::from_micros(999_999)), "1000.0ms");
        assert_eq!(format_duration(Duration::from_micros(1_500_000)), "1.50s");
    }

    #[test]
    fn renders_sorted_timeline_with_summary() {
        // Emit order swaps span and child cmd (parent finishes after child),
        // so this exercises the sort-by-start-time guarantee.
        let entries = vec![
            cmd("git rev-parse HEAD", Some("repo"), 50, 4_000, 1, true),
            span("prewarm", 30, 4_100, 1),
            span("init_logging", 0, 8, 1),
            span("user_config_load", 4_200, 280, 38),
        ];
        // Wall = 6ms; traced span = 4.48ms (4.2ms start → 4.48ms end);
        // untraced prelude/epilogue = 6 - 4.48 = ~1.52ms.
        let out = render_timeline(&entries, Duration::from_micros(6_000));
        let expected = "   ts(ms)      dur   tid  kind   name
    0.000      8us     1  span   init_logging
    0.030    4.1ms     1  span   prewarm
    0.050    4.0ms     1  cmd    git rev-parse HEAD [repo]
    4.200    280us    38  span   user_config_load

1 subprocess totaling 4.0ms (slowest: 4.0ms git rev-parse HEAD [repo])
traced: 4.5ms (first → last [wt-trace] record)
wall:   6.0ms (spawn → wait; +1.5ms untraced prelude/epilogue)
";
        assert_eq!(out, expected);
    }

    #[test]
    fn cmd_failure_is_visible_in_name_column() {
        let entries = vec![cmd("git foo", None, 0, 1_000, 1, false)];
        let out = render_timeline(&entries, Duration::from_millis(2));
        assert!(out.contains("git foo  (ok=false)"), "{out}");
    }
}
