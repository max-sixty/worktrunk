//! The `wt remove` command: validate removal targets, approve hooks, and
//! dispatch each removal to the output handler.

use std::collections::HashSet;
use std::path::Path;

use anyhow::Context;
use worktrunk::HookType;
use worktrunk::config::UserConfig;
use worktrunk::git::{BranchDeletionMode, ErrorExt, Repository, ResolvedWorktree};
use worktrunk::styling::{eprintln, info_message};

use crate::cli::{RemoveArgs, SwitchFormat};
use crate::output::{BackgroundFallbackMode, handle_remove_output};

use super::hook_plan::{ApprovedHookPlan, HookPlanBuilder};
use super::hooks::HookAnnouncer;
use super::repository_ext::RepositoryCliExt;
use super::worktree::RemoveResult;
use super::{RemoveTarget, flag_pair, resolve_worktree_arg};

/// Validated removal plans, categorized for ordered execution.
///
/// Multi-worktree removal validates all targets upfront, then executes in order:
/// other worktrees first, branch-only cases next, current worktree last.
struct RemovePlans {
    others: Vec<RemoveResult>,
    branch_only: Vec<RemoveResult>,
    current: Option<RemoveResult>,
    errors: Vec<anyhow::Error>,
}

impl RemovePlans {
    fn has_valid_plans(&self) -> bool {
        !self.others.is_empty() || !self.branch_only.is_empty() || self.current.is_some()
    }

    fn record_error(&mut self, e: anyhow::Error) {
        // The remove command collects per-target errors and surfaces each
        // individually (partial-success path). Render the typed
        // diagnostic block when present so locked/dirty/etc. errors
        // carry their hint, falling back to the short label otherwise.
        let rendered = e.render_diagnostic().unwrap_or_else(|| e.to_string());
        if !rendered.is_empty() {
            eprintln!("{rendered}");
        }
        self.errors.push(e);
    }
}

/// Validate all removal targets, returning categorized plans.
///
/// Resolves each branch name, determines whether it's the current worktree,
/// another worktree, or branch-only, and prepares the removal plan.
/// Errors are collected (not fatal) to support partial success.
fn validate_remove_targets(
    repo: &Repository,
    branches: Vec<String>,
    keep_branch: bool,
    force_delete: bool,
    force: bool,
) -> RemovePlans {
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

    let deletion_mode = BranchDeletionMode::from_flags(keep_branch, force_delete);
    let worktrees = repo.list_worktrees().ok();

    // Capture once for the validation loop. Validation only reads — actual
    // removals run later in `handle_remove_output`, so ref state is stable
    // across candidates here. Errors propagate to per-candidate calls, which
    // fall back to capturing internally when None.
    let snapshot = repo.capture_refs().ok();

    let mut plans = RemovePlans {
        others: Vec::new(),
        branch_only: Vec::new(),
        current: None,
        errors: Vec::new(),
    };

    for branch_name in &branches {
        let resolved = match resolve_worktree_arg(repo, branch_name) {
            Ok(r) => r,
            Err(e) => {
                plans.record_error(e);
                continue;
            }
        };

        match resolved {
            ResolvedWorktree::Worktree { path, branch } => {
                // Use canonical paths to avoid symlink/normalization mismatches
                let path_canonical = dunce::canonicalize(&path).unwrap_or(path);
                let is_current = current_worktree.as_ref() == Some(&path_canonical);

                if is_current {
                    match repo.prepare_worktree_removal(
                        RemoveTarget::Current,
                        deletion_mode,
                        force,
                        None,
                        worktrees,
                        snapshot.as_ref(),
                    ) {
                        Ok(result) => plans.current = Some(result),
                        Err(e) => plans.record_error(e),
                    }
                    continue;
                }

                // Non-current worktree: remove by branch name, or by path for
                // detached worktrees (which have no branch).
                let target = if let Some(ref branch_name) = branch {
                    RemoveTarget::Branch(branch_name)
                } else {
                    RemoveTarget::Path(&path_canonical)
                };
                match repo.prepare_worktree_removal(
                    target,
                    deletion_mode,
                    force,
                    None,
                    worktrees,
                    snapshot.as_ref(),
                ) {
                    Ok(result) => plans.others.push(result),
                    Err(e) => plans.record_error(e),
                }
            }
            ResolvedWorktree::BranchOnly { branch } => {
                match repo.prepare_worktree_removal(
                    RemoveTarget::Branch(&branch),
                    deletion_mode,
                    force,
                    None,
                    worktrees,
                    snapshot.as_ref(),
                ) {
                    Ok(result) => plans.branch_only.push(result),
                    Err(e) => plans.record_error(e),
                }
            }
        }
    }

    plans
}

