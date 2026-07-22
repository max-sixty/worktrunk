//! CLI for worktrunk performance testing and tracing.
//!
//! Run `wt-perf --help` (and `wt-perf <subcommand> --help`) for usage.

use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use wt_perf::{
    PRUNE_REAL_MERGED, PRUNE_REAL_UNMERGED, canonicalize, ensure_prune_real_repo,
    invalidate_caches_auto, parse_config, parse_pair, wt_perf_cache_dir,
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
        /// Config name: typical-N, branches-N, branches-N-M, divergent, mixed-W-B, prune-M-U, prune-real[-M-U], picker-test
        config: String,

        /// Directory to create repo in (default: wt-perf cache directory)
        #[arg(long)]
        path: Option<PathBuf>,
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

    /// Run a `wt` command with tracing on and render a timeline.
    ///
    /// Runs the child with `-vv` so it writes `trace.jsonl`, reads that back,
    /// sorts the records by start time, and prints a column-aligned timeline
    /// to stdout. With `--chrome`, emits Chrome Trace Format JSON instead —
    /// pipe to a file and open in chrome://tracing or https://ui.perfetto.dev.
    #[command(after_long_help = r#"EXAMPLES:
  # Text timeline of `wt list` in the current repo
  wt-perf timeline -- list

  # Cold-cache run (invalidates the traced repo, then runs)
  wt-perf timeline --cold -- list

  # Cold run against a specific repo (setup prints the exact path)
  wt-perf timeline --cold -- -C ~/.cache/wt-perf/typical-1 list

  # Chrome Trace Format JSON for Perfetto
  wt-perf timeline --chrome -- list > trace.json
"#)]
    Timeline {
        /// Invalidate the traced repo's caches before running (cold measurement).
        #[arg(long)]
        cold: bool,

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
        Commands::Setup { config, path } => {
            // `prune-real[-M-U]`: cache-managed rust-scale fixture (built once
            // under the wt-perf cache dir, repaired after a live prune consumes
            // its candidates) — takes no --path and never offers cleanup.
            // Tested before parse_config so its `prune-` arm never sees it.
            let prune_real = if config == "prune-real" {
                Some((PRUNE_REAL_MERGED, PRUNE_REAL_UNMERGED))
            } else {
                parse_pair(&config, "prune-real-")
            };
            if let Some((merged, unmerged)) = prune_real {
                if path.is_some() {
                    eprintln!(
                        "prune-real fixtures are cache-managed under {}; --path is not supported",
                        wt_perf_cache_dir().join("bench-repos").display()
                    );
                    std::process::exit(1);
                }
                let repo = ensure_prune_real_repo(merged, unmerged);
                eprintln!(
                    "Ready: main @ {}, {} worktrees, {} branches",
                    repo.display(),
                    merged + unmerged + 1,
                    merged + unmerged
                );
                eprintln!();
                eprintln!(
                    "  wt-perf timeline -- -C {} step prune --dry-run --min-age 0s",
                    repo.display()
                );
                eprintln!(
                    "  wt-perf timeline -- -C {} step prune --min-age 0s   # live; next setup/bench run re-creates the candidates",
                    repo.display()
                );
                // No `wt-perf invalidate` hint: deleting this fixture's
                // worktree indexes flips prune's clean-worktree gate and
                // degrades every later run (ensure_prune_real_repo heals it,
                // but only on the next setup/bench call).
                return;
            }

            let spec = parse_config(&config).unwrap_or_else(|| {
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
                    "  prune-M-U       - M squash-merged candidates + U unmerged worktrees/branches (wt step prune workload)"
                );
                eprintln!(
                    "  prune-real[-M-U] - rust-lang/rust clone + M squash-merged candidates + U unmerged worktrees/branches, cached in the wt-perf cache dir (default {PRUNE_REAL_MERGED}-{PRUNE_REAL_UNMERGED}; first run clones from network)"
                );
                eprintln!("  picker-test     - Config for wt switch interactive picker testing");
                std::process::exit(1);
            });

            let base_path = if let Some(p) = path {
                std::fs::create_dir_all(&p).unwrap();
                canonicalize(&p).unwrap()
            } else {
                let dir = wt_perf_cache_dir().join(&config);
                if dir.exists() {
                    std::fs::remove_dir_all(&dir).unwrap();
                }
                std::fs::create_dir_all(&dir).unwrap();
                canonicalize(&dir).unwrap()
            };

            eprintln!("Creating {} repo...", config);
            let (worktrees, branches) = spec.create_at(&base_path);

            let mut parts = vec![format!("main @ {}", base_path.display())];
            if worktrees > 1 {
                parts.push(format!("{} worktrees", worktrees));
            }
            if branches > 0 {
                parts.push(format!("{} branches", branches));
            }
            eprintln!("Created: {}", parts.join(", "));
            eprintln!();
            let example_args = if matches!(spec, wt_perf::SetupConfig::Prune { .. }) {
                "step prune --dry-run --min-age 0s"
            } else {
                "list --progressive"
            };
            eprintln!(
                "  wt-perf timeline -- -C {} {}",
                base_path.display(),
                example_args
            );
            eprintln!(
                "  wt-perf timeline --chrome -- -C {} {} > trace.json",
                base_path.display(),
                example_args
            );
            eprintln!("  wt-perf invalidate {}", base_path.display());
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

        Commands::Timeline {
            cold,
            chrome,
            wt_args,
        } => run_timeline(cold, chrome, &wt_args),
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

/// Run a `wt -vv` command and render the `trace.jsonl` it writes.
///
/// `-vv` writes the machine trace to `<git-common-dir>/wt/logs/trace.jsonl` in
/// the repo wt operated on (the humanized stderr/`trace.log` isn't parseable).
/// We locate that repo the same way wt does — a `-C` in the args, else the
/// cwd — and read the file back after the run.
fn run_timeline(cold: bool, chrome: bool, wt_args: &[String]) {
    let wt = resolve_wt_binary();
    // The trace lands in the repo wt operates on — resolved from `-C`/cwd the
    // same way wt resolves it, so we never read a different repo than wt wrote.
    // `--cold` invalidates that same repo.
    let trace_dir = wt_target_dir(wt_args);

    if cold {
        let path = canonicalize(&trace_dir).unwrap_or_else(|e| {
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
        print!("{}", worktrunk::trace::render_timeline(&entries, wall));
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
}
