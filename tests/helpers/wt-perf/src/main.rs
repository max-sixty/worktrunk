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
//! # Set up select test environment
//! wt-perf setup select-test
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
        /// Config name: typical-N, branches-N, branches-N-M, divergent, select-test
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
                eprintln!("  select-test     - Config for wt select testing");
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

            let repo_path = base_path.join("main");
            eprintln!();
            eprintln!("✅ Repository created");
            eprintln!();
            eprintln!("Main worktree: {}", repo_path.display());
            if repo_config.worktrees > 1 {
                eprintln!("Worktrees: {} total", repo_config.worktrees);
                for i in 1..repo_config.worktrees {
                    eprintln!(
                        "  - wt-{}: {}",
                        i,
                        base_path.join(format!("wt-{i}")).display()
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
                repo_path.display()
            );
            eprintln!();
            eprintln!("To invalidate caches (cold run):");
            eprintln!("  wt-perf invalidate {}", repo_path.display());

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
            let input = match file {
                Some(path) if path.as_os_str() != "-" => match std::fs::read_to_string(&path) {
                    Ok(content) => content,
                    Err(e) => {
                        eprintln!("Error reading {}: {}", path.display(), e);
                        std::process::exit(1);
                    }
                },
                _ => {
                    if std::io::stdin().is_terminal() {
                        eprintln!("Reading from stdin... (pipe trace data or use Ctrl+D to end)");
                        eprintln!();
                        eprintln!(
                            "Hint: RUST_LOG=debug wt list 2>&1 | grep wt-trace | wt-perf trace"
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
                eprintln!("No trace entries found in input.");
                eprintln!();
                eprintln!("Trace lines should look like:");
                eprintln!("  [wt-trace] ts=1234567890 tid=3 cmd=\"git status\" dur=12.3ms ok=true");
                eprintln!("  [wt-trace] ts=1234567890 tid=3 event=\"Showed skeleton\"");
                eprintln!();
                eprintln!("To capture traces, run with RUST_LOG=debug:");
                eprintln!("  RUST_LOG=debug wt list 2>&1 | grep wt-trace | wt-perf trace");
                std::process::exit(1);
            }

            println!("{}", worktrunk::trace::to_chrome_trace(&entries));
        }
    }
}
