//! Skim item implementations.
//!
//! Wrappers for ListItem and header row that implement SkimItem for the interactive selector.

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::Arc;

use anstyle::Reset;
use color_print::cformat;
use skim::prelude::*;
use worktrunk::git::Repository;
use worktrunk::styling::INFO_SYMBOL;

use super::super::list::model::ListItem;
use super::log_formatter::{
    FIELD_DELIM, batch_fetch_stats, format_log_output, process_log_with_dimming, strip_hash_markers,
};
use super::pager::{get_diff_pager, run_git_diff_with_pager};
use super::preview::{PreviewMode, PreviewStateData};

/// Header item for column names (non-selectable)
pub(super) struct HeaderSkimItem {
    pub display_text: String,
    pub display_text_with_ansi: String,
}

impl SkimItem for HeaderSkimItem {
    fn text(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.display_text)
    }

    fn display<'a>(&'a self, _context: skim::DisplayContext<'a>) -> skim::AnsiString<'a> {
        skim::AnsiString::parse(&self.display_text_with_ansi)
    }

    fn output(&self) -> Cow<'_, str> {
        Cow::Borrowed("") // Headers produce no output if selected
    }
}

/// Wrapper to implement SkimItem for ListItem
pub(super) struct WorktreeSkimItem {
    pub display_text: String,
    pub display_text_with_ansi: String,
    pub branch_name: String,
    pub item: Arc<ListItem>,
}

impl SkimItem for WorktreeSkimItem {
    fn text(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.display_text)
    }

    fn display<'a>(&'a self, _context: skim::DisplayContext<'a>) -> skim::AnsiString<'a> {
        skim::AnsiString::parse(&self.display_text_with_ansi)
    }

    fn output(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.branch_name)
    }

    fn preview(&self, context: PreviewContext<'_>) -> ItemPreview {
        let mode = PreviewStateData::read_mode();

        // Build preview: tabs header + content
        let mut result = Self::render_preview_tabs(mode);
        result.push_str(&self.preview_for_mode(mode, context.width, context.height));

        ItemPreview::AnsiText(result)
    }
}

impl WorktreeSkimItem {
    /// Render the tab header for the preview window
    ///
    /// Shows all preview modes as tabs, with the current mode bolded
    /// and unselected modes dimmed. Controls shown below in normal text
    /// for visual distinction from inactive tabs.
    pub(super) fn render_preview_tabs(mode: PreviewMode) -> String {
        // Full SGR reset (\x1b[0m) to ensure clean state between styled elements.
        // Using anstyle::Reset instead of cformat's </> (\x1b[22m) because some terminals
        // don't properly handle the intensity-only reset after bold text. This matches
        // the pattern used in src/styling/format.rs for similar ANSI reset handling.
        let reset = Reset;

        /// Format a tab label with bold (active) or dimmed (inactive) styling
        fn format_tab(label: &str, is_active: bool) -> String {
            if is_active {
                cformat!("<bold>{}</>", label)
            } else {
                cformat!("<dim>{}</>", label)
            }
        }

        let tab1 = format_tab("1: HEAD±", mode == PreviewMode::WorkingTree);
        let tab2 = format_tab("2: log", mode == PreviewMode::Log);
        let tab3 = format_tab("3: main…±", mode == PreviewMode::BranchDiff);
        let tab4 = format_tab("4: remote⇅", mode == PreviewMode::UpstreamDiff);

        // Controls use dim yellow to distinguish from dimmed (white) tabs
        let controls = cformat!(
            "<dim,yellow>Enter: switch | alt-c: create | Esc: cancel | ctrl-u/d: scroll | alt-p: toggle</>"
        );

        // End each tab and controls with full reset to prevent style bleeding
        // into dividers and preview content
        format!(
            "{tab1}{reset} | {tab2}{reset} | {tab3}{reset} | {tab4}{reset}\n{controls}{reset}\n\n"
        )
    }

    /// Render preview for the given mode with specified dimensions
    fn preview_for_mode(&self, mode: PreviewMode, width: usize, height: usize) -> String {
        match mode {
            PreviewMode::WorkingTree => self.render_working_tree_preview(width),
            PreviewMode::Log => self.render_log_preview(width, height),
            PreviewMode::BranchDiff => self.render_branch_diff_preview(width),
            PreviewMode::UpstreamDiff => self.render_upstream_diff_preview(width),
        }
    }

