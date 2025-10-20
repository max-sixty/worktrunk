use anstyle::{AnsiColor, Color, Style};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{Shell as CompletionShell, generate};
use std::io;
use std::process;
use worktrunk::git::{
    GitError, branch_exists, count_commits, get_ahead_behind_in, get_all_branches,
    get_available_branches, get_branch_diff_stats_in, get_changed_files, get_commit_timestamp_in,
    get_current_branch, get_current_branch_in, get_default_branch, get_git_common_dir,
    get_working_tree_diff_stats_in, get_worktree_root, has_merge_commits, is_ancestor, is_dirty,
    is_dirty_in, is_in_worktree, list_worktrees, worktree_for_branch,
};
use worktrunk::shell;

#[derive(Parser)]
#[command(name = "wt")]
#[command(about = "Git worktree management", long_about = None)]
#[command(version = env!("VERGEN_GIT_DESCRIBE"))]
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

        /// Hook mode (none, prompt)
        #[arg(long, default_value = "none")]
        hook: String,
    },

    /// List all worktrees
    List,

    /// Switch to a worktree (creates if doesn't exist)
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

    /// Finish current worktree and return to primary
    Finish {
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

    /// Merge and cleanup worktree
    Merge {
        /// Target branch to merge into (defaults to default branch)
        target: Option<String>,

        /// Keep worktree after merging (don't finish)
        #[arg(short, long)]
        keep: bool,
    },

    /// Hook commands (for shell integration)
    Hook {
        /// Hook type
        hook_type: String,
    },

    /// Generate shell completion script
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

#[derive(ValueEnum, Clone, Copy)]
enum Shell {
    Bash,
    Fish,
    Zsh,
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Init { shell, cmd, hook } => {
            handle_init(&shell, &cmd, &hook).map_err(GitError::CommandFailed)
        }
        Commands::List => handle_list(),
        Commands::Switch {
            branch,
            create,
            base,
            internal,
        } => handle_switch(&branch, create, base.as_deref(), internal),
        Commands::Finish { internal } => handle_finish(internal),
        Commands::Push {
            target,
            allow_merge_commits,
        } => handle_push(target.as_deref(), allow_merge_commits),
        Commands::Merge { target, keep } => handle_merge(target.as_deref(), keep),
        Commands::Hook { hook_type } => handle_hook(&hook_type).map_err(GitError::CommandFailed),
        Commands::Completion { shell } => {
            handle_completion(shell);
            Ok(())
        }
        Commands::Complete { args } => handle_complete(args),
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        process::exit(1);
    }
}

fn handle_init(shell_name: &str, cmd: &str, hook_str: &str) -> Result<(), String> {
    let shell = shell_name.parse::<shell::Shell>()?;
    let hook = hook_str.parse::<shell::Hook>()?;

    let init = shell::ShellInit::new(shell, cmd.to_string(), hook);

    let output = init
        .generate()
        .map_err(|e| format!("Failed to generate shell code: {}", e))?;

    println!("{}", output);
    Ok(())
}

struct WorktreeInfo {
    path: std::path::PathBuf,
    head: String,
    branch: Option<String>,
    timestamp: i64,
    ahead: usize,
    behind: usize,
    working_tree_diff: (usize, usize),
    branch_diff: (usize, usize),
    is_primary: bool,
    detached: bool,
    bare: bool,
    locked: Option<String>,
    prunable: Option<String>,
}

