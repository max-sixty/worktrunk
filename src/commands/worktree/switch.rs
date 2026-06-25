//! Worktree switch operations.
//!
//! Planning and executing worktree switches, plus [`SwitchPipeline`] — the
//! full switch sequence (bare-repo fix-up, hooks, approval, execution, output)
//! shared by the `wt switch` argument path and the interactive picker.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::display::format_relative_time_short;
use anyhow::{Context, bail};
use color_print::cformat;
use dunce::canonicalize;
use serde::Serialize;
use worktrunk::HookType;
use worktrunk::config::{
    UserConfig, ValidationScope, expand_template, template_references_var, validate_template,
};
use worktrunk::git::remote_ref::{
    self, AzureDevOpsProvider, GitHubProvider, GitLabProvider, GiteaProvider, RemoteRefInfo,
    RemoteRefProvider, parse_ref_url,
};
use worktrunk::git::{
    GitError, GitRemoteUrl, RefContext, RefType, Repository, SwitchSuggestionCtx,
    current_or_recover,
};
use worktrunk::shell_exec::{ShellEscapeMode, directive_shell_escape_mode, shell_escape_for};
use worktrunk::styling::{
    eprintln, format_with_gutter, hint_message, info_message, progress_message, suggest_command,
    warning_message,
};

use super::resolve::{compute_worktree_path, offer_bare_repo_worktree_path_fix, path_mismatch};
use super::types::{CreationMethod, SwitchBranchInfo, SwitchPlan, SwitchResult};
use crate::cli::{SwitchArgs, SwitchFormat};
use crate::commands::backup::back_up_clobbered_path_now;
use crate::commands::command_approval::approve_hooks;
use crate::commands::command_executor::FailureStrategy;
use crate::commands::command_executor::{CommandContext, build_hook_context};
use crate::commands::flag_pair;
use crate::commands::hook_plan::{ApprovedHookPlan, HookPlanBuilder, register_planned};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrProviderChoice {
    GitHub,
    Gitea,
    AzureDevOps,
}

/// Choose which provider should handle `pr:<number>` resolution.
///
/// Priority:
/// 1. `forge.platform` config (`github` / `gitea` / `azure-devops`)
/// 2. Primary remote URL detection (host contains `github`/`gitea`/`dev.azure.com`)
/// 3. CLI auth lookup — if `tea` has a login for this host but `gh` does
///    not, pick Gitea; otherwise default to GitHub
///
/// The default-to-GitHub fall-through means a self-hosted Gitea on a branded
/// host (e.g. `git.example.com`) without `tea login add` will see a single
/// GitHub error (with hint to set `forge.platform = "gitea"`), instead of a
/// wrapped two-provider error.
fn choose_pr_provider(repo: &Repository) -> anyhow::Result<PrProviderChoice> {
    if let Some(platform_raw) = repo
        .load_project_config()?
        .and_then(|c| c.forge_platform().map(str::to_string))
    {
        let platform = platform_raw.to_ascii_lowercase();
        match platform.as_str() {
            "github" => return Ok(PrProviderChoice::GitHub),
            "gitea" => return Ok(PrProviderChoice::Gitea),
            "azure-devops" | "azuredevops" => return Ok(PrProviderChoice::AzureDevOps),
            "gitlab" => {
                bail!("forge.platform is set to gitlab; use mr:<number> instead of pr:<number>")
            }
            _ => bail!(
                "Invalid forge.platform value `{platform_raw}` in .config/wt.toml; \
                 expected one of: github, gitlab, gitea, azure-devops"
            ),
        }
    }

    // GitHub still wins in mixed-remote setups (preserves pre-Gitea/Azure
    // behaviour for repos that grew a mirror later). Scan every remote so a
    // non-primary `origin` doesn't hide a GitHub mirror.
    let all_parsed: Vec<_> = repo
        .all_remote_urls()
        .into_iter()
        .filter_map(|(_, url)| GitRemoteUrl::parse(&url))
        .collect();

    if all_parsed.iter().any(|u| u.is_github()) {
        return Ok(PrProviderChoice::GitHub);
    }
    if all_parsed.iter().any(|u| u.is_gitea()) {
        return Ok(PrProviderChoice::Gitea);
    }
    if all_parsed.iter().any(|u| u.is_azure_devops()) {
        return Ok(PrProviderChoice::AzureDevOps);
    }
    if all_parsed.iter().any(|u| u.is_gitlab()) {
        bail!("Detected GitLab remote; use mr:<number> instead of pr:<number>")
    }

    // No recognisable forge remote. Use the primary remote (raw URL — `insteadOf`
    // rewrites are for git transport and may not reflect the real forge host)
    // to ask the CLIs which one is configured for this host. If only `tea` has a
    // login, pick Gitea; otherwise default to GitHub (the common case, and the
    // one users get useful errors from when nothing is set up).
    let Some(host) = repo
        .primary_remote()
        .ok()
        .and_then(|remote| repo.remote_url(&remote))
        .and_then(|url| GitRemoteUrl::parse(&url))
        .map(|u| u.host().to_string())
    else {
        return Ok(PrProviderChoice::GitHub);
    };

    if remote_ref::gitea::is_authed_for(&host) && !remote_ref::github::is_authed_for(&host) {
        Ok(PrProviderChoice::Gitea)
    } else {
        Ok(PrProviderChoice::GitHub)
    }
}

