use anyhow::Context;
use color_print::cformat;
use skim::prelude::*;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use worktrunk::config::WorktrunkConfig;
use worktrunk::git::{Repository, parse_numstat_line};
use worktrunk::shell::extract_filename_from_path;
use worktrunk::shell_exec::Cmd;

use super::list::collect;
use super::list::layout::{DiffDisplayConfig, DiffVariant};
use super::list::model::ListItem;
use super::worktree::{execute_switch, plan_switch};
use crate::output::handle_switch_output;
use crate::pager::{git_config_pager, parse_pager_value};

/// Cached pager command, detected once at startup.
///
/// None means no pager should be used (empty config or "cat").
/// We cache this to avoid running `git config` on every preview render.
static CACHED_PAGER: OnceLock<Option<String>> = OnceLock::new();

/// Get the cached pager command, initializing if needed.
///
/// Precedence (highest to lowest):
/// 1. `[select] pager` in user config (explicit override, used as-is)
/// 2. `GIT_PAGER` environment variable (with auto-detection applied)
/// 3. `core.pager` git config (with auto-detection applied)
fn get_diff_pager() -> Option<&'static String> {
    CACHED_PAGER
        .get_or_init(|| {
            // Check user config first for explicit pager override
            // When set, use exactly as specified (no auto-detection)
            if let Ok(config) = WorktrunkConfig::load()
                && let Some(select_config) = config.select
                && let Some(pager) = select_config.pager
                && !pager.trim().is_empty()
            {
                return Some(pager);
            }

            // GIT_PAGER takes precedence over core.pager
            if let Ok(pager) = std::env::var("GIT_PAGER") {
                return parse_pager_value(&pager);
            }

            // Fall back to core.pager config
            git_config_pager()
        })
        .as_ref()
}

/// Check if the pager spawns its own internal pager (e.g., less).
///
/// Some pagers like delta and bat spawn `less` by default, which hangs in
/// non-TTY contexts like skim's preview panel. These need `--paging=never`.
///
/// Used only when user hasn't set `[select] pager` config explicitly.
/// When config is set, that value is used as-is without modification.
fn pager_needs_paging_disabled(pager_cmd: &str) -> bool {
    // Split on whitespace to get the command name, then extract basename
    // Uses extract_filename_from_path for consistent handling of Windows paths and .exe
    pager_cmd
        .split_whitespace()
        .next()
        .and_then(extract_filename_from_path)
        // bat is called "batcat" on Debian/Ubuntu
        // Case-insensitive for Windows where commands might be Delta.exe, BAT.EXE, etc.
        .is_some_and(|basename| {
            basename.eq_ignore_ascii_case("delta")
                || basename.eq_ignore_ascii_case("bat")
                || basename.eq_ignore_ascii_case("batcat")
        })
}

/// Check if user has explicitly configured a select-specific pager.
fn has_explicit_pager_config() -> bool {
    WorktrunkConfig::load()
        .ok()
        .and_then(|config| config.select)
        .and_then(|select| select.pager)
        .is_some_and(|p| !p.trim().is_empty())
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
/// buffering. Returns None if pipeline fails or times out (caller should fall back to raw diff).
///
/// When `[select] pager` is not configured, automatically appends `--paging=never` for
/// delta/bat/batcat pagers to prevent hangs. To override this behavior, set an explicit
/// pager command in config: `[select] pager = "delta"` (or with custom flags).
fn run_git_diff_with_pager(git_args: &[&str], pager_cmd: &str) -> Option<String> {
    // Note: pager_cmd is expected to be valid shell code (like git's core.pager).
    // Users with paths containing special chars must quote them in their config.

    // Apply auto-detection only when user hasn't set explicit config
    // If config is set, use the value as-is (user has full control)
    let pager_with_args = if !has_explicit_pager_config() && pager_needs_paging_disabled(pager_cmd)
    {
        format!("{} --paging=never", pager_cmd)
    } else {
        pager_cmd.to_string()
    };

    // Build shell pipeline: git <args> | pager
    // Shell-escape args to handle paths with spaces
    let escaped_args: Vec<String> = git_args
        .iter()
        .map(|arg| shlex::try_quote(arg).unwrap_or((*arg).into()).into_owned())
        .collect();
    let pipeline = format!("git {} | {}", escaped_args.join(" "), pager_with_args);

    log::debug!("Running pager pipeline: {}", pipeline);

    // Spawn pipeline
    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(&pipeline)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        // Prevent subprocesses from writing to the directive file
        .env_remove(worktrunk::shell_exec::DIRECTIVE_FILE_ENV_VAR)
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

/// Preview modes for the interactive selector
///
/// Each mode shows a different aspect of the worktree:
/// 1. WorkingTree: Uncommitted changes (git diff HEAD --stat)
/// 2. Log: Commit history since diverging from the default branch (git log with merge-base)
/// 3. BranchDiff: Line diffs since the merge-base with the default branch (git diff --stat DEFAULT…)
/// 4. UpstreamDiff: Diff vs upstream tracking branch (ahead/behind)
///
/// Loosely aligned with `wt list` columns, though not a perfect match:
/// - Tab 1 corresponds to "HEAD±" column
/// - Tab 2 shows commits (related to "main↕" counts)
/// - Tab 3 corresponds to "main…± (--full)" column
/// - Tab 4 corresponds to "Remote⇅" column
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewMode {
    WorkingTree = 1,
    Log = 2,
    BranchDiff = 3,
    UpstreamDiff = 4,
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
            4 => Self::UpstreamDiff,
            _ => Self::WorkingTree,
        }
    }
}

