use skim::prelude::*;
use std::borrow::Cow;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use worktrunk::config::WorktrunkConfig;
use worktrunk::git::{GitError, GitResultExt, Repository};

use super::list::model::{ListItem, gather_list_data};
use super::worktree::handle_switch;
use crate::output::handle_switch_output;

/// Preview modes for the interactive selector
///
/// Each mode shows a different aspect of the worktree:
/// 1. WorkingTree: Uncommitted changes (git diff HEAD --stat)
/// 2. History: Commit history since diverging from main (git log with merge-base)
/// 3. BranchDiff: Line diffs in commits ahead of main (git diff --stat main…)
///
/// Loosely aligned with `wt list` columns, though not a perfect match:
/// - Mode 1 corresponds to "HEAD±" column
/// - Mode 2 shows commits (related to "main↕" counts)
/// - Mode 3 corresponds to "main…± (--full)" column
///
/// Note: Order of modes 2 & 3 could potentially be swapped
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewMode {
    WorkingTree = 1,
    History = 2,
    BranchDiff = 3,
}

impl PreviewMode {
    fn from_u8(n: u8) -> Self {
        match n {
            2 => Self::History,
            3 => Self::BranchDiff,
            _ => Self::WorkingTree,
        }
    }

    fn read_from_state() -> Self {
        let state_path = Self::state_path();
        fs::read_to_string(&state_path)
            .ok()
            .and_then(|s| s.trim().parse::<u8>().ok())
            .map(Self::from_u8)
            .unwrap_or(Self::WorkingTree)
    }

    fn state_path() -> PathBuf {
        // Use per-process temp file to avoid race conditions when running multiple instances
        std::env::temp_dir().join(format!("wt-select-mode-{}", std::process::id()))
    }
}

/// RAII wrapper for preview state file lifecycle management
struct PreviewState {
    path: PathBuf,
}

impl PreviewState {
    fn new() -> Self {
        let path = PreviewMode::state_path();
        let _ = fs::write(&path, "1");
        Self { path }
    }
}

impl Drop for PreviewState {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Wrapper to implement SkimItem for ListItem
struct WorktreeSkimItem {
    display_text: String,
    branch_name: String,
    item: Arc<ListItem>,
}

impl SkimItem for WorktreeSkimItem {
    fn text(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.display_text)
    }

    fn output(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.branch_name)
    }

    fn preview(&self, _context: PreviewContext<'_>) -> ItemPreview {
        let mode = PreviewMode::read_from_state();
        let preview_text = match mode {
            PreviewMode::WorkingTree => self.render_working_tree_preview(),
            PreviewMode::History => self.render_history_preview(),
            PreviewMode::BranchDiff => self.render_branch_diff_preview(),
        };

        ItemPreview::AnsiText(preview_text)
    }
}

impl WorktreeSkimItem {
    /// Common diff rendering pattern: check stat, show stat + full diff if non-empty
    fn render_diff_preview(&self, args: &[&str], no_changes_msg: &str) -> String {
        let mut output = String::new();
        let repo = Repository::current();

        // Check stat output first
        let mut stat_args = args.to_vec();
        stat_args.push("--stat");

        if let Ok(stat) = repo.run_command(&stat_args)
            && !stat.trim().is_empty()
        {
            output.push_str(&stat);
            output.push_str("\n\n");

            // Show full diff with renderer
            if let Ok(diff) = repo.run_diff_with_pager(args) {
                output.push_str(&diff);
            }
        } else {
            output.push_str(no_changes_msg);
            output.push('\n');
        }

        output
    }

    /// Render Mode 1: Working tree preview (uncommitted changes vs HEAD)
    /// Matches `wt list` "HEAD±" column
    fn render_working_tree_preview(&self) -> String {
        let Some(wt_info) = self.item.worktree_info() else {
            return "No worktree (branch only)\n".to_string();
        };

        let path = wt_info.worktree.path.display().to_string();
        self.render_diff_preview(&["-C", &path, "diff", "HEAD"], "No uncommitted changes")
    }

    /// Render Mode 3: Branch diff preview (line diffs in commits ahead of main)
    /// Matches `wt list` "main…± (--full)" column
    fn render_branch_diff_preview(&self) -> String {
        if self.item.counts().ahead == 0 {
            return "No commits ahead of main\n".to_string();
        }

        let merge_base = format!("main...{}", self.item.head());
        self.render_diff_preview(&["diff", &merge_base], "No changes vs main")
    }

