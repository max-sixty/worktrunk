//! Worktree switch operations.
//!
//! Functions for planning and executing worktree switches.

use std::path::Path;

use anyhow::Context;
use color_print::cformat;
use dunce::canonicalize;
use worktrunk::config::UserConfig;
use worktrunk::git::remote_ref::{
    self, GitHubProvider, GitLabProvider, RemoteRefInfo, RemoteRefProvider,
};
use worktrunk::git::{GitError, RefContext, RefType, Repository};
use worktrunk::styling::{
    eprintln, format_with_gutter, hint_message, info_message, progress_message, suggest_command,
    warning_message,
};

use super::resolve::{compute_clobber_backup, compute_worktree_path};
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
///  ┃ by @alice · open · feature-auth · https://github.com/owner/repo/pull/101
/// ```
fn format_ref_context(ctx: &impl RefContext) -> String {
    let mut status_parts = vec![format!("by @{}", ctx.author()), ctx.state().to_string()];
    if ctx.draft() {
        status_parts.push("draft".to_string());
    }
    status_parts.push(ctx.source_ref());
    let status_line = status_parts.join(" · ");

    cformat!(
        "<bold>{}</> ({}{})\n{status_line} · <bright-black>{}</>",
        ctx.title(),
        ctx.ref_type().symbol(),
        ctx.number(),
        ctx.url()
    )
}

/// Resolve a remote ref (PR or MR) using the unified provider interface.
fn resolve_remote_ref(
    repo: &Repository,
    provider: &dyn RemoteRefProvider,
    number: u32,
    create: bool,
    base: Option<&str>,
) -> anyhow::Result<ResolvedTarget> {
    let ref_type = provider.ref_type();
    let symbol = ref_type.symbol();

    // --base is invalid with pr:/mr: syntax (check early, no network needed)
    if base.is_some() {
        return Err(GitError::RefBaseConflict { ref_type, number }.into());
    }

    // Fetch ref info (network call via gh/glab CLI)
    eprintln!(
        "{}",
        progress_message(cformat!("Fetching {} {symbol}{number}...", ref_type.name()))
    );

    let repo_root = repo.repo_path();
    let info = provider.fetch_info(number, repo_root)?;

    // Display context with URL (as gutter under fetch progress)
    eprintln!("{}", format_with_gutter(&format_ref_context(&info), None));

    // --create is invalid with pr:/mr: syntax (check after fetch to show branch name)
    if create {
        return Err(GitError::RefCreateConflict {
            ref_type,
            number,
            branch: info.source_branch.clone(),
        }
        .into());
    }

    if info.is_cross_repo {
        return resolve_fork_ref(repo, provider, number, &info);
    }

    // Same-repo ref: fetch the branch to ensure remote tracking refs exist
    resolve_same_repo_ref(repo, &info)
}

