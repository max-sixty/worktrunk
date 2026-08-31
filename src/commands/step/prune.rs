//! `wt step prune` — remove worktrees and branches integrated into the default branch.
//!
//! Live-path concurrency: candidate checks fan out on the rayon pool and
//! stream results to the main thread, which queues per-candidate jobs
//! (removals and skip lines) in scan-completion order onto a worker pool
//! sized like rayon's ([`RemovalJob`]). Checks and hook-free removals hold
//! the read side of [`RemovalContext::check_lock`] and run concurrently; the
//! exceptional removals serialize on the write side
//! ([`removal_needs_write`]). Repository operations coordinate the narrower
//! Git worktree-registry reads and teardowns themselves, so status checks,
//! fsmonitor shutdown, and trash renames still overlap. One FIFO queue carrying
//! both removals and skip lines means a single worker (`RAYON_NUM_THREADS=1`)
//! reproduces the serial total order the deterministic-output tests pin. The
//! first failing removal
//! flips an abort flag that drains the remaining queue unexecuted; the
//! current worktree is removed last, after the fan-out, because its removal
//! cd's the shell to the primary.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Context;
use color_print::cformat;
use crossbeam_channel as chan;
use rayon::prelude::*;
use worktrunk::HookType;
use worktrunk::config::{Approvals, ProjectConfig, UserConfig};
use worktrunk::git::{
    BranchDeletionMode, IntegrationReason, RefSnapshot, Repository, WorktreeInfo,
};
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{
    eprintln, format_with_gutter, hint_message, info_message, println, success_message,
};
use worktrunk::trace::Span;

use crate::output::print_json;

use super::super::hook_plan::{ApprovedHookPlan, HookPlan, HookPlanBuilder};
use super::super::hooks::HookAnnouncer;
use super::super::repository_ext::{RemoveTarget, RepositoryCliExt};
use super::super::worktree::{BranchFate, RemovalPlan};
use crate::output::{BackgroundFallbackMode, RemovalExecution, handle_remove_output};

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
    /// Whether the removal deletes `branch`. The kind is a plan, not an
    /// outcome: a branch a sibling worktree still has checked out is retained,
    /// so a worktree candidate can take the worktree and leave the branch
    /// standing. The summary counts this rather than the kind, so it never
    /// reports a branch the run deliberately kept.
    ///
    /// Starts as the scan's prediction (what the dry run prints); on the live
    /// path [`record_fate`](Self::record_fate) replaces it with the executed
    /// [`BranchFate`], so a deletion the CAS refused mid-run is counted as
    /// retained, not as the plan hoped.
    deletes_branch: bool,
    /// The executed fate, kept whole for `--format=json`'s `branch_outcome`.
    /// `None` on the dry-run path, which runs no removal to have an outcome —
    /// that path reports `branch_deleted`, its prediction, instead.
    fate: Option<BranchFate>,
}