/// Terminate processes whose working directory is under a worktree, for
/// `--reap` (experimental). Discovery, the controlling-terminal / self
/// exclusions, and the `SIGTERM`→`SIGKILL` escalation live in
/// [`worktrunk::git::reap`]; this wrapper renders the user-facing progress.
///
/// Called before the worktree directory is staged/renamed — cwd matching
/// requires the directory at its original path. Best-effort: no candidate,
/// or a survivor that ignored both signals, never blocks the removal.
#[cfg(unix)]
fn reap_worktree_processes(worktree_path: &Path, label: &str) {
    use color_print::cformat;
    use worktrunk::git::reap;
    use worktrunk::styling::{
        format_with_gutter, progress_message, success_message, warning_message,
    };

    let procs = reap::collect_reapable(worktree_path);
    if procs.is_empty() {
        eprintln!(
            "{}",
            info_message(cformat!(
                "No processes to reap under <bold>{label}</> worktree"
            ))
        );
        return;
    }

    let count = procs.len();
    let noun = reap::process_noun(count);
    eprintln!(
        "{}",
        progress_message(cformat!(
            "Reaping {count} {noun} under <bold>{label}</> worktree"
        ))
    );
    let listing = procs
        .iter()
        .map(|p| format!("{} {}", p.pid, p.command))
        .collect::<Vec<_>>()
        .join("\n");
    eprintln!("{}", format_with_gutter(&listing, None));

    let pids: Vec<u32> = procs.iter().map(|p| p.pid).collect();
    let gone = reap::reap_pids(&pids);
    match reap::reap_summary(count, gone) {
        Ok(msg) => eprintln!("{}", success_message(msg)),
        Err(msg) => eprintln!("{}", warning_message(msg)),
    }
}

/// Reap the removed worktree's processes when `--reap` is set and the result
/// removed a worktree (branch-only deletions have no directory to scope by).
/// The display label prefers the branch name, falling back to the worktree
/// directory name. No-op on non-Unix, where `--reap` is rejected up front.
fn maybe_reap_result(result: &RemoveResult, reap_enabled: bool) {
    #[cfg(unix)]
    if reap_enabled && let Some(path) = result.removed_worktree_path() {
        let label = result
            .branch_name()
            .map(str::to_string)
            .or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "worktree".to_string());
        reap_worktree_processes(path, &label);
    }
    #[cfg(not(unix))]
    let _ = (result, reap_enabled);
}

