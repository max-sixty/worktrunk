use clap::{Parser, Subcommand};
use std::process;
use worktrunk::git::{
    GitError, branch_exists, count_commits, get_changed_files, get_current_branch,
    get_default_branch, get_git_common_dir, get_worktree_root, has_merge_commits, is_ancestor,
    is_dirty, is_dirty_in, is_in_worktree, list_worktrees, worktree_for_branch,
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
        /// Target branch to merge into
        target: String,

        /// Squash commits
        #[arg(long)]
        squash: bool,
    },

    /// Hook commands (for shell integration)
    Hook {
        /// Hook type
        hook_type: String,
    },
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
        Commands::Merge { target, squash } => {
            handle_merge(&target, squash).map_err(GitError::CommandFailed)
        }
        Commands::Hook { hook_type } => handle_hook(&hook_type).map_err(GitError::CommandFailed),
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

fn handle_list() -> Result<(), GitError> {
    let worktrees = list_worktrees()?;

    for wt in worktrees {
        println!("{}", wt.path.display());
        println!("  HEAD: {}", &wt.head[..8.min(wt.head.len())]);

        if let Some(branch) = wt.branch {
            println!("  branch: {}", branch);
        }

        if wt.detached {
            println!("  (detached)");
        }

        if wt.bare {
            println!("  (bare)");
        }

        if let Some(reason) = wt.locked {
            if reason.is_empty() {
                println!("  (locked)");
            } else {
                println!("  (locked: {})", reason);
            }
        }

        if let Some(reason) = wt.prunable {
            if reason.is_empty() {
                println!("  (prunable)");
            } else {
                println!("  (prunable: {})", reason);
            }
        }

        println!();
    }

    Ok(())
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
    let output = if create {
        if let Some(base_branch) = base {
            process::Command::new("git")
                .args([
                    "worktree",
                    "add",
                    worktree_path.to_str().unwrap(),
                    "-b",
                    branch,
                    base_branch,
                ])
                .output()
                .map_err(|e| GitError::CommandFailed(e.to_string()))?
        } else {
            process::Command::new("git")
                .args([
                    "worktree",
                    "add",
                    worktree_path.to_str().unwrap(),
                    "-b",
                    branch,
                ])
                .output()
                .map_err(|e| GitError::CommandFailed(e.to_string()))?
        }
    } else {
        process::Command::new("git")
            .args(["worktree", "add", worktree_path.to_str().unwrap(), branch])
            .output()
            .map_err(|e| GitError::CommandFailed(e.to_string()))?
    };

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
        println!("Note: Use 'wt-switch' (with shell integration) for automatic cd");
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
            println!("Note: Use 'wt-finish' (with shell integration) for automatic cd");
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

fn handle_merge(target: &str, squash: bool) -> Result<(), String> {
    // TODO: Implement actual merge logic
    println!("Merging into: {} (squash: {})", target, squash);
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