impl Candidate {
    /// Record what a removal actually did to the branch, replacing the scan's
    /// prediction. Both fields move together — `deletes_branch` is what the
    /// summary counts, `fate` what `--format=json` names — so they are set
    /// here rather than at each call site.
    fn record_fate(&mut self, fate: BranchFate) {
        self.deletes_branch = fate.deleted();
        self.fate = Some(fate);
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

/// The current-worktree candidate held back until every other removal ran
/// (its removal cd's the shell to the primary), with its scan-time plan.
type DeferredCurrent = (Candidate, Option<RemovalPlan>);

/// One unit of work for the removal workers, queued in scan-completion order.
///
/// Skip lines ride the same queue as removals so that with a single worker
/// (`RAYON_NUM_THREADS=1`) prune's per-candidate output keeps one total
/// order — the property the deterministic-output tests pin. With more
/// workers, whole lines from different candidates interleave freely: a skip
/// is a single line, and removal output may span lines but names its branch
/// on each (one `eprintln!` writes one line under one stderr lock).
enum RemovalJob {
    /// Execute a removal via [`try_remove`] and report the outcome.
    Remove {
        candidate: Candidate,
        /// Boxed to keep the variants near parity
        /// (`clippy::large_enum_variant` — `RemovalPlan` is large).
        plan: Box<Option<RemovalPlan>>,
    },
    /// Print an already-formatted skip line.
    PrintSkip(String),
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

/// Build a human-readable count like "3 worktrees & branches".
///
/// Worktree + branch is the default pair (matching progress messages'
/// "worktree & branch" pattern). Unpaired items listed separately.
///
/// Counts what each removal took, not what its kind selected — see
/// [`Candidate::deletes_branch`]. A branch a sibling checkout retained leaves
/// only its worktree, and a retained branch whose worktree was stale metadata
/// leaves only the pruned entry, which reads the same as any other stale
/// entry ("Pruned 1 worktree", matching the `pruned` line above it).
fn prune_summary(candidates: &[Candidate]) -> String {
    let mut worktree_with_branch = 0usize;
    let mut worktree_only = 0usize;
    let mut branch_only = 0usize;
    for c in candidates {
        match &c.kind {
            CandidateKind::BranchOnly if c.deletes_branch => branch_only += 1,
            // Nothing but the stale worktree entry went. An orphan branch
            // that kept its branch never reaches here — with no worktree
            // entry it removed nothing, and `try_remove` excludes the no-op
            // from the removed list entirely.
            CandidateKind::BranchOnly => worktree_only += 1,
            // A stale detached worktree never has a branch.
            CandidateKind::StaleDetached => worktree_only += 1,
            CandidateKind::Current | CandidateKind::Other => {
                if c.deletes_branch {
                    worktree_with_branch += 1;
                } else {
                    worktree_only += 1;
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
    if worktree_only > 0 {
        let noun = if worktree_only == 1 {
            "worktree"
        } else {
            "worktrees"
        };
        parts.push(format!("{worktree_only} {noun}"));
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

/// Loop-invariant context for [`try_remove`]: every field is identical at
/// both call sites in [`step_prune`] — the removal workers and the deferred
/// current worktree (only the `Candidate` varies). Built once and passed by
/// reference.
struct RemovalContext<'a> {
    repo: &'a Repository,
    foreground: bool,
    hook_plan: &'a ApprovedHookPlan,
    /// Coordinates the parallel workers (scan checks and removals, both on
    /// the read side) against the few removals that need exclusivity (write
    /// side — see [`removal_needs_write`]).
    ///
    /// The lock exists for the Windows `.git/config` race: git rewrites
    /// config via lockfile + atomic rename, and a concurrent reader's plain
    /// `fopen` fails on the rename (#2801). Historically every removal held
    /// the write side because branch deletion was `git branch -D`, which
    /// rewrites `.git/config` (it drops the `[branch "<name>"]` section —
    /// even when none exists). The removal chain has since moved to the CAS
    /// `git update-ref -d`, and neither it nor `git worktree remove` (both the
    /// scoped metadata prune and the rename-failure fallback) touches
    /// `.git/config`, so hook-free removals never rewrite it and can run
    /// concurrently. Verified empirically: with `.git/config` made immutable,
    /// only `git branch -D` reports `could not write config file`.
    /// (`git branch -D` remains reachable only via `delete_branch_if_safe`'s
    /// force arm, which prune never uses, and its snapshot-miss arm,
    /// unreachable here because the chain captures the snapshot immediately
    /// before consulting it.)
    check_lock: &'a RwLock<()>,
}

/// Which removals must hold the write side of [`RemovalContext::check_lock`]
/// instead of joining the parallel (read-side) fan-out:
///
/// - **Hook-bearing worktree removals** — the `pre-remove` body runs
///   foreground here, and hook bodies are arbitrary commands (`git branch
///   -D`, `git config`, anything), so it keeps the exclusion every removal
///   had before removals parallelized; the write side also keeps the hook
///   stream and announce lines from interleaving with other candidates'
///   output. (`post-remove`/`post-switch` pipelines spawn detached and
///   always ran outside the lock.)
/// - **`--foreground` worktree removals** — the foreground path runs a TTY
///   trash-cleanup spinner, and concurrent spinners would fight over the
///   cursor.
/// - **The current worktree** — deferred until after the fan-out drains and
///   run alone; write for uniformity with its post-switch hooks.
///
/// Called with the plan in hand, so it asks what the removal will do rather
/// than predicting from the candidate's shape: a `BranchOnly` plan runs neither
/// a hook body nor a spinner, whatever selected it. `StaleDetached` never
/// reaches here — [`try_remove`] prunes its entry and returns.
///
/// Everything else fans out on the read side. The Git worktree-registry calls
/// inside those removals take their own repository-scoped lock; see
/// [`prune_worktree_entry`](Repository::prune_worktree_entry).
fn removal_needs_write(kind: CandidateKind, plan: &RemovalPlan, ctx: &RemovalContext<'_>) -> bool {
    if matches!(kind, CandidateKind::Current) {
        return true;
    }
    match plan {
        // The worktree whose `pre-remove` body and trash-cleanup spinner this
        // removal runs.
        RemovalPlan::Worktree { worktree_path, .. } => {
            ctx.foreground
                || ctx
                    .hook_plan
                    .has_hooks_for(worktree_path, &[HookType::PreRemove, HookType::PostRemove])
        }
        RemovalPlan::BranchOnly { .. } => false,
    }
}

/// Try to remove a candidate immediately. Returns `Ok(Some(fate))` if removed
/// — the executed outcome the summary counts and `--format=json` names —
/// `Ok(None)` if the removal turned out to be a no-op, `Err` on execution
/// error.
///
/// `plan` is the scan-time `prepare_worktree_removal` result from
/// [`check_one`]; only `StaleDetached` candidates arrive without one (their
/// entry is pruned directly here — there is nothing to plan). Scan-time plans
/// may be stale by execution; the pre-rename `ensure_clean` and the
/// branch-delete CAS re-validate what matters — and the returned
/// [`BranchFate`] is how a CAS
/// refusal reaches the summary.
fn try_remove(
    candidate: &Candidate,
    plan: Option<RemovalPlan>,
    ctx: &RemovalContext<'_>,
) -> anyhow::Result<Option<BranchFate>> {
    let _span = Span::new(format!("prune-remove:{}", candidate.label));

    if matches!(candidate.kind, CandidateKind::StaleDetached) {
        // Output side: no exclusive output here (no spinner, no hook stream),
        // so join the parallel read side of `check_lock`.
        let _read = ctx.check_lock.read().unwrap_or_else(|e| e.into_inner());
        // Name the stale entry rather than sweeping the repository, so a
        // sibling whose directory is merely absent right now (unmounted
        // volume, half-finished `mv`) keeps its registration. `gather_check_items`
        // never selects a locked worktree, so the scoped removal can't hit
        // git's lock refusal.
        let path = candidate
            .path
            .as_deref()
            .context("stale detached candidate has no worktree path")?;
        ctx.repo.prune_worktree_entry(path)?;
        // A stale detached entry has no branch to delete.
        return Ok(Some(BranchFate::NotAttempted));
    }

    let plan = plan.context("candidate arrived without a removal plan")?;
    // Read side for the parallel default, write side for the exclusive cases
    // (see `removal_needs_write`). The guards protect `()` — there is no
    // shared state to corrupt, so a poisoned lock is meaningless here.
    // Recover the guard rather than `.expect()`-ing: a panic elsewhere should
    // surface as itself, not as a cascade of secondary poison panics on every
    // later removal/reader.
    let (_read, _write) = if removal_needs_write(candidate.kind, &plan, ctx) {
        (
            None,
            Some(ctx.check_lock.write().unwrap_or_else(|e| e.into_inner())),
        )
    } else {
        (
            Some(ctx.check_lock.read().unwrap_or_else(|e| e.into_inner())),
            None,
        )
    };
    let mut announcer = HookAnnouncer::new(ctx.repo, true);
    // `SynchronousForNonCurrent`: a rename-failure fallback completes inline,
    // so the candidate counts as removed only once the worktree and branch
    // are actually gone — before the final summary prints.
    let execution = if ctx.foreground {
        RemovalExecution::Foreground
    } else {
        RemovalExecution::Background(BackgroundFallbackMode::SynchronousForNonCurrent)
    };
    let fate = handle_remove_output(&plan, execution, ctx.hook_plan, true, &mut announcer)?;
    announcer.flush()?;
    let branch_deleted = fate.deleted();
    // A branch-only candidate that kept its branch removed nothing at all —
    // unless the removal pruned a stale worktree entry, a removal worth
    // counting. Excluding the no-op keeps the summary and JSON honest when a
    // concurrent writer advanced an orphan branch between scan and delete
    // (the per-item retention warning has already told the user).
    if !branch_deleted
        && let RemovalPlan::BranchOnly {
            prune_entry: None, ..
        } = plan
    {
        return Ok(None);
    }
    Ok(Some(fate))
}

/// One candidate skipped because its project hooks aren't yet approved.
/// Carries enough context for the end-of-run hint to print a per-candidate
/// `wt -C <path> remove` line and annotate candidates whose own
/// `.config/wt.toml` differs from the invoking worktree's (so the user knows
/// `wt config approvals add` from current can't approve their hooks).
struct SkippedApproval {
    /// `Some` for candidates whose plan removes a worktree (the only
    /// `(approval required)` source — branch-only plans and stale-detached
    /// entries run no hooks).
    path: Option<PathBuf>,
    /// True when the candidate's `.config/wt.toml` doesn't match the
    /// invoking worktree's bytes — `wt config approvals add` from current
    /// approves only invoking's templates, so this candidate needs the
    /// per-worktree `wt -C <path> remove` form to surface its own hooks.
    differs: bool,
}

/// Per-item parallel work output: integration verdict plus the two filters
/// that decide whether the item becomes a candidate (matches `wt remove`'s
/// gate) or gets skipped with a "younger than" message.
struct CheckOutcome {
    effective_target: String,
    reason: Option<IntegrationReason>,
    /// Removal plan from `prepare_worktree_removal` — the same gate `wt
    /// remove` uses, computed here on the parallel scan so `try_remove`
    /// doesn't re-derive it (a `git status` per worktree) at removal time.
    /// Planning is pure (a stale entry's prune rides the plan as
    /// `prune_entry`; execution performs it), which is what lets this scan
    /// double as `--dry-run` and plan every source. `None` means not
    /// removable (dirty, locked, primary — filtered silently, never reported
    /// as "younger than") — except detached stale entries, which need no
    /// plan: `try_remove` prunes them directly.
    plan: Option<RemovalPlan>,
    /// Whether the item passed the removability gate (see `plan`).
    removable: bool,
    /// What the removal takes, carried onto [`Candidate::deletes_branch`].
    deletes_branch: bool,
    /// `Some(_)` if `min_age` is set and the age could be resolved; the
    /// caller compares against `min_age_duration` to decide on the skip.
    age: Option<Duration>,
}

/// One check item's full parallel work: integration + removability + age.
/// Held under the check-lock read guard at the call site so it never overlaps
/// a write-side removal (see [`removal_needs_write`]).
#[allow(clippy::too_many_arguments)]
fn check_one(
    item: &CheckItem,
    repo: &Repository,
    snapshot: &RefSnapshot,
    integration_target: &str,
    worktrees: &[WorktreeInfo],
    current_path: &Path,
    min_age_duration: Duration,
    now_secs: u64,
) -> anyhow::Result<CheckOutcome> {
    let _span = Span::new(format!("prune-check:{}", item.integration_ref));
    let (effective_target, reason) =
        repo.integration_reason(snapshot, &item.integration_ref, integration_target)?;
    if reason.is_none() {
        return Ok(CheckOutcome {
            effective_target,
            reason,
            plan: None,
            removable: false,
            deletes_branch: false,
            age: None,
        });
    }
    // A detached stale entry can't be planned — `prepare_worktree_removal`'s
    // missing-directory fallback needs a branch to fall back to — and needs
    // no plan: `try_remove` prunes the entry directly.
    let detached_stale = matches!(&item.source,
        CheckSource::Prunable { wt_idx } if worktrees[*wt_idx].branch.is_none());
    let plan = if detached_stale {
        None
    } else {
        match &item.source {
            CheckSource::Orphan => repo
                .prepare_worktree_removal(
                    RemoveTarget::BranchOnly(item.integration_ref.clone()),
                    BranchDeletionMode::SafeDelete,
                    false,
                    current_path,
                    Some(worktrees),
                    Some(snapshot),
                )
                .ok(),
            CheckSource::Linked { wt_idx } | CheckSource::Prunable { wt_idx } => {
                let wt = &worktrees[*wt_idx];
                repo.prepare_worktree_removal(
                    // The scan already knows which worktree this is; naming it
                    // by branch would re-resolve to git's first-listed
                    // checkout, which for a duplicated branch is a different
                    // worktree — and for a stale entry is the live one. A
                    // `WorktreePath` target degrades to branch-only deletion
                    // when the directory is gone, so a stale entry plans its
                    // prune.
                    RemoveTarget::WorktreePath(wt.path.clone()),
                    BranchDeletionMode::SafeDelete,
                    false,
                    current_path,
                    Some(worktrees),
                    Some(snapshot),
                )
                .ok()
            }
        }
    };
    let removable = detached_stale || plan.is_some();
    let deletes_branch = plan.as_ref().is_some_and(RemovalPlan::deletes_branch);
    let age = if min_age_duration > Duration::ZERO {
        match &item.source {
            CheckSource::Linked { wt_idx } => worktree_age(repo, &worktrees[*wt_idx], now_secs)?,
            CheckSource::Orphan => orphan_branch_age(repo, &item.integration_ref, now_secs),
            CheckSource::Prunable { .. } => None,
        }
    } else {
        None
    };
    Ok(CheckOutcome {
        effective_target,
        reason,
        plan,
        removable,
        deletes_branch,
        age,
    })
}

/// Build the metadata fields a [`Candidate`] needs from a check item, shared
/// by the dry-run and live-removal paths.
fn candidate_fields(
    item: &CheckItem,
    repo: &Repository,
    worktrees: &[WorktreeInfo],
    current_root: &Path,
) -> (
    String,
    Option<String>,
    Option<PathBuf>,
    CandidateKind,
    &'static str,
) {
    match &item.source {
        CheckSource::Linked { wt_idx } => {
            let wt = &worktrees[*wt_idx];
            let label = wt.branch.clone().unwrap_or_else(|| {
                let short = repo.short_sha(&wt.head).unwrap_or_else(|_| wt.head.clone());
                format!("(detached {short})")
            });
            let wt_path = dunce::canonicalize(&wt.path).unwrap_or_else(|_| wt.path.clone());
            let kind = if wt_path == *current_root {
                CandidateKind::Current
            } else {
                CandidateKind::Other
            };
            let branch = if wt.detached { None } else { wt.branch.clone() };
            (label, branch, Some(wt.path.clone()), kind, "")
        }
        CheckSource::Prunable { wt_idx } => {
            let wt = &worktrees[*wt_idx];
            let label = wt.branch.clone().unwrap_or_else(|| {
                let short = repo.short_sha(&wt.head).unwrap_or_else(|_| wt.head.clone());
                format!("(detached {short})")
            });
            match &wt.branch {
                // The stale entry's own path, so the removal targets it rather
                // than re-resolving the branch to whichever worktree git lists
                // first — which, for a branch this one duplicates, is a live
                // worktree the prune never selected.
                Some(branch) => (
                    label,
                    Some(branch.clone()),
                    Some(wt.path.clone()),
                    CandidateKind::BranchOnly,
                    " (stale)",
                ),
                None => (
                    label,
                    None,
                    Some(wt.path.clone()),
                    CandidateKind::StaleDetached,
                    " (stale)",
                ),
            }
        }
        CheckSource::Orphan => (
            item.integration_ref.clone(),
            Some(item.integration_ref.clone()),
            None,
            CandidateKind::BranchOnly,
            " (branch only)",
        ),
    }
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
    let is_bare = repo.is_bare().context("checking whether repo is bare")?;

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

        // Unborn worktrees (`git worktree add --orphan`, HEAD = null OID)
        // have no commits to integrate, so `integration_reason` would abort
        // the whole prune scan with `fatal: Needed a single revision` from
        // `git rev-parse` on the unborn branch. Skip them — they're never
        // auto-prunable.
        if !wt.has_commits() {
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

        // Skip the main worktree: `git worktree list` puts it first (a
        // documented guarantee), so no per-worktree `git rev-parse` probe is
        // needed. Bare repos have no main worktree — `list_worktrees()`
        // filters the bare entry, leaving only linked worktrees — so the
        // default-branch check above is their primary guard.
        if idx == 0 && !is_bare {
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
                    "branch_deleted": c.deletes_branch,
                    "reason": info.reason_desc,
                    "target": info.effective_target,
                })
            })
            .collect();
        print_json(&items)?;
        return Ok(());
    }

    // The human preview mirrors the `--format=json` plan above (the worktrees
    // that would be removed), so it goes to stdout. The skipped-young caveat and
    // the "nothing to remove" no-op below are narration the json omits — they
    // stay on stderr. See /writing-user-outputs.
    let mut dry_candidates = Vec::new();
    for (candidate, info) in dry_run_info {
        println!(
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
    println!(
        "{}",
        hint_message(format!(
            "{} would be removed (dry run)",
            prune_summary(&dry_candidates)
        ))
    );
    Ok(())
}

/// Build the pessimistic hook plan up front — every worktree entry in
/// `check_items` × `pre-remove`/`post-remove`, plus the primary × `post-switch`
/// when the current worktree appears in `check_items`. The actual scan may
/// narrow this set; the pessimistic shape is what lets `try_remove` stream
/// without a final approval gate, and what the per-hook approval-state queries
/// resolve against (every hook is selected from the invoking worktree's
/// `.config/wt.toml`, whatever its anchor).
///
/// `Orphan` items name no worktree, so they contribute nothing. `Prunable` ones
/// do, even though the stale entry they usually describe plans a pure branch
/// deletion that runs no hooks: git calls an entry prunable when the `.git` file
/// its metadata points at is gone, which leaves the worktree *directory* still
/// there whenever only that file went (an interrupted `rm -rf`, a copy that
/// skipped dotfiles), and the scan then plans a full worktree removal whose
/// hooks anchor at that path and must resolve (`ApprovedHookPlan::lookup`
/// matches the anchor exactly, by design). For the ordinary stale entry the
/// plan resolves to `BranchOnly` and the registration goes unused.
fn build_pessimistic_plan(
    repo: &Repository,
    check_items: &[CheckItem],
    worktrees: &[WorktreeInfo],
    current_root: &Path,
    project_config: Option<&ProjectConfig>,
    user_config: &UserConfig,
    project_id: Option<&str>,
) -> anyhow::Result<HookPlan> {
    let mut builder = HookPlanBuilder::new(project_config, user_config, project_id);
    let mut has_current = false;
    for item in check_items {
        let (CheckSource::Linked { wt_idx } | CheckSource::Prunable { wt_idx }) = &item.source
        else {
            continue;
        };
        let wt = &worktrees[*wt_idx];
        builder.add(&wt.path, &[HookType::PreRemove, HookType::PostRemove]);
        let wt_path = dunce::canonicalize(&wt.path).unwrap_or_else(|_| wt.path.clone());
        if wt_path == *current_root {
            has_current = true;
        }
    }
    if has_current {
        let primary = repo.home_path()?;
        builder.add(&primary, &[HookType::PostSwitch]);
    }
    Ok(builder.finish())
}

/// Project commands for one hook type that aren't yet approved for this
/// project. Empty means either no project commands exist for that hook, or
/// they're all approved — in both cases the hook doesn't gate any candidate.
fn unapproved_for_hook(
    repo: &Repository,
    hook_type: HookType,
    project_config: Option<&ProjectConfig>,
    user_config: &UserConfig,
    project_id: Option<&str>,
    approvals: &Approvals,
) -> Vec<String> {
    let Ok(home) = repo.home_path() else {
        return Vec::new();
    };
    let mut b = HookPlanBuilder::new(project_config, user_config, project_id);
    b.add(&home, &[hook_type]);
    b.finish()
        .unapproved_project_commands(approvals, project_id)
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

    // Broad set of things that might be prunable. The parallel pass below
    // narrows this down via integration + removability + age, leaving the
    // exact worktrees prune will attempt to remove for the hook approval gate.
    let check_items = {
        let _span = Span::new("prune-gather");
        gather_check_items(&repo, worktrees, default_branch.as_deref())?
    };

    let mut skipped_young: Vec<String> = Vec::new();

    // Streaming dry-run path: scans run in parallel, results are collected and
    // sorted for deterministic output. No removals, no approval — just print.
    if dry_run {
        let check_lock = RwLock::new(());
        let scan_span = Span::new("prune-scan");
        let dry_run_info: Vec<(Candidate, DryRunInfo)> = std::thread::scope(|s| {
            let (tx, rx) = chan::unbounded::<(usize, anyhow::Result<CheckOutcome>)>();
            // Pre-shadow with references so `move` on s.spawn moves only `tx`
            // (so it's dropped when the spawn ends and `rx` can terminate),
            // while the heavy state stays borrowed and remains usable by the
            // main thread.
            let repo_ref = &repo;
            let snapshot_ref = &snapshot;
            let check_items_ref = &check_items;
            let integration_target_ref = integration_target.as_str();
            let current_path_ref = current_root.as_path();
            let check_lock_ref = &check_lock;
            s.spawn(move || {
                check_items_ref
                    .par_iter()
                    .enumerate()
                    .for_each(|(idx, item)| {
                        let outcome = {
                            let _read = check_lock_ref.read().unwrap_or_else(|e| e.into_inner());
                            check_one(
                                item,
                                repo_ref,
                                snapshot_ref,
                                integration_target_ref,
                                worktrees,
                                current_path_ref,
                                min_age_duration,
                                now_secs,
                            )
                        };
                        let _ = tx.send((idx, outcome));
                    });
            });

            let mut info = Vec::new();
            for (idx, outcome) in &rx {
                let outcome = outcome.context("checking branch integration")?;
                let Some(reason) = outcome.reason else {
                    continue;
                };
                if !outcome.removable {
                    continue;
                }
                let item = &check_items[idx];
                let (label, branch, path, kind, suffix) =
                    candidate_fields(item, &repo, worktrees, &current_root);
                if let Some(age) = outcome.age
                    && age < min_age_duration
                {
                    skipped_young.push(label);
                    continue;
                }
                info.push((
                    Candidate {
                        check_idx: idx,
                        branch,
                        label,
                        path,
                        kind,
                        deletes_branch: outcome.deletes_branch,
                        fate: None,
                    },
                    DryRunInfo {
                        reason_desc: reason.description().to_string(),
                        effective_target: outcome.effective_target,
                        suffix,
                    },
                ));
            }
            anyhow::Ok(info)
        })?;
        drop(scan_span);
        return render_dry_run(dry_run_info, skipped_young, min_age, format);
    }

    // Live path: prune NEVER prompts for hook approval inline. Streaming
    // would otherwise deadlock against an approval prompt the moment the
    // first positive arrives, so instead:
    //
    //   * With `--yes`: every project command is auto-approved, the plan
    //     runs in full (matches `wt remove --yes`).
    //   * Without `--yes`: already-approved project commands run; a
    //     candidate whose hooks include any unapproved project command is
    //     SKIPPED with `(approval required)` and a hint to pre-approve
    //     (`wt config approvals add`) or remove individually
    //     (`wt -C <wt> remove`). Unapproved hooks never run silently.
    let project_id_owned = repo.project_identifier().ok();
    let project_id = project_id_owned.as_deref();
    let project_config = repo.load_project_config()?;
    let approvals = if yes {
        Approvals::default()
    } else {
        Approvals::load().context("Failed to load approvals")?
    };
    let (pre_remove_unapproved, post_remove_unapproved, post_switch_unapproved) = if yes {
        (Vec::new(), Vec::new(), Vec::new())
    } else {
        (
            unapproved_for_hook(
                &repo,
                HookType::PreRemove,
                project_config.as_ref(),
                &config,
                project_id,
                &approvals,
            ),
            unapproved_for_hook(
                &repo,
                HookType::PostRemove,
                project_config.as_ref(),
                &config,
                project_id,
                &approvals,
            ),
            unapproved_for_hook(
                &repo,
                HookType::PostSwitch,
                project_config.as_ref(),
                &config,
                project_id,
                &approvals,
            ),
        )
    };
    let pessimistic_plan = build_pessimistic_plan(
        &repo,
        &check_items,
        worktrees,
        &current_root,
        project_config.as_ref(),
        &config,
        project_id,
    )?;
    let hook_plan = if yes {
        // `approve(pid, true)` cannot return None and never prompts.
        pessimistic_plan
            .approve(project_id, true)?
            .unwrap_or_else(ApprovedHookPlan::empty)
    } else {
        // Drops project entries whose commands aren't all approved. The
        // skip-for-approval check above ensures any candidate reaching
        // `try_remove` already has its hooks fully approved.
        pessimistic_plan.approve_readonly(&approvals, project_id)
    };
    // The invoking worktree's project-config bytes — load once so the per-
    // candidate "(different hooks on branch)" annotation in the skip hint
    // can compare each candidate's own `.config/wt.toml` against this
    // baseline. Byte-equal is approximate (whitespace differences flag too)
    // but the result drives a hint, not behavior.
    let invoking_project_bytes = repo
        .project_config_path()
        .ok()
        .flatten()
        .and_then(|p| std::fs::read(p).ok());
    let mut skipped_approval: Vec<SkippedApproval> = Vec::new();

    let check_lock = RwLock::new(());
    let removal_ctx = RemovalContext {
        repo: &repo,
        foreground,
        hook_plan: &hook_plan,
        check_lock: &check_lock,
    };
    // Flipped by the first failing removal: the rest of the queue drains
    // without executing (matching the serial loop's abort-on-first-error),
    // and the error propagates after the workers finish.
    let abort = AtomicBool::new(false);

    // Streaming live path: scans run in parallel and the main thread queues a
    // job for each result as it arrives — a "Skipped ..." line or a removal.
    // Removals execute concurrently on the worker pool (read side of
    // `check_lock`; the exceptional candidates take the write side — see
    // `removal_needs_write`). The current worktree is the one exception to
    // the fan-out: its removal cd's to the primary, so defer it until last.
    let scan_span = Span::new("prune-scan");
    let (removed, deferred_current) = std::thread::scope(
        |s| -> anyhow::Result<(Vec<Candidate>, Option<DeferredCurrent>)> {
            let (tx, rx) = chan::unbounded::<(usize, anyhow::Result<CheckOutcome>)>();
            // Pre-shadow with references so `move` on s.spawn moves only `tx`
            // (so it's dropped when the spawn ends and `rx` can terminate),
            // while the heavy state stays borrowed and remains usable by the
            // main thread.
            let repo_ref = &repo;
            let snapshot_ref = &snapshot;
            let check_items_ref = &check_items;
            let integration_target_ref = integration_target.as_str();
            let current_path_ref = current_root.as_path();
            let check_lock_ref = &check_lock;
            s.spawn(move || {
                check_items_ref
                    .par_iter()
                    .enumerate()
                    .for_each(|(idx, item)| {
                        let outcome = {
                            let _read = check_lock_ref.read().unwrap_or_else(|e| e.into_inner());
                            check_one(
                                item,
                                repo_ref,
                                snapshot_ref,
                                integration_target_ref,
                                worktrees,
                                current_path_ref,
                                min_age_duration,
                                now_secs,
                            )
                        };
                        let _ = tx.send((idx, outcome));
                    });
            });

            // Removal workers, sized like the scan's rayon pool so
            // `RAYON_NUM_THREADS=1` serializes removals too (deterministic
            // output for tests). Results flow back on `done_rx`; the channel
            // closes once every worker has drained the job queue and exited,
            // so draining it below also waits out all in-flight printing.
            let (job_tx, job_rx) = chan::unbounded::<RemovalJob>();
            let (done_tx, done_rx) =
                chan::unbounded::<(Candidate, anyhow::Result<Option<BranchFate>>)>();
            let abort_ref = &abort;
            let removal_ctx_ref = &removal_ctx;
            // Empty check_items → zero workers, correctly: no jobs can queue.
            let workers = rayon::current_num_threads().min(check_items.len());
            for _ in 0..workers {
                let job_rx = job_rx.clone();
                let done_tx = done_tx.clone();
                s.spawn(move || {
                    for job in job_rx {
                        match job {
                            RemovalJob::Remove { candidate, plan } => {
                                // Skip (not print, not remove) once a sibling
                                // failed; queued skip lines below still print
                                // — they were discovered before the failure.
                                if abort_ref.load(Ordering::Relaxed) {
                                    continue;
                                }
                                let result = try_remove(&candidate, *plan, removal_ctx_ref)
                                    .with_context(|| candidate.removal_context());
                                if result.is_err() {
                                    abort_ref.store(true, Ordering::Relaxed);
                                }
                                if done_tx.send((candidate, result)).is_err() {
                                    return;
                                }
                            }
                            RemovalJob::PrintSkip(line) => {
                                // Hold the read side so a skip line can't
                                // land inside a write-side removal's
                                // exclusive output window (spinner, hook
                                // stream).
                                let _read = removal_ctx_ref
                                    .check_lock
                                    .read()
                                    .unwrap_or_else(|e| e.into_inner());
                                eprintln!("{line}");
                            }
                        }
                    }
                });
            }
            drop(job_rx);
            drop(done_tx);

            let mut deferred_current: Option<DeferredCurrent> = None;
            for (idx, outcome) in &rx {
                // A check error fails the whole run — flip `abort` so the
                // workers drain their queue without executing more removals
                // (whose results this early return would silently drop).
                let outcome = outcome
                    .context("checking branch integration")
                    .inspect_err(|_| abort_ref.store(true, Ordering::Relaxed))?;
                let Some(_reason) = outcome.reason else {
                    continue;
                };
                if !outcome.removable {
                    continue;
                }
                let item = &check_items[idx];
                let (label, branch, path, kind, _suffix) =
                    candidate_fields(item, &repo, worktrees, &current_root);
                if let Some(age) = outcome.age
                    && age < min_age_duration
                {
                    let line = info_message(cformat!(
                        "Skipped <bold>{label}</> (younger than {min_age})"
                    ))
                    .to_string();
                    let _ = job_tx.send(RemovalJob::PrintSkip(line));
                    skipped_young.push(label);
                    continue;
                }
                // Keyed on the plan, not the candidate's shape: only a
                // worktree removal runs `pre-remove`/`post-remove`, and a
                // stale entry whose directory turned out to still exist plans
                // one like any other (see `build_pessimistic_plan`). Branch-
                // only plans and detached stale entries run no hooks.
                let needs_approval = match &outcome.plan {
                    Some(RemovalPlan::Worktree { .. }) => {
                        !pre_remove_unapproved.is_empty()
                            || !post_remove_unapproved.is_empty()
                            || (matches!(kind, CandidateKind::Current)
                                && !post_switch_unapproved.is_empty())
                    }
                    Some(RemovalPlan::BranchOnly { .. }) | None => false,
                };
                if needs_approval {
                    let line =
                        info_message(cformat!("Skipped <bold>{label}</> (approval required)"))
                            .to_string();
                    let _ = job_tx.send(RemovalJob::PrintSkip(line));
                    let differs = path.as_deref().is_some_and(|wt_path| {
                        let candidate_bytes =
                            std::fs::read(wt_path.join(".config").join("wt.toml")).ok();
                        candidate_bytes != invoking_project_bytes
                    });
                    skipped_approval.push(SkippedApproval { path, differs });
                    continue;
                }
                let candidate = Candidate {
                    check_idx: idx,
                    label,
                    branch,
                    path,
                    kind,
                    deletes_branch: outcome.deletes_branch,
                    fate: None,
                };
                if matches!(candidate.kind, CandidateKind::Current) {
                    deferred_current = Some((candidate, outcome.plan));
                } else {
                    let _ = job_tx.send(RemovalJob::Remove {
                        candidate,
                        plan: Box::new(outcome.plan),
                    });
                }
            }
            drop(job_tx);

            let mut removed: Vec<Candidate> = Vec::new();
            let mut first_err: Option<anyhow::Error> = None;
            for (mut candidate, result) in &done_rx {
                match result {
                    // Record the executed outcome, not the scan's prediction —
                    // what the summary and `--format=json` report.
                    Ok(Some(fate)) => {
                        candidate.record_fate(fate);
                        removed.push(candidate);
                    }
                    Ok(None) => {}
                    Err(err) if first_err.is_none() => first_err = Some(err),
                    // Concurrent failures (e.g. one Ctrl-C killing every
                    // in-flight child) all carry the same story; report the
                    // first and keep the rest out of the terminal.
                    Err(err) => {
                        tracing::debug!(
                            error = %err,
                            "additional removal failure for {}: {err:#}",
                            candidate.label
                        );
                    }
                }
            }
            if let Some(err) = first_err {
                return Err(err);
            }
            Ok((removed, deferred_current))
        },
    )?;
    drop(scan_span);

    let mut removed = removed;
    // Deterministic order for `--format=json` regardless of which worker
    // finished first (the dry-run path sorts the same way); the deferred
    // current worktree stays last.
    removed.sort_by_key(|c| c.check_idx);
    // Remove deferred current worktree last (cd-to-primary happens here)
    if let Some((mut current, plan)) = deferred_current
        && let Some(fate) =
            try_remove(&current, plan, &removal_ctx).with_context(|| current.removal_context())?
    {
        current.record_fate(fate);
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
                    "branch_outcome": c.fate.map(|f| f.json_outcome()),
                })
            })
            .collect();
        print_json(&items)?;
    } else if removed.is_empty() {
        if skipped_young.is_empty() && skipped_approval.is_empty() {
            eprintln!("{}", info_message("No merged worktrees to remove"));
        }
    } else {
        eprintln!(
            "{}",
            success_message(format!("Pruned {}", prune_summary(&removed)))
        );
    }

    if !skipped_approval.is_empty() {
        for block in approval_hint_blocks(
            &pre_remove_unapproved,
            &post_remove_unapproved,
            &post_switch_unapproved,
            &skipped_approval,
        ) {
            eprintln!("{}", hint_message(block.headline));
            eprintln!("{}", format_with_gutter(&block.body, None));
        }
    }

    Ok(())
}

/// One headline+gutter pair for the `(approval required)` end-of-run hint.
struct ApprovalHintBlock {
    headline: String,
    body: String,
}

/// Build the headline+gutter pairs for `(approval required)` skips: a
/// `wt config approvals add` block listing the unapproved templates from the
/// invoking worktree's config, and a per-worktree `wt -C <path> remove` block
/// for the skipped candidates. Candidates whose own `.config/wt.toml` differs
/// from the invoking worktree get a `(different hooks on branch)` annotation —
/// `wt config approvals add` from current approves only current's templates,
/// so the per-worktree form is the only path for them.
fn approval_hint_blocks(
    pre_remove: &[String],
    post_remove: &[String],
    post_switch: &[String],
    skipped: &[SkippedApproval],
) -> Vec<ApprovalHintBlock> {
    let mut blocks = Vec::new();
    let templates: Vec<String> = [
        ("pre-remove", pre_remove),
        ("post-remove", post_remove),
        ("post-switch", post_switch),
    ]
    .into_iter()
    .flat_map(|(hook, ts)| ts.iter().map(move |t| format!("{hook}: {t}")))
    .collect();
    if !templates.is_empty() {
        blocks.push(ApprovalHintBlock {
            headline: cformat!(
                "Pre-approve hooks for the current worktree with <underline>wt config approvals add</>:"
            ),
            body: templates.join("\n"),
        });
    }
    let wt_lines: Vec<String> = skipped
        .iter()
        .filter_map(|s| {
            let path = s.path.as_ref()?;
            let display = format_path_for_display(path);
            let suffix = if s.differs {
                " (different hooks on branch)"
            } else {
                ""
            };
            Some(format!("wt -C {display} remove{suffix}"))
        })
        .collect();
    if !wt_lines.is_empty() {
        let lead = if templates.is_empty() {
            "Remove"
        } else {
            "Or remove"
        };
        blocks.push(ApprovalHintBlock {
            headline: format!("{lead} specific worktrees individually:"),
            body: wt_lines.join("\n"),
        });
    }
    blocks
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
            deletes_branch: !matches!(kind, CandidateKind::StaleDetached),
            fate: None,
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
    fn approval_hint_blocks_list_templates_and_per_worktree_paths() {
        let skipped = vec![
            SkippedApproval {
                path: Some(PathBuf::from("/wt/a")),
                differs: false,
            },
            SkippedApproval {
                path: Some(PathBuf::from("/wt/b")),
                differs: true,
            },
        ];
        let blocks = approval_hint_blocks(
            &["echo pre".to_string()],
            &[],
            &["echo switch".to_string()],
            &skipped,
        );
        // Strip ANSI so the snapshot stays readable; the underline-styling
        // contract is pinned separately by
        // `approval_hint_headline_uses_underline_for_command_suggestion`.
        use ansi_str::AnsiStr;
        let rendered: Vec<String> = blocks
            .iter()
            .map(|b| format!("[{}]\n{}", b.headline.ansi_strip(), b.body))
            .collect();
        insta::assert_snapshot!(rendered.join("\n---\n"), @r"
        [Pre-approve hooks for the current worktree with wt config approvals add:]
        pre-remove: echo pre
        post-switch: echo switch
        ---
        [Or remove specific worktrees individually:]
        wt -C /wt/a remove
        wt -C /wt/b remove (different hooks on branch)
        ");
    }

    #[test]
    fn approval_hint_headline_uses_underline_for_command_suggestion() {
        let blocks = approval_hint_blocks(
            &["echo pre".to_string()],
            &[],
            &[],
            &[SkippedApproval {
                path: Some(PathBuf::from("/wt/a")),
                differs: false,
            }],
        );
        // The styling guide mandates `<underline>` for commands in hints.
        // Building the expected substring through the same `cformat!` macro
        // sidesteps hardcoded escape codes while still catching a regression
        // to backticks or `<bold>`.
        let expected = cformat!("<underline>wt config approvals add</>");
        assert!(
            blocks[0].headline.contains(&expected),
            "command must be wrapped in underline styling; got: {:?}",
            blocks[0].headline
        );
    }

    #[test]
    fn approval_hint_blocks_drop_template_block_when_no_templates() {
        let skipped = vec![SkippedApproval {
            path: Some(PathBuf::from("/wt/x")),
            differs: false,
        }];
        let blocks = approval_hint_blocks(&[], &[], &[], &skipped);
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            blocks[0].headline,
            "Remove specific worktrees individually:"
        );
        assert_eq!(blocks[0].body, "wt -C /wt/x remove");
    }

    #[test]
    fn prune_summary_counts_each_candidate_kind() {
        let mut detached = candidate(CandidateKind::Other, "det");
        detached.branch = None;
        detached.deletes_branch = false;
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

    /// A branch a sibling worktree still has checked out is retained, so the
    /// removal takes only the worktree — a live one for `Other`, a stale entry
    /// for `BranchOnly`. Counting the kind instead would report branches that
    /// are still there.
    #[test]
    fn prune_summary_counts_a_retained_branch_as_worktree_only() {
        let mut shared_worktree = candidate(CandidateKind::Other, "shared");
        shared_worktree.deletes_branch = false;
        let mut shared_stale = candidate(CandidateKind::BranchOnly, "shared-stale");
        shared_stale.deletes_branch = false;
        assert_eq!(prune_summary(&[shared_worktree]), "1 worktree");
        assert_eq!(prune_summary(&[shared_stale]), "1 worktree");
    }
}
