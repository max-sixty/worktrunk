use anstyle::{AnsiColor, Color, Style};
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{Shell as CompletionShell, generate};
use rayon::prelude::*;
use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};
use unicode_width::UnicodeWidthStr;
use worktrunk::config::{format_worktree_path, load_config};
use worktrunk::error_format::{format_error, format_error_with_bold, format_hint, format_warning};
use worktrunk::git::{
    GitError, Worktree, branch_exists_in, count_commits_in, get_ahead_behind_in,
    get_all_branches_in, get_available_branches, get_branch_diff_stats_in, get_changed_files_in,
    get_commit_message_in, get_commit_subjects_in, get_commit_timestamp_in, get_current_branch_in,
    get_default_branch_in, get_git_common_dir_in, get_merge_base_in, get_repo_root_in,
    get_upstream_branch_in, get_working_tree_diff_stats_in, get_worktree_root_in,
    get_worktree_state_in, has_merge_commits_in, has_staged_changes_in, is_ancestor_in,
    is_dirty_in, is_in_worktree_in, list_worktrees, run_git_command, worktree_for_branch,
};
use worktrunk::shell;

/// A piece of text with an optional style
#[derive(Clone, Debug)]
struct StyledString {
    text: String,
    style: Option<Style>,
}

impl StyledString {
    fn new(text: impl Into<String>, style: Option<Style>) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }

    fn raw(text: impl Into<String>) -> Self {
        Self::new(text, None)
    }

    fn styled(text: impl Into<String>, style: Style) -> Self {
        Self::new(text, Some(style))
    }

    /// Returns the visual width (unicode-aware, no ANSI codes)
    fn width(&self) -> usize {
        self.text.width()
    }

    /// Renders to a string with ANSI escape codes
    fn render(&self) -> String {
        if let Some(style) = &self.style {
            format!("{}{}{}", style.render(), self.text, style.render_reset())
        } else {
            self.text.clone()
        }
    }
}

/// A line composed of multiple styled strings
#[derive(Clone, Debug, Default)]
struct StyledLine {
    segments: Vec<StyledString>,
}

impl StyledLine {
    fn new() -> Self {
        Self::default()
    }

    /// Add a raw (unstyled) segment
    fn push_raw(&mut self, text: impl Into<String>) {
        self.segments.push(StyledString::raw(text));
    }

    /// Add a styled segment
    fn push_styled(&mut self, text: impl Into<String>, style: Style) {
        self.segments.push(StyledString::styled(text, style));
    }

    /// Add a segment (StyledString)
    fn push(&mut self, segment: StyledString) {
        self.segments.push(segment);
    }

    /// Pad with spaces to reach a specific width
    fn pad_to(&mut self, target_width: usize) {
        let current_width = self.width();
        if current_width < target_width {
            self.push_raw(" ".repeat(target_width - current_width));
        }
    }

    /// Returns the total visual width
    fn width(&self) -> usize {
        self.segments.iter().map(|s| s.width()).sum()
    }

    /// Renders the entire line with ANSI escape codes
    fn render(&self) -> String {
        self.segments.iter().map(|s| s.render()).collect()
    }
}

#[derive(Parser)]
#[command(name = "wt")]
#[command(about = "Git worktree management", long_about = None)]
#[command(version = env!("VERGEN_GIT_DESCRIBE"))]
#[command(disable_help_subcommand = true)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate shell integration code
    Init {
        /// Shell to generate code for (bash, fish, zsh)
        shell: String,

        /// Command prefix (default: wt)
        #[arg(long, default_value = "wt")]
        cmd: String,
    },

    /// List all worktrees
    List,

    /// Switch to a worktree
    Switch {
        /// Branch name or worktree path
        branch: String,

        /// Create a new branch
        #[arg(short = 'c', long)]
        create: bool,

        /// Base branch to create from (only with --create)
        #[arg(short = 'b', long)]
        base: Option<String>,

        /// Use internal mode (outputs directives for shell wrapper)
        #[arg(long, hide = true)]
        internal: bool,
    },

    /// Finish current worktree, returning to primary if current
    Remove {
        /// Use internal mode (outputs directives for shell wrapper)
        #[arg(long, hide = true)]
        internal: bool,
    },

    /// Push changes between worktrees
    Push {
        /// Target branch (defaults to default branch)
        target: Option<String>,

        /// Allow pushing merge commits (non-linear history)
        #[arg(long)]
        allow_merge_commits: bool,
    },

    /// Merge worktree into target branch
    Merge {
        /// Target branch to merge into (defaults to default branch)
        target: Option<String>,

        /// Squash all commits into one before merging
        #[arg(short, long)]
        squash: bool,

        /// Keep worktree after merging (don't remove)
        #[arg(short, long)]
        keep: bool,
    },

    /// Generate shell completion script (deprecated - use init instead)
    #[command(hide = true)]
    Completion {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },

    /// Internal completion helper (hidden)
    #[command(hide = true)]
    Complete {
        /// Arguments to complete
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(clap::ValueEnum, Clone, Copy)]
enum Shell {
    Bash,
    Fish,
    Zsh,
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Init { shell, cmd } => handle_init(&shell, &cmd).map_err(GitError::CommandFailed),
        Commands::List => handle_list(),
        Commands::Switch {
            branch,
            create,
            base,
            internal,
        } => load_config()
            .map_err(|e| GitError::CommandFailed(format!("Failed to load config: {}", e)))
            .and_then(|config| {
                handle_switch(
                    &branch,
                    create,
                    base.as_deref(),
                    internal,
                    &config.worktree_path,
                )
            }),
        Commands::Remove { internal } => handle_remove(internal),
        Commands::Push {
            target,
            allow_merge_commits,
        } => handle_push(target.as_deref(), allow_merge_commits),
        Commands::Merge {
            target,
            squash,
            keep,
        } => handle_merge(target.as_deref(), squash, keep),
        Commands::Completion { shell } => {
            handle_completion(shell);
            Ok(())
        }
        Commands::Complete { args } => handle_complete(args),
    };

    if let Err(e) = result {
        // Error messages are already formatted with emoji and colors
        eprintln!("{}", e);
        process::exit(1);
    }
}

fn handle_init(shell_name: &str, cmd: &str) -> Result<(), String> {
    let shell = shell_name.parse::<shell::Shell>()?;

    let init = shell::ShellInit::new(shell, cmd.to_string());

    // Generate shell integration code
    let integration_output = init
        .generate()
        .map_err(|e| format!("Failed to generate shell code: {}", e))?;

    println!("{}", integration_output);

    // Generate and append static completions
    println!();
    println!("# Static completions (commands and flags)");

    // Generate completions to a string so we can filter out hidden commands
    let mut completion_output = Vec::new();
    let mut cmd = Cli::command();
    let completion_shell = match shell {
        shell::Shell::Bash => CompletionShell::Bash,
        shell::Shell::Fish => CompletionShell::Fish,
        shell::Shell::Zsh => CompletionShell::Zsh,
        // Oil Shell is POSIX-compatible, use Bash completions
        shell::Shell::Oil => CompletionShell::Bash,
        // Other shells don't have completion support yet
        shell::Shell::Elvish
        | shell::Shell::Nushell
        | shell::Shell::Powershell
        | shell::Shell::Xonsh => {
            eprintln!("Completion not yet supported for {}", shell);
            std::process::exit(1);
        }
    };
    generate(completion_shell, &mut cmd, "wt", &mut completion_output);

    // Filter out lines for hidden commands (completion, complete)
    let completion_str = String::from_utf8_lossy(&completion_output);
    let filtered: Vec<&str> = completion_str
        .lines()
        .filter(|line| {
            // Remove lines that complete the hidden commands
            !(line.contains("\"completion\"")
                || line.contains("\"complete\"")
                || line.contains("-a \"completion\"")
                || line.contains("-a \"complete\""))
        })
        .collect();

    for line in filtered {
        println!("{}", line);
    }

    Ok(())
}