fn handle_list() -> Result<(), GitError> {
    let worktrees = list_worktrees()?;

    if worktrees.is_empty() {
        return Ok(());
    }

    // First worktree is the primary
    let primary = &worktrees[0];
    let primary_branch = primary.branch.as_ref();

    // Gather enhanced information for all worktrees
    let mut infos: Vec<WorktreeInfo> = Vec::new();

    for (idx, wt) in worktrees.iter().enumerate() {
        let is_primary = idx == 0;

        // Get commit timestamp
        let timestamp = get_commit_timestamp_in(&wt.path, &wt.head).unwrap_or(0);

        // Calculate ahead/behind relative to primary branch (only if primary has a branch)
        let (ahead, behind) = if is_primary || primary_branch.is_none() {
            (0, 0)
        } else {
            get_ahead_behind_in(&wt.path, primary_branch.unwrap(), &wt.head).unwrap_or((0, 0))
        };

        // Get working tree diff stats
        let working_tree_diff = get_working_tree_diff_stats_in(&wt.path).unwrap_or((0, 0));

        // Get branch diff stats (downstream of primary, only if primary has a branch)
        let branch_diff = if is_primary || primary_branch.is_none() {
            (0, 0)
        } else {
            get_branch_diff_stats_in(&wt.path, primary_branch.unwrap(), &wt.head).unwrap_or((0, 0))
        };

        infos.push(WorktreeInfo {
            path: wt.path.clone(),
            head: wt.head.clone(),
            branch: wt.branch.clone(),
            timestamp,
            ahead,
            behind,
            working_tree_diff,
            branch_diff,
            is_primary,
            detached: wt.detached,
            bare: wt.bare,
            locked: wt.locked.clone(),
            prunable: wt.prunable.clone(),
        });
    }

    // Sort by most recent commit (descending)
    infos.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    // Calculate column widths for alignment
    let widths = calculate_column_widths(&infos);

    // Display formatted output
    for info in &infos {
        format_worktree_line(info, &widths);
    }

    Ok(())
}

struct ColumnWidths {
    branch: usize,
    ahead_behind: usize,
    working_diff: usize,
    branch_diff: usize,
    states: usize,
}

fn calculate_column_widths(infos: &[WorktreeInfo]) -> ColumnWidths {
    let mut max_branch = 0;
    let mut max_ahead_behind = 0;
    let mut max_working_diff = 0;
    let mut max_branch_diff = 0;
    let mut max_states = 0;

    for info in infos {
        // Branch name
        let branch_len = info.branch.as_deref().unwrap_or("(detached)").len();
        max_branch = max_branch.max(branch_len);

        // Ahead/behind
        if !info.is_primary && (info.ahead > 0 || info.behind > 0) {
            let ahead_behind_len = format!("↑{} ↓{}", info.ahead, info.behind).len();
            max_ahead_behind = max_ahead_behind.max(ahead_behind_len);
        }

        // Working tree diff
        let (wt_added, wt_deleted) = info.working_tree_diff;
        if wt_added > 0 || wt_deleted > 0 {
            let working_diff_len = format!("+{} -{}", wt_added, wt_deleted).len();
            max_working_diff = max_working_diff.max(working_diff_len);
        }

        // Branch diff
        if !info.is_primary {
            let (br_added, br_deleted) = info.branch_diff;
            if br_added > 0 || br_deleted > 0 {
                let branch_diff_len = format!("(+{} -{})", br_added, br_deleted).len();
                max_branch_diff = max_branch_diff.max(branch_diff_len);
            }
        }

        // States
        let states = format_states(info);
        if !states.is_empty() {
            max_states = max_states.max(states.len());
        }
    }

    ColumnWidths {
        branch: max_branch,
        ahead_behind: max_ahead_behind,
        working_diff: max_working_diff,
        branch_diff: max_branch_diff,
        states: max_states,
    }
}

