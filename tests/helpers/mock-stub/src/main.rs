//! Config-driven mock executable for integration tests.
//!
//! Reads a JSON config file to determine responses. When invoked as `gh`,
//! looks for `gh.json` and responds based on config.
//!
//! Config location: `WORKTRUNK_TEST_MOCK_CONFIG_DIR` env var (set by test harness)
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
//! - `wait_for_file`: wait for this path under the config directory to exist
//!   before responding. Lets a test observe a loading state, then causally
//!   release the response without another user input.

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
    wait_for_file: Option<String>,
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
    PathBuf::from(
        env::var_os("WORKTRUNK_TEST_MOCK_CONFIG_DIR")
            .expect("mock: WORKTRUNK_TEST_MOCK_CONFIG_DIR not set"),
    )
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