struct WorktreeInfo {
    path: std::path::PathBuf,
    head: String,
    branch: Option<String>,
    timestamp: i64,
    commit_message: String,
    ahead: usize,
    behind: usize,
    working_tree_diff: (usize, usize),
    branch_diff: (usize, usize),
    is_primary: bool,
    is_current: bool,
    detached: bool,
    bare: bool,
    locked: Option<String>,
    prunable: Option<String>,
    upstream_remote: Option<String>,
    upstream_ahead: usize,
    upstream_behind: usize,
    worktree_state: Option<String>,
}

fn handle_list() -> Result<(), GitError> {
    let worktrees = list_worktrees()?;

    if worktrees.is_empty() {
        return Ok(());
    }

    // First worktree is the primary
    let primary = &worktrees[0];
    let primary_branch = primary.branch.as_ref();

    // Get current worktree to identify active one
    let current_worktree_path = get_worktree_root_in(Path::new(".")).ok();

    // Helper function to process a single worktree
    let process_worktree = |idx: usize, wt: &Worktree| -> WorktreeInfo {
        let is_primary = idx == 0;
        let is_current = current_worktree_path
            .as_ref()
            .map(|p| p == &wt.path)
            .unwrap_or(false);

        // Get commit timestamp
        let timestamp = get_commit_timestamp_in(&wt.path, &wt.head).unwrap_or(0);

        // Get commit message
        let commit_message = get_commit_message_in(&wt.path, &wt.head).unwrap_or_default();

        // Calculate ahead/behind relative to primary branch (only if primary has a branch)
        let (ahead, behind) = if is_primary {
            (0, 0)
        } else if let Some(pb) = primary_branch {
            get_ahead_behind_in(&wt.path, pb, &wt.head).unwrap_or((0, 0))
        } else {
            (0, 0)
        };
        let working_tree_diff = get_working_tree_diff_stats_in(&wt.path).unwrap_or((0, 0));

        // Get branch diff stats (downstream of primary, only if primary has a branch)
        let branch_diff = if is_primary {
            (0, 0)
        } else if let Some(pb) = primary_branch {
            get_branch_diff_stats_in(&wt.path, pb, &wt.head).unwrap_or((0, 0))
        } else {
            (0, 0)
        };

        // Get upstream tracking info
        let (upstream_remote, upstream_ahead, upstream_behind) = if let Some(ref branch) = wt.branch
        {
            if let Ok(Some(upstream_branch)) = get_upstream_branch_in(&wt.path, branch) {
                // Extract remote name from "origin/main" -> "origin"
                let remote = upstream_branch
                    .split('/')
                    .next()
                    .unwrap_or("origin")
                    .to_string();
                let (ahead, behind) =
                    get_ahead_behind_in(&wt.path, &upstream_branch, &wt.head).unwrap_or((0, 0));
                (Some(remote), ahead, behind)
            } else {
                (None, 0, 0)
            }
        } else {
            (None, 0, 0)
        };

        // Get worktree state (merge/rebase/etc)
        let worktree_state = get_worktree_state_in(&wt.path).unwrap_or(None);

        WorktreeInfo {
            path: wt.path.clone(),
            head: wt.head.clone(),
            branch: wt.branch.clone(),
            timestamp,
            commit_message,
            ahead,
            behind,
            working_tree_diff,
            branch_diff,
            is_primary,
            is_current,
            detached: wt.detached,
            bare: wt.bare,
            locked: wt.locked.clone(),
            prunable: wt.prunable.clone(),
            upstream_remote,
            upstream_ahead,
            upstream_behind,
            worktree_state,
        }
    };

    // Gather enhanced information for all worktrees in parallel
    //
    // Parallelization strategy: Use Rayon to process worktrees concurrently.
    // Each worktree requires ~5 git operations (timestamp, ahead/behind, diffs).
    //
    // Benchmark results: See benches/list.rs for sequential vs parallel comparison.
    //
    // Decision: Always use parallel for simplicity and 2+ worktree performance.
    // Rayon overhead (~1-2ms) is acceptable for single-worktree case.
    //
    // TODO: Could parallelize the 5 git commands within each worktree if needed,
    // but worktree-level parallelism provides the best cost/benefit tradeoff
    let mut infos: Vec<WorktreeInfo> = if std::env::var("WT_SEQUENTIAL").is_ok() {
        // Sequential iteration (for benchmarking)
        worktrees
            .iter()
            .enumerate()
            .map(|(idx, wt)| process_worktree(idx, wt))
            .collect()
    } else {
        // Parallel iteration (default)
        worktrees
            .par_iter()
            .enumerate()
            .map(|(idx, wt)| process_worktree(idx, wt))
            .collect()
    };

    // Sort by most recent commit (descending)
    infos.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    // Calculate responsive layout based on terminal width
    let layout = calculate_responsive_layout(&infos);

    // Display header
    format_header_line(&layout);

    // Display formatted output
    for info in &infos {
        format_worktree_line(info, &layout);
    }

    Ok(())
}

fn format_relative_time(timestamp: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let seconds_ago = now - timestamp;

    if seconds_ago < 0 {
        return "in the future".to_string();
    }

    let minutes = seconds_ago / 60;
    let hours = minutes / 60;
    let days = hours / 24;
    let weeks = days / 7;
    let months = days / 30;
    let years = days / 365;

    if years > 0 {
        format!("{} year{} ago", years, if years == 1 { "" } else { "s" })
    } else if months > 0 {
        format!("{} month{} ago", months, if months == 1 { "" } else { "s" })
    } else if weeks > 0 {
        format!("{} week{} ago", weeks, if weeks == 1 { "" } else { "s" })
    } else if days > 0 {
        format!("{} day{} ago", days, if days == 1 { "" } else { "s" })
    } else if hours > 0 {
        format!("{} hour{} ago", hours, if hours == 1 { "" } else { "s" })
    } else if minutes > 0 {
        format!(
            "{} minute{} ago",
            minutes,
            if minutes == 1 { "" } else { "s" }
        )
    } else {
        "just now".to_string()
    }
}

/// Find the common prefix among all paths
fn find_common_prefix(paths: &[PathBuf]) -> PathBuf {
    if paths.is_empty() {
        return PathBuf::new();
    }

    let first = &paths[0];
    let mut prefix = PathBuf::new();

    for component in first.components() {
        let candidate = prefix.join(component);
        if paths.iter().all(|p| p.starts_with(&candidate)) {
            prefix = candidate;
        } else {
            break;
        }
    }

    prefix
}

/// Shorten a path relative to a common prefix
fn shorten_path(path: &Path, prefix: &Path) -> String {
    if let Ok(relative) = path.strip_prefix(prefix) {
        if relative.as_os_str().is_empty() {
            ".".to_string()
        } else {
            format!("./{}", relative.display())
        }
    } else {
        path.display().to_string()
    }
}

/// Truncate text at word boundary with ellipsis, respecting terminal width
fn truncate_at_word_boundary(text: &str, max_width: usize) -> String {
    use unicode_width::UnicodeWidthStr;

    if text.width() <= max_width {
        return text.to_string();
    }

    // Build up string until we hit the width limit (accounting for "..." = 3 width)
    let target_width = max_width.saturating_sub(3);
    let mut current_width = 0;
    let mut last_space_idx = None;
    let mut last_idx = 0;

    for (idx, ch) in text.char_indices() {
        let char_width = UnicodeWidthStr::width(ch.to_string().as_str());
        if current_width + char_width > target_width {
            break;
        }
        if ch.is_whitespace() {
            last_space_idx = Some(idx);
        }
        current_width += char_width;
        last_idx = idx + ch.len_utf8();
    }

    // Use last space if found, otherwise truncate at last character that fits
    let truncate_at = last_space_idx.unwrap_or(last_idx);
    format!("{}...", &text[..truncate_at].trim())
}

/// Get terminal width, defaulting to 80 if detection fails
fn get_terminal_width() -> usize {
    terminal_size::terminal_size()
        .map(|(terminal_size::Width(w), _)| w as usize)
        .unwrap_or(80)
}

