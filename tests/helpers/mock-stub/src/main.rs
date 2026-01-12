//! Config-driven mock executable for integration tests.
//!
//! Reads a JSON config file to determine responses. When invoked as `gh`,
//! looks for `gh.json` in the same directory and responds based on config.
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
//! Command matching:
//! - `gh --version` → outputs version string
//! - `gh auth ...` → matches "auth" command
//! - `gh pr list ...` → matches "pr" command
//!
//! Response types:
//! - `file`: read and output contents of specified file (relative to config dir)
//! - `output`: output literal string
//! - `exit_code`: exit with specified code (default 0)
//!
//! Usage in tests:
//! 1. Write config to bin_dir/gh.json
//! 2. Write data files to bin_dir/ (e.g., pr_data.json)
//! 3. Copy mock-stub binary as bin_dir/gh (Unix) or bin_dir/gh.exe (Windows)
//! 4. Command::new("gh") now works on all platforms

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::exit;

/// Configuration for the mock command
#[derive(Debug)]
struct Config {
    version: Option<String>,
    commands: HashMap<String, CommandResponse>,
}

/// How to respond to a command
#[derive(Debug)]
struct CommandResponse {
    file: Option<String>,
    output: Option<String>,
    exit_code: i32,
}

fn main() {
    let exe_path = env::current_exe().expect("failed to get executable path");
    let exe_dir = exe_path
        .parent()
        .expect("mock: executable has no parent directory");

    // Get command name by stripping path and extension
    // e.g., /tmp/bin/gh.exe -> gh, /tmp/bin/gh -> gh
    let cmd_name = exe_path
        .file_stem()
        .expect("mock: executable has no file stem")
        .to_string_lossy();

    // Look for config file: gh.json for gh command
    let config_path = exe_dir.join(format!("{}.json", cmd_name));

    let debug = env::var("MOCK_DEBUG").is_ok();
    if debug {
        eprintln!("mock: exe_path={}", exe_path.display());
        eprintln!("mock: cmd_name={}", cmd_name);
        eprintln!("mock: config_path={}", config_path.display());
    }

    if !config_path.exists() {
        eprintln!("mock: config not found: {}", config_path.display());
        eprintln!("Expected JSON config file for mock command.");
        exit(1);
    }

    let config = parse_config(&config_path);
    let args: Vec<String> = env::args().skip(1).collect();

    if debug {
        eprintln!("mock: args={:?}", args);
        eprintln!("mock: config={:?}", config);
    }

    // Handle --version flag
    if args.first().map(|s| s.as_str()) == Some("--version")
        && let Some(version) = &config.version
    {
        println!("{}", version);
        exit(0);
    }

    // Match first argument against commands, fall back to _default
    let default_response = CommandResponse {
        file: None,
        output: None,
        exit_code: 1,
    };
    let response = args
        .first()
        .and_then(|cmd| config.commands.get(cmd))
        .or_else(|| config.commands.get("_default"))
        .unwrap_or(&default_response);

    // Output response
    if let Some(file) = &response.file {
        let file_path = exe_dir.join(file);
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

    exit(response.exit_code);
}

fn parse_config(path: &PathBuf) -> Config {
    let content = fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("mock: failed to read config {}: {}", path.display(), e);
        exit(1);
    });

    // Simple JSON parsing without serde dependency
    // Format: { "version": "...", "commands": { "cmd": { "file": "...", "output": "...", "exit_code": N } } }
    parse_json_config(&content).unwrap_or_else(|e| {
        eprintln!("mock: failed to parse config {}: {}", path.display(), e);
        exit(1);
    })
}

