//! CLI for worktrunk performance testing and tracing.
//!
//! Run `wt-perf --help` (and `wt-perf <subcommand> --help`) for usage.

use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use worktrunk::trace::{TraceEntry, TraceEntryKind, TraceResult};
use wt_perf::{
    canonicalize, create_mixed_repo_at, create_repo_at, invalidate_caches_auto, parse_config,
};

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
        /// Config name: typical-N, branches-N, branches-N-M, divergent, mixed-W-B, picker-test
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

    /// Parse a trace.jsonl and output Chrome Trace Format JSON
    #[command(after_long_help = r#"EXAMPLES:
  # Capture a trace, then convert it. --progressive is required — without it,
  # TTY-gated events (Skeleton rendered, First result received) don't fire
  # when stdout is a pipe.
  wt -vv list --progressive
  wt-perf trace .git/wt/logs/trace.jsonl > trace.json

  # Then either:
  #   - Open trace.json in chrome://tracing or https://ui.perfetto.dev
  #   - Query with: trace_processor trace.json -Q 'SELECT * FROM slice LIMIT 10'

  # Find milestone events (instant events have dur=0)
  trace_processor trace.json -Q 'SELECT name, ts/1e6 as ms FROM slice WHERE dur = 0'

  # Install trace_processor for SQL analysis:
  curl -LO https://get.perfetto.dev/trace_processor && chmod +x trace_processor
"#)]
    Trace {
        /// Path to a trace.jsonl file (reads from stdin if omitted)
        file: Option<PathBuf>,
    },

    /// Analyze a trace.jsonl for duplicate commands (cache effectiveness)
    #[command(after_long_help = r#"EXAMPLES:
  # Check cache effectiveness for wt list
  wt -vv list --progressive
  wt-perf cache-check .git/wt/logs/trace.jsonl