/// Preview state persistence (mode only, layout auto-detected)
///
/// State file format: Single digit representing preview mode (1-4)
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
            "<dim,yellow>Enter: switch | Esc: cancel | ctrl-u/d: scroll | alt-p: toggle</>"
        );

        format!(
            "{} | {} | {} | {}\n{}\n\n",
            tab1, tab2, tab3, tab4, controls
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
        use worktrunk::styling::INFO_SYMBOL;

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
        use worktrunk::styling::INFO_SYMBOL;

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
        use worktrunk::styling::INFO_SYMBOL;

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
        use worktrunk::styling::INFO_SYMBOL;
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

/// Batch fetch diffstats for multiple commits using git diff-tree --stdin.
/// Returns a map of full_hash -> (insertions, deletions).
///
/// Failures are silent (preview context).
fn batch_fetch_stats(repo: &Repository, hashes: &[String]) -> HashMap<String, (usize, usize)> {
    if hashes.is_empty() {
        return HashMap::new();
    }

    // --root: include stats for root commits (no parent to diff against)
    // Each hash needs a trailing newline for git to process it
    let stdin_data = hashes.iter().map(|h| format!("{h}\n")).collect::<String>();
    let Ok(output) = Cmd::new("git")
        .args(["diff-tree", "--numstat", "-r", "--root", "--stdin"])
        .current_dir(repo.worktree_base().unwrap_or_else(|_| ".".into()))
        .stdin_bytes(stdin_data)
        .run()
    else {
        return HashMap::new();
    };

    // Parse output: hash line followed by numstat lines
    let mut stats: HashMap<String, (usize, usize)> = HashMap::new();
    let mut current_hash: Option<String> = None;
    let mut current_stats = (0usize, 0usize);

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        // Hash line (40 or 64 hex chars)
        if line.chars().all(|c| c.is_ascii_hexdigit()) && (line.len() == 40 || line.len() == 64) {
            // Save previous hash's stats
            if let Some(hash) = current_hash.take() {
                stats.insert(hash, current_stats);
            }
            current_hash = Some(line.to_string());
            current_stats = (0, 0);
        } else if let Some((ins, del)) = parse_numstat_line(line) {
            current_stats.0 += ins;
            current_stats.1 += del;
        }
    }

    // Don't forget the last hash
    if let Some(hash) = current_hash {
        stats.insert(hash, current_stats);
    }

    stats
}

/// Field delimiter for git log format with timestamps
const FIELD_DELIM: char = '\x1f';

/// Start delimiter for full hash (SOH - Start of Heading)
const HASH_START: char = '\x01';

/// End delimiter for full hash (NUL)
const HASH_END: char = '\x00';

/// Timestamp column width ("12mo" is the longest)
const TIMESTAMP_WIDTH: usize = 4;

/// Process git log output: strip hash prefix and dim non-unique commits.
///
/// - `unique_commits = None`: show everything bright (default branch)
/// - `unique_commits = Some(set)`: bright if in set, dim otherwise
/// - Graph-only lines pass through unchanged
///
/// Returns (processed_output, list_of_full_hashes) for batch stats lookup.
fn process_log_with_dimming(
    log_output: &str,
    unique_commits: Option<&HashSet<String>>,
) -> (String, Vec<String>) {
    use ansi_str::AnsiStr;
    use std::fmt::Write;

    let dim = anstyle::Style::new().dimmed();
    let reset = anstyle::Reset;

    let mut result = String::with_capacity(log_output.len());
    let mut hashes = Vec::new();

    for (i, line) in log_output.lines().enumerate() {
        if i > 0 {
            result.push('\n');
        }

        // Parse commit line: graph_prefix + SOH + full_hash + NUL + display
        if let Some(hash_start) = line.find(HASH_START)
            && let Some(hash_end_offset) = line[hash_start + 1..].find(HASH_END)
        {
            let hash_end = hash_start + 1 + hash_end_offset;
            let graph_prefix = &line[..hash_start];
            let full_hash = &line[hash_start + 1..hash_end];
            let display = &line[hash_end + 1..];

            // Collect hash for stats lookup
            hashes.push(full_hash.to_string());

            // Bright if: no dimming (None) OR commit is in unique set
            let is_bright = match unique_commits {
                None => true,                         // Default branch: all bright
                Some(set) => set.contains(full_hash), // Feature branch: bright if unique
            };

            // Keep SOH hash NUL markers for format_log_output to extract hash for stats lookup
            if is_bright {
                result.push_str(graph_prefix);
                result.push(HASH_START);
                result.push_str(full_hash);
                result.push(HASH_END);
                result.push_str(display);
            } else {
                // Dim: strip colors and wrap in dim style, but keep hash markers
                let _ = write!(
                    result,
                    "{}{HASH_START}{full_hash}{HASH_END}{dim}{}{reset}",
                    graph_prefix,
                    display.ansi_strip()
                );
            }
            continue;
        }
        // Graph-only lines: pass through unchanged
        result.push_str(line);
    }
    (result, hashes)
}