fn format_states(info: &WorktreeInfo) -> String {
    let mut states = Vec::new();

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

fn format_worktree_line(info: &WorktreeInfo, widths: &ColumnWidths) {
    let primary_style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan)));

    let branch_display = info.branch.as_deref().unwrap_or("(detached)");
    let short_head = &info.head[..8.min(info.head.len())];

    let mut parts = Vec::new();

    // Branch name (left-aligned)
    parts.push(format!("{:width$}", branch_display, width = widths.branch));

    // Short HEAD (fixed width)
    parts.push(short_head.to_string());

    // Ahead/behind (left-aligned in its column)
    if widths.ahead_behind > 0 {
        if !info.is_primary && (info.ahead > 0 || info.behind > 0) {
            parts.push(format!(
                "{:width$}",
                format!("↑{} ↓{}", info.ahead, info.behind),
                width = widths.ahead_behind
            ));
        } else {
            parts.push(" ".repeat(widths.ahead_behind));
        }
    }

    // Working tree diff (left-aligned in its column)
    if widths.working_diff > 0 {
        let (wt_added, wt_deleted) = info.working_tree_diff;
        if wt_added > 0 || wt_deleted > 0 {
            parts.push(format!(
                "{:width$}",
                format!("+{} -{}", wt_added, wt_deleted),
                width = widths.working_diff
            ));
        } else {
            parts.push(" ".repeat(widths.working_diff));
        }
    }

    // Branch diff (left-aligned in its column)
    if widths.branch_diff > 0 {
        if !info.is_primary {
            let (br_added, br_deleted) = info.branch_diff;
            if br_added > 0 || br_deleted > 0 {
                parts.push(format!(
                    "{:width$}",
                    format!("(+{} -{})", br_added, br_deleted),
                    width = widths.branch_diff
                ));
            } else {
                parts.push(" ".repeat(widths.branch_diff));
            }
        } else {
            parts.push(" ".repeat(widths.branch_diff));
        }
    }

    // States (left-aligned in its column)
    if widths.states > 0 {
        let states = format_states(info);
        if !states.is_empty() {
            parts.push(format!("{:width$}", states, width = widths.states));
        } else {
            parts.push(" ".repeat(widths.states));
        }
    }

    // Path (no padding needed, it's the last column)
    parts.push(info.path.display().to_string());

    let line = parts.join("  ");

    if info.is_primary {
        println!(
            "{}{}{}",
            primary_style.render(),
            line,
            primary_style.render_reset()
        );
    } else {
        println!("{}", line);
    }
}

fn handle_switch(
    branch: &str,
    create: bool,
    base: Option<&str>,
    internal: bool,
) -> Result<(), GitError> {
    // Check for conflicting conditions
    if create && branch_exists(branch)? {
        return Err(GitError::CommandFailed(format!(
            "Branch '{}' already exists. Remove --create flag to switch to it.",
            branch
        )));
    }

    // Check if base flag was provided without create flag
    if base.is_some() && !create {
        eprintln!("Warning: --base flag is only used with --create, ignoring");
    }

    // Check if worktree already exists for this branch
    if let Some(existing_path) = worktree_for_branch(branch)? {
        if existing_path.exists() {
            if internal {
                println!("__WORKTRUNK_CD__{}", existing_path.display());
            }
            return Ok(());
        } else {
            return Err(GitError::CommandFailed(format!(
                "Worktree directory missing for '{}'. Run 'git worktree prune' to clean up.",
                branch
            )));
        }
    }

    // No existing worktree, create one
    let git_common_dir = get_git_common_dir()?
        .canonicalize()
        .map_err(|e| GitError::CommandFailed(format!("Failed to canonicalize path: {}", e)))?;

    let repo_root = git_common_dir
        .parent()
        .ok_or_else(|| GitError::CommandFailed("Invalid git directory".to_string()))?;

    let repo_name = repo_root
        .file_name()
        .ok_or_else(|| GitError::CommandFailed("Invalid repository path".to_string()))?
        .to_str()
        .ok_or_else(|| GitError::CommandFailed("Invalid UTF-8 in path".to_string()))?;

    let parent_dir = repo_root
        .parent()
        .ok_or_else(|| GitError::CommandFailed("Invalid repository location".to_string()))?;

    let worktree_path = parent_dir.join(format!("{}.{}", repo_name, branch));

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

    let output = process::Command::new("git")
        .args(&args)
        .output()
        .map_err(|e| GitError::CommandFailed(e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(GitError::CommandFailed(stderr.to_string()));
    }

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
        println!("Path: {}", worktree_path.display());
        println!("Note: Use 'wt switch' (with shell integration) for automatic cd");
    }

    Ok(())
}

