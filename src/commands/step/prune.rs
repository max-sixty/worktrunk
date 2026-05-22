//! `wt step prune` — remove worktrees and branches integrated into the default branch.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::Context;
use color_print::cformat;
use crossbeam_channel as chan;
use rayon::prelude::*;
use worktrunk::HookType;
use worktrunk::config::UserConfig;
use worktrunk::git::{BranchDeletionMode, RefSnapshot, Repository, WorktreeInfo};
use worktrunk::styling::{eprintln, hint_message, info_message, println, success_message};

use super::super::hook_plan::{ApprovedHookPlan, HookPlanBuilder};
use super::super::hooks::HookAnnouncer;
use super::super::repository_ext::{RemoveTarget, RepositoryCliExt};
use crate::output::{BackgroundFallbackMode, handle_remove_output};

/// A candidate worktree or branch selected for removal.
#[derive(Clone)]
struct Candidate {
    /// Original index in `check_items` (for deterministic output ordering)
    check_idx: usize,
    /// Branch name (None for detached HEAD worktrees)
    branch: Option<String>,
    /// Display label: branch name or abbreviated commit SHA
    label: String,
    /// Worktree path (for detached worktrees and stale metadata)
    path: Option<PathBuf>,
    /// Current worktree, other worktree, branch-only, or stale detached metadata
    kind: CandidateKind,
}

impl Candidate {
    fn remove_target(&self) -> anyhow::Result<RemoveTarget<'_>> {
        match self.kind {
            CandidateKind::Current => Ok(RemoveTarget::Current),
            CandidateKind::BranchOnly => Ok(RemoveTarget::Branch(
                self.branch
                    .as_ref()
                    .context("BranchOnly candidate missing branch")?,
            )),
            CandidateKind::StaleDetached => Err(anyhow::anyhow!(
                "stale detached candidate has no remove target"
            )),
            CandidateKind::Other => match &self.branch {
                Some(branch) => Ok(RemoveTarget::Branch(branch)),
                None => Ok(RemoveTarget::Path(
                    self.path
                        .as_ref()
                        .context("detached candidate missing path")?,
                )),
            },
        }
    }

    /// Error context for `try_remove` failures: distinguishes branch-only
    /// removals (no worktree exists) from worktree removals.
    fn removal_context(&self) -> String {
        match self.kind {
            CandidateKind::BranchOnly => format!("removing branch {}", self.label),
            CandidateKind::StaleDetached => format!("pruning stale worktree for {}", self.label),
            CandidateKind::Current | CandidateKind::Other => {
                format!("removing worktree for {}", self.label)
            }
        }
    }
}

#[derive(Clone, Copy)]
enum CandidateKind {
    Current,
    Other,
    BranchOnly,
    StaleDetached,
}

impl CandidateKind {
    fn as_str(&self) -> &'static str {
        match self {
            CandidateKind::Current => "current",
            CandidateKind::Other => "worktree",
            CandidateKind::BranchOnly => "branch_only",
            CandidateKind::StaleDetached => "stale_worktree",
        }
    }
}

/// Where a candidate originated, used to drive integration checks and dry-run labels.
enum CheckSource {
    /// Worktree with directory gone (prunable)
    Prunable { wt_idx: usize },
    /// Linked worktree
    Linked { wt_idx: usize },
    /// Local branch without a worktree entry
    Orphan,
}

struct CheckItem {
    integration_ref: String,
    source: CheckSource,
}

/// Per-candidate context displayed only in dry-run output.
struct DryRunInfo {
    reason_desc: String,
    effective_target: String,
    suffix: &'static str,
}

enum PruneEvent {
    Candidate(usize),
    SkippedYoung(String),
}