/// Format git log output with timestamps and diffstats.
///
/// Takes pre-processed log output (graph + commits) and a stats map.
/// Each commit line has format: `graph_prefix short_hash \x1f timestamp \x1f decoration message`
///
/// The full hash for stats lookup is embedded as: `SOH full_hash NUL` before the short hash,
/// but this was already stripped by process_log_with_dimming. We need the hash in the line
/// to look up stats - so we keep the full hash in a different delimiter for this function.
fn format_log_output(log_output: &str, stats: &HashMap<String, (usize, usize)>) -> String {
    use crate::display::format_relative_time_short;
    format_log_output_with_formatter(log_output, stats, format_relative_time_short)
}

/// Format git log output with a custom time formatter.
///
/// This variant allows dependency injection for testing with deterministic timestamps.
fn format_log_output_with_formatter<F>(
    log_output: &str,
    stats: &HashMap<String, (usize, usize)>,
    format_time: F,
) -> String
where
    F: Fn(i64) -> String,
{
    use ansi_str::AnsiStr;
    use unicode_width::UnicodeWidthStr;

    // First pass: find max display width of graph+hash prefix for alignment
    let max_prefix_width = log_output
        .lines()
        .filter(|line| line.contains(FIELD_DELIM))
        .filter_map(|line| {
            let first_delim = line.find(FIELD_DELIM)?;
            let graph_hash_raw = &line[..first_delim];
            let graph_hash = strip_hash_markers(graph_hash_raw);
            // Calculate display width (strip ANSI, measure unicode width)
            Some(graph_hash.ansi_strip().width())
        })
        .max()
        .unwrap_or(0);

    // Second pass: format with alignment
    let mut result = Vec::new();
    for line in log_output.lines() {
        if line.contains(FIELD_DELIM) {
            // Commit line - look up stats by hash extracted from line
            let commit_stats = extract_hash_from_line(line)
                .and_then(|h| stats.get(h))
                .copied()
                .unwrap_or((0, 0));
            result.push(format_commit_line(
                line,
                commit_stats,
                max_prefix_width,
                &format_time,
            ));
        } else {
            // Graph-only line - pass through
            result.push(line.to_string());
        }
    }

    result.join("\n")
}

/// Extract the full hash from a commit line that still has SOH/NUL markers.
/// Returns None if not found (line already processed or malformed).
fn extract_hash_from_line(line: &str) -> Option<&str> {
    let hash_start = line.find(HASH_START)?;
    let hash_end_offset = line[hash_start + 1..].find(HASH_END)?;
    Some(&line[hash_start + 1..hash_start + 1 + hash_end_offset])
}