struct ColumnWidths {
    branch: usize,
    time: usize,
    message: usize,
    ahead_behind: usize,
    working_diff: usize,
    branch_diff: usize,
    upstream: usize,
    states: usize,
}

struct LayoutConfig {
    widths: ColumnWidths,
    ideal_widths: ColumnWidths, // Maximum widths for padding sparse columns
    common_prefix: PathBuf,
    max_message_len: usize,
}

fn calculate_column_widths(infos: &[WorktreeInfo]) -> ColumnWidths {
    let mut max_branch = 0;
    let mut max_time = 0;
    let mut max_message = 0;
    let mut max_ahead_behind = 0;
    let mut max_working_diff = 0;
    let mut max_branch_diff = 0;
    let mut max_upstream = 0;
    let mut max_states = 0;

    for info in infos {
        // Branch name
        let branch_len = info.branch.as_deref().unwrap_or("(detached)").width();
        max_branch = max_branch.max(branch_len);

        // Time
        let time_str = format_relative_time(info.timestamp);
        max_time = max_time.max(time_str.width());

        // Message (truncate to 50 chars max)
        let msg_len = info.commit_message.chars().take(50).count();
        max_message = max_message.max(msg_len);

        // Ahead/behind
        if !info.is_primary && (info.ahead > 0 || info.behind > 0) {
            let ahead_behind_len = format!("↑{} ↓{}", info.ahead, info.behind).width();
            max_ahead_behind = max_ahead_behind.max(ahead_behind_len);
        }

        // Working tree diff
        let (wt_added, wt_deleted) = info.working_tree_diff;
        if wt_added > 0 || wt_deleted > 0 {
            let working_diff_len = format!("+{} -{}", wt_added, wt_deleted).width();
            max_working_diff = max_working_diff.max(working_diff_len);
        }

        // Branch diff
        if !info.is_primary {
            let (br_added, br_deleted) = info.branch_diff;
            if br_added > 0 || br_deleted > 0 {
                let branch_diff_len = format!("+{} -{}", br_added, br_deleted).width();
                max_branch_diff = max_branch_diff.max(branch_diff_len);
            }
        }

        // Upstream tracking
        if info.upstream_ahead > 0 || info.upstream_behind > 0 {
            let remote_name = info.upstream_remote.as_deref().unwrap_or("origin");
            let upstream_len = format!(
                "{} ↑{} ↓{}",
                remote_name, info.upstream_ahead, info.upstream_behind
            )
            .width();
            max_upstream = max_upstream.max(upstream_len);
        }

        // States (including worktree_state)
        let states = format_all_states(info);
        if !states.is_empty() {
            max_states = max_states.max(states.width());
        }
    }

    ColumnWidths {
        branch: max_branch,
        time: max_time,
        message: max_message,
        ahead_behind: max_ahead_behind,
        working_diff: max_working_diff,
        branch_diff: max_branch_diff,
        upstream: max_upstream,
        states: max_states,
    }
}

/// Calculate responsive layout based on terminal width
fn calculate_responsive_layout(infos: &[WorktreeInfo]) -> LayoutConfig {
    let terminal_width = get_terminal_width();
    let paths: Vec<PathBuf> = infos.iter().map(|info| info.path.clone()).collect();
    let common_prefix = find_common_prefix(&paths);

    // Count how many rows have each sparse column
    let non_primary_count = infos.iter().filter(|i| !i.is_primary).count();
    let ahead_behind_count = infos
        .iter()
        .filter(|i| !i.is_primary && (i.ahead > 0 || i.behind > 0))
        .count();
    let working_diff_count = infos
        .iter()
        .filter(|i| {
            let (added, deleted) = i.working_tree_diff;
            added > 0 || deleted > 0
        })
        .count();
    let branch_diff_count = infos
        .iter()
        .filter(|i| {
            if i.is_primary {
                return false;
            }
            let (added, deleted) = i.branch_diff;
            added > 0 || deleted > 0
        })
        .count();
    let upstream_count = infos
        .iter()
        .filter(|i| i.upstream_ahead > 0 || i.upstream_behind > 0)
        .count();
    let states_count = infos
        .iter()
        .filter(|i| {
            i.worktree_state.is_some()
                || (i.detached && i.branch.is_some())
                || i.bare
                || i.locked.is_some()
                || i.prunable.is_some()
        })
        .count();

    // A column is "dense" if it appears in >50% of applicable rows
    // For ahead/behind and branch_diff, applicable = non-primary rows
    // For others, applicable = all rows
    let ahead_behind_is_dense = non_primary_count > 0 && ahead_behind_count * 2 > non_primary_count;
    let working_diff_is_dense = working_diff_count * 2 > infos.len();
    let branch_diff_is_dense = non_primary_count > 0 && branch_diff_count * 2 > non_primary_count;
    let upstream_is_dense = upstream_count * 2 > infos.len();
    let states_is_dense = states_count * 2 > infos.len();

    // Calculate ideal column widths
    let ideal_widths = calculate_column_widths(infos);

    // Essential columns (always shown):
    // - current indicator: 2 chars
    // - branch: variable
    // - short HEAD: 8 chars
    // - path: at least 20 chars (we'll use shortened paths)
    // - spacing: 2 chars between columns

    let spacing = 2;
    let current_indicator = 2;
    let short_head = 8;
    let min_path = 20;

    // Calculate base width needed
    let base_width =
        current_indicator + ideal_widths.branch + spacing + short_head + spacing + min_path;

    // Available width for optional columns
    let available = terminal_width.saturating_sub(base_width);

    // Priority order for columns (from high to low):
    // 1. time (15-20 chars)
    // 2. message (20-50 chars, flexible)
    // 3. ahead_behind - commits difference (if any worktree has it)
    // 4. branch_diff - line diff in commits (if any worktree has it)
    // 5. working_diff - line diff in working tree (if any worktree has it)
    // 6. upstream (if any worktree has it)
    // 7. states (if any worktree has it)

    let mut remaining = available;
    let mut widths = ColumnWidths {
        branch: ideal_widths.branch,
        time: 0,
        message: 0,
        ahead_behind: 0,
        working_diff: 0,
        branch_diff: 0,
        upstream: 0,
        states: 0,
    };

    // Time column (high priority, ~15 chars)
    if remaining >= ideal_widths.time + spacing {
        widths.time = ideal_widths.time;
        remaining = remaining.saturating_sub(ideal_widths.time + spacing);
    }

    // Message column (flexible, 20-50 chars)
    let max_message_len = if remaining >= 50 + spacing {
        remaining = remaining.saturating_sub(50 + spacing);
        50
    } else if remaining >= 30 + spacing {
        let msg_len = remaining.saturating_sub(spacing).min(ideal_widths.message);
        remaining = remaining.saturating_sub(msg_len + spacing);
        msg_len
    } else if remaining >= 20 + spacing {
        let msg_len = 20;
        remaining = remaining.saturating_sub(msg_len + spacing);
        msg_len
    } else {
        0
    };

    if max_message_len > 0 {
        widths.message = max_message_len;
    }

    // Ahead/behind column (only if dense and fits)
    if ahead_behind_is_dense
        && ideal_widths.ahead_behind > 0
        && remaining >= ideal_widths.ahead_behind + spacing
    {
        widths.ahead_behind = ideal_widths.ahead_behind;
        remaining = remaining.saturating_sub(ideal_widths.ahead_behind + spacing);
    }

    // Working diff column (only if dense and fits)
    if working_diff_is_dense
        && ideal_widths.working_diff > 0
        && remaining >= ideal_widths.working_diff + spacing
    {
        widths.working_diff = ideal_widths.working_diff;
        remaining = remaining.saturating_sub(ideal_widths.working_diff + spacing);
    }

    // Branch diff column (only if dense and fits)
    if branch_diff_is_dense
        && ideal_widths.branch_diff > 0
        && remaining >= ideal_widths.branch_diff + spacing
    {
        widths.branch_diff = ideal_widths.branch_diff;
        remaining = remaining.saturating_sub(ideal_widths.branch_diff + spacing);
    }

    // Upstream column (only if dense and fits)
    if upstream_is_dense
        && ideal_widths.upstream > 0
        && remaining >= ideal_widths.upstream + spacing
    {
        widths.upstream = ideal_widths.upstream;
        remaining = remaining.saturating_sub(ideal_widths.upstream + spacing);
    }

    // States column (only if dense and fits)
    if states_is_dense && ideal_widths.states > 0 && remaining >= ideal_widths.states + spacing {
        widths.states = ideal_widths.states;
    }

    LayoutConfig {
        widths,
        ideal_widths,
        common_prefix,
        max_message_len,
    }
}

