//! Config-driven mock executable for integration tests.
//!
//! Reads a JSON config file to determine responses. When invoked as `gh`,
//! looks for `gh.json` and responds based on config.
//!
//! Config location: `MOCK_CONFIG_DIR` env var (set by test harness)
//!
//! Config format:
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
//! - `delay_ms`: sleep this long before responding (default 0), to simulate a
//!   slow command (e.g. a forge call the picker streams in behind its frame)
//! - `hold_until_parent_exit`: hold the response until the parent process (the
//!   `wt` under test) exits, then exit without producing output. Lets a test
//!   pin a *transient* frame — e.g. the picker's `Loading open PRs…` marker,
//!   on screen only while a forge call is in flight — on screen for exactly as
//!   long as the picker lives, with no fixed `delay_ms` to outguess boot
//!   latency. Overrides `delay_ms` and any `file`/`output` response.

use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
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
    #[serde(default)]
    delay_ms: u64,
    #[serde(default)]
    hold_until_parent_exit: bool,
}

/// Block until the parent process (the `wt` under test) exits, then return.
///
/// A test that asserts a *transient* frame — the picker's `Loading open PRs…`
/// marker, on screen only while a forge call is in flight — needs that frame to
/// stay up for exactly as long as the picker lives, with no fixed sleep to
/// outguess boot latency. This mock *is* the in-flight forge call: the picker's
/// fetch thread runs it with a piped stdout and blocks reading that pipe to EOF,
/// so as long as this process neither writes a full response nor exits, the
/// marker stays. When the test aborts the picker, `wt` detaches the fetch thread
/// and exits, which closes the read end of our stdout pipe; the next write here
/// then fails with `BrokenPipe`.
///
/// Polling for that write error is a *causal* parent-death signal — release is
/// tied to the parent exiting, not to a timer — and it needs no platform
/// primitive (`getppid` / `OpenProcess`), so it behaves identically on Unix and
/// Windows. Rust's runtime ignores `SIGPIPE`, so the broken write surfaces as an
/// `Err` rather than killing this process.
///
/// The one-byte probes written into stdout are harmless: this mode's contract is
/// that the parent aborts without ever parsing the response, and the fetch
/// thread drains the pipe until process exit, so the bytes are never observed. A
/// generous cap bounds a stuck orphan if the pipe somehow never breaks.
fn wait_for_parent_exit() {
    // 20ms per poll × 3000 = a 60s ceiling, far beyond any picker lifetime; it
    // exists only so a detached mock can't linger forever if detection fails.
    const MAX_POLLS: u32 = 3000;
    let mut stdout = io::stdout();
    for _ in 0..MAX_POLLS {
        match stdout.write_all(&[0]).and_then(|()| stdout.flush()) {
            // Write succeeded → the parent's read end is still open → still alive.
            Ok(()) => sleep(Duration::from_millis(20)),
            // BrokenPipe (or any other write failure) → the parent has exited.
            Err(_) => return,
        }
    }
}

/// Get command name from argv\[0\].
fn command_name() -> String {
    let argv0 = env::args().next().expect("mock: no argv[0]");
    std::path::Path::new(&argv0)
        .file_stem()
        .expect("mock: argv[0] has no file stem")
        .to_string_lossy()
        .into_owned()
}

fn config_dir() -> PathBuf {
    PathBuf::from(env::var_os("MOCK_CONFIG_DIR").expect("mock: MOCK_CONFIG_DIR not set"))
}

/// Append this invocation's argv to `<MOCK_CALL_LOG_DIR>/<command>.calls` so
/// a test can assert *how many times* and *with what arguments* a command was
/// spawned, not just what it returned. Needed wherever the spawn count is the
/// behavior under test — e.g. the fsmonitor sweep resolving every daemon in
/// one batched `lsof` rather than one call per PID.
///
/// Opt-in, and deliberately NOT written next to the JSON config. Tests
/// routinely place `MOCK_CONFIG_DIR` inside the repo under test
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
    let Some(dir) = env::var_os("MOCK_CALL_LOG_DIR") else {
        return;
    };
    let path = PathBuf::from(dir).join(format!("{}.calls", cmd_name));
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{}", args.join(" "));
    }
}

fn main() {
    let cmd_name = command_name();
    let config_dir = config_dir();
    let config_path = config_dir.join(format!("{}.json", cmd_name));

    log_invocation(&cmd_name, &env::args().skip(1).collect::<Vec<_>>());

    let content = fs::read_to_string(&config_path).unwrap_or_else(|e| {
        eprintln!("mock: failed to read {}: {}", config_path.display(), e);
        exit(1);
    });

    let config: Config = serde_json::from_str(&content).unwrap_or_else(|e| {
        eprintln!("mock: failed to parse {}: {}", config_path.display(), e);
        exit(1);
    });

    let args: Vec<String> = env::args().skip(1).collect();

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
        delay_ms: 0,
        hold_until_parent_exit: false,
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

    // Hold the response for the parent's whole lifetime, then exit without
    // output — the marker-clearing edge is the parent's death, not a timer.
    if response.hold_until_parent_exit {
        wait_for_parent_exit();
        exit(0);
    }

    // Simulate a slow command (e.g. a forge call) so tests can observe the
    // caller's in-flight UI before the response lands.
    if response.delay_ms > 0 {
        sleep(Duration::from_millis(response.delay_ms));
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
