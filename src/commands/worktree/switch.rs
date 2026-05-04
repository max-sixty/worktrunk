//! Worktree switch operations.
//!
//! Planning and executing worktree switches, plus the `wt switch` entry point
//! that wires hooks, approvals, output, and shell integration around them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::display::format_relative_time_short;
use anyhow::Context;
use color_print::cformat;
use dunce::canonicalize;
use serde::Serialize;
use worktrunk::HookType;
use worktrunk::config::{
    UserConfig, ValidationScope, expand_template, template_references_var, validate_template,
};
use worktrunk::git::remote_ref::{
    self, GitHubProvider, GitLabProvider, RemoteRefInfo, RemoteRefProvider,
};
use worktrunk::git::{
    GitError, RefContext, RefType, Repository, SwitchSuggestionCtx, current_or_recover,
};
use worktrunk::styling::{
    eprintln, format_with_gutter, hint_message, info_message, progress_message, suggest_command,
    warning_message,
};

use super::resolve::{
    compute_clobber_backup, compute_worktree_path, offer_bare_repo_worktree_path_fix, path_mismatch,
};
use super::types::{CreationMethod, SwitchBranchInfo, SwitchPlan, SwitchResult};
use crate::cli::SwitchFormat;
use crate::commands::command_approval::{approve_hooks, approve_or_skip};
use crate::commands::command_executor::FailureStrategy;
use crate::commands::command_executor::{CommandContext, build_hook_context};
use crate::commands::hooks::{HookAnnouncer, execute_hook};
use crate::commands::template_vars::TemplateVars;
use crate::output::{
    execute_user_command, handle_switch_output, is_shell_integration_active,
    prompt_shell_integration,
};

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

    let info = provider.fetch_info(number, repo)?;

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
    let repo_root = repo.repo_path()?;
    let local_branch = remote_ref::local_branch_name(info);
    let expected_remote = match remote_ref::find_remote(repo, info) {
        Ok(remote) => Some(remote),
        Err(e) => {
            log::debug!("Could not resolve remote for {}: {e:#}", ref_type.name());
            None
        }
    };

    // Check if branch already exists and is tracking this ref
    if let Some(tracks_this) = remote_ref::branch_tracks_ref(
        repo_root,
        &local_branch,
        provider,
        number,
        expected_remote.as_deref(),
    ) {
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
                    base_pr_upstream: None,
                },
            });
        }

        // Branch exists but doesn't track this ref - try prefixed name (GitHub only)
        if let Some(prefixed) = info.prefixed_local_branch_name() {
            if let Some(prefixed_tracks) = remote_ref::branch_tracks_ref(
                repo_root,
                &prefixed,
                provider,
                number,
                expected_remote.as_deref(),
            ) {
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
                            base_pr_upstream: None,
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
            let remote = remote_ref::find_remote(repo, info)?;
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
            // GitHub: URLs already in info, just find remote.
            let remote = remote_ref::find_remote(repo, info)?;
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
            // find_remote_by_url matches by (host, owner, repo); ssh vs https
            // doesn't matter (test_find_remote_by_url_cross_protocol).
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

/// Resolve a same-repo (non-fork) PR/MR.
fn resolve_same_repo_ref(
    repo: &Repository,
    info: &RemoteRefInfo,
) -> anyhow::Result<ResolvedTarget> {
    fetch_same_repo_branch(repo, info)?;

    Ok(ResolvedTarget {
        branch: info.source_branch.clone(),
        method: CreationMethod::Regular {
            create_branch: false,
            base_branch: None,
            base_pr_upstream: None,
        },
    })
}

/// Fetch a same-repo PR/MR's source branch with an explicit refspec so the
/// remote-tracking ref exists locally even in repos with limited fetch
/// refspecs (single-branch clones, bare repos).
fn fetch_same_repo_branch(repo: &Repository, info: &RemoteRefInfo) -> anyhow::Result<()> {
    let remote = remote_ref::find_remote(repo, info)?;
    let branch = &info.source_branch;
    eprintln!(
        "{}",
        progress_message(cformat!("Fetching <bold>{branch}</> from {remote}..."))
    );
    let refspec = format!("+refs/heads/{branch}:refs/remotes/{remote}/{branch}");
    // Use -- to prevent branch names starting with - from being interpreted as flags
    repo.run_command(&["fetch", "--", &remote, &refspec])
        .with_context(|| cformat!("Failed to fetch branch <bold>{}</> from {}", branch, remote))?;
    Ok(())
}

/// Resolve a `--base` value, expanding `pr:`/`mr:` shortcuts. Non-shortcut
/// inputs go through [`Repository::resolve_worktree_name`] (handles `@`/`-`/`^`).
///
/// Returns the resolved ref plus, when the user picked a `pr:`/`mr:` shortcut
/// against a same-repo PR/MR, the `(remote, branch)` pair the new branch
/// should be configured to track — see [`CreationMethod::Regular`].
///
/// When the bare name doesn't exist locally but a single remote has it,
/// returns the remote-qualified form so the validation in
/// [`resolve_switch_target`] doesn't reject `wt switch -c new --base
/// remote-only-branch`. Git's rev-parse doesn't auto-expand `foo` to
/// `refs/remotes/origin/foo`. The new branch's upstream is unset downstream
/// to keep `git push` from targeting the base.
fn resolve_base_ref(
    repo: &Repository,
    base: &str,
) -> anyhow::Result<(String, Option<(String, String)>)> {
    if let Some(suffix) = base.strip_prefix("pr:")
        && let Ok(number) = suffix.parse::<u32>()
    {
        return resolve_remote_ref_as_base(repo, &GitHubProvider, number);
    }

    if let Some(suffix) = base.strip_prefix("mr:")
        && let Ok(number) = suffix.parse::<u32>()
    {
        return resolve_remote_ref_as_base(repo, &GitLabProvider, number);
    }

    let resolved = repo.resolve_worktree_name(base)?;

    if !repo.ref_exists(&resolved)? {
        let remotes = repo.branch(&resolved).remotes()?;
        if remotes.len() == 1 {
            return Ok((format!("{}/{}", remotes[0], resolved), None));
        }
    }

    Ok((resolved, None))
}

/// Resolve `pr:{N}` / `mr:{N}` for `--base`. Same-repo returns the source
/// branch name plus the (remote, branch) the new branch should track; fork
/// returns the PR head SHA so we don't create a tracking branch for a ref
/// the user hasn't asked to check out.
fn resolve_remote_ref_as_base(
    repo: &Repository,
    provider: &dyn RemoteRefProvider,
    number: u32,
) -> anyhow::Result<(String, Option<(String, String)>)> {
    let ref_type = provider.ref_type();
    let symbol = ref_type.symbol();

    eprintln!(
        "{}",
        progress_message(cformat!(
            "Fetching base {} {symbol}{number}...",
            ref_type.name()
        ))
    );

    let info = provider.fetch_info(number, repo)?;
    eprintln!("{}", format_with_gutter(&format_ref_context(&info), None));

    if !info.is_cross_repo {
        fetch_same_repo_branch(repo, &info)?;
        let remote = remote_ref::find_remote(repo, &info)?;
        return Ok((
            info.source_branch.clone(),
            Some((remote, info.source_branch.clone())),
        ));
    }

    let remote = remote_ref::find_remote(repo, &info)?;
    let display = ref_type.display(number);
    repo.run_command(&["fetch", "--", &remote, &provider.tracking_ref(number)])
        .with_context(|| cformat!("Failed to fetch <bold>{display}</> from {remote}"))?;
    let sha = repo
        .run_command(&["rev-parse", "FETCH_HEAD"])
        .context("Failed to resolve FETCH_HEAD to a commit SHA")?
        .trim()
        .to_string();
    Ok((sha, None))
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
    let mut resolved_branch = repo
        .resolve_worktree_name(branch)
        .context("Failed to resolve branch name")?;

    // Handle remote-tracking ref names (e.g., "origin/username/feature-1" from the picker).
    // Strip the remote prefix so DWIM can create a local tracking branch.
    if !create && let Some(local_name) = repo.strip_remote_prefix(&resolved_branch) {
        resolved_branch = local_name;
    }

    // Resolve and validate base (only when --create is set)
    let (resolved_base, base_pr_upstream) = if let Some(base_str) = base {
        if !create {
            eprintln!(
                "{}",
                warning_message("--base flag is only used with --create, ignoring")
            );
            (None, None)
        } else {
            let (resolved, upstream) = resolve_base_ref(repo, base_str)?;
            if !repo.ref_exists(&resolved)? {
                return Err(GitError::ReferenceNotFound {
                    reference: resolved,
                }
                .into());
            }
            (Some(resolved), upstream)
        }
    } else {
        (None, None)
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
            // `--foreground` is required: background removal leaves a placeholder
            // directory at the original path (to keep shell PWD valid), which
            // would block the subsequent `wt switch` with "Directory already exists".
            let remove_cmd = suggest_command("remove", &[&resolved_branch], &["--foreground"]);
            let switch_cmd = suggest_command("switch", &[&resolved_branch], &[]);
            eprintln!(
                "{}",
                hint_message(cformat!(
                    "To switch to the remote branch, delete this branch and run without <underline>--create</>: <underline>{remove_cmd} && {switch_cmd}</>"
                ))
            );
        }
    }

    // Compute base branch for creation. When the cached default branch
    // no longer resolves locally, return None and let the downstream
    // StaleDefaultBranch error emerge at the actual use site.
    let base_branch = if create {
        resolved_base.or_else(|| {
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
            base_pr_upstream,
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
            last_fetch_ago: format_last_fetch_ago(repo),
            pr_mr_platform: repo.detect_ref_type(),
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
        .with_context(|| {
            cformat!(
                "Failed to create local branch <bold>{}</> from {}",
                branch,
                label
            )
        })?;

    // Configure branch tracking for pull and push
    let branch_remote_key = format!("branch.{}.remote", branch);
    let branch_merge_key = format!("branch.{}.merge", branch);
    let merge_ref = format!("refs/{}", remote_ref);

    repo.set_config(&branch_remote_key, remote)
        .with_context(|| format!("Failed to configure branch.{}.remote", branch))?;
    repo.set_config(&branch_merge_key, &merge_ref)
        .with_context(|| format!("Failed to configure branch.{}.merge", branch))?;

    // Only configure pushRemote if we have a fork URL (not using prefixed branch)
    if let Some(url) = fork_push_url {
        let branch_push_remote_key = format!("branch.{}.pushRemote", branch);
        repo.set_config(&branch_push_remote_key, url)
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
                branch: Some(target.branch),
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

    // Phase 2b: Path-based fallback for detached worktrees.
    // If the argument looks like a path (not a branch name), try to find a worktree there.
    if !create {
        let candidate = Path::new(branch);
        let abs_path = if candidate.is_absolute() {
            Some(candidate.to_path_buf())
        } else if candidate.components().count() > 1 {
            // Relative path with directory separators (e.g., "../repo.feature").
            // Single-component names are ambiguous with branch names (already tried in Phase 2).
            std::env::current_dir().ok().map(|cwd| cwd.join(candidate))
        } else {
            None
        };
        if let Some(abs_path) = abs_path
            && let Some((path, wt_branch)) = repo.worktree_at_path(&abs_path)?
        {
            let canonical = canonicalize(&path).unwrap_or_else(|_| path.clone());
            return Ok(SwitchPlan::Existing {
                path: canonical,
                branch: wt_branch,
                new_previous,
            });
        }
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
    run_hooks: bool,
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
                    base_pr_upstream,
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

                    // `--base pr:N` / `--base mr:N` against a same-repo PR/MR: the
                    // user asked for a custom local name pointing at an existing
                    // remote branch — wire up tracking so `git push` from the new
                    // worktree pushes back to the PR/MR's source branch instead
                    // of failing with "no upstream branch". See issue #2497.
                    if *create_branch
                        && let Some((upstream_remote, upstream_branch)) = base_pr_upstream
                    {
                        repo.set_config(&format!("branch.{branch}.remote"), upstream_remote)?;
                        repo.set_config(
                            &format!("branch.{branch}.merge"),
                            &format!("refs/heads/{upstream_branch}"),
                        )?;
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
                            info_message(cformat!("Push configured to fork: <underline>{url}</>"))
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

            // PR/MR identity travels into both the pre-start hook below and the
            // SwitchResult — TemplateVars::for_post_switch then forwards it to
            // background post-switch / post-start hooks.
            let (pr_number, pr_url) = match &method {
                CreationMethod::ForkRef {
                    number, ref_url, ..
                } => (Some(*number), Some(ref_url.clone())),
                CreationMethod::Regular { .. } => (None, None),
            };

            // Execute post-create commands
            if run_hooks {
                let ctx = CommandContext::new(repo, config, Some(&branch), &worktree_path, force);
                let mut vars = TemplateVars::new()
                    .with_target(&branch)
                    .with_target_worktree_path(&worktree_path);
                match &method {
                    CreationMethod::Regular { base_branch, .. } => {
                        vars = vars
                            .with_base_strs(base_branch.as_deref(), base_worktree_path.as_deref());
                    }
                    CreationMethod::ForkRef {
                        number, ref_url, ..
                    } => {
                        vars = vars.with_pr(Some(*number), Some(ref_url));
                    }
                }
                ctx.execute_pre_start_commands(&vars.as_extra_vars())?;
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
                    pr_number,
                    pr_url,
                },
                SwitchBranchInfo {
                    branch: Some(branch),
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

/// Format the last fetch time as a self-contained phrase for error hint parentheticals.
///
/// Returns e.g. "last fetched 3h ago" or "last fetched just now".
/// Returns `None` if FETCH_HEAD doesn't exist (never fetched).
fn format_last_fetch_ago(repo: &Repository) -> Option<String> {
    let epoch = repo.last_fetch_epoch()?;
    let relative = format_relative_time_short(epoch as i64);
    if relative == "now" || relative == "future" {
        Some("last fetched just now".to_string())
    } else {
        Some(format!("last fetched {relative} ago"))
    }
}

/// Structured output for `wt switch --format=json`.
#[derive(Serialize)]
struct SwitchJsonOutput {
    action: &'static str,
    /// Branch name
    #[serde(skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    /// Absolute worktree path
    path: PathBuf,
    /// True if branch was created (--create flag)
    #[serde(skip_serializing_if = "Option::is_none")]
    created_branch: Option<bool>,
    /// Base branch when creating (e.g., "main")
    #[serde(skip_serializing_if = "Option::is_none")]
    base_branch: Option<String>,
    /// Remote tracking branch if auto-created
    #[serde(skip_serializing_if = "Option::is_none")]
    from_remote: Option<String>,
}

impl SwitchJsonOutput {
    fn from_result(result: &SwitchResult, branch_info: &SwitchBranchInfo) -> Self {
        let (action, path, created_branch, base_branch, from_remote) = match result {
            SwitchResult::AlreadyAt(path) => ("already_at", path, None, None, None),
            SwitchResult::Existing { path } => ("existing", path, None, None, None),
            SwitchResult::Created {
                path,
                created_branch,
                base_branch,
                from_remote,
                ..
            } => (
                "created",
                path,
                Some(*created_branch),
                base_branch.clone(),
                from_remote.clone(),
            ),
        };
        Self {
            action,
            branch: branch_info.branch.clone(),
            path: path.clone(),
            created_branch,
            base_branch,
            from_remote,
        }
    }
}

/// Options for the switch command
pub struct SwitchOptions<'a> {
    pub branch: &'a str,
    pub create: bool,
    pub base: Option<&'a str>,
    pub execute: Option<&'a str>,
    pub execute_args: &'a [String],
    pub yes: bool,
    pub clobber: bool,
    /// Resolved from --cd/--no-cd flags: Some(true) = cd, Some(false) = no cd, None = use config
    pub change_dir: Option<bool>,
    pub verify: bool,
    pub format: crate::cli::SwitchFormat,
}

/// Run pre-switch hooks before branch resolution or worktree creation.
///
/// Symbolic arguments (`-`, `@`, `^`) are resolved to concrete branch names
/// before building the hook context so `{{ target }}`, `{{ target_worktree_path }}`,
/// and the Active overrides point at the real destination. When resolution
/// fails (e.g., no previous branch for `-`), the raw argument is used — the
/// same error surfaces later from `plan_switch` with the canonical message.
///
/// Directional vars:
/// - `base` / `base_worktree_path`: current (source) branch and worktree
/// - `target` / `target_worktree_path`: destination branch and worktree (if it exists)
pub(crate) fn run_pre_switch_hooks(
    repo: &Repository,
    config: &UserConfig,
    target_branch: &str,
    yes: bool,
) -> anyhow::Result<()> {
    let current_wt = repo.current_worktree();
    let current_path = current_wt.path().to_path_buf();
    let resolved_target = repo
        .resolve_worktree_name(target_branch)
        .unwrap_or_else(|_| target_branch.to_string());
    let pre_ctx = CommandContext::new(repo, config, Some(&resolved_target), &current_path, yes);

    let pre_switch_approved = approve_hooks(&pre_ctx, &[HookType::PreSwitch])?;
    if pre_switch_approved {
        // Base vars: source (where the user currently is). Target vars and
        // Active overrides come from the destination worktree if it exists —
        // for creates the planned path is computed later during plan_switch,
        // so worktree_path stays at its default (the source = cwd).
        let base_branch = current_wt.branch().ok().flatten().unwrap_or_default();
        let dest_path = repo.worktree_for_branch(&resolved_target).ok().flatten();

        let mut vars = TemplateVars::new()
            .with_base(&base_branch, &current_path)
            .with_target(&resolved_target);
        if let Some(p) = dest_path.as_deref() {
            vars = vars.with_target_worktree_path(p).with_active_worktree(p);
        }
        let extra_vars = vars.as_extra_vars();

        execute_hook(
            &pre_ctx,
            HookType::PreSwitch,
            &extra_vars,
            FailureStrategy::FailFast,
            crate::output::pre_hook_display_path(pre_ctx.worktree_path),
        )?;
    }
    Ok(())
}

/// Hook types that apply after a switch operation.
///
/// Creates trigger pre-start + post-start + post-switch hooks;
/// existing worktrees trigger only post-switch.
fn switch_post_hook_types(is_create: bool) -> &'static [HookType] {
    if is_create {
        &[
            HookType::PreStart,
            HookType::PostStart,
            HookType::PostSwitch,
        ]
    } else {
        &[HookType::PostSwitch]
    }
}

/// Approve switch hooks upfront and show "Commands declined" if needed.
///
/// Returns `true` if hooks are approved to run.
/// Returns `false` if hooks should be skipped (`!verify` or user declined).
pub(crate) fn approve_switch_hooks(
    repo: &Repository,
    config: &UserConfig,
    plan: &SwitchPlan,
    yes: bool,
    verify: bool,
) -> anyhow::Result<bool> {
    if !verify {
        return Ok(false);
    }

    let ctx = CommandContext::new(repo, config, plan.branch(), plan.worktree_path(), yes);
    let on_decline = if plan.is_create() {
        "Commands declined, continuing worktree creation"
    } else {
        "Commands declined"
    };
    approve_or_skip(&ctx, switch_post_hook_types(plan.is_create()), on_decline)
}

/// Spawn post-switch (and post-start for creates) background hooks.
pub(crate) fn spawn_switch_background_hooks(
    repo: &Repository,
    config: &UserConfig,
    result: &SwitchResult,
    branch: Option<&str>,
    yes: bool,
    extra_vars: &[(&str, &str)],
    hooks_display_path: Option<&Path>,
) -> anyhow::Result<()> {
    let ctx = CommandContext::new(repo, config, branch, result.path(), yes);

    let mut announcer = HookAnnouncer::new(repo, config, false);
    announcer.register(&ctx, HookType::PostSwitch, extra_vars, hooks_display_path)?;
    if matches!(result, SwitchResult::Created { .. }) {
        announcer.register(&ctx, HookType::PostStart, extra_vars, hooks_display_path)?;
    }
    announcer.flush()
}

/// Handle the switch command.
pub fn run_switch(
    opts: SwitchOptions<'_>,
    config: &mut UserConfig,
    binary_name: &str,
) -> anyhow::Result<()> {
    let SwitchOptions {
        branch,
        create,
        base,
        execute,
        execute_args,
        yes,
        clobber,
        change_dir: change_dir_flag,
        verify,
        format,
    } = opts;

    let (repo, is_recovered) = current_or_recover().context("Failed to switch worktree")?;

    // Resolve change_dir: explicit CLI flags > project config > global config > default (true)
    // Now that we have the repo, we can resolve project-specific config.
    let change_dir = change_dir_flag.unwrap_or_else(|| {
        let project_id = repo.project_identifier().ok();
        config.resolved(project_id.as_deref()).switch.cd()
    });

    // Build switch suggestion context for enriching error hints with --execute/trailing args.
    // Without this, errors like "branch already exists" would suggest `wt switch <branch>`
    // instead of the full `wt switch <branch> --execute=<cmd> -- <args>`.
    let suggestion_ctx = execute.map(|exec| {
        let escaped = shell_escape::escape(exec.into());
        SwitchSuggestionCtx {
            extra_flags: vec![format!("--execute={escaped}")],
            trailing_args: execute_args.to_vec(),
        }
    });

    // Run pre-switch hooks before branch resolution or worktree creation.
    // {{ branch }} receives the raw user input (before resolution).
    // Skip when recovered — the source worktree is gone, nothing to run hooks against.
    if verify && !is_recovered {
        run_pre_switch_hooks(&repo, config, branch, yes)?;
    }

    // Offer to fix worktree-path for bare repos with hidden directory names (.git, .bare).
    offer_bare_repo_worktree_path_fix(&repo, config)?;

    // Validate and resolve the target branch.
    let plan = plan_switch(&repo, branch, create, base, clobber, config).map_err(|err| {
        match suggestion_ctx {
            Some(ref ctx) => match err.downcast::<GitError>() {
                Ok(git_err) => GitError::WithSwitchSuggestion {
                    source: Box::new(git_err),
                    ctx: ctx.clone(),
                }
                .into(),
                Err(err) => err,
            },
            None => err,
        }
    })?;

    // "Approve at the Gate": collect and approve hooks upfront
    // This ensures approval happens once at the command entry point
    // If user declines, skip hooks but continue with worktree operation
    let hooks_approved = approve_switch_hooks(&repo, config, &plan, yes, verify)?;

    // Pre-flight: validate all templates before mutation (worktree creation).
    // Catches syntax errors and undefined variables early so a broken template
    // doesn't leave behind a half-created worktree that blocks re-running.
    validate_switch_templates(&repo, config, &plan, execute, execute_args, hooks_approved)?;

    // Capture source (base) worktree identity BEFORE the switch, so post-switch
    // hooks can reference where the user came from via {{ base }} / {{ base_worktree_path }}.
    let source_branch = repo
        .current_worktree()
        .branch()
        .ok()
        .flatten()
        .unwrap_or_default();
    let source_path = repo
        .current_worktree()
        .root()
        .ok()
        .map(|p| worktrunk::path::to_posix_path(&p.to_string_lossy()))
        .unwrap_or_default();

    // Execute the validated plan
    let (result, branch_info) = execute_switch(&repo, plan, config, yes, hooks_approved)?;

    // --format=json: write structured result to stdout. All behavior (hooks,
    // --execute, shell integration) proceeds normally — format only affects output.
    if format == SwitchFormat::Json {
        let json = SwitchJsonOutput::from_result(&result, &branch_info);
        let json = serde_json::to_string(&json).context("Failed to serialize to JSON")?;
        println!("{json}");
    }

    // Early exit for benchmarking time-to-first-output
    if std::env::var_os("WORKTRUNK_FIRST_OUTPUT").is_some() {
        return Ok(());
    }

    // Compute path mismatch lazily (deferred from plan_switch for existing worktrees).
    // Skip for detached HEAD worktrees (branch is None) — no branch to compute expected path from.
    let branch_info = match &result {
        SwitchResult::Existing { path } | SwitchResult::AlreadyAt(path) => {
            let expected_path = branch_info
                .branch
                .as_deref()
                .and_then(|b| path_mismatch(&repo, b, path, config));
            SwitchBranchInfo {
                expected_path,
                ..branch_info
            }
        }
        _ => branch_info,
    };

    // Show success message (temporal locality: immediately after worktree operation)
    // Returns path to display in hooks when user's shell won't be in the worktree
    // Also shows worktree-path hint on first --create (before shell integration warning)
    //
    // When recovered from a deleted worktree, current_dir() and current_worktree().root()
    // both fail — fall back to repo_path() (the main worktree root).
    let fallback_path = repo.repo_path()?.to_path_buf();
    let cwd = std::env::current_dir().unwrap_or(fallback_path.clone());
    let source_root = repo.current_worktree().root().unwrap_or(fallback_path);
    let hooks_display_path =
        handle_switch_output(&result, &branch_info, change_dir, Some(&source_root), &cwd)?;

    // Offer shell integration if not already installed/active
    // (only shows prompt/hint when shell integration isn't working)
    // With --execute: show hints only (don't interrupt with prompt)
    // Skip when change_dir is false — user opted out of cd, so shell integration is irrelevant
    // Best-effort: don't fail switch if offer fails
    if change_dir && !is_shell_integration_active() {
        let skip_prompt = execute.is_some();
        let _ = prompt_shell_integration(&repo, config, binary_name, skip_prompt);
    }

    // Build template vars for base/target context (used by both hooks and
    // --execute). "base" is the source worktree the user switched from (all
    // switches), or the branch they branched from (creates). "target" matches
    // the bare vars (the destination) — kept symmetric with pre-switch.
    let template_vars =
        TemplateVars::for_post_switch(&result, &branch_info, &source_branch, &source_path);
    let extra_vars = template_vars.as_extra_vars();

    // Spawn background hooks after success message
    // - post-switch: runs on ALL switches (shows "@ path" when shell won't be there)
    // - post-start: runs only when creating a NEW worktree
    // Batch hooks into a single message when both types are present
    if hooks_approved {
        spawn_switch_background_hooks(
            &repo,
            config,
            &result,
            branch_info.branch.as_deref(),
            yes,
            &extra_vars,
            hooks_display_path.as_deref(),
        )?;
    }

    // Execute user command after post-start hooks have been spawned
    // Note: execute_args requires execute via clap's `requires` attribute
    if let Some(cmd) = execute {
        // Build template context for expansion (includes base vars when creating)
        let ctx = CommandContext::new(
            &repo,
            config,
            branch_info.branch.as_deref(),
            result.path(),
            yes,
        );
        let template_vars = build_hook_context(&ctx, &extra_vars, None)?;
        let vars: HashMap<&str, &str> = template_vars
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        // Expand template variables in command (shell_escape: true for safety)
        let expanded_cmd = expand_template(cmd, &vars, true, &repo, "--execute command")?;

        // Append any trailing args (after --) to the execute command
        // Each arg is also expanded, then shell-escaped
        let full_cmd = if execute_args.is_empty() {
            expanded_cmd
        } else {
            let expanded_args: Result<Vec<_>, _> = execute_args
                .iter()
                .map(|arg| expand_template(arg, &vars, false, &repo, "--execute argument"))
                .collect();
            let escaped_args: Vec<_> = expanded_args?
                .iter()
                .map(|arg| shell_escape::escape(arg.into()).into_owned())
                .collect();
            format!("{} {}", expanded_cmd, escaped_args.join(" "))
        };
        execute_user_command(&full_cmd, hooks_display_path.as_deref())?;
    }

    Ok(())
}

/// Validate all templates that will be expanded after worktree creation.
///
/// Catches syntax errors and undefined variable references *before* the
/// irreversible worktree creation, so a broken template doesn't leave behind
/// a worktree that blocks re-running the command.
///
/// This is a best-effort pre-flight check: it catches definite errors (syntax,
/// unknown variables) but cannot catch failures from conditional variables that
/// are absent at expansion time (e.g., `upstream` when no tracking is configured).
/// Such late failures propagate as normal errors — no panics.
///
/// ## Why only switch needs pre-flight validation
///
/// Switch is the only command where template failure after mutation creates a
/// **blocking half-state**: `wt switch -c <branch>` creates a worktree, then if
/// hook/--execute expansion fails, the worktree exists and the same command
/// can't be re-run (branch already exists). Other commands don't have this
/// problem:
///
/// - **Pre-operation hooks** (pre-merge, pre-remove, pre-commit) run before the
///   irreversible operation, so template errors abort cleanly.
/// - **Post-operation hooks** (post-merge, post-remove) run after the operation
///   completed successfully — template failure is a missed notification, not a
///   blocking state. The user can fix the template and run `wt hook` manually.
///
/// Validates:
/// - `--execute` command template (if present)
/// - `--execute` trailing arg templates (if present)
/// - Hook templates (post-create, post-start, post-switch) from user and project config
fn validate_switch_templates(
    repo: &Repository,
    config: &UserConfig,
    plan: &SwitchPlan,
    execute: Option<&str>,
    execute_args: &[String],
    hooks_approved: bool,
) -> anyhow::Result<()> {
    // Validate --execute template and trailing args
    if let Some(cmd) = execute {
        validate_template(
            cmd,
            ValidationScope::SwitchExecute,
            repo,
            "--execute command",
        )?;
        for arg in execute_args {
            validate_template(
                arg,
                ValidationScope::SwitchExecute,
                repo,
                "--execute argument",
            )?;
        }
    }

    // Validate hook templates only when hooks will actually run
    if !hooks_approved {
        return Ok(());
    }

    let project_config = repo.load_project_config()?;
    let user_hooks = config.hooks(repo.project_identifier().ok().as_deref());

    for &hook_type in switch_post_hook_types(plan.is_create()) {
        let (user_cfg, proj_cfg) = crate::commands::hooks::lookup_hook_configs(
            &user_hooks,
            project_config.as_ref(),
            hook_type,
        );
        for (source, cfg) in [("user", user_cfg), ("project", proj_cfg)] {
            if let Some(cfg) = cfg {
                for cmd in cfg.commands() {
                    // Skip full validation for lazy templates ({{ vars.X }}) —
                    // they're expanded at runtime after prior pipeline steps set
                    // the vars. Syntax is still checked by expand_commands.
                    if template_references_var(&cmd.template, "vars") {
                        continue;
                    }
                    let name = match &cmd.name {
                        Some(n) => format!("{source} {hook_type}:{n}"),
                        None => format!("{source} {hook_type} hook"),
                    };
                    validate_template(
                        &cmd.template,
                        ValidationScope::Hook(hook_type),
                        repo,
                        &name,
                    )?;
                }
            }
        }
    }

    Ok(())
}
