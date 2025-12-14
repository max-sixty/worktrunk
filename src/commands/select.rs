use anyhow::Context;
use color_print::cformat;
use skim::prelude::*;
use std::borrow::Cow;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use worktrunk::config::WorktrunkConfig;
use worktrunk::git::Repository;

use super::list::collect;
use super::list::layout::{DiffDisplayConfig, DiffVariant};
use super::list::model::ListItem;
use super::worktree::handle_switch;
use crate::output::handle_switch_output;

/// Cached pager command, detected once at startup.
///
/// None means no pager should be used (empty config or "cat").
/// We cache this to avoid running `git config` on every preview render.
static CACHED_PAGER: OnceLock<Option<String>> = OnceLock::new();

/// Get the cached pager command, initializing if needed.
fn get_diff_pager() -> Option<&'static String> {
    CACHED_PAGER
        .get_or_init(|| {
            // Returns Some(pager) if valid, None if empty/cat (no pager desired)
            let parse_pager = |s: &str| -> Option<String> {
                let trimmed = s.trim();
                (!trimmed.is_empty() && trimmed != "cat").then(|| trimmed.to_string())
            };

            // GIT_PAGER takes precedence - if set (even to "cat" or empty), don't fall back
            if let Ok(pager) = std::env::var("GIT_PAGER") {
                return parse_pager(&pager);
            }

            // Fall back to core.pager config
            Command::new("git")
                .args(["config", "--get", "core.pager"])
                .output()
                .ok()
                .and_then(|output| {
                    if output.status.success() {
                        String::from_utf8(output.stdout)
                            .ok()
                            .and_then(|s| parse_pager(&s))
                    } else {
                        None
                    }
                })
        })
        .as_ref()
}

/// Check if the pager spawns its own internal pager (e.g., less).
///
/// Some pagers like delta and bat spawn `less` by default, which hangs in
/// non-TTY contexts like skim's preview panel. These need `--paging=never`.
///
/// TODO: Replace this hardcoded detection with a config option like
/// `select.pager = "delta --paging=never"` so users can specify their own
/// pager command with appropriate flags. This would eliminate the need to
/// maintain a list of pagers that need special handling.
fn pager_needs_paging_disabled(pager_cmd: &str) -> bool {
    // Split on whitespace to get the command name, then check basename
    pager_cmd
        .split_whitespace()
        .next()
        .and_then(|cmd| cmd.rsplit('/').next())
        // bat is called "batcat" on Debian/Ubuntu
        .is_some_and(|basename| matches!(basename, "delta" | "bat" | "batcat"))
}

/// Maximum time to wait for pager to complete.
///
/// Pager blocking can freeze skim's event loop, making the UI unresponsive.
/// If the pager takes longer than this, kill it and fall back to raw diff.
const PAGER_TIMEOUT: Duration = Duration::from_millis(2000);

/// Skim uses this percentage of terminal height.
const SKIM_HEIGHT_PERCENT: usize = 90;

/// Maximum number of list items visible in down layout before scrolling.
const MAX_VISIBLE_ITEMS: usize = 12;

/// Lines reserved for skim chrome (header + prompt/margins).
const LIST_CHROME_LINES: usize = 4;

/// Minimum preview lines to keep usable even with many items.
const MIN_PREVIEW_LINES: usize = 5;

/// Run git diff piped directly through the pager as a streaming pipeline.
///
/// Runs `git <args> | pager` as a single shell command, avoiding intermediate
/// buffering. For pagers that spawn their own sub-pager (delta, bat), adds
/// `--paging=never` to prevent them from spawning less.
/// Returns None if pipeline fails or times out (caller should fall back to raw diff).
fn run_git_diff_with_pager(git_args: &[&str], pager_cmd: &str) -> Option<String> {
    // Note: pager_cmd is expected to be valid shell code (like git's core.pager).
    // Users with paths containing special chars must quote them in their config.

    // Some pagers spawn `less` by default which hangs in non-TTY contexts
    let pager_with_args = if pager_needs_paging_disabled(pager_cmd) {
        format!("{} --paging=never", pager_cmd)
    } else {
        pager_cmd.to_string()
    };

    // Build shell pipeline: git <args> | pager
    // Shell-escape args to handle paths with spaces
    let escaped_args: Vec<String> = git_args.iter().map(|arg| shell_escape(arg)).collect();
    let pipeline = format!("git {} | {}", escaped_args.join(" "), pager_with_args);

    log::debug!("Running pager pipeline: {}", pipeline);

    // Spawn pipeline
    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(&pipeline)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            log::debug!("Failed to spawn pager pipeline: {}", e);
            return None;
        }
    };

    // Read output in a thread to avoid blocking
    let stdout = child.stdout.take()?;
    let reader_thread = std::thread::spawn(move || {
        use std::io::Read;
        let mut stdout = stdout;
        let mut output = Vec::new();
        let _ = stdout.read_to_end(&mut output);
        output
    });

    // Wait for pipeline with timeout
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = reader_thread.join().ok()?;
                if status.success() {
                    return String::from_utf8(output).ok();
                } else {
                    log::debug!("Pager pipeline exited with status: {}", status);
                    return None;
                }
            }
            Ok(None) => {
                if start.elapsed() > PAGER_TIMEOUT {
                    log::debug!("Pager pipeline timed out after {:?}", PAGER_TIMEOUT);
                    let _ = child.kill();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => {
                log::debug!("Failed to wait for pager pipeline: {}", e);
                let _ = child.kill();
                return None;
            }
        }
    }
}

