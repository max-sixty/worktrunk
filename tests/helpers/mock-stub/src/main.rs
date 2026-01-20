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
//! - `output`: output literal string
//! - `exit_code`: exit with specified code (default 0)

use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::exit;

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

fn main() {
    let cmd_name = command_name();
    let config_dir = config_dir();
    let config_path = config_dir.join(format!("{}.json", cmd_name));

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