"#)]
    CacheCheck {
        /// Path to a trace.jsonl file (reads from stdin if omitted)
        file: Option<PathBuf>,
    },

    /// Run a `wt` command with tracing on and render a timeline.
    ///
    /// Runs the child with `-vv` so it writes `trace.jsonl`, reads that back,
    /// sorts the records by start time, and prints a column-aligned timeline
    /// to stdout. With `--chrome`, emits Chrome Trace Format JSON instead —
    /// pipe to a file and open in chrome://tracing or https://ui.perfetto.dev.
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
            // `mixed-W-B`: W worktrees + B branches in varied states (warm
            // re-run benchmark fixture); handled separately since it doesn't
            // map onto the flat `RepoConfig`.
            let mixed = parse_mixed(&config);

            let repo_config = if mixed.is_some() {
                None
            } else {
                Some(parse_config(&config).unwrap_or_else(|| {
                    eprintln!("Unknown config: {}", config);
                    eprintln!();
                    eprintln!("Available configs:");
                    eprintln!(
                        "  typical-N       - Typical repo with N worktrees (500 commits, 100 files)"
                    );
                    eprintln!("  branches-N      - N branches with 1 commit each");
                    eprintln!("  branches-N-M    - N branches with M commits each");
                    eprintln!("  divergent       - 200 branches × 20 commits (GH #461 scenario)");
                    eprintln!("  mixed-W-B       - W worktrees + B branches in varied states");
                    eprintln!(
                        "  picker-test     - Config for wt switch interactive picker testing"
                    );
                    std::process::exit(1);
                }))
            };

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
            let (worktrees, branches) = match (mixed, &repo_config) {
                (Some((w, b)), _) => {
                    create_mixed_repo_at(w, b, &base_path);
                    (w, b)
                }
                (None, Some(cfg)) => {
                    create_repo_at(cfg, &base_path);
                    (cfg.worktrees, cfg.branches)
                }
                (None, None) => unreachable!("repo_config is Some when mixed is None"),
            };

            let mut parts = vec![format!("main @ {}", base_path.display())];
            if worktrees > 1 {
                parts.push(format!("{} worktrees", worktrees));
            }
            if branches > 0 {
                parts.push(format!("{} branches", branches));
            }
            eprintln!("Created: {}", parts.join(", "));
            eprintln!();
            eprintln!(
                "  wt-perf timeline -- -C {} list --progressive",
                base_path.display()
            );
            eprintln!(
                "  wt-perf timeline --chrome -- -C {} list --progressive > trace.json",
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

/// Parse a `mixed-W-B` config string into `(worktrees, branches)`.
fn parse_mixed(config: &str) -> Option<(usize, usize)> {
    let rest = config.strip_prefix("mixed-")?;
    let (w, b) = rest.split_once('-')?;
    Some((w.parse().ok()?, b.parse().ok()?))
}

/// Resolve the `wt` binary as a sibling of the current executable
/// (`target/{debug,release}/wt-perf` → `target/{debug,release}/wt`).
/// `EXE_SUFFIX` keeps this correct on Windows, where Cargo builds
/// `wt-perf.exe` next to `wt.exe`.
fn resolve_wt_binary() -> PathBuf {
    let me = std::env::current_exe().unwrap_or_else(|e| {
        eprintln!("Failed to resolve current executable: {e}");
        std::process::exit(1);
    });
    let exe = format!("wt{}", std::env::consts::EXE_SUFFIX);
    let candidate = me.parent().map(|p| p.join(&exe)).unwrap_or_default();
    if !candidate.is_file() {
        eprintln!(
            "wt binary not found at {} — run `cargo build --release --bin wt` (or `cargo build --bin wt`) first.",
            candidate.display()
        );
        std::process::exit(1);
    }
    candidate
}

/// Run a `wt -vv` command and render the `trace.jsonl` it writes.
///
/// `-vv` writes the machine trace to `<git-common-dir>/wt/logs/trace.jsonl` in
/// the repo wt operated on (the humanized stderr/`trace.log` isn't parseable).
/// We locate that repo the same way wt does — a `-C` in the args, else the
/// cwd — and read the file back after the run.
fn run_timeline(cold: bool, repo: Option<PathBuf>, chrome: bool, wt_args: &[String]) {
    let wt = resolve_wt_binary();
    // The trace lands in the repo wt operates on — resolved from `-C`/cwd the
    // same way wt resolves it, so we never read a different repo than wt wrote.
    // `--repo` governs only `--cold` invalidation.
    let trace_dir = wt_target_dir(wt_args);

    if cold {
        let path = canonicalize(repo.as_deref().unwrap_or(&trace_dir)).unwrap_or_else(|e| {
            eprintln!("Invalid --cold repo path: {e}");
            std::process::exit(1);
        });
        if !path.join(".git").exists() {
            eprintln!("--cold target is not a git repository: {}", path.display());
            std::process::exit(1);
        }
        invalidate_caches_auto(&path);
    }

    let jsonl = trace_jsonl_path(&trace_dir).unwrap_or_else(|| {
        eprintln!(
            "Could not locate a git repository for the trace at {} — run from inside a repo or pass a `-C <path>` in the wt args.",
            trace_dir.display()
        );
        std::process::exit(1);
    });
    // Drop any prior run's trace first, so an early-exiting child (e.g. clap
    // intercepting `--help`/`--version` before `init_logging`) surfaces the
    // absent-file error below rather than a stale timeline.
    let _ = std::fs::remove_file(&jsonl);

    // Measure spawn → wait wall externally. The trace can't see the
    // process prelude (argv parsing, dyld, the time before `init_logging`
    // registers the logger and the trace_epoch is set) or the epilogue
    // (drop, exit), so the externally-measured duration is the only honest
    // answer to "how long did the whole thing take". Quantize to
    // microseconds — same precision as in-trace records, so the output
    // doesn't mix `4.5ms` and `19.161583ms`.
    let started = Instant::now();
    let output = Command::new(&wt)
        .arg("-vv")
        .args(wt_args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .unwrap_or_else(|e| {
            eprintln!("Failed to spawn {}: {e}", wt.display());
            std::process::exit(1);
        });
    let wall = Duration::from_micros(started.elapsed().as_micros() as u64);

    let content = std::fs::read_to_string(&jsonl).unwrap_or_else(|e| {
        eprintln!("Failed to read {}: {e}", jsonl.display());
        eprintln!("wt exited with {}; check that the command runs past `init_logging` (e.g. avoid `--version`/`--help`).", output.status);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.is_empty() {
            eprintln!("--- wt stderr ---\n{stderr}");
        }
        std::process::exit(1);
    });
    let entries = worktrunk::trace::parse_lines(&content);

    if entries.is_empty() {
        eprintln!(
            "No trace records in {}. wt exited with {}.",
            jsonl.display(),
            output.status,
        );
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

/// The repo wt will operate on, mirroring wt's own resolution: a `-C <path>` /
/// `-C<path>` in the args (wt's global flag), else the current directory. This
/// is the directory whose `trace.jsonl` wt writes, so reading it back can't
/// drift to a different repo.
fn wt_target_dir(wt_args: &[String]) -> PathBuf {
    let mut args = wt_args.iter();
    while let Some(arg) = args.next() {
        if arg == "-C" {
            if let Some(path) = args.next() {
                return PathBuf::from(path);
            }
        } else if let Some(path) = arg.strip_prefix("-C") {
            return PathBuf::from(path);
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// `<git-common-dir>/wt/logs/trace.jsonl` for the repo at `dir`, or `None`
/// when `dir` isn't inside a git repository. The common dir is shared across
/// linked worktrees, so this resolves to the same file wt writes.
fn trace_jsonl_path(dir: &std::path::Path) -> Option<PathBuf> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--git-common-dir"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let common = String::from_utf8(out.stdout).ok()?;
    let common = PathBuf::from(common.trim());
    let common = if common.is_absolute() {
        common
    } else {
        dir.join(common)
    };
    Some(common.join("wt").join("logs").join("trace.jsonl"))
}

/// Render parsed entries as a column-aligned, start-time-sorted timeline.
///
/// `wall` is the externally-measured spawn → wait duration. The trace
/// can't see the prelude (argv parsing, dyld, time before `init_logging`
/// sets the trace epoch) or the exit path, so reporting `wall` lets
/// readers see how much of the process the trace actually accounts for —
/// the gap between `traced` and `wall` is the unobserved overhead.
///
/// Column alignment uses `tabwriter`'s elastic tabstops (write `\t`-separated
/// rows, padding is computed at flush). Durations are rendered via
/// `Duration`'s `Debug` impl, which produces compact units (`999µs`, `4.5ms`,
/// `1.5s`) — matches what we want without a dedicated humanization crate.
fn render_timeline(entries: &[TraceEntry], wall: Duration) -> String {
    let mut sorted: Vec<&TraceEntry> = entries.iter().collect();
    sorted.sort_by_key(|e| e.start_time_us.unwrap_or(0));

    let mut tw = tabwriter::TabWriter::new(Vec::<u8>::new())
        .minwidth(2)
        .padding(2);
    writeln!(tw, "ts(ms)\tdur\ttid\tkind\tname").unwrap();
    for e in &sorted {
        let (kind, dur, name) = describe(e);
        let ts_ms = e.start_time_us.unwrap_or(0) as f64 / 1_000.0;
        let tid = e
            .thread_id
            .map(|t| t.to_string())
            .unwrap_or_else(|| "-".into());
        writeln!(tw, "{ts_ms:.3}\t{dur:?}\t{tid}\t{kind}\t{name}").unwrap();
    }
    tw.flush().unwrap();
    let mut out = String::from_utf8(tw.into_inner().unwrap()).unwrap();

    // Summary: subprocess totals + traced span + true process wall.
    let cmds: Vec<(Duration, String)> = sorted
        .iter()
        .filter_map(|e| match &e.kind {
            TraceEntryKind::Command { duration, .. } => {
                let (_, _, name) = describe(e);
                Some((*duration, name))
            }
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
            "{} subprocess{plural} totaling {cmd_total:?} (slowest: {dur:?} {name})\n",
            cmds.len(),
        ));
    } else {
        out.push_str("0 subprocesses\n");
    }
    out.push_str(&format!("traced: {traced:?} (first → last record)\n"));
    out.push_str(&format!(
        "wall:   {wall:?} (spawn → wait; +{untraced:?} untraced prelude/epilogue)\n"
    ));
    out
}

/// Extract the (kind, duration, display-name) tuple for a trace entry.
fn describe(e: &TraceEntry) -> (&'static str, Duration, String) {
    match &e.kind {
        TraceEntryKind::Command {
            command,
            duration,
            result,
            ..
        } => {
            let mut label = match e.context.as_deref() {
                Some(c) => format!("{command} [{c}]"),
                None => command.clone(),
            };
            match result {
                TraceResult::Completed { success: false } => label.push_str("  (ok=false)"),
                TraceResult::Error { message } => label.push_str(&format!("  (err: {message})")),
                TraceResult::Completed { success: true } => {}
            }
            ("cmd", *duration, label)
        }
        TraceEntryKind::Span { name, duration } => ("span", *duration, name.clone()),
        TraceEntryKind::Instant { name } => ("event", Duration::ZERO, name.clone()),
    }
}

/// Duration of an entry (zero for instant events).
fn duration_of(e: &TraceEntry) -> Duration {
    match &e.kind {
        TraceEntryKind::Command { duration, .. } | TraceEntryKind::Span { duration, .. } => {
            *duration
        }
        TraceEntryKind::Instant { .. } => Duration::ZERO,
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
            "No trace records found in input.\n\
             Capture one by running the target command with `-vv`, then read\n\
             `.git/wt/logs/trace.jsonl`. See `wt-perf <subcommand> --help`."
        );
        std::process::exit(1);
    }

    entries
}

/// Analyze trace entries for cache effectiveness.
///
/// Outputs structured JSON to stdout, composable with jq. The analysis lives in
/// `worktrunk::trace::CacheReport` so `wt config state logs profile` and this
/// helper share one implementation: each `(command, context)` pair run N times
/// counts the first call as necessary and the rest as extra, with wasted time
/// summed over all but the slowest (likely cold) run per context. Commands that
/// read stdin (`stdin=true`) are excluded — their input isn't in the command
/// string, so identical lines aren't necessarily redundant.
fn cache_check(entries: &[worktrunk::trace::TraceEntry]) {
    let report = worktrunk::trace::CacheReport::from_entries(entries);
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `wt_target_dir` mirrors wt's `-C` resolution so the trace is read from
    /// the repo wt wrote it to. Covers the space form (`-C path`), the attached
    /// form (`-C<path>`), first-occurrence wins, and the cwd fallback.
    #[test]
    fn wt_target_dir_resolves_minus_c() {
        let s = |v: &[&str]| wt_target_dir(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(s(&["-C", "/tmp/repo", "list"]), PathBuf::from("/tmp/repo"));
        assert_eq!(s(&["-C/tmp/repo", "list"]), PathBuf::from("/tmp/repo"));
        assert_eq!(s(&["-C", "/a", "-C", "/b"]), PathBuf::from("/a")); // first wins
        // No `-C` → current directory (not the literal "list" argument).
        assert_eq!(s(&["list"]), std::env::current_dir().unwrap());
    }

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

    #[test]
    fn renders_sorted_timeline_with_summary() {
        // Emit order swaps span and child cmd (parent finishes after child),
        // so this exercises the sort-by-start-time guarantee. Durations are
        // chosen so std `Duration` Debug renders compact (no trailing
        // sub-millisecond precision): 4ms, 4.1ms, 280µs, 8µs.
        let entries = vec![
            cmd("git rev-parse HEAD", Some("repo"), 50, 4_000, 1, true),
            span("prewarm", 30, 4_100, 1),
            span("init_logging", 0, 8, 1),
            span("user_config_load", 4_200, 280, 38),
        ];
        // Wall = 6ms; traced = 4.48ms (4.2ms start → 4.48ms end);
        // untraced prelude/epilogue = 6 - 4.48 = ~1.52ms.
        insta::assert_snapshot!(
            render_timeline(&entries, Duration::from_micros(6_000)),
            @r"
        ts(ms)  dur    tid  kind  name
        0.000   8µs    1    span  init_logging
        0.030   4.1ms  1    span  prewarm
        0.050   4ms    1    cmd   git rev-parse HEAD [repo]
        4.200   280µs  38   span  user_config_load

        1 subprocess totaling 4ms (slowest: 4ms git rev-parse HEAD [repo])
        traced: 4.48ms (first → last record)
        wall:   6ms (spawn → wait; +1.52ms untraced prelude/epilogue)
        "
        );
    }

    #[test]
    fn cmd_failure_annotates_name() {
        let entries = vec![cmd("git foo", None, 0, 1_000, 1, false)];
        insta::assert_snapshot!(
            render_timeline(&entries, Duration::from_millis(2)),
            @r"
        ts(ms)  dur  tid  kind  name
        0.000   1ms  1    cmd   git foo  (ok=false)

        1 subprocess totaling 1ms (slowest: 1ms git foo  (ok=false))
        traced: 1ms (first → last record)
        wall:   2ms (spawn → wait; +1ms untraced prelude/epilogue)
        "
        );
    }
}