    /// Common diff rendering pattern: check stat, show stat + full diff if non-empty
    fn render_diff_preview(&self, args: &[&str], no_changes_msg: &str, width: usize) -> String {
        let mut output = String::new();
        let Ok(repo) = Repository::current() else {
            return no_changes_msg.to_string();
        };

        // Check stat output first
        let mut stat_args = args.to_vec();
        stat_args.push("--stat");
        stat_args.push("--color=always");
        let stat_width_arg = format!("--stat-width={}", width);
        stat_args.push(&stat_width_arg);

        if let Ok(stat) = repo.run_command(&stat_args)
            && !stat.trim().is_empty()
        {
            output.push_str(&stat);

            // Build diff args with color
            let mut diff_args = args.to_vec();
            diff_args.push("--color=always");

            // Try streaming through pager first (git diff | pager), fall back to plain diff
            let diff = get_diff_pager()
                .and_then(|pager| run_git_diff_with_pager(&diff_args, pager, width))
                .or_else(|| repo.run_command(&diff_args).ok());

            if let Some(diff) = diff {
                output.push_str(&diff);
            }
        } else {
            output.push_str(no_changes_msg);
            output.push('\n');
        }

        output
    }

    /// Render Tab 1: Working tree preview (uncommitted changes vs HEAD)
    /// Matches `wt list` "HEAD±" column
    fn render_working_tree_preview(&self, width: usize) -> String {
        let Some(wt_info) = self.item.worktree_data() else {
            // Branch without worktree - selecting will create one
            let branch = self.item.branch_name();
            return format!(
                "{INFO_SYMBOL} {branch} is branch only — press Enter to create worktree\n"
            );
        };

        let branch = self.item.branch_name();
        let path = wt_info.path.display().to_string();

        self.render_diff_preview(
            &["-C", &path, "diff", "HEAD"],
            &cformat!("{INFO_SYMBOL} <bold>{branch}</> has no uncommitted changes"),
            width,
        )
    }

    /// Render Tab 3: Branch diff preview (line diffs in commits ahead of default branch)
    /// Matches `wt list` "main…± (--full)" column
    fn render_branch_diff_preview(&self, width: usize) -> String {
        let branch = self.item.branch_name();
        let Ok(repo) = Repository::current() else {
            return cformat!("{INFO_SYMBOL} <bold>{branch}</> has no commits ahead of main\n");
        };
        let Some(default_branch) = repo.default_branch() else {
            return cformat!("{INFO_SYMBOL} <bold>{branch}</> has no commits ahead of main\n");
        };
        if self.item.counts.is_some_and(|c| c.ahead == 0) {
            return cformat!(
                "{INFO_SYMBOL} <bold>{branch}</> has no commits ahead of <bold>{default_branch}</>\n"
            );
        }

        let merge_base = format!("{}...{}", default_branch, self.item.head());
        self.render_diff_preview(
            &["diff", &merge_base],
            &cformat!(
                "{INFO_SYMBOL} <bold>{branch}</> has no file changes vs <bold>{default_branch}</>"
            ),
            width,
        )
    }

