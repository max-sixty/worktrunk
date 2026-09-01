//! Output handlers for worktree operations using the global output context

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anstyle::AnsiColor;
use color_print::cformat;
use worktrunk::shell_exec::{Cmd, shell_cwd};
use worktrunk::styling::{eprint, format_bash_with_gutter, stderr};

use crate::commands::command_executor::CommandContext;
use crate::commands::command_executor::FailureStrategy;
use crate::commands::hook_plan::{ApprovedHookPlan, execute_planned_hook, register_planned};
use crate::commands::hooks::HookAnnouncer;
use crate::commands::process::{
    HookLog, InternalOp, build_remove_command, build_remove_command_staged, spawn_detached,
};
use crate::commands::worktree::hooks::PostRemoveContext;
use crate::commands::worktree::{
    BranchFate, RemovalPlan, RetainedReason, SharedBranchCheckout, SwitchBranchInfo, SwitchResult,
};
use worktrunk::config::UserConfig;
use worktrunk::git::ErrorExt;
use worktrunk::git::GitError;
use worktrunk::git::IntegrationReason;
use worktrunk::git::Repository;
use worktrunk::git::path_dir_name;
use worktrunk::git::{
    BranchDeletionMode, BranchDeletionOutcome, BranchDeletionResult, RemoveOptions,
    execute_branch_deletion, remove_worktree_with_cleanup, stage_worktree_removal,
};
use worktrunk::path::{canonicalize_with_parents, format_path_for_display};
use worktrunk::progress::{Progress, format_stats_paren};
use worktrunk::remove_dir::remove_dir_with_progress;
use worktrunk::styling::{
    FormattedMessage, eprintln, error_message, format_with_gutter, hint_message, info_message,
    progress_message, success_message, suggest_command, verbosity, warning_message,
};

use super::shell_integration::{
    compute_shell_warning_reason, explicit_path_hint, git_subcommand_warning,
    print_shell_integration_hint, should_show_explicit_path_hint,
};

// ============================================================================
// Foreground Trash Cleanup
// ============================================================================

/// Walk the staged trash directory, unlinking files with a TTY spinner.
///
/// Used by the foreground removal path after `remove_worktree_with_cleanup`
/// renames the worktree into `<git-common-dir>/wt/trash/`. The spinner shows
/// `⠼ Removing N files · X MiB` while the walk proceeds; the returned counts
/// drive the post-op summary so the success message matches the spinner.
///
/// Suppresses the spinner when stderr isn't a TTY (auto-detected by
/// `Progress::start`) or when verbosity ≥ 1 (verbose mode prefers structured
/// output over live updates). The walk itself is best-effort — see
/// [`remove_dir_with_progress`].
fn cleanup_staged_with_progress(staged: &Path) -> (usize, u64) {
    let progress = if verbosity() >= 1 {
        Progress::disabled()
    } else {
        Progress::start("Removing")
    };
    remove_dir_with_progress(staged, &progress);
    let totals = progress.totals();
    progress.finish();
    totals
}

// ============================================================================
// Background Removal Helpers
// ============================================================================

/// The removal parameters shared by [`spawn_background_removal`] and
/// [`execute_instant_removal_or_fallback`]. `branch_name` / `deletion_mode` /
/// `target_branch` drive the freshly-recomputed branch deletion; the rest
/// control the worktree removal itself.
struct BackgroundRemoval<'a> {
    worktree_path: &'a Path,
    branch_name: Option<&'a str>,
    deletion_mode: BranchDeletionMode,
    target_branch: Option<&'a str>,
    force_worktree: bool,
    changed_directory: bool,
    /// `true` when the planner already decided the branch would be retained
    /// (unmerged, or `--no-delete-branch`) — `print_hints` has explained why,
    /// so [`warn_if_branch_retained`] stays silent on the expected
    /// `NotDeleted` outcome and only fires when the deletion command errors.
    /// `false` means the planner predicted deletion; a `NotDeleted` here is a
    /// race (e.g. `pre-remove` hook advanced the branch) that the user has
    /// otherwise no signal for.
    planner_expected_retention: bool,
}

/// How a background removal behaves when the rename-into-trash fast path fails.
#[derive(Clone, Copy)]
pub enum BackgroundFallbackMode {
    /// Spawn the legacy detached `git worktree remove` — the default for
    /// `wt remove`, `wt merge`, and the picker.
    Detached,
    /// Run the fallback removal and branch deletion synchronously for a
    /// non-current worktree. `wt step prune` uses this so a candidate's
    /// removal is complete by the time it is reported removed — before the
    /// final summary prints, never outliving the prune process.
    SynchronousForNonCurrent,
}

/// How [`handle_remove_output`] executes a [`RemovalPlan`]: one axis, one
/// value — inline with chrome, staged-and-detached with chrome, or inline
/// with none.
#[derive(Clone, Copy)]
pub enum RemovalExecution {
    /// Remove inline and report actual outcomes: progress message, spinner
    /// over the trash cleanup, success message from what really happened.
    Foreground,
    /// Announce, then stage the worktree into trash and hand the `rm -rf` to a
    /// detached process; the branch deletion still runs synchronously on the
    /// fast path. The mode says what happens when the rename-into-trash fails.
    Background(BackgroundFallbackMode),
    /// The TUI (picker) path: the same inline removal as `Foreground` with no
    /// terminal output, no spinner, and no `cd` directive — skim owns the
    /// terminal. Only [`RemovalPlan::Worktree`] arrives here; the picker
    /// deletes branch-only rows directly via `execute_branch_deletion`.
    Silent,
}

enum BackgroundRemovalPlan {
    Detached(String),
    CompletedSynchronously,
}

/// Print `live` when a complete porcelain record names the requested branch
/// and is either non-prunable or locked.
///
/// The caller checks both this script's exit status and its exact output:
/// an empty successful result means no live checkout, `live` means retain, and
/// any tool failure or unexpected output fails closed. Paragraph mode (`RS =
/// ""`) keeps each worktree's `branch`, `prunable`, and `locked` fields
/// associated. A lock wins if a record ever carries both status fields.
const LIVE_BRANCH_WORKTREE_AWK: &str = r#"BEGIN { RS = ""; FS = "\n" }
{
    has_branch = 0
    prunable = 0
    locked = 0
    for (i = 1; i <= NF; i++) {
        if ($i == worktrunk_wanted_branch) has_branch = 1
        if ($i == "prunable" || index($i, "prunable ") == 1) prunable = 1
        if ($i == "locked" || index($i, "locked ") == 1) locked = 1
    }
    if (has_branch && (!prunable || locked)) {
        print "live"
        exit
    }
}"#;

/// Spawn background worktree removal: clean-check, stop fsmonitor,
/// rename-then-prune, spawn detached rm.
///
/// Shared sequence for both detached HEAD and branch background removal paths.
/// The caller is responsible for output messages before this call, and hooks
/// after. Returns the branch's fate — known synchronously on every path except
/// the detached fallback, whose CAS tail runs after this process exits.
fn spawn_background_removal(
    repo: &Repository,
    main_path: &Path,
    removal: &BackgroundRemoval<'_>,
    log_label: &str,
    fallback_mode: BackgroundFallbackMode,
) -> anyhow::Result<BranchFate> {
    let (remove_plan, fate) = execute_instant_removal_or_fallback(repo, removal, fallback_mode)?;

    if let BackgroundRemovalPlan::Detached(remove_command) = remove_plan {
        spawn_detached(
            repo,
            main_path,
            &remove_command,
            log_label,
            &HookLog::Internal(InternalOp::Remove),
            None,
        )?;
    }
    Ok(fate)
}

/// Execute instant worktree removal via rename-then-prune.
///
/// This function has side effects: it renames the worktree directory and prunes git metadata.
/// On the fast path, the branch is also deleted synchronously (since after prune, the branch
/// is no longer checked out in any worktree), and the background command is just `rm -rf`.
/// If rename fails (cross-filesystem, permissions, Windows file locking), either returns the
/// legacy `git worktree remove` command with branch deletion deferred to the background, or
/// runs that fallback synchronously for non-current worktrees when the caller needs the
/// removal complete before it reports success (`wt step prune`).
///
/// The caller is responsible for spawning detached plans in the background.
fn execute_instant_removal_or_fallback(
    repo: &Repository,
    removal: &BackgroundRemoval<'_>,
    fallback_mode: BackgroundFallbackMode,
) -> anyhow::Result<(BackgroundRemovalPlan, BranchFate)> {
    let BackgroundRemoval {
        worktree_path,
        branch_name,
        deletion_mode,
        target_branch,
        force_worktree,
        changed_directory,
        planner_expected_retention,
    } = *removal;

    // Dirty-worktree gate, fsmonitor stop, then the rename into .git/wt/trash/
    // (instant on same filesystem) — the same prelude the synchronous
    // `remove_worktree_with_cleanup` runs. On the fast path the background
    // process is then just an `rm -rf`.
    if let Some(staged_path) =
        stage_worktree_removal(repo, worktree_path, branch_name, force_worktree)?
    {
        // Delete branch synchronously now that prune has removed the worktree metadata.
        // Fresh refs, not the pre-hook planning decision: hooks or concurrent
        // processes may have advanced the branch (`execute_branch_deletion`).
        let fate = if let Some(branch) = branch_name
            && !deletion_mode.should_keep()
        {
            let result = execute_branch_deletion(
                repo,
                branch,
                target_branch.unwrap_or("HEAD"),
                deletion_mode.is_force(),
            );
            warn_if_branch_retained(branch, &result, planner_expected_retention);
            BranchFate::from_result(Some(&result))
        } else {
            BranchFate::NotAttempted
        };
        if changed_directory {
            // Create an empty placeholder at the original path so the shell's working
            // directory ($env.PWD) remains valid until the wrapper has cd'd away.
            // Without this, shells that validate PWD (notably Nushell) emit errors
            // between binary exit and the cd directive executing.
            // Best-effort: if create_dir fails (permissions, race), the only effect
            // is that Nushell may still emit PWD errors — not a correctness issue.
            let _ = std::fs::create_dir(worktree_path);
        }
        Ok((
            BackgroundRemovalPlan::Detached(build_remove_command_staged(
                &staged_path,
                worktree_path,
                changed_directory,
            )),
            fate,
        ))
    } else {
        if matches!(
            fallback_mode,
            BackgroundFallbackMode::SynchronousForNonCurrent
        ) && !changed_directory
        {
            repo.remove_worktree(worktree_path, force_worktree)?;
            let fate = if let Some(branch) = branch_name
                && !deletion_mode.should_keep()
            {
                delete_branch_in_synchronous_fallback(
                    repo,
                    branch,
                    target_branch,
                    deletion_mode,
                    planner_expected_retention,
                )
            } else {
                BranchFate::NotAttempted
            };
            return Ok((BackgroundRemovalPlan::CompletedSynchronously, fate));
        }

        // Fallback: cross-filesystem, permissions, Windows file locking, etc.
        // Use legacy git worktree remove which handles these cases. For
        // safe-delete, decide here whether to append a branch-deletion step:
        // run worktrunk's full integration check now, and if the branch is
        // integrated, append an atomic `git update-ref -d <ref> <sha>` keyed
        // to the snapshotted SHA. This matches the fast path and the
        // synchronous fallback — same `BranchDeletionMode::SafeDelete` input
        // yields the same semantics (squash-merged, ancestor, patch-id-match
        // all accepted), and the CAS protects against tip movement between
        // the foreground check and the detached delete. Force-delete keeps
        // the unconditional `git branch -D` shell tail.
        let (command, fate) = match (branch_name, deletion_mode) {
            (Some(branch), BranchDeletionMode::ForceDelete) => (
                build_remove_command(
                    worktree_path,
                    Some(branch),
                    force_worktree,
                    changed_directory,
                ),
                BranchFate::Deferred,
            ),
            (Some(branch), BranchDeletionMode::SafeDelete) => {
                let cas_tail = build_cas_branch_delete_tail(repo, branch, target_branch);
                // No tail means the foreground integration check declined (or
                // couldn't run): the detached process won't touch the branch,
                // so its survival is known here, not deferred — and when the
                // planner promised deletion, the progress message already said
                // "worktree & branch", so the survival gets the same
                // correction the fast path emits. (A check that errored
                // collapses into this: it couldn't confirm integration, which
                // is the message's effective claim.)
                let fate = match cas_tail {
                    Some(_) => BranchFate::Deferred,
                    None => {
                        warn_if_branch_retained(
                            branch,
                            &Ok(BranchDeletionResult {
                                outcome: BranchDeletionOutcome::NotDeleted,
                                integration_target: target_branch.unwrap_or("HEAD").to_string(),
                            }),
                            planner_expected_retention,
                        );
                        // Same `NotDeleted` the warning was built from: the
                        // foreground check declined, so the branch survives
                        // unmerged as far as anything downstream can tell.
                        BranchFate::Retained(RetainedReason::Unmerged)
                    }
                };
                (
                    build_remove_command_with_tail(
                        worktree_path,
                        force_worktree,
                        changed_directory,
                        cas_tail.as_deref(),
                    ),
                    fate,
                )
            }
            _ => (
                build_remove_command(worktree_path, None, force_worktree, changed_directory),
                BranchFate::NotAttempted,
            ),
        };
        Ok((BackgroundRemovalPlan::Detached(command), fate))
    }
}