fn resolve_pr_target(
    repo: &Repository,
    number: u32,
    create: bool,
    base: Option<&str>,
) -> anyhow::Result<ResolvedTarget> {
    if base.is_some() {
        return Err(GitError::RefBaseConflict {
            ref_type: RefType::Pr,
            number,
        }
        .into());
    }

    match choose_pr_provider(repo)? {
        PrProviderChoice::GitHub => resolve_remote_ref(repo, &GitHubProvider, number, create, base),
        PrProviderChoice::Gitea => resolve_remote_ref(repo, &GiteaProvider, number, create, base),
        PrProviderChoice::AzureDevOps => {
            resolve_remote_ref(repo, &AzureDevOpsProvider, number, create, base)
        }
    }
}

fn resolve_pr_base(
    repo: &Repository,
    number: u32,
) -> anyhow::Result<(String, Option<(String, String)>)> {
    match choose_pr_provider(repo)? {
        PrProviderChoice::GitHub => resolve_remote_ref_as_base(repo, &GitHubProvider, number),
        PrProviderChoice::Gitea => resolve_remote_ref_as_base(repo, &GiteaProvider, number),
        PrProviderChoice::AzureDevOps => {
            resolve_remote_ref_as_base(repo, &AzureDevOpsProvider, number)
        }
    }
}

