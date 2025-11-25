//! Statusline output for shell prompts and editors.
//!
//! Outputs a single-line status for the current worktree:
//! `branch  status  ±working  commits  upstream  [ci]`
//!
//! This command reuses the data collection infrastructure from `wt list`,
//! avoiding duplication of git operations.

use anyhow::{Context, Result};
use std::env;
use std::io::{self, Read};
use std::path::Path;
use std::time::Duration;
use worktrunk::git::Repository;
use worktrunk::styling::print;

use super::list::{self, CollectOptions};

/// Claude Code context parsed from stdin JSON
struct ClaudeCodeContext {
    /// Working directory from `.workspace.current_dir`
    current_dir: String,
    /// Model name from `.model.display_name`
    model_name: Option<String>,
}

impl ClaudeCodeContext {
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

        if input.is_empty() {
            return None;
        }

        // Parse JSON
        let json: serde_json::Value = serde_json::from_str(&input).ok()?;

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
    if claude_code {
        output = format_directory_fish_style(&cwd.display().to_string());
    }

    // Git status
    let repo = Repository::at(&cwd);
    if repo.git_dir().is_ok()
        && let Some(status_line) = get_git_status(&repo, &cwd)?
    {
        if !output.is_empty() {
            output.push_str("  ");
        }
        output.push_str(&status_line);
    }

    // Model name (claude-code mode only)
    if let Some(model) = model_name {
        output.push_str("  | ");
        output.push_str(&model);
    }

    // Output with ANSI reset prefix
    if !output.is_empty() {
        if claude_code {
            let reset = anstyle::Reset;
            print!("{reset} {output}");
        } else {
            print!("{output}");
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
}