/// Shell-escape a string for use in sh -c commands.
fn shell_escape(s: &str) -> String {
    // If it contains special chars, wrap in single quotes and escape existing single quotes
    if s.chars()
        .any(|c| c.is_whitespace() || "\"'\\$`!*?[]{}|&;<>()".contains(c))
    {
        format!("'{}'", s.replace('\'', "'\\''"))
    } else {
        s.to_string()
    }
}

/// Preview modes for the interactive selector
///
/// Each mode shows a different aspect of the worktree:
/// 1. WorkingTree: Uncommitted changes (git diff HEAD --stat)
/// 2. Log: Commit history since diverging from main (git log with merge-base)
/// 3. BranchDiff: Line diffs in commits ahead of main (git diff --stat main…)
///
/// Loosely aligned with `wt list` columns, though not a perfect match:
/// - Tab 1 corresponds to "HEAD±" column
/// - Tab 2 shows commits (related to "main↕" counts)
/// - Tab 3 corresponds to "main…± (--full)" column
///
/// TODO: Consider adding tab 4 "remote±" showing diff vs upstream tracking branch
/// (unpushed commits). Would align with "Remote⇅" column in `wt list`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewMode {
    WorkingTree = 1,
    Log = 2,
    BranchDiff = 3,
}

/// Typical terminal character aspect ratio (width/height).
///
/// Terminal characters are taller than wide - typically around 0.5 (twice as tall as wide).
/// This varies by font, but 0.5 is a reasonable default for monospace fonts.
const CHAR_ASPECT_RATIO: f64 = 0.5;

/// Preview layout orientation for the interactive selector
///
/// Preview window position (auto-detected at startup based on terminal dimensions)
///
/// - Right: Preview on the right side (50% width) - better for wide terminals
/// - Down: Preview below the list - better for tall/vertical monitors
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum PreviewLayout {
    #[default]
    Right,
    Down,
}

impl PreviewLayout {
    /// Auto-detect layout based on terminal dimensions.
    ///
    /// Terminal dimensions are in characters, not pixels. Since characters are
    /// typically twice as tall as wide (~0.5 aspect ratio), we correct for this
    /// when calculating the effective aspect ratio.
    ///
    /// Example: 180 cols × 136 rows
    /// - Raw ratio: 180/136 = 1.32 (appears landscape)
    /// - Effective: 1.32 × 0.5 = 0.66 (actually portrait!)
    ///
    /// Returns Down for portrait (effective ratio < 1.0), Right for landscape.
    fn auto_detect() -> Self {
        let (cols, rows) = terminal_size::terminal_size()
            .map(|(terminal_size::Width(w), terminal_size::Height(h))| (w as f64, h as f64))
            .unwrap_or((80.0, 24.0));

        // Effective aspect ratio accounting for character shape
        let effective_ratio = (cols / rows) * CHAR_ASPECT_RATIO;

        if effective_ratio < 1.0 {
            Self::Down
        } else {
            Self::Right
        }
    }
}

impl PreviewLayout {
    /// Calculate the preview window spec for skim
    ///
    /// For Right layout: always 50%
    /// For Down layout: dynamically sized based on item count - list gets
    /// up to MAX_VISIBLE_ITEMS lines, preview gets the rest (min 5 lines)
    fn to_preview_window_spec(self, num_items: usize) -> String {
        match self {
            Self::Right => "right:50%".to_string(),
            Self::Down => {
                let height = terminal_size::terminal_size()
                    .map(|(_, terminal_size::Height(h))| h as usize)
                    .unwrap_or(24);

                let available = height * SKIM_HEIGHT_PERCENT / 100;
                let list_lines = LIST_CHROME_LINES + num_items.min(MAX_VISIBLE_ITEMS);
                // Ensure preview doesn't exceed available space while trying to maintain minimum
                let remaining = available.saturating_sub(list_lines);
                let preview_lines = remaining.max(MIN_PREVIEW_LINES).min(available);

                format!("down:{}", preview_lines)
            }
        }
    }
}

impl PreviewMode {
    fn from_u8(n: u8) -> Self {
        match n {
            2 => Self::Log,
            3 => Self::BranchDiff,
            _ => Self::WorkingTree,
        }
    }
}

