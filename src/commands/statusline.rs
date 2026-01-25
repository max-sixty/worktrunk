//! Statusline output for shell prompts and editors.
//!
//! Outputs a single-line status for the current worktree:
//! `branch  status  ±working  commits  upstream  [ci]`
//!
//! This command reuses the data collection infrastructure from `wt list`,
//! avoiding duplication of git operations.

use std::env;
use std::io::{self, IsTerminal, Read};
use std::path::{Component, Path};

use ansi_str::AnsiStr;
use anyhow::{Context, Result};
use worktrunk::git::Repository;
use worktrunk::styling::{fix_dim_after_color_reset, get_terminal_width, truncate_visible};

use super::list::{self, CollectOptions, StatuslineSegment};

/// Claude Code context parsed from stdin JSON
struct ClaudeCodeContext {
    /// Working directory from `.workspace.current_dir`
    current_dir: String,
    /// Model name from `.model.display_name`
    model_name: Option<String>,
    /// Context window usage percentage from `.context_window.used_percentage`
    context_used_percentage: Option<f64>,
}

impl ClaudeCodeContext {
    /// Parse Claude Code context from a JSON string.
    /// Returns None if the string is empty or not valid JSON.
    fn parse(input: &str) -> Option<Self> {
        if input.is_empty() {
            return None;
        }

        let v: serde_json::Value = serde_json::from_str(input).ok()?;

        let current_dir = v
            .pointer("/workspace/current_dir")
            .and_then(|v| v.as_str())
            .unwrap_or(".")
            .to_string();

        let model_name = v
            .pointer("/model/display_name")
            .and_then(|v| v.as_str())
            .map(String::from);

        let context_used_percentage = v
            .pointer("/context_window/used_percentage")
            .and_then(|v| v.as_f64());

        Some(Self {
            current_dir,
            model_name,
            context_used_percentage,
        })
    }

