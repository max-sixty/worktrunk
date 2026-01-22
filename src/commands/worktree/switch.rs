//! Worktree switch operations.
//!
//! Functions for planning and executing worktree switches.

use std::path::Path;

use anyhow::Context;
use color_print::cformat;
use dunce::canonicalize;
use worktrunk::config::UserConfig;
use worktrunk::git::mr_ref;
use worktrunk::git::pr_ref::{self, fork_remote_url, prefixed_local_branch_name};
use worktrunk::git::{GitError, RefContext, RefType, Repository};
use worktrunk::styling::{
    eprintln, format_with_gutter, hint_message, info_message, progress_message, suggest_command,
    warning_message,
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

/// Format PR/MR context for gutter display after fetching.
///
/// Returns two lines for gutter formatting:
/// ```text
///  ┃ Fix authentication bug in login flow (#101)
///  ┃ by @alice · open · https://github.com/owner/repo/pull/101
/// ```
fn format_ref_context(ctx: &impl RefContext) -> String {
    let mut status_parts = vec![format!("by @{}", ctx.author()), ctx.state().to_string()];
    if ctx.draft() {
        status_parts.push("draft".to_string());
    }
    let status_line = status_parts.join(" · ");

    cformat!(
        "<bold>{}</> ({}{})\n{status_line} · <bright-black>{}</>",
        ctx.title(),
        ctx.ref_type().symbol(),
        ctx.number(),
        ctx.url()
    )
}

/// Resolve a PR reference (`pr:<number>` syntax).
fn resolve_pr_ref(
    repo: &Repository,
    pr_number: u32,
    create: bool,
    base: Option<&str>,
) -> anyhow::Result<ResolvedTarget> {
    // --create and --base are invalid with pr: syntax
    if create {
        return Err(GitError::RefCreateConflict {
            ref_type: RefType::Pr,
            number: pr_number,
        }
        .into());
    }
    if base.is_some() {
        return Err(GitError::RefBaseConflict {
            ref_type: RefType::Pr,
            number: pr_number,
        }
        .into());
    }

    // Fetch PR info (network call via gh CLI)
    eprintln!(
        "{}",
        progress_message(cformat!("Fetching PR #{pr_number}..."))
    );

    let repo_root = repo.repo_path();
    let pr_info = pr_ref::fetch_pr_info(pr_number, repo_root)?;

    // Display PR context with URL (as gutter under fetch progress)
    eprintln!(
        "{}",
        format_with_gutter(&format_ref_context(&pr_info), None)
    );

    if pr_info.is_cross_repository {
        // Fork PR: check if branch already exists and is tracking this PR
        let local_branch = pr_ref::local_branch_name(&pr_info);

        // Determine if we need to use a prefixed branch name due to conflicts
        let (final_branch, fork_push_url) = if let Some(tracks_this) =
            pr_ref::branch_tracks_pr(repo_root, &local_branch, pr_number)
        {
            if tracks_this {
                eprintln!(
                    "{}",
                    info_message(cformat!(
                        "Branch <bold>{local_branch}</> already configured for PR #{pr_number}"
                    ))
                );
                return Ok(ResolvedTarget {
                    branch: local_branch,
                    method: CreationMethod::Regular {
                        create_branch: false,
                        base_branch: None,
                    },
                });
            } else {
                // Branch exists but doesn't track this PR - use prefixed name
                let prefixed = prefixed_local_branch_name(&pr_info);

                // Check if the prefixed branch also exists and tracks this PR
                if let Some(prefixed_tracks) =
                    pr_ref::branch_tracks_pr(repo_root, &prefixed, pr_number)
                {
                    if prefixed_tracks {
                        eprintln!(
                            "{}",
                            info_message(cformat!(
                                "Branch <bold>{prefixed}</> already configured for PR #{pr_number}"
                            ))
                        );
                        return Ok(ResolvedTarget {
                            branch: prefixed,
                            method: CreationMethod::Regular {
                                create_branch: false,
                                base_branch: None,
                            },
                        });
                    }
                    // Prefixed branch exists but tracks something else - error
                    return Err(GitError::BranchTracksDifferentRef {
                        branch: prefixed,
                        ref_type: RefType::Pr,
                        number: pr_number,
                    }
                    .into());
                }

                // Use prefixed branch name; push won't work (None for fork_push_url)
                (prefixed, None)
            }
        } else {
            // Branch doesn't exist - use unprefixed name with push support
            let fork_push_url =
                fork_remote_url(&pr_info.host, &pr_info.head_owner, &pr_info.head_repo);
            (local_branch, Some(fork_push_url))
        };

        // Resolve the remote now (during planning) to fail early if no matching remote exists
        let remote = repo
            .find_remote_for_repo(Some(&pr_info.host), &pr_info.base_owner, &pr_info.base_repo)
            .ok_or_else(|| {
                let suggested_url =
                    fork_remote_url(&pr_info.host, &pr_info.base_owner, &pr_info.base_repo);
                GitError::NoRemoteForRepo {
                    owner: pr_info.base_owner.clone(),
                    repo: pr_info.base_repo.clone(),
                    suggested_url,
                }
            })?;

        return Ok(ResolvedTarget {
            branch: final_branch,
            method: CreationMethod::ForkRef {
                ref_type: RefType::Pr,
                number: pr_number,
                fork_push_url,
                ref_url: pr_info.url,
                remote,
            },
        });
    }

    // Same-repo PR: fetch the branch to ensure remote refs are up-to-date.
    // Use host-aware matching for multi-host setups (e.g., github.com + github.enterprise.com).
    let remote = repo
        .find_remote_for_repo(Some(&pr_info.host), &pr_info.base_owner, &pr_info.base_repo)
        .ok_or_else(|| {
            let suggested_url =
                fork_remote_url(&pr_info.host, &pr_info.base_owner, &pr_info.base_repo);
            GitError::NoRemoteForRepo {
                owner: pr_info.base_owner.clone(),
                repo: pr_info.base_repo.clone(),
                suggested_url,
            }
        })?;
    let branch = &pr_info.head_ref_name;

    eprintln!(
        "{}",
        progress_message(cformat!("Fetching <bold>{branch}</> from {remote}..."))
    );
    repo.run_command(&["fetch", &remote, branch])
        .with_context(|| format!("Failed to fetch branch '{}' from {}", branch, remote))?;

    Ok(ResolvedTarget {
        branch: pr_info.head_ref_name,
        method: CreationMethod::Regular {
            create_branch: false,
            base_branch: None,
        },
    })
}

/// Resolve an MR reference (`mr:<number>` syntax).
fn resolve_mr_ref(
    repo: &Repository,
    mr_number: u32,
    create: bool,
    base: Option<&str>,
) -> anyhow::Result<ResolvedTarget> {
    // --create and --base are invalid with mr: syntax
    if create {
        return Err(GitError::RefCreateConflict {
            ref_type: RefType::Mr,
            number: mr_number,
        }
        .into());
    }
    if base.is_some() {
        return Err(GitError::RefBaseConflict {
            ref_type: RefType::Mr,
            number: mr_number,
        }
        .into());
    }

    // Fetch MR info (network call via glab CLI)
    eprintln!(
        "{}",
        progress_message(cformat!("Fetching MR !{mr_number}..."))
    );

    let repo_root = repo.repo_path();
    let mr_info = mr_ref::fetch_mr_info(mr_number, repo_root)?;

    // Display MR context with URL (as gutter under fetch progress)
    eprintln!(
        "{}",
        format_with_gutter(&format_ref_context(&mr_info), None)
    );

    if mr_info.is_cross_project {
        // Fork MR: check if branch already exists and is tracking this MR
        let local_branch = mr_ref::local_branch_name(&mr_info);

        if let Some(tracks_this) = mr_ref::branch_tracks_mr(repo_root, &local_branch, mr_number) {
            if tracks_this {
                eprintln!(
                    "{}",
                    info_message(cformat!(
                        "Branch <bold>{local_branch}</> already configured for MR !{mr_number}"
                    ))
                );
                return Ok(ResolvedTarget {
                    branch: local_branch,
                    method: CreationMethod::Regular {
                        create_branch: false,
                        base_branch: None,
                    },
                });
            } else {
                // TODO: Consider adding prefixed branch support for MRs like we do for PRs.
                // For now, MRs with conflicting branch names return an error.
                // See https://github.com/max-sixty/worktrunk/issues/714 for PR support.
                return Err(GitError::BranchTracksDifferentRef {
                    branch: local_branch,
                    ref_type: RefType::Mr,
                    number: mr_number,
                }
                .into());
            }
        }

        // Branch doesn't exist - need fork setup
        let fork_push_url = mr_ref::fork_remote_url(&mr_info).ok_or_else(|| {
            anyhow::anyhow!(
                "MR !{} is from a fork but glab didn't provide source project URL; \
                 upgrade glab or checkout the fork branch manually",
                mr_number
            )
        })?;
        let target_url = mr_ref::target_remote_url(&mr_info).ok_or_else(|| {
            anyhow::anyhow!(
                "MR !{} is from a fork but glab didn't provide target project URL; \
                 upgrade glab or checkout the fork branch manually",
                mr_number
            )
        })?;

        // Resolve the remote now (during planning) to fail early if no matching remote exists
        let remote = repo.find_remote_by_url(&target_url).ok_or_else(|| {
            anyhow::anyhow!(
                "No remote found for target project; \
                 add a remote pointing to {} (e.g., `git remote add upstream {}`)",
                target_url,
                target_url
            )
        })?;

        return Ok(ResolvedTarget {
            branch: local_branch,
            method: CreationMethod::ForkRef {
                ref_type: RefType::Mr,
                number: mr_number,
                fork_push_url: Some(fork_push_url),
                ref_url: mr_info.url,
                remote,
            },
        });
    }

    // Same-repo MR: just use the branch name
    Ok(ResolvedTarget {
        branch: mr_info.source_branch,
        method: CreationMethod::Regular {
            create_branch: false,
            base_branch: None,
        },
    })
}

/// Resolve the switch target, handling pr:/mr: syntax and --create/--base flags.
///
/// This is the first phase of planning: determine what branch we're switching to
/// and how we'll create the worktree. May involve network calls for PR/MR resolution.
fn resolve_switch_target(
    repo: &Repository,
    branch: &str,
    create: bool,
    base: Option<&str>,
) -> anyhow::Result<ResolvedTarget> {
    // Handle pr:<number> syntax
    if let Some(pr_number) = pr_ref::parse_pr_ref(branch) {
        return resolve_pr_ref(repo, pr_number, create, base);
    }

    // Handle mr:<number> syntax (GitLab MRs)
    if let Some(mr_number) = mr_ref::parse_mr_ref(branch) {
        return resolve_mr_ref(repo, mr_number, create, base);
    }

    // Regular branch switch
    let resolved_branch = repo
        .resolve_worktree_name(branch)
        .context("Failed to resolve branch name")?;

    // Resolve and validate base
    let resolved_base = if let Some(base_str) = base {
        let resolved = repo.resolve_worktree_name(base_str)?;
        if !create {
            eprintln!(
                "{}",
                warning_message("--base flag is only used with --create, ignoring")
            );
            None
        } else if !repo.ref_exists(&resolved)? {
            return Err(GitError::ReferenceNotFound {
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
            eprintln!(
                "{}",
                warning_message(cformat!(
                    "Branch <bold>{resolved_branch}</> exists on remote ({remote_ref}); creating new branch from base instead"
                ))
            );
            let remove_cmd = suggest_command("remove", &[&resolved_branch], &[]);
            let switch_cmd = suggest_command("switch", &[&resolved_branch], &[]);
            eprintln!(
                "{}",
                hint_message(cformat!(
                    "To switch to the remote branch, delete this branch and run without <bright-black>--create</>: <bright-black>{remove_cmd} && {switch_cmd}</>"
                ))
            );
        }
    }

    // Compute base branch for creation
    let base_branch = if create {
        resolved_base.or_else(|| {
            // Check for invalid configured default branch
            if let Some(configured) = repo.invalid_default_branch_config() {
                eprintln!(
                    "{}",
                    warning_message(cformat!(
                        "Configured default branch <bold>{configured}</> does not exist locally"
                    ))
                );
                eprintln!(
                    "{}",
                    hint_message(cformat!(
                        "To reset, run <bright-black>wt config state default-branch clear</>"
                    ))
                );
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
/// Note: Fork PR/MR branch existence is checked earlier in resolve_switch_target()
/// where we can also check if it's tracking the correct PR/MR.
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
        return Err(GitError::BranchNotFound {
            branch: branch.to_string(),
            show_create_hint: true,
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

/// Set up a local branch for a fork PR or MR.
///
/// Creates the branch from FETCH_HEAD, configures tracking (remote, merge ref,
/// pushRemote), and creates the worktree. Returns an error if any step fails -
/// caller is responsible for cleanup.
///
/// # Arguments
///
/// * `remote_ref` - The ref to track (e.g., "pull/123/head" or "merge-requests/101/head")
/// * `fork_push_url` - URL to push to, or `None` if push isn't supported (prefixed branch)
/// * `label` - Human-readable label for error messages (e.g., "PR #123" or "MR !101")
fn setup_fork_branch(
    repo: &Repository,
    branch: &str,
    remote: &str,
    remote_ref: &str,
    fork_push_url: Option<&str>,
    worktree_path: &Path,
    label: &str,
) -> anyhow::Result<()> {
    // Create local branch from FETCH_HEAD
    // Use -- to prevent branch names starting with - from being interpreted as flags
    repo.run_command(&["branch", "--", branch, "FETCH_HEAD"])
        .with_context(|| format!("Failed to create local branch '{}' from {}", branch, label))?;

    // Configure branch tracking for pull and push
    let branch_remote_key = format!("branch.{}.remote", branch);
    let branch_merge_key = format!("branch.{}.merge", branch);
    let merge_ref = format!("refs/{}", remote_ref);

    repo.run_command(&["config", &branch_remote_key, remote])
        .with_context(|| format!("Failed to configure branch.{}.remote", branch))?;
    repo.run_command(&["config", &branch_merge_key, &merge_ref])
        .with_context(|| format!("Failed to configure branch.{}.merge", branch))?;

    // Only configure pushRemote if we have a fork URL (not using prefixed branch)
    if let Some(url) = fork_push_url {
        let branch_push_remote_key = format!("branch.{}.pushRemote", branch);
        repo.run_command(&["config", &branch_push_remote_key, url])
            .with_context(|| format!("Failed to configure branch.{}.pushRemote", branch))?;
    }

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
    config: &UserConfig,
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
    config: &UserConfig,
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
            let _ = repo.set_switch_previous(new_previous.as_deref());

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
                SwitchResult::Existing { path }
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
                eprintln!(
                    "{}",
                    warning_message(cformat!(
                        "Moving <bold>{path_display}</> to <bold>{backup_display}</> (--clobber)"
                    ))
                );

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

                CreationMethod::ForkRef {
                    ref_type,
                    number,
                    fork_push_url,
                    ref_url: _,
                    remote,
                } => {
                    // Compute the ref path based on type
                    let remote_ref = match ref_type {
                        RefType::Pr => format!("pull/{}/head", number),
                        RefType::Mr => format!("merge-requests/{}/head", number),
                    };
                    let label = ref_type.display(*number);

                    // Fetch the ref (remote was resolved during planning)
                    repo.run_command(&["fetch", remote, &remote_ref])
                        .with_context(|| format!("Failed to fetch {} from {}", label, remote))?;

                    // Execute branch creation and configuration with cleanup on failure.
                    let setup_result = setup_fork_branch(
                        repo,
                        &branch,
                        remote,
                        &remote_ref,
                        fork_push_url.as_deref(),
                        &worktree_path,
                        &label,
                    );

                    if let Err(e) = setup_result {
                        // Cleanup: try to delete the branch if it was created
                        let _ = repo.run_command(&["branch", "-D", "--", &branch]);
                        return Err(e);
                    }

                    // Show push configuration or warning about prefixed branch
                    if let Some(url) = fork_push_url {
                        eprintln!(
                            "{}",
                            info_message(cformat!(
                                "Push configured to fork: <bright-black>{url}</>"
                            ))
                        );
                    } else {
                        // Prefixed branch name due to conflict - push won't work
                        eprintln!(
                            "{}",
                            warning_message(cformat!(
                                "Using prefixed branch name <bold>{branch}</> due to name conflict"
                            ))
                        );
                        eprintln!(
                            "{}",
                            hint_message(
                                "Push to fork is not supported with prefixed branches; feedback welcome at https://github.com/max-sixty/worktrunk/issues/714",
                            )
                        );
                    }

                    (false, None, Some(label))
                }
            };

            // Compute base worktree path for hooks and result
            let base_worktree_path = base_branch
                .as_ref()
                .and_then(|b| repo.worktree_for_branch(b).ok().flatten())
                .map(|p| worktrunk::path::to_posix_path(&p.to_string_lossy()));

            // Execute post-create commands
            if !no_verify {
                let ctx = CommandContext::new(repo, config, Some(&branch), &worktree_path, force);

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
                    CreationMethod::ForkRef {
                        ref_type,
                        number,
                        ref_url,
                        ..
                    } => {
                        let num_str = number.to_string();
                        let (num_key, url_key) = match ref_type {
                            RefType::Pr => ("pr_number", "pr_url"),
                            RefType::Mr => ("mr_number", "mr_url"),
                        };
                        let extra_vars: Vec<(&str, &str)> =
                            vec![(num_key, &num_str), (url_key, ref_url)];
                        ctx.execute_post_create_commands(&extra_vars)?;
                    }
                }
            }

            // Record successful switch in history
            let _ = repo.set_switch_previous(new_previous.as_deref());

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