/// Preview state persistence (mode only, layout auto-detected)
///
/// State file format: Single digit representing preview mode (1=WorkingTree, 2=Log, 3=BranchDiff)
struct PreviewStateData;

impl PreviewStateData {
    fn state_path() -> PathBuf {
        // Use per-process temp file to avoid race conditions when running multiple instances
        std::env::temp_dir().join(format!("wt-select-state-{}", std::process::id()))
    }

    /// Read current preview mode from state file
    fn read_mode() -> PreviewMode {
        let state_path = Self::state_path();
        fs::read_to_string(&state_path)
            .ok()
            .and_then(|s| s.trim().parse::<u8>().ok())
            .map(PreviewMode::from_u8)
            .unwrap_or(PreviewMode::WorkingTree)
    }

    fn write_mode(mode: PreviewMode) {
        let state_path = Self::state_path();
        let _ = fs::write(&state_path, format!("{}", mode as u8));
    }
}

/// RAII wrapper for preview state file lifecycle management
struct PreviewState {
    path: PathBuf,
    initial_layout: PreviewLayout,
}

impl PreviewState {
    fn new() -> Self {
        let path = PreviewStateData::state_path();
        PreviewStateData::write_mode(PreviewMode::WorkingTree);
        Self {
            path,
            initial_layout: PreviewLayout::auto_detect(),
        }
    }
}

impl Drop for PreviewState {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Header item for column names (non-selectable)
struct HeaderSkimItem {
    display_text: String,
    display_text_with_ansi: String,
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
struct WorktreeSkimItem {
    display_text: String,
    display_text_with_ansi: String,
    branch_name: String,
    item: Arc<ListItem>,
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
    fn render_preview_tabs(mode: PreviewMode) -> String {
        use anstyle::Style;

        /// Format a tab label with bold (active) or dimmed (inactive) styling
        fn format_tab(label: &str, is_active: bool) -> String {
            let style = if is_active {
                Style::new().bold()
            } else {
                Style::new().dimmed()
            };
            format!("{}{}{}", style.render(), label, style.render_reset())
        }

        let tab1 = format_tab("1: HEAD±", mode == PreviewMode::WorkingTree);
        let tab2 = format_tab("2: log", mode == PreviewMode::Log);
        let tab3 = format_tab("3: main…±", mode == PreviewMode::BranchDiff);

        // Controls use dim yellow to distinguish from dimmed (white) tabs
        // while remaining subdued
        let controls_style = Style::new()
            .dimmed()
            .fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::Yellow)));
        let controls = format!(
            "{}Enter: switch | Esc: cancel | ctrl-u/d: scroll | alt-p: toggle{}",
            controls_style.render(),
            controls_style.render_reset()
        );

