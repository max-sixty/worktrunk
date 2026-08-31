//! Config-driven playback mode for the mock commands the test suite puts on
//! `PATH`.
//!
//! [`mock_commands`](super::mock_commands) links the `wt` binary itself into a
//! test's mock bin dir under names like `gh` or `glab`. `main()` calls
//! [`maybe_run`] before anything else, and an invocation whose argv\[0\] is such
//! a name plays back the JSON config instead of running wt. The mocks being
//! the binary under test is what keeps them fresh: every runner rebuilds `wt`
//! whenever the integration tests build, so there is no separate helper
//! binary to go missing or stale.
//!
//! Config location: `WORKTRUNK_TEST_MOCK_CONFIG_DIR` env var, set by the test
//! harness on the wt under test and inherited by the commands it spawns.
//! Production wt never sees the variable, so the dispatch is inert outside the
//! suite.
//!
//! Config format (`<command>.json` in the config dir):
//! ```json
//! {
//!   "version": "gh version 2.0.0 (mock)",
//!   "commands": {
//!     "auth": { "exit_code": 0 },
//!     "pr": { "file": "pr_data.json" },
//!     "run": { "output": "[{\"status\": \"completed\"}]" }
//!   }
//! }
//! ```
//!
//! Command matching (in priority order):
//! 1. `gh --version` → outputs version string
//! 2. Triple: `glab mr view 123` → matches "mr view 123" (first three args)
//! 3. Compound: `gh mr list ...` → matches "mr list" (first two args)
//! 4. Single: `gh mr ...` → matches "mr" (first arg only)
//! 5. `_default` → fallback if no match
//!
//! This allows different responses for `glab mr view 1` vs `glab mr view 2`.
//!
//! Response types:
//! - `file`: read and output contents of specified file (relative to config dir)
//! - `output`: output literal string to stdout
//! - `stderr`: output literal string to stderr
//! - `exit_code`: exit with specified code (default 0)
//! - `wait_for_file`: wait for this path under the config directory to exist
//!   before responding. Lets a test observe a loading state, then causally
//!   release the response without another user input.

use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::exit;
use std::thread::sleep;
use std::time::Duration;

#[derive(Debug, Deserialize)]
struct Config {
    version: Option<String>,
    #[serde(default)]
    commands: HashMap<String, CommandResponse>,
}

#[derive(Debug, Deserialize)]
struct CommandResponse {
    file: Option<String>,
    output: Option<String>,
    stderr: Option<String>,
    #[serde(default)]
    exit_code: i32,
    wait_for_file: Option<String>,
}

/// Play back the mock config and exit when this invocation is a mock command:
/// `WORKTRUNK_TEST_MOCK_CONFIG_DIR` is set, argv\[0\] is not wt's own name, and
/// the config dir holds a `<argv0>.json` for it. Returns (doing nothing)
/// otherwise. Must run before argument parsing — a mock's arguments are the
/// mocked tool's, not wt's.
pub fn maybe_run() {
    let Some(dir) = env::var_os("WORKTRUNK_TEST_MOCK_CONFIG_DIR") else {
        return;
    };
    // args_os, not args: `env::args()` panics during iteration on a
    // non-Unicode argument, and this runs inside wt's `main()` on every
    // invocation. A non-UTF8 or degenerate argv[0] yields a name matching no
    // config, so the invocation falls through to "not a mock" instead.
    let Some(name) = env::args_os()
        .next()
        .and_then(|arg0| crate::path::executable_name(Path::new(&arg0)))
    else {
        return;
    };
    // The wt under test runs with the variable set too (that's how its mock
    // children inherit it); its own names never select playback, whatever a
    // config dir contains — a `wt.json` would otherwise shadow the binary
    // under test. Case-insensitive, because the config probe below goes
    // through the filesystem, which matches names case-insensitively on
    // macOS and Windows — an exact comparison would let argv[0] `WT` reach
    // a `wt.json`. `MockConfig::new` rejects these names on the harness side.
    if name.eq_ignore_ascii_case("wt") || name.eq_ignore_ascii_case("git-wt") {
        return;
    }
    let config_dir = PathBuf::from(dir);
    // Playback is only for commands the harness registered, and a registered
    // mock always has its config: `MockConfig::write` writes `<name>.json`
    // before linking the binary, and is the only way to create a mock link.
    // So a foreign argv[0] with no config is not a half-configured mock —
    // it's wt itself under another name, e.g. the argv0-validation tests'
    // `wt;touch` symlink, and it must run as wt.
    if !config_dir.join(format!("{name}.json")).exists() {
        return;
    }
    run(&name, &config_dir);
}

