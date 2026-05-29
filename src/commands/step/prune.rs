//! `wt step prune` — remove worktrees and branches integrated into the default branch.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
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

use super::super::hook_plan::{ApprovedHookPlan, HookPlan, HookPlanBuilder};
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
    /// Serializes the parallel check readers against the removal writer.
    /// `try_remove` takes the write guard while the background scan workers
    /// hold a read guard around `integration_reason` and
    /// `prepare_worktree_removal` — load-bearing for the Windows `.git/config`
    /// race (rename-fallback rewrites it via lockfile+rename, readers fan out
    /// child git processes that read it).
    check_lock: &'a RwLock<()>,
}

/// Try to remove a candidate immediately. Returns Ok(true) if removed,
/// Ok(false) if skipped (preparation error), Err on execution error.
fn try_remove(candidate: &Candidate, ctx: &RemovalContext<'_>) -> anyhow::Result<bool> {
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

/// One candidate skipped because its project hooks aren't yet approved.
/// Carries enough context for the end-of-run hint to print a per-candidate
/// `wt -C <path> remove` line and annotate candidates whose own
/// `.config/wt.toml` differs from the invoking worktree's (so the user knows
/// `wt config approvals add` from current can't approve their hooks).
struct SkippedApproval {
    /// `Some` for `Linked` candidates (the only `(approval required)`
    /// source — branch-only and stale-detached don't run hooks).
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
    /// Result of `prepare_worktree_removal` — the same gate `wt remove` uses.
    /// Dirty, locked, and primary worktrees end up `false` and are filtered
    /// silently, never reported as "younger than" or processed downstream.
    removable: bool,
    /// `Some(_)` if `min_age` is set and the age could be resolved; the
    /// caller compares against `min_age_duration` to decide on the skip.
    age: Option<Duration>,
}

/// One check item's full parallel work: integration + removability + age.
/// Held under the check-lock read guard at the call site to serialize against
/// `try_remove` rewriting `.git/config` on the Windows rename-fallback path.
#[allow(clippy::too_many_arguments)]
fn check_one(
    item: &CheckItem,
    repo: &Repository,
    snapshot: &RefSnapshot,
    integration_target: &str,
    config: &UserConfig,
    worktrees: &[WorktreeInfo],
    min_age_duration: Duration,
    now_secs: u64,
) -> anyhow::Result<CheckOutcome> {
    let (effective_target, reason) =
        repo.integration_reason(snapshot, &item.integration_ref, integration_target)?;
    if reason.is_none() {
        return Ok(CheckOutcome {
            effective_target,
            reason,
            removable: false,
            age: None,
        });
    }
    let removable = match &item.source {
        CheckSource::Prunable { .. } | CheckSource::Orphan => true,
        CheckSource::Linked { wt_idx } => {
            let wt = &worktrees[*wt_idx];
            let target = match &wt.branch {
                Some(b) if !wt.detached => RemoveTarget::Branch(b),
                _ => RemoveTarget::Path(&wt.path),
            };
            repo.prepare_worktree_removal(
                target,
                BranchDeletionMode::SafeDelete,
                false,
                config,
                None,
                Some(worktrees),
                Some(snapshot),
            )
            .is_ok()
        }
    };
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
        removable,
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
                Some(branch) => (
                    label,
                    Some(branch.clone()),
                    None,
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

/// Build the pessimistic hook plan up front — every linked worktree in
/// `check_items` × `pre-remove`/`post-remove`, plus the primary × `post-switch`
/// when the current worktree appears in `check_items`. The actual scan may
/// narrow this set; the pessimistic shape is what lets `try_remove` stream
/// without a final approval gate, and what the per-hook approval-state queries
/// resolve against (every hook is selected from the invoking worktree's
/// `.config/wt.toml`, whatever its anchor).
///
/// `pre-remove`/`post-remove`/`post-switch` never run for `BranchOnly` /
/// `StaleDetached` / `Orphan` candidates — those are pure branch deletions —
/// so they contribute nothing to the plan.
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
        let CheckSource::Linked { wt_idx } = &item.source else {
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
    let check_items = gather_check_items(&repo, worktrees, default_branch.as_deref())?;

    let mut skipped_young: Vec<String> = Vec::new();

    // Streaming dry-run path: scans run in parallel, results are collected and
    // sorted for deterministic output. No removals, no approval — just print.
    if dry_run {
        let check_lock = RwLock::new(());
        let mut dry_run_info: Vec<(Candidate, DryRunInfo)> = std::thread::scope(|s| {
            let (tx, rx) = chan::unbounded::<(usize, anyhow::Result<CheckOutcome>)>();
            // Pre-shadow with references so `move` on s.spawn moves only `tx`
            // (so it's dropped when the spawn ends and `rx` can terminate),
            // while the heavy state stays borrowed and remains usable by the
            // main thread.
            let repo_ref = &repo;
            let snapshot_ref = &snapshot;
            let config_ref = &config;
            let check_items_ref = &check_items;
            let integration_target_ref = integration_target.as_str();
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
                                config_ref,
                                worktrees,
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
        dry_run_info.sort_by_key(|(c, _)| c.check_idx);
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
        config: &config,
        foreground,
        hook_plan: &hook_plan,
        worktrees,
        snapshot: &snapshot,
        check_lock: &check_lock,
    };

    // Streaming live path: scans run in parallel and the main thread acts on
    // each result as it arrives — print "Skipped (younger than X)" or call
    // `try_remove` immediately for positives. The current worktree is the one
    // exception: its removal cd's to the primary, so defer it until last.
    let (removed, deferred_current) =
        std::thread::scope(|s| -> anyhow::Result<(Vec<Candidate>, Option<Candidate>)> {
            let (tx, rx) = chan::unbounded::<(usize, anyhow::Result<CheckOutcome>)>();
            // Pre-shadow with references so `move` on s.spawn moves only `tx`
            // (so it's dropped when the spawn ends and `rx` can terminate),
            // while the heavy state stays borrowed and remains usable by the
            // main thread's removal calls.
            let repo_ref = &repo;
            let snapshot_ref = &snapshot;
            let config_ref = &config;
            let check_items_ref = &check_items;
            let integration_target_ref = integration_target.as_str();
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
                                config_ref,
                                worktrees,
                                min_age_duration,
                                now_secs,
                            )
                        };
                        let _ = tx.send((idx, outcome));
                    });
            });

            let mut removed: Vec<Candidate> = Vec::new();
            let mut deferred_current: Option<Candidate> = None;
            for (idx, outcome) in &rx {
                let outcome = outcome.context("checking branch integration")?;
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
                    eprintln!(
                        "{}",
                        info_message(cformat!(
                            "Skipped <bold>{label}</> (younger than {min_age})"
                        ))
                    );
                    skipped_young.push(label);
                    continue;
                }
                let needs_approval = match kind {
                    CandidateKind::Other => {
                        !pre_remove_unapproved.is_empty() || !post_remove_unapproved.is_empty()
                    }
                    CandidateKind::Current => {
                        !pre_remove_unapproved.is_empty()
                            || !post_remove_unapproved.is_empty()
                            || !post_switch_unapproved.is_empty()
                    }
                    // Pure branch deletions don't run hooks.
                    CandidateKind::BranchOnly | CandidateKind::StaleDetached => false,
                };
                if needs_approval {
                    eprintln!(
                        "{}",
                        info_message(cformat!("Skipped <bold>{label}</> (approval required)"))
                    );
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
                };
                if matches!(candidate.kind, CandidateKind::Current) {
                    deferred_current = Some(candidate);
                } else if try_remove(&candidate, &removal_ctx)
                    .with_context(|| candidate.removal_context())?
                {
                    removed.push(candidate);
                }
            }
            Ok((removed, deferred_current))
        })?;

    let mut removed = removed;
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