        format!("{} | {} | {}\n{}\n\n", tab1, tab2, tab3, controls)
    }

    /// Render preview for the given mode with specified dimensions
    fn preview_for_mode(&self, mode: PreviewMode, width: usize, height: usize) -> String {
        match mode {
            PreviewMode::WorkingTree => self.render_working_tree_preview(width),
            PreviewMode::Log => self.render_log_preview(width, height),
            PreviewMode::BranchDiff => self.render_branch_diff_preview(width),
        }
    }

    /// Common diff rendering pattern: check stat, show stat + full diff if non-empty
    fn render_diff_preview(&self, args: &[&str], no_changes_msg: &str, width: usize) -> String {
        let mut output = String::new();
        let repo = Repository::current();

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
                .and_then(|pager| run_git_diff_with_pager(&diff_args, pager))
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
        use worktrunk::styling::INFO_EMOJI;

        let Some(wt_info) = self.item.worktree_data() else {
            // Branch without worktree - selecting will create one
            let branch = self.item.branch_name();
            return format!(
                "{INFO_EMOJI} {branch} is branch only — press Enter to create worktree\n"
            );
        };

        let branch = self.item.branch_name();
        let path = wt_info.path.display().to_string();
        self.render_diff_preview(
            &["-C", &path, "diff", "HEAD"],
            &cformat!("{INFO_EMOJI} <bold>{branch}</> has no uncommitted changes"),
            width,
        )
    }

    /// Render Tab 3: Branch diff preview (line diffs in commits ahead of default branch)
    /// Matches `wt list` "main…± (--full)" column
    fn render_branch_diff_preview(&self, width: usize) -> String {
        use worktrunk::styling::INFO_EMOJI;

        let branch = self.item.branch_name();
        let repo = Repository::current();
        let Ok(default_branch) = repo.default_branch() else {
            return cformat!("{INFO_EMOJI} <bold>{branch}</> has no commits ahead of main\n");
        };
        if self.item.counts().ahead == 0 {
            return cformat!(
                "{INFO_EMOJI} <bold>{branch}</> has no commits ahead of <bold>{default_branch}</>\n"
            );
        }

        let merge_base = format!("{}...{}", default_branch, self.item.head());
        self.render_diff_preview(
            &["diff", &merge_base],
            &cformat!("{INFO_EMOJI} <bold>{branch}</> has no changes vs <bold>{default_branch}</>"),
            width,
        )
    }

    /// Render Tab 2: Log preview
    fn render_log_preview(&self, width: usize, height: usize) -> String {
        use worktrunk::styling::INFO_EMOJI;
        // Minimum preview width to show timestamps (adds ~7 chars: space + 4-char time + space)
        // Note: preview is typically 50% of terminal width, so 50 = 100-col terminal
        const TIMESTAMP_WIDTH_THRESHOLD: usize = 50;
        // Tab header takes 3 lines (tabs + controls + blank)
        const HEADER_LINES: usize = 3;

        let mut output = String::new();
        let show_timestamps = width >= TIMESTAMP_WIDTH_THRESHOLD;
        // Calculate how many log lines fit in preview (height minus header)
        let log_limit = height.saturating_sub(HEADER_LINES).max(1);
        let repo = Repository::current();
        let head = self.item.head();
        let branch = self.item.branch_name();
        let Ok(default_branch) = repo.default_branch() else {
            output.push_str(&cformat!("{INFO_EMOJI} <bold>{branch}</> has no commits\n"));
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
            output.push_str(&cformat!("{INFO_EMOJI} <bold>{branch}</> has no commits\n"));
            return output;
        };

        let merge_base = merge_base_output.trim();
        let is_default_branch = branch == default_branch;

        // Format strings for git log
        // Without timestamps: hash (colored/dimmed), then message
        // Dim format: only hash is dimmed, message stays normal (matches upstream style)
        let no_timestamp_format = "--format=%C(auto)%h%C(auto)%d%C(reset) %s";
        let dim_no_timestamp_format = "--format=%C(dim)%h%C(reset) %s";

        // With timestamps and diffstat, we use field delimiter and --numstat for predictable parsing:
        // - \x1f (unit separator) separates fields within a commit line
        // - --numstat gives tab-separated "added\tdeleted\tfilename" lines we sum
        // Format: graph + hash \x1f timestamp \x1f message (followed by numstat lines)
        const TIMESTAMP_WIDTH: usize = 4; // "12mo" is the longest
        const FIELD_DELIM: char = '\x1f';
        let timestamp_format = "--format=%C(auto)%h\x1f%ct\x1f%C(auto)%d%C(reset) %s";
        // Dim format: hash is dimmed, timestamp will also be dimmed
        let dim_timestamp_format = "--format=%C(dim)%h%C(reset)\x1f%ct\x1f %s";

        // Helper to format log output with timestamps and diffstat
        // Parses --numstat output: commit lines have \x1f, stat lines have tabs
        let format_log_output = |log_output: &str| -> String {
            use crate::display::format_relative_time_short;
            use worktrunk::styling::{ADDITION, DELETION};

            let dim_style = anstyle::Style::new().dimmed();
            let reset = anstyle::Reset;

            // State machine: accumulate stats for each commit
            let mut pending_commit: Option<&str> = None;
            let mut pending_stats: (usize, usize) = (0, 0);
            let mut result = Vec::new();

            // Format a commit line with stats
            let format_commit =
                |commit_line: &str, (insertions, deletions): (usize, usize)| -> String {
                    if let Some(first_delim) = commit_line.find(FIELD_DELIM)
                        && let Some(second_delim) = commit_line[first_delim + 1..].find(FIELD_DELIM)
                    {
                        let graph_hash = &commit_line[..first_delim];
                        let timestamp_str =
                            &commit_line[first_delim + 1..first_delim + 1 + second_delim];
                        let rest = &commit_line[first_delim + 1 + second_delim + 1..];

                        let time = timestamp_str
                            .parse::<i64>()
                            .map(format_relative_time_short)
                            .unwrap_or_default();

                        // Use the same diff formatting as wt list (aligned columns)
                        let diff_config = DiffDisplayConfig {
                            variant: DiffVariant::Signs,
                            positive_style: ADDITION,
                            negative_style: DELETION,
                            always_show_zeros: false,
                        };
                        // 3 digits handles values up to 999; overflow uses K notation
                        let stat_str =
                            format!(" {}", diff_config.format_aligned(insertions, deletions, 3));

                        format!(
                            "{}{} {dim_style}{:>width$}{reset}{}",
                            graph_hash,
                            stat_str,
                            time,
                            rest,
                            width = TIMESTAMP_WIDTH
                        )
                    } else {
                        commit_line.to_string()
                    }
                };

            for line in log_output.lines() {
                if line.contains(FIELD_DELIM) {
                    // This is a commit line - emit any pending commit first
                    if let Some(prev) = pending_commit.take() {
                        result.push(format_commit(prev, pending_stats));
                    }
                    pending_commit = Some(line);
                    pending_stats = (0, 0);
                } else if let Some((ins, del)) = Self::parse_numstat_line(line) {
                    // Accumulate stats for pending commit
                    pending_stats.0 += ins;
                    pending_stats.1 += del;
                }
                // Skip empty/graph-only lines
            }

            // Don't forget final commit
            if let Some(last) = pending_commit {
                result.push(format_commit(last, pending_stats));
            }

            result.join("\n")
        };

        let log_limit_str = log_limit.to_string();
        if is_default_branch {
            // Viewing default branch itself - show history without dimming
            let format = if show_timestamps {
                timestamp_format
            } else {
                no_timestamp_format
            };
            let mut args = vec!["log", "--graph", format, "--color=always"];
            if show_timestamps {
                args.push("--numstat");
            }
            args.extend_from_slice(&["-n", &log_limit_str, head]);
            if let Ok(log_output) = repo.run_command(&args) {
                if show_timestamps {
                    output.push_str(&format_log_output(&log_output));
                } else {
                    output.push_str(&log_output);
                }
            }
        } else {
            // Not on default branch - show bright commits unique to this branch, dimmed commits on default
            // Total commits shown is capped at log_limit (based on preview height)

            // Part 1: Bright commits (merge-base..HEAD)
            let range = format!("{}..{}", merge_base, head);
            let format = if show_timestamps {
                timestamp_format
            } else {
                no_timestamp_format
            };
            let mut args = vec!["log", "--graph", format, "--color=always"];
            if show_timestamps {
                args.push("--numstat");
            }
            args.push(&range);
            let mut bright_count = 0;
            if let Ok(log_output) = repo.run_command(&args)
                && !log_output.is_empty()
            {
                // Count commit lines (those with field delimiter, not numstat lines)
                bright_count = log_output.lines().filter(|l| l.contains('\x1f')).count();
                if show_timestamps {
                    output.push_str(&format_log_output(&log_output));
                } else {
                    output.push_str(&log_output);
                }
                // Ensure newline between bright and dim sections
                if !output.ends_with('\n') {
                    output.push('\n');
                }
            }

            // Part 2: Dimmed commits on default branch (history before merge-base)
            // Only show enough to reach LOG_LIMIT total
            let dim_limit = log_limit.saturating_sub(bright_count);
            if dim_limit > 0 {
                let format = if show_timestamps {
                    dim_timestamp_format
                } else {
                    dim_no_timestamp_format
                };
                let dim_limit_str = dim_limit.to_string();
                let mut args = vec!["log", "--graph", format, "--color=always"];
                if show_timestamps {
                    args.push("--numstat");
                }
                args.extend_from_slice(&["-n", &dim_limit_str, merge_base]);
                if let Ok(log_output) = repo.run_command(&args) {
                    if show_timestamps {
                        output.push_str(&format_log_output(&log_output));
                    } else {
                        output.push_str(&log_output);
                    }
                }
            }
        }

        output
    }

    /// Parse a git numstat line and extract insertions/deletions
    ///
    /// Numstat format: `added<TAB>deleted<TAB>filename`
    /// With --graph --color=always, lines have ANSI-colored graph prefix like `ESC[31m|ESC[m `.
    /// Binary files show "-" instead of numbers.
    ///
    /// Returns Some((insertions, deletions)) for valid numstat lines.
    fn parse_numstat_line(line: &str) -> Option<(usize, usize)> {
        use ansi_str::AnsiStr;

        // First strip ANSI escape sequences (graph coloring contains digits that confuse parsing)
        let stripped = line.ansi_strip();

        // Strip graph prefix (e.g., "| ") and find tab-separated values
        let trimmed = stripped.trim_start_matches(|c: char| !c.is_ascii_digit() && c != '-');

        // Must have at least two tab-separated fields
        let mut parts = trimmed.split('\t');
        let added_str = parts.next()?;
        let deleted_str = parts.next()?;

        // "-" means binary file, treat as 0
        let added = if added_str == "-" {
            0
        } else {
            added_str.parse().ok()?
        };
        let deleted = if deleted_str == "-" {
            0
        } else {
            deleted_str.parse().ok()?
        };

        Some((added, deleted))
    }
}

