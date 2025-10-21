use std::path::PathBuf;
use worktrunk::config::WorktrunkConfig;
use worktrunk::git::{GitError, Repository};
use worktrunk::styling::{format_error, format_error_with_bold, format_hint};

/// Result of a worktree switch operation
pub enum SwitchResult {
    /// Switched to existing worktree at the given path
    ExistingWorktree(PathBuf),
    /// Created new worktree at the given path
    CreatedWorktree { path: PathBuf, created_branch: bool },
}

impl SwitchResult {
    /// Format the result for display (non-internal mode)
    pub fn format_user_output(&self, branch: &str) -> Option<String> {
        match self {
            SwitchResult::ExistingWorktree(_) => None,
            SwitchResult::CreatedWorktree {
                path,
                created_branch,
            } => {
                let msg = if *created_branch {
                    format!(
                        "Created new branch and worktree for '{}' at {}",
                        branch,
                        path.display()
                    )
                } else {
                    format!(
                        "Added worktree for existing branch '{}' at {}",
                        branch,
                        path.display()
                    )
                };
                Some(format!(
                    "{}\n\nTo enable automatic cd, run: wt configure-shell",
                    msg
                ))
            }
        }
    }

    /// Format the result for shell integration (internal mode)
    pub fn format_internal_output(&self, branch: &str) -> Option<String> {
        match self {
            SwitchResult::ExistingWorktree(path) => {
                Some(format!("__WORKTRUNK_CD__{}", path.display()))
            }
            SwitchResult::CreatedWorktree {
                path,
                created_branch,
            } => {
                let msg = if *created_branch {
                    format!("Created new branch and worktree for '{}'", branch)
                } else {
                    format!("Added worktree for existing branch '{}'", branch)
                };
                Some(format!(
                    "__WORKTRUNK_CD__{}\n{} at {}",
                    path.display(),
                    msg,
                    path.display()
                ))
            }
        }
    }
}

/// Result of a worktree remove operation
pub enum RemoveResult {
    /// Already on default branch, no action taken
    AlreadyOnDefault(String),
    /// Removed worktree and returned to primary
    RemovedWorktree { primary_path: PathBuf },
    /// Switched to default branch in main repo
    SwitchedToDefault(String),
}

impl RemoveResult {
    /// Format the result for display (non-internal mode)
    pub fn format_user_output(&self) -> Option<String> {
        match self {
            RemoveResult::AlreadyOnDefault(branch) => {
                Some(format!("Already on default branch '{}'", branch))
            }
            RemoveResult::RemovedWorktree { primary_path } => Some(format!(
                "Moved to primary worktree and removed worktree\nPath: {}\n\nTo enable automatic cd, run: wt configure-shell",
                primary_path.display()
            )),
            RemoveResult::SwitchedToDefault(branch) => {
                Some(format!("Switched to default branch '{}'", branch))
            }
        }
    }

    /// Format the result for shell integration (internal mode)
    pub fn format_internal_output(&self) -> Option<String> {
        match self {
            RemoveResult::AlreadyOnDefault(_) => None,
            RemoveResult::RemovedWorktree { primary_path } => {
                Some(format!("__WORKTRUNK_CD__{}", primary_path.display()))
            }
            RemoveResult::SwitchedToDefault(_) => None,
        }
    }
}

pub fn handle_switch(
    branch: &str,
    create: bool,
    base: Option<&str>,
    execute: Option<&str>,
    config: &WorktrunkConfig,
) -> Result<SwitchResult, GitError> {
    let repo = Repository::current();

    // Check for conflicting conditions
    if create && repo.branch_exists(branch)? {
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
            worktrunk::styling::format_warning("--base flag is only used with --create, ignoring")
        );
    }

    // Check if worktree already exists for this branch
    match repo.worktree_for_branch(branch)? {
        Some(existing_path) if existing_path.exists() => {
            if let Some(cmd) = execute {
                execute_command_in_worktree(&existing_path, cmd)?;
            }
            // Canonicalize the path for cleaner display
            let canonical_existing_path = existing_path.canonicalize().unwrap_or(existing_path);
            return Ok(SwitchResult::ExistingWorktree(canonical_existing_path));
        }
        Some(_) => {
            return Err(GitError::CommandFailed(format_error_with_bold(
                "Worktree directory missing for '",
                branch,
                "'. Run 'git worktree prune' to clean up.",
            )));
        }
        None => {}
    }

    // No existing worktree, create one
    let repo_root = repo.repo_root()?;

    let repo_name = repo_root
        .file_name()
        .ok_or_else(|| GitError::CommandFailed("Invalid repository path".to_string()))?
        .to_str()
        .ok_or_else(|| GitError::CommandFailed("Invalid UTF-8 in path".to_string()))?;

    let worktree_path = repo_root.join(config.format_path(repo_name, branch));

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

    repo.run_command(&args)
        .map_err(|e| GitError::CommandFailed(format!("Failed to create worktree: {}", e)))?;

    if let Some(cmd) = execute {
        execute_command_in_worktree(&worktree_path, cmd)?;
    }

    // Canonicalize the path for cleaner display
    let canonical_path = worktree_path.canonicalize().unwrap_or(worktree_path);

    Ok(SwitchResult::CreatedWorktree {
        path: canonical_path,
        created_branch: create,
    })
}