/// Delete the just-removed worktree's branch in the synchronous fallback path.
///
/// Only `wt step prune` reaches this (via `SynchronousForNonCurrent`). Mirrors
/// the fast path above via `execute_branch_deletion` (fresh refs + CAS), so
/// squash-merged / patch-id-matched branches that prune accepted as candidates
/// are deleted here too — a plain `git branch -d` would refuse them on
/// reachability from HEAD alone and leave the branch behind while the worktree
/// was already removed. The caller filters out `None` branches and `Keep` modes.
fn delete_branch_in_synchronous_fallback(
    repo: &Repository,
    branch: &str,
    target_branch: Option<&str>,
    deletion_mode: BranchDeletionMode,
    planner_expected_retention: bool,
) -> BranchFate {
    let result = execute_branch_deletion(
        repo,
        branch,
        target_branch.unwrap_or("HEAD"),
        deletion_mode.is_force(),
    );
    warn_if_branch_retained(branch, &result, planner_expected_retention);
    BranchFate::from_result(Some(&result))
}

/// Surface the residual branch when `delete_branch_if_safe` returned an
/// unexpected outcome the user wouldn't otherwise see.
///
/// The worktree has already been removed by the time this runs, so silently
/// dropping the branch-deletion outcome (the prior behavior) left the user
/// with no signal that the branch survived. The fast path, the synchronous
/// fallback, and the detached fallback's known-retained arm (no CAS tail to
/// append) all route through here so the message is consistent.
///
/// - `Ok(RetainedRaced)`: atomic CAS rejected the delete because the ref
///   tip moved between integration check and delete (a hook, a concurrent
///   push). Always surface — the unmerged commits would otherwise vanish
///   silently from the user's view.
/// - `Ok(RetainedCheckedOut)`: the final topology read found the branch in a
///   worktree. Always surface the path; neither the moved-ref nor unmerged
///   wording describes this outcome.
/// - `Ok(NotDeleted)`: integration check declined the branch. Warn only
///   when the planner predicted deletion (a `pre-remove` hook commit, or
///   similar race) — otherwise `print_hints` has explained the case and a
///   second message would duplicate.
/// - `Ok(ForceDeleted)` / `Ok(Integrated(_))`: succeeded; no message.
/// - `Err`: `tracing::warn!` for developer diagnostics; the failure modes
///   here (`git update-ref` exec error, refs DB I/O failure) are not
///   user-actionable beyond re-running the command.
fn warn_if_branch_retained(
    branch: &str,
    result: &anyhow::Result<BranchDeletionResult>,
    planner_expected_retention: bool,
) {
    match result {
        Ok(result) => match &result.outcome {
            BranchDeletionOutcome::RetainedCheckedOut { path } => {
                eprintln!(
                    "{}",
                    retained_checked_out_branch_message(branch, path, true)
                );
            }
            BranchDeletionOutcome::RetainedRaced => {
                // The branch tip moved between the integration check and the
                // atomic delete (a hook commit, a concurrent push). The
                // compare-and-swap refused — fail-closed — so the unmerged
                // commits are preserved. Always surface, regardless of planner
                // prediction.
                eprintln!("{}", retained_raced_branch_message(branch, true));
            }
            BranchDeletionOutcome::NotDeleted if !planner_expected_retention => {
                let cmd = suggest_command("remove", &[branch], &["-D"]);
                eprintln!(
                    "{}",
                    warning_message(cformat!(
                        "Removed worktree but kept branch <bold>{branch}</> (not integrated); to delete, run <bold>{cmd}</>"
                    ))
                );
            }
            BranchDeletionOutcome::NotDeleted
            | BranchDeletionOutcome::ForceDeleted
            | BranchDeletionOutcome::Integrated(_) => {}
        },
        Err(e) => {
            tracing::warn!(branch = %branch, error = %e, "Failed to delete branch {branch} after removing worktree: {e}");
        }
    }
}

/// Compute the safe-delete branch-deletion shell tail for the Detached
/// fallback path.
///
/// Captures a fresh snapshot, runs the same `integration_reason` check the
/// fast path uses, and — if the branch is integrated — returns a fail-closed
/// topology guard followed by atomic
/// `git update-ref -d refs/heads/<branch> <expected-sha>`. The detached process
/// re-reads `git worktree list --porcelain` after removing the original
/// worktree and skips deletion if any non-prunable worktree now holds the
/// branch; stale `prunable` records do not strand it. The tail is joined to
/// `git worktree remove` with `&&`, so the exact target's registration is
/// already gone before this guard runs; every same-branch record that remains
/// belongs to a different worktree. The CAS then protects against tip movement.
/// The detached executor provides POSIX `sh` on Unix and requires Git Bash on
/// Windows, so the record-aware `awk` guard is available on both paths.
///
/// Returns `None` when the branch is not integrated, when the snapshot
/// doesn't carry the branch SHA, or when the snapshot/integration call
/// errors — all "don't delete" outcomes that preserve the pre-CAS detached
/// behavior of skipping branch deletion.
fn build_cas_branch_delete_tail(
    repo: &Repository,
    branch: &str,
    target_branch: Option<&str>,
) -> Option<String> {
    use shell_escape::unix::escape;

    let target = target_branch.unwrap_or("HEAD");
    let snapshot = repo.capture_refs().ok()?;
    let (_effective_target, reason) = repo.integration_reason(&snapshot, branch, target).ok()?;
    reason?;
    let expected_sha = snapshot.local_branch(branch)?.commit_sha.clone();

    let ref_name = format!("refs/heads/{branch}");
    let branch_line = format!("branch {ref_name}");
    let branch_line_escaped = escape(branch_line.as_str().into());
    let ref_escaped = escape(ref_name.as_str().into());
    let sha_escaped = escape(expected_sha.as_str().into());
    Some(format!(
        "worktrunk_worktrees=$(git worktree list --porcelain) && {{ worktrunk_live_checkout=$(printf '%s\\n' \"$worktrunk_worktrees\" | awk -v worktrunk_wanted_branch={branch_line_escaped} '{LIVE_BRANCH_WORKTREE_AWK}') && {{ if [ \"$worktrunk_live_checkout\" = live ]; then :; elif [ -z \"$worktrunk_live_checkout\" ]; then git update-ref -d {ref_escaped} {sha_escaped}; else false; fi; }}; }}"
    ))
}

/// Build the detached worktree-removal command, optionally appending a
/// branch-deletion tail (force-delete or CAS-delete) with `&&` so the branch
/// step runs only when the worktree removal succeeded.
fn build_remove_command_with_tail(
    worktree_path: &Path,
    force_worktree: bool,
    changed_directory: bool,
    tail: Option<&str>,
) -> String {
    let remove_command =
        build_remove_command(worktree_path, None, force_worktree, changed_directory);
    match tail {
        Some(tail) => format!("{remove_command} && {tail}"),
        None => remove_command,
    }
}

/// List top-level entries remaining in a directory after a failed removal.
///
/// Returns None if the directory doesn't exist, can't be read, or is empty.
/// Entries are sorted, with directories suffixed with `/`.
fn list_remaining_entries(path: &Path) -> Option<Vec<String>> {
    let mut entries: Vec<String> = std::fs::read_dir(path)
        .ok()?
        .filter_map(|e| {
            let e = e.ok()?;
            let name = e.file_name().to_string_lossy().into_owned();
            if e.file_type().ok()?.is_dir() {
                Some(format!("{name}/"))
            } else {
                Some(name)
            }
        })
        .collect();
    entries.sort();
    (!entries.is_empty()).then_some(entries)
}

// ============================================================================
// Switch Output Handlers
// ============================================================================

/// Format a switch message based on what was created
///
/// # Message formats
/// - Branch + worktree created (`--create`): "Created branch X from Y and worktree @ path"
/// - Branch from remote + worktree (DWIM): "Created branch X (tracking remote) and worktree @ path"
/// - Worktree only created: "Created worktree for X @ path"
/// - Switched to existing: "Switched to worktree for X @ path"
fn format_switch_message(
    branch: &str,
    path: &Path,
    worktree_created: bool,
    created_branch: bool,
    base_branch: Option<&str>,
    from_remote: Option<&str>,
) -> String {
    let path_display = format_path_for_display(path);

    if created_branch {
        // --create flag: created branch and worktree
        match base_branch {
            Some(base) => cformat!(
                "Created branch <bold>{branch}</> from <bold>{base}</> and worktree @ <bold>{path_display}</>"
            ),
            None => {
                cformat!("Created branch <bold>{branch}</> and worktree @ <bold>{path_display}</>")
            }
        }
    } else if let Some(remote) = from_remote {
        // DWIM from remote: created local tracking branch and worktree
        cformat!(
            "Created branch <bold>{branch}</> (tracking <bold>{remote}</>) and worktree @ <bold>{path_display}</>"
        )
    } else if worktree_created {
        // Local branch existed, created worktree only
        cformat!("Created worktree for <bold>{branch}</> @ <bold>{path_display}</>")
    } else {
        // Switched to existing worktree
        cformat!("Switched to worktree for <bold>{branch}</> @ <bold>{path_display}</>")
    }
}

struct SwitchOutputContext {
    path: PathBuf,
    path_display: String,
    branch: String,
    shell_warning_reason: Option<String>,
    user_wont_be_in_worktree: bool,
    is_git_subcommand: bool,
}

fn build_switch_output_context(
    result: &SwitchResult,
    branch_info: &SwitchBranchInfo,
    change_dir: bool,
) -> SwitchOutputContext {
    let path = super::to_logical_path(result.path());
    let path_display = format_path_for_display(&path);
    let branch = branch_info
        .branch
        .clone()
        .unwrap_or_else(|| "detached worktree".to_string());

    let is_git_subcommand = crate::is_git_subcommand();
    let is_shell_integration_active = super::is_shell_integration_active();
    let shell_warning_reason = if !change_dir || is_shell_integration_active {
        None
    } else if is_git_subcommand {
        Some("ran git wt; running through git prevents cd".to_string())
    } else {
        Some(compute_shell_warning_reason())
    };
    let user_wont_be_in_worktree = !change_dir || shell_warning_reason.is_some();

    SwitchOutputContext {
        path,
        path_display,
        branch,
        shell_warning_reason,
        user_wont_be_in_worktree,
        is_git_subcommand,
    }
}