fn parse_json_config(json: &str) -> Result<Config, String> {
    // Very simple JSON parser - just enough for our config format
    // Avoids serde dependency to keep binary small and fast to compile

    let json = json.trim();
    if !json.starts_with('{') || !json.ends_with('}') {
        return Err("expected JSON object".to_string());
    }

    let mut version = None;
    let mut commands = HashMap::new();

    // Extract version if present
    if let Some(start) = json.find("\"version\"")
        && let Some(value) = extract_string_value(&json[start..])
    {
        version = Some(value);
    }

    // Extract commands object
    if let Some(start) = json.find("\"commands\"") {
        let rest = &json[start + 10..]; // skip "commands"
        if let Some(obj_start) = rest.find('{') {
            let obj_rest = &rest[obj_start..];
            if let Some(obj_end) = find_matching_brace(obj_rest) {
                let commands_json = &obj_rest[1..obj_end];
                parse_commands(commands_json, &mut commands)?;
            }
        }
    }

    Ok(Config { version, commands })
}

fn extract_string_value(json: &str) -> Option<String> {
    // Find the colon after the key
    let colon = json.find(':')?;
    let rest = json[colon + 1..].trim_start();

    // Find the opening quote
    if !rest.starts_with('"') {
        return None;
    }

    // Find the closing quote (handle escaped quotes)
    let mut chars = rest[1..].chars().peekable();
    let mut value = String::new();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&next) = chars.peek() {
                chars.next();
                match next {
                    'n' => value.push('\n'),
                    't' => value.push('\t'),
                    'r' => value.push('\r'),
                    '"' => value.push('"'),
                    '\\' => value.push('\\'),
                    _ => {
                        value.push('\\');
                        value.push(next);
                    }
                }
            }
        } else if c == '"' {
            return Some(value);
        } else {
            value.push(c);
        }
    }
    None
}

fn find_matching_brace(json: &str) -> Option<usize> {
    let mut depth = 0;
    let mut in_string = false;
    let mut escape_next = false;

    for (i, c) in json.chars().enumerate() {
        if escape_next {
            escape_next = false;
            continue;
        }
        if c == '\\' && in_string {
            escape_next = true;
            continue;
        }
        if c == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_commands(
    json: &str,
    commands: &mut HashMap<String, CommandResponse>,
) -> Result<(), String> {
    // Parse command entries: "cmd": { ... }
    let mut rest = json.trim();

    while !rest.is_empty() {
        // Skip whitespace and commas
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == ',');
        if rest.is_empty() {
            break;
        }

        // Find command name
        if !rest.starts_with('"') {
            break;
        }
        let name_end = rest[1..].find('"').ok_or("unterminated command name")?;
        let name = rest[1..=name_end].to_string();
        rest = &rest[name_end + 2..];

        // Find colon and opening brace
        rest = rest.trim_start();
        if !rest.starts_with(':') {
            return Err(format!("expected ':' after command name '{}'", name));
        }
        rest = rest[1..].trim_start();

        if !rest.starts_with('{') {
            return Err(format!("expected '{{' for command '{}'", name));
        }

        let brace_end = find_matching_brace(rest).ok_or("unterminated command object")?;
        let cmd_json = &rest[1..brace_end];
        rest = &rest[brace_end + 1..];

        // Parse command response
        let mut file = None;
        let mut output = None;
        let mut exit_code = 0;

        if let Some(start) = cmd_json.find("\"file\"") {
            file = extract_string_value(&cmd_json[start..]);
        }
        if let Some(start) = cmd_json.find("\"output\"") {
            output = extract_string_value(&cmd_json[start..]);
        }
        if let Some(start) = cmd_json.find("\"exit_code\"") {
            let after_key = &cmd_json[start + 11..];
            if let Some(colon) = after_key.find(':') {
                let value_str = after_key[colon + 1..].trim_start();
                // Parse integer until non-digit
                let num_str: String = value_str
                    .chars()
                    .take_while(|c| c.is_ascii_digit() || *c == '-')
                    .collect();
                if let Ok(n) = num_str.parse() {
                    exit_code = n;
                }
            }
        }

        commands.insert(
            name,
            CommandResponse {
                file,
                output,
                exit_code,
            },
        );
    }

    Ok(())
}
