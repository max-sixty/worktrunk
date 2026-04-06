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

            // Create repo at base_path (main worktree location)
            // Worktrees will be siblings: base_path.feature-wt-N
            eprintln!("Creating {} repo...", config);
            create_repo_at(&repo_config, &base_path);

            let repo_name = base_path.file_name().unwrap().to_str().unwrap();
            let parent_dir = base_path.parent().unwrap();
            eprintln!();
            eprintln!("✅ Repository created");
            eprintln!();
            eprintln!("Main worktree: {}", base_path.display());
            if repo_config.worktrees > 1 {
                eprintln!("Worktrees: {} total", repo_config.worktrees);
                for i in 1..repo_config.worktrees {
                    let branch = format!("feature-wt-{i}");
                    eprintln!(
                        "  - {}: {}",
                        branch,
                        parent_dir.join(format!("{repo_name}.{branch}")).display()
                    );
                }
            }
            if repo_config.branches > 0 {
                eprintln!("Branches: {}", repo_config.branches);
            }
            eprintln!();
            eprintln!("To run with tracing:");
            eprintln!(
                "  RUST_LOG=debug wt -C {} list 2>&1 | grep wt-trace | wt-perf trace > trace.json",
                base_path.display()
            );
            eprintln!();
            eprintln!("To invalidate caches (cold run):");
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
            eprintln!("✅ Caches invalidated for {}", repo.display());
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

/// Truncate a string for display, respecting UTF-8 char boundaries.
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut boundary = max - 3;
        while !s.is_char_boundary(boundary) {
            boundary -= 1;
        }
        format!("{}...", &s[..boundary])
    }
}

/// Analyze trace entries for cache effectiveness and report findings.
///
/// Two analyses:
/// 1. Same-context duplicates — commands that ran multiple times for the
///    same worktree, indicating missing or bypassed caches
/// 2. Summary — total commands, unique commands, duplicate counts
fn cache_check(entries: &[worktrunk::trace::TraceEntry]) {
    use std::collections::{BTreeMap, HashMap, HashSet};
    use worktrunk::trace::TraceEntryKind;

    let mut total_commands = 0;
    let mut cmd_counts: HashMap<&str, usize> = HashMap::new();
    let mut pair_counts: HashMap<(&str, &str), usize> = HashMap::new();
    let mut contexts: HashSet<&str> = HashSet::new();

    for entry in entries {
        if let TraceEntryKind::Command { command, .. } = &entry.kind {
            let ctx = entry.context.as_deref().unwrap_or("(none)");
            *cmd_counts.entry(command.as_str()).or_insert(0) += 1;
            *pair_counts.entry((command.as_str(), ctx)).or_insert(0) += 1;
            contexts.insert(ctx);
            total_commands += 1;
        }
    }

    // === Same-context duplicates ===
    let mut cmd_ctx_info: BTreeMap<&str, Vec<(&str, usize)>> = BTreeMap::new();
    for ((cmd, ctx), count) in &pair_counts {
        if *count > 1 {
            cmd_ctx_info.entry(cmd).or_default().push((ctx, *count));
        }
    }

    if !cmd_ctx_info.is_empty() {
        println!(
            "\
=== Same-context duplicates (potential cache misses) ===

Commands that ran multiple times for the SAME worktree.
These are the strongest signal of missing/bypassed caches.
"
        );

        let mut sorted: Vec<_> = cmd_ctx_info.iter().collect();
        sorted.sort_by(|a, b| {
            let max_a = a.1.iter().map(|(_, c)| c).max().unwrap();
            let max_b = b.1.iter().map(|(_, c)| c).max().unwrap();
            max_b.cmp(max_a)
        });

        let mut total_wasted = 0usize;
        for (cmd, ctx_list) in &sorted {
            let max_count = ctx_list.iter().map(|(_, c)| c).max().unwrap();
            let extra: usize = ctx_list.iter().map(|(_, c)| c - 1).sum();
            total_wasted += extra;
            println!(
                "  {} (max {}x/context, {} extra calls)",
                truncate(cmd, 70),
                max_count,
                extra
            );
            let mut sorted_ctx: Vec<_> = ctx_list.to_vec();
            sorted_ctx.sort_by(|a, b| b.1.cmp(&a.1));
            for (ctx, count) in sorted_ctx.iter().take(3) {
                println!("      {}x in [{}]", count, ctx);
            }
            if sorted_ctx.len() > 3 {
                println!("      ... and {} more contexts", sorted_ctx.len() - 3);
            }
        }
        println!();
        println!("  Total extra calls from same-context duplicates: {total_wasted}");
    }

    // === Summary ===
    let dup_count: usize = cmd_counts.values().filter(|c| **c > 1).count();
    let dup_total: usize = cmd_counts.values().filter(|c| **c > 1).map(|c| c - 1).sum();
    println!(
        "\
\n=== Summary ===

  {total_commands} commands total, {} unique, {} contexts
  {dup_count} commands ran more than once ({dup_total} extra calls)",
        cmd_counts.len(),
        contexts.len()
    );
}