/// Build a human-readable count like "3 worktrees & branches".
///
/// Worktree + branch is the default pair (matching progress messages'
/// "worktree & branch" pattern). Unpaired items listed separately.
fn prune_summary(candidates: &[Candidate]) -> String {
    let mut worktree_with_branch = 0usize;
    let mut detached_worktree = 0usize;
    let mut branch_only = 0usize;
    for c in candidates {
        match &c.kind {
            CandidateKind::BranchOnly => branch_only += 1,
            // A stale detached worktree never has a branch.
            CandidateKind::StaleDetached => detached_worktree += 1,
            CandidateKind::Current | CandidateKind::Other => {
                if c.branch.is_some() {
                    worktree_with_branch += 1;
                } else {
                    detached_worktree += 1;
                }
            }
        }
    }
    let mut parts = Vec::new();
    if worktree_with_branch > 0 {
        let noun = if worktree_with_branch == 1 {
            "worktree & branch"
        } else {
            "worktrees & branches"
        };
        parts.push(format!("{worktree_with_branch} {noun}"));
    }
    if detached_worktree > 0 {
        let noun = if detached_worktree == 1 {
            "worktree"
        } else {
            "worktrees"
        };
        parts.push(format!("{detached_worktree} {noun}"));
    }
    if branch_only > 0 {
        let noun = if branch_only == 1 {
            "branch"
        } else {
            "branches"
        };
        parts.push(format!("{branch_only} {noun}"));
    }
    parts.join(", ")
}

/// Loop-invariant context for [`try_remove`]: every field is identical at all
/// three call sites in [`step_prune`] (only the `Candidate` varies). Built once
/// and passed by reference.
struct RemovalContext<'a> {
    repo: &'a Repository,
    config: &'a UserConfig,
    foreground: bool,
    hook_plan: &'a ApprovedHookPlan,
    worktrees: &'a [WorktreeInfo],
    snapshot: &'a RefSnapshot,
    /// Serializes the parallel `integration_reason` readers against the
    /// removal writer — load-bearing for the Windows `.git/config` race fix.
    /// `try_remove` acquires the write guard at the top of its body and holds
    /// it over the whole body.
    check_lock: &'a RwLock<()>,
}

/// Try to remove a candidate immediately. Returns Ok(true) if removed,
/// Ok(false) if skipped (preparation error), Err on execution error.
fn try_remove(candidate: &Candidate, ctx: &RemovalContext<'_>) -> anyhow::Result<bool> {
    // Take the same write guard used by integration-check readers. The current
    // phase ordering drains those readers before removal so approval can be
    // exact; keeping the guard here preserves the Windows `.git/config`
    // serialization if this flow is refactored back to overlap checks and
    // removals. Held over the whole body keeps the lock confined to this file.
    //
    // The guard protects `()` — there is no shared state to corrupt, so a
    // poisoned lock is meaningless here. Recover the guard rather than
    // `.expect()`-ing: a panic elsewhere should surface as itself, not as a
    // cascade of secondary poison panics on every later removal/reader.
    let _write = ctx.check_lock.write().unwrap_or_else(|e| e.into_inner());

    if matches!(candidate.kind, CandidateKind::StaleDetached) {
        ctx.repo.prune_worktrees()?;
        return Ok(true);
    }

    let target = candidate.remove_target()?;
    let plan = match ctx.repo.prepare_worktree_removal(
        target,
        BranchDeletionMode::SafeDelete,
        false,
        ctx.config,
        None,
        Some(ctx.worktrees),
        Some(ctx.snapshot),
    ) {
        Ok(plan) => plan,
        Err(_) => {
            // prepare_worktree_removal is the gate: if the worktree can't
            // be removed (dirty, locked, etc.), it's simply not selected.
            return Ok(false);
        }
    };
    let mut announcer = HookAnnouncer::new(ctx.repo, ctx.config, true);
    // `SynchronousForNonCurrent`: prune keeps the rename-failure fallback's
    // `.git/config` rewrite serialized with its integration-check readers.
    handle_remove_output(
        &plan,
        ctx.foreground,
        ctx.hook_plan,
        true,
        false,
        &mut announcer,
        BackgroundFallbackMode::SynchronousForNonCurrent,
    )?;
    announcer.flush()?;
    Ok(true)
}

fn can_attempt_candidate_removal(
    candidate: &Candidate,
    repo: &Repository,
    config: &UserConfig,
    worktrees: &[WorktreeInfo],
    snapshot: &RefSnapshot,
) -> anyhow::Result<bool> {
    if matches!(
        candidate.kind,
        CandidateKind::BranchOnly | CandidateKind::StaleDetached
    ) {
        return Ok(true);
    }

    let target = candidate.remove_target()?;
    Ok(repo
        .prepare_worktree_removal(
            target,
            BranchDeletionMode::SafeDelete,
            false,
            config,
            None,
            Some(worktrees),
            Some(snapshot),
        )
        .is_ok())
}