pub fn handle_select(is_directive_mode: bool) -> anyhow::Result<()> {
    let repo = Repository::current();

    // Initialize preview mode state file (auto-cleanup on drop)
    let state = PreviewState::new();

    // Load config (or use default) for path mismatch detection
    let config = WorktrunkConfig::load()
        .inspect_err(|e| log::warn!("Config load failed, using defaults: {}", e))
        .unwrap_or_default();

    // Gather list data using simplified collection (buffered mode)
    // Skip expensive operations not needed for select UI
    let skip_tasks = [
        collect::TaskKind::BranchDiff,
        collect::TaskKind::CiStatus,
        collect::TaskKind::MergeTreeConflicts,
    ]
    .into_iter()
    .collect();

    let Some(list_data) = collect::collect(
        &repo,
        true,  // show_branches (include branches without worktrees)
        false, // show_remotes (local branches only, not remote branches)
        &skip_tasks,
        false, // show_progress (no progress bars)
        false, // render_table (select renders its own UI)
        &config,
    )?
    else {
        return Ok(());
    };

    // Use the same layout system as `wt list` for proper column alignment
    // List width depends on preview position:
    // - Right layout: skim splits ~50% for list, ~50% for preview
    // - Down layout: list gets full width, preview is below
    let terminal_width = super::list::layout::get_safe_list_width();
    let skim_list_width = match state.initial_layout {
        PreviewLayout::Right => terminal_width / 2,
        PreviewLayout::Down => terminal_width,
    };
    let layout = super::list::layout::calculate_layout_with_width(
        &list_data.items,
        &skip_tasks,
        skim_list_width,
    );

    // Render header using layout system (need both plain and styled text for skim)
    let header_line = layout.render_header_line();
    let header_display_text = header_line.render();
    let header_plain_text = header_line.plain_text();

    // Convert to skim items using the layout system for rendering
    let mut items: Vec<Arc<dyn SkimItem>> = list_data
        .items
        .into_iter()
        .map(|item| {
            let branch_name = item.branch_name().to_string();

            // Use layout system to render the line - this handles all column alignment
            let rendered_line = layout.render_list_item_line(&item, None);
            let display_text_with_ansi = rendered_line.render();
            let display_text = rendered_line.plain_text();

            Arc::new(WorktreeSkimItem {
                display_text,
                display_text_with_ansi,
                branch_name,
                item: Arc::new(item),
            }) as Arc<dyn SkimItem>
        })
        .collect();

    // Insert header row at the beginning (will be non-selectable via header_lines option)
    items.insert(
        0,
        Arc::new(HeaderSkimItem {
            display_text: header_plain_text,
            display_text_with_ansi: header_display_text,
        }) as Arc<dyn SkimItem>,
    );

    // Get state path for key bindings
    let state_path_str = state.path.display().to_string();

    // Calculate half-page scroll: skim uses 90% of terminal height, half of that = 45%
    let half_page = terminal_size::terminal_size()
        .map(|(_, terminal_size::Height(h))| (h as usize * 45 / 100).max(5))
        .unwrap_or(10);

    // Calculate preview window spec based on auto-detected layout
    // items.len() - 1 because we added a header row
    let num_items = items.len().saturating_sub(1);
    let preview_window_spec = state.initial_layout.to_preview_window_spec(num_items);

    // Configure skim options with Rust-based preview and mode switching keybindings
    let options = SkimOptionsBuilder::default()
        .height("90%".to_string())
        .layout("reverse".to_string())
        .header_lines(1) // Make first line (header) non-selectable
        .multi(false)
        .no_info(true) // Hide info line (matched/total counter)
        .preview(Some("".to_string())) // Enable preview (empty string means use SkimItem::preview())
        .preview_window(preview_window_spec)
        // Color scheme using fzf's --color=light values: dark text (237) on light gray bg (251)
        //
        // Terminal color compatibility is tricky:
        // - current_bg:254 (original): too bright on dark terminals, washes out text
        // - current_bg:236 (fzf dark): too dark on light terminals, jarring contrast
        // - current_bg:251 + current:-1: light bg works on both, but unstyled text
        //   becomes unreadable on dark terminals (light-on-light)
        // - current_bg:251 + current:237: fzf's light theme, best compromise
        //
        // The light theme works universally because:
        // - On dark terminals: light gray highlight stands out clearly
        // - On light terminals: light gray is subtle but visible
        // - Dark text (237) ensures readability regardless of terminal theme
        .color(Some(
            "fg:-1,bg:-1,header:-1,matched:108,current:237,current_bg:251,current_match:108"
                .to_string(),
        ))
        .bind(vec![
            // Mode switching (1/2/3 keys change preview content)
            format!(
                "1:execute-silent(echo 1 > {0})+refresh-preview",
                state_path_str
            ),
            format!(
                "2:execute-silent(echo 2 > {0})+refresh-preview",
                state_path_str
            ),
            format!(
                "3:execute-silent(echo 3 > {0})+refresh-preview",
                state_path_str
            ),
            // Preview toggle (alt-p shows/hides preview)
            // Note: skim doesn't support change-preview-window like fzf, only toggle
            "alt-p:toggle-preview".to_string(),
            // Preview scrolling (half-page based on terminal height)
            format!("ctrl-u:preview-up({half_page})"),
            format!("ctrl-d:preview-down({half_page})"),
        ])
        // Legend/controls moved to preview window tabs (render_preview_tabs)
        .no_clear(true) // Prevent skim from clearing screen, we'll do it manually
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build skim options: {}", e))?;

    // Create item receiver
    let (tx, rx): (SkimItemSender, SkimItemReceiver) = unbounded();
    for item in items {
        tx.send(item)
            .map_err(|e| anyhow::anyhow!("Failed to send item to skim: {}", e))?;
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
        let config = WorktrunkConfig::load().context("Failed to load config")?;

        // Switch to the selected worktree
        // handle_switch can handle both branch names and worktree paths
        let (result, branch_info) = handle_switch(&identifier, false, None, false, false, &config)?;

        // Clear the terminal screen after skim exits to prevent artifacts
        // Use stderr for terminal control sequences - in directive mode, stdout goes to a FIFO
        // for directive parsing, so terminal control must go through stderr to reach the TTY
        use crossterm::{execute, terminal};
        use std::io::stderr;
        execute!(stderr(), terminal::Clear(terminal::ClearType::All))?;
        execute!(stderr(), crossterm::cursor::MoveTo(0, 0))?;

        // Show success message; emit cd directive if in directive mode
        handle_switch_output(&result, &branch_info, false, is_directive_mode)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_preview_mode_from_u8() {
        assert_eq!(PreviewMode::from_u8(1), PreviewMode::WorkingTree);
        assert_eq!(PreviewMode::from_u8(2), PreviewMode::Log);
        assert_eq!(PreviewMode::from_u8(3), PreviewMode::BranchDiff);
        // Invalid values default to WorkingTree
        assert_eq!(PreviewMode::from_u8(0), PreviewMode::WorkingTree);
        assert_eq!(PreviewMode::from_u8(99), PreviewMode::WorkingTree);
    }

    #[test]
    fn test_preview_layout_to_preview_window_spec() {
        // Right is always 50%
        assert_eq!(PreviewLayout::Right.to_preview_window_spec(10), "right:50%");

        // Down calculates based on item count
        let spec = PreviewLayout::Down.to_preview_window_spec(5);
        assert!(spec.starts_with("down:"));
    }

    #[test]
    fn test_preview_state_data_read_default() {
        // Use unique path to avoid interference from parallel tests
        let state_path = std::env::temp_dir().join("wt-test-read-default");
        let _ = fs::remove_file(&state_path);

        // When state file doesn't exist, read returns default
        let mode = fs::read_to_string(&state_path)
            .ok()
            .and_then(|s| s.trim().parse::<u8>().ok())
            .map(PreviewMode::from_u8)
            .unwrap_or(PreviewMode::WorkingTree);
        assert_eq!(mode, PreviewMode::WorkingTree);
    }

    #[test]
    fn test_preview_state_data_roundtrip() {
        // Use unique path to avoid interference from parallel tests
        let state_path = std::env::temp_dir().join("wt-test-roundtrip");

        // Write and read back various modes
        let _ = fs::write(&state_path, "1");
        let mode = fs::read_to_string(&state_path)
            .ok()
            .and_then(|s| s.trim().parse::<u8>().ok())
            .map(PreviewMode::from_u8)
            .unwrap_or(PreviewMode::WorkingTree);
        assert_eq!(mode, PreviewMode::WorkingTree);

        let _ = fs::write(&state_path, "2");
        let mode = fs::read_to_string(&state_path)
            .ok()
            .and_then(|s| s.trim().parse::<u8>().ok())
            .map(PreviewMode::from_u8)
            .unwrap_or(PreviewMode::WorkingTree);
        assert_eq!(mode, PreviewMode::Log);

        let _ = fs::write(&state_path, "3");
        let mode = fs::read_to_string(&state_path)
            .ok()
            .and_then(|s| s.trim().parse::<u8>().ok())
            .map(PreviewMode::from_u8)
            .unwrap_or(PreviewMode::WorkingTree);
        assert_eq!(mode, PreviewMode::BranchDiff);

        // Cleanup
        let _ = fs::remove_file(&state_path);
    }

    #[test]
    fn test_pager_needs_paging_disabled() {
        // delta - plain command name
        assert!(pager_needs_paging_disabled("delta"));
        // delta - with arguments
        assert!(pager_needs_paging_disabled("delta --side-by-side"));
        assert!(pager_needs_paging_disabled("delta --paging=always"));
        // delta - full path
        assert!(pager_needs_paging_disabled("/usr/bin/delta"));
        assert!(pager_needs_paging_disabled(
            "/opt/homebrew/bin/delta --line-numbers"
        ));
        // bat - also spawns less by default
        assert!(pager_needs_paging_disabled("bat"));
        assert!(pager_needs_paging_disabled("/usr/bin/bat"));
        assert!(pager_needs_paging_disabled("bat --style=plain"));
        // Pagers that don't spawn sub-pagers
        assert!(!pager_needs_paging_disabled("less"));
        assert!(!pager_needs_paging_disabled("diff-so-fancy"));
        assert!(!pager_needs_paging_disabled("colordiff"));
        // Edge cases - similar names but not delta/bat
        assert!(!pager_needs_paging_disabled("delta-preview"));
        assert!(!pager_needs_paging_disabled("/path/to/delta-preview"));
        assert!(pager_needs_paging_disabled("batcat")); // Debian's bat package name
    }

    #[test]
    fn test_shell_escape_simple() {
        // Simple strings pass through unchanged
        assert_eq!(shell_escape("hello"), "hello");
        assert_eq!(shell_escape("foo-bar"), "foo-bar");
        assert_eq!(shell_escape("path/to/file"), "path/to/file");
    }

    #[test]
    fn test_shell_escape_with_spaces() {
        // Strings with spaces get quoted
        assert_eq!(shell_escape("hello world"), "'hello world'");
        assert_eq!(shell_escape("path/to/my file"), "'path/to/my file'");
    }

    #[test]
    fn test_shell_escape_with_special_chars() {
        // Special shell characters get quoted
        assert_eq!(shell_escape("foo$bar"), "'foo$bar'");
        assert_eq!(shell_escape("foo`bar"), "'foo`bar'");
        assert_eq!(shell_escape("foo;bar"), "'foo;bar'");
        assert_eq!(shell_escape("foo|bar"), "'foo|bar'");
        assert_eq!(shell_escape("foo&bar"), "'foo&bar'");
        assert_eq!(shell_escape("foo*bar"), "'foo*bar'");
        assert_eq!(shell_escape("foo?bar"), "'foo?bar'");
    }

    #[test]
    fn test_shell_escape_with_quotes() {
        // Single quotes get escaped specially
        assert_eq!(shell_escape("it's"), "'it'\\''s'");
        assert_eq!(shell_escape("don't"), "'don'\\''t'");
        // Double quotes trigger quoting but don't need internal escaping
        assert_eq!(shell_escape("say \"hi\""), "'say \"hi\"'");
    }

    #[test]
    fn test_render_preview_tabs_working_tree_mode() {
        let output = WorktreeSkimItem::render_preview_tabs(PreviewMode::WorkingTree);
        // Tab 1 should be bold (active), tabs 2 and 3 dimmed
        assert!(output.contains("1: HEAD±"));
        assert!(output.contains("2: log"));
        assert!(output.contains("3: main…±"));
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
    }

    #[test]
    fn test_render_preview_tabs_branch_diff_mode() {
        let output = WorktreeSkimItem::render_preview_tabs(PreviewMode::BranchDiff);
        assert!(output.contains("1: HEAD±"));
        assert!(output.contains("2: log"));
        assert!(output.contains("3: main…±"));
    }

    #[test]
    fn test_parse_numstat_line_basic() {
        // Tab-separated: added<TAB>deleted<TAB>filename
        let result = WorktreeSkimItem::parse_numstat_line("10\t5\tfile.rs");
        assert_eq!(result, Some((10, 5)));
    }

    #[test]
    fn test_parse_numstat_line_insertions_only() {
        let result = WorktreeSkimItem::parse_numstat_line("15\t0\tfile.rs");
        assert_eq!(result, Some((15, 0)));
    }

    #[test]
    fn test_parse_numstat_line_deletions_only() {
        let result = WorktreeSkimItem::parse_numstat_line("0\t8\tfile.rs");
        assert_eq!(result, Some((0, 8)));
    }

    #[test]
    fn test_parse_numstat_line_binary_file() {
        // Binary files show "-" instead of numbers
        let result = WorktreeSkimItem::parse_numstat_line("-\t-\timage.png");
        assert_eq!(result, Some((0, 0)));
    }

    #[test]
    fn test_parse_numstat_line_with_graph_prefix() {
        // Git graph prefixes the numstat line with graph characters
        let result = WorktreeSkimItem::parse_numstat_line("| 10\t5\tfile.rs");
        assert_eq!(result, Some((10, 5)));

        // First numstat line after commit has "* | " prefix
        let result = WorktreeSkimItem::parse_numstat_line("* | 11\t0\tCargo.toml");
        assert_eq!(result, Some((11, 0)));

        // Subsequent numstat lines have "| " prefix
        let result = WorktreeSkimItem::parse_numstat_line("| 17\t3\tsrc/main.rs");
        assert_eq!(result, Some((17, 3)));

        // With ANSI colors (--color=always adds escape codes to graph)
        // ESC[31m = red, ESC[m = reset
        let esc = '\x1b';
        let ansi_colored = format!("{esc}[31m|{esc}[m 11\t0\tCargo.toml");
        let result = WorktreeSkimItem::parse_numstat_line(&ansi_colored);
        assert_eq!(result, Some((11, 0)));
    }

    #[test]
    fn test_parse_numstat_line_not_numstat() {
        // Not a numstat line
        assert_eq!(
            WorktreeSkimItem::parse_numstat_line("* abc1234 Fix bug"),
            None
        );
        assert_eq!(WorktreeSkimItem::parse_numstat_line(""), None);
        assert_eq!(WorktreeSkimItem::parse_numstat_line("regular text"), None);
    }
}
