//! Worktree switch operations.
//!
//! Functions for planning and executing worktree switches.

use std::path::Path;

use anyhow::Context;
use color_print::cformat;
use dunce::canonicalize;
use worktrunk::config::WorktrunkConfig;
use worktrunk::git::pr_ref::fork_remote_url;
use worktrunk::git::{GitError, Repository};
use worktrunk::styling::{
    hint_message, info_message, progress_message, suggest_command, warning_message,
};

use super::resolve::{compute_clobber_backup, compute_worktree_path, paths_match};
use super::types::{CreationMethod, SwitchBranchInfo, SwitchPlan, SwitchResult};
use crate::commands::command_executor::CommandContext;

/// Result of resolving the switch target.
struct ResolvedTarget {
    /// The resolved branch name
    branch: String,
    /// How to create the worktree
    method: CreationMethod,
}

/// Resolve the switch target, handling pr: syntax and --create/--base flags.
///
/// This is the first phase of planning: determine what branch we're switching to
/// and how we'll create the worktree. May involve network calls for PR resolution.
fn resolve_switch_target(
    repo: &Repository,
    branch: &str,
    create: bool,
    base: Option<&str>,
) -> anyhow::Result<ResolvedTarget> {
    use worktrunk::git::pr_ref::{fetch_pr_info, local_branch_name};

    // Handle pr:<number> syntax
    if let Some(pr_number) = worktrunk::git::pr_ref::parse_pr_ref(branch) {
        // --create and --base are invalid with pr: syntax
        if create {
            return Err(GitError::PrCreateConflict { pr_number }.into());
        }
        if base.is_some() {
            return Err(GitError::PrBaseConflict { pr_number }.into());
        }

        // Fetch PR info (network call via gh CLI)
        crate::output::print(progress_message(cformat!("Fetching PR #{pr_number}...")))?;

        let repo_root = repo.repo_path()?;
        let pr_info = fetch_pr_info(pr_number, &repo_root)?;

        if pr_info.is_cross_repository {
            // Fork PR: will need fetch + pushRemote config (see pr_ref module docs)
            let local_branch = local_branch_name(&pr_info);

            // Check if branch already exists and is tracking this PR
            // If so, we can reuse it without re-fetching or re-configuring
            if let Some(tracks_this_pr) =
                worktrunk::git::pr_ref::branch_tracks_pr(&repo_root, &local_branch, pr_number)
            {
                if tracks_this_pr {
                    // Branch exists and tracks this PR - just create worktree
                    crate::output::print(info_message(cformat!(
                        "Branch <bold>{local_branch}</> already configured for PR #{pr_number}"
                    )))?;
                    return Ok(ResolvedTarget {
                        branch: local_branch,
                        method: CreationMethod::Regular {
                            create_branch: false,
                            base_branch: None,
                        },
                    });
                } else {
                    // Branch exists but tracks something else
                    return Err(GitError::BranchTracksDifferentPr {
                        branch: local_branch,
                        pr_number,
                    }
                    .into());
                }
            }

            // Branch doesn't exist - need full fork PR setup
            let remote_url = repo.primary_remote_url().unwrap_or_default();
            let fork_push_url =
                fork_remote_url(&pr_info.head_owner, &pr_info.head_repo, &remote_url);

            return Ok(ResolvedTarget {
                branch: local_branch,
                method: CreationMethod::ForkPr {
                    pr_number,
                    fork_push_url,
                    pr_url: pr_info.url,
                    base_owner: pr_info.base_owner,
                    base_repo: pr_info.base_repo,
                },
            });
        } else {
            // Same-repo PR: just use the branch name, regular switch
            return Ok(ResolvedTarget {
                branch: pr_info.head_ref_name,
                method: CreationMethod::Regular {
                    create_branch: false,
                    base_branch: None,
                },
            });
        }
    }

    // Regular branch switch
    let resolved_branch = repo
        .resolve_worktree_name(branch)
        .context("Failed to resolve branch name")?;

    // Resolve and validate base
    let resolved_base = if let Some(base_str) = base {
        let resolved = repo.resolve_worktree_name(base_str)?;
        if !create {
            crate::output::print(warning_message(
                "--base flag is only used with --create, ignoring",
            ))?;
            None
        } else if !repo.ref_exists(&resolved)? {
            return Err(GitError::InvalidReference {
                reference: resolved,
            }
            .into());
        } else {
            Some(resolved)
        }
    } else {
        None
    };

    // Validate --create constraints
    if create {
        let branch_handle = repo.branch(&resolved_branch);
        if branch_handle.exists_locally()? {
            return Err(GitError::BranchAlreadyExists {
                branch: resolved_branch,
            }
            .into());
        }

        // Warn if --create would shadow a remote branch
        let remotes = branch_handle.remotes()?;
        if !remotes.is_empty() {
            let remote_ref = format!("{}/{}", remotes[0], resolved_branch);
            crate::output::print(warning_message(cformat!(
                "Branch <bold>{resolved_branch}</> exists on remote ({remote_ref}); creating new branch from base instead"
            )))?;
            let remove_cmd = suggest_command("remove", &[&resolved_branch], &[]);
            let switch_cmd = suggest_command("switch", &[&resolved_branch], &[]);
            crate::output::print(hint_message(cformat!(
                "To switch to the remote branch, delete this branch and run without <bright-black>--create</>: <bright-black>{remove_cmd} && {switch_cmd}</>"
            )))?;
        }
    }

    // Compute base branch for creation
    let base_branch = if create {
        resolved_base.or_else(|| {
            // Check for invalid configured default branch
            if let Some(configured) = repo.invalid_default_branch_config() {
                let _ = crate::output::print(warning_message(cformat!(
                    "Configured default branch <bold>{configured}</> does not exist locally"
                )));
                let _ = crate::output::print(hint_message(cformat!(
                    "To reset, run <bright-black>wt config state default-branch clear</>"
                )));
            }
            repo.resolve_target_branch(None)
                .ok()
                .filter(|b| repo.branch(b).exists_locally().unwrap_or(false))
        })
    } else {
        None
    };

    Ok(ResolvedTarget {
        branch: resolved_branch,
        method: CreationMethod::Regular {
            create_branch: create,
            base_branch,
        },
    })
}