fn print_switch_directory_hint(branch: &str, is_git_subcommand: bool) {
    if is_git_subcommand {
        eprintln!("{}", hint_message(git_subcommand_warning()));
    } else if super::retired_shell_wrapper_active() {
        super::print_outdated_shell_wrapper_hint_once();
    } else if should_show_explicit_path_hint() {
        eprintln!("{}", hint_message(explicit_path_hint(branch)));
    }
}

fn handle_switch_already_at_output(ctx: &SwitchOutputContext) -> Option<PathBuf> {
    eprintln!(
        "{}",
        info_message(cformat!(
            "Already on worktree for <bold>{}</> @ <bold>{}</>",
            ctx.branch,
            ctx.path_display
        ))
    );
    None
}

fn handle_switch_existing_output(ctx: &SwitchOutputContext) -> Option<PathBuf> {
    if let Some(reason) = &ctx.shell_warning_reason {
        eprintln!(
            "{}",
            warning_message(cformat!(
                "Worktree for <bold>{}</> @ <bold>{}</>, but cannot change directory — {reason}",
                ctx.branch,
                ctx.path_display
            ))
        );
        print_switch_directory_hint(&ctx.branch, ctx.is_git_subcommand);
    } else {
        eprintln!(
            "{}",
            info_message(format_switch_message(
                &ctx.branch,
                &ctx.path,
                false, // worktree_created
                false, // created_branch
                None,
                None,
            ))
        );
    }

    ctx.user_wont_be_in_worktree.then(|| ctx.path.clone())
}

fn maybe_print_worktree_path_hint(created_branch: bool) {
    if !created_branch {
        return;
    }

    if let Ok(repo) = worktrunk::git::Repository::current() {
        let has_custom_config = UserConfig::load()
            .map(|c| {
                c.has_custom_worktree_path()
                    || repo
                        .project_identifier()
                        .ok()
                        .is_some_and(|p| c.has_project_worktree_path(&p))
            })
            .unwrap_or(false);
        if !has_custom_config && !repo.has_shown_hint("worktree-path") {
            let hint = hint_message(cformat!(
                "To customize worktree locations, run <underline>wt config create</>"
            ));
            eprintln!("{}", hint);
            let _ = repo.mark_hint_shown("worktree-path");
        }
    }
}

fn handle_switch_created_output(
    ctx: &SwitchOutputContext,
    created_branch: bool,
    base_branch: Option<&str>,
    from_remote: Option<&str>,
) -> Option<PathBuf> {
    eprintln!(
        "{}",
        success_message(format_switch_message(
            &ctx.branch,
            &ctx.path,
            true, // worktree_created
            created_branch,
            base_branch,
            from_remote,
        ))
    );

    maybe_print_worktree_path_hint(created_branch);

    if let Some(reason) = &ctx.shell_warning_reason {
        eprintln!(
            "{}",
            warning_message(cformat!("Cannot change directory — {reason}"))
        );
        print_switch_directory_hint(&ctx.branch, ctx.is_git_subcommand);
    }

    ctx.user_wont_be_in_worktree.then(|| ctx.path.clone())
}

struct BranchDeletionDisplay {
    result: BranchDeletionResult,
    show_unmerged_hint: bool,
}

/// The canonical "branch retained because unmerged" info + hint lines, as an
/// `(info, hint)` pair. [`print_retained_unmerged_branch`] prints them; the
/// picker stashes them via `stash_retained_unmerged_branch` (it can't print
/// mid-render) from its branch-only keep path — a `/ branch` row whose unmerged
/// branch `SafeDelete` declines to delete stays put, so this explains the no-op.
/// (A worktree removal that keeps its branch transforms the row to `/ branch`
/// live instead, with no stashed message.) Shared so the emit paths can't drift
/// in wording, flag, or styling.
pub(crate) fn retained_unmerged_branch_messages(branch_name: &str) -> (String, String) {
    let info = info_message(cformat!(
        "Branch <bold>{branch_name}</> retained; has unmerged changes"
    ))
    .to_string();
    let cmd = suggest_command("remove", &[branch_name], &["-D"]);
    let hint = hint_message(cformat!(
        "To delete the unmerged branch, run <underline>{cmd}</>"
    ))
    .to_string();
    (info, hint)
}

fn print_retained_unmerged_branch(branch_name: &str) {
    let (info, hint) = retained_unmerged_branch_messages(branch_name);
    eprintln!("{info}");
    eprintln!("{hint}");
}

/// Explain that the branch was kept because another worktree still has it
/// checked out. Removing this worktree can't free a ref that's live elsewhere,
/// so the branch is retained and the surviving checkout is named — the state is
/// only reachable through `git worktree add --force`, so the user may not
/// remember creating it.
///
/// Info by default: nothing went wrong, and this reads as a sibling of
/// [`retained_unmerged_branch_messages`], the other "kept the branch, here's
/// why" line. A refused `-D` is different — `-D` overrides every other
/// retention wt has, so one that doesn't delete is unexpected and warns.
fn print_branch_checked_out_elsewhere(branch_name: &str, shared: &SharedBranchCheckout) {
    let path = format_path_for_display(&shared.path);
    let message = if shared.refused_force_delete {
        warning_message(cformat!(
            "Branch <bold>{branch_name}</> retained despite <bold>-D</>; still checked out @ <bold>{path}</>"
        ))
        .to_string()
    } else {
        info_message(cformat!(
            "Branch <bold>{branch_name}</> retained; still checked out @ <bold>{path}</>"
        ))
        .to_string()
    };
    eprintln!("{message}");
}

/// The canonical "branch retained because the atomic CAS delete was refused"
/// warning. The ref moved between the integration check and the delete (a hook
/// commit, a concurrent push), so the integrated branch is kept fail-closed
/// rather than dropping the new commits. Shared across the worktree-removal and
/// branch-only emit paths so the wording, flag, and styling can't drift.
///
/// `removed_worktree` selects the lead-in: the worktree has already been removed
/// on the worktree-removal paths, but the branch-only path never had one.
fn retained_raced_branch_message(branch_name: &str, removed_worktree: bool) -> String {
    let lead_in = if removed_worktree {
        "Removed worktree but kept branch"
    } else {
        "Kept branch"
    };
    let cmd = suggest_command("remove", &[branch_name], &["-D"]);
    warning_message(cformat!(
        "{lead_in} <bold>{branch_name}</> (moved during deletion); inspect commits, then run <bold>{cmd}</> if safe"
    ))
    .to_string()
}

/// The canonical message for a branch retained by the final topology read.
///
/// Unlike [`retained_raced_branch_message`], this is not a ref-tip race and
/// does not suggest `-D`: Git refuses to delete a branch while the named
/// worktree holds it. The path is the actionable explanation.
fn retained_checked_out_branch_message(
    branch_name: &str,
    path: &Path,
    removed_worktree: bool,
) -> String {
    let lead_in = if removed_worktree {
        "Removed worktree but retained branch"
    } else {
        "Retained branch"
    };
    let path = format_path_for_display(path);
    warning_message(cformat!(
        "{lead_in} <bold>{branch_name}</>; checked out @ <bold>{path}</>"
    ))
    .to_string()
}

/// Handle the result of a branch deletion attempt.
///
/// Converts a deletion attempt into structured display data:
/// - `NotDeleted`: We checked and chose not to delete (not integrated) — sets
///   `show_unmerged_hint`.
/// - `RetainedCheckedOut`: the final fresh topology read found a checkout.
///   Callers surface its path, not the unmerged hint.
/// - `RetainedRaced`: integration check passed but the atomic CAS delete was
///   refused because the ref moved (a hook or concurrent process advanced it).
///   Callers surface this with [`retained_raced_branch_message`], not the
///   unmerged hint, so it is *not* folded into `show_unmerged_hint`.
/// - `Err(e)`: Git command failed - show warning with actual error
fn handle_branch_deletion_result(
    result: anyhow::Result<BranchDeletionResult>,
    branch_name: &str,
) -> anyhow::Result<BranchDeletionDisplay> {
    match result {
        Ok(result) => Ok(BranchDeletionDisplay {
            show_unmerged_hint: matches!(result.outcome, BranchDeletionOutcome::NotDeleted),
            result,
        }),
        Err(e) => {
            // Git command failed - this is an error (we decided to delete but couldn't)
            eprintln!(
                "{}",
                error_message(cformat!("Failed to delete branch <bold>{branch_name}</>"))
            );
            eprintln!("{}", format_with_gutter(&e.display_message(), None));
            Err(e)
        }
    }
}

struct FlagNote {
    text: String,
    symbol: Option<String>,
    suffix: String,
}

impl FlagNote {
    fn empty() -> Self {
        Self {
            text: String::new(),
            symbol: None,
            suffix: String::new(),
        }
    }

    fn text_only(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            symbol: None,
            suffix: String::new(),
        }
    }

    fn with_symbol(
        text: impl Into<String>,
        symbol: impl Into<String>,
        suffix: impl Into<String>,
    ) -> Self {
        Self {
            text: text.into(),
            symbol: Some(symbol.into()),
            suffix: suffix.into(),
        }
    }

    fn after(&self, color: AnsiColor) -> String {
        match &self.symbol {
            Some(s) => match color {
                AnsiColor::Cyan => cformat!("{}<cyan>{}</>", s, self.suffix),
                AnsiColor::Green => cformat!("{}<green>{}</>", s, self.suffix),
                _ => format!("{s}{}", self.suffix),
            },
            None => String::new(),
        }
    }
}

/// Get flag acknowledgment note for remove messages
///
/// `target_branch`: The branch we checked integration against (shown in reason)
fn flag_note(
    deletion_mode: BranchDeletionMode,
    outcome: &BranchDeletionOutcome,
    target_branch: Option<&str>,
) -> FlagNote {
    if deletion_mode.should_keep() {
        return FlagNote::text_only(" (--no-delete-branch)");
    }

    match outcome {
        BranchDeletionOutcome::NotDeleted
        | BranchDeletionOutcome::RetainedCheckedOut { .. }
        | BranchDeletionOutcome::RetainedRaced => FlagNote::empty(),
        BranchDeletionOutcome::ForceDeleted => FlagNote::text_only(" (--force-delete)"),
        BranchDeletionOutcome::Integrated(reason) => {
            let Some(target) = target_branch else {
                return FlagNote::empty();
            };
            let symbol = reason.symbol();
            let desc = reason.description();
            FlagNote::with_symbol(
                cformat!(" ({desc} <bold>{target}</>,"),
                cformat!(" <dim>{symbol}</>"),
                ")",
            )
        }
    }
}

/// Show switch message when changing directory after worktree removal.
///
/// When shell integration is not active, warns that cd cannot happen.
/// This is important for remove/merge since the user would be left in a deleted directory.
///
/// # Warning Message Format
///
/// Uses the standard "Cannot change directory — {reason}" pattern.
/// See [`compute_shell_warning_reason`] for the full list of reasons.
fn print_switch_message_if_changed(
    changed_directory: bool,
    main_path: &Path,
) -> anyhow::Result<()> {
    if !changed_directory {
        return Ok(());
    }

    // Use main_path for discovery - the worktree we came from may have been removed
    let Ok(repo) = Repository::at(main_path) else {
        return Ok(());
    };
    let Ok(Some(dest_branch)) = repo.worktree_at(main_path).branch() else {
        return Ok(());
    };

    let logical_path = super::to_logical_path(main_path);
    let path_display = format_path_for_display(&logical_path);

    if super::is_shell_integration_active() {
        // Shell integration active - cd will work
        eprintln!(
            "{}",
            info_message(cformat!(
                "Switched to worktree for <bold>{dest_branch}</> @ <bold>{path_display}</>"
            ))
        );
    } else if crate::is_git_subcommand() {
        // Running as `git wt` - explain why cd can't work
        eprintln!(
            "{}",
            warning_message(
                "Cannot change directory — ran git wt; running through git prevents cd",
            )
        );
        eprintln!("{}", hint_message(git_subcommand_warning()));
    } else {
        // Shell integration not active - compute specific reason
        let reason = compute_shell_warning_reason();
        eprintln!(
            "{}",
            warning_message(cformat!("Cannot change directory — {reason}"))
        );
        // Show appropriate hint based on invocation mode
        if super::retired_shell_wrapper_active() {
            super::print_outdated_shell_wrapper_hint_once();
        } else if should_show_explicit_path_hint() {
            eprintln!("{}", hint_message(explicit_path_hint(&dest_branch)));
        } else {
            print_shell_integration_hint(&repo);
        }
    }
    Ok(())
}

