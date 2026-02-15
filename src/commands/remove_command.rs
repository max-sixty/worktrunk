//! Remove command handler.
//!
//! Orchestrates worktree removal for both git and jj repositories.
//! Handles single and multi-worktree removal, hook approval, and cleanup.

use std::collections::HashSet;

use anyhow::Context;
use worktrunk::HookType;
use worktrunk::config::UserConfig;
use worktrunk::git::{Repository, ResolvedWorktree};
use worktrunk::styling::{eprintln, info_message, warning_message};

use super::command_approval::approve_hooks;
use super::context::CommandEnv;
use super::worktree::{
    OperationMode, RemoveResult, handle_remove, handle_remove_current, resolve_worktree_arg,
};
use crate::output::handle_remove_output;

/// Options for the remove command.
pub struct RemoveOptions {
    pub branches: Vec<String>,
    pub delete_branch: bool,
    pub force_delete: bool,
    pub foreground: bool,
    pub no_background: bool,
    pub verify: bool,
    pub yes: bool,
    pub force: bool,
}

/// Handle the `wt remove` command.
///
/// Orchestrates worktree removal: VCS detection, validation, hook approval,
/// and execution. Supports both single and multi-worktree removal.
pub fn handle_remove_command(opts: RemoveOptions) -> anyhow::Result<()> {
    let RemoveOptions {
        branches,
        delete_branch,
        force_delete,
        foreground,
        no_background,
        verify,
        yes,
        force,
    } = opts;

    let config = UserConfig::load().context("Failed to load config")?;

    // Open workspace once, route by VCS type via downcast
    let workspace = worktrunk::workspace::open_workspace()?;
    let Some(repo) = workspace.as_any().downcast_ref::<Repository>() else {
        // JJ path: resolve targets, approve hooks, remove each workspace
        let cwd = std::env::current_dir()?;
        let targets = if branches.is_empty() {
            let name = workspace
                .current_name(&cwd)?
                .ok_or_else(|| anyhow::anyhow!("Not inside a jj workspace"))?;
            vec![name]
        } else {
            branches
        };

        let run_hooks = approve_remove_hooks(verify, yes)?;

        for name in &targets {
            let ws_path = workspace.workspace_path(name)?;
            super::handle_remove_jj::remove_jj_workspace_and_cd(
                &*workspace,
                name,
                &ws_path,
                run_hooks,
                yes,
                None,
            )?;
        }
        return Ok(());
    };

    // Handle deprecated --no-background flag
    if no_background {
        eprintln!(
            "{}",
            warning_message("--no-background is deprecated; use --foreground instead")
        );
    }
    let background = !(foreground || no_background);

    // Validate conflicting flags
    if !delete_branch && force_delete {
        return Err(worktrunk::git::GitError::Other {
            message: "Cannot use --force-delete with --no-delete-branch".into(),
        }
        .into());
    }

    if branches.is_empty() {
        // Single worktree removal: validate FIRST, then approve, then execute
        let result = handle_remove_current(!delete_branch, force_delete, force, &config)
            .context("Failed to remove worktree")?;

        // Early exit for benchmarking time-to-first-output
        if std::env::var_os("WORKTRUNK_FIRST_OUTPUT").is_some() {
            return Ok(());
        }

        // "Approve at the Gate": approval happens AFTER validation passes
        let run_hooks = approve_remove_hooks(verify, yes)?;

        handle_remove_output(&result, background, run_hooks)
    } else {
        // Multi-worktree removal: validate ALL first, then approve, then execute
        // This supports partial success - some may fail validation while others succeed.
        let current_worktree = repo
            .current_worktree()
            .root()
            .ok()
            .and_then(|p| dunce::canonicalize(&p).ok());

        // Dedupe inputs to avoid redundant planning/execution
        let branches: Vec<_> = {
            let mut seen = HashSet::new();
            branches
                .into_iter()
                .filter(|b| seen.insert(b.clone()))
                .collect()
        };

        // Phase 1: Validate all targets (resolution + preparation)
        // Store successful plans for execution after approval
        let mut plans_others: Vec<RemoveResult> = Vec::new();
        let mut plans_branch_only: Vec<RemoveResult> = Vec::new();
        let mut plan_current: Option<RemoveResult> = None;
        let mut all_errors: Vec<anyhow::Error> = Vec::new();

        // Helper: record error and continue
        let mut record_error = |e: anyhow::Error| {
            eprintln!("{}", e);
            all_errors.push(e);
        };

        for branch_name in &branches {
            // Resolve the target
            let resolved =
                match resolve_worktree_arg(repo, branch_name, &config, OperationMode::Remove) {
                    Ok(r) => r,
                    Err(e) => {
                        record_error(e);
                        continue;
                    }
                };

            match resolved {
                ResolvedWorktree::Worktree { path, branch } => {
                    // Use canonical paths to avoid symlink/normalization mismatches
                    let path_canonical = dunce::canonicalize(&path).unwrap_or(path);
                    let is_current = current_worktree.as_ref() == Some(&path_canonical);

                    if is_current {
                        // Current worktree - use handle_remove_current for detached HEAD
                        match handle_remove_current(!delete_branch, force_delete, force, &config) {
                            Ok(result) => plan_current = Some(result),
                            Err(e) => record_error(e),
                        }
                        continue;
                    }

                    // Non-current worktree - branch is always Some because:
                    // - "@" resolves to current worktree (handled by is_current branch above)
                    // - Other names resolve via resolve_worktree_arg which always sets branch: Some(...)
                    let branch_for_remove = branch.as_ref().unwrap();

                    match handle_remove(
                        branch_for_remove,
                        !delete_branch,
                        force_delete,
                        force,
                        &config,
                    ) {
                        Ok(result) => plans_others.push(result),
                        Err(e) => record_error(e),
                    }
                }
                ResolvedWorktree::BranchOnly { branch } => {
                    match handle_remove(&branch, !delete_branch, force_delete, force, &config) {
                        Ok(result) => plans_branch_only.push(result),
                        Err(e) => record_error(e),
                    }
                }
            }
        }

        // If no valid plans, bail early (all failed validation)
        let has_valid_plans =
            !plans_others.is_empty() || !plans_branch_only.is_empty() || plan_current.is_some();
        if !has_valid_plans {
            anyhow::bail!("");
        }

        // Early exit for benchmarking time-to-first-output
        if std::env::var_os("WORKTRUNK_FIRST_OUTPUT").is_some() {
            return Ok(());
        }

        // Phase 2: Approve hooks (only if we have valid plans)
        // TODO(pre-remove-context): Approval context uses current worktree,
        // but hooks execute in each target worktree.
        let run_hooks = approve_remove_hooks(verify, yes)?;

        // Phase 3: Execute all validated plans
        // Remove other worktrees first
        for result in plans_others {
            handle_remove_output(&result, background, run_hooks)?;
        }

        // Handle branch-only cases
        for result in plans_branch_only {
            handle_remove_output(&result, background, run_hooks)?;
        }

        // Remove current worktree last (if it was in the list)
        if let Some(result) = plan_current {
            handle_remove_output(&result, background, run_hooks)?;
        }

        // Exit with failure if any validation errors occurred
        if !all_errors.is_empty() {
            anyhow::bail!("");
        }

        Ok(())
    }
}

/// Approve remove hooks if verification is enabled.
///
/// Shared between git and jj remove paths. Returns `true` if hooks should run.
fn approve_remove_hooks(verify: bool, yes: bool) -> anyhow::Result<bool> {
    if !verify {
        return Ok(false);
    }
    let env = CommandEnv::for_action_branchless()?;
    let ctx = env.context(yes);
    let approved = approve_hooks(
        &ctx,
        &[
            HookType::PreRemove,
            HookType::PostRemove,
            HookType::PostSwitch,
        ],
    )?;
    if !approved {
        eprintln!("{}", info_message("Commands declined, continuing removal"));
    }
    Ok(approved)
}