/// Walk the worktree list and the local branch list to build the set of
/// candidates whose integration status needs checking.
///
/// Returns the items in a deterministic order: worktree entries first
/// (preserving `worktrees` order), then orphan branches.
fn gather_check_items(
    repo: &Repository,
    worktrees: &[WorktreeInfo],
    default_branch: Option<&str>,
) -> anyhow::Result<Vec<CheckItem>> {
    let mut check_items: Vec<CheckItem> = Vec::new();
    // Track branches seen via worktree entries so we don't double-count
    // in the orphan branch scan below.
    let mut seen_branches: HashSet<String> = HashSet::new();

    for (idx, wt) in worktrees.iter().enumerate() {
        if let Some(branch) = &wt.branch {
            seen_branches.insert(branch.clone());
        }

        if wt.locked.is_some() {
            continue;
        }

        if let Some(branch) = &wt.branch
            && default_branch == Some(branch.as_str())
        {
            continue;
        }

        if wt.is_prunable() {
            let integration_ref = wt.branch.clone().unwrap_or_else(|| wt.head.clone());
            check_items.push(CheckItem {
                integration_ref,
                source: CheckSource::Prunable { wt_idx: idx },
            });
            continue;
        }

        // Skip main worktree (non-linked); in bare repos all are linked,
        // so the default-branch check above is the primary guard.
        let wt_tree = repo.worktree_at(&wt.path);
        if !wt_tree
            .is_linked()
            .context("checking whether worktree is linked")?
        {
            continue;
        }

        let integration_ref = match &wt.branch {
            Some(b) if !wt.detached => b.clone(),
            _ => wt.head.clone(),
        };

        check_items.push(CheckItem {
            integration_ref,
            source: CheckSource::Linked { wt_idx: idx },
        });
    }

    for branch in repo.all_branches().context("listing branches")? {
        if seen_branches.contains(&branch) {
            continue;
        }
        if default_branch == Some(branch.as_str()) {
            continue;
        }
        check_items.push(CheckItem {
            integration_ref: branch,
            source: CheckSource::Orphan,
        });
    }

    Ok(check_items)
}

/// Resolve the age of a linked worktree from filesystem metadata.
///
/// Tries `git_dir.created()` first; on filesystems that don't track creation
/// time (e.g. older ext4) falls back to the `commondir` mtime, which git
/// touches when the worktree is first created.
fn worktree_age(
    repo: &Repository,
    wt: &WorktreeInfo,
    now_secs: u64,
) -> anyhow::Result<Option<Duration>> {
    let wt_tree = repo.worktree_at(&wt.path);
    let git_dir = wt_tree.git_dir().context("resolving worktree git dir")?;
    let metadata = fs::metadata(&git_dir).context("Failed to read worktree git dir")?;
    let created = metadata
        .created()
        .or_else(|_| fs::metadata(git_dir.join("commondir")).and_then(|m| m.modified()));

    let Ok(created) = created else {
        return Ok(None);
    };
    let Ok(created_epoch) = created.duration_since(std::time::UNIX_EPOCH) else {
        return Ok(None);
    };
    Ok(Some(Duration::from_secs(
        now_secs.saturating_sub(created_epoch.as_secs()),
    )))
}

/// Resolve the age of an orphan branch via its reflog creation timestamp.
///
/// Returns `None` if the reflog is missing or unparsable — callers treat
/// "unknown age" as "old enough", matching the previous inline behavior.
fn orphan_branch_age(repo: &Repository, branch: &str, now_secs: u64) -> Option<Duration> {
    let ref_name = format!("refs/heads/{branch}");
    let stdout = repo
        .run_command(&["reflog", "show", "--format=%ct", &ref_name])
        .ok()?;
    let created_epoch = stdout
        .trim()
        .lines()
        .last()
        .and_then(|s| s.parse::<u64>().ok())?;
    Some(Duration::from_secs(now_secs.saturating_sub(created_epoch)))
}