fn format_all_states(info: &WorktreeInfo) -> String {
    let mut states = Vec::new();

    // Worktree state (merge/rebase/etc)
    if let Some(ref state) = info.worktree_state {
        states.push(format!("[{}]", state));
    }

    // Don't show detached state if branch is None (already shown in branch column)
    if info.detached && info.branch.is_some() {
        states.push("(detached)".to_string());
    }
    if info.bare {
        states.push("(bare)".to_string());
    }
    if let Some(ref reason) = info.locked {
        if reason.is_empty() {
            states.push("(locked)".to_string());
        } else {
            states.push(format!("(locked: {})", reason));
        }
    }
    if let Some(ref reason) = info.prunable {
        if reason.is_empty() {
            states.push("(prunable)".to_string());
        } else {
            states.push(format!("(prunable: {})", reason));
        }
    }

    states.join(" ")
}

fn format_header_line(layout: &LayoutConfig) {
    let widths = &layout.widths;
    let dim_style = Style::new().dimmed();

    let mut line = StyledLine::new();

    // Branch
    let header = format!("{:width$}", "Branch", width = widths.branch);
    line.push_styled(header, dim_style);
    line.push_raw("  ");

    // Age (Time)
    if widths.time > 0 {
        let header = format!("{:width$}", "Age", width = widths.time);
        line.push_styled(header, dim_style);
        line.push_raw("  ");
    }

    // Ahead/behind (commits)
    if layout.ideal_widths.ahead_behind > 0 {
        let header = format!(
            "{:width$}",
            "Cmts",
            width = layout.ideal_widths.ahead_behind
        );
        line.push_styled(header, dim_style);
        line.push_raw("  ");
    }

    // Branch diff (line diff in commits)
    if layout.ideal_widths.branch_diff > 0 {
        let header = format!(
            "{:width$}",
            "Cmt +/-",
            width = layout.ideal_widths.branch_diff
        );
        line.push_styled(header, dim_style);
        line.push_raw("  ");
    }

    // Working tree diff
    if layout.ideal_widths.working_diff > 0 {
        let header = format!(
            "{:width$}",
            "WT +/-",
            width = layout.ideal_widths.working_diff
        );
        line.push_styled(header, dim_style);
        line.push_raw("  ");
    }

    // Upstream
    if layout.ideal_widths.upstream > 0 {
        let header = format!("{:width$}", "Remote", width = layout.ideal_widths.upstream);
        line.push_styled(header, dim_style);
        line.push_raw("  ");
    }

    // Commit (fixed width: 8 chars)
    line.push_styled("Commit  ", dim_style);
    line.push_raw("  ");

    // Message
    if widths.message > 0 {
        let header = format!("{:width$}", "Message", width = widths.message);
        line.push_styled(header, dim_style);
        line.push_raw("  ");
    }

    // States
    if layout.ideal_widths.states > 0 {
        let header = format!("{:width$}", "State", width = layout.ideal_widths.states);
        line.push_styled(header, dim_style);
        line.push_raw("  ");
    }

    // Path
    line.push_styled("Path", dim_style);

    println!("{}", line.render());
}

fn format_worktree_line(info: &WorktreeInfo, layout: &LayoutConfig) {
    let widths = &layout.widths;
    let primary_style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan)));
    let current_style = Style::new()
        .bold()
        .fg_color(Some(Color::Ansi(AnsiColor::Magenta)));
    let green_style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green)));
    let red_style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Red)));
    let yellow_style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow)));
    let dim_style = Style::new().dimmed();

    let branch_display = info.branch.as_deref().unwrap_or("(detached)");
    let short_head = &info.head[..8.min(info.head.len())];

    // Determine styles: current worktree is bold magenta, primary is cyan
    let text_style = if info.is_current {
        Some(current_style)
    } else if info.is_primary {
        Some(primary_style)
    } else {
        None
    };

    // Start building the line
    let mut line = StyledLine::new();

    // Branch name
    let branch_text = format!("{:width$}", branch_display, width = widths.branch);
    if let Some(style) = text_style {
        line.push_styled(branch_text, style);
    } else {
        line.push_raw(branch_text);
    }
    line.push_raw("  ");

    // Age (Time)
    if widths.time > 0 {
        let time_str = format!(
            "{:width$}",
            format_relative_time(info.timestamp),
            width = widths.time
        );
        line.push_styled(time_str, dim_style);
        line.push_raw("  ");
    }

    // Ahead/behind (commits difference) - always reserve space if ANY row uses it
    if layout.ideal_widths.ahead_behind > 0 {
        if !info.is_primary && (info.ahead > 0 || info.behind > 0) {
            let ahead_behind_text = format!(
                "{:width$}",
                format!("↑{} ↓{}", info.ahead, info.behind),
                width = layout.ideal_widths.ahead_behind
            );
            line.push_styled(ahead_behind_text, yellow_style);
        } else {
            // No data for this row: pad with spaces
            line.push_raw(" ".repeat(layout.ideal_widths.ahead_behind));
        }
        line.push_raw("  ");
    }

    // Branch diff (line diff in commits) - always reserve space if ANY row uses it
    if layout.ideal_widths.branch_diff > 0 {
        if !info.is_primary {
            let (br_added, br_deleted) = info.branch_diff;
            if br_added > 0 || br_deleted > 0 {
                // Build the diff as a mini styled line
                let mut diff_segment = StyledLine::new();
                diff_segment.push_styled(format!("+{}", br_added), green_style);
                diff_segment.push_raw(" ");
                diff_segment.push_styled(format!("-{}", br_deleted), red_style);
                diff_segment.pad_to(layout.ideal_widths.branch_diff);
                // Append all segments from diff_segment to main line
                for segment in diff_segment.segments {
                    line.push(segment);
                }
            } else {
                // No data for this row: pad with spaces
                line.push_raw(" ".repeat(layout.ideal_widths.branch_diff));
            }
        } else {
            // Primary row: pad with spaces
            line.push_raw(" ".repeat(layout.ideal_widths.branch_diff));
        }
        line.push_raw("  ");
    }

    // Working tree diff (line diff in working tree) - always reserve space if ANY row uses it
    if layout.ideal_widths.working_diff > 0 {
        let (wt_added, wt_deleted) = info.working_tree_diff;
        if wt_added > 0 || wt_deleted > 0 {
            // Build the diff as a mini styled line
            let mut diff_segment = StyledLine::new();
            diff_segment.push_styled(format!("+{}", wt_added), green_style);
            diff_segment.push_raw(" ");
            diff_segment.push_styled(format!("-{}", wt_deleted), red_style);
            diff_segment.pad_to(layout.ideal_widths.working_diff);
            // Append all segments from diff_segment to main line
            for segment in diff_segment.segments {
                line.push(segment);
            }
        } else {
            // No data for this row: pad with spaces
            line.push_raw(" ".repeat(layout.ideal_widths.working_diff));
        }
        line.push_raw("  ");
    }

    // Upstream tracking - always reserve space if ANY row uses it
    if layout.ideal_widths.upstream > 0 {
        if info.upstream_ahead > 0 || info.upstream_behind > 0 {
            let remote_name = info.upstream_remote.as_deref().unwrap_or("origin");
            // Build the upstream as a mini styled line
            let mut upstream_segment = StyledLine::new();
            upstream_segment.push_styled(remote_name, dim_style);
            upstream_segment.push_raw(" ");
            upstream_segment.push_styled(format!("↑{}", info.upstream_ahead), green_style);
            upstream_segment.push_raw(" ");
            upstream_segment.push_styled(format!("↓{}", info.upstream_behind), red_style);
            upstream_segment.pad_to(layout.ideal_widths.upstream);
            // Append all segments from upstream_segment to main line
            for segment in upstream_segment.segments {
                line.push(segment);
            }
        } else {
            // No data for this row: pad with spaces
            line.push_raw(" ".repeat(layout.ideal_widths.upstream));
        }
        line.push_raw("  ");
    }

    // Commit (short HEAD, fixed width: 8 chars)
    if let Some(style) = text_style {
        line.push_styled(short_head, style);
    } else {
        line.push_raw(short_head);
    }
    line.push_raw("  ");

    // Message (left-aligned, truncated at word boundary)
    if widths.message > 0 {
        let msg = format!(
            "{:width$}",
            truncate_at_word_boundary(&info.commit_message, layout.max_message_len),
            width = widths.message
        );
        line.push_styled(msg, dim_style);
        line.push_raw("  ");
    }

    // States - always reserve space if ANY row uses it
    if layout.ideal_widths.states > 0 {
        let states = format_all_states(info);
        if !states.is_empty() {
            let states_text = format!("{:width$}", states, width = layout.ideal_widths.states);
            line.push_raw(states_text);
        } else {
            // No data for this row: pad with spaces
            line.push_raw(" ".repeat(layout.ideal_widths.states));
        }
        line.push_raw("  ");
    }

    // Path (no padding needed, it's the last column, use shortened path)
    let path_str = shorten_path(&info.path, &layout.common_prefix);
    if let Some(style) = text_style {
        line.push_styled(path_str, style);
    } else {
        line.push_raw(path_str);
    }

    println!("{}", line.render());
}

