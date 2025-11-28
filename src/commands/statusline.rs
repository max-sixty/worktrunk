//! Statusline output for shell prompts and editors.
//!
//! Outputs a single-line status for the current worktree:
//! `branch  status  ±working  commits  upstream  [ci]`
//!
//! This command reuses the data collection infrastructure from `wt list`,
//! avoiding duplication of git operations.

use crate::output;
use anyhow::{Context, Result};
use std::env;
use std::io::{self, Read};
use std::path::Path;
use std::time::Duration;
use worktrunk::git::Repository;

use super::list::{self, CollectOptions};

/// Claude Code context parsed from stdin JSON
struct ClaudeCodeContext {
    /// Working directory from `.workspace.current_dir`
    current_dir: String,
    /// Model name from `.model.display_name`
    model_name: Option<String>,
}

impl ClaudeCodeContext {
    /// Parse Claude Code context from a JSON string.
    /// Returns None if the string is empty or not valid JSON.
    fn parse(input: &str) -> Option<Self> {
        if input.is_empty() {
            return None;
        }

        // Parse JSON
        let json: serde_json::Value = serde_json::from_str(input).ok()?;

        let current_dir = json
            .get("workspace")
            .and_then(|w| w.get("current_dir"))
            .and_then(|d| d.as_str())
            .unwrap_or(".")
            .to_string();

        let model_name = json
            .get("model")
            .and_then(|m| m.get("display_name"))
            .and_then(|n| n.as_str())
            .map(|s| s.to_string());

        Some(Self {
            current_dir,
            model_name,
        })
    }

    /// Try to read and parse Claude Code context from stdin.
    /// Returns None if stdin is empty or not valid JSON.
    fn from_stdin() -> Option<Self> {
        // Non-blocking read with timeout
        // Claude Code pipes JSON to stdin, but we shouldn't block if nothing is there
        use std::sync::mpsc;
        use std::thread;

        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            let mut input = String::new();
            let _ = io::stdin().read_to_string(&mut input);
            let _ = tx.send(input);
        });

        // Wait up to 10ms for stdin
        let input = rx.recv_timeout(Duration::from_millis(10)).ok()?;

        Self::parse(&input)
    }
}

/// Format a directory path in fish-style (abbreviated parent directories).
///
/// Examples:
/// - `/home/user/workspace/project` -> `~/w/project`
/// - `/home/user` -> `~`
/// - `/tmp/test` -> `/t/test`
fn format_directory_fish_style(path: &str) -> String {
    let home = env::var("HOME").unwrap_or_default();

    // Replace home with ~
    let path = if !home.is_empty() && path.starts_with(&home) {
        format!("~{}", &path[home.len()..])
    } else {
        path.to_string()
    };

    let is_absolute = path.starts_with('/');
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();

    if parts.is_empty() {
        return path;
    }

    let mut result = String::new();
    if is_absolute {
        result.push('/');
    }

    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            result.push('/');
        }

        // Keep ~ and last component full, abbreviate others to first char
        if (i == 0 && *part == "~") || i == parts.len() - 1 {
            result.push_str(part);
        } else if let Some(c) = part.chars().next() {
            result.push(c);
        }
    }

    result
}

/// Run the statusline command.
///
/// Uses the output system like other commands:
/// - Interactive mode: output goes to stdout
/// - Directive mode: output goes to stderr, keeping stdout clean for shell directives
pub fn run(claude_code: bool) -> Result<()> {
    // Get context - either from stdin (claude-code mode) or current directory
    let (cwd, model_name) = if claude_code {
        let ctx = ClaudeCodeContext::from_stdin();
        let current_dir = ctx
            .as_ref()
            .map(|c| c.current_dir.clone())
            .unwrap_or_else(|| env::current_dir().unwrap_or_default().display().to_string());
        let model = ctx.and_then(|c| c.model_name);
        (Path::new(&current_dir).to_path_buf(), model)
    } else {
        (
            env::current_dir().context("Failed to get current directory")?,
            None,
        )
    };

    // Build output string
    let mut output = String::new();

    // Directory (claude-code mode only)
    let dir_str = if claude_code {
        let formatted = format_directory_fish_style(&cwd.display().to_string());
        output = formatted.clone();
        Some(formatted)
    } else {
        None
    };

    // Git status
    let repo = Repository::at(&cwd);
    if repo.git_dir().is_ok()
        && let Some(status_line) = get_git_status(&repo, &cwd)?
    {
        // In claude-code mode, skip branch name if directory matches worktrunk template
        // TODO: Use actual configured template from config instead of hardcoding ".{branch}"
        // Template: {repo}.{branch} - so directory should end with ".{branch}"
        let status_to_show = if let Some(ref dir) = dir_str {
            // status_line format: "branch  rest..." - check if dir ends with .branch
            if let Some((branch, rest)) = status_line.split_once("  ") {
                // Normalize branch name for comparison (slashes become dashes in paths)
                let normalized_branch = worktrunk::config::sanitize_branch_name(branch);
                let pattern = format!(".{normalized_branch}");
                if dir.ends_with(&pattern) {
                    // Directory already shows branch via worktrunk template, skip it
                    rest.to_string()
                } else {
                    status_line
                }
            } else {
                status_line
            }
        } else {
            status_line
        };

        if !status_to_show.is_empty() {
            if !output.is_empty() {
                output.push_str("  ");
            }
            output.push_str(&status_to_show);
        }
    }

    // Model name (claude-code mode only)
    if let Some(model) = model_name {
        output.push_str("  | ");
        output.push_str(&model);
    }

    // Output via output system (goes to stderr, like `wt list`)
    if !output.is_empty() {
        if claude_code {
            use std::io::Write;
            let reset = anstyle::Reset;
            // Bypass anstream - write directly to stdout to preserve ANSI codes
            // regardless of TTY detection (Claude Code expects raw ANSI)
            writeln!(std::io::stdout(), "{reset} {output}")?;
            std::io::stdout().flush()?;
        } else {
            output::data(output)?;
        }
    }

    Ok(())
}