/// Append this invocation's argv to
/// `<WORKTRUNK_TEST_MOCK_CALL_LOG_DIR>/<command>.calls` so a test can assert
/// *how many times* and *with what arguments* a command was spawned, not just
/// what it returned. Needed wherever the spawn count is the behavior under
/// test — e.g. the fsmonitor sweep resolving every daemon in one batched
/// `lsof` rather than one call per PID.
///
/// Opt-in, and deliberately NOT written next to the JSON config. Tests
/// routinely place `WORKTRUNK_TEST_MOCK_CONFIG_DIR` inside the repo under test
/// (`<repo>/.bin`), so logging there would create an untracked file mid-run
/// and change what the command being tested observes — a hook-spawned mock
/// dirties the working tree, and `wt merge` then stashes. Observability must
/// not perturb the system under test, so the log goes wherever the test says
/// and nowhere by default.
///
/// One line per invocation, arguments space-joined. Best-effort: a log-write
/// failure must not change what the mock returns, or a test would fail on the
/// logging rather than on its actual assertion.
fn log_invocation(cmd_name: &str, args: &[String]) {
    let Some(dir) = env::var_os("WORKTRUNK_TEST_MOCK_CALL_LOG_DIR") else {
        return;
    };
    append_call_line(
        &PathBuf::from(dir).join(format!("{}.calls", cmd_name)),
        args,
    );
}

/// Append one whole line to the call log.
///
/// A single `write_all` of the line *including* its newline, never
/// `writeln!`: the latter issues the newline as a second write, and mocks
/// log concurrently — the picker overlaps `gh pr list` with `gh pr view`, and
/// every mock opens the log itself. Two appends can then interleave into one
/// merged line plus an empty one, so a spawn that a test filters for by
/// prefix vanishes from the log even though the command ran. An append-mode
/// write of a whole short line is atomic against other writers, which is why
/// `command_log` writes its JSONL entries the same way.
fn append_call_line(path: &Path, args: &[String]) {
    let line = format!("{}\n", args.join(" "));
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(line.as_bytes());
    }
}