fn handle_finish(internal: bool) -> Result<(), GitError> {
    // Check for uncommitted changes
    if is_dirty()? {
        return Err(GitError::CommandFailed(
            "Working tree has uncommitted changes. Commit or stash them first.".to_string(),
        ));
    }

    // Get current state
    let current_branch = get_current_branch()?;
    let default_branch = get_default_branch()?;
    let in_worktree = is_in_worktree()?;

    // If we're on default branch and not in a worktree, nothing to do
    if !in_worktree && current_branch.as_deref() == Some(&default_branch) {
        if !internal {
            println!("Already on default branch '{}'", default_branch);
        }
        return Ok(());
    }

    if in_worktree {
        // In worktree: navigate to primary worktree and remove this one
        let worktree_root = get_worktree_root()?;
        let common_dir = get_git_common_dir()?
            .canonicalize()
            .map_err(|e| GitError::CommandFailed(format!("Failed to canonicalize path: {}", e)))?;

        let primary_worktree_dir = common_dir
            .parent()
            .ok_or_else(|| GitError::CommandFailed("Invalid git directory".to_string()))?;

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
            println!("Path: {}", primary_worktree_dir.display());
            println!("Note: Use 'wt finish' (with shell integration) for automatic cd");
        }
    } else {
        // In main repo but not on default branch: switch to default
        let output = process::Command::new("git")
            .args(["switch", &default_branch])
            .output()
            .map_err(|e| GitError::CommandFailed(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(GitError::CommandFailed(stderr.to_string()));
        }

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
        None => get_default_branch()?,
    };

    // Check if it's a fast-forward
    if !is_ancestor(&target_branch, "HEAD")? {
        return Err(GitError::CommandFailed(format!(
            "Not a fast-forward from '{}' to HEAD. The target branch has commits not in your current branch.",
            target_branch
        )));
    }

    // Check for merge commits unless allowed
    if !allow_merge_commits && has_merge_commits(&target_branch, "HEAD")? {
        return Err(GitError::CommandFailed(
            "Found merge commits in push range. Use --allow-merge-commits to push non-linear history.".to_string(),
        ));
    }

    // Configure receive.denyCurrentBranch if needed
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
            let push_files = get_changed_files(&target_branch, "HEAD")?;

            // Get files changed in the worktree
            let wt_status_output = process::Command::new("git")
                .args(["status", "--porcelain"])
                .current_dir(wt_path)
                .output()
                .map_err(|e| GitError::CommandFailed(e.to_string()))?;

            let wt_files: Vec<String> = String::from_utf8_lossy(&wt_status_output.stdout)
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
                eprintln!("Cannot push: conflicting uncommitted changes in:");
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
    let commit_count = count_commits(&target_branch, "HEAD")?;
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
    let git_common_dir = get_git_common_dir()?;

    // Perform the push
    let push_result = process::Command::new("git")
        .args([
            "push",
            git_common_dir.to_str().unwrap(),
            &format!("HEAD:{}", target_branch),
        ])
        .output()
        .map_err(|e| GitError::CommandFailed(e.to_string()))?;

    if !push_result.status.success() {
        let stderr = String::from_utf8_lossy(&push_result.stderr);
        return Err(GitError::CommandFailed(format!("Push failed: {}", stderr)));
    }

    println!("Successfully pushed to '{}'", target_branch);
    Ok(())
}