/// Compute the target directory for `cd` when moving the shell between
/// worktrees, preserving the user's subdirectory position when possible.
///
/// If the user is in `source_root/apps/gateway/` and `target_root/apps/gateway/`
/// exists, returns `target_root/apps/gateway/`. Otherwise returns `target_root`.
///
/// Shared by every command that relocates the shell — `switch`, `remove` (and
/// `merge`, which lands via the same handler), and `step relocate` — so they
/// preserve subdirectory position identically (canonicalizing to survive
/// symlinks, and falling back to the root when the subdir is absent).
pub(crate) fn resolve_subdir_in_target(
    target_root: &Path,
    source_root: Option<&Path>,
    cwd: &Path,
) -> PathBuf {
    if let Some(source_root) = source_root {
        // Canonicalize both paths to handle symlinks (e.g., /var -> /private/var
        // on macOS). Through `canonicalize_with_parents` rather than `dunce`
        // directly, so a cwd past Windows' 260-character limit is spelled like
        // the short worktree root containing it — otherwise `strip_prefix` below
        // fails and the shell silently lands at the target root (#3898).
        let cwd = canonicalize_with_parents(cwd);
        let source_root = canonicalize_with_parents(source_root);
        if let Ok(relative) = cwd.strip_prefix(&source_root)
            && !relative.as_os_str().is_empty()
        {
            let candidate = target_root.join(relative);
            if candidate.is_dir() {
                return candidate;
            }
        }
    }
    target_root.to_path_buf()
}

/// Handle output for a switch operation
///
/// # Shell Integration Warnings
///
/// Always warn when the shell's directory won't change. Users expect to be in
/// the target worktree after switching.
///
/// **When to warn:** Shell integration is not active (no directive files set).
/// This applies to both `Existing` and `Created` results.
///
/// **When NOT to warn:**
/// - `AlreadyAt` — user is already in the target directory
/// - Shell integration IS active — cd will happen automatically
///
/// **Warning format:** `Cannot change directory — {reason}`
///
/// See [`compute_shell_warning_reason`] for the full list of reasons.
///
/// **Message order for Created:** Success message first, then warning. Creation
/// is a real accomplishment, but users still need to know they won't cd there.
///
/// # Arguments
///
/// * `change_dir` — When false, skip the directory change (user requested `--no-cd`)
///
/// # Return Value
///
/// Returns `Some(path)` when post-switch hooks should show "@ path" in their
/// announcements (because the user's shell won't be in that directory). This happens when:
/// - Shell integration is not active (user's shell stays in original directory)
/// - `change_dir` is false (user explicitly requested no directory change)
///
/// Returns `None` when the user will be in the worktree directory (shell integration
/// active or already at the worktree), so no path annotation needed.
pub fn handle_switch_output(
    result: &SwitchResult,
    branch_info: &SwitchBranchInfo,
    change_dir: bool,
    source_worktree_root: Option<&Path>,
    cwd: &Path,
) -> anyhow::Result<Option<std::path::PathBuf>> {
    // Set target directory for command execution, preserving subdirectory position.
    // If the user is in apps/gateway/ in the source worktree and that directory exists
    // in the target, cd to apps/gateway/ in the target instead of the root.
    if change_dir {
        let cd_target = resolve_subdir_in_target(result.path(), source_worktree_root, cwd);
        super::change_directory(&cd_target)?;
    }

    // Translate to the user's logical (symlink-preserved) path for display messages.
    // The cd directive (above) handles its own translation internally.
    let ctx = build_switch_output_context(result, branch_info, change_dir);

    let display_path_for_hooks = match result {
        SwitchResult::AlreadyAt(_) => handle_switch_already_at_output(&ctx),
        SwitchResult::Existing { .. } => handle_switch_existing_output(&ctx),
        SwitchResult::Created {
            created_branch,
            base_branch,
            from_remote,
            ..
        } => handle_switch_created_output(
            &ctx,
            *created_branch,
            base_branch.as_deref(),
            from_remote.as_deref(),
        ),
    };

    stderr().flush()?;
    Ok(display_path_for_hooks)
}

/// Execute the --execute command after hooks have run.
///
/// `display_path` is shown when the user's shell won't be in the worktree
/// directory (shell integration not active). This helps users understand where
/// the command runs.
///
pub fn execute_user_command(argv: &[String], display_path: Option<&Path>) -> anyhow::Result<()> {
    super::global::print_outdated_execute_wrapper_warning();
    let command = super::global::format_exec_argv(argv);

    // Show what command is being executed (section header + gutter content)
    // Include path when user's shell won't be there (shell integration not active)
    let header = match display_path {
        Some(path) => {
            let path_display = format_path_for_display(path);
            cformat!("Executing (--execute) @ <bold>{path_display}</>:")
        }
        None => "Executing (--execute):".to_string(),
    };
    eprintln!("{}", progress_message(header));
    eprintln!("{}", format_bash_with_gutter(&command));

    super::execute(argv.to_vec())?;

    Ok(())
}

/// Execute a [`RemovalPlan`] and narrate it per `execution`.
///
/// Returns the branch's [`BranchFate`] so callers report what happened rather
/// than what the plan intended — the prune summary and `--format=json` both
/// read it. Worktree-removal failures propagate as `Err`, and for a
/// `Worktree` plan a surviving branch is a fate, not an error — the removal
/// was the primary operation. For a `BranchOnly` plan the deletion *is* the
/// operation, so a hard command failure (not a declined or raced deletion)
/// still propagates as `Err`.
///
/// Approval is handled at the gate (command entry point), not here. The
/// `announcer`'s `show_branch` setting (set by the caller) controls whether
/// hook announce lines include the branch name for batch-context disambiguation.
/// When `quiet` is true (prune context), suppresses informational messages
/// like "No worktree found for branch X" that are noise in batch operations.
///
/// `hook_plan` is the frozen, approved hook set; an empty plan (`--no-hooks`,
/// declined, or no project config) runs no project hooks. `pre-remove` /
/// `post-remove` / `post-switch` execute only from it — the selection was
/// frozen at the gate, never re-read.
///
/// [`RemovalExecution::Silent`] (the TUI picker — this runs while skim owns
/// the terminal) removes a `Worktree` plan inline with no progress/success
/// messages, no trash-cleanup spinner, and no `cd` directive: just the git
/// removal plus the planned hooks. (The picker can't prompt mid-render, so its
/// `hook_plan` comes from a read-only `Approvals` filter — see `do_removal`.)
///
/// The [`BackgroundFallbackMode`] inside [`RemovalExecution::Background`]
/// selects how a removal whose rename-into-trash fast path fails behaves:
/// every caller but `wt step prune` passes
/// [`BackgroundFallbackMode::Detached`] (spawn the legacy `git worktree
/// remove`); prune passes [`BackgroundFallbackMode::SynchronousForNonCurrent`]
/// so a candidate's removal is complete by the time prune reports it removed.
pub fn handle_remove_output(
    plan: &RemovalPlan,
    execution: RemovalExecution,
    hook_plan: &ApprovedHookPlan,
    quiet: bool,
    announcer: &mut HookAnnouncer<'_>,
) -> anyhow::Result<BranchFate> {
    match plan {
        RemovalPlan::Worktree {
            main_path,
            worktree_path,
            changed_directory,
            branch_name,
            deletion_mode,
            target_branch,
            integration_reason,
            force_worktree,
            removed_commit,
            branch_checked_out_at,
        } => handle_removed_worktree_output(
            WorktreeRemovalContext {
                main_path,
                worktree_path,
                changed_directory: *changed_directory,
                branch_name: branch_name.as_deref(),
                deletion_mode: *deletion_mode,
                target_branch: target_branch.as_deref(),
                integration_reason: *integration_reason,
                force_worktree: *force_worktree,
                removed_commit: removed_commit.as_deref(),
                branch_checked_out_at: branch_checked_out_at.as_ref(),
                hook_plan,
                execution,
            },
            announcer,
        ),
        RemovalPlan::BranchOnly {
            branch_name,
            deletion_mode,
            prune_entry,
            target_branch,
            integration_reason,
            branch_checked_out_at,
        } => handle_branch_only_output(
            branch_name,
            *deletion_mode,
            prune_entry.as_deref(),
            *integration_reason,
            target_branch.as_deref(),
            branch_checked_out_at.as_ref(),
            quiet,
        ),
    }
}