/// Fetch PR/MR info while showing a "still waiting" status.
///
/// The host lookup (`gh`/`glab` API) captures its output and can stall on a slow
/// network, so without feedback the command looks frozen. The watchdog clears
/// before the caller prints the resolved ref context. No command gutter — the
/// host CLI invocation isn't readily available here, and the status line alone
/// is the signal.
fn fetch_ref_info(
    provider: &dyn RemoteRefProvider,
    number: u32,
    repo: &Repository,
) -> anyhow::Result<RemoteRefInfo> {
    let _watchdog = worktrunk::progress::Watchdog::start(
        &format!("the {} info", provider.ref_type().name()),
        None,
    );
    provider.fetch_info(number, repo)
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

    let info = fetch_ref_info(provider, number, repo)?;

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

        // Branch exists but doesn't track this ref - try prefixed name (GitHub/Gitea)
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

/// Parse a `pr:N` / `mr:N` shortcut into its ref type and number, first
/// normalising a forge PR/MR web URL (e.g.
/// `https://github.com/owner/repo/pull/123`) into the same literal shortcut so
/// both forms flow through one dispatch. Returns `None` for a regular branch
/// name, which callers resolve as an ordinary ref.
fn parse_ref_shortcut(input: &str) -> Option<(RefType, u32)> {
    let normalised = parse_ref_url(input);
    let input = normalised.as_deref().unwrap_or(input);
    if let Some(number) = input
        .strip_prefix("pr:")
        .and_then(|s| s.parse::<u32>().ok())
    {
        return Some((RefType::Pr, number));
    }
    if let Some(number) = input
        .strip_prefix("mr:")
        .and_then(|s| s.parse::<u32>().ok())
    {
        return Some((RefType::Mr, number));
    }
    None
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
    match parse_ref_shortcut(base) {
        Some((RefType::Pr, number)) => return resolve_pr_base(repo, number),
        Some((RefType::Mr, number)) => {
            return resolve_remote_ref_as_base(repo, &GitLabProvider, number);
        }
        None => {}
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

    let info = fetch_ref_info(provider, number, repo)?;
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
    // `pr:N` dispatches to GitHub, Gitea, or Azure DevOps based on remotes;
    // `mr:N` to GitLab. Forge PR/MR web URLs normalise to the same shortcuts.
    match parse_ref_shortcut(branch) {
        Some((RefType::Pr, number)) => return resolve_pr_target(repo, number, create, base),
        Some((RefType::Mr, number)) => {
            return resolve_remote_ref(repo, &GitLabProvider, number, create, base);
        }
        None => {}
    }

    // Regular branch switch
    let mut resolved_branch = repo
        .resolve_worktree_name(branch)
        .context("Failed to resolve branch name")?;

    // Handle remote-tracking ref names (e.g., "origin/username/feature-1" from the picker).
    // Strip the remote prefix only when there is no exact local branch/worktree,
    // so a local branch literally named `origin/foo` is not retargeted to `foo`.
    if !create
        && repo.worktree_for_branch(&resolved_branch)?.is_none()
        && !repo.branch(&resolved_branch).exists_locally()?
        && let Some(local_name) = repo.strip_remote_prefix(&resolved_branch)
    {
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
) -> anyhow::Result<bool> {
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

    // Handle clobber for stale directories. Returns whether `execute_switch`
    // must back up a path occupying `worktree_path` before creating the
    // worktree; the backup itself happens at execution time so a path that
    // races in after planning is still moved atomically (see
    // `back_up_clobbered_path`).
    if !path.exists() {
        return Ok(false);
    }
    if clobber {
        return Ok(true);
    }
    let is_create = matches!(
        method,
        CreationMethod::Regular {
            create_branch: true,
            ..
        }
    );
    Err(GitError::WorktreePathExists {
        branch: branch.to_string(),
        path: path.to_path_buf(),
        create: is_create,
    }
    .into())
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
fn plan_switch(
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
    let needs_clobber_backup = validate_worktree_creation(
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
        needs_clobber_backup,
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
fn execute_switch(
    repo: &Repository,
    plan: SwitchPlan,
    config: &UserConfig,
    force: bool,
    run_hooks: bool,
    hook_plan: &ApprovedHookPlan,
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
            needs_clobber_backup,
            new_previous,
        } => {
            // Handle --clobber backup if needed (shared for all creation methods)
            if needs_clobber_backup {
                // Atomically move the stale path aside, to a timestamped backup
                // name. A name already taken (a same-second clobber, or one
                // that raced in after planning) is never overwritten — the move
                // falls back to the next free `-N` name.
                let backup_path = back_up_clobbered_path_now(&worktree_path)?;

                let path_display = worktrunk::path::format_path_for_display(&worktree_path);
                let backup_display = worktrunk::path::format_path_for_display(&backup_path);
                eprintln!(
                    "{}",
                    warning_message(cformat!(
                        "Moved <bold>{path_display}</> to <bold>{backup_display}</> (--clobber)"
                    ))
                );
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

                    // Build git worktree add command. Options come first, then
                    // `--` separates them from the path and any positional ref,
                    // so branch/base names that begin with `-` cannot be
                    // misinterpreted by git as flags. `-b <branch>` keeps the
                    // branch as the *value* of `-b`, which is safe even when
                    // the branch name starts with `-`.
                    let worktree_path_str = worktree_path.to_string_lossy();
                    let mut args: Vec<&str> = vec!["worktree", "add"];

                    // For DWIM fallback: when the branch doesn't exist locally,
                    // git worktree add relies on DWIM to auto-create it from a
                    // remote tracking branch. DWIM fails in repos without configured
                    // fetch refspecs (bare repos, single-branch clones). Explicitly
                    // create from the tracking ref in that case.
                    let tracking_ref;

                    let trailing_ref: Option<&str> = if *create_branch {
                        args.push("-b");
                        args.push(&branch);
                        base_branch.as_deref()
                    } else if !local_branch_existed {
                        // Explicit -b when there's exactly one remote tracking ref.
                        // Git's DWIM relies on the fetch refspec including this branch,
                        // which may not hold in single-branch clones or bare repos.
                        let remotes = branch_handle.remotes().unwrap_or_default();
                        if remotes.len() == 1 {
                            tracking_ref = format!("{}/{}", remotes[0], branch);
                            args.extend(["-b", &branch]);
                            Some(tracking_ref.as_str())
                        } else {
                            // Multiple or zero remotes: let git's DWIM handle (or error)
                            Some(branch.as_str())
                        }
                    } else {
                        Some(branch.as_str())
                    };

                    args.push("--");
                    args.push(worktree_path_str.as_ref());
                    if let Some(r) = trailing_ref {
                        args.push(r);
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

            // Compute base worktree path for hooks and result.
            //
            // `git worktree add` already mutated the worktree list, but `repo`
            // cached it pre-start (populated by `plan_switch`). Reading
            // `worktree_for_branch` through `repo` here would observe the stale
            // pre-start inventory — see the caching contract in
            // `git/repository/mod.rs`. Probe through a fresh `Repository::at`
            // so the lookup reflects the post-start state.
            let base_worktree_path = base_branch
                .as_ref()
                .and_then(|b| {
                    Repository::at(repo.discovery_path())
                        .and_then(|fresh| fresh.worktree_for_branch(b))
                        .ok()
                        .flatten()
                })
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

            // Execute pre-start commands. `hook_repo` roots the render context
            // in the new worktree (created just above); the commands come from
            // the frozen `hook_plan`, selected at the gate from the invoking
            // worktree's config.
            if run_hooks {
                let hook_repo = Repository::at(&worktree_path)?;
                let ctx =
                    CommandContext::new(&hook_repo, config, Some(&branch), &worktree_path, force);
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
                ctx.execute_pre_create_commands(&vars.as_extra_vars(), hook_plan, &worktree_path)?;
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

/// Emit the structured `--format=json` result to stdout when requested.
///
/// A no-op for `SwitchFormat::Text`.
fn emit_switch_json(
    format: SwitchFormat,
    result: &SwitchResult,
    branch_info: &SwitchBranchInfo,
) -> anyhow::Result<()> {
    if format != SwitchFormat::Json {
        return Ok(());
    }
    let json = SwitchJsonOutput::from_result(result, branch_info);
    let json = serde_json::to_string(&json).context("Failed to serialize to JSON")?;
    println!("{json}");
    Ok(())
}

/// Options for the switch command
struct SwitchOptions<'a> {
    branch: &'a str,
    create: bool,
    base: Option<&'a str>,
    execute: Option<&'a str>,
    execute_args: &'a [String],
    yes: bool,
    clobber: bool,
    /// Resolved from --cd/--no-cd flags: Some(true) = cd, Some(false) = no cd, None = use config
    change_dir: Option<bool>,
    verify: bool,
    format: crate::cli::SwitchFormat,
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
fn run_pre_switch_hooks(
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
            HookType::PreCreate,
            HookType::PostCreate,
            HookType::PostSwitch,
        ]
    } else {
        &[HookType::PostSwitch]
    }
}

/// Approve switch hooks upfront and show "Commands declined" if needed.
///
/// Switch hooks resolve their commands from the invoking worktree's
/// `.config/wt.toml` — the worktree `wt switch` ran in. Selecting them here,
/// at the gate, freezes the exact commands `execute_switch` will run into the
/// [`ApprovedHookPlan`].
///
/// Returns `(hooks_approved, plan)`. `hooks_approved` is `false` and the plan
/// empty when `!verify` or the user declined; the covered switch hooks
/// (`pre-start` / `post-start` / `post-switch`) execute only from `plan`.
fn approve_switch_hooks(
    repo: &Repository,
    config: &UserConfig,
    plan: &SwitchPlan,
    yes: bool,
    verify: bool,
) -> anyhow::Result<(bool, ApprovedHookPlan)> {
    if !verify {
        return Ok((false, ApprovedHookPlan::empty()));
    }

    // Non-fatal: a destination with no project hooks must still switch even
    // when the project identifier can't be resolved (the plan ends up empty
    // and `approve` never needs it).
    let project_id = repo.project_identifier().ok();
    let pid = project_id.as_deref();
    let project_config = repo.load_project_config()?;
    let mut builder = HookPlanBuilder::new(project_config.as_ref(), config, pid);
    builder.add(
        plan.worktree_path(),
        switch_post_hook_types(plan.is_create()),
    );
    match builder.finish().approve(pid, yes)? {
        Some(approved) => Ok((true, approved)),
        None => {
            let on_decline = if plan.is_create() {
                "Commands declined, continuing worktree creation without hooks"
            } else {
                "Commands declined, switching without hooks"
            };
            eprintln!("{}", info_message(on_decline));
            Ok((false, ApprovedHookPlan::empty()))
        }
    }
}

/// Spawn post-switch (and post-start for creates) background hooks.
fn spawn_switch_background_hooks(
    config: &UserConfig,
    result: &SwitchResult,
    branch: Option<&str>,
    yes: bool,
    extra_vars: &[(&str, &str)],
    hooks_display_path: Option<&Path>,
    hook_plan: &ApprovedHookPlan,
) -> anyhow::Result<()> {
    // Background hooks run in the new/destination worktree. `hook_repo` roots
    // the *render* context there; the command set is the frozen `hook_plan`
    // (selected at the gate from the invoking worktree's config), so no
    // `.config/wt.toml` is re-read.
    let hook_repo = Repository::at(result.path())?;
    let ctx = CommandContext::new(&hook_repo, config, branch, result.path(), yes);

    let mut announcer = HookAnnouncer::new(&hook_repo, false);
    register_planned(
        &mut announcer,
        hook_plan,
        result.path(),
        &ctx,
        HookType::PostSwitch,
        extra_vars,
        hooks_display_path,
    )?;
    if matches!(result, SwitchResult::Created { .. }) {
        register_planned(
            &mut announcer,
            hook_plan,
            result.path(),
            &ctx,
            HookType::PostCreate,
            extra_vars,
            hooks_display_path,
        )?;
    }
    announcer.flush()
}

/// Capture the source worktree's branch and root for `{{ base }}` /
/// `{{ base_worktree_path }}` in post-switch hooks. Returns empty strings
/// when recovered from a deleted CWD — the source worktree is gone, and
/// `current_worktree()` would resolve to the recovered ancestor (typically
/// the main worktree), which would misleadingly report main's branch/path
/// as the user's "base".
fn capture_switch_source(repo: &Repository, is_recovered: bool) -> (String, String) {
    if is_recovered {
        return (String::new(), String::new());
    }
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
    (source_branch, source_path)
}

/// The full switch sequence shared by the argument path ([`run_switch`]) and
/// the interactive picker.
///
/// Each caller only resolves a branch identifier and loads config; everything
/// else runs in [`SwitchPipeline::run`] — the bare-repo path-fix offer,
/// pre-switch hooks, source-identity capture, `plan_switch` →
/// `approve_switch_hooks` → `validate_switch_templates` → `execute_switch` →
/// output → background hooks → `--execute`. One sequence, so the two entry
/// points cannot drift. In particular the single `verify` / `yes` pair gates
/// every hook, so the picker and the argument path cannot diverge on hook
/// approval — the picker once auto-approved `pre-switch` hooks because it kept
/// its own copy of that call.
///
/// The picker-vs-argument differences are field values, not separate code: the
/// picker passes `verify: true`, `yes: false`, `capture_source: false`,
/// `suggestion_ctx: None`, `execute: None`, and `shell_integration_binary:
/// None`.
pub(crate) struct SwitchPipeline<'a> {
    pub repo: &'a Repository,
    /// Mutable because the bare-repo path-fix offer
    /// (`offer_bare_repo_worktree_path_fix`) and the shell-integration offer
    /// (`prompt_shell_integration`) record onto it; every other step reborrows
    /// it shared.
    pub config: &'a mut UserConfig,
    /// Branch identifier — a CLI argument or the picker's selection. Symbolic
    /// forms (`-`, `@`, `pr:`/`mr:`) are resolved downstream by `plan_switch`.
    pub identifier: &'a str,
    pub create: bool,
    pub base: Option<&'a str>,
    pub clobber: bool,
    pub verify: bool,
    /// `--yes`: skip approval prompts and force past clobber checks.
    pub yes: bool,
    pub change_dir: bool,
    pub format: SwitchFormat,
    /// True when `current_or_recover` recovered from a deleted CWD. Suppresses
    /// pre-switch hooks (no source worktree to run them against) and source
    /// capture.
    pub is_recovered: bool,
    /// Error-enrichment context for a failed `plan_switch`, so the hint
    /// suggests the full `wt switch … --execute=… -- …`. `None` for the
    /// picker, which has no `--execute`.
    pub suggestion_ctx: Option<SwitchSuggestionCtx>,
    /// Whether to capture the source worktree's branch/root before the switch,
    /// for post-switch `{{ base }}` / `{{ base_worktree_path }}`. The argument
    /// path captures; the picker does not — it does not track where the user
    /// came from, so an existing switch's base vars stay unset.
    pub capture_source: bool,
    /// `--execute` command and its trailing args. `None` / empty for the picker.
    pub execute: Option<&'a str>,
    pub execute_args: &'a [String],
    /// Binary name for the shell-integration offer. `Some` only on the argument
    /// path; the picker does not offer shell integration.
    pub shell_integration_binary: Option<&'a str>,
}

impl SwitchPipeline<'_> {
    /// Plan, approve, execute, and report the switch, then spawn its
    /// background hooks and run any `--execute` command.
    pub(crate) fn run(self) -> anyhow::Result<()> {
        let Self {
            repo,
            config,
            identifier,
            create,
            base,
            clobber,
            verify,
            yes,
            change_dir,
            format,
            is_recovered,
            suggestion_ctx,
            capture_source,
            execute,
            execute_args,
            shell_integration_binary,
        } = self;

        // Offer to fix worktree-path for bare repos with hidden directory names
        // (.git, .bare) before anything reads worktree-path config.
        offer_bare_repo_worktree_path_fix(repo, config, identifier)?;

        // Run pre-switch hooks before branch resolution or worktree creation.
        // {{ branch }} receives the raw user input (before resolution). Skip
        // when recovered — the source worktree is gone, nothing to run hooks
        // against. `yes` is the single switch-wide flag, so the picker (no
        // `--yes`) and the argument path gate `pre-switch` hooks identically.
        if verify && !is_recovered {
            run_pre_switch_hooks(repo, config, identifier, yes)?;
        }

        // Capture source (base) worktree identity BEFORE the switch, for
        // post-switch {{ base }} / {{ base_worktree_path }}. Done here — after
        // pre-switch hooks, before plan / approve / validate, none of which
        // move the current worktree. The picker passes `capture_source: false`;
        // it does not track where the user came from.
        let (source_branch, source_path) = if capture_source {
            capture_switch_source(repo, is_recovered)
        } else {
            (String::new(), String::new())
        };

        // Validate and resolve the target branch.
        let plan = plan_switch(repo, identifier, create, base, clobber, config).map_err(|err| {
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

        // "Approve at the Gate": collect and approve hooks upfront. Approval
        // happens once at the command entry point. If the user declines, skip
        // hooks but continue with the worktree operation. Switch hooks resolve
        // their config from the invoking worktree — see `approve_switch_hooks`.
        let (hooks_approved, hook_plan) = approve_switch_hooks(repo, config, &plan, yes, verify)?;

        // Pre-flight: validate all templates before mutation (worktree
        // creation). Catches syntax errors and undefined variables early so a
        // broken template doesn't leave behind a half-created worktree that
        // blocks re-running.
        validate_switch_templates(repo, config, &plan, execute, execute_args, hooks_approved)?;

        // Execute the validated plan.
        let (result, branch_info) =
            execute_switch(repo, plan, config, yes, hooks_approved, &hook_plan)?;

        // --format=json: write structured result to stdout. All behavior
        // (hooks, --execute, shell integration) proceeds normally — format only
        // affects output.
        emit_switch_json(format, &result, &branch_info)?;

        // Early exit for benchmarking time-to-first-output.
        if std::env::var_os("WORKTRUNK_FIRST_OUTPUT").is_some() {
            return Ok(());
        }

        // Compute path mismatch lazily (deferred from plan_switch for existing
        // worktrees). Skip detached HEAD worktrees (branch is None) — no branch
        // to compute the expected path from.
        let branch_info = match &result {
            SwitchResult::Existing { path } | SwitchResult::AlreadyAt(path) => {
                let expected_path = branch_info
                    .branch
                    .as_deref()
                    .and_then(|b| path_mismatch(repo, b, path, config));
                SwitchBranchInfo {
                    expected_path,
                    ..branch_info
                }
            }
            _ => branch_info,
        };

        // Show success message (temporal locality: immediately after the
        // worktree operation). Returns the path to display in hooks when the
        // user's shell won't be in the worktree, and shows the worktree-path
        // hint on first --create (before the shell integration warning).
        //
        // When the user's CWD has been deleted, `std::env::current_dir()`
        // fails — fall back to `repo_path()` (the main worktree root).
        // `current_worktree().root()` resolves against the Repository's
        // discovery path, which is alive even after recovery, but we keep the
        // same fallback for any pathological case where rev-parse fails.
        let fallback_path = repo.repo_path()?.to_path_buf();
        let cwd = std::env::current_dir().unwrap_or(fallback_path.clone());
        let source_root = repo.current_worktree().root().unwrap_or(fallback_path);
        let hooks_display_path =
            handle_switch_output(&result, &branch_info, change_dir, Some(&source_root), &cwd)?;

        // Offer shell integration if not already installed/active (only shows
        // the prompt/hint when shell integration isn't working). With
        // --execute, show hints only — don't interrupt with a prompt. Skip when
        // change_dir is false (the user opted out of cd, so shell integration
        // is irrelevant) and on the picker path (no `binary_name`).
        // Best-effort: don't fail the switch if the offer fails.
        if let Some(binary_name) = shell_integration_binary
            && change_dir
            && !is_shell_integration_active()
        {
            let skip_prompt = execute.is_some();
            let _ = prompt_shell_integration(repo, config, binary_name, skip_prompt);
        }

        // Build template vars for base/target context (used by both hooks and
        // --execute). "base" is the source worktree the user switched from (all
        // switches), or the branch they branched from (creates). "target"
        // matches the bare vars (the destination) — kept symmetric with
        // pre-switch.
        let template_vars =
            TemplateVars::for_post_switch(&result, &branch_info, &source_branch, &source_path);
        let extra_vars = template_vars.as_extra_vars();

        // Spawn background hooks after the success message.
        // - post-switch: runs on ALL switches (shows "@ path" when the shell
        //   won't be there)
        // - post-start: runs only when creating a NEW worktree
        if hooks_approved {
            spawn_switch_background_hooks(
                config,
                &result,
                branch_info.branch.as_deref(),
                yes,
                &extra_vars,
                hooks_display_path.as_deref(),
                &hook_plan,
            )?;
        }

        // Execute the user command after post-start hooks have been spawned.
        // Note: execute_args requires execute via clap's `requires` attribute.
        if let Some(cmd) = execute {
            // Build template context for expansion (includes base vars when
            // creating).
            let ctx = CommandContext::new(
                repo,
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

            // The `--execute` payload is parsed by the active directive shell:
            // the PowerShell wrapper `Invoke-Expression`s the EXEC directive
            // file, every other wrapper (and the direct `sh -c` non-integration
            // path) is POSIX. Escape interpolated values for whichever it is —
            // the one place the escaping is shell-aware (hooks/aliases stay
            // POSIX). See `worktrunk::shell_exec::directive_shell_escape_mode`.
            let escape_mode = directive_shell_escape_mode();

            // Expand template variables in command, escaped for the directive shell.
            let expanded_cmd = expand_template(cmd, &vars, escape_mode, repo, "--execute command")?;

            // Append any trailing args (after --) to the execute command.
            // Each arg is template-expanded literally, then escaped for the
            // directive shell so the wrapper parses it as one literal argument.
            let full_cmd = if execute_args.is_empty() {
                expanded_cmd
            } else {
                let expanded_args: Result<Vec<_>, _> = execute_args
                    .iter()
                    .map(|arg| {
                        expand_template(
                            arg,
                            &vars,
                            ShellEscapeMode::Literal,
                            repo,
                            "--execute argument",
                        )
                    })
                    .collect();
                let escaped_args: Vec<_> = expanded_args?
                    .iter()
                    .map(|arg| shell_escape_for(escape_mode, arg))
                    .collect();
                format!("{} {}", expanded_cmd, escaped_args.join(" "))
            };
            execute_user_command(&full_cmd, hooks_display_path.as_deref())?;
        }

        Ok(())
    }
}

/// Handle the switch command.
fn run_switch(
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
        let escaped = shell_escape::unix::escape(exec.into());
        SwitchSuggestionCtx {
            extra_flags: vec![format!("--execute={escaped}")],
            trailing_args: execute_args.to_vec(),
        }
    });

    SwitchPipeline {
        repo: &repo,
        config,
        identifier: branch,
        create,
        base,
        clobber,
        verify,
        yes,
        change_dir,
        format,
        is_recovered,
        suggestion_ctx,
        capture_source: true,
        execute,
        execute_args,
        shell_integration_binary: Some(binary_name),
    }
    .run()
}

/// Entry point for the `wt switch` command.
pub fn handle_switch_command(args: SwitchArgs, yes: bool) -> anyhow::Result<()> {
    let verify = args.hooks.resolve();

    // With no branch argument, `wt switch` opens a TUI picker — config
    // deprecation warnings would render above the picker and push it down.
    // They're still shown by other commands (`wt list`, `wt merge`, …).
    if args.branch.is_none() {
        worktrunk::config::suppress_warnings();
    }

    UserConfig::load()
        .context("Failed to load config")
        .and_then(|mut config| {
            // No branch argument: open interactive picker
            let change_dir_flag = flag_pair(args.cd, args.no_cd);

            let Some(branch) = args.branch else {
                // No branch argument: open the interactive picker.
                return crate::commands::handle_picker(
                    args.branches,
                    args.remotes,
                    args.prs,
                    change_dir_flag,
                    args.format,
                );
            };

            run_switch(
                SwitchOptions {
                    branch: &branch,
                    create: args.create,
                    base: args.base.as_deref(),
                    execute: args.execute.as_deref(),
                    execute_args: &args.execute_args,
                    yes,
                    clobber: args.clobber,
                    change_dir: change_dir_flag,
                    verify,
                    format: args.format,
                },
                &mut config,
                &crate::binary_name(),
            )
        })
}

/// Whether `value` is a single clean program-name token — the form `--execute`
/// keeps accepting unchanged once it switches to the argv input model.
///
/// First character `[A-Za-z0-9._/@]`, the rest additionally `+`/`-`. On
/// Windows, `\` and `:` are also allowed so native paths (`C:\dir\tool`) are
/// not flagged; on POSIX they are shell metacharacters and stay excluded.
/// This rejects a leading `-`/`+` (an option-like `argv[0]` resolves
/// differently), whitespace, `{{ }}` template markup, and every shell
/// metacharacter — any of which means the value is not a bare program name.
fn is_clean_program_token(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    let first_ok = first.is_ascii_alphanumeric()
        || matches!(first, '.' | '_' | '/' | '@')
        || (cfg!(windows) && first == '\\');
    first_ok
        && chars.all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '.' | '_' | '/' | '@' | '+' | '-')
                || (cfg!(windows) && matches!(c, '\\' | ':'))
        })
}

/// Warn when a `--execute` value will change behavior under the upcoming argv
/// input model.
///
/// A future release runs `-x` as a single program (with arguments after `--`),
/// not a shell command line. Warn now for any value that is not a single
/// program token — shell syntax, multiple words, or `{{ }}` markup (flagged
/// conservatively, even when it would expand to a clean name).
///
/// A single program token — including a path — is unaffected and stays silent.
/// A bare name that is really a shell alias/function/builtin is not detectable
/// here without the user's shell, so it is left to fail loudly at the cutover
/// rather than guessed at. Informational only — it never blocks the switch.
///
/// The hint reconstructs the command line that runs today — the `-x` value
/// with its trailing args appended, the way `run_switch` joins them — and
/// wraps it for whichever shell the active wrapper evaluates the payload
/// with (`sh` / `fish` / `pwsh`), so the suggestion is behavior-preserving
/// on fish/PowerShell, not just POSIX sh, and complete when trailing args
/// are present. It also links the tracking issue (#2860) so anyone whose
/// workflow the argv model would regress can report the case before the
/// cutover.
fn warn_if_execute_form_deprecated(cmd: &str, execute_args: &[String]) {
    if is_clean_program_token(cmd) {
        return;
    }
    let mode = directive_shell_escape_mode();
    let (shell, flag) = match mode {
        ShellEscapeMode::PowerShell => ("pwsh", "-Command"),
        ShellEscapeMode::Fish => ("fish", "-c"),
        _ => ("sh", "-c"),
    };
    let command_line = if execute_args.is_empty() {
        cmd.to_string()
    } else {
        let escaped: Vec<String> = execute_args
            .iter()
            .map(|arg| shell_escape_for(mode, arg))
            .collect();
        format!("{} {}", cmd, escaped.join(" "))
    };
    let suggested = shell_escape_for(mode, &command_line);
    eprintln!(
        "{}",
        warning_message(cformat!(
            "<bold>--execute</> will change in a future release: it will run a single program, with arguments after <bold>--</>, not a shell command line"
        ))
    );
    eprintln!(
        "{}",
        hint_message(cformat!(
            "Comment at <underline>https://github.com/max-sixty/worktrunk/issues/2860</> if the new single-program form would make a workflow worse; to run this command line unchanged, pass it to a shell: <underline>--execute {shell} -- {flag} {suggested}</>"
        ))
    );
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
/// Hook templates checked here come from the invoking worktree's
/// `.config/wt.toml` — the same config the switch hooks run against — so the
/// templates validated are the ones that will actually be expanded.
///
/// Validates:
/// - `--execute` command template (if present)
/// - `--execute` trailing arg templates (if present)
/// - Hook templates (pre-start, post-start, post-switch) from user and project config
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
        warn_if_execute_form_deprecated(cmd, execute_args);
    }

    // Validate hook templates only when hooks will actually run
    if !hooks_approved {
        return Ok(());
    }

    let project_config = repo.load_project_config()?;
    let user_hooks = config.hooks(repo.project_identifier().ok().as_deref());

    for &hook_type in switch_post_hook_types(plan.is_create()) {
        let user_cfg = user_hooks.get(hook_type);
        let proj_cfg = project_config.as_ref().and_then(|c| c.hooks.get(hook_type));
        for (source, cfg) in [("user", user_cfg), ("project", proj_cfg)] {
            if let Some(cfg) = cfg {
                for cmd in cfg.commands() {
                    // Skip full validation for templates referencing {{ vars.X }} —
                    // those values come from git config at execution time, after
                    // prior pipeline steps set them. Syntax is still checked by
                    // prepare_steps.
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

#[cfg(test)]
mod tests {
    use super::*;
    use worktrunk::testing::TestRepo;

    #[test]
    fn is_clean_program_token_matches_only_bare_names() {
        // Bare program names — unchanged under the argv input model.
        for ok in [
            "git",
            "claude",
            "node18",
            "my-tool",
            "tool.sh",
            "/usr/bin/env",
            "./build",
            "_x",
            "@scope/pkg",
        ] {
            assert!(is_clean_program_token(ok), "expected clean token: {ok:?}");
        }
        // Not bare names — empty, whitespace, shell syntax, template markup,
        // or an option-like leading character.
        for bad in [
            "",
            "npm run dev",
            "a && b",
            "echo $HOME",
            "code {{ worktree_path }}",
            "a|b",
            "-flag",
            "+x",
        ] {
            assert!(!is_clean_program_token(bad), "expected non-token: {bad:?}");
        }
        // A native Windows path is a clean token only when targeting Windows;
        // on POSIX, `\` and `:` are shell metacharacters.
        assert_eq!(
            is_clean_program_token(r"C:\Tools\foo.exe"),
            cfg!(windows),
            "Windows path classification should follow the target OS"
        );
    }

    #[test]
    fn capture_switch_source_returns_empty_when_recovered() {
        // When recovered from a deleted CWD, post-switch hooks must see empty
        // `{{ base }}` / `{{ base_worktree_path }}` rather than the recovered
        // ancestor's identity (typically the main worktree's branch/path).
        let test = TestRepo::with_initial_commit();
        let (branch, path) = capture_switch_source(&test.repo, true);
        assert_eq!(branch, "");
        assert_eq!(path, "");
    }

    #[test]
    fn capture_switch_source_returns_branch_and_path_normally() {
        // When not recovered, the helper reports the current worktree's
        // identity. This guards against accidental regressions to the
        // `is_recovered` gate (e.g., always returning empty).
        let test = TestRepo::with_initial_commit();
        let (branch, path) = capture_switch_source(&test.repo, false);
        assert_eq!(branch, "main");
        assert!(!path.is_empty(), "source_path should be the worktree root");
    }

    #[test]
    fn choose_pr_provider_prefers_github_over_azure() {
        // Mixed-remote setup: a repo with both a GitHub remote and an Azure
        // DevOps remote falls through to GitHub. Operators with an explicit
        // preference set `forge.platform`.
        let test = TestRepo::with_initial_commit();
        test.run_git(&["remote", "add", "origin", "https://github.com/myorg/myrepo"]);
        test.run_git(&[
            "remote",
            "add",
            "azure",
            "https://dev.azure.com/myorg/proj/_git/myrepo",
        ]);

        assert_eq!(
            choose_pr_provider(&test.repo).unwrap(),
            PrProviderChoice::GitHub
        );
    }

    #[test]
    fn choose_pr_provider_azure_only() {
        // Azure-only repo (no GitHub remote) gets the Azure provider.
        let test = TestRepo::with_initial_commit();
        test.run_git(&[
            "remote",
            "add",
            "origin",
            "https://dev.azure.com/myorg/proj/_git/myrepo",
        ]);

        assert_eq!(
            choose_pr_provider(&test.repo).unwrap(),
            PrProviderChoice::AzureDevOps
        );
    }

    #[test]
    fn choose_pr_provider_no_recognised_remote() {
        // Falls back to GitHub when no recognisable forge remote exists,
        // preserving the existing error message from `gh`.
        let test = TestRepo::with_initial_commit();
        assert_eq!(
            choose_pr_provider(&test.repo).unwrap(),
            PrProviderChoice::GitHub
        );
    }

    #[test]
    fn choose_pr_provider_forge_platform_override_wins() {
        // The bug worth covering: a mixed-remote repo where the user explicitly
        // pinned `forge.platform = "azure-devops"`. Without the override, the
        // GitHub remote would win — and the user has no way to redirect `pr:N`.
        // A regression that drops the project-config read would flip this
        // assertion to `GitHub`.
        let test = TestRepo::with_initial_commit();
        test.run_git(&["remote", "add", "origin", "https://github.com/myorg/myrepo"]);
        test.run_git(&[
            "remote",
            "add",
            "azure",
            "https://dev.azure.com/myorg/proj/_git/myrepo",
        ]);
        test.write_project_config("[forge]\nplatform = \"azure-devops\"\n");

        assert_eq!(
            choose_pr_provider(&test.repo).unwrap(),
            PrProviderChoice::AzureDevOps
        );
    }

    #[test]
    fn choose_pr_provider_forge_platform_github_in_azure_only_repo() {
        // Inverse override: Azure-only remotes but `forge.platform = "github"`.
        // Verifies the config arm flips the inferred-from-remotes default.
        let test = TestRepo::with_initial_commit();
        test.run_git(&[
            "remote",
            "add",
            "origin",
            "https://dev.azure.com/myorg/proj/_git/myrepo",
        ]);
        test.write_project_config("[forge]\nplatform = \"github\"\n");

        assert_eq!(
            choose_pr_provider(&test.repo).unwrap(),
            PrProviderChoice::GitHub
        );
    }
}