/// Get git status line for the current worktree
fn get_git_status(repo: &Repository, cwd: &Path) -> Result<Option<String>> {
    // Get current worktree info
    let worktrees = repo.list_worktrees()?;
    let current_worktree = worktrees
        .worktrees
        .iter()
        .find(|wt| cwd.starts_with(&wt.path));

    let Some(wt) = current_worktree else {
        // Not in a worktree - just show branch name
        if let Ok(Some(branch)) = repo.current_branch() {
            return Ok(Some(branch));
        }
        return Ok(None);
    };

    // Get default branch for comparisons
    let default_branch = match repo.default_branch() {
        Ok(b) => b,
        Err(_) => {
            // Can't determine default branch - just show current branch
            return Ok(Some(wt.branch.as_deref().unwrap_or("HEAD").to_string()));
        }
    };

    // Determine if this is the main worktree
    let main_worktree = worktrees
        .worktrees
        .iter()
        .find(|w| w.branch.as_deref() == Some(default_branch.as_str()))
        .unwrap_or_else(|| worktrees.main());
    let is_main = wt.path == main_worktree.path;

    // Build item with identity fields
    let mut items = vec![list::build_worktree_item(wt, is_main, true, false)];

    // Populate computed fields (parallel git operations) and format status_line
    list::populate_items(
        &mut items,
        &default_branch,
        CollectOptions {
            fetch_ci: true,
            check_merge_tree_conflicts: false,
        },
    )?;

    // Return the pre-formatted status line
    if let Some(ref status_line) = items[0].display.status_line {
        Ok(Some(status_line.clone()))
    } else {
        // Fallback: just show branch name
        Ok(Some(wt.branch.as_deref().unwrap_or("HEAD").to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_directory_fish_style() {
        // Test paths that don't depend on HOME
        assert_eq!(format_directory_fish_style("/tmp/test"), "/t/test");
        assert_eq!(format_directory_fish_style("/"), "/");
        assert_eq!(format_directory_fish_style("/var/log/app"), "/v/l/app");

        // Test with actual HOME (if set)
        if let Ok(home) = env::var("HOME") {
            let test_path = format!("{home}/workspace/project");
            let result = format_directory_fish_style(&test_path);
            assert!(result.starts_with("~/"), "Expected ~ prefix, got: {result}");
            assert!(
                result.ends_with("/project"),
                "Expected /project suffix, got: {result}"
            );
        }
    }

    #[test]
    fn test_claude_code_context_parse_full() {
        // Full Claude Code context JSON (as documented)
        let json = r#"{
            "hook_event_name": "Status",
            "session_id": "abc123",
            "cwd": "/current/working/directory",
            "model": {
                "id": "claude-opus-4-1",
                "display_name": "Opus"
            },
            "workspace": {
                "current_dir": "/home/user/project",
                "project_dir": "/home/user/project"
            },
            "version": "1.0.80"
        }"#;

        let ctx = ClaudeCodeContext::parse(json).expect("should parse");
        assert_eq!(ctx.current_dir, "/home/user/project");
        assert_eq!(ctx.model_name, Some("Opus".to_string()));
    }

    #[test]
    fn test_claude_code_context_parse_minimal() {
        // Minimal JSON with just the fields we need
        let json = r#"{
            "workspace": {"current_dir": "/tmp/test"},
            "model": {"display_name": "Haiku"}
        }"#;

        let ctx = ClaudeCodeContext::parse(json).expect("should parse");
        assert_eq!(ctx.current_dir, "/tmp/test");
        assert_eq!(ctx.model_name, Some("Haiku".to_string()));
    }

    #[test]
    fn test_claude_code_context_parse_missing_model() {
        // Model is optional
        let json = r#"{"workspace": {"current_dir": "/tmp/test"}}"#;

        let ctx = ClaudeCodeContext::parse(json).expect("should parse");
        assert_eq!(ctx.current_dir, "/tmp/test");
        assert_eq!(ctx.model_name, None);
    }

    #[test]
    fn test_claude_code_context_parse_missing_workspace() {
        // Missing workspace defaults to "."
        let json = r#"{"model": {"display_name": "Sonnet"}}"#;

        let ctx = ClaudeCodeContext::parse(json).expect("should parse");
        assert_eq!(ctx.current_dir, ".");
        assert_eq!(ctx.model_name, Some("Sonnet".to_string()));
    }

    #[test]
    fn test_claude_code_context_parse_empty() {
        assert!(ClaudeCodeContext::parse("").is_none());
    }

    #[test]
    fn test_claude_code_context_parse_invalid_json() {
        assert!(ClaudeCodeContext::parse("not json").is_none());
        assert!(ClaudeCodeContext::parse("{invalid}").is_none());
    }

    #[test]
    fn test_branch_deduplication_with_slashes() {
        // Simulate the actual scenario:
        // - Directory: ~/w/insta.claude-fix-snapshot-merge-conflicts-xyz
        // - Branch: claude/fix-snapshot-merge-conflicts-xyz
        let dir = "~/w/insta.claude-fix-snapshot-merge-conflicts-xyz";
        let branch = "claude/fix-snapshot-merge-conflicts-xyz";

        let normalized_branch = worktrunk::config::sanitize_branch_name(branch);
        let pattern = format!(".{normalized_branch}");

        assert!(
            dir.ends_with(&pattern),
            "Directory '{}' should end with pattern '{}' (normalized from branch '{}')",
            dir,
            pattern,
            branch
        );
    }
}