/// Render dry-run output (text or JSON) and the `Skipped (younger than ...)`
/// trailer. Returns once printing is complete; the caller exits early.
fn render_dry_run(
    mut dry_run_info: Vec<(Candidate, DryRunInfo)>,
    mut skipped_young: Vec<String>,
    min_age: &str,
    format: crate::cli::SwitchFormat,
) -> anyhow::Result<()> {
    // Sort by original check order for deterministic output regardless of
    // channel completion order.
    dry_run_info.sort_by_key(|(c, _)| c.check_idx);

    if format == crate::cli::SwitchFormat::Json {
        let items: Vec<serde_json::Value> = dry_run_info
            .iter()
            .map(|(c, info)| {
                serde_json::json!({
                    "branch": c.branch,
                    "path": c.path,
                    "kind": c.kind.as_str(),
                    "reason": info.reason_desc,
                    "target": info.effective_target,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
        return Ok(());
    }

    let mut dry_candidates = Vec::new();
    for (candidate, info) in dry_run_info {
        eprintln!(
            "{}",
            info_message(cformat!(
                "<bold>{}</>{} — {} {}",
                candidate.label,
                info.suffix,
                info.reason_desc,
                info.effective_target
            ))
        );
        dry_candidates.push(candidate);
    }

    // Report skipped worktrees (after candidates, before summary).
    // Sort for deterministic output regardless of channel completion order.
    skipped_young.sort();
    if !skipped_young.is_empty() {
        let names = skipped_young
            .iter()
            .map(|n| cformat!("<bold>{n}</>"))
            .collect::<Vec<_>>()
            .join(", ");
        eprintln!(
            "{}",
            info_message(format!("Skipped {names} (younger than {min_age})"))
        );
    }

    if dry_candidates.is_empty() {
        if skipped_young.is_empty() {
            eprintln!("{}", info_message("No merged worktrees to remove"));
        }
        return Ok(());
    }
    eprintln!(
        "{}",
        hint_message(format!(
            "{} would be removed (dry run)",
            prune_summary(&dry_candidates)
        ))
    );
    Ok(())
}

/// Build and approve, once, the frozen hook plan `wt step prune` may run.
///
/// `pre-remove` runs only when a *live linked* worktree is removed (stale-
/// metadata and orphan-branch removals delete just the branch — no
/// `pre-remove`/`post-remove`/`post-switch`). Approval is built after
/// integration, age, and removability checks have selected the exact live
/// worktrees prune will attempt to remove. `post-switch` is anchored at the
/// primary worktree only when the current worktree is in that removal set.
/// Every hook is selected from the invoking worktree's `.config/wt.toml`,
/// whatever its anchor.
///
/// A declined prompt yields an empty plan — every executor runs no hooks.
fn approve_prune_hooks(
    repo: &Repository,
    config: &UserConfig,
    candidates: &[Candidate],
    yes: bool,
) -> anyhow::Result<ApprovedHookPlan> {
    // Non-fatal: prune candidates with no project hooks must still prune even
    // when the project identifier can't be resolved (the plan ends up empty
    // and `approve` never needs it).
    let project_id = repo.project_identifier().ok();
    let pid = project_id.as_deref();
    // Every prune hook is selected from the invoking worktree's
    // `.config/wt.toml` — the worktree `wt step prune` ran in.
    let project_config = repo.load_project_config()?;

    let removed_worktree_paths: Vec<&Path> = candidates
        .iter()
        .filter_map(|candidate| match candidate.kind {
            CandidateKind::Current | CandidateKind::Other => candidate.path.as_deref(),
            CandidateKind::BranchOnly | CandidateKind::StaleDetached => None,
        })
        .collect();

    let mut builder = HookPlanBuilder::new();
    for &wt_path in &removed_worktree_paths {
        builder.add(
            wt_path,
            &[HookType::PreRemove, HookType::PostRemove],
            project_config.as_ref(),
            config,
            pid,
        );
    }
    if candidates
        .iter()
        .any(|candidate| matches!(candidate.kind, CandidateKind::Current))
    {
        let primary_path = repo.home_path()?;
        builder.add(
            &primary_path,
            &[HookType::PostSwitch],
            project_config.as_ref(),
            config,
            pid,
        );
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
}

/// Remove worktrees and branches integrated into the default branch.
///
/// Handles four cases: live worktrees with branches (removed + branch deleted),
/// detached HEAD worktrees (directory removed, no branch to delete), stale worktree
/// entries (pruned + branch deleted), and orphan branches without worktrees (deleted).
/// Skips the main/primary worktree, locked worktrees, and worktrees younger than
/// `min_age`. Removes the current worktree last to trigger cd to primary.
pub fn step_prune(
    dry_run: bool,
    yes: bool,
    min_age: &str,
    foreground: bool,
    format: crate::cli::SwitchFormat,
) -> anyhow::Result<()> {
    let min_age_duration =
        humantime::parse_duration(min_age).context("Invalid --min-age duration")?;

    let repo = Repository::current()?;
    let config = UserConfig::load()?;

    // Capture once at command entry. Reused for every per-branch
    // `integration_reason` probe later in this function.
    let snapshot = repo.capture_refs().context("capturing repository refs")?;

    // Pass the local default branch (e.g. "main") directly — `integration_reason`
    // ORs over local + upstream internally, so a branch merged into either side
    // counts as integrated.
    let integration_target = repo
        .default_branch()
        .context("cannot determine default branch")?;

    let worktrees = repo.list_worktrees().context("listing worktrees")?;
    let current_root = repo
        .current_worktree()
        .root()
        .context("resolving current worktree root")?
        .to_path_buf();
    let current_root = dunce::canonicalize(&current_root).unwrap_or(current_root);
    let now_secs = worktrunk::utils::epoch_now();

    let default_branch = repo.default_branch();

    // Build the broad integration-check set first. Hook approval happens after
    // these checks, age filtering, and a live-worktree removability preflight
    // have selected the exact worktrees prune will attempt to remove.
    let check_items = gather_check_items(&repo, worktrees, default_branch.as_deref())?;

    let mut skipped_young: Vec<String> = Vec::new();
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut events: Vec<PruneEvent> = Vec::new();

    // Parallel integration checks.
    //
    // Spawn integration checks on a background thread via rayon par_iter,
    // sending each result through a channel as it completes. The main thread
    // processes results as they arrive: age-filtering, printing "Skipped"
    // messages, and collecting removal candidates. The hook approval gate runs
    // only after this has narrowed the broad check set.
    //
    // `check_lock` marks the Windows `.git/config` critical section. Each
    // `integration_reason` call (which fans out git subprocesses that read
    // `.git/config`) is held under a read guard. Later removals take the write
    // guard around the branch-deleting paths that rewrite `.git/config` via
    // lockfile+rename. The exact-approval phase ordering drains this thread
    // before removals begin, but the explicit guard keeps the serialization
    // local to prune if checks/removals are overlapped again.
    let check_lock = Arc::new(RwLock::new(()));
    let (tx, rx) = chan::unbounded();
    let integration_refs: Vec<String> = check_items
        .iter()
        .map(|item| item.integration_ref.clone())
        .collect();

    // Intentionally detached: if the main thread returns early (error in
    // the recv loop), remaining rayon tasks silently fail to send on the
    // closed channel and the thread cleans up on its own. Empty
    // integration_refs produces an empty par_iter that completes immediately.
    let repo_clone = repo.clone();
    let target = integration_target.clone();
    // Share by Arc — main thread keeps `snapshot_arc` for `try_remove`; the
    // rayon worker takes a refcount-bump clone. No deep snapshot copy.
    let snapshot_arc = Arc::new(snapshot);
    let snapshot_for_thread = Arc::clone(&snapshot_arc);
    let lock_for_thread = Arc::clone(&check_lock);
    std::thread::spawn(move || {
        integration_refs
            .into_par_iter()
            .enumerate()
            .for_each(|(idx, ref_name)| {
                // Hold the read guard only across `integration_reason` (all
                // its child git readers), then drop it before `send` so a
                // waiting writer is not blocked by channel backpressure.
                let result = {
                    let _read = lock_for_thread.read().unwrap_or_else(|e| e.into_inner());
                    repo_clone.integration_reason(&snapshot_for_thread, &ref_name, &target)
                };
                let _ = tx.send((idx, result));
            });
    });

    // Collect integration context alongside candidates for dry-run display.
    let mut dry_run_info: Vec<(Candidate, DryRunInfo)> = Vec::new();

    // Process results as they arrive from the channel.
    for (idx, result) in rx {
        let (effective_target, reason) = result.context("checking branch integration")?;
        let Some(reason) = reason else {
            continue;
        };

        let item = &check_items[idx];

        // Linked worktrees need special handling: age check via filesystem
        // metadata, current-worktree deferral, and path-based candidates.
        if let CheckSource::Linked { wt_idx } = &item.source {
            let wt = &worktrees[*wt_idx];
            let label = wt.branch.clone().unwrap_or_else(|| {
                let short = repo.short_sha(&wt.head).unwrap_or_else(|_| wt.head.clone());
                format!("(detached {short})")
            });

            // Skip recently-created worktrees that look "merged" because
            // they were just created from the default branch
            if min_age_duration > Duration::ZERO
                && let Some(age) = worktree_age(&repo, wt, now_secs)?
                && age < min_age_duration
            {
                if !dry_run {
                    events.push(PruneEvent::SkippedYoung(label.clone()));
                }
                skipped_young.push(label);
                continue;
            }

            let wt_path = dunce::canonicalize(&wt.path).unwrap_or(wt.path.clone());
            let is_current = wt_path == current_root;
            let candidate = Candidate {
                check_idx: idx,
                branch: if wt.detached { None } else { wt.branch.clone() },
                label,
                path: Some(wt.path.clone()),
                kind: if is_current {
                    CandidateKind::Current
                } else {
                    CandidateKind::Other
                },
            };
            if dry_run {
                let info = DryRunInfo {
                    reason_desc: reason.description().to_string(),
                    effective_target,
                    suffix: "",
                };
                dry_run_info.push((candidate, info));
            } else {
                let candidate_idx = candidates.len();
                candidates.push(candidate);
                events.push(PruneEvent::Candidate(candidate_idx));
            }
            continue;
        }

        // Stale detached metadata and branch-only candidates: prunable
        // branch worktrees and orphan branches.
        let (branch, label, path, kind, suffix) = match &item.source {
            CheckSource::Prunable { wt_idx } => {
                let wt = &worktrees[*wt_idx];
                let label = wt.branch.clone().unwrap_or_else(|| {
                    let short = repo.short_sha(&wt.head).unwrap_or_else(|_| wt.head.clone());
                    format!("(detached {short})")
                });
                match &wt.branch {
                    Some(branch) => (
                        Some(branch.clone()),
                        label,
                        None,
                        CandidateKind::BranchOnly,
                        " (stale)",
                    ),
                    None => (
                        None,
                        label,
                        Some(wt.path.clone()),
                        CandidateKind::StaleDetached,
                        " (stale)",
                    ),
                }
            }
            CheckSource::Orphan => (
                Some(item.integration_ref.clone()),
                item.integration_ref.clone(),
                None,
                CandidateKind::BranchOnly,
                " (branch only)",
            ),
            CheckSource::Linked { .. } => unreachable!(),
        };

        // Age check for orphan branches via reflog creation timestamp
        if matches!(&item.source, CheckSource::Orphan)
            && min_age_duration > Duration::ZERO
            && let Some(branch) = &branch
            && let Some(age) = orphan_branch_age(&repo, branch, now_secs)
            && age < min_age_duration
        {
            if !dry_run {
                events.push(PruneEvent::SkippedYoung(branch.clone()));
            }
            skipped_young.push(branch.clone());
            continue;
        }

        let candidate = Candidate {
            check_idx: idx,
            label,
            branch,
            path,
            kind,
        };
        if dry_run {
            let info = DryRunInfo {
                reason_desc: reason.description().to_string(),
                effective_target,
                suffix,
            };
            dry_run_info.push((candidate, info));
        } else {
            let candidate_idx = candidates.len();
            candidates.push(candidate);
            events.push(PruneEvent::Candidate(candidate_idx));
        }
    }

    if dry_run {
        return render_dry_run(dry_run_info, skipped_young, min_age, format);
    }

    let mut removable = Vec::with_capacity(candidates.len());
    let mut approval_candidates = Vec::new();
    for candidate in &candidates {
        let can_remove = can_attempt_candidate_removal(
            candidate,
            &repo,
            &config,
            worktrees,
            snapshot_arc.as_ref(),
        )?;
        removable.push(can_remove);
        if can_remove {
            approval_candidates.push(candidate.clone());
        }
    }

    let hook_plan = approve_prune_hooks(&repo, &config, &approval_candidates, yes)?;

    // Loop-invariant context shared by every `try_remove` call below.
    let removal_ctx = RemovalContext {
        repo: &repo,
        config: &config,
        foreground,
        hook_plan: &hook_plan,
        worktrees,
        snapshot: &snapshot_arc,
        check_lock: &check_lock,
    };

    let mut removed: Vec<Candidate> = Vec::new();
    let mut deferred_current: Option<Candidate> = None;
    for event in events {
        match event {
            PruneEvent::SkippedYoung(label) => {
                eprintln!(
                    "{}",
                    info_message(cformat!(
                        "Skipped <bold>{label}</> (younger than {min_age})"
                    ))
                );
            }
            PruneEvent::Candidate(candidate_idx) => {
                if !removable[candidate_idx] {
                    continue;
                }
                let candidate = candidates[candidate_idx].clone();
                if matches!(candidate.kind, CandidateKind::Current) {
                    deferred_current = Some(candidate);
                } else if try_remove(&candidate, &removal_ctx)
                    .with_context(|| candidate.removal_context())?
                {
                    removed.push(candidate);
                }
            }
        }
    }

    // Remove deferred current worktree last (cd-to-primary happens here)
    if let Some(current) = deferred_current
        && try_remove(&current, &removal_ctx).with_context(|| current.removal_context())?
    {
        removed.push(current);
    }

    if format == crate::cli::SwitchFormat::Json {
        let items: Vec<serde_json::Value> = removed
            .iter()
            .map(|c| {
                serde_json::json!({
                    "branch": c.branch,
                    "path": c.path,
                    "kind": c.kind.as_str(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
    } else if removed.is_empty() {
        if skipped_young.is_empty() {
            eprintln!("{}", info_message("No merged worktrees to remove"));
        }
    } else {
        eprintln!(
            "{}",
            success_message(format!("Pruned {}", prune_summary(&removed)))
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(kind: CandidateKind, label: &str) -> Candidate {
        Candidate {
            check_idx: 0,
            branch: Some(label.to_string()),
            label: label.to_string(),
            path: None,
            kind,
        }
    }

    #[test]
    fn removal_context_distinguishes_branch_only_from_worktree() {
        assert_eq!(
            candidate(CandidateKind::BranchOnly, "orphan").removal_context(),
            "removing branch orphan"
        );
        assert_eq!(
            candidate(CandidateKind::Other, "feature").removal_context(),
            "removing worktree for feature"
        );
        assert_eq!(
            candidate(CandidateKind::Current, "feature").removal_context(),
            "removing worktree for feature"
        );
        assert_eq!(
            candidate(CandidateKind::StaleDetached, "gone").removal_context(),
            "pruning stale worktree for gone"
        );
    }

    #[test]
    fn stale_detached_candidate_has_no_remove_target() {
        // `try_remove` and `can_attempt_candidate_removal` short-circuit
        // `StaleDetached` before calling `remove_target` — but assert the
        // guard arm so the contract is pinned if that ordering ever changes.
        let err = candidate(CandidateKind::StaleDetached, "gone")
            .remove_target()
            .unwrap_err();
        assert!(err.to_string().contains("no remove target"));
    }

    #[test]
    fn prune_summary_counts_each_candidate_kind() {
        let mut detached = candidate(CandidateKind::Other, "det");
        detached.branch = None;
        let candidates = [
            candidate(CandidateKind::Other, "feat-a"),
            candidate(CandidateKind::Other, "feat-b"),
            detached,
            candidate(CandidateKind::StaleDetached, "gone"),
            candidate(CandidateKind::BranchOnly, "orphan"),
        ];
        // 2 worktree+branch, 2 detached (one live + one stale), 1 branch-only.
        assert_eq!(
            prune_summary(&candidates),
            "2 worktrees & branches, 2 worktrees, 1 branch"
        );
    }
}