/// Check if branch already has a worktree.
///
/// Returns `Some(Existing)` if worktree exists and is valid.
/// Returns error if worktree record exists but directory is missing.
/// Returns `None` if no worktree exists for this branch.
fn check_existing_worktree(
    repo: &Repository,
    branch: &str,
    expected_path: &Path,
    new_previous: Option<String>,
) -> anyhow::Result<Option<SwitchPlan>> {
    match repo.worktree_for_branch(branch)? {
        Some(existing_path) if existing_path.exists() => Ok(Some(SwitchPlan::Existing {
            path: canonicalize(&existing_path).unwrap_or(existing_path),
            branch: branch.to_string(),
            expected_path: expected_path.to_path_buf(),
            new_previous,
        })),
        Some(_) => Err(GitError::WorktreeMissing {
            branch: branch.to_string(),
        }
        .into()),
        None => Ok(None),
    }
}

/// Validate that we can create a worktree at the given path.
///
/// Checks:
/// - Path not occupied by another worktree
/// - For regular switches (not --create), branch must exist
/// - Handles --clobber for stale directories
///
/// Note: Fork PR branch existence is checked earlier in resolve_switch_target()
/// where we can also check if it's tracking the correct PR.
fn validate_worktree_creation(
    repo: &Repository,
    branch: &str,
    path: &Path,
    clobber: bool,
    method: &CreationMethod,
) -> anyhow::Result<Option<std::path::PathBuf>> {
    // For regular switches without --create, validate branch exists
    if let CreationMethod::Regular {
        create_branch: false,
        ..
    } = method
        && !repo.branch(branch).exists()?
    {
        return Err(GitError::InvalidReference {
            reference: branch.to_string(),
        }
        .into());
    }

    // Check if path is occupied by another worktree
    if let Some((existing_path, occupant)) = repo.worktree_at_path(path)? {
        if !existing_path.exists() {
            let occupant_branch = occupant.unwrap_or_else(|| branch.to_string());
            return Err(GitError::WorktreeMissing {
                branch: occupant_branch,
            }
            .into());
        }
        return Err(GitError::WorktreePathOccupied {
            branch: branch.to_string(),
            path: path.to_path_buf(),
            occupant,
        }
        .into());
    }

    // Handle clobber for stale directories
    let is_create = matches!(
        method,
        CreationMethod::Regular {
            create_branch: true,
            ..
        }
    );
    compute_clobber_backup(path, branch, clobber, is_create)
}