fn print_worktree_info(path: &std::path::Path, command: &str) {
    println!("Path: {}", path.display());
    println!(
        "Note: Use 'wt {}' (with shell integration) for automatic cd",
        command
    );
}

fn handle_switch(
    branch: &str,
    create: bool,
    base: Option<&str>,
    internal: bool,
    worktree_path_template: &str,
) -> Result<(), GitError> {
    // Check for conflicting conditions
    if create && branch_exists_in(Path::new("."), branch)? {
        return Err(GitError::CommandFailed(format_error_with_bold(
            "Branch '",
            branch,
            "' already exists. Remove --create flag to switch to it.",
        )));
    }

    // Check if base flag was provided without create flag
    if base.is_some() && !create {
        eprintln!(
            "{}",
            format_warning("--base flag is only used with --create, ignoring")
        );
    }

    // Check if worktree already exists for this branch
    if let Some(existing_path) = worktree_for_branch(branch)? {
        if existing_path.exists() {
            if internal {
                println!("__WORKTRUNK_CD__{}", existing_path.display());
            }
            return Ok(());
        } else {
            return Err(GitError::CommandFailed(format_error_with_bold(
                "Worktree directory missing for '",
                branch,
                "'. Run 'git worktree prune' to clean up.",
            )));
        }
    }

    // No existing worktree, create one
    let repo_root = get_repo_root_in(Path::new("."))?;

    let repo_name = repo_root
        .file_name()
        .ok_or_else(|| GitError::CommandFailed("Invalid repository path".to_string()))?
        .to_str()
        .ok_or_else(|| GitError::CommandFailed("Invalid UTF-8 in path".to_string()))?;

    let worktree_name = format_worktree_path(worktree_path_template, repo_name, branch);
    let worktree_path = repo_root.join(worktree_name);

    // Create the worktree
    // Build git worktree add command
    let mut args = vec!["worktree", "add", worktree_path.to_str().unwrap()];
    if create {
        args.push("-b");
        args.push(branch);
        if let Some(base_branch) = base {
            args.push(base_branch);
        }
    } else {
        args.push(branch);
    }

    run_git_command(&args, Some(Path::new(".")))
        .map_err(|e| GitError::CommandFailed(format!("Failed to create worktree: {}", e)))
        .map(|_| ())?;

    // Output success message
    let success_msg = if create {
        format!("Created new branch and worktree for '{}'", branch)
    } else {
        format!("Added worktree for existing branch '{}'", branch)
    };

    if internal {
        println!("__WORKTRUNK_CD__{}", worktree_path.display());
        println!("{} at {}", success_msg, worktree_path.display());
    } else {
        println!("{}", success_msg);
        print_worktree_info(&worktree_path, "switch");
    }

    Ok(())
}

fn handle_remove(internal: bool) -> Result<(), GitError> {
    // Check for uncommitted changes
    if is_dirty_in(Path::new("."))? {
        return Err(GitError::CommandFailed(format_error(
            "Working tree has uncommitted changes. Commit or stash them first.",
        )));
    }

    // Get current state
    let current_branch = get_current_branch_in(Path::new("."))?;
    let default_branch = get_default_branch_in(Path::new("."))?;
    let in_worktree = is_in_worktree_in(Path::new("."))?;

    // If we're on default branch and not in a worktree, nothing to do
    if !in_worktree && current_branch.as_deref() == Some(&default_branch) {
        if !internal {
            println!("Already on default branch '{}'", default_branch);
        }
        return Ok(());
    }

    if in_worktree {
        // In worktree: navigate to primary worktree and remove this one
        let worktree_root = get_worktree_root_in(Path::new("."))?;
        let primary_worktree_dir = get_repo_root_in(Path::new("."))?;

        if internal {
            println!("__WORKTRUNK_CD__{}", primary_worktree_dir.display());
        }

        // Schedule worktree removal (synchronous for now, could be async later)
        let remove_result = process::Command::new("git")
            .args(["worktree", "remove", worktree_root.to_str().unwrap()])
            .output()
            .map_err(|e| GitError::CommandFailed(e.to_string()))?;

        if !remove_result.status.success() {
            let stderr = String::from_utf8_lossy(&remove_result.stderr);
            eprintln!("Warning: Failed to remove worktree: {}", stderr);
            eprintln!(
                "You may need to run 'git worktree remove {}' manually",
                worktree_root.display()
            );
        }

        if !internal {
            println!("Moved to primary worktree and removed worktree");
            print_worktree_info(&primary_worktree_dir, "remove");
        }
    } else {
        // In main repo but not on default branch: switch to default
        run_git_command(&["switch", &default_branch], Some(Path::new(".")))
            .map_err(|e| {
                GitError::CommandFailed(format!("Failed to switch to '{}': {}", default_branch, e))
            })
            .map(|_| ())?;

        if !internal {
            println!("Switched to default branch '{}'", default_branch);
        }
    }

    Ok(())
}

