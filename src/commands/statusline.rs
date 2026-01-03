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
use worktrunk::git::Repository;

use super::list::{self, CollectOptions};

#[derive(serde::Deserialize, Default)]
struct ClaudeCodeContextJson {
    #[serde(default)]
    workspace: ClaudeCodeWorkspace,
    #[serde(default)]
    model: ClaudeCodeModel,
}

#[derive(serde::Deserialize, Default)]
struct ClaudeCodeWorkspace {
    current_dir: Option<String>,
}

#[derive(serde::Deserialize, Default)]
struct ClaudeCodeModel {
    display_name: Option<String>,
}

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

        let json: ClaudeCodeContextJson = serde_json::from_str(input).ok()?;
        let current_dir = json
            .workspace
            .current_dir
            .unwrap_or_else(|| ".".to_string());
        let model_name = json.model.display_name;

        Some(Self {
            current_dir,
            model_name,
        })
    }

    /// Try to read and parse Claude Code context from stdin.
    /// Returns None if stdin is a terminal or not valid JSON.
    fn from_stdin() -> Option<Self> {
        use std::io::IsTerminal;

        if io::stdin().is_terminal() {
            return None;
        }

        let mut input = String::new();
        io::stdin().read_to_string(&mut input).ok()?;
        Self::parse(&input)
    }
}

/// Format a directory path in fish-style (abbreviated parent directories).
///
/// Examples:
/// - `/home/user/workspace/project` -> `~/w/project`
/// - `/home/user` -> `~`
/// - `/tmp/test` -> `/t/test`
fn format_directory_fish_style(path: &Path) -> String {
    use std::path::Component;

    // Replace home directory prefix with ~
    let (suffix, tilde_prefix) = worktrunk::path::home_dir()
        .and_then(|home| path.strip_prefix(&home).ok().map(|s| (s, true)))
        .unwrap_or((path, false));

    // Collect normal components (skip RootDir, CurDir, etc.)
    let components: Vec<_> = suffix
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy()),
            _ => None,
        })
        .collect();

    // Build result: ~/a/b/last or /a/b/last
    let abbreviated = components
        .iter()
        .enumerate()
        .map(|(i, s)| {
            if i == components.len() - 1 {
                s.to_string() // Keep last component full
            } else {
                s.chars().next().map(String::from).unwrap_or_default()
            }
        })
        .collect::<Vec<_>>();

    match (tilde_prefix, abbreviated.is_empty()) {
        (true, true) => "~".to_string(),
        (true, false) => format!("~/{}", abbreviated.join("/")),
        (false, _) if path.is_absolute() => format!("/{}", abbreviated.join("/")),
        (false, _) => abbreviated.join("/"),
    }
}

/// Run the statusline command.
///
/// Output uses `output::stdout()` for raw stdout (bypasses anstream color detection).
/// Shell prompts (PS1) and Claude Code always expect ANSI codes.
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
        let formatted = format_directory_fish_style(&cwd);
        output = formatted.clone();
        Some(formatted)
    } else {
        None
    };

    // Git status (skip links in claude-code mode - OSC 8 not supported)
    let repo = Repository::at(&cwd);
    if repo.git_dir().is_ok()
        && let Some(status_line) = get_git_status(&repo, &cwd, !claude_code)?
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

    // Output via data() - shell prompts and Claude Code always expect ANSI codes
    if !output.is_empty() {
        use worktrunk::styling::fix_dim_after_color_reset;
        let reset = anstyle::Reset;
        let output = fix_dim_after_color_reset(&output);
        output::stdout(format!("{reset} {output}"))?;
    }

    Ok(())
}

/// Get git status line for the current worktree
///
/// When `include_links` is true, CI status includes clickable OSC 8 hyperlinks.
fn get_git_status(repo: &Repository, cwd: &Path, include_links: bool) -> Result<Option<String>> {
    // Get current worktree info
    let worktrees = repo.list_worktrees()?;
    let current_worktree = worktrees.iter().find(|wt| cwd.starts_with(&wt.path));

    let Some(wt) = current_worktree else {
        // Not in a worktree - just show branch name
        if let Ok(Some(branch)) = repo.current_branch() {
            return Ok(Some(branch.to_string()));
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
    // Effective target for integration checks: upstream if ahead of local, else local.
    let integration_target = repo.effective_integration_target(&default_branch);

    // Determine if this is the main worktree
    let main_worktree = worktrees
        .iter()
        .find(|w| w.branch.as_deref() == Some(default_branch.as_str()))
        .unwrap_or(&worktrees[0]);
    let is_main = wt.path == main_worktree.path;

    // Build item with identity fields
    let mut item = list::build_worktree_item(wt, is_main, true, false);

    // Populate computed fields (parallel git operations) and format status_line
    // Compute everything (same as --full) for complete status symbols
    // Pass default_branch for stable informational stats,
    // and integration_target for integration status checks.
    list::populate_item(
        &mut item,
        &default_branch,
        &integration_target,
        CollectOptions::default(),
    )?;

    // Format statusline with link control
    let statusline = item.format_statusline_with_options(include_links);
    if statusline.is_empty() {
        // Fallback: just show branch name
        Ok(Some(wt.branch.as_deref().unwrap_or("HEAD").to_string()))
    } else {
        Ok(Some(statusline))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_format_directory_fish_style() {
        // Test absolute paths (Unix-style paths only meaningful on Unix)
        #[cfg(unix)]
        {
            assert_eq!(
                format_directory_fish_style(Path::new("/tmp/test")),
                "/t/test"
            );
            assert_eq!(format_directory_fish_style(Path::new("/")), "/");
            assert_eq!(
                format_directory_fish_style(Path::new("/var/log/app")),
                "/v/l/app"
            );
        }

        // Test with actual HOME (if set)
        if let Ok(home) = env::var("HOME") {
            // Basic home substitution
            let test_path = format!("{home}/workspace/project");
            let result = format_directory_fish_style(Path::new(&test_path));
            assert!(result.starts_with("~/"), "Expected ~ prefix, got: {result}");
            assert!(
                result.ends_with("/project"),
                "Expected /project suffix, got: {result}"
            );

            // Exact HOME path should become just ~
            assert_eq!(format_directory_fish_style(Path::new(&home)), "~");

            // Path that shares HOME as string prefix but not as path component
            // e.g., /home/user vs /home/usered/nested
            let path_outside_home = format!("{home}ed/nested");
            let result = format_directory_fish_style(Path::new(&path_outside_home));
            assert!(
                !result.starts_with("~"),
                "Path sharing HOME string prefix should not use ~: {result}"
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
