//! CLI for worktrunk performance testing and tracing.
//!
//! # Usage
//!
//! ```bash
//! # Set up a benchmark repo
//! wt-perf setup typical-8 --path /tmp/bench
//!
//! # Invalidate caches for cold run
//! wt-perf invalidate /tmp/bench/main
//!
//! # Parse trace logs (pipe from wt command)
//! RUST_LOG=debug wt list 2>&1 | grep wt-trace | wt-perf trace > trace.json
//!
//! # Set up picker test environment
//! wt-perf setup picker-test
//! ```

use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;

use clap::{Parser, Subcommand};
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
  RUST_LOG=debug wt list 2>&1 | grep wt-trace | wt-perf trace > trace.json

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
  RUST_LOG=debug wt list 2>&1 | grep wt-trace | wt-perf cache-check

  # From a file
  wt-perf cache-check trace.log

  # With a benchmark repo
  cargo run -p wt-perf -- setup typical-8 --persist
  RUST_LOG=debug wt -C /tmp/wt-perf-typical-8 list 2>&1 | \
    grep wt-trace | cargo run -p wt-perf -- cache-check
"#)]
    CacheCheck {
        /// Path to trace log file (reads from stdin if omitted)
        file: Option<PathBuf>,
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
                "  RUST_LOG=debug wt -C {} list 2>&1 | grep wt-trace | wt-perf trace > trace.json",
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
                    "\
Reading from stdin... (pipe trace data or use Ctrl+D to end)

Hint: RUST_LOG=debug wt list 2>&1 | grep wt-trace | wt-perf <subcommand>"
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
            "\
No trace entries found in input.

Trace lines should look like:
  [wt-trace] ts=1234567890 tid=3 cmd=\"git status\" dur_us=12300 ok=true
  [wt-trace] ts=1234567890 tid=3 event=\"Showed skeleton\"

To capture traces, run with RUST_LOG=debug:
  RUST_LOG=debug wt list 2>&1 | grep wt-trace | wt-perf <subcommand>"
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