fn handle_push(target: Option<&str>, allow_merge_commits: bool) -> Result<(), GitError> {
    // Get target branch (default to default branch if not provided)
    let target_branch = match target {
        Some(b) => b.to_string(),
        None => get_default_branch_in(Path::new("."))?,
    };

    // Check if it's a fast-forward
    if !is_ancestor_in(Path::new("."), &target_branch, "HEAD")? {
        let error_msg =
            format_error_with_bold("Not a fast-forward from '", &target_branch, "' to HEAD");
        let hint_msg = format_hint(
            "The target branch has commits not in your current branch. Consider 'git pull' or 'git rebase'",
        );
        return Err(GitError::CommandFailed(format!(
            "{}\n{}",
            error_msg, hint_msg
        )));
    }

    // Check for merge commits unless allowed
    if !allow_merge_commits && has_merge_commits_in(Path::new("."), &target_branch, "HEAD")? {
        return Err(GitError::CommandFailed(format_error(
            "Found merge commits in push range. Use --allow-merge-commits to push non-linear history.",
        )));
    }

    // Configure receive.denyCurrentBranch if needed
    // TODO: These git config commands don't use run_git_command() because they don't check
    // status.success() and may rely on exit codes for missing keys. Should be refactored.
    let deny_config_output = process::Command::new("git")
        .args(["config", "receive.denyCurrentBranch"])
        .output()
        .map_err(|e| GitError::CommandFailed(e.to_string()))?;

    let current_config = String::from_utf8_lossy(&deny_config_output.stdout);
    if current_config.trim() != "updateInstead" {
        process::Command::new("git")
            .args(["config", "receive.denyCurrentBranch", "updateInstead"])
            .output()
            .map_err(|e| GitError::CommandFailed(e.to_string()))?;
    }

    // Find worktree for target branch
    let target_worktree = worktree_for_branch(&target_branch)?;

    if let Some(ref wt_path) = target_worktree {
        // Check if target worktree is dirty
        if is_dirty_in(wt_path)? {
            // Get files changed in the push
            let push_files = get_changed_files_in(Path::new("."), &target_branch, "HEAD")?;

            // Get files changed in the worktree
            let wt_status_output = run_git_command(&["status", "--porcelain"], Some(wt_path))?;

            let wt_files: Vec<String> = wt_status_output
                .lines()
                .filter_map(|line| {
                    // Parse porcelain format: "XY filename"
                    let parts: Vec<&str> = line.splitn(2, ' ').collect();
                    parts.get(1).map(|s| s.trim().to_string())
                })
                .collect();

            // Find overlapping files
            let overlapping: Vec<String> = push_files
                .iter()
                .filter(|f| wt_files.contains(f))
                .cloned()
                .collect();

            if !overlapping.is_empty() {
                eprintln!(
                    "{}",
                    format_error("Cannot push: conflicting uncommitted changes in:")
                );
                for file in &overlapping {
                    eprintln!("  - {}", file);
                }
                return Err(GitError::CommandFailed(format!(
                    "Commit or stash changes in {} first",
                    wt_path.display()
                )));
            }
        }
    }

    // Count commits and show info
    let commit_count = count_commits_in(Path::new("."), &target_branch, "HEAD")?;
    if commit_count > 0 {
        let commit_text = if commit_count == 1 {
            "commit"
        } else {
            "commits"
        };
        println!(
            "Pushing {} {} to '{}'",
            commit_count, commit_text, target_branch
        );
    }

    // Get git common dir for the push
    let git_common_dir = get_git_common_dir_in(Path::new("."))?;

    // Perform the push
    let push_target = format!("HEAD:{}", target_branch);
    run_git_command(
        &["push", git_common_dir.to_str().unwrap(), &push_target],
        Some(Path::new(".")),
    )
    .map_err(|e| GitError::CommandFailed(format!("Push failed: {}", e)))?;

    println!("Successfully pushed to '{}'", target_branch);
    Ok(())
}

fn generate_squash_message(
    target_branch: &str,
    subjects: &[String],
    llm_config: &worktrunk::config::LlmConfig,
) -> String {
    // Try LLM generation if configured
    if let Some(ref command) = llm_config.command {
        if let Ok(llm_message) =
            try_generate_llm_message(target_branch, subjects, command, &llm_config.args)
        {
            return llm_message;
        }
        // If LLM fails, fall through to deterministic approach
        eprintln!("Warning: LLM generation failed, using deterministic message");
    }

    // Fallback: deterministic commit message
    let mut commit_message = format!("Squash commits from {}\n\n", target_branch);
    commit_message.push_str("Combined commits:\n");
    for subject in subjects.iter().rev() {
        // Reverse so they're in chronological order
        commit_message.push_str(&format!("- {}\n", subject));
    }
    commit_message
}

fn try_generate_llm_message(
    target_branch: &str,
    subjects: &[String],
    command: &str,
    args: &[String],
) -> Result<String, Box<dyn std::error::Error>> {
    // Build context prompt
    let mut context = format!(
        "Squashing commits on current branch since branching from {}\n\n",
        target_branch
    );
    context.push_str("Commits being combined:\n");
    for subject in subjects.iter().rev() {
        context.push_str(&format!("- {}\n", subject));
    }

    let prompt = "Generate a conventional commit message (feat/fix/docs/style/refactor) that combines these changes into one cohesive message. Output only the commit message without any explanation.";
    let full_prompt = format!("{}\n\n{}", context, prompt);

    // Execute LLM command
    let output = process::Command::new(command)
        .args(args)
        .arg(&full_prompt)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("LLM command failed: {}", stderr).into());
    }

    let message = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if message.is_empty() {
        return Err("LLM returned empty message".into());
    }

    Ok(message)
}

fn handle_squash(target_branch: &str) -> Result<(), GitError> {
    // Get merge base with target branch
    let merge_base = get_merge_base_in(Path::new("."), "HEAD", target_branch)?;

    // Count commits since merge base
    let commit_count = count_commits_in(Path::new("."), &merge_base, "HEAD")?;

    // Check if there are staged changes
    let has_staged = has_staged_changes_in(Path::new("."))?;

    // Handle different scenarios
    if commit_count == 0 && !has_staged {
        // No commits and no staged changes - nothing to squash
        println!("No commits to squash - already at merge base");
        return Ok(());
    }

    if commit_count == 0 && has_staged {
        // Just staged changes, no commits - would need to commit but this shouldn't happen in merge flow
        return Err(GitError::CommandFailed(format_error(
            "Staged changes without commits - please commit them first",
        )));
    }

    if commit_count == 1 && !has_staged {
        // Single commit, no staged changes - nothing to do
        println!(
            "Only 1 commit since '{}' - no squashing needed",
            target_branch
        );
        return Ok(());
    }

    // One or more commits (possibly with staged changes) - squash them
    println!("Squashing {} commits into one...", commit_count);

    // Get commit subjects for the squash message
    let range = format!("{}..HEAD", merge_base);
    let subjects = get_commit_subjects_in(Path::new("."), &range)?;

    // Load config and generate commit message
    let config = load_config()
        .map_err(|e| GitError::CommandFailed(format!("Failed to load config: {}", e)))?;
    let commit_message = generate_squash_message(target_branch, &subjects, &config.llm);

    // Reset to merge base (soft reset stages all changes)
    run_git_command(&["reset", "--soft", &merge_base], Some(Path::new(".")))
        .map_err(|e| GitError::CommandFailed(format!("Failed to reset to merge base: {}", e)))?;

    // Commit with the generated message
    run_git_command(&["commit", "-m", &commit_message], Some(Path::new(".")))
        .map_err(|e| GitError::CommandFailed(format!("Failed to create squash commit: {}", e)))?;

    println!("Successfully squashed {} commits into one", commit_count);
    Ok(())
}

fn handle_merge(target: Option<&str>, squash: bool, keep: bool) -> Result<(), GitError> {
    // Get current branch
    let current_branch = get_current_branch_in(Path::new("."))?
        .ok_or_else(|| GitError::CommandFailed(format_error("Not on a branch (detached HEAD)")))?;

    // Get target branch (default to default branch if not provided)
    let target_branch = match target {
        Some(b) => b.to_string(),
        None => get_default_branch_in(Path::new("."))?,
    };

    // Check if already on target branch
    if current_branch == target_branch {
        println!("Already on '{}', nothing to merge", target_branch);
        return Ok(());
    }

    // Check for uncommitted changes
    if is_dirty_in(Path::new("."))? {
        return Err(GitError::CommandFailed(format_error(
            "Working tree has uncommitted changes. Commit or stash them first.",
        )));
    }

    // Squash commits if requested
    if squash {
        handle_squash(&target_branch)?;
    }

    // Rebase onto target
    println!("Rebasing onto '{}'...", target_branch);

    run_git_command(&["rebase", &target_branch], Some(Path::new("."))).map_err(|e| {
        GitError::CommandFailed(format!("Failed to rebase onto '{}': {}", target_branch, e))
    })?;

    // Fast-forward push to target branch (reuse handle_push logic)
    println!("Fast-forwarding '{}' to current HEAD...", target_branch);
    handle_push(Some(&target_branch), false)?;

    // Finish worktree unless --keep was specified
    if !keep {
        println!("Cleaning up worktree...");

        // Get primary worktree path before finishing (while we can still run git commands)
        let primary_worktree_dir = get_repo_root_in(Path::new("."))?;

        handle_remove(false)?;

        // Check if we need to switch to target branch
        let new_branch = get_current_branch_in(&primary_worktree_dir)?;
        if new_branch.as_deref() != Some(&target_branch) {
            println!("Switching to '{}'...", target_branch);
            run_git_command(&["switch", &target_branch], Some(&primary_worktree_dir)).map_err(
                |e| {
                    GitError::CommandFailed(format!(
                        "Failed to switch to '{}': {}",
                        target_branch, e
                    ))
                },
            )?;
        }
    } else {
        println!(
            "Successfully merged to '{}' (worktree preserved)",
            target_branch
        );
    }

    Ok(())
}