fn run(cmd_name: &str, config_dir: &Path) -> ! {
    let config_path = config_dir.join(format!("{}.json", cmd_name));

    // Lossy for the same reason as the name above: a non-UTF8 argument can't
    // match a config key (JSON keys are UTF-8), and the mock must not panic
    // on one.
    let args: Vec<String> = env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();

    log_invocation(cmd_name, &args);

    let content = fs::read_to_string(&config_path).unwrap_or_else(|e| {
        eprintln!("mock: failed to read {}: {}", config_path.display(), e);
        exit(1);
    });

    let config: Config = serde_json::from_str(&content).unwrap_or_else(|e| {
        eprintln!("mock: failed to parse {}: {}", config_path.display(), e);
        exit(1);
    });

    // Handle --version flag
    if args.first().map(|s| s.as_str()) == Some("--version")
        && let Some(version) = &config.version
    {
        println!("{}", version);
        exit(0);
    }

    // Match against commands with priority: triple > compound > single > _default
    // Triple: "mr view 123" matches before "mr view"
    // Compound: "mr list" matches before "mr"
    let default_response = CommandResponse {
        file: None,
        output: None,
        stderr: None,
        exit_code: 1,
        wait_for_file: None,
    };

    // Try triple match first (e.g., "mr view 1", "mr view 2")
    let triple_key = if args.len() >= 3 {
        Some(format!("{} {} {}", args[0], args[1], args[2]))
    } else {
        None
    };

    // Try compound match (e.g., "mr list", "mr view")
    let compound_key = if args.len() >= 2 {
        Some(format!("{} {}", args[0], args[1]))
    } else {
        None
    };

    let response = triple_key
        .as_ref()
        .and_then(|key| config.commands.get(key))
        // Fall back to compound match
        .or_else(|| {
            compound_key
                .as_ref()
                .and_then(|key| config.commands.get(key))
        })
        // Fall back to single-arg match
        .or_else(|| args.first().and_then(|cmd| config.commands.get(cmd)))
        // Fall back to _default
        .or_else(|| config.commands.get("_default"))
        .unwrap_or(&default_response);

    // Causal release gate for tests that must first observe the caller's
    // in-flight state. Bound the wait so a failed test cannot orphan the mock.
    if let Some(path) = &response.wait_for_file {
        const MAX_POLLS: u32 = 6000; // 10ms × 6000 = 60s
        let release = config_dir.join(path);
        let mut released = false;
        for _ in 0..MAX_POLLS {
            if release.exists() {
                released = true;
                break;
            }
            sleep(Duration::from_millis(10));
        }
        if !released {
            eprintln!(
                "mock: timed out waiting for release file {}",
                release.display()
            );
            exit(1);
        }
    }

    if let Some(file) = &response.file {
        let file_path = config_dir.join(file);
        match fs::read_to_string(&file_path) {
            Ok(contents) => {
                print!("{}", contents);
                io::stdout().flush().unwrap();
            }
            Err(e) => {
                eprintln!("mock: failed to read {}: {}", file_path.display(), e);
                exit(1);
            }
        }
    } else if let Some(output) = &response.output {
        print!("{}", output);
        io::stdout().flush().unwrap();
    }

    if let Some(stderr_output) = &response.stderr {
        eprint!("{}", stderr_output);
        io::stderr().flush().unwrap();
    }

    exit(response.exit_code);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Concurrent mock spawns share one call log, so every line has to land
    /// whole. Writers here are threads rather than processes because the
    /// interleaving is per write syscall, not per process — each call opens
    /// its own append handle either way — and a thread test needs no built
    /// binary. Against a `writeln!` implementation this fails on the first
    /// run: the newline arrives as a separate write, so one entry ends up
    /// pasted onto the tail of another and a test filtering the log by
    /// command prefix stops seeing a spawn that happened.
    #[test]
    fn concurrent_appends_keep_each_call_on_its_own_line() {
        const WRITERS: usize = 8;
        const CALLS_PER_WRITER: usize = 400;
        let view = ["pr", "view", "42", "--json", "comments"];
        let list = ["pr", "list", "--head", "flaky"];

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gh.calls");
        std::thread::scope(|scope| {
            for writer in 0..WRITERS {
                let path = &path;
                scope.spawn(move || {
                    let argv: &[&str] = if writer % 2 == 0 { &view } else { &list };
                    let args: Vec<String> = argv.iter().map(|arg| (*arg).to_string()).collect();
                    for _ in 0..CALLS_PER_WRITER {
                        append_call_line(path, &args);
                    }
                });
            }
        });

        let logged = fs::read_to_string(&path).unwrap();
        let malformed: Vec<_> = logged
            .lines()
            .filter(|line| *line != view.join(" ") && *line != list.join(" "))
            .collect();
        // Count and one example, not the whole list: an interleaving run
        // corrupts thousands of lines, and the shapes repeat.
        assert_eq!((malformed.len(), malformed.first().copied()), (0, None));
        assert_eq!(logged.lines().count(), WRITERS * CALLS_PER_WRITER);
    }
}