/// Set up a local branch for a fork PR.
///
/// Creates the branch, configures tracking, and creates the worktree.
/// Returns an error if any step fails - caller is responsible for cleanup.
fn setup_fork_pr_branch(
    repo: &Repository,
    branch: &str,
    remote: &str,
    pr_ref: &str,
    fork_push_url: &str,
    worktree_path: &Path,
    pr_number: u32,
) -> anyhow::Result<()> {
    // Create local branch from FETCH_HEAD
    repo.run_command(&["branch", branch, "FETCH_HEAD"])
        .with_context(|| {
            format!(
                "Failed to create local branch '{}' from PR #{}",
                branch, pr_number
            )
        })?;

    // Configure branch tracking for pull and push
    let branch_remote_key = format!("branch.{}.remote", branch);
    let branch_merge_key = format!("branch.{}.merge", branch);
    let branch_push_remote_key = format!("branch.{}.pushRemote", branch);
    let merge_ref = format!("refs/{}", pr_ref);

    repo.run_command(&["config", &branch_remote_key, remote])
        .with_context(|| format!("Failed to configure branch.{}.remote", branch))?;
    repo.run_command(&["config", &branch_merge_key, &merge_ref])
        .with_context(|| format!("Failed to configure branch.{}.merge", branch))?;
    repo.run_command(&["config", &branch_push_remote_key, fork_push_url])
        .with_context(|| format!("Failed to configure branch.{}.pushRemote", branch))?;

    // Create worktree (delayed streaming: silent if fast, shows progress if slow)
    let worktree_path_str = worktree_path.to_string_lossy();
    repo.run_command_delayed_stream(
        &["worktree", "add", worktree_path_str.as_ref(), branch],
        Repository::SLOW_OPERATION_DELAY_MS,
        Some(
            progress_message(cformat!("Creating worktree for <bold>{}</>...", branch)).to_string(),
        ),
    )
    .map_err(|e| GitError::WorktreeCreationFailed {
        branch: branch.to_string(),
        base_branch: None,
        error: e.to_string(),
    })?;

    Ok(())
}

/// Validate and plan a switch operation.
///
/// This performs all validation upfront, returning a `SwitchPlan` that can be
/// executed later. Call this BEFORE approval prompts to ensure users aren't
/// asked to approve hooks for operations that will fail.
///
/// Warnings (remote branch shadow, --base without --create, invalid default branch)
/// are printed during planning since they're informational, not blocking.
pub fn plan_switch(
    repo: &Repository,
    branch: &str,
    create: bool,
    base: Option<&str>,
    clobber: bool,
    config: &WorktrunkConfig,
) -> anyhow::Result<SwitchPlan> {
    // Record current branch for `wt switch -` support
    let new_previous = repo.current_worktree().branch().ok().flatten();

    // Phase 1: Resolve target (handles pr:, validates --create/--base, may do network)
    let target = resolve_switch_target(repo, branch, create, base)?;

    // Phase 2: Compute expected path
    let expected_path = compute_worktree_path(repo, &target.branch, config)?;

    // Phase 3: Check if worktree already exists for this branch
    if let Some(existing) =
        check_existing_worktree(repo, &target.branch, &expected_path, new_previous.clone())?
    {
        return Ok(existing);
    }

    // Phase 4: Validate we can create at this path
    let clobber_backup = validate_worktree_creation(
        repo,
        &target.branch,
        &expected_path,
        clobber,
        &target.method,
    )?;

    // Phase 5: Return the plan
    Ok(SwitchPlan::Create {
        branch: target.branch,
        worktree_path: expected_path,
        method: target.method,
        clobber_backup,
        new_previous,
    })
}