fn handle_merge(target: Option<&str>, keep: bool) -> Result<(), GitError> {
    // Get current branch
    let current_branch = get_current_branch()?
        .ok_or_else(|| GitError::CommandFailed("Not on a branch (detached HEAD)".to_string()))?;

    // Get target branch (default to default branch if not provided)
    let target_branch = match target {
        Some(b) => b.to_string(),
        None => get_default_branch()?,
    };

    // Check if already on target branch
    if current_branch == target_branch {
        println!("Already on '{}', nothing to merge", target_branch);
        return Ok(());
    }

    // Check for uncommitted changes
    if is_dirty()? {
        return Err(GitError::CommandFailed(
            "Working tree has uncommitted changes. Commit or stash them first.".to_string(),
        ));
    }

    // Rebase onto target
    println!("Rebasing onto '{}'...", target_branch);

    let rebase_result = process::Command::new("git")
        .args(["rebase", &target_branch])
        .output()
        .map_err(|e| GitError::CommandFailed(e.to_string()))?;

    if !rebase_result.status.success() {
        let stderr = String::from_utf8_lossy(&rebase_result.stderr);
        return Err(GitError::CommandFailed(format!(
            "Failed to rebase onto '{}': {}",
            target_branch, stderr
        )));
    }

    // Fast-forward push to target branch (reuse handle_push logic)
    println!("Fast-forwarding '{}' to current HEAD...", target_branch);
    handle_push(Some(&target_branch), false)?;

    // Finish worktree unless --keep was specified
    if !keep {
        println!("Cleaning up worktree...");

        // Get primary worktree path before finishing (while we can still run git commands)
        let common_dir = get_git_common_dir()?
            .canonicalize()
            .map_err(|e| GitError::CommandFailed(format!("Failed to canonicalize path: {}", e)))?;
        let primary_worktree_dir = common_dir
            .parent()
            .ok_or_else(|| GitError::CommandFailed("Invalid git directory".to_string()))?
            .to_path_buf();

        handle_finish(false)?;

        // Check if we need to switch to target branch
        let new_branch = get_current_branch_in(&primary_worktree_dir)?;
        if new_branch.as_deref() != Some(&target_branch) {
            println!("Switching to '{}'...", target_branch);
            let switch_result = process::Command::new("git")
                .args(["switch", &target_branch])
                .current_dir(&primary_worktree_dir)
                .output()
                .map_err(|e| GitError::CommandFailed(e.to_string()))?;

            if !switch_result.status.success() {
                let stderr = String::from_utf8_lossy(&switch_result.stderr);
                return Err(GitError::CommandFailed(format!(
                    "Failed to switch to '{}': {}",
                    target_branch, stderr
                )));
            }
        }
    } else {
        println!(
            "Successfully merged to '{}' (worktree preserved)",
            target_branch
        );
    }

    Ok(())
}

fn handle_hook(hook_type: &str) -> Result<(), String> {
    match hook_type {
        "prompt" => {
            // TODO: Implement prompt hook logic
            // This would update tracking, show current worktree, etc.
            Ok(())
        }
        _ => Err(format!("Unknown hook type: {}", hook_type)),
    }
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

    match subcommand.as_str() {
        "switch" => {
            // Check if we're completing --base flag value
            if args.len() >= 3 {
                for i in 2..args.len() - 1 {
                    if args[i] == "--base" || args[i] == "-b" {
                        // We're completing the value after --base
                        return CompletionContext::BaseFlag;
                    }
                }
            }
            CompletionContext::SwitchBranch
        }
        "push" => CompletionContext::PushTarget,
        "merge" => CompletionContext::MergeTarget,
        _ => CompletionContext::Unknown,
    }
}

fn handle_complete(args: Vec<String>) -> Result<(), GitError> {
    let context = parse_completion_context(&args);

    match context {
        CompletionContext::SwitchBranch => {
            // Complete with available branches (excluding those with worktrees)
            let branches = get_available_branches().unwrap_or_else(|e| {
                if std::env::var("WT_DEBUG_COMPLETION").is_ok() {
                    eprintln!("completion error: {}", e);
                }
                Vec::new()
            });
            for branch in branches {
                println!("{}", branch);
            }
        }
        CompletionContext::PushTarget
        | CompletionContext::MergeTarget
        | CompletionContext::BaseFlag => {
            // Complete with all branches
            let branches = get_all_branches().unwrap_or_else(|e| {
                if std::env::var("WT_DEBUG_COMPLETION").is_ok() {
                    eprintln!("completion error: {}", e);
                }
                Vec::new()
            });
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
}