/// Execute a command in the specified worktree directory
fn execute_command_in_worktree(
    worktree_path: &std::path::Path,
    command: &str,
) -> Result<(), GitError> {
    use std::io::Write;
    use std::process::Command;

    // Use platform-specific shell
    #[cfg(target_os = "windows")]
    let (shell, shell_arg) = ("cmd", "/C");
    #[cfg(not(target_os = "windows"))]
    let (shell, shell_arg) = ("sh", "-c");

    let output = Command::new(shell)
        .arg(shell_arg)
        .arg(command)
        .current_dir(worktree_path)
        .output()
        .map_err(|e| GitError::CommandFailed(format!("Failed to execute command: {}", e)))?;

    // Forward stdout/stderr to user
    std::io::stdout().write_all(&output.stdout).ok();
    std::io::stderr().write_all(&output.stderr).ok();

    if !output.status.success() {
        return Err(GitError::CommandFailed(format!(
            "Command '{}' exited with status: {}",
            command,
            output.status.code().unwrap_or(-1)
        )));
    }

    Ok(())
}

pub fn handle_remove() -> Result<RemoveResult, GitError> {
    let repo = Repository::current();

    // Check for uncommitted changes
    repo.ensure_clean_working_tree()?;

    // Get current state
    let current_branch = repo.current_branch()?;
    let default_branch = repo.default_branch()?;
    let in_worktree = repo.is_in_worktree()?;

    // If we're on default branch and not in a worktree, nothing to do
    if !in_worktree && current_branch.as_deref() == Some(&default_branch) {
        return Ok(RemoveResult::AlreadyOnDefault(default_branch));
    }

    if in_worktree {
        // In worktree: navigate to primary worktree and remove this one
        let worktree_root = repo.worktree_root()?;
        let primary_worktree_dir = repo.repo_root()?;

        // Remove the worktree
        if let Err(e) = repo.remove_worktree(&worktree_root) {
            eprintln!("Warning: Failed to remove worktree: {}", e);
            eprintln!(
                "You may need to run 'git worktree remove {}' manually",
                worktree_root.display()
            );
        }

        // Canonicalize the path for cleaner display
        let canonical_primary_path = primary_worktree_dir
            .canonicalize()
            .unwrap_or(primary_worktree_dir);

        Ok(RemoveResult::RemovedWorktree {
            primary_path: canonical_primary_path,
        })
    } else {
        // In main repo but not on default branch: switch to default
        repo.run_command(&["switch", &default_branch])
            .map_err(|e| {
                GitError::CommandFailed(format!("Failed to switch to '{}': {}", default_branch, e))
            })?;

        Ok(RemoveResult::SwitchedToDefault(default_branch))
    }
}

/// Check for conflicting uncommitted changes in target worktree
fn check_worktree_conflicts(
    repo: &Repository,
    target_worktree: &Option<std::path::PathBuf>,
    target_branch: &str,
) -> Result<(), GitError> {
    let Some(wt_path) = target_worktree else {
        return Ok(());
    };

    let wt_repo = Repository::at(wt_path);
    if !wt_repo.is_dirty()? {
        return Ok(());
    }

    // Get files changed in the push
    let push_files = repo.changed_files(target_branch, "HEAD")?;

    // Get files changed in the worktree
    let wt_status_output = wt_repo.run_command(&["status", "--porcelain"])?;

    let wt_files: Vec<String> = wt_status_output
        .lines()
        .filter_map(|line| {
            // Parse porcelain format: "XY filename"
            line.split_once(' ')
                .map(|(_, filename)| filename.trim().to_string())
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

    Ok(())
}

pub fn handle_push(target: Option<&str>, allow_merge_commits: bool) -> Result<(), GitError> {
    let repo = Repository::current();

    // Get target branch (default to default branch if not provided)
    let target_branch = target.map_or_else(|| repo.default_branch(), |b| Ok(b.to_string()))?;

    // Check if it's a fast-forward
    if !repo.is_ancestor(&target_branch, "HEAD")? {
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
    if !allow_merge_commits && repo.has_merge_commits(&target_branch, "HEAD")? {
        return Err(GitError::CommandFailed(format_error(
            "Found merge commits in push range. Use --allow-merge-commits to push non-linear history.",
        )));
    }

    // Configure receive.denyCurrentBranch if needed
    let current_config = repo.get_config("receive.denyCurrentBranch")?;
    if current_config.as_deref() != Some("updateInstead") {
        repo.set_config("receive.denyCurrentBranch", "updateInstead")?;
    }

    // Check for conflicting changes in target worktree
    let target_worktree = repo.worktree_for_branch(&target_branch)?;
    check_worktree_conflicts(&repo, &target_worktree, &target_branch)?;

    // Count commits and show info
    let commit_count = repo.count_commits(&target_branch, "HEAD")?;
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
    let git_common_dir = repo.git_common_dir()?;

    // Perform the push
    let push_target = format!("HEAD:{}", target_branch);
    repo.run_command(&["push", git_common_dir.to_str().unwrap(), &push_target])
        .map_err(|e| GitError::CommandFailed(format!("Push failed: {}", e)))?;

    println!("Successfully pushed to '{}'", target_branch);
    Ok(())
}
