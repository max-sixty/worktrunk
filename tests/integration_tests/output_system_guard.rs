//! Guard test to prevent stdout leaks in command code
//!
//! In directive mode (`--internal`), stdout is reserved for shell script output.
//! Any `println!` or `print!` in command code would leak into the script and
//! potentially be eval'd by the shell wrapper.
//!
//! This test enforces: **No stdout writes in command code except via output system**
//!
//! Allowed:
//! - `output::*` functions (route to correct stream based on mode)
//! - `eprintln!` / `eprint!` (stderr is safe)
//!
//! Exceptions:
//! - `init.rs` - intentionally outputs shell code to stdout for `eval`

use std::fs;
use std::path::Path;

/// Files that are allowed to use println!/print! for stdout
/// These intentionally output to stdout (e.g., shell code for eval)
const STDOUT_ALLOWED_FILES: &[&str] = &[
    "init.rs",       // Outputs shell integration code for: eval "$(wt config shell init bash)"
    "statusline.rs", // Outputs status line text for shell prompts (PS1)
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
    scan_directory(&commands_dir, &stdout_tokens, &mut violations);

    if !violations.is_empty() {
        panic!(
            "stdout writes found in command code (use output::* instead):\n\n{}\n\n\
             In directive mode, stdout is reserved for shell script output.\n\
             Use output::info(), output::hint(), output::data(), etc. instead.",
            violations.join("\n")
        );
    }
}

fn scan_directory(dir: &Path, tokens: &[&str], violations: &mut Vec<String>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            scan_directory(&path, tokens, violations);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            check_file(&path, tokens, violations);
        }
    }
}

fn check_file(path: &Path, tokens: &[&str], violations: &mut Vec<String>) {
    let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");

    // Skip files that are allowed to use stdout
    if STDOUT_ALLOWED_FILES.contains(&file_name) {
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