/// Execute a validated switch plan.
///
/// Takes a `SwitchPlan` from `plan_switch()` and executes it.
/// For `SwitchPlan::Existing`, just records history.
/// For `SwitchPlan::Create`, creates the worktree and runs hooks.
pub fn execute_switch(
    repo: &Repository,
    plan: SwitchPlan,
    config: &WorktrunkConfig,
    force: bool,
    no_verify: bool,
) -> anyhow::Result<(SwitchResult, SwitchBranchInfo)> {
    match plan {
        SwitchPlan::Existing {
            path,
            branch,
            expected_path,
            new_previous,
        } => {
            let _ = repo.record_switch_previous(new_previous.as_deref());

            let current_dir = std::env::current_dir()
                .ok()
                .and_then(|p| canonicalize(&p).ok());
            let already_at_worktree = current_dir
                .as_ref()
                .map(|cur| cur == &path)
                .unwrap_or(false);

            let mismatch_path = if !paths_match(&path, &expected_path) {
                Some(expected_path)
            } else {
                None
            };

            let result = if already_at_worktree {
                SwitchResult::AlreadyAt(path)
            } else {
                SwitchResult::Existing(path)
            };

            Ok((
                result,
                SwitchBranchInfo {
                    branch,
                    expected_path: mismatch_path,
                },
            ))
        }

        SwitchPlan::Create {
            branch,
            worktree_path,
            method,
            clobber_backup,
            new_previous,
        } => {
            // Handle --clobber backup if needed (shared for all creation methods)
            if let Some(backup_path) = &clobber_backup {
                let path_display = worktrunk::path::format_path_for_display(&worktree_path);
                let backup_display = worktrunk::path::format_path_for_display(backup_path);
                crate::output::print(warning_message(cformat!(
                    "Moving <bold>{path_display}</> to <bold>{backup_display}</> (--clobber)"
                )))?;

                std::fs::rename(&worktree_path, backup_path).with_context(|| {
                    format!("Failed to move {path_display} to {backup_display}")
                })?;
            }

            // Execute based on creation method
            let (created_branch, base_branch, from_remote) = match &method {
                CreationMethod::Regular {
                    create_branch,
                    base_branch,
                } => {
                    // Check if local branch exists BEFORE git worktree add (for DWIM detection)
                    let branch_handle = repo.branch(&branch);
                    let local_branch_existed =
                        !create_branch && branch_handle.exists_locally().unwrap_or(false);

                    // Build git worktree add command
                    let worktree_path_str = worktree_path.to_string_lossy();
                    let mut args = vec!["worktree", "add", worktree_path_str.as_ref()];

                    if *create_branch {
                        args.push("-b");
                        args.push(&branch);
                        if let Some(base) = base_branch {
                            args.push(base);
                        }
                    } else {
                        args.push(&branch);
                    }

                    // Delayed streaming: silent if fast, shows progress if slow
                    let progress_msg = Some(
                        progress_message(cformat!("Creating worktree for <bold>{}</>...", branch))
                            .to_string(),
                    );
                    if let Err(e) = repo.run_command_delayed_stream(
                        &args,
                        Repository::SLOW_OPERATION_DELAY_MS,
                        progress_msg,
                    ) {
                        return Err(GitError::WorktreeCreationFailed {
                            branch: branch.clone(),
                            base_branch: base_branch.clone(),
                            error: e.to_string(),
                        }
                        .into());
                    }

                    // Safety: unset unsafe upstream when creating a new branch from a remote
                    // tracking branch. When `git worktree add -b feature origin/main` runs,
                    // git sets feature to track origin/main. This is dangerous because
                    // `git push` would push to main instead of the feature branch.
                    // See: https://github.com/max-sixty/worktrunk/issues/713
                    if *create_branch
                        && let Some(base) = base_branch
                        && repo.is_remote_tracking_branch(base)
                    {
                        // Unset the upstream to prevent accidental pushes
                        branch_handle.unset_upstream()?;
                    }

                    // Report tracking info only if git's DWIM created the branch from a remote
                    let from_remote = if !create_branch && !local_branch_existed {
                        branch_handle.upstream()?
                    } else {
                        None
                    };

                    (*create_branch, base_branch.clone(), from_remote)
                }

                CreationMethod::ForkPr {
                    pr_number,
                    fork_push_url,
                    pr_url: _,
                    base_owner,
                    base_repo,
                } => {
                    let pr_ref = format!("pull/{}/head", pr_number);

                    // Find the remote that points to the base repo (where PR refs live)
                    let remote = repo
                        .find_remote_for_repo(base_owner, base_repo)
                        .ok_or_else(|| {
                            // Construct suggested URL using primary remote's protocol/host
                            let reference_url = repo.primary_remote_url().unwrap_or_default();
                            let suggested_url =
                                fork_remote_url(base_owner, base_repo, &reference_url);
                            GitError::NoRemoteForRepo {
                                owner: base_owner.clone(),
                                repo: base_repo.clone(),
                                suggested_url,
                            }
                        })?;

                    // Fetch the PR head (progress already shown during planning)
                    repo.run_command(&["fetch", &remote, &pr_ref])
                        .with_context(|| {
                            format!("Failed to fetch PR #{} from {}", pr_number, remote)
                        })?;

                    // Execute branch creation and configuration with cleanup on failure.
                    // If any step after branch creation fails, we must delete the branch
                    // to avoid leaving orphaned state that blocks future attempts.
                    let setup_result = setup_fork_pr_branch(
                        repo,
                        &branch,
                        &remote,
                        &pr_ref,
                        fork_push_url,
                        &worktree_path,
                        *pr_number,
                    );

                    if let Err(e) = setup_result {
                        // Cleanup: try to delete the branch if it was created
                        // (ignore errors - branch may not exist if creation failed)
                        let _ = repo.run_command(&["branch", "-D", &branch]);
                        return Err(e);
                    }

                    crate::output::print(info_message(cformat!(
                        "Push configured to fork: <bright-black>{fork_push_url}</>"
                    )))?;

                    (false, None, Some(format!("PR #{}", pr_number)))
                }
            };

            // Compute base worktree path for hooks and result
            let base_worktree_path = base_branch
                .as_ref()
                .and_then(|b| repo.worktree_for_branch(b).ok().flatten())
                .map(|p| worktrunk::path::to_posix_path(&p.to_string_lossy()));

            // Execute post-create commands
            if !no_verify {
                let repo_root = repo.repo_path()?;
                let ctx = CommandContext::new(
                    repo,
                    config,
                    Some(&branch),
                    &worktree_path,
                    &repo_root,
                    force,
                );

                match &method {
                    CreationMethod::Regular { base_branch, .. } => {
                        let extra_vars: Vec<(&str, &str)> = [
                            base_branch.as_ref().map(|b| ("base", b.as_str())),
                            base_worktree_path
                                .as_ref()
                                .map(|p| ("base_worktree_path", p.as_str())),
                        ]
                        .into_iter()
                        .flatten()
                        .collect();
                        ctx.execute_post_create_commands(&extra_vars)?;
                    }
                    CreationMethod::ForkPr {
                        pr_number, pr_url, ..
                    } => {
                        let pr_num_str = pr_number.to_string();
                        let extra_vars: Vec<(&str, &str)> =
                            vec![("pr_number", &pr_num_str), ("pr_url", pr_url)];
                        ctx.execute_post_create_commands(&extra_vars)?;
                    }
                }
            }

            // Record successful switch in history
            let _ = repo.record_switch_previous(new_previous.as_deref());

            Ok((
                SwitchResult::Created {
                    path: worktree_path,
                    created_branch,
                    base_branch,
                    base_worktree_path,
                    from_remote,
                },
                SwitchBranchInfo {
                    branch,
                    expected_path: None,
                },
            ))
        }
    }
}