/// Strip SOH...NUL hash markers from output (used when not formatting with timestamps).
fn strip_hash_markers(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == HASH_START {
            // Skip until NUL
            while let Some(&next) = chars.peek() {
                chars.next();
                if next == HASH_END {
                    break;
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}

/// Format a single commit line with stats, padding the prefix to target_width for alignment
fn format_commit_line<F>(
    commit_line: &str,
    (insertions, deletions): (usize, usize),
    target_width: usize,
    format_time: &F,
) -> String
where
    F: Fn(i64) -> String,
{
    use ansi_str::AnsiStr;
    use unicode_width::UnicodeWidthStr;
    use worktrunk::styling::{ADDITION, DELETION};

    let dim_style = anstyle::Style::new().dimmed();
    let reset = anstyle::Reset;

    if let Some(first_delim) = commit_line.find(FIELD_DELIM)
        && let Some(second_delim) = commit_line[first_delim + 1..].find(FIELD_DELIM)
    {
        let graph_hash_raw = &commit_line[..first_delim];
        // Strip SOH...NUL hash markers from graph_hash portion
        let graph_hash = strip_hash_markers(graph_hash_raw);
        let timestamp_str = &commit_line[first_delim + 1..first_delim + 1 + second_delim];
        let rest = &commit_line[first_delim + 1 + second_delim + 1..];

        let time = timestamp_str
            .parse::<i64>()
            .map(format_time)
            .unwrap_or_default();

        // Use the same diff formatting as wt list (aligned columns)
        let diff_config = DiffDisplayConfig {
            variant: DiffVariant::Signs,
            positive_style: ADDITION,
            negative_style: DELETION,
            always_show_zeros: false,
        };
        let stat_str = format!(" {}", diff_config.format_aligned(insertions, deletions));

        // Pad graph_hash to target_width for column alignment
        let current_width = graph_hash.ansi_strip().width();
        let padding = " ".repeat(target_width.saturating_sub(current_width));

        format!(
            "{}{}{} {dim_style}{:>width$}{reset}{}",
            graph_hash,
            padding,
            stat_str,
            time,
            rest,
            width = TIMESTAMP_WIDTH
        )
    } else {
        commit_line.to_string()
    }
}

pub fn handle_select(
    show_branches: bool,
    show_remotes: bool,
    config: &WorktrunkConfig,
) -> anyhow::Result<()> {
    use std::io::IsTerminal;

    // Select requires an interactive terminal for the TUI
    if !std::io::stdin().is_terminal() {
        anyhow::bail!("wt select requires an interactive terminal");
    }

    let repo = Repository::current()?;

    // Initialize preview mode state file (auto-cleanup on drop)
    let state = PreviewState::new();

    // Gather list data using simplified collection (buffered mode)
    // Skip expensive operations not needed for select UI
    let skip_tasks = [
        collect::TaskKind::BranchDiff,
        collect::TaskKind::CiStatus,
        collect::TaskKind::MergeTreeConflicts,
    ]
    .into_iter()
    .collect();

    // Use 500ms timeout for git commands to show TUI faster on large repos.
    // Typical slow operations: merge-tree ~400-1800ms, rev-list ~200-600ms.
    // 500ms allows most operations to complete while cutting off tail latency.
    // Operations that timeout fail silently (data not shown), but TUI stays responsive.
    let command_timeout = Some(std::time::Duration::from_millis(500));

    let Some(list_data) = collect::collect(
        &repo,
        show_branches,
        show_remotes,
        &skip_tasks,
        false, // show_progress (no progress bars)
        false, // render_table (select renders its own UI)
        config,
        command_timeout,
        true, // skip_expensive_for_stale (faster for repos with many stale branches)
    )?
    else {
        return Ok(());
    };

    // Use the same layout system as `wt list` for proper column alignment
    // List width depends on preview position:
    // - Right layout: skim splits ~50% for list, ~50% for preview
    // - Down layout: list gets full width, preview is below
    let terminal_width = crate::display::get_terminal_width();
    let skim_list_width = match state.initial_layout {
        PreviewLayout::Right => terminal_width / 2,
        PreviewLayout::Down => terminal_width,
    };
    let layout = super::list::layout::calculate_layout_with_width(
        &list_data.items,
        &skip_tasks,
        skim_list_width,
        &list_data.main_worktree_path,
        None, // URL column not shown in select
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
            let rendered_line = layout.render_list_item_line(&item);
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

    // Get state path for key bindings (shell-escaped for safety)
    let state_path_display = state.path.display().to_string();
    let state_path_str = shlex::try_quote(&state_path_display)
        .map(|s| s.into_owned())
        .unwrap_or(state_path_display);

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
            // Mode switching (1/2/3/4 keys change preview content)
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
            format!(
                "4:execute-silent(echo 4 > {0})+refresh-preview",
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
        let repo = Repository::current().context("Failed to switch worktree")?;

        // Switch to the selected worktree (no creation, no approval prompts)
        let plan = plan_switch(&repo, &identifier, false, None, false, &config)?;
        let (result, branch_info) = execute_switch(&repo, plan, &config, false, true)?;

        // Clear the terminal screen after skim exits to prevent artifacts
        // Use stderr for terminal control - stdout is reserved for data output
        use crossterm::{execute, terminal};
        use std::io::stderr;
        execute!(stderr(), terminal::Clear(terminal::ClearType::All))?;
        execute!(stderr(), crossterm::cursor::MoveTo(0, 0))?;

        // Show success message; emit cd directive if shell integration is active
        handle_switch_output(&result, &branch_info, None)?;
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
        assert_eq!(PreviewMode::from_u8(4), PreviewMode::UpstreamDiff);
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

        let _ = fs::write(&state_path, "4");
        let mode = fs::read_to_string(&state_path)
            .ok()
            .and_then(|s| s.trim().parse::<u8>().ok())
            .map(PreviewMode::from_u8)
            .unwrap_or(PreviewMode::WorkingTree);
        assert_eq!(mode, PreviewMode::UpstreamDiff);

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

        // Case-insensitive matching (Windows command names)
        assert!(pager_needs_paging_disabled("Delta"));
        assert!(pager_needs_paging_disabled("DELTA"));
        assert!(pager_needs_paging_disabled("BAT"));
        assert!(pager_needs_paging_disabled("Bat"));
        assert!(pager_needs_paging_disabled("BatCat"));
        assert!(pager_needs_paging_disabled("delta.exe"));
        assert!(pager_needs_paging_disabled("Delta.EXE"));
    }

    #[test]
    fn test_has_explicit_pager_config() {
        // This function loads real config, so we just test that it doesn't panic
        // The behavior is covered by integration tests that set actual config
        let _ = has_explicit_pager_config();
    }

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

    // format_log_output tests use dependency injection for deterministic time formatting.
    // The format_log_output_with_formatter function accepts a time formatter closure.

    /// Fixed time formatter for deterministic tests
    fn fixed_time_formatter(_timestamp: i64) -> String {
        "1h".to_string() // Return a fixed time for all timestamps
    }

    /// Create a stats map with a single entry
    fn stats_for(
        hash: &str,
        insertions: usize,
        deletions: usize,
    ) -> HashMap<String, (usize, usize)> {
        let mut map = HashMap::new();
        map.insert(hash.to_string(), (insertions, deletions));
        map
    }

    /// Create a stats map with multiple entries
    fn multi_stats(entries: &[(&str, usize, usize)]) -> HashMap<String, (usize, usize)> {
        entries
            .iter()
            .map(|(h, i, d)| (h.to_string(), (*i, *d)))
            .collect()
    }

    #[test]
    fn test_format_log_output_single_commit() {
        // Simulate git log output with SOH/NUL markers: * SOH full_hash NUL short_hash \x1f timestamp \x1f message
        let full_hash = "abc1234567890123456789012345678901234567ab";
        let input = format!("* \x01{}\x00abc1234\x1f1699999000\x1f Fix bug", full_hash);
        let stats = stats_for(full_hash, 5, 2);
        let output = format_log_output_with_formatter(&input, &stats, fixed_time_formatter);

        // Should contain the hash and message
        assert!(output.contains("abc1234"));
        assert!(output.contains("Fix bug"));
        // Should contain formatted time
        assert!(output.contains("1h"));
        // Should contain stats
        assert!(output.contains("+5"), "expected +5 in output: {}", output);
    }

    #[test]
    fn test_format_log_output_with_stats() {
        // Commit line with pre-computed stats
        let full_hash = "abc1234567890123456789012345678901234567ab";
        let input = format!(
            "* \x01{}\x00abc1234\x1f1699999000\x1f Add feature",
            full_hash
        );
        // Stats are pre-computed (accumulated from numstat lines)
        let stats = stats_for(full_hash, 13, 5);
        let output = format_log_output_with_formatter(&input, &stats, fixed_time_formatter);

        // Should contain the hash and message
        assert!(output.contains("abc1234"));
        assert!(output.contains("Add feature"));
        // Verify stats are present (green +13, red -5)
        assert!(output.contains("+13"), "expected +13 in output: {}", output);
        assert!(output.contains("-5"), "expected -5 in output: {}", output);
    }

    #[test]
    fn test_format_log_output_multiple_commits() {
        // Two commits with pre-computed stats
        let hash1 = "abc1234567890123456789012345678901234567ab";
        let hash2 = "def5678901234567890123456789012345678901cd";
        let input = format!(
            "* \x01{}\x00abc1234\x1f1699999000\x1f First commit\n\
             * \x01{}\x00def5678\x1f1699998000\x1f Second commit",
            hash1, hash2
        );
        let stats = multi_stats(&[(hash1, 5, 2), (hash2, 10, 3)]);
        let output = format_log_output_with_formatter(&input, &stats, fixed_time_formatter);

        // Both commits should be in output
        assert!(output.contains("abc1234"));
        assert!(output.contains("def5678"));
        assert!(output.contains("First commit"));
        assert!(output.contains("Second commit"));

        // Output should be two lines (one per commit)
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 2, "Expected 2 lines, got: {:?}", lines);
    }

    #[test]
    fn test_format_log_output_empty_input() {
        let stats = HashMap::new();
        let output = format_log_output_with_formatter("", &stats, fixed_time_formatter);
        assert!(output.is_empty());
    }

    #[test]
    fn test_format_log_output_preserves_graph_lines() {
        // Merge commit with graph continuation line between commits
        let hash1 = "abc1234567890123456789012345678901234567ab";
        let hash2 = "def5678901234567890123456789012345678901cd";
        let input = format!(
            "*   \x01{}\x00abc1234\x1f1699999000\x1f Merge branch\n\
             |\\  \n\
             | * \x01{}\x00def5678\x1f1699998000\x1f Feature commit",
            hash1, hash2
        );
        let stats = multi_stats(&[(hash1, 0, 0), (hash2, 5, 2)]);
        let output = format_log_output_with_formatter(&input, &stats, fixed_time_formatter);

        // Graph line should be preserved between commits
        assert!(output.contains("|\\"), "graph line should be preserved");
        assert!(output.contains("abc1234"), "first commit should be present");
        assert!(
            output.contains("def5678"),
            "second commit should be present"
        );

        // Verify order: merge commit, graph line, feature commit
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 3, "Expected 3 lines: {:?}", lines);
        assert!(lines[0].contains("abc1234"));
        assert!(lines[1].contains("\\"));
        assert!(lines[2].contains("def5678"));
    }

    #[test]
    fn test_format_log_output_no_stats() {
        // Commit without stats (not in stats map)
        let full_hash = "abc1234567890123456789012345678901234567ab";
        let input = format!(
            "* \x01{}\x00abc1234\x1f1699999000\x1f Just a commit",
            full_hash
        );
        let stats = HashMap::new(); // Empty stats - shows no diff
        let output = format_log_output_with_formatter(&input, &stats, fixed_time_formatter);

        assert!(output.contains("abc1234"));
        assert!(output.contains("Just a commit"));
    }

    #[test]
    fn test_format_log_output_with_graph_prefix() {
        // Git graph output includes graph characters
        let full_hash = "abc1234567890123456789012345678901234567ab";
        let input = format!(
            "* \x01{}\x00abc1234\x1f1699999000\x1f Commit with graph",
            full_hash
        );
        let stats = stats_for(full_hash, 5, 2);
        let output = format_log_output_with_formatter(&input, &stats, fixed_time_formatter);

        assert!(output.contains("abc1234"));
        assert!(output.contains("Commit with graph"));
        // Verify stats are present
        assert!(output.contains("+5"), "expected +5 in output: {}", output);
        assert!(output.contains("-2"), "expected -2 in output: {}", output);
    }

    #[test]
    fn test_format_log_output_zero_stats() {
        // Commit with zero stats (e.g., binary-only changes)
        let full_hash = "abc1234567890123456789012345678901234567ab";
        let input = format!("* \x01{}\x00abc1234\x1f1699999000\x1f Add image", full_hash);
        let stats = stats_for(full_hash, 0, 0);
        let output = format_log_output_with_formatter(&input, &stats, fixed_time_formatter);

        assert!(output.contains("abc1234"));
        assert!(output.contains("Add image"));
    }

    #[test]
    fn test_format_log_output_malformed_commit_line() {
        // Line without proper field delimiters passes through
        let input = "abc1234 regular commit line";
        let stats = HashMap::new();
        let output = format_log_output_with_formatter(input, &stats, fixed_time_formatter);

        // Lines without \x1f delimiter pass through unchanged
        assert!(output.contains("abc1234"));
    }

    #[test]
    fn test_format_log_output_commit_line_missing_second_delimiter() {
        // Only one delimiter - malformed
        let input = "abc1234\x1f1699999000 Fix bug";
        let stats = HashMap::new();
        let output = format_log_output_with_formatter(input, &stats, fixed_time_formatter);

        // Should output the line as-is since it's malformed (only one \x1f)
        assert!(output.contains("abc1234"));
    }

    #[test]
    fn test_format_log_output_stats_only_deletions() {
        // Commit with only deletions (no insertions)
        let full_hash = "abc1234567890123456789012345678901234567ab";
        let input = format!(
            "* \x01{}\x00abc1234\x1f1699999000\x1f Remove old code",
            full_hash
        );
        let stats = stats_for(full_hash, 0, 50);
        let output = format_log_output_with_formatter(&input, &stats, fixed_time_formatter);

        assert!(output.contains("abc1234"));
        assert!(output.contains("Remove old code"));
        // Should show deletions
        assert!(output.contains("-50"), "expected -50 in output: {}", output);
    }

    #[test]
    fn test_format_log_output_large_stats() {
        // Commit with large stats (tests K notation)
        let full_hash = "abc1234567890123456789012345678901234567ab";
        let input = format!(
            "* \x01{}\x00abc1234\x1f1699999000\x1f Big refactor",
            full_hash
        );
        let stats = stats_for(full_hash, 1500, 800);
        let output = format_log_output_with_formatter(&input, &stats, fixed_time_formatter);

        assert!(output.contains("abc1234"));
        // Large numbers should use K notation
        assert!(
            output.contains("+1K") || output.contains("+1.5K"),
            "expected K notation in output: {}",
            output
        );
    }

    #[test]
    fn test_format_commit_line_directly() {
        // Test the format_commit_line function directly
        let commit_line = "abc1234\x1f1699999000\x1f Test commit";
        let stats = (10, 5);
        let target_width = 7; // "abc1234" is 7 chars, no padding needed
        let output = format_commit_line(commit_line, stats, target_width, &fixed_time_formatter);

        assert!(output.contains("abc1234"));
        assert!(output.contains("Test commit"));
        assert!(output.contains("+10"), "expected +10 in output: {}", output);
        assert!(output.contains("-5"), "expected -5 in output: {}", output);
        assert!(output.contains("1h"), "expected time in output: {}", output);
    }

    #[test]
    fn test_format_commit_line_with_padding() {
        // Test that padding aligns shorter hashes to target width
        let commit_line = "abc12\x1f1699999000\x1f Short hash";
        let stats = (5, 2);
        let target_width = 9; // Pad "abc12" (5 chars) to 9 chars
        let output = format_commit_line(commit_line, stats, target_width, &fixed_time_formatter);

        // Should have 4 spaces of padding after hash before stats
        assert!(output.contains("abc12    "), "expected padding: {}", output);
    }

    // Tests for process_log_with_dimming
    //
    // Input format: graph_prefix + SOH (\x01) + full_hash + NUL (\x00) + display
    // Example: "* \x01abc123...def456\x00abc1234 (HEAD) message"

    /// Parse output to determine which lines are dimmed vs bright.
    /// Returns (is_dimmed, content) for each line.
    fn parse_dimming_output(output: &str) -> Vec<(bool, String)> {
        use ansi_str::AnsiStr;
        output
            .lines()
            .map(|line| {
                // Check if line contains dim escape sequence (\x1b[2m)
                let is_dimmed = line.contains("\x1b[2m");
                let content = line.ansi_strip().to_string();
                (is_dimmed, content)
            })
            .collect()
    }

    #[test]
    fn test_process_log_with_dimming_parses_commit_line() {
        // Simulates git log output with SOH/NUL delimiters around full hash
        let hash = "abc123456789012345678901234567890123456789";
        let input = format!("* \x01{}\x00abc1234 (HEAD) Fix bug", hash);

        let unique = HashSet::from([hash.to_string()]);
        let (output, hashes) = process_log_with_dimming(&input, Some(&unique));

        // SOH/NUL markers are preserved for format_log_output to extract hashes
        assert!(
            output.contains('\x01'),
            "SOH should be preserved for format_log_output"
        );
        assert!(
            output.contains('\x00'),
            "NUL should be preserved for format_log_output"
        );
        assert!(output.contains("abc1234"), "short hash preserved");
        assert!(output.contains("Fix bug"), "message preserved");

        // Hashes should be collected for batch stats lookup
        assert_eq!(hashes.len(), 1);
        assert_eq!(hashes[0], hash);

        // Should be bright (in unique set)
        let parsed = parse_dimming_output(&output);
        assert_eq!(parsed.len(), 1);
        assert!(!parsed[0].0, "commit in unique set should be bright");
    }

    #[test]
    fn test_process_log_with_dimming_dims_non_unique() {
        let unique_hash = "abc123456789012345678901234567890123456789";
        let other_hash = "def123456789012345678901234567890123456789";

        let input = format!(
            "* \x01{}\x00abc1234 Unique commit\n\
             * \x01{}\x00def1234 Not unique",
            unique_hash, other_hash
        );

        let unique = HashSet::from([unique_hash.to_string()]);
        let (output, hashes) = process_log_with_dimming(&input, Some(&unique));

        // Both hashes should be collected
        assert_eq!(hashes.len(), 2);

        let parsed = parse_dimming_output(&output);
        assert_eq!(parsed.len(), 2);

        // First commit (unique) should be bright
        assert!(!parsed[0].0, "unique commit should be bright");
        assert!(parsed[0].1.contains("Unique commit"));

        // Second commit (not unique) should be dimmed
        assert!(parsed[1].0, "non-unique commit should be dimmed");
        assert!(parsed[1].1.contains("Not unique"));
    }

    #[test]
    fn test_process_log_with_dimming_none_means_all_bright() {
        // None = default branch, show everything bright
        let hash = "abc123456789012345678901234567890123456789";
        let input = format!("* \x01{}\x00abc1234 Some commit", hash);

        let (output, hashes) = process_log_with_dimming(&input, None);

        assert_eq!(hashes.len(), 1);
        let parsed = parse_dimming_output(&output);
        assert_eq!(parsed.len(), 1);
        assert!(!parsed[0].0, "None means default branch, all bright");
    }

    #[test]
    fn test_process_log_with_dimming_empty_set_means_all_dim() {
        // Some(empty) = feature branch with no unique commits, dim everything
        let hash = "abc123456789012345678901234567890123456789";
        let input = format!("* \x01{}\x00abc1234 Some commit", hash);

        let empty: HashSet<String> = HashSet::new();
        let (output, hashes) = process_log_with_dimming(&input, Some(&empty));

        assert_eq!(hashes.len(), 1);
        let parsed = parse_dimming_output(&output);
        assert_eq!(parsed.len(), 1);
        assert!(
            parsed[0].0,
            "Some(empty) means feature branch with no unique commits, all dim"
        );
    }

    #[test]
    fn test_process_log_with_dimming_preserves_graph_lines() {
        let hash = "abc123456789012345678901234567890123456789";
        // Git graph can have continuation lines between commits
        let input = format!(
            "* \x01{}\x00abc1234 First\n\
             |\n\
             * \x01{}\x00def1234 Second",
            hash, "def123456789012345678901234567890123456789"
        );

        let unique = HashSet::from([hash.to_string()]);
        let (output, _hashes) = process_log_with_dimming(&input, Some(&unique));

        // Graph-only line should be preserved unchanged
        assert!(output.contains("\n|\n"), "graph line should be preserved");
    }

    #[test]
    fn test_process_log_with_dimming_sha256_compatible() {
        // SHA-256 hashes are 64 characters (not 40)
        let sha256_hash = "abc1234567890123456789012345678901234567890123456789012345678901";
        assert_eq!(sha256_hash.len(), 64);

        let input = format!("* \x01{}\x00abc1234 SHA-256 repo", sha256_hash);

        let unique = HashSet::from([sha256_hash.to_string()]);
        let (output, hashes) = process_log_with_dimming(&input, Some(&unique));

        assert_eq!(hashes[0], sha256_hash);
        let parsed = parse_dimming_output(&output);
        assert!(!parsed[0].0, "SHA-256 hash should be matched correctly");
        assert!(parsed[0].1.contains("SHA-256 repo"));
    }

    #[test]
    fn test_process_log_with_dimming_strips_ansi_when_dimming() {
        let hash = "abc123456789012345678901234567890123456789";
        // Simulate colored git output
        let input = format!(
            "* \x01{}\x00\x1b[33mabc1234\x1b[m\x1b[33m (HEAD)\x1b[m message",
            hash
        );

        // Use a different hash to trigger dimming
        let other_unique = HashSet::from(["other".to_string()]);
        let (output, _hashes) = process_log_with_dimming(&input, Some(&other_unique));

        // Dimmed output should have colors stripped
        let parsed = parse_dimming_output(&output);
        assert!(parsed[0].0, "should be dimmed");
        // The ansi_strip should have removed the color codes
        assert!(parsed[0].1.contains("abc1234"));
        assert!(parsed[0].1.contains("(HEAD)"));
    }

    // Tests for strip_hash_markers

    #[test]
    fn test_strip_hash_markers_removes_soh_nul_block() {
        let full_hash = "abc1234567890123456789012345678901234567ab";
        let input = format!("* \x01{}\x00abc1234 message", full_hash);
        let output = strip_hash_markers(&input);

        assert!(!output.contains('\x01'));
        assert!(!output.contains('\x00'));
        assert_eq!(output, "* abc1234 message");
    }

    #[test]
    fn test_strip_hash_markers_preserves_other_content() {
        // No markers - content unchanged
        let input = "* abc1234 (HEAD -> main) Initial commit";
        let output = strip_hash_markers(input);
        assert_eq!(output, input);
    }

    #[test]
    fn test_strip_hash_markers_handles_multiple_markers() {
        let input = "line1 \x01hash1\x00 content1\nline2 \x01hash2\x00 content2";
        let output = strip_hash_markers(input);
        assert_eq!(output, "line1  content1\nline2  content2");
    }

    // Tests for extract_hash_from_line

    #[test]
    fn test_extract_hash_from_line_finds_hash() {
        let full_hash = "abc1234567890123456789012345678901234567ab";
        let line = format!("* \x01{}\x00abc1234 message", full_hash);
        let extracted = extract_hash_from_line(&line);
        assert_eq!(extracted, Some(full_hash));
    }

    #[test]
    fn test_extract_hash_from_line_sha256() {
        let sha256_hash = "abc1234567890123456789012345678901234567890123456789012345678901";
        let line = format!("* \x01{}\x00abc1234 message", sha256_hash);
        let extracted = extract_hash_from_line(&line);
        assert_eq!(extracted, Some(sha256_hash));
    }

    #[test]
    fn test_extract_hash_from_line_no_markers() {
        let line = "* abc1234 message";
        let extracted = extract_hash_from_line(line);
        assert_eq!(extracted, None);
    }

    #[test]
    fn test_extract_hash_from_line_incomplete_markers() {
        // Only SOH, no NUL
        let line = "* \x01abc1234 message";
        let extracted = extract_hash_from_line(line);
        assert_eq!(extracted, None);
    }
}