/// Handle output for BranchOnly removal (branch exists but no worktree)
///
/// `prune_entry` is the stale worktree entry the plan fell back from, if any;
/// it is unregistered here, first — unconditionally, unlike the branch
/// deletion the `should_keep`/CAS logic below may decline.
///
/// When `quiet` is true, suppresses the "No worktree found for branch X"
/// info line for non-pruned cases (noise in prune/batch context).
fn handle_branch_only_output(
    branch_name: &str,
    deletion_mode: BranchDeletionMode,
    prune_entry: Option<&Path>,
    integration_reason: Option<IntegrationReason>,
    target_branch: Option<&str>,
    branch_checked_out_at: Option<&SharedBranchCheckout>,
    quiet: bool,
) -> anyhow::Result<BranchFate> {
    let pruned = if let Some(path) = prune_entry {
        Repository::current()?.prune_worktree_entry(path)?;
        true
    } else {
        false
    };
    let branch_info = if pruned {
        cformat!("Worktree directory missing for <bold>{branch_name}</>; pruned")
    } else {
        cformat!("No worktree found for branch <bold>{branch_name}</>")
    };

    // If we won't delete the branch, show info and return early
    if deletion_mode.should_keep() {
        eprintln!("{}", info_message(&branch_info));
        // A sibling `--force` checkout kept the branch alive; name it so the
        // user knows why the pruned branch survived rather than being deleted.
        if let Some(shared) = branch_checked_out_at {
            print_branch_checked_out_elsewhere(branch_name, shared);
        }
        stderr().flush()?;
        return Ok(BranchFate::NotAttempted);
    }

    let check_target = target_branch.unwrap_or("HEAD");

    // Force-delete bypasses CAS entirely — `git branch -D` is the user's
    // explicit override. For safe-delete with a pre-computed integration
    // reason, re-run the deletion against fresh refs
    // (`execute_branch_deletion`): the atomic CAS catches any tip movement
    // since planning, rather than trusting the planner's stale view of the ref.
    let deletion = if let Some(integrated_reason) = integration_reason
        && !deletion_mode.is_force()
    {
        let repo = worktrunk::git::Repository::current()?;
        let result =
            execute_branch_deletion(&repo, branch_name, check_target, false).map(|mut r| {
                // Preserve the planner's recorded reason so a CAS-accepted
                // delete still reports the original "integrated because X"
                // explanation. The fresh integration target stays as the
                // helper computed it.
                if matches!(r.outcome, BranchDeletionOutcome::Integrated(_)) {
                    r.outcome = BranchDeletionOutcome::Integrated(integrated_reason);
                }
                r
            });
        handle_branch_deletion_result(result, branch_name)?
    } else if deletion_mode.is_force() {
        let repo = worktrunk::git::Repository::current()?;
        let result = repo.run_command(&["branch", "-D", "--", branch_name]);
        handle_branch_deletion_result(
            result.map(|_| BranchDeletionResult {
                outcome: BranchDeletionOutcome::ForceDeleted,
                integration_target: check_target.to_string(),
            }),
            branch_name,
        )?
    } else {
        BranchDeletionDisplay {
            result: BranchDeletionResult {
                outcome: BranchDeletionOutcome::NotDeleted,
                integration_target: check_target.to_string(),
            },
            show_unmerged_hint: true,
        }
    };

    let retained = match &deletion.result.outcome {
        BranchDeletionOutcome::RetainedCheckedOut { path } => {
            eprintln!("{}", info_message(&branch_info));
            eprintln!(
                "{}",
                retained_checked_out_branch_message(branch_name, path, false)
            );
            true
        }
        BranchDeletionOutcome::RetainedRaced => {
            eprintln!("{}", info_message(&branch_info));
            eprintln!("{}", retained_raced_branch_message(branch_name, false));
            true
        }
        BranchDeletionOutcome::NotDeleted => {
            eprintln!("{}", info_message(&branch_info));
            if deletion.show_unmerged_hint {
                print_retained_unmerged_branch(branch_name);
            }
            true
        }
        BranchDeletionOutcome::Integrated(_) | BranchDeletionOutcome::ForceDeleted => false,
    };

    if !retained {
        let flag_note = flag_note(
            deletion_mode,
            &deletion.result.outcome,
            Some(&deletion.result.integration_target),
        );
        let flag_text = &flag_note.text;
        let flag_after = flag_note.after(AnsiColor::Green);

        if pruned {
            // Combined: pruned stale metadata & deleted branch in one line
            eprintln!(
                "{}",
                FormattedMessage::new(cformat!(
                    "<green>✓ Pruned stale worktree & removed branch <bold>{branch_name}</>{flag_text}</>{flag_after}"
                ))
            );
        } else {
            if !quiet {
                eprintln!("{}", info_message(&branch_info));
            }
            eprintln!(
                "{}",
                FormattedMessage::new(cformat!(
                    "<green>✓ Removed branch <bold>{branch_name}</>{flag_text}</>{flag_after}"
                ))
            );
        }
    }

    stderr().flush()?;
    Ok(BranchFate::from_outcome(&deletion.result.outcome))
}

/// Register post-remove and post-switch hooks after worktree removal onto the
/// caller's announcer.
///
/// Pipelines come from two contexts: post-remove uses the removed worktree's
/// identity (branch, path, commit), while post-switch (only when
/// `changed_directory` is true) uses the destination worktree's branch. Both
/// types share whatever announce line the caller's announcer eventually
/// flushes — multi-phase callers (e.g. `wt merge`) batch with later phases,
/// standalone callers (e.g. `wt remove`) flush immediately after.
///
/// Only runs if `ctx.verify` is true (hooks approved).
fn spawn_hooks_after_remove(
    repo: &Repository,
    ctx: &WorktreeRemovalContext<'_>,
    removed_branch: &str,
    announcer: &mut HookAnnouncer<'_>,
) -> anyhow::Result<()> {
    let Ok(config) = UserConfig::load() else {
        return Ok(());
    };

    // When removing the current worktree, user cd's to main_path → use post_hook logic
    // (suppresses path if shell integration will cd there).
    // When removing a different worktree, user stays at cwd → use pre_hook logic
    // (shows path if main_path differs from cwd).
    let display_path = if ctx.changed_directory {
        super::post_hook_display_path(ctx.main_path)
    } else {
        super::pre_hook_display_path(ctx.main_path)
    };

    // Build post-remove template variables from the removed worktree identity.
    let remove_vars =
        PostRemoveContext::new(ctx.worktree_path, ctx.removed_commit, ctx.main_path, repo);
    let extra_vars = remove_vars.extra_vars(removed_branch);

    // All hooks use remove_ctx for spawning: log files are named after the removed
    // branch since both post-remove and post-switch are consequences of that removal.
    let remove_ctx = CommandContext::new(repo, &config, Some(removed_branch), ctx.main_path, false);

    // `post-remove` is *about* the removed worktree (gone by now); it was
    // selected and frozen into `hook_plan` at the gate, anchored at the removed
    // worktree path. `remove_ctx` (rooted at `ctx.main_path`) only renders.
    register_planned(
        announcer,
        ctx.hook_plan,
        ctx.worktree_path,
        &remove_ctx,
        worktrunk::HookType::PostRemove,
        &extra_vars,
        display_path,
    )?;

    // Post-switch: only when the user actually changed directory. Anchored at
    // the destination worktree (where the user landed) at the gate.
    if ctx.changed_directory {
        let dest_branch = repo.worktree_at(ctx.main_path).branch()?;
        let switch_ctx =
            CommandContext::new(repo, &config, dest_branch.as_deref(), ctx.main_path, false);
        register_planned(
            announcer,
            ctx.hook_plan,
            ctx.main_path,
            &switch_ctx,
            worktrunk::HookType::PostSwitch,
            &[],
            display_path,
        )?;
    }

    Ok(())
}

// ============================================================================
// Removal Display Info: Shared data for background/foreground output
// ============================================================================

/// Information needed to display removal messages and hints.
///
/// This struct captures the outcome of a branch deletion decision (freshly
/// computed for background mode or actual for foreground mode) so that message
/// formatting can be shared between both modes.
struct RemovalDisplayInfo {
    /// The observed deletion outcome, including retained race cases.
    outcome: BranchDeletionOutcome,
    /// The target branch used for integration check (may be upstream if ahead of local)
    integration_target: Option<String>,
    /// Whether the branch was integrated (used for hints when branch is kept)
    branch_was_integrated: bool,
    /// Whether to show the "unmerged, run -D" hint (foreground only, based on actual deletion)
    show_unmerged_hint: bool,
    /// Whether --force was used (for display purposes)
    force_worktree: bool,
}

impl RemovalDisplayInfo {
    /// Build display info from the refreshed integration check (background mode).
    ///
    /// The caller refreshes this after `pre-remove` hooks and immediately
    /// before staging/removing the worktree.
    fn from_precomputed(
        deletion_mode: BranchDeletionMode,
        pre_computed_integration: Option<IntegrationReason>,
        target_branch: Option<&str>,
        force_worktree: bool,
    ) -> Self {
        let (outcome, integration_target) = if deletion_mode.should_keep() {
            (
                BranchDeletionOutcome::NotDeleted,
                target_branch.map(String::from),
            )
        } else if deletion_mode.is_force() {
            (
                BranchDeletionOutcome::ForceDeleted,
                target_branch.map(String::from),
            )
        } else {
            let outcome = match pre_computed_integration {
                Some(r) => BranchDeletionOutcome::Integrated(r),
                None => BranchDeletionOutcome::NotDeleted,
            };
            (outcome, target_branch.map(String::from))
        };

        Self {
            outcome,
            integration_target,
            branch_was_integrated: pre_computed_integration.is_some(),
            show_unmerged_hint: false, // Background mode never shows this hint
            force_worktree,
        }
    }

    /// Build display info from actual deletion result (foreground mode).
    fn from_branch_result(
        branch_deletion: Option<anyhow::Result<BranchDeletionResult>>,
        branch_name: &str,
        pre_computed_integration: Option<IntegrationReason>,
        target_branch: Option<&str>,
        force_worktree: bool,
    ) -> anyhow::Result<Self> {
        let branch_was_integrated = pre_computed_integration.is_some();

        let (outcome, integration_target, show_unmerged_hint) = match branch_deletion {
            Some(result) => {
                let deletion = handle_branch_deletion_result(result, branch_name)?;
                // Only use integration_target for display if we had a real target (not "HEAD" fallback)
                let display_target =
                    target_branch.map(|_| deletion.result.integration_target.clone());
                (
                    deletion.result.outcome,
                    display_target,
                    deletion.show_unmerged_hint,
                )
            }
            None => (
                BranchDeletionOutcome::NotDeleted,
                target_branch.map(String::from),
                false,
            ),
        };

        Ok(Self {
            outcome,
            integration_target,
            branch_was_integrated,
            show_unmerged_hint,
            force_worktree,
        })
    }

    /// Whether the branch will be/was deleted.
    fn branch_deleted(&self) -> bool {
        matches!(
            self.outcome,
            BranchDeletionOutcome::ForceDeleted | BranchDeletionOutcome::Integrated(_)
        )
    }

    /// Print the removal message (progress for background, success for foreground).
    ///
    /// `stats` carries the trash-cleanup file/byte counts surfaced by the
    /// foreground spinner (see `remove_dir_with_progress`). Pass `None` for
    /// the background path — those don't run a spinner and shouldn't show
    /// stats. The `(N files · X MiB)` suffix is gray, matching the
    /// "stats parentheses" convention in the user-output skill.
    fn print_message(
        &self,
        branch_name: &str,
        foreground: bool,
        stats: Option<(usize, u64)>,
    ) -> anyhow::Result<()> {
        let flag_note = flag_note(
            if self.branch_deleted() {
                BranchDeletionMode::SafeDelete // Doesn't matter, outcome already determined
            } else {
                BranchDeletionMode::Keep
            },
            &self.outcome,
            self.integration_target.as_deref(),
        );
        let force_text = if self.force_worktree {
            " (--force)"
        } else {
            ""
        };
        let stats_paren = stats
            .map(|(f, b)| format_stats_paren(f, b))
            .unwrap_or_default();

        let msg = if foreground {
            if self.branch_deleted() {
                let flag_text = &flag_note.text;
                success_message(cformat!(
                    "Removed <bold>{branch_name}</> worktree{force_text} & branch{flag_text}"
                ))
                .append(&flag_note.after(AnsiColor::Green))
                .append(&stats_paren)
            } else {
                success_message(cformat!(
                    "Removed <bold>{branch_name}</> worktree{force_text}"
                ))
                .append(&stats_paren)
            }
        } else if self.branch_deleted() {
            let flag_text = &flag_note.text;
            progress_message(cformat!(
                "Removing <bold>{branch_name}</> worktree{force_text} & branch in background{flag_text}"
            ))
            .append(&flag_note.after(AnsiColor::Cyan))
        } else {
            progress_message(cformat!(
                "Removing <bold>{branch_name}</> worktree{force_text} in background"
            ))
        };
        eprintln!("{msg}");
        Ok(())
    }

    /// Print hints about branch status (why it was kept, how to force delete).
    fn print_hints(
        &self,
        branch_name: &str,
        deletion_mode: BranchDeletionMode,
        pre_computed_integration: Option<IntegrationReason>,
    ) -> anyhow::Result<()> {
        if self.branch_deleted() {
            return Ok(());
        }

        // A raced retention isn't "unmerged" — the branch was integrated but
        // either gained a checkout or its tip moved during the delete. Surface
        // the dedicated message rather than falling through to the generic
        // unmerged hint, whose explanation would be false.
        match &self.outcome {
            BranchDeletionOutcome::RetainedCheckedOut { path } => {
                eprintln!(
                    "{}",
                    retained_checked_out_branch_message(branch_name, path, true)
                );
                return Ok(());
            }
            BranchDeletionOutcome::RetainedRaced => {
                eprintln!("{}", retained_raced_branch_message(branch_name, true));
                return Ok(());
            }
            BranchDeletionOutcome::NotDeleted
            | BranchDeletionOutcome::ForceDeleted
            | BranchDeletionOutcome::Integrated(_) => {}
        }

        if deletion_mode.should_keep() {
            if let Some(reason) = pre_computed_integration.as_ref() {
                // User kept an integrated branch - show integration info
                let target = self.integration_target.as_deref().unwrap_or("target");
                let desc = reason.description();
                let symbol = reason.symbol();
                eprintln!(
                    "{}",
                    hint_message(cformat!(
                        "Branch integrated ({desc} <underline>{target}</>, <dim>{symbol}</>); retained with <underline>--no-delete-branch</>"
                    ))
                );
            }
        } else if self.show_unmerged_hint
            || (!deletion_mode.should_keep() && !self.branch_was_integrated)
        {
            // Unmerged, no flag - show how to force delete
            // (Background: !should_keep && !integrated, Foreground: show_unmerged_hint)
            let cmd = suggest_command("remove", &[branch_name], &["-D"]);
            eprintln!(
                "{}",
                hint_message(cformat!(
                    "Branch unmerged; to delete, run <underline>{cmd}</>"
                ))
            );
        }
        // else: Unmerged + flag - no hint (flag had no effect)

        Ok(())
    }
}

