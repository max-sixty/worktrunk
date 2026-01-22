//! Guard test to prevent stdout leaks in command code
//!
//! stdout is reserved for data output (e.g., JSON). User-visible messages go to stderr.
//! When shell integration is active (`WORKTRUNK_DIRECTIVE_FILE` is set), directives are written
//! to the file, not stdout.
//!
//! This test enforces: **No accidental stdout writes in command code**
//!
//! Allowed:
//! - `eprintln!` / `eprint!` (stderr is safe)
//! - `println!` / `print!` in files listed in `STDOUT_ALLOWED_PATHS` (data output)
//!
//! When adding stdout output:
//! - Use `worktrunk::styling::println` for color-aware output
//! - Add the file path to `STDOUT_ALLOWED_PATHS` with a comment explaining why

use std::fs;
use std::path::Path;

use path_slash::PathExt as _;

/// Paths (relative to src/commands/) that are allowed to use println!/print! for stdout.
/// These intentionally output data to stdout for scripting/piping.
const STDOUT_ALLOWED_PATHS: &[&str] = &[
    // Shell integration code for: eval "$(wt config shell init bash)"
    "init.rs",
    // Status line text for shell prompts (PS1)
    "statusline.rs",
    // Table and summary output for wt list
    "list/collect/mod.rs",
    // JSON output for wt list --format=json
    "list/mod.rs",
    // State data output (branch names, previous worktree, etc.)
    "config/state.rs",
    // Hint list output
    "config/hints.rs",
    // LLM prompt output for wt step commit --show-prompt
    "step_commands.rs",
];

/// Substrings that indicate the line is a special case (e.g., in a comment or test reference)
const ALLOWED_LINE_PATTERNS: &[&str] = &[
    "spacing_test.rs", // Test file reference
];

#[test]
fn check_no_stdout_in_commands() {
    let project_root = env!("CARGO_MANIFEST_DIR");
    let commands_dir = Path::new(project_root).join("src/commands");

    // Forbidden tokens that write to stdout
    let stdout_tokens = ["print!", "println!"];

    let mut violations = Vec::new();

    // Recursively scan all .rs files under src/commands/
    scan_directory(
        &commands_dir,
        &stdout_tokens,
        &mut violations,
        &commands_dir,
    );

    if !violations.is_empty() {
        panic!(
            "Unexpected stdout writes in command code:\n\n{}\n\n\
             stdout is reserved for data output (JSON, tables).\n\
             Use worktrunk::styling::println for stdout, eprintln for stderr.\n\
             Add file path to STDOUT_ALLOWED_PATHS if stdout is intentional.",
            violations.join("\n")
        );
    }
}

fn scan_directory(dir: &Path, tokens: &[&str], violations: &mut Vec<String>, commands_dir: &Path) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            scan_directory(&path, tokens, violations, commands_dir);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            check_file(&path, tokens, violations, commands_dir);
        }
    }
}

fn check_file(path: &Path, tokens: &[&str], violations: &mut Vec<String>, commands_dir: &Path) {
    // Get path relative to src/commands/ for matching against STDOUT_ALLOWED_PATHS
    let relative_path = path
        .strip_prefix(commands_dir)
        .map(|p| p.to_slash_lossy())
        .unwrap_or_default();

    // Skip files that are allowed to use stdout
    if STDOUT_ALLOWED_PATHS.contains(&relative_path.as_ref()) {
        return;
    }

    let contents = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return,
    };

    let relative_path = path
        .strip_prefix(env!("CARGO_MANIFEST_DIR"))
        .unwrap_or(path)
        .display();

    for (line_num, line) in contents.lines().enumerate() {
        // Skip lines with allowed patterns
        if ALLOWED_LINE_PATTERNS
            .iter()
            .any(|pattern| line.contains(pattern))
        {
            continue;
        }

        for token in tokens {
            if let Some(pos) = line.find(token) {
                // Skip eprint!/eprintln! - they go to stderr and are safe
                // When we match print!/println!, check if preceded by 'e' (part of eprint/eprintln)
                // Also verify the 'e' is at a word boundary (start of line, or after non-alphanumeric)
                if pos > 0 {
                    let prev_char = line.as_bytes()[pos - 1];
                    if prev_char == b'e' {
                        // Check this 'e' is at a word boundary (not part of some_eprint)
                        if pos == 1
                            || !line.as_bytes()[pos - 2].is_ascii_alphanumeric()
                                && line.as_bytes()[pos - 2] != b'_'
                        {
                            continue;
                        }
                    }
                }

                // Skip if the token is in a comment
                if let Some(comment_pos) = line.find("//")
                    && comment_pos < pos
                {
                    continue;
                }

                violations.push(format!(
                    "{}:{}: {}",
                    relative_path,
                    line_num + 1,
                    line.trim()
                ));
                break; // Only report once per line
            }
        }
    }
}