    /// Try to read and parse Claude Code context from stdin.
    /// Returns None if stdin is a terminal or not valid JSON.
    fn from_stdin() -> Option<Self> {
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

/// Priority for directory segment (Claude Code only).
/// Highest priority - directory context is essential.
const PRIORITY_DIRECTORY: u8 = 0;

/// Priority for model name segment (Claude Code only).
/// Same as Branch - model identity is important.
const PRIORITY_MODEL: u8 = 1;

/// Priority for context gauge segment (Claude Code only).
/// Lower priority than model (higher number = dropped first when truncating).
const PRIORITY_CONTEXT: u8 = 2;

/// Format context usage as a moon phase gauge.
///
/// Uses moon phase emoji to show fill level (waning - gets darker as context fills).
/// Thresholds use exponential halving where each range is half the previous.
/// Formula: 5 buckets with ratio 16:8:4:2:1, normalized to 100% (sum = 31).
/// - 🌕 (0-51%) - full moon (plenty of room) - 16/31 ≈ 52%
/// - 🌔 (52-77%) - waning gibbous - 8/31 ≈ 26%
/// - 🌓 (78-90%) - last quarter - 4/31 ≈ 13%
/// - 🌒 (91-97%) - waning crescent - 2/31 ≈ 7%
/// - 🌑 (98-100%) - new moon (nearly full, warning) - 1/31 ≈ 3%
fn format_context_gauge(percentage: f64) -> String {
    // Clamp to valid range to handle edge cases (negative or >100%)
    let clamped = percentage.clamp(0.0, 100.0);
    let symbol = match clamped as u32 {
        0..=51 => '🌕',
        52..=77 => '🌔',
        78..=90 => '🌓',
        91..=97 => '🌒',
        _ => '🌑',
    };
    // Display the original percentage (not clamped) for transparency
    format!("{symbol} {:.0}%", percentage)
}

/// Run the statusline command.
///
/// Output uses `println!` for raw stdout (bypasses anstream color detection).
/// Shell prompts (PS1) and Claude Code always expect ANSI codes.
pub fn run(claude_code: bool) -> Result<()> {
    // Get context - either from stdin (claude-code mode) or current directory
    let (cwd, model_name, context_used_percentage) = if claude_code {
        let ctx = ClaudeCodeContext::from_stdin();
        let current_dir = ctx
            .as_ref()
            .map(|c| c.current_dir.clone())
            .unwrap_or_else(|| env::current_dir().unwrap_or_default().display().to_string());
        let model = ctx.as_ref().and_then(|c| c.model_name.clone());
        let context_pct = ctx.and_then(|c| c.context_used_percentage);
        (Path::new(&current_dir).to_path_buf(), model, context_pct)
    } else {
        (
            env::current_dir().context("Failed to get current directory")?,
            None,
            None,
        )
    };

    // Build segments with priorities
    let mut segments: Vec<StatuslineSegment> = Vec::new();

    // Directory (claude-code mode only) - priority 0
    let dir_str = if claude_code {
        let formatted = format_directory_fish_style(&cwd);
        // Only push non-empty directory segments (empty can happen if cwd is ".")
        if !formatted.is_empty() {
            segments.push(StatuslineSegment::new(
                formatted.clone(),
                PRIORITY_DIRECTORY,
            ));
        }
        Some(formatted)
    } else {
        None
    };

    // Git status segments (skip links in claude-code mode - OSC 8 not supported)
    if let Ok(repo) = Repository::current()
        && repo.worktree_at(&cwd).git_dir().is_ok()
    {
        let git_segments = get_git_status_segments(&repo, &cwd, !claude_code)?;

        // In claude-code mode, skip branch segment if directory matches worktrunk template
        let git_segments = if let Some(ref dir) = dir_str {
            filter_redundant_branch(git_segments, dir)
        } else {
            git_segments
        };

        segments.extend(git_segments);
    }

    // Model name (claude-code mode only) - priority 1 (same as Branch)
    if let Some(model) = model_name {
        // Use "| " prefix to visually separate from git status
        segments.push(StatuslineSegment::new(format!("| {model}"), PRIORITY_MODEL));
    }

    // Context gauge (claude-code mode only) - priority 2 (placed after model)
    if let Some(pct) = context_used_percentage {
        segments.push(StatuslineSegment::new(
            format_context_gauge(pct),
            PRIORITY_CONTEXT,
        ));
    }

    if segments.is_empty() {
        return Ok(());
    }

    // Fit segments to terminal width using priority-based dropping
    let max_width = get_terminal_width();
    // Reserve 1 char for leading space (ellipsis handled by truncate_visible fallback)
    let content_budget = max_width.saturating_sub(1);
    let fitted_segments = StatuslineSegment::fit_to_width(segments, content_budget);

    // Join and apply final truncation as fallback
    let output = StatuslineSegment::join(&fitted_segments);

    let reset = anstyle::Reset;
    let output = fix_dim_after_color_reset(&output);
    let output = truncate_visible(&format!("{reset} {output}"), max_width);

    println!("{}", output);

    Ok(())
}

/// Filter out branch segment if directory already shows it via worktrunk template.
fn filter_redundant_branch(segments: Vec<StatuslineSegment>, dir: &str) -> Vec<StatuslineSegment> {
    use super::list::columns::ColumnKind;

    // Find the branch segment by its column kind (not priority, which could be shared)
    if let Some(branch_seg) = segments.iter().find(|s| s.kind == Some(ColumnKind::Branch)) {
        // Strip ANSI codes in case branch becomes styled in future
        let raw_branch = branch_seg.content.ansi_strip();
        // Normalize branch name for comparison (slashes become dashes in paths)
        let normalized_branch = worktrunk::config::sanitize_branch_name(&raw_branch);
        let pattern = format!(".{normalized_branch}");

        if dir.ends_with(&pattern) {
            // Directory already shows branch via worktrunk template, skip branch segment
            return segments
                .into_iter()
                .filter(|s| s.kind != Some(ColumnKind::Branch))
                .collect();
        }
    }

    segments
}

/// Get git status as prioritized segments for the current worktree.
///
/// When `include_links` is true, CI status includes clickable OSC 8 hyperlinks.
fn get_git_status_segments(
    repo: &Repository,
    cwd: &Path,
    include_links: bool,
) -> Result<Vec<StatuslineSegment>> {
    use super::list::columns::ColumnKind;

    // Get current worktree info
    let worktrees = repo.list_worktrees()?;
    let current_worktree = worktrees.iter().find(|wt| cwd.starts_with(&wt.path));

    let Some(wt) = current_worktree else {
        // Not in a worktree - just show branch name as a segment
        if let Ok(Some(branch)) = repo.current_worktree().branch() {
            return Ok(vec![StatuslineSegment::from_column(
                branch.to_string(),
                ColumnKind::Branch,
            )]);
        }
        return Ok(vec![]);
    };

    // If we can't determine the default branch, just show current branch
    if repo.default_branch().is_none() {
        return Ok(vec![StatuslineSegment::from_column(
            wt.branch.as_deref().unwrap_or("HEAD").to_string(),
            ColumnKind::Branch,
        )]);
    }

    // Determine if this is the primary worktree
    // - Normal repos: the main worktree (repo root)
    // - Bare repos: the default branch's worktree
    let is_home = repo
        .primary_worktree()
        .ok()
        .flatten()
        .is_some_and(|p| wt.path == p);

    // Build item with identity fields
    let mut item = list::build_worktree_item(wt, is_home, true, false);

    // Load URL template from project config (if configured)
    let url_template = repo.url_template();

    // Build collect options with URL template
    let options = CollectOptions {
        url_template,
        ..Default::default()
    };

    // Populate computed fields (parallel git operations)
    // Compute everything (same as --full) for complete status symbols
    list::populate_item(repo, &mut item, options)?;

    // Get prioritized segments
    let segments = item.format_statusline_segments(include_links);

    if segments.is_empty() {
        // Fallback: just show branch name
        Ok(vec![StatuslineSegment::from_column(
            wt.branch.as_deref().unwrap_or("HEAD").to_string(),
            ColumnKind::Branch,
        )])
    } else {
        Ok(segments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn test_statusline_truncation() {
        use color_print::cformat;

        // Simulate a long statusline with styled content
        let long_line =
            cformat!("main  <cyan>?</><dim>^</>  http://very-long-branch-name.localhost:3000");

        // Truncate to 30 visible characters
        let truncated = truncate_visible(&long_line, 30);

        // Should end with ellipsis and be shorter
        assert!(
            truncated.contains('…'),
            "Truncated line should contain ellipsis: {truncated}"
        );

        // Visible width should be <= 30
        let visible: String = truncated
            .chars()
            .filter(|c| !c.is_ascii_control())
            .collect();
        // Simple check: the truncated output should be shorter than original
        let original_visible: String = long_line
            .chars()
            .filter(|c| !c.is_ascii_control())
            .collect();
        assert!(
            visible.len() < original_visible.len(),
            "Truncated should be shorter: {} vs {}",
            visible.len(),
            original_visible.len()
        );
    }

    #[test]
    fn test_context_gauge_formatting() {
        // Test boundary values for each moon phase symbol (waning - darker as context fills)
        // Thresholds use exponential halving: ratio 16:8:4:2:1, normalized to 100%
        assert_eq!(format_context_gauge(0.0), "🌕 0%");
        assert_eq!(format_context_gauge(51.0), "🌕 51%");
        assert_eq!(format_context_gauge(52.0), "🌔 52%");
        assert_eq!(format_context_gauge(77.0), "🌔 77%");
        assert_eq!(format_context_gauge(78.0), "🌓 78%");
        assert_eq!(format_context_gauge(90.0), "🌓 90%");
        assert_eq!(format_context_gauge(91.0), "🌒 91%");
        assert_eq!(format_context_gauge(97.0), "🌒 97%");
        assert_eq!(format_context_gauge(98.0), "🌑 98%");
        assert_eq!(format_context_gauge(100.0), "🌑 100%");
    }

    #[test]
    fn test_context_gauge_fractional_percentages() {
        // Fractional values are rounded (per {:.0} format specifier)
        // Rust uses banker's rounding (round half to even)
        assert_eq!(format_context_gauge(42.7), "🌕 43%"); // 43% is in 0-51% range
        assert_eq!(format_context_gauge(0.4), "🌕 0%");
        assert_eq!(format_context_gauge(0.5), "🌕 0%"); // banker's rounding: 0.5 rounds to even (0)
        assert_eq!(format_context_gauge(1.5), "🌕 2%"); // banker's rounding: 1.5 rounds to even (2)
        assert_eq!(format_context_gauge(99.9), "🌑 100%");
    }

    #[test]
    fn test_context_gauge_edge_cases() {
        // Negative values: symbol clamps to bright (low usage), but display shows original value
        assert_eq!(format_context_gauge(-5.0), "🌕 -5%");
        assert_eq!(format_context_gauge(-0.1), "🌕 -0%"); // rounds to -0%

        // Values over 100%: symbol clamps to dark (high usage), but display shows original value
        assert_eq!(format_context_gauge(105.0), "🌑 105%");
        assert_eq!(format_context_gauge(150.0), "🌑 150%");
    }

    #[test]
    fn test_claude_code_context_parse_with_context_window() {
        let json = r#"{
            "workspace": {"current_dir": "/tmp/test"},
            "model": {"display_name": "Opus"},
            "context_window": {"used_percentage": 42.5}
        }"#;

        let ctx = ClaudeCodeContext::parse(json).expect("should parse");
        assert_eq!(ctx.current_dir, "/tmp/test");
        assert_eq!(ctx.model_name, Some("Opus".to_string()));
        assert_eq!(ctx.context_used_percentage, Some(42.5));
    }

    #[test]
    fn test_claude_code_context_parse_missing_context_window() {
        // context_window is optional
        let json = r#"{
            "workspace": {"current_dir": "/tmp/test"},
            "model": {"display_name": "Opus"}
        }"#;

        let ctx = ClaudeCodeContext::parse(json).expect("should parse");
        assert_eq!(ctx.context_used_percentage, None);
    }

    #[test]
    fn test_claude_code_context_parse_context_window_missing_percentage() {
        // context_window can exist without used_percentage
        let json = r#"{
            "workspace": {"current_dir": "/tmp/test"},
            "context_window": {}
        }"#;

        let ctx = ClaudeCodeContext::parse(json).expect("should parse");
        assert_eq!(ctx.context_used_percentage, None);
    }
}
