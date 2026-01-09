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
use worktrunk::git::{Repository, parse_numstat_line};
use worktrunk::shell::extract_filename_from_path;

use super::list::collect;
use super::list::layout::{DiffDisplayConfig, DiffVariant};
use super::list::model::ListItem;
use super::worktree::handle_switch;
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
        let tab4 = format_tab("4: remote⇅", mode == PreviewMode::UpstreamDiff);

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
        let repo = Repository::current();
        let Ok(default_branch) = repo.default_branch() else {
            return cformat!("{INFO_SYMBOL} <bold>{branch}</> has no commits ahead of main\n");
        };
        if self.item.counts().ahead == 0 {
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
        let repo = Repository::current();
        let head = self.item.head();
        let branch = self.item.branch_name();
        let Ok(default_branch) = repo.default_branch() else {
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
        // Dim format: only hash is dimmed, message stays normal (matches upstream style)
        let no_timestamp_format = "--format=%C(auto)%h%C(auto)%d%C(reset) %s";
        let dim_no_timestamp_format = "--format=%C(dim)%h%C(reset) %s";

        // With timestamps and diffstat, we use field delimiter and --numstat for predictable parsing:
        // - \x1f (unit separator) separates fields within a commit line
        // - --numstat gives tab-separated "added\tdeleted\tfilename" lines we sum
        // Format: graph + hash \x1f timestamp \x1f message (followed by numstat lines)
        let timestamp_format = format!(
            "--format=%C(auto)%h{}%ct{}%C(auto)%d%C(reset) %s",
            FIELD_DELIM, FIELD_DELIM
        );
        // Dim format: hash is dimmed, timestamp will also be dimmed
        let dim_timestamp_format = format!(
            "--format=%C(dim)%h%C(reset){}%ct{} %s",
            FIELD_DELIM, FIELD_DELIM
        );

        let log_limit_str = log_limit.to_string();
        if is_default_branch {
            // Viewing default branch itself - show history without dimming
            let format: &str = if show_timestamps {
                &timestamp_format
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
            let format: &str = if show_timestamps {
                &timestamp_format
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
                bright_count = log_output
                    .lines()
                    .filter(|l| l.contains(FIELD_DELIM))
                    .count();
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
                let format: &str = if show_timestamps {
                    &dim_timestamp_format
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
}

/// Field delimiter for git log format with timestamps
const FIELD_DELIM: char = '\x1f';

/// Timestamp column width ("12mo" is the longest)
const TIMESTAMP_WIDTH: usize = 4;

/// Format git log output with timestamps and diffstats.
///
/// Parses git log output in the format:
/// `graph_hash\x1ftimestamp\x1f decoration message`
/// followed by numstat lines (`added\tdeleted\tfilename`).
///
/// Returns formatted output with aligned timestamps and diff stats.
fn format_log_output(log_output: &str) -> String {
    use crate::display::format_relative_time_short;
    format_log_output_with_formatter(log_output, format_relative_time_short)
}

/// Format git log output with a custom time formatter.
///
/// This variant allows dependency injection for testing with deterministic timestamps.
fn format_log_output_with_formatter<F>(log_output: &str, format_time: F) -> String
where
    F: Fn(i64) -> String,
{
    // State machine: accumulate stats for each commit
    let mut pending_commit: Option<&str> = None;
    let mut pending_stats: (usize, usize) = (0, 0);
    let mut result = Vec::new();

    for line in log_output.lines() {
        if line.contains(FIELD_DELIM) {
            // This is a commit line - emit any pending commit first
            if let Some(prev) = pending_commit.take() {
                result.push(format_commit_line(prev, pending_stats, &format_time));
            }
            pending_commit = Some(line);
            pending_stats = (0, 0);
        } else if let Some((ins, del)) = parse_numstat_line(line) {
            // Accumulate stats for pending commit
            pending_stats.0 += ins;
            pending_stats.1 += del;
        }
        // Skip empty/graph-only lines
    }

    // Don't forget final commit
    if let Some(last) = pending_commit {
        result.push(format_commit_line(last, pending_stats, &format_time));
    }

    result.join("\n")
}

/// Format a single commit line with stats
fn format_commit_line<F>(
    commit_line: &str,
    (insertions, deletions): (usize, usize),
    format_time: &F,
) -> String
where
    F: Fn(i64) -> String,
{
    use worktrunk::styling::{ADDITION, DELETION};

    let dim_style = anstyle::Style::new().dimmed();
    let reset = anstyle::Reset;

    if let Some(first_delim) = commit_line.find(FIELD_DELIM)
        && let Some(second_delim) = commit_line[first_delim + 1..].find(FIELD_DELIM)
    {
        let graph_hash = &commit_line[..first_delim];
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
}

pub fn handle_select() -> anyhow::Result<()> {
    use std::io::IsTerminal;

    // Select requires an interactive terminal for the TUI
    if !std::io::stdin().is_terminal() {
        anyhow::bail!("wt select requires an interactive terminal");
    }

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

    // Use 500ms timeout for git commands to show TUI faster on large repos.
    // Typical slow operations: merge-tree ~400-1800ms, rev-list ~200-600ms.
    // 500ms allows most operations to complete while cutting off tail latency.
    // Operations that timeout fail silently (data not shown), but TUI stays responsive.
    let command_timeout = Some(std::time::Duration::from_millis(500));

    let Some(list_data) = collect::collect(
        &repo,
        true,  // show_branches (include branches without worktrees)
        false, // show_remotes (local branches only, not remote branches)
        &skip_tasks,
        false, // show_progress (no progress bars)
        false, // render_table (select renders its own UI)
        &config,
        command_timeout,
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

        // Switch to the selected worktree
        // handle_switch can handle both branch names and worktree paths
        let (result, branch_info) =
            handle_switch(&identifier, false, None, false, false, false, &config)?;

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

    #[test]
    fn test_format_log_output_single_commit() {
        // Simulate git log output: hash\x1ftimestamp\x1f message
        let input = "abc1234\x1f1699999000\x1f Fix bug";
        let output = format_log_output_with_formatter(input, fixed_time_formatter);

        // Should contain the hash and message
        assert!(output.contains("abc1234"));
        assert!(output.contains("Fix bug"));
        // Should contain formatted time
        assert!(output.contains("1h"));
    }

    #[test]
    fn test_format_log_output_with_numstat() {
        // Commit line followed by numstat lines
        let input = "abc1234\x1f1699999000\x1f Add feature\n\
                     10\t5\tfile1.rs\n\
                     3\t0\tfile2.rs";
        let output = format_log_output_with_formatter(input, fixed_time_formatter);

        // Should contain the hash and message
        assert!(output.contains("abc1234"));
        // Stats should be accumulated: 10+3=13 insertions, 5+0=5 deletions
        // The output should contain the stats in the formatted line
        assert!(output.contains("Add feature"));
        // Verify stats are present (green +13, red -5)
        assert!(output.contains("+13"), "expected +13 in output: {}", output);
        assert!(output.contains("-5"), "expected -5 in output: {}", output);
    }

    #[test]
    fn test_format_log_output_multiple_commits() {
        // Two commits, each with numstat
        let input = "abc1234\x1f1699999000\x1f First commit\n\
                     5\t2\tfile.rs\n\
                     def5678\x1f1699998000\x1f Second commit\n\
                     10\t3\tother.rs";
        let output = format_log_output_with_formatter(input, fixed_time_formatter);

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
        let output = format_log_output_with_formatter("", fixed_time_formatter);
        assert!(output.is_empty());
    }

    #[test]
    fn test_format_log_output_no_numstat() {
        // Commit without numstat lines
        let input = "abc1234\x1f1699999000\x1f Just a commit";
        let output = format_log_output_with_formatter(input, fixed_time_formatter);

        assert!(output.contains("abc1234"));
        assert!(output.contains("Just a commit"));
    }

    #[test]
    fn test_format_log_output_with_graph_prefix() {
        // Git graph output includes graph characters
        let input = "* abc1234\x1f1699999000\x1f Commit with graph\n\
                     | 5\t2\tfile.rs";
        let output = format_log_output_with_formatter(input, fixed_time_formatter);

        assert!(output.contains("abc1234"));
        assert!(output.contains("Commit with graph"));
        // Verify stats are present
        assert!(output.contains("+5"), "expected +5 in output: {}", output);
        assert!(output.contains("-2"), "expected -2 in output: {}", output);
    }

    #[test]
    fn test_format_log_output_binary_files() {
        // Binary files show "-" in numstat
        let input = "abc1234\x1f1699999000\x1f Add image\n\
                     -\t-\timage.png\n\
                     5\t0\tdocs.md";
        let output = format_log_output_with_formatter(input, fixed_time_formatter);

        // Binary files treated as 0 additions/deletions
        // Should still format the commit line
        assert!(output.contains("abc1234"));
        assert!(output.contains("Add image"));
        // Verify stats: 0 (binary) + 5 = 5 insertions, 0 deletions
        assert!(output.contains("+5"), "expected +5 in output: {}", output);
    }

    #[test]
    fn test_format_log_output_malformed_commit_line() {
        // Line without proper field delimiters should be passed through
        let input = "abc1234 regular commit line";
        let output = format_log_output_with_formatter(input, fixed_time_formatter);

        // Should be empty since no valid commit lines (no FIELD_DELIM)
        assert!(output.is_empty());
    }

    #[test]
    fn test_format_log_output_commit_line_missing_second_delimiter() {
        // Only one delimiter - malformed
        let input = "abc1234\x1f1699999000 Fix bug";
        let output = format_log_output_with_formatter(input, fixed_time_formatter);

        // Should output the line as-is since it's malformed (only one \x1f)
        assert!(output.contains("abc1234"));
    }

    #[test]
    fn test_format_log_output_stats_only_deletions() {
        // Commit with only deletions (no insertions)
        let input = "abc1234\x1f1699999000\x1f Remove old code\n\
                     0\t50\told_file.rs";
        let output = format_log_output_with_formatter(input, fixed_time_formatter);

        assert!(output.contains("abc1234"));
        assert!(output.contains("Remove old code"));
        // Should show deletions
        assert!(output.contains("-50"), "expected -50 in output: {}", output);
    }

    #[test]
    fn test_format_log_output_large_stats() {
        // Commit with large stats (tests K notation)
        let input = "abc1234\x1f1699999000\x1f Big refactor\n\
                     1500\t800\tlarge_file.rs";
        let output = format_log_output_with_formatter(input, fixed_time_formatter);

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
        let output = format_commit_line(commit_line, stats, &fixed_time_formatter);

        assert!(output.contains("abc1234"));
        assert!(output.contains("Test commit"));
        assert!(output.contains("+10"), "expected +10 in output: {}", output);
        assert!(output.contains("-5"), "expected -5 in output: {}", output);
        assert!(output.contains("1h"), "expected time in output: {}", output);
    }
}