/// Resolve a fork (cross-repo) PR/MR.
fn resolve_fork_ref(
    repo: &Repository,
    provider: &dyn RemoteRefProvider,
    number: u32,
    info: &RemoteRefInfo,
) -> anyhow::Result<ResolvedTarget> {
    let ref_type = provider.ref_type();
    let repo_root = repo.repo_path();
    let local_branch = remote_ref::local_branch_name(info);

    // Check if branch already exists and is tracking this ref
    if let Some(tracks_this) =
        remote_ref::branch_tracks_ref(repo_root, &local_branch, provider, number)
    {
        if tracks_this {
            eprintln!(
                "{}",
                info_message(cformat!(
                    "Branch <bold>{local_branch}</> already configured for {}",
                    ref_type.display(number)
                ))
            );
            return Ok(ResolvedTarget {
                branch: local_branch,
                method: CreationMethod::Regular {
                    create_branch: false,
                    base_branch: None,
                },
            });
        }

        // Branch exists but doesn't track this ref - try prefixed name (GitHub only)
        if let Some(prefixed) = info.prefixed_local_branch_name() {
            if let Some(prefixed_tracks) =
                remote_ref::branch_tracks_ref(repo_root, &prefixed, provider, number)
            {
                if prefixed_tracks {
                    eprintln!(
                        "{}",
                        info_message(cformat!(
                            "Branch <bold>{prefixed}</> already configured for {}",
                            ref_type.display(number)
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
                    ref_type,
                    number,
                }
                .into());
            }

            // Use prefixed branch name; push won't work (None for fork_push_url)
            // This is GitHub-only (GitLab doesn't support prefixed names)
            let remote = find_github_remote(repo, info)?;
            return Ok(ResolvedTarget {
                branch: prefixed,
                method: CreationMethod::ForkRef {
                    ref_type,
                    number,
                    ref_path: provider.ref_path(number),
                    fork_push_url: None,
                    ref_url: info.url.clone(),
                    remote,
                },
            });
        }

        // GitLab doesn't support prefixed branch names - error
        return Err(GitError::BranchTracksDifferentRef {
            branch: local_branch,
            ref_type,
            number,
        }
        .into());
    }

    // Branch doesn't exist - need to create it with push support.
    // Resolve remote and URLs based on platform.
    let (fork_push_url, remote) = match ref_type {
        RefType::Pr => {
            // GitHub: URLs already in info, just find remote
            let remote = find_github_remote(repo, info)?;
            (info.fork_push_url.clone(), remote)
        }
        RefType::Mr => {
            // GitLab: fetch project URLs now (deferred from fetch_mr_info for perf)
            let urls =
                worktrunk::git::remote_ref::gitlab::fetch_gitlab_project_urls(info, repo_root)?;
            let target_url = urls.target_url.ok_or_else(|| {
                anyhow::anyhow!(
                    "{} is from a fork but glab didn't provide target project URL; \
                     upgrade glab or checkout the fork branch manually",
                    ref_type.display(number)
                )
            })?;
            // TODO(gitlab-protocol): We only try the URL based on glab's git_protocol setting.
            // If the user's remote uses the other protocol (ssh vs https), we'll fail to find it.
            // Consider trying both ssh and https URLs before erroring.
            let remote = repo.find_remote_by_url(&target_url).ok_or_else(|| {
                anyhow::anyhow!(
                    "No remote found for target project; \
                     add a remote pointing to {} (e.g., `git remote add upstream {}`)",
                    target_url,
                    target_url
                )
            })?;
            if urls.fork_push_url.is_none() {
                anyhow::bail!(
                    "{} is from a fork but glab didn't provide source project URL; \
                     upgrade glab or checkout the fork branch manually",
                    ref_type.display(number)
                );
            }
            (urls.fork_push_url, remote)
        }
    };

    Ok(ResolvedTarget {
        branch: local_branch,
        method: CreationMethod::ForkRef {
            ref_type,
            number,
            ref_path: provider.ref_path(number),
            fork_push_url,
            ref_url: info.url.clone(),
            remote,
        },
    })
}

/// Find the remote for a GitHub PR (where PR refs live).
fn find_github_remote(repo: &Repository, info: &RemoteRefInfo) -> anyhow::Result<String> {
    use worktrunk::git::remote_ref::PlatformData;

    let PlatformData::GitHub {
        host,
        base_owner,
        base_repo,
        ..
    } = &info.platform_data
    else {
        anyhow::bail!("find_github_remote called on non-GitHub ref");
    };

    repo.find_remote_for_repo(Some(host), base_owner, base_repo)
        .ok_or_else(|| {
            let suggested_url =
                worktrunk::git::remote_ref::github::fork_remote_url(host, base_owner, base_repo);
            GitError::NoRemoteForRepo {
                owner: base_owner.clone(),
                repo: base_repo.clone(),
                suggested_url,
            }
            .into()
        })
}

/// Resolve a same-repo (non-fork) PR/MR.
fn resolve_same_repo_ref(
    repo: &Repository,
    info: &RemoteRefInfo,
) -> anyhow::Result<ResolvedTarget> {
    use worktrunk::git::remote_ref::PlatformData;

    // Find the remote for the same-repo PR/MR and fetch the branch with an
    // explicit refspec. This ensures the remote tracking branch is created even
    // in repos with limited fetch refspecs (single-branch clones, bare repos).
    let remote = match &info.platform_data {
        PlatformData::GitHub {
            host,
            base_owner,
            base_repo,
            ..
        } => {
            let suggested_url =
                worktrunk::git::remote_ref::github::fork_remote_url(host, base_owner, base_repo);
            repo.find_remote_for_repo(Some(host), base_owner, base_repo)
                .ok_or_else(|| GitError::NoRemoteForRepo {
                    owner: base_owner.clone(),
                    repo: base_repo.clone(),
                    suggested_url,
                })?
        }
        PlatformData::GitLab {
            host,
            base_owner,
            base_repo,
            ..
        } => repo
            .find_remote_for_repo(Some(host), base_owner, base_repo)
            .ok_or_else(|| GitError::NoRemoteForRepo {
                owner: base_owner.clone(),
                repo: base_repo.clone(),
                suggested_url: format!("https://{host}/{base_owner}/{base_repo}.git"),
            })?,
    };

    let branch = &info.source_branch;
    eprintln!(
        "{}",
        progress_message(cformat!("Fetching <bold>{branch}</> from {remote}..."))
    );
    // Explicit refspec creates/updates the remote-tracking ref even when it's outside
    // the configured fetch refspec (e.g., single-branch clones, bare repos).
    let refspec = format!("+refs/heads/{branch}:refs/remotes/{remote}/{branch}");
    // Use -- to prevent branch names starting with - from being interpreted as flags
    repo.run_command(&["fetch", "--", &remote, &refspec])
        .with_context(|| format!("Failed to fetch branch '{}' from {}", branch, remote))?;

    Ok(ResolvedTarget {
        branch: info.source_branch.clone(),
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
    if let Some(suffix) = branch.strip_prefix("pr:")
        && let Ok(number) = suffix.parse::<u32>()
    {
        return resolve_remote_ref(repo, &GitHubProvider, number, create, base);
    }

    // Handle mr:<number> syntax (GitLab MRs)
    if let Some(suffix) = branch.strip_prefix("mr:")
        && let Ok(number) = suffix.parse::<u32>()
    {
        return resolve_remote_ref(repo, &GitLabProvider, number, create, base);
    }

    // Regular branch switch
    let resolved_branch = repo
        .resolve_worktree_name(branch)
        .context("Failed to resolve branch name")?;

    // Resolve and validate base (only when --create is set)
    let resolved_base = if let Some(base_str) = base {
        if !create {
            eprintln!(
                "{}",
                warning_message("--base flag is only used with --create, ignoring")
            );
            None
        } else {
            let resolved = repo.resolve_worktree_name(base_str)?;
            if !repo.ref_exists(&resolved)? {
                return Err(GitError::ReferenceNotFound {
                    reference: resolved,
                }
                .into());
            }
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
    // Use -- to prevent branch names starting with - from being interpreted as flags
    let worktree_path_str = worktree_path.to_string_lossy();
    let git_args = ["worktree", "add", "--", worktree_path_str.as_ref(), branch];
    repo.run_command_delayed_stream(
        &git_args,
        Repository::SLOW_OPERATION_DELAY_MS,
        Some(
            progress_message(cformat!("Creating worktree for <bold>{}</>...", branch)).to_string(),
        ),
    )
    .map_err(|e| worktree_creation_error(&e, branch.to_string(), None))?;

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

    // Phase 2: Check if worktree already exists for this branch (fast path)
    // This avoids computing the worktree path template (~7 git commands) for existing switches.
    match repo.worktree_for_branch(&target.branch)? {
        Some(existing_path) if existing_path.exists() => {
            return Ok(SwitchPlan::Existing {
                path: canonicalize(&existing_path).unwrap_or(existing_path),
                branch: target.branch,
                new_previous,
            });
        }
        Some(_) => {
            return Err(GitError::WorktreeMissing {
                branch: target.branch,
            }
            .into());
        }
        None => {}
    }

    // Phase 3: Compute expected path (only needed for create)
    let expected_path = compute_worktree_path(repo, &target.branch, config)?;

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
/// For `SwitchPlan::Existing`, just records history. The returned
/// `SwitchBranchInfo` has `expected_path: None` — callers fill it in after
/// first output to avoid computing path mismatch on the hot path.
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
            new_previous,
        } => {
            let current_dir = std::env::current_dir()
                .ok()
                .and_then(|p| canonicalize(&p).ok());
            let already_at_worktree = current_dir
                .as_ref()
                .map(|cur| cur == &path)
                .unwrap_or(false);

            // Only update switch history when actually switching worktrees.
            // Updating on AlreadyAt would corrupt `wt switch -` by recording
            // the current branch as "previous" even though no switch occurred.
            if !already_at_worktree {
                let _ = repo.set_switch_previous(new_previous.as_deref());
            }

            let result = if already_at_worktree {
                SwitchResult::AlreadyAt(path)
            } else {
                SwitchResult::Existing { path }
            };

            // Path mismatch is computed lazily by callers after first output,
            // avoiding ~7 git commands on the hot path for existing switches.
            Ok((
                result,
                SwitchBranchInfo {
                    branch,
                    expected_path: None,
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

                    // For DWIM fallback: when the branch doesn't exist locally,
                    // git worktree add relies on DWIM to auto-create it from a
                    // remote tracking branch. DWIM fails in repos without configured
                    // fetch refspecs (bare repos, single-branch clones). Explicitly
                    // create from the tracking ref in that case.
                    let tracking_ref;

                    if *create_branch {
                        args.push("-b");
                        args.push(&branch);
                        if let Some(base) = base_branch {
                            args.push(base);
                        }
                    } else if !local_branch_existed {
                        // Explicit -b when there's exactly one remote tracking ref.
                        // Git's DWIM relies on the fetch refspec including this branch,
                        // which may not hold in single-branch clones or bare repos.
                        let remotes = branch_handle.remotes().unwrap_or_default();
                        if remotes.len() == 1 {
                            tracking_ref = format!("{}/{}", remotes[0], branch);
                            args.extend(["-b", &branch, tracking_ref.as_str()]);
                        } else {
                            // Multiple or zero remotes: let git's DWIM handle (or error)
                            args.push(&branch);
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
                        return Err(worktree_creation_error(
                            &e,
                            branch.clone(),
                            base_branch.clone(),
                        )
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

                    // Report tracking info when the branch was auto-created from a remote
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
                    ref_path,
                    fork_push_url,
                    ref_url: _,
                    remote,
                } => {
                    let label = ref_type.display(*number);

                    // Fetch the ref (remote was resolved during planning)
                    // Use -- to prevent refs starting with - from being interpreted as flags
                    repo.run_command(&["fetch", "--", remote, ref_path])
                        .with_context(|| format!("Failed to fetch {} from {}", label, remote))?;

                    // Execute branch creation and configuration with cleanup on failure.
                    let setup_result = setup_fork_branch(
                        repo,
                        &branch,
                        remote,
                        ref_path,
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

/// Resolve the deferred path mismatch for existing worktree switches.
///
fn worktree_creation_error(
    err: &anyhow::Error,
    branch: String,
    base_branch: Option<String>,
) -> GitError {
    let (output, command) = Repository::extract_failed_command(err);
    GitError::WorktreeCreationFailed {
        branch,
        base_branch,
        error: output,
        command,
    }
}