    /// Render Mode 2: History preview
    fn render_history_preview(&self) -> String {
        const HISTORY_LIMIT: &str = "10";

        let mut output = String::new();
        let repo = Repository::current();
        let head = self.item.head();

        // Get merge-base with main
        //
        // Note on error handling: This code runs in an interactive preview pane that updates
        // on every keystroke. We intentionally use silent fallbacks rather than propagating
        // errors to avoid disruptive error messages during navigation. The preview is
        // supplementary - users can still select worktrees even if preview fails.
        //
        // Alternative: Check specific conditions (main branch exists, valid HEAD, etc.) before
        // running git commands. This would provide better diagnostics but adds latency to
        // every preview render. Trade-off: simplicity + speed vs. detailed error messages.
        let Ok(merge_base_output) = repo.run_command(&["merge-base", "main", head]) else {
            output.push_str("No commits\n");
            return output;
        };

        let merge_base = merge_base_output.trim();

        let branch = self.item.branch_name();
        let is_main = branch == "main" || branch == "master";

        if is_main {
            // Viewing main itself - show history without dimming
            if let Ok(log_output) = repo.run_command(&[
                "log",
                "--graph",
                "--decorate",
                "--oneline",
                "--color=always",
                "-n",
                HISTORY_LIMIT,
                head,
            ]) {
                output.push_str(&log_output);
            }
        } else {
            // Not on main - show bright commits not on main, dimmed commits on main

            // Part 1: Bright commits (merge-base..HEAD)
            let range = format!("{}..{}", merge_base, head);
            if let Ok(log_output) =
                repo.run_command(&["log", "--graph", "--oneline", "--color=always", &range])
            {
                output.push_str(&log_output);
            }

            // Part 2: Dimmed commits on main (history before merge-base)
            if let Ok(log_output) = repo.run_command(&[
                "log",
                "--graph",
                "--oneline",
                "--format=%C(dim)%h %s%C(reset)",
                "--color=always",
                "-n",
                HISTORY_LIMIT,
                merge_base,
            ]) {
                output.push_str(&log_output);
            }
        }

        output
    }
}

