//! Guard test to prevent stdout leaks in command code
//!
//! stdout carries the command's answer — content the user may want to pipe,
//! redirect, or capture: data (JSON, tables), rendered views (`wt config show`,
//! `wt hook show`), `--dry-run` previews (the whole answer when nothing
//! mutates), and shell integration output. Interactive status, progress, and
//! errors go to stderr. When shell integration is active (directive env vars
//! set), directives are written to files, not stdout.
//!
//! This test enforces: **No accidental stdout writes in command code**
//!
//! Allowed:
//! - `eprintln!` / `eprint!` (stderr is safe)
//! - `println!` / `print!` in files listed in `STDOUT_ALLOWED_PATHS`
//!
//! When adding stdout output:
//! - Use `worktrunk::styling::println` for color-aware output, or
//!   `crate::output::println_verbatim` where the consumer renders the escapes
//!   and is never a tty (the statusline, `--help-page`)
//! - Add the file path to `STDOUT_ALLOWED_PATHS` with a comment explaining why
//!
//! Neither printer panics when the consumer closes the pipe, which
//! [`test_stdout_surfaces_survive_a_closed_consumer`] checks from the outside.

use std::fs;
use std::path::Path;
use std::process::Stdio;

use path_slash::PathExt as _;
use rstest::rstest;

use crate::common::{TestRepo, repo};

/// Paths (relative to src/commands/) that are allowed to use println!/print! for stdout.
/// These intentionally output data to stdout for scripting/piping.
const STDOUT_ALLOWED_PATHS: &[&str] = &[
    // Shell integration code for: eval "$(wt config shell init bash)"
    "init.rs",
    // Table and summary output for wt list
    "list/collect/mod.rs",
    // State data output (branch names, previous worktree, etc.)
    "config/state.rs",
    // Hint list output
    "config/hints.rs",
    // Alias introspection output (show / dry-run), intended to be pipeable
    "config/alias.rs",
    // Template evaluation output for scripting
    "eval.rs",
    // LLM prompt output for wt step commit --show-prompt and squash --show-prompt
    "step/commit.rs",
    "step/squash.rs",
    // wt step copy-ignored dry-run plan (human preview + --format=json)
    "step/copy_ignored.rs",
    // wt step prune dry-run plan (human preview + --format=json)
    "step/prune.rs",
    // wt step relocate dry-run human preview (show_dry_run_preview)
    "relocate.rs",
    // wt config shell install/uninstall --dry-run preview (the interactive
    // `?` re-preview still goes to stderr)
    "configure_shell.rs",
    // JSON output for wt switch --format=json
    "worktree/switch.rs",
    // Migrated TOML output for wt config update --print (pipeable)
    "config/update.rs",
    // Hook listing for wt hook show (paged), and the wt hook --dry-run preview
    "hook_commands.rs",
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

/// Every stdout surface exits cleanly when its consumer stops reading.
///
/// std's `print!`/`println!` panic on a `BrokenPipe`, so a command whose
/// answer went out through them died with exit 101 and a Rust panic message
/// the moment the reader left — `wt list | head -3`, and `wt list statusline`
/// on the surface a shell prompt runs on every redraw. anstream's macros and
/// `println_verbatim!` drop that error instead, and one of the two now carries
/// every answer this binary writes to stdout.
///
/// Which of the two a surface uses is about color, not about this, so the
/// cases below span both.
///
/// The child's stdout pipe is closed before it is waited on, so its first
/// write has no reader. `--version` writes a few dozen bytes and still trips
/// it: `EPIPE` is about whether a reader is attached, not about filling the
/// pipe buffer.
#[rstest]
fn test_stdout_surfaces_survive_a_closed_consumer(repo: TestRepo) {
    for args in [
        &["--version"][..],
        &["merge", "--help-page"][..],
        &["merge", "--help-description"][..],
        &["merge", "--help-md"][..],
        &["list"][..],
        &["list", "--full"][..],
        &["list", "statusline"][..],
        &["list", "--format=json"][..],
        &["config", "update", "--print"][..],
    ] {
        let mut command = repo.wt_command();
        let mut child = command
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("failed to spawn wt {args:?}: {e}"));

        drop(child.stdout.take().expect("stdout was piped"));

        let output = child
            .wait_with_output()
            .unwrap_or_else(|e| panic!("failed to wait for wt {args:?}: {e}"));
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "wt {args:?} exited {:?} with its consumer gone; stderr: {stderr}",
            output.status.code()
        );
        assert!(
            !stderr.contains("panicked"),
            "wt {args:?} panicked with its consumer gone; stderr: {stderr}"
        );
    }
}