    /// Render Tab 4: Upstream diff preview (ahead/behind vs tracking branch)
    /// Matches `wt list` "Remote⇅" column
    fn render_upstream_diff_preview(&self, width: usize) -> String {
        let branch = self.item.branch_name();

        // Check if this branch has an upstream tracking branch
        // Use as_ref() to avoid cloning UpstreamStatus on every preview render
        let Some(active) = self.item.upstream.as_ref().and_then(|u| u.active()) else {
            return cformat!("{INFO_SYMBOL} <bold>{branch}</> has no upstream tracking branch\n");
        };

        // Use @{u} syntax for performance (avoids extra git command to resolve upstream ref)
        // Format: branch@{u} resolves to the upstream tracking branch
        let upstream_ref = format!("{}@{{u}}", branch);

        if active.ahead == 0 && active.behind == 0 {
            return cformat!("{INFO_SYMBOL} <bold>{branch}</> is up to date with upstream\n");
        }

        // Handle different states: ahead only, behind only, or diverged
        // Use ⇡/⇣ symbols to match wt list's Remote⇅ column
        if active.ahead > 0 && active.behind > 0 {
            // Diverged: show local changes (what would be pushed)
            // Use three-dot diff to show changes unique to local branch
            let range = format!("{}...{}", upstream_ref, self.item.head());
            self.render_diff_preview(
                &["diff", &range],
                &cformat!(
                    "{INFO_SYMBOL} <bold>{branch}</> has diverged (⇡{} ⇣{}) but no unique file changes",
                    active.ahead,
                    active.behind
                ),
                width,
            )
        } else if active.ahead > 0 {
            // Ahead only: show unpushed commits
            let range = format!("{}...{}", upstream_ref, self.item.head());
            self.render_diff_preview(
                &["diff", &range],
                &cformat!("{INFO_SYMBOL} <bold>{branch}</> has no unpushed file changes"),
                width,
            )
        } else {
            // Behind only: show what upstream has that we don't
            let range = format!("{}...{}", self.item.head(), upstream_ref);
            self.render_diff_preview(
                &["diff", &range],
                &cformat!(
                    "{INFO_SYMBOL} <bold>{branch}</> is behind upstream (⇣{}) but no file changes",
                    active.behind
                ),
                width,
            )
        }
    }