fn handle_completion(shell: Shell) {
    let mut cmd = Cli::command();
    let completion_shell = match shell {
        Shell::Bash => CompletionShell::Bash,
        Shell::Fish => CompletionShell::Fish,
        Shell::Zsh => CompletionShell::Zsh,
    };
    generate(completion_shell, &mut cmd, "wt", &mut io::stdout());
}

#[derive(Debug, PartialEq)]
enum CompletionContext {
    SwitchBranch,
    PushTarget,
    MergeTarget,
    BaseFlag,
    Unknown,
}

fn parse_completion_context(args: &[String]) -> CompletionContext {
    // args format: ["wt", "switch", "partial"]
    // or: ["wt", "switch", "--create", "new", "--base", "partial"]

    if args.len() < 2 {
        return CompletionContext::Unknown;
    }

    let subcommand = &args[1];

    // Check if the previous argument was a flag that expects a value
    // If so, we're completing that flag's value
    if args.len() >= 3 {
        let prev_arg = &args[args.len() - 2];
        if prev_arg == "--base" || prev_arg == "-b" {
            return CompletionContext::BaseFlag;
        }
    }

    // Otherwise, complete based on the subcommand's positional argument
    match subcommand.as_str() {
        "switch" => CompletionContext::SwitchBranch,
        "push" => CompletionContext::PushTarget,
        "merge" => CompletionContext::MergeTarget,
        _ => CompletionContext::Unknown,
    }
}

fn get_branches_for_completion<F>(get_branches_fn: F) -> Vec<String>
where
    F: FnOnce() -> Result<Vec<String>, GitError>,
{
    get_branches_fn().unwrap_or_else(|e| {
        if std::env::var("WT_DEBUG_COMPLETION").is_ok() {
            eprintln!("completion error: {}", e);
        }
        Vec::new()
    })
}