// ============================================================================

struct WorktreeRemovalContext<'a> {
    main_path: &'a Path,
    worktree_path: &'a Path,
    changed_directory: bool,
    branch_name: Option<&'a str>,
    deletion_mode: BranchDeletionMode,
    target_branch: Option<&'a str>,
    /// Planning-time integration verdict (display and retention prediction
    /// only — the deletion re-decides via `delete_branch_if_safe`'s CAS).
    integration_reason: Option<IntegrationReason>,
    force_worktree: bool,
    removed_commit: Option<&'a str>,
    /// A surviving checkout of this branch, when one exists. `Some` means the
    /// branch was retained rather than deleted; see [`SharedBranchCheckout`].
    branch_checked_out_at: Option<&'a SharedBranchCheckout>,
    /// The frozen, approved hook plan. `pre-remove` / `post-remove` /
    /// `post-switch` execute only from this — no `.config/wt.toml` re-read,
    /// no `ProjectConfig` snapshot to thread.
    hook_plan: &'a ApprovedHookPlan,
    execution: RemovalExecution,
}

impl WorktreeRemovalContext<'_> {
    /// The fallback mode for a background removal. Only the background paths
    /// consult this; they are reached only via [`RemovalExecution::Background`],
    /// so the other arms default to the standard detached fallback rather than
    /// carrying an unreachable panic in the removal path.
    fn background_fallback(&self) -> BackgroundFallbackMode {
        match self.execution {
            RemovalExecution::Background(mode) => mode,
            RemovalExecution::Foreground | RemovalExecution::Silent => {
                BackgroundFallbackMode::Detached
            }
        }
    }
}

fn execute_pre_remove_hooks_if_needed(
    repo: &Repository,
    ctx: &WorktreeRemovalContext<'_>,
) -> anyhow::Result<()> {
    let Ok(config) = UserConfig::load() else {
        return Ok(());
    };

    // `pre-remove` runs in the worktree being removed (still on disk here).
    // `pre_remove_repo` roots the *render* context there for template vars;
    // the command set is the frozen `hook_plan` selected at the gate, so no
    // `.config/wt.toml` is re-read here.
    let pre_remove_repo = Repository::at(ctx.worktree_path)?;
    let command_ctx = CommandContext::new(
        &pre_remove_repo,
        &config,
        ctx.branch_name,
        ctx.worktree_path,
        false, // yes=false for CommandContext (not approval-related)
    );
    let display_path = if ctx.changed_directory {
        None
    } else {
        Some(ctx.worktree_path)
    };
    let target_branch = repo
        .worktree_at(ctx.main_path)
        .branch()
        .ok()
        .flatten()
        .unwrap_or_default();
    let target_path_str = worktrunk::path::to_posix_path(&ctx.main_path.to_string_lossy());
    let extra_vars: Vec<(&str, &str)> = vec![
        ("target", &target_branch),
        ("target_worktree_path", &target_path_str),
    ];

    execute_planned_hook(
        ctx.hook_plan,
        ctx.worktree_path,
        &command_ctx,
        worktrunk::HookType::PreRemove,
        &extra_vars,
        FailureStrategy::FailFast,
        display_path,
    )
}

fn prepare_remove_directory_change(
    main_path: &Path,
    worktree_path: &Path,
    changed_directory: bool,
) -> anyhow::Result<()> {
    if changed_directory {
        // Preserve the user's subdirectory position, mirroring `wt switch`
        // (#3343). The removal hasn't run yet, so the current directory still
        // exists inside the worktree being removed — if the user is in
        // `worktree/apps/gateway/` and `main/apps/gateway/` exists, cd there
        // instead of the main worktree root. Falls back to the root when the
        // subdir is absent in the destination or the cwd can't be read.
        // `shell_cwd()` rather than the process cwd, so the preservation
        // survives a `wt remove` nested in an alias or hook body, whose
        // process cwd is the worktree root (#3723).
        let cd_target = shell_cwd()
            .map(|cwd| resolve_subdir_in_target(main_path, Some(worktree_path), &cwd))
            .unwrap_or_else(|| main_path.to_path_buf());
        super::change_directory(&cd_target)?;
        stderr().flush()?; // Force flush to ensure shell processes the cd
        // Mark that the CWD worktree is being removed, so the error handler
        // can show a hint if a subsequent command (e.g., post-merge hook) fails.
        super::mark_cwd_removed();
    }

    Ok(())
}

fn handle_detached_removed_worktree_output(
    repo: &Repository,
    ctx: &WorktreeRemovalContext<'_>,
    announcer: &mut HookAnnouncer<'_>,
) -> anyhow::Result<BranchFate> {
    if matches!(ctx.execution, RemovalExecution::Foreground) {
        eprintln!(
            "{}",
            progress_message(cformat!(
                "Removing worktree @ <bold>{}</>... (detached HEAD, no branch to delete)",
                format_path_for_display(ctx.worktree_path)
            ))
        );
        let snapshot = repo.capture_refs()?;
        let output = remove_worktree_with_cleanup(
            repo,
            &snapshot,
            ctx.worktree_path,
            RemoveOptions {
                branch: None,
                deletion_mode: ctx.deletion_mode,
                target_branch: ctx.target_branch.map(String::from),
                force_worktree: ctx.force_worktree,
            },
        )
        .map_err(|err| GitError::WorktreeRemovalFailed {
            branch: path_dir_name(ctx.worktree_path).to_string(),
            path: ctx.worktree_path.to_path_buf(),
            remaining_entries: list_remaining_entries(ctx.worktree_path),
            error: err.display_message(),
        })?;
        let (files, bytes) = output
            .staged_path
            .as_deref()
            .map(cleanup_staged_with_progress)
            .unwrap_or((0, 0));
        let stats_paren = format_stats_paren(files, bytes);
        eprintln!(
            "{}",
            success_message(cformat!(
                "Removed worktree @ <bold>{}</> (detached HEAD, no branch to delete)",
                format_path_for_display(ctx.worktree_path)
            ))
            .append(&stats_paren)
        );
    } else {
        let path_display = format_path_for_display(ctx.worktree_path);
        eprintln!(
            "{}",
            progress_message(cformat!(
                "Removing worktree @ <bold>{path_display}</> in background (detached HEAD, no branch to delete)"
            ))
        );

        spawn_background_removal(
            repo,
            ctx.main_path,
            &BackgroundRemoval {
                worktree_path: ctx.worktree_path,
                branch_name: None,
                deletion_mode: ctx.deletion_mode,
                target_branch: ctx.target_branch,
                force_worktree: ctx.force_worktree,
                changed_directory: ctx.changed_directory,
                // No branch → field is unused, but pick the silent-on-NotDeleted
                // default in case the detached HEAD ever gains a branch name.
                planner_expected_retention: true,
            },
            "detached",
            ctx.background_fallback(),
        )?;
    }

    // Post-remove hooks for detached HEAD use "HEAD" as the branch identifier
    spawn_hooks_after_remove(repo, ctx, "HEAD", announcer)?;
    stderr().flush()?;
    Ok(BranchFate::NotAttempted)
}

fn handle_named_removed_worktree_foreground(
    repo: &Repository,
    ctx: &WorktreeRemovalContext<'_>,
    branch_name: &str,
    announcer: &mut HookAnnouncer<'_>,
) -> anyhow::Result<BranchFate> {
    eprintln!(
        "{}",
        progress_message(cformat!("Removing <bold>{branch_name}</> worktree..."))
    );

    let snapshot = repo.capture_refs()?;
    let output = remove_worktree_with_cleanup(
        repo,
        &snapshot,
        ctx.worktree_path,
        RemoveOptions {
            branch: Some(branch_name.to_string()),
            deletion_mode: ctx.deletion_mode,
            target_branch: ctx.target_branch.map(String::from),
            force_worktree: ctx.force_worktree,
        },
    )
    .map_err(|err| GitError::WorktreeRemovalFailed {
        branch: branch_name.into(),
        path: ctx.worktree_path.to_path_buf(),
        remaining_entries: list_remaining_entries(ctx.worktree_path),
        error: err.display_message(),
    })?;
    let stats = output
        .staged_path
        .as_deref()
        .map(cleanup_staged_with_progress)
        .unwrap_or((0, 0));

    // The observed fate, read before the display path consumes (and, on Err,
    // propagates) the deletion result.
    let fate = BranchFate::from_result(output.branch_result.as_ref());

    let display_info = RemovalDisplayInfo::from_branch_result(
        output.branch_result,
        branch_name,
        ctx.integration_reason,
        ctx.target_branch,
        ctx.force_worktree,
    )?;

    display_info.print_message(branch_name, true, Some(stats))?;
    display_info.print_hints(branch_name, ctx.deletion_mode, ctx.integration_reason)?;
    if let Some(shared) = ctx.branch_checked_out_at {
        print_branch_checked_out_elsewhere(branch_name, shared);
    }
    print_switch_message_if_changed(ctx.changed_directory, ctx.main_path)?;

    spawn_hooks_after_remove(repo, ctx, branch_name, announcer)?;
    stderr().flush()?;
    Ok(fate)
}

fn handle_named_removed_worktree_background(
    repo: &Repository,
    ctx: &WorktreeRemovalContext<'_>,
    branch_name: &str,
    announcer: &mut HookAnnouncer<'_>,
) -> anyhow::Result<BranchFate> {
    let display_info = RemovalDisplayInfo::from_precomputed(
        ctx.deletion_mode,
        ctx.integration_reason,
        ctx.target_branch,
        ctx.force_worktree,
    );

    display_info.print_message(branch_name, false, None)?;
    display_info.print_hints(branch_name, ctx.deletion_mode, ctx.integration_reason)?;
    if let Some(shared) = ctx.branch_checked_out_at {
        print_branch_checked_out_elsewhere(branch_name, shared);
    }
    print_switch_message_if_changed(ctx.changed_directory, ctx.main_path)?;

    // Planner predicted retention when the user asked to keep, or when the
    // integration check at planning time returned no reason — `print_hints`
    // has already explained either case. A `NotDeleted` outcome surprises only
    // if the planner predicted deletion.
    let planner_expected_retention =
        ctx.deletion_mode.should_keep() || ctx.integration_reason.is_none();

    let fate = spawn_background_removal(
        repo,
        ctx.main_path,
        &BackgroundRemoval {
            worktree_path: ctx.worktree_path,
            branch_name: Some(branch_name),
            deletion_mode: ctx.deletion_mode,
            target_branch: ctx.target_branch,
            force_worktree: ctx.force_worktree,
            changed_directory: ctx.changed_directory,
            planner_expected_retention,
        },
        branch_name,
        ctx.background_fallback(),
    )?;

    spawn_hooks_after_remove(repo, ctx, branch_name, announcer)?;
    stderr().flush()?;
    Ok(fate)
}