    /// Render Tab 2: Log preview
    fn render_log_preview(&self, width: usize, height: usize) -> String {
        // Minimum preview width to show timestamps (adds ~7 chars: space + 4-char time + space)
        // Note: preview is typically 50% of terminal width, so 50 = 100-col terminal
        const TIMESTAMP_WIDTH_THRESHOLD: usize = 50;
        // Tab header takes 3 lines (tabs + controls + blank)
        const HEADER_LINES: usize = 3;

        let mut output = String::new();
        let show_timestamps = width >= TIMESTAMP_WIDTH_THRESHOLD;
        // Calculate how many log lines fit in preview (height minus header)
        let log_limit = height.saturating_sub(HEADER_LINES).max(1);
        let head = self.item.head();
        let branch = self.item.branch_name();
        let Ok(repo) = Repository::current() else {
            output.push_str(&cformat!(
                "{INFO_SYMBOL} <bold>{branch}</> has no commits\n"
            ));
            return output;
        };
        let Some(default_branch) = repo.default_branch() else {
            output.push_str(&cformat!(
                "{INFO_SYMBOL} <bold>{branch}</> has no commits\n"
            ));
            return output;
        };

        // Get merge-base with default branch
        //
        // Note on error handling: This code runs in an interactive preview pane that updates
        // on every keystroke. We intentionally use silent fallbacks rather than propagating
        // errors to avoid disruptive error messages during navigation. The preview is
        // supplementary - users can still select worktrees even if preview fails.
        //
        // Alternative: Check specific conditions (default branch exists, valid HEAD, etc.) before
        // running git commands. This would provide better diagnostics but adds latency to
        // every preview render. Trade-off: simplicity + speed vs. detailed error messages.
        let Ok(merge_base_output) = repo.run_command(&["merge-base", &default_branch, head]) else {
            output.push_str(&cformat!(
                "{INFO_SYMBOL} <bold>{branch}</> has no commits\n"
            ));
            return output;
        };

        let merge_base = merge_base_output.trim();
        let is_default_branch = branch == default_branch;

        // Format strings for git log
        // Without timestamps: hash (colored/dimmed), then message
        // Format includes full hash (for matching) between SOH and NUL delimiters.
        // Display content uses \x1f to separate fields for timestamp parsing.
        // Format: SOH full_hash NUL short_hash \x1f timestamp \x1f decorations+message
        // Using delimiters allows parsing without assuming fixed hash length (SHA-256 safe)
        // Note: Use %x01/%x00 (git's hex escapes) to avoid embedding control chars in argv
        let timestamp_format = format!(
            "--format=%x01%H%x00%C(auto)%h{}%ct{}%C(auto)%d%C(reset) %s",
            FIELD_DELIM, FIELD_DELIM
        );
        let no_timestamp_format = "--format=%x01%H%x00%C(auto)%h%C(auto)%d%C(reset) %s";

        let log_limit_str = log_limit.to_string();

        // Get commits after merge-base (for dimming logic)
        // These are commits reachable from HEAD but not from merge-base, shown bright.
        // Commits before merge-base (shared with default branch) are shown dimmed.
        // Bounded to log_limit since we only need to check displayed commits.
        let unique_commits: Option<HashSet<String>> = if is_default_branch {
            // On default branch: no dimming (None means show everything bright)
            None
        } else {
            // On feature branch: get commits unique to this branch
            // rev-list A...B --right-only gives commits reachable from B but not A
            let range = format!("{}...{}", merge_base, head);
            let commits = repo
                .run_command(&["rev-list", &range, "--right-only", "-n", &log_limit_str])
                .map(|out| out.lines().map(String::from).collect())
                .unwrap_or_default();
            Some(commits) // Some(empty) means dim everything
        };

        // Get graph output (no --numstat to avoid blank continuation lines)
        let format: &str = if show_timestamps {
            &timestamp_format
        } else {
            no_timestamp_format
        };
        let args = vec![
            "log",
            "--graph",
            format,
            "--color=always",
            "-n",
            &log_limit_str,
            head,
        ];

        if let Ok(log_output) = repo.run_command(&args) {
            let (processed, hashes) =
                process_log_with_dimming(&log_output, unique_commits.as_ref());
            if show_timestamps {
                // Batch fetch stats for all commits
                let stats = batch_fetch_stats(&repo, &hashes);
                output.push_str(&format_log_output(&processed, &stats));
            } else {
                // Strip hash markers (SOH...NUL) since we're not using format_log_output
                output.push_str(&strip_hash_markers(&processed));
            }
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_preview_tabs_working_tree_mode() {
        let output = WorktreeSkimItem::render_preview_tabs(PreviewMode::WorkingTree);
        // Tab 1 should be bold (active), tabs 2/3/4 dimmed
        assert!(output.contains("1: HEAD±"));
        assert!(output.contains("2: log"));
        assert!(output.contains("3: main…±"));
        assert!(output.contains("4: remote⇅"));
        assert!(output.contains("Enter: switch"));
        // Verify structure: tabs on first line, controls on second
        assert!(output.contains(" | "));
        assert!(output.ends_with("\n\n"));
    }

    #[test]
    fn test_render_preview_tabs_log_mode() {
        let output = WorktreeSkimItem::render_preview_tabs(PreviewMode::Log);
        assert!(output.contains("1: HEAD±"));
        assert!(output.contains("2: log"));
        assert!(output.contains("3: main…±"));
        assert!(output.contains("4: remote⇅"));
    }

    #[test]
    fn test_render_preview_tabs_branch_diff_mode() {
        let output = WorktreeSkimItem::render_preview_tabs(PreviewMode::BranchDiff);
        assert!(output.contains("1: HEAD±"));
        assert!(output.contains("2: log"));
        assert!(output.contains("3: main…±"));
        assert!(output.contains("4: remote⇅"));
    }

    #[test]
    fn test_render_preview_tabs_upstream_diff_mode() {
        let output = WorktreeSkimItem::render_preview_tabs(PreviewMode::UpstreamDiff);
        assert!(output.contains("1: HEAD±"));
        assert!(output.contains("2: log"));
        assert!(output.contains("3: main…±"));
        assert!(output.contains("4: remote⇅"));
    }

    #[test]
    fn test_render_preview_tabs_ansi_codes() {
        // Test that ANSI escape sequences properly reset to prevent style bleeding
        let output = WorktreeSkimItem::render_preview_tabs(PreviewMode::WorkingTree);

        let first_line = output.lines().next().unwrap();
        let second_line = output.lines().nth(1).unwrap();

        // Each styled tab should end with a full reset (\x1b[0m) before the divider
        // This prevents bold/dim from bleeding into the " | " dividers
        let full_reset = "\x1b[0m";

        // Count resets - should have one after each of the 4 tabs
        assert_eq!(first_line.matches(full_reset).count(), 4);

        // The sequence should be: style + text + [22m + [0m + divider
        // Check that dividers come after full resets
        let parts: Vec<&str> = first_line.split(" | ").collect();
        assert_eq!(parts.len(), 4);
        assert!(parts.iter().all(|part| part.ends_with(full_reset)));

        // Controls line should end with full reset to ensure clean state for preview content
        assert!(second_line.ends_with(full_reset));
    }
}