fn handle_complete(args: Vec<String>) -> Result<(), GitError> {
    let context = parse_completion_context(&args);

    match context {
        CompletionContext::SwitchBranch => {
            // Complete with available branches (excluding those with worktrees)
            let branches = get_branches_for_completion(get_available_branches);
            for branch in branches {
                println!("{}", branch);
            }
        }
        CompletionContext::PushTarget
        | CompletionContext::MergeTarget
        | CompletionContext::BaseFlag => {
            // Complete with all branches
            let branches = get_branches_for_completion(|| get_all_branches_in(Path::new(".")));
            for branch in branches {
                println!("{}", branch);
            }
        }
        CompletionContext::Unknown => {
            // No completions
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_completion_context_switch() {
        let args = vec!["wt".to_string(), "switch".to_string(), "feat".to_string()];
        assert_eq!(
            parse_completion_context(&args),
            CompletionContext::SwitchBranch
        );
    }

    #[test]
    fn test_parse_completion_context_push() {
        let args = vec!["wt".to_string(), "push".to_string(), "ma".to_string()];
        assert_eq!(
            parse_completion_context(&args),
            CompletionContext::PushTarget
        );
    }

    #[test]
    fn test_parse_completion_context_merge() {
        let args = vec!["wt".to_string(), "merge".to_string(), "de".to_string()];
        assert_eq!(
            parse_completion_context(&args),
            CompletionContext::MergeTarget
        );
    }

    #[test]
    fn test_parse_completion_context_base_flag() {
        let args = vec![
            "wt".to_string(),
            "switch".to_string(),
            "--create".to_string(),
            "new".to_string(),
            "--base".to_string(),
            "dev".to_string(),
        ];
        assert_eq!(parse_completion_context(&args), CompletionContext::BaseFlag);
    }

    #[test]
    fn test_parse_completion_context_unknown() {
        let args = vec!["wt".to_string()];
        assert_eq!(parse_completion_context(&args), CompletionContext::Unknown);
    }

    #[test]
    fn test_parse_completion_context_base_flag_short() {
        let args = vec![
            "wt".to_string(),
            "switch".to_string(),
            "--create".to_string(),
            "new".to_string(),
            "-b".to_string(),
            "dev".to_string(),
        ];
        assert_eq!(parse_completion_context(&args), CompletionContext::BaseFlag);
    }

    #[test]
    fn test_parse_completion_context_base_at_end() {
        // --base at the end with empty string (what shell sends when completing)
        let args = vec![
            "wt".to_string(),
            "switch".to_string(),
            "--create".to_string(),
            "new".to_string(),
            "--base".to_string(),
            "".to_string(), // Shell sends empty string for cursor position
        ];
        // Should detect BaseFlag context
        assert_eq!(parse_completion_context(&args), CompletionContext::BaseFlag);
    }

    #[test]
    fn test_parse_completion_context_multiple_base_flags() {
        // Multiple --base flags (last one wins)
        let args = vec![
            "wt".to_string(),
            "switch".to_string(),
            "--create".to_string(),
            "new".to_string(),
            "--base".to_string(),
            "main".to_string(),
            "--base".to_string(),
            "develop".to_string(),
        ];
        assert_eq!(parse_completion_context(&args), CompletionContext::BaseFlag);
    }

    #[test]
    fn test_parse_completion_context_empty_args() {
        let args = vec![];
        assert_eq!(parse_completion_context(&args), CompletionContext::Unknown);
    }

    #[test]
    fn test_parse_completion_context_switch_only() {
        // Just "wt switch" with no other args
        let args = vec!["wt".to_string(), "switch".to_string()];
        assert_eq!(
            parse_completion_context(&args),
            CompletionContext::SwitchBranch
        );
    }

    #[test]
    fn test_styled_string_width() {
        use crate::StyledString;

        // ASCII strings
        let s = StyledString::raw("hello");
        assert_eq!(s.width(), 5);

        // Unicode arrows
        let s = StyledString::raw("↑3 ↓2");
        assert_eq!(
            s.width(),
            5,
            "↑3 ↓2 should have width 5, not {}",
            s.text.len()
        );

        // Mixed Unicode
        let s = StyledString::raw("日本語");
        assert_eq!(s.width(), 6); // CJK characters are typically width 2

        // Emoji
        let s = StyledString::raw("🎉");
        assert_eq!(s.width(), 2); // Emoji are typically width 2
    }

    #[test]
    fn test_styled_line_width() {
        use crate::StyledLine;

        let mut line = StyledLine::new();
        line.push_raw("Branch");
        line.push_raw("  ");
        line.push_raw("↑3 ↓2");

        // "Branch" (6) + "  " (2) + "↑3 ↓2" (5) = 13
        assert_eq!(line.width(), 13, "Line width should be 13");
    }

    #[test]
    fn test_styled_line_padding() {
        use crate::StyledLine;

        let mut line = StyledLine::new();
        line.push_raw("test");
        assert_eq!(line.width(), 4);

        line.pad_to(10);
        assert_eq!(line.width(), 10, "After padding to 10, width should be 10");

        // Padding when already at target should not change width
        line.pad_to(10);
        assert_eq!(line.width(), 10, "Padding again should not change width");
    }

    #[test]
    fn test_column_width_calculation_with_unicode() {
        use crate::{WorktreeInfo, calculate_column_widths};
        use std::path::PathBuf;

        let info1 = WorktreeInfo {
            path: PathBuf::from("/test"),
            head: "abc123".to_string(),
            branch: Some("main".to_string()),
            timestamp: 0,
            commit_message: "Test".to_string(),
            ahead: 3,
            behind: 2,
            working_tree_diff: (100, 50),
            branch_diff: (200, 30),
            is_primary: false,
            is_current: false,
            detached: false,
            bare: false,
            locked: None,
            prunable: None,
            upstream_remote: Some("origin".to_string()),
            upstream_ahead: 4,
            upstream_behind: 0,
            worktree_state: None,
        };

        let widths = calculate_column_widths(&[info1]);

        // "↑3 ↓2" has visual width 5 (not 9 bytes)
        assert_eq!(widths.ahead_behind, 5, "↑3 ↓2 should have width 5");

        // "+100 -50" has width 8
        assert_eq!(widths.working_diff, 8, "+100 -50 should have width 8");

        // "+200 -30" has width 8
        assert_eq!(widths.branch_diff, 8, "+200 -30 should have width 8");

        // "origin ↑4 ↓0" has visual width 12 (not more due to Unicode arrows)
        assert_eq!(widths.upstream, 12, "origin ↑4 ↓0 should have width 12");
    }

    #[test]
    fn test_column_alignment_with_all_columns() {
        use crate::{
            ColumnWidths, LayoutConfig, StyledLine, WorktreeInfo, format_all_states, shorten_path,
        };
        use std::path::PathBuf;

        // Create test data with all columns populated
        let info = WorktreeInfo {
            path: PathBuf::from("/test/path"),
            head: "abc12345".to_string(),
            branch: Some("test-branch".to_string()),
            timestamp: 0,
            commit_message: "Test message".to_string(),
            ahead: 3,
            behind: 2,
            working_tree_diff: (100, 50),
            branch_diff: (200, 30),
            is_primary: false,
            is_current: false,
            detached: false,
            bare: false,
            locked: Some("test lck".to_string()), // "(locked: test lck)" = 18 chars
            prunable: None,
            upstream_remote: Some("origin".to_string()),
            upstream_ahead: 4,
            upstream_behind: 0,
            worktree_state: None,
        };

        let layout = LayoutConfig {
            widths: ColumnWidths {
                branch: 11,
                time: 13,
                message: 12,
                ahead_behind: 5,
                working_diff: 8,
                branch_diff: 8,
                upstream: 12,
                states: 18,
            },
            ideal_widths: ColumnWidths {
                branch: 11,
                time: 13,
                message: 12,
                ahead_behind: 5,
                working_diff: 8,
                branch_diff: 8,
                upstream: 12,
                states: 18,
            },
            common_prefix: PathBuf::from("/test"),
            max_message_len: 12,
        };

        // Build header line manually (mimicking format_header_line logic)
        let mut header = StyledLine::new();
        header.push_raw(format!("{:width$}", "Branch", width = layout.widths.branch));
        header.push_raw("  ");
        header.push_raw(format!("{:width$}", "Age", width = layout.widths.time));
        header.push_raw("  ");
        header.push_raw(format!(
            "{:width$}",
            "Cmts",
            width = layout.ideal_widths.ahead_behind
        ));
        header.push_raw("  ");
        header.push_raw(format!(
            "{:width$}",
            "Cmt +/-",
            width = layout.ideal_widths.branch_diff
        ));
        header.push_raw("  ");
        header.push_raw(format!(
            "{:width$}",
            "WT +/-",
            width = layout.ideal_widths.working_diff
        ));
        header.push_raw("  ");
        header.push_raw(format!(
            "{:width$}",
            "Remote",
            width = layout.ideal_widths.upstream
        ));
        header.push_raw("  ");
        header.push_raw("Commit  ");
        header.push_raw("  ");
        header.push_raw(format!(
            "{:width$}",
            "Message",
            width = layout.widths.message
        ));
        header.push_raw("  ");
        header.push_raw(format!(
            "{:width$}",
            "State",
            width = layout.ideal_widths.states
        ));
        header.push_raw("  ");
        header.push_raw("Path");

        // Build data line manually (mimicking format_worktree_line logic)
        let mut data = StyledLine::new();
        data.push_raw(format!(
            "{:width$}",
            "test-branch",
            width = layout.widths.branch
        ));
        data.push_raw("  ");
        data.push_raw(format!(
            "{:width$}",
            "9 months ago",
            width = layout.widths.time
        ));
        data.push_raw("  ");
        // Ahead/behind
        let ahead_behind_text = format!(
            "{:width$}",
            "↑3 ↓2",
            width = layout.ideal_widths.ahead_behind
        );
        data.push_raw(ahead_behind_text);
        data.push_raw("  ");
        // Branch diff
        let mut branch_diff_segment = StyledLine::new();
        branch_diff_segment.push_raw("+200 -30");
        branch_diff_segment.pad_to(layout.ideal_widths.branch_diff);
        for seg in branch_diff_segment.segments {
            data.push(seg);
        }
        data.push_raw("  ");
        // Working diff
        let mut working_diff_segment = StyledLine::new();
        working_diff_segment.push_raw("+100 -50");
        working_diff_segment.pad_to(layout.ideal_widths.working_diff);
        for seg in working_diff_segment.segments {
            data.push(seg);
        }
        data.push_raw("  ");
        // Upstream
        let mut upstream_segment = StyledLine::new();
        upstream_segment.push_raw("origin ↑4 ↓0");
        upstream_segment.pad_to(layout.ideal_widths.upstream);
        for seg in upstream_segment.segments {
            data.push(seg);
        }
        data.push_raw("  ");
        // Commit (fixed 8 chars)
        data.push_raw("abc12345");
        data.push_raw("  ");
        // Message
        data.push_raw(format!(
            "{:width$}",
            "Test message",
            width = layout.widths.message
        ));
        data.push_raw("  ");
        // State
        let states = format_all_states(&info);
        data.push_raw(format!(
            "{:width$}",
            states,
            width = layout.ideal_widths.states
        ));
        data.push_raw("  ");
        // Path
        data.push_raw(shorten_path(&info.path, &layout.common_prefix));

        // Verify both lines have columns at the same positions
        // We'll check this by verifying specific column start positions
        let header_str = header.render();
        let data_str = data.render();

        // Remove ANSI codes for position checking (our test data doesn't have styles anyway)
        assert!(header_str.contains("Branch"));
        assert!(data_str.contains("test-branch"));

        // The key test: both lines should have the same visual width up to "Path" column
        // (Path is variable width, so we only check up to there)
        let header_width_without_path = header.width() - "Path".len();
        let data_width_without_path =
            data.width() - shorten_path(&info.path, &layout.common_prefix).len();

        assert_eq!(
            header_width_without_path, data_width_without_path,
            "Header and data rows should have same width before Path column"
        );
    }

    #[test]
    fn test_sparse_column_padding() {
        use crate::StyledLine;

        // Build simplified lines to test sparse column padding
        let mut line1 = StyledLine::new();
        line1.push_raw(format!("{:8}", "branch-a"));
        line1.push_raw("  ");
        // Has ahead/behind
        line1.push_raw(format!("{:5}", "↑3 ↓2"));
        line1.push_raw("  ");

        let mut line2 = StyledLine::new();
        line2.push_raw(format!("{:8}", "branch-b"));
        line2.push_raw("  ");
        // No ahead/behind, should pad with spaces
        line2.push_raw(" ".repeat(5));
        line2.push_raw("  ");

        // Both lines should have same width up to this point
        assert_eq!(
            line1.width(),
            line2.width(),
            "Rows with and without sparse column data should have same width"
        );
    }
}