/// Execute and narrate a [`RemovalPlan::Worktree`] plan.
fn handle_removed_worktree_output(
    ctx: WorktreeRemovalContext<'_>,
    announcer: &mut HookAnnouncer<'_>,
) -> anyhow::Result<BranchFate> {
    // Use main_path for discovery - the worktree being removed might be cwd,
    // and git operations after removal need a valid working directory.
    let repo = worktrunk::git::Repository::at(ctx.main_path)?;

    execute_pre_remove_hooks_if_needed(&repo, &ctx)?;

    // No re-validation after `pre-remove` hooks: the pre-rename `ensure_clean`
    // in the removal core catches a hook-dirtied worktree, and the branch
    // deletion re-decides against fresh refs (`delete_branch_if_safe`'s CAS)
    // — one mechanism per guarantee. `ctx.integration_reason` /
    // `ctx.target_branch` carry the planning-time verdict for display.

    // TUI (picker) path: the removal runs in a background thread while skim
    // owns the terminal, so no messages, no spinner, no `cd` directive (the
    // picker manages its own cwd). The git removal runs inline — same as the
    // foreground path, minus the chrome.
    if matches!(ctx.execution, RemovalExecution::Silent) {
        return remove_removed_worktree_silently(&repo, &ctx, announcer);
    }

    prepare_remove_directory_change(ctx.main_path, ctx.worktree_path, ctx.changed_directory)?;

    // Handle detached HEAD case (no branch known)
    let Some(branch_name) = ctx.branch_name else {
        return handle_detached_removed_worktree_output(&repo, &ctx, announcer);
    };

    if matches!(ctx.execution, RemovalExecution::Foreground) {
        handle_named_removed_worktree_foreground(&repo, &ctx, branch_name, announcer)
    } else {
        handle_named_removed_worktree_background(&repo, &ctx, branch_name, announcer)
    }
}

/// Remove a [`RemovalPlan::Worktree`] target with no terminal output — the
/// TUI (`wt switch` picker) path of [`handle_remove_output`].
///
/// `pre-remove` has already run (when it was approved — `verify`), and the
/// caller skipped the `cd` directive (the picker manages its own process cwd).
/// This does the synchronous git worktree removal and registers `post-remove` /
/// `post-switch` hooks onto `announcer`, but with no progress/success message
/// and no trash-cleanup spinner — `eprintln!` while skim owns the terminal
/// would corrupt the frame. A removal failure propagates as-is (the picker logs
/// it); there's no TTY to render the foreground path's nicer "remaining
/// entries" error against.
fn remove_removed_worktree_silently(
    repo: &Repository,
    ctx: &WorktreeRemovalContext<'_>,
    announcer: &mut HookAnnouncer<'_>,
) -> anyhow::Result<BranchFate> {
    let snapshot = repo.capture_refs()?;
    let options = RemoveOptions {
        branch: ctx.branch_name.map(String::from),
        deletion_mode: ctx.deletion_mode,
        target_branch: ctx.target_branch.map(String::from),
        force_worktree: ctx.force_worktree,
    };
    let output = remove_worktree_with_cleanup(repo, &snapshot, ctx.worktree_path, options)?;
    if let Some(staged) = output.staged_path {
        // Best-effort, same as the fast-path cleanup in the foreground handler
        // but without the TTY spinner (`cleanup_staged_with_progress`).
        let _ = std::fs::remove_dir_all(&staged);
    }

    // A best-effort deletion's failure is deliberately not narrated here
    // (no terminal to narrate to); the fate still reports the branch as
    // surviving.
    let fate = BranchFate::from_result(output.branch_result.as_ref());

    // Post-remove (and post-switch when the picker cd'd away) hooks — registered
    // onto the caller's announcer, which `flush`es after this returns.
    spawn_hooks_after_remove(repo, ctx, ctx.branch_name.unwrap_or("HEAD"), announcer)?;
    Ok(fate)
}

/// Run a shell command with streaming output, signal forwarding, and ANSI reset.
///
/// Entry point for foreground hook and alias execution. `wt step for-each`
/// (`for_each.rs`, direct argv exec with no shell) and the background
/// pipeline runner (`run_pipeline.rs`, redirects to log files and runs
/// detached) have their own spawning logic.
///
/// Capabilities: optional stdout→stderr redirect for deterministic ordering,
/// SIGINT/SIGTERM forwarding to child process group, ANSI reset before child
/// runs, `Cmd` tracing/logging, and CD directive control.
///
/// ## Directive files
///
/// `directives` controls whether the child can write shell-integration
/// directives back to the parent shell. The CD file holds a raw path and is
/// safe to pass through.
///
/// - `DirectivePassthrough::default()` — scrubs all directive env vars from
///   the child. Used by background hooks (outlive the parent shell).
/// - `DirectivePassthrough::inherit_from_env()` — re-adds CD. Used by aliases
///   and foreground hooks, which may emit `cd` directives.
///
/// ## Stdout routing
///
/// `redirect_stdout_to_stderr` controls whether the child's stdout is merged
/// onto wt's stderr (`true`) or passed through unchanged (`false`).
///
/// - Hooks and `for-each` pass `true`: their output is decoration around
///   wt's own stderr messages, and merging keeps "Running …" / progress lines
///   ordered with the child's own writes.
/// - Aliases pass `false`: `wt <alias>` is a user-defined command and its
///   stdout must remain pipeable, so `wt my-alias | jq` and similar
///   compositions work (#2478).
///
/// ## ANSI reset
///
/// Resets ANSI codes on stderr before the child runs. Terminal emulators
/// maintain a single rendering state machine — if stdout writes color codes
/// but stderr's output arrives next, the terminal applies stdout's color
/// state to stderr's text. The reset to stderr prevents this.
///
/// ## Git discovery
///
/// `scrub_git_discovery` removes inherited `GIT_DIR`/`GIT_WORK_TREE` (and the
/// rest of [`INHERITED_GIT_PATH_VARS`]) from the child so its `git` commands
/// discover the repo from `working_dir`. Hooks pass `true` (they operate on the
/// worktree wt targets); aliases pass `false` (they keep wt's inherited context,
/// like a top-level command the user typed). See issue #3373.
///
/// [`INHERITED_GIT_PATH_VARS`]: worktrunk::shell_exec::INHERITED_GIT_PATH_VARS
pub fn execute_shell_command(
    working_dir: &std::path::Path,
    command: &str,
    stdin_content: Option<&str>,
    command_log_label: Option<&str>,
    directives: DirectivePassthrough,
    redirect_stdout_to_stderr: bool,
    scrub_git_discovery: bool,
) -> anyhow::Result<()> {
    // Flush stdout before executing command to ensure all our messages appear
    // before the child process output
    stderr().flush()?;

    // Reset ANSI codes on stderr to prevent color bleeding (see function docs for details)
    // This fixes color bleeding observed when worktrunk prints colored output to stdout
    // followed immediately by child process output to stderr (e.g., pre-commit run output).
    eprint!("{}", anstyle::Reset);
    stderr().flush().ok(); // Ignore flush errors - reset is best-effort, command execution should proceed

    let mut cmd = Cmd::shell(command)
        .current_dir(working_dir)
        .forward_signals();

    // User hooks discover their repo from the cwd wt sets, not an inherited
    // GIT_DIR/GIT_WORK_TREE (issue #3373). Aliases keep the inherited context.
    if scrub_git_discovery {
        cmd = cmd.scrub_git_discovery_env();
    }

    if redirect_stdout_to_stderr {
        cmd = cmd.stdout(Stdio::from(std::io::stderr()));
    }

    if let Some(label) = command_log_label {
        cmd = cmd.external(label);
    }

    if let Some(content) = stdin_content {
        cmd = cmd.stdin_bytes(content);
    } else {
        // Inherit the parent's stdin so interactive children (e.g. TUI
        // pickers) keep their controlling terminal. `inherit_stdin()` also
        // keeps the child in the parent's process group so `tcsetattr` on
        // `/dev/tty` succeeds — see the method's doc comment for the
        // SIGTTOU rationale.
        cmd = cmd.inherit_stdin();
    }

    if let Some(path) = directives.cd_file {
        cmd = cmd.directive_cd_file(path);
    }

    cmd.stream()?;

    // Flush to ensure all output appears before we continue
    stderr().flush()?;

    Ok(())
}

/// Selector for which directive file env vars to pass through to a child shell.
///
/// `Default` (no fields set) scrubs all directive env vars from the child;
/// [`DirectivePassthrough::inherit_from_env`] reads the current process
/// environment and re-adds CD.
#[derive(Debug, Default, Clone)]
pub struct DirectivePassthrough {
    pub cd_file: Option<std::path::PathBuf>,
}

impl DirectivePassthrough {
    /// Pass the CD directive file through to the child, reading the current
    /// process environment. Used by project aliases and foreground hooks that
    /// may legitimately emit a `cd` directive.
    pub fn inherit_from_env() -> Self {
        use worktrunk::shell_exec::DIRECTIVE_CD_FILE_ENV_VAR;
        Self {
            cd_file: read_directive_env(DIRECTIVE_CD_FILE_ENV_VAR),
        }
    }
}