pub fn handle_select() -> Result<(), GitError> {
    let repo = Repository::current();

    // Initialize preview mode state file (auto-cleanup on drop)
    let _state = PreviewState::new();

    // Gather list data using existing logic
    let Some(list_data) = gather_list_data(&repo, false, false, false)? else {
        return Ok(());
    };

    // Calculate max branch name length for alignment
    let max_branch_len = list_data
        .items
        .iter()
        .map(|item| item.branch_name().len())
        .max()
        .unwrap_or(20);

    // Convert to skim items - store full ListItem for preview rendering
    let items: Vec<Arc<dyn SkimItem>> = list_data
        .items
        .into_iter()
        .map(|item| {
            let branch_name = item.branch_name().to_string();
            let commit_msg = item
                .commit_details()
                .commit_message
                .lines()
                .next()
                .unwrap_or("");

            // Build display text with aligned columns
            let mut display_text = format!("{:<width$}", branch_name, width = max_branch_len);

            // Add status symbols for worktrees (fixed width)
            let status = if let Some(wt_info) = item.worktree_info() {
                format!("{:<8}", wt_info.status_symbols.render())
            } else {
                "        ".to_string()
            };
            display_text.push_str(&status);

            // Add commit message
            display_text.push_str("  ");
            display_text.push_str(commit_msg);

            Arc::new(WorktreeSkimItem {
                display_text,
                branch_name,
                item: Arc::new(item),
            }) as Arc<dyn SkimItem>
        })
        .collect();

    // Get state path for key bindings
    let state_path_str = _state.path.display().to_string();

    // Configure skim options with Rust-based preview and mode switching keybindings
    let options = SkimOptionsBuilder::default()
        .height("50%".to_string())
        .multi(false)
        .preview(Some("".to_string())) // Enable preview (empty string means use SkimItem::preview())
        .preview_window("right:50%".to_string())
        .color(Some(
            "fg:-1,bg:-1,matched:108,current:-1,current_bg:254,current_match:108".to_string(),
        ))
        .bind(vec![
            // Mode switching
            format!(
                "1:execute-silent(echo 1 > {})+refresh-preview",
                state_path_str
            ),
            format!(
                "2:execute-silent(echo 2 > {})+refresh-preview",
                state_path_str
            ),
            format!(
                "3:execute-silent(echo 3 > {})+refresh-preview",
                state_path_str
            ),
            // Preview scrolling
            "ctrl-u:preview-page-up".to_string(),
            "ctrl-d:preview-page-down".to_string(),
        ])
        .header(Some(
            "1: working | 2: history | 3: diff | ctrl-u/d: scroll | ctrl-/: toggle".to_string(),
        ))
        .build()
        .map_err(|e| GitError::CommandFailed(format!("Failed to build skim options: {}", e)))?;

    // Create item receiver
    let (tx, rx): (SkimItemSender, SkimItemReceiver) = unbounded();
    for item in items {
        tx.send(item)
            .map_err(|e| GitError::CommandFailed(format!("Failed to send item to skim: {}", e)))?;
    }
    drop(tx);

    // Run skim
    let output = Skim::run_with(&options, Some(rx));

    // Handle selection
    if let Some(out) = output
        && !out.is_abort
        && let Some(selected) = out.selected_items.first()
    {
        // Get branch name or worktree path from selected item
        // (output() returns the worktree path for existing worktrees, branch name otherwise)
        let identifier = selected.output().to_string();

        // Load config
        let config = WorktrunkConfig::load().git_context("Failed to load config")?;

        // Switch to the selected worktree
        // handle_switch can handle both branch names and worktree paths
        let (result, resolved_branch) =
            handle_switch(&identifier, false, None, false, false, &config)?;

        // Show success message (show shell integration hint if not configured)
        handle_switch_output(&result, &resolved_branch, false)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_preview_mode_from_u8() {
        assert_eq!(PreviewMode::from_u8(1), PreviewMode::WorkingTree);
        assert_eq!(PreviewMode::from_u8(2), PreviewMode::History);
        assert_eq!(PreviewMode::from_u8(3), PreviewMode::BranchDiff);
        // Invalid values default to WorkingTree
        assert_eq!(PreviewMode::from_u8(0), PreviewMode::WorkingTree);
        assert_eq!(PreviewMode::from_u8(99), PreviewMode::WorkingTree);
    }

    #[test]
    fn test_preview_mode_state_file_read_default() {
        // When state file doesn't exist or is invalid, default to WorkingTree
        let state_path = PreviewMode::state_path();
        // Clean up any existing state
        let _ = fs::remove_file(&state_path);

        assert_eq!(PreviewMode::read_from_state(), PreviewMode::WorkingTree);
    }

    #[test]
    fn test_preview_mode_state_file_roundtrip() {
        // Use a unique test file to avoid conflicts with concurrent tests
        let test_state_path =
            std::env::temp_dir().join(format!("wt-select-mode-test-{}", std::process::id()));

        // Write mode 1 (WorkingTree)
        fs::write(&test_state_path, "1").unwrap();
        let mode = fs::read_to_string(&test_state_path)
            .ok()
            .and_then(|s| s.trim().parse::<u8>().ok())
            .map(PreviewMode::from_u8)
            .unwrap_or(PreviewMode::WorkingTree);
        assert_eq!(mode, PreviewMode::WorkingTree);

        // Write mode 2 (History)
        fs::write(&test_state_path, "2").unwrap();
        let mode = fs::read_to_string(&test_state_path)
            .ok()
            .and_then(|s| s.trim().parse::<u8>().ok())
            .map(PreviewMode::from_u8)
            .unwrap_or(PreviewMode::WorkingTree);
        assert_eq!(mode, PreviewMode::History);

        // Write mode 3 (BranchDiff)
        fs::write(&test_state_path, "3").unwrap();
        let mode = fs::read_to_string(&test_state_path)
            .ok()
            .and_then(|s| s.trim().parse::<u8>().ok())
            .map(PreviewMode::from_u8)
            .unwrap_or(PreviewMode::WorkingTree);
        assert_eq!(mode, PreviewMode::BranchDiff);

        // Cleanup
        let _ = fs::remove_file(&test_state_path);
    }
}
