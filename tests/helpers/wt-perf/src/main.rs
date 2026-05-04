//! CLI for worktrunk performance testing and tracing.
//!
//! Run `wt-perf --help` (and `wt-perf <subcommand> --help`) for usage.

use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use wt_perf::{
    canonicalize, create_repo_at, invalidate_caches_auto, parse_config, render_timeline,
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
    /// Sets `RUST_LOG=debug` on the child so `[wt-trace]` records emit on
    /// stderr alongside the rest of debug output, parses out the trace
    /// records, sorts them by start time, and prints a column-aligned
    /// timeline to stdout. With `--chrome`, emits Chrome Trace Format JSON
    /// instead — pipe to a file and open in chrome://tracing or
    /// https://ui.perfetto.dev.
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

/// Run a `wt` command with `RUST_LOG=debug`, capture stderr, and render.
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
    // answer to "how long did the whole thing take". Quantize to
    // microseconds — same precision as in-trace records, so the output
    // doesn't mix `4.5ms` and `19.161583ms`.
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
    let wall = Duration::from_micros(started.elapsed().as_micros() as u64);

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