fn read_directive_env(var: &str) -> Option<std::path::PathBuf> {
    std::env::var_os(var)
        .map(std::path::PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;

    /// Exercises each arm of [`warn_if_branch_retained`] for coverage and
    /// to pin the suppression rule. Output goes to stderr and isn't captured
    /// here — the assertion is that the call doesn't panic on any arm.
    /// User-visible message shapes are exercised by the integration tests
    /// (e.g. `test_merge_pre_remove_new_commit_keeps_branch`).
    #[test]
    fn warn_if_branch_retained_arms() {
        let ok = |outcome| {
            Ok::<_, anyhow::Error>(BranchDeletionResult {
                outcome,
                integration_target: "main".to_string(),
            })
        };

        // NotDeleted + planner did NOT expect retention → warn (surprise race)
        warn_if_branch_retained("feature", &ok(BranchDeletionOutcome::NotDeleted), false);

        // NotDeleted + planner DID expect retention → silent (print_hints
        // already explained)
        warn_if_branch_retained("feature", &ok(BranchDeletionOutcome::NotDeleted), true);

        // RetainedRaced → always surface, regardless of planner prediction
        warn_if_branch_retained("feature", &ok(BranchDeletionOutcome::RetainedRaced), false);
        warn_if_branch_retained("feature", &ok(BranchDeletionOutcome::RetainedRaced), true);

        // RetainedCheckedOut → surface the checkout path
        warn_if_branch_retained(
            "feature",
            &ok(BranchDeletionOutcome::RetainedCheckedOut {
                path: Path::new("/tmp/repo.feature-survivor").to_path_buf(),
            }),
            false,
        );

        // Success arms → silent
        warn_if_branch_retained("feature", &ok(BranchDeletionOutcome::ForceDeleted), false);
        warn_if_branch_retained(
            "feature",
            &ok(BranchDeletionOutcome::Integrated(
                IntegrationReason::SameCommit,
            )),
            false,
        );

        // Err arm → tracing::warn! only, no stderr message
        warn_if_branch_retained(
            "feature",
            &Err(anyhow::anyhow!("simulated git failure")),
            false,
        );
    }

    /// The shared raced-retention message varies its lead-in with
    /// `removed_worktree` and always suggests the canonical `-D` recovery.
    #[test]
    fn retained_raced_branch_message_lead_in_varies() {
        let removed = retained_raced_branch_message("feature", true);
        assert!(removed.contains("Removed worktree but kept branch"));
        assert!(removed.contains("moved during deletion"));
        assert!(removed.contains("wt remove -D feature"));

        let branch_only = retained_raced_branch_message("feature", false);
        assert!(branch_only.contains("Kept branch"));
        assert!(!branch_only.contains("Removed worktree"));
        assert!(branch_only.contains("wt remove -D feature"));
    }

    #[test]
    fn retained_checked_out_branch_message_names_checkout() {
        let path = Path::new("/tmp/repo.feature-survivor");
        let removed = retained_checked_out_branch_message("feature", path, true);
        assert!(removed.contains("Removed worktree but retained branch"));
        assert!(removed.contains("/tmp/repo.feature-survivor"));
        assert!(!removed.contains("moved during deletion"));
        assert!(!removed.contains("not integrated"));

        let branch_only = retained_checked_out_branch_message("feature", path, false);
        assert!(branch_only.contains("Retained branch"));
        assert!(!branch_only.contains("Removed worktree"));
    }

    #[test]
    fn build_remove_command_with_tail_appends_only_when_present() {
        let path = Path::new("/tmp/wt");
        let bare = build_remove_command_with_tail(path, false, false, None);
        // No tail → the command is exactly the bare worktree removal.
        assert!(!bare.contains("&&"));
        let tailed = build_remove_command_with_tail(
            path,
            false,
            false,
            Some("git update-ref -d refs/heads/x deadbeef"),
        );
        // A tail is chained with `&&` so it runs only after a successful removal.
        assert_eq!(
            tailed,
            format!("{bare} && git update-ref -d refs/heads/x deadbeef")
        );
    }

    /// The detached safe-delete tail samples worktree topology immediately
    /// before the ref CAS. A checkout created after the tail was planned must
    /// make the tail succeed without deleting the branch.
    #[test]
    fn detached_cas_tail_retains_branch_checked_out_after_planning() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        test.create_branch("feature");
        let repo = Repository::at(test.root_path()).unwrap();
        let expected_sha = test.git_output(&["rev-parse", "feature"]);
        let tail = build_cas_branch_delete_tail(&repo, "feature", Some("main")).unwrap();

        let normalized_tail = tail.replace(&expected_sha, "<sha>");
        assert!(
            normalized_tail.contains("awk -v worktrunk_wanted_branch='branch refs/heads/feature'")
                && normalized_tail.contains("git update-ref -d refs/heads/feature <sha>"),
            "the guard must associate the exact branch record with the existing CAS: {normalized_tail}"
        );

        let checkout = test.home_path().join("repo.feature-detached-race");
        test.run_git(&["worktree", "add", checkout.to_str().unwrap(), "feature"]);

        let guarded = Cmd::new("sh")
            .args(["-c", &tail])
            .current_dir(test.root_path())
            .run()
            .unwrap();
        assert!(
            guarded.status.success(),
            "a matching checkout is a clean skip"
        );
        assert!(
            repo.run_command(&["rev-parse", "--verify", "refs/heads/feature"])
                .is_ok(),
            "the detached guard must retain a newly checked-out branch"
        );
        assert!(
            repo.worktree_at(&checkout)
                .run_command(&["rev-parse", "--verify", "HEAD"])
                .is_ok(),
            "the guarded tail must not orphan the checkout"
        );

        let failing_awk = format!("awk() {{ return 2; }}; {tail}");
        let failed_guard = Cmd::new("sh")
            .args(["-c", &failing_awk])
            .current_dir(test.root_path())
            .run()
            .unwrap();
        assert!(
            !failed_guard.status.success(),
            "an awk/tool failure is not the no-match case"
        );
        assert!(
            repo.run_command(&["rev-parse", "--verify", "refs/heads/feature"])
                .is_ok(),
            "a topology-parser failure must fail closed"
        );
    }

    /// A lock protects a temporarily absent worktree. The detached parser must
    /// let it win even over a synthetic record that also says `prunable`.
    #[test]
    fn detached_topology_parser_retains_locked_prunable_record() {
        let porcelain = "worktree /missing/feature
HEAD 0123456789abcdef
branch refs/heads/feature
locked detachable media
prunable gitdir file points to non-existent location

";
        let command = format!(
            "awk -v worktrunk_wanted_branch='branch refs/heads/feature' '{LIVE_BRANCH_WORKTREE_AWK}'"
        );
        let output = Cmd::new("sh")
            .args(["-c", &command])
            .stdin_bytes(porcelain)
            .run()
            .unwrap();

        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "live");
    }

    /// The exact porcelain branch line and ref argument are shell-escaped. A
    /// prunable stale registration is not a live checkout, so that arm still
    /// reaches the expected-SHA CAS without interpreting a valid but
    /// shell-active branch name.
    #[test]
    fn detached_cas_tail_escapes_branch_and_deletes_when_not_checked_out() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let branch = "feature;touch${IFS}PWNED";
        test.create_branch(branch);
        let repo = Repository::at(test.root_path()).unwrap();
        let tail = build_cas_branch_delete_tail(&repo, branch, Some("main")).unwrap();
        assert!(
            tail.contains("'branch refs/heads/feature;touch${IFS}PWNED'"),
            "the complete porcelain line must be one escaped shell word: {tail}"
        );
        assert!(
            tail.contains("'refs/heads/feature;touch${IFS}PWNED'"),
            "the CAS ref must be escaped independently: {tail}"
        );

        let stale = test.home_path().join("repo.feature-stale");
        test.run_git(&["worktree", "add", stale.to_str().unwrap(), branch]);
        std::fs::remove_dir_all(&stale).unwrap();

        let output = Cmd::new("sh")
            .args(["-c", &tail])
            .current_dir(test.root_path())
            .run()
            .unwrap();
        assert!(output.status.success(), "no checkout should run the CAS");
        assert!(
            repo.run_command(&["rev-parse", "--verify", &format!("refs/heads/{branch}")])
                .is_err(),
            "the intended branch should be deleted"
        );
        assert!(
            !test.root_path().join("PWNED").exists(),
            "the branch name must never be interpreted by the shell"
        );
    }

    #[test]
    fn test_format_switch_message() {
        let path = PathBuf::from("/tmp/test");

        // Switched to existing worktree (no creation)
        let msg = format_switch_message("feature", &path, false, false, None, None);
        assert_snapshot!(msg, @"Switched to worktree for [1mfeature[22m @ [1m/tmp/test[22m");

        // Created branch and worktree with --create
        let msg = format_switch_message("feature", &path, true, true, Some("main"), None);
        assert_snapshot!(msg, @"Created branch [1mfeature[22m from [1mmain[22m and worktree @ [1m/tmp/test[22m");

        // Created worktree from remote (DWIM) - also creates local tracking branch
        let msg =
            format_switch_message("feature", &path, true, false, None, Some("origin/feature"));
        assert_snapshot!(msg, @"Created branch [1mfeature[22m (tracking [1morigin/feature[22m) and worktree @ [1m/tmp/test[22m");

        // Created worktree only (local branch already existed)
        let msg = format_switch_message("feature", &path, true, false, None, None);
        assert!(!msg.contains("branch")); // Should NOT mention branch creation
        assert_snapshot!(msg, @"Created worktree for [1mfeature[22m @ [1m/tmp/test[22m");
    }

    #[test]
    fn test_flag_note() {
        // --no-delete-branch flag (text only, no symbol, no suffix)
        let note = flag_note(
            BranchDeletionMode::Keep,
            &BranchDeletionOutcome::NotDeleted,
            None,
        );
        assert_eq!(note.text, " (--no-delete-branch)");
        assert!(note.symbol.is_none());
        assert!(note.suffix.is_empty());

        // NotDeleted without flag (empty)
        let note = flag_note(
            BranchDeletionMode::SafeDelete,
            &BranchDeletionOutcome::NotDeleted,
            None,
        );
        assert!(note.text.is_empty());
        assert!(note.symbol.is_none());
        assert!(note.suffix.is_empty());

        // Force deleted (text only, no symbol, no suffix)
        let note = flag_note(
            BranchDeletionMode::ForceDelete,
            &BranchDeletionOutcome::ForceDeleted,
            None,
        );
        assert_eq!(note.text, " (--force-delete)");
        assert!(note.symbol.is_none());
        assert!(note.suffix.is_empty());

        // Integration reasons - text includes description and target, symbol is separate, suffix is closing paren
        let cases = [
            (IntegrationReason::SameCommit, "same commit as"),
            (IntegrationReason::Ancestor, "ancestor of"),
            (IntegrationReason::NoAddedChanges, "no added changes on"),
            (IntegrationReason::TreesMatch, "tree matches"),
            (IntegrationReason::MergeAddsNothing, "all changes in"),
        ];
        for (reason, expected_desc) in cases {
            let note = flag_note(
                BranchDeletionMode::SafeDelete,
                &BranchDeletionOutcome::Integrated(reason),
                Some("main"),
            );
            assert!(
                note.text.contains(expected_desc),
                "reason {:?} text should contain '{}'",
                reason,
                expected_desc
            );
            assert!(
                note.text.contains("main"),
                "reason {:?} text should contain target 'main'",
                reason
            );
            assert!(
                note.symbol.is_some(),
                "reason {:?} should have a symbol",
                reason
            );
            let symbol = note.symbol.as_ref().unwrap();
            assert!(
                symbol.contains(reason.symbol()),
                "reason {:?} symbol part should contain the symbol",
                reason
            );
            assert_eq!(
                note.suffix, ")",
                "reason {:?} suffix should be closing paren",
                reason
            );
        }
    }

    #[test]
    fn test_resolve_subdir_in_target_no_source_root() {
        let target = PathBuf::from("/target/worktree");
        let cwd = PathBuf::from("/some/dir");
        assert_eq!(resolve_subdir_in_target(&target, None, &cwd), target);
    }

    #[test]
    fn test_resolve_subdir_in_target_subdir_exists() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        std::fs::create_dir_all(source.join("apps/gateway")).unwrap();
        std::fs::create_dir_all(target.join("apps/gateway")).unwrap();

        let cwd = source.join("apps/gateway");
        let result = resolve_subdir_in_target(&target, Some(&source), &cwd);
        assert_eq!(result, target.join("apps/gateway"));
    }

    #[test]
    fn test_resolve_subdir_in_target_subdir_deeper_than_windows_max_path() {
        // A cwd nested past Windows' 260-character path limit comes back from
        // `dunce` spelled `\\?\C:\…` while its short worktree root stays `C:\…`,
        // so `strip_prefix` fails and the shell lands at the target root with no
        // error rather than at the mirrored subdirectory (#3898). Off Windows
        // the depth is unremarkable and this just exercises a deep tree.
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");

        let mut relative = PathBuf::from("node_modules");
        while source.join(&relative).as_os_str().len() < 320 {
            relative.push("nested-dependency-directory");
        }
        std::fs::create_dir_all(source.join(&relative)).unwrap();
        std::fs::create_dir_all(target.join(&relative)).unwrap();

        let result = resolve_subdir_in_target(&target, Some(&source), &source.join(&relative));
        assert_eq!(result, target.join(&relative));
    }

    #[test]
    fn test_resolve_subdir_in_target_subdir_missing() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        std::fs::create_dir_all(source.join("apps/gateway")).unwrap();
        std::fs::create_dir_all(&target).unwrap();

        let cwd = source.join("apps/gateway");
        let result = resolve_subdir_in_target(&target, Some(&source), &cwd);
        assert_eq!(result, target); // Falls back to root
    }

    #[test]
    fn test_resolve_subdir_in_target_at_root() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&target).unwrap();

        let result = resolve_subdir_in_target(&target, Some(&source), &source);
        assert_eq!(result, target);
    }

    #[test]
    fn test_git_subcommand_warning() {
        let warning = git_subcommand_warning();
        assert_snapshot!(warning, @"For automatic cd, invoke directly (with the [4m-[24m): [4mgit-wt[24m");
    }
}