/// Entry point for the `wt remove` command.
///
/// # Command flow
///
/// 1. **Validate** all target worktrees up front via `prepare_worktree_removal`
///    (clean check, branch-deletion-safety check, force-flag handling).
/// 2. **Approve hooks** (`pre-remove`, `post-remove`, `post-switch`) if
///    running interactively and any hooks are configured.
/// 3. **Dispatch to `handle_remove_output`** per target. For each, the output
///    handler runs `pre-remove` hooks in the worktree, then either:
///    - **Foreground** (`--foreground`): stop fsmonitor → rename into
///      `.git/wt/trash/<name>-<timestamp>/` → prune metadata → delete branch
///      → synchronous `remove_dir_all` on the staged directory.
///    - **Background** (default): stop fsmonitor → rename + prune +
///      synchronous branch delete → spawn detached `rm -rf` on the staged
///      directory. Cross-filesystem or locked worktrees fall back to
///      `git worktree remove` in the detached process.
/// 4. **Post-remove hooks** run in the background after dispatch.
/// 5. **Internal sweep** (fire-and-forget, after primary output): stale
///    `.git/wt/trash/` entries older than 24 hours are removed by a detached
///    `rm -rf`, and orphaned `git fsmonitor--daemon` processes (worktree gone)
///    are terminated. Runs last so it never delays the user-visible progress
///    or success message. See [`super::process::run_internal_sweep`].
pub fn handle_remove_command(args: RemoveArgs, yes: bool) -> anyhow::Result<()> {
    let json_mode = args.format == SwitchFormat::Json;
    let verify = args.hooks.resolve();
    UserConfig::load()
        .context("Failed to load config")
        .and_then(|config| {
            let repo = Repository::current().context("Failed to remove worktree")?;

            // CLI flags override config; otherwise fall through to [remove] delete-branch
            // (defaults to true).
            let project = repo.project_identifier().ok();
            let cli_override = flag_pair(args.delete_branch, args.no_delete_branch);
            let delete_branch =
                cli_override.unwrap_or_else(|| config.remove(project.as_deref()).delete_branch());

            // Validate conflicting flags
            if !delete_branch && args.force_delete {
                return Err(worktrunk::git::GitError::Other {
                    message: "Cannot use --force-delete with delete-branch=false (set via --no-delete-branch or [remove] delete-branch = false)".into(),
                }
                .into());
            }

            // `--reap` relies on per-process cwd discovery (`lsof`/`ps`), which
            // has no cheap Windows equivalent — reject it there rather than
            // silently no-op.
            #[cfg(not(unix))]
            if args.reap {
                anyhow::bail!("--reap is not supported on Windows");
            }

            // Helper: build and approve, once, the frozen hook plan the
            // removal will run. Every hook (`pre-remove` / `post-remove` per
            // removed worktree, `post-switch` per post-removal destination) is
            // selected from the invoking worktree's `.config/wt.toml` — the
            // worktree `wt remove` ran in. `!verify` (`--no-hooks`) or a
            // declined prompt yields an empty plan — every executor then runs
            // no project hooks.
            let approve_remove = |removed_worktree_paths: &[&Path],
                                  destination_paths: &[&Path],
                                  yes: bool|
             -> anyhow::Result<ApprovedHookPlan> {
                if !verify {
                    return Ok(ApprovedHookPlan::empty());
                }
                // Non-fatal: a worktree with no project hooks must remove even
                // when the project identifier can't be resolved (the plan ends
                // up empty and `approve` never needs it). Matches the pre-plan
                // behaviour where the empty-batch fast path ran first.
                let project_id = repo.project_identifier().ok();
                let pid = project_id.as_deref();
                let project_config = repo.load_project_config()?;
                let mut builder = HookPlanBuilder::new(project_config.as_ref(), &config, pid);
                for &wt_path in removed_worktree_paths {
                    builder.add(wt_path, &[HookType::PreRemove, HookType::PostRemove]);
                }
                let mut seen_dests = std::collections::HashSet::new();
                for &dest in destination_paths {
                    if !seen_dests.insert(dest) {
                        continue;
                    }
                    builder.add(dest, &[HookType::PostSwitch]);
                }
                match builder.finish().approve(pid, yes)? {
                    Some(plan) => Ok(plan),
                    None => {
                        eprintln!(
                            "{}",
                            info_message("Commands declined, continuing removal without hooks")
                        );
                        Ok(ApprovedHookPlan::empty())
                    }
                }
            };

            let branches = args.branches;

            if branches.is_empty() {
                // Single worktree removal: validate FIRST, then approve, then execute
                let result = repo
                    .prepare_worktree_removal(
                        RemoveTarget::Current,
                        BranchDeletionMode::from_flags(!delete_branch, args.force_delete),
                        args.force,
                        None,
                        None,
                        None,
                    )
                    .context("Failed to remove worktree")?;

                // Early exit for benchmarking time-to-first-output
                if std::env::var_os("WORKTRUNK_FIRST_OUTPUT").is_some() {
                    return Ok(());
                }

                // "Approve at the Gate": approval happens AFTER validation passes
                let plan = approve_remove(
                    result.removed_worktree_path().as_slice(),
                    result.destination_path().as_slice(),
                    yes,
                )?;

                maybe_reap_result(&result, args.reap);

                let mut announcer = HookAnnouncer::new(&repo, false);
                handle_remove_output(
                    &result,
                    args.foreground,
                    &plan,
                    false,
                    false,
                    &mut announcer,
                    BackgroundFallbackMode::Detached,
                )?;
                announcer.flush()?;
                if json_mode {
                    let json = serde_json::json!([result.to_json()]);
                    println!("{}", serde_json::to_string_pretty(&json)?);
                }
                // Fire-and-forget repo-wide internal cleanup (stale trash +
                // orphaned fsmonitor daemons) — runs after primary output so
                // it never delays the user-visible progress/success message.
                super::process::run_internal_sweep(&repo);
                Ok(())
            } else {
                // Multi-worktree removal: validate ALL first, then approve, then execute
                let plans = validate_remove_targets(
                    &repo,
                    branches,
                    !delete_branch,
                    args.force_delete,
                    args.force,
                );

                if !plans.has_valid_plans() {
                    anyhow::bail!("");
                }

                // Early exit for benchmarking time-to-first-output
                if std::env::var_os("WORKTRUNK_FIRST_OUTPUT").is_some() {
                    return Ok(());
                }

                // Approve hooks (only if we have valid plans). Each removed
                // worktree's `pre-remove` / `post-remove` is approved against
                // that worktree's config, and its `post-switch` against the
                // worktree the user lands in — see `approve_remove` above.
                // (`destination_targets` is mostly the primary worktree
                // repeated; the helper dedups by template.)
                let all_plans = || {
                    plans
                        .others
                        .iter()
                        .chain(&plans.branch_only)
                        .chain(plans.current.iter())
                };
                let removed_targets: Vec<&Path> =
                    all_plans().filter_map(|r| r.removed_worktree_path()).collect();
                let destination_targets: Vec<&Path> =
                    all_plans().filter_map(|r| r.destination_path()).collect();
                let plan = approve_remove(&removed_targets, &destination_targets, yes)?;

                // Execute all validated plans: others first, branch-only next, current last
                let show_branch =
                    plans.others.len() + plans.branch_only.len() + plans.current.iter().len() > 1;
                let run = |result: &RemoveResult| -> anyhow::Result<()> {
                    maybe_reap_result(result, args.reap);
                    let mut announcer = HookAnnouncer::new(&repo, show_branch);
                    handle_remove_output(
                        result,
                        args.foreground,
                        &plan,
                        false,
                        false,
                        &mut announcer,
                        BackgroundFallbackMode::Detached,
                    )?;
                    announcer.flush()
                };
                for result in &plans.others {
                    run(result)?;
                }
                for result in &plans.branch_only {
                    run(result)?;
                }
                if let Some(ref result) = plans.current {
                    run(result)?;
                }

                if json_mode {
                    let json_items: Vec<serde_json::Value> = plans
                        .others
                        .iter()
                        .chain(&plans.branch_only)
                        .chain(plans.current.as_ref())
                        .map(RemoveResult::to_json)
                        .collect();
                    println!("{}", serde_json::to_string_pretty(&json_items)?);
                }

                // Fire-and-forget repo-wide internal cleanup (stale trash +
                // orphaned fsmonitor daemons) — runs after primary output so
                // it never delays the user-visible progress/success messages.
                super::process::run_internal_sweep(&repo);

                if !plans.errors.is_empty() {
                    anyhow::bail!("");
                }

                Ok(())
            }
        })
}
