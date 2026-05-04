//! `wt step prune` — remove worktrees and branches integrated into the default branch.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use color_print::cformat;
use crossbeam_channel as chan;
use rayon::prelude::*;
use worktrunk::HookType;
use worktrunk::config::UserConfig;
use worktrunk::git::{BranchDeletionMode, Repository, WorktreeInfo};
use worktrunk::styling::{eprintln, hint_message, info_message, println, success_message};

use super::super::command_approval::approve_or_skip;
use super::super::context::CommandEnv;
use super::super::hooks::HookAnnouncer;
use super::super::repository_ext::{RemoveTarget, RepositoryCliExt};
use crate::output::handle_remove_output;

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

    // Gather candidates: integrated worktrees + integrated branch-only refs
    struct Candidate {
        /// Original index in check_items (for deterministic output ordering)
        check_idx: usize,
        /// Branch name (None for detached HEAD worktrees)
        branch: Option<String>,
        /// Display label: branch name or abbreviated commit SHA
        label: String,
        /// Worktree path (for Path-based removal of detached worktrees)
        path: Option<PathBuf>,
        /// Current worktree, other worktree, or branch-only (no worktree)
        kind: CandidateKind,
    }
    enum CandidateKind {
        Current,
        Other,
        BranchOnly,
    }

    impl CandidateKind {
        fn as_str(&self) -> &'static str {
            match self {
                CandidateKind::Current => "current",
                CandidateKind::Other => "worktree",
                CandidateKind::BranchOnly => "branch_only",
            }
        }
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
            match (&c.kind, &c.branch) {
                (CandidateKind::BranchOnly, _) => branch_only += 1,
                (CandidateKind::Current | CandidateKind::Other, Some(_)) => {
                    worktree_with_branch += 1;
                }
                (CandidateKind::Current | CandidateKind::Other, None) => {
                    detached_worktree += 1;
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

    // For non-dry-run, approve hooks upfront so we can remove inline.
    let run_hooks = if dry_run {
        false // unused in dry-run path
    } else {
        let env = CommandEnv::for_action_branchless()?;
        let ctx = env.context(yes);
        approve_or_skip(
            &ctx,
            &[
                HookType::PreRemove,
                HookType::PostRemove,
                HookType::PostSwitch,
            ],
            "Commands declined, continuing removal",
        )?
    };

    let mut removed: Vec<Candidate> = Vec::new();
    let mut deferred_current: Option<Candidate> = None;
    let mut skipped_young: Vec<String> = Vec::new();
    // Track branches seen via worktree entries so we don't double-count
    // in the orphan branch scan below.
    let mut seen_branches: std::collections::HashSet<String> = std::collections::HashSet::new();

    /// Try to remove a candidate immediately. Returns Ok(true) if removed,
    /// Ok(false) if skipped (preparation error), Err on execution error.
    fn try_remove(
        candidate: &Candidate,
        repo: &Repository,
        config: &UserConfig,
        foreground: bool,
        run_hooks: bool,
        worktrees: &[WorktreeInfo],
        snapshot: &worktrunk::git::RefSnapshot,
    ) -> anyhow::Result<bool> {
        let target = match candidate.kind {
            CandidateKind::Current => RemoveTarget::Current,
            CandidateKind::BranchOnly => RemoveTarget::Branch(
                candidate
                    .branch
                    .as_ref()
                    .context("BranchOnly candidate missing branch")?,
            ),
            CandidateKind::Other => match &candidate.branch {
                Some(branch) => RemoveTarget::Branch(branch),
                None => RemoveTarget::Path(
                    candidate
                        .path
                        .as_ref()
                        .context("detached candidate missing path")?,
                ),
            },
        };
        let plan = match repo.prepare_worktree_removal(
            target,
            BranchDeletionMode::SafeDelete,
            false,
            config,
            None,
            Some(worktrees),
            Some(snapshot),
        ) {
            Ok(plan) => plan,
            Err(_) => {
                // prepare_worktree_removal is the gate: if the worktree can't
                // be removed (dirty, locked, etc.), it's simply not selected.
                return Ok(false);
            }
        };
        let mut announcer = HookAnnouncer::new(repo, config, true);
        handle_remove_output(&plan, foreground, run_hooks, true, &mut announcer)?;
        announcer.flush()?;
        Ok(true)
    }

    enum CheckSource {
        /// Worktree with directory gone (prunable)
        Prunable { branch: String },
        /// Linked worktree
        Linked { wt_idx: usize },
        /// Local branch without a worktree entry
        Orphan,
    }

    struct CheckItem {
        integration_ref: String,
        source: CheckSource,
    }

    let mut check_items: Vec<CheckItem> = Vec::new();

    for (idx, wt) in worktrees.iter().enumerate() {
        if let Some(branch) = &wt.branch {
            seen_branches.insert(branch.clone());
        }

        if wt.locked.is_some() {
            continue;
        }

        if let Some(branch) = &wt.branch
            && default_branch.as_deref() == Some(branch.as_str())
        {
            continue;
        }

        if wt.is_prunable() {
            if let Some(branch) = &wt.branch {
                check_items.push(CheckItem {
                    integration_ref: branch.clone(),
                    source: CheckSource::Prunable {
                        branch: branch.clone(),
                    },
                });
            }
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
        if default_branch.as_deref() == Some(branch.as_str()) {
            continue;
        }
        check_items.push(CheckItem {
            integration_ref: branch,
            source: CheckSource::Orphan,
        });
    }

    // Parallel integration checks with inline removals.
    //
    // Spawn integration checks on a background thread via rayon par_iter,
    // sending each result through a channel as it completes. The main thread
    // processes results as they arrive: age-filtering, printing "Skipped"
    // messages, and removing candidates immediately. This overlaps integration
    // checking with removal — output appears as soon as the first check
    // completes instead of waiting for all checks to finish.
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
    let snapshot_arc = std::sync::Arc::new(snapshot);
    let snapshot_for_thread = std::sync::Arc::clone(&snapshot_arc);
    std::thread::spawn(move || {
        integration_refs
            .into_par_iter()
            .enumerate()
            .for_each(|(idx, ref_name)| {
                let result =
                    repo_clone.integration_reason(&snapshot_for_thread, &ref_name, &target);
                let _ = tx.send((idx, result));
            });
    });

    // Collect integration context alongside candidates for dry-run display.
    struct DryRunInfo {
        reason_desc: String,
        effective_target: String,
        suffix: &'static str,
    }
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
            if min_age_duration > Duration::ZERO {
                let wt_tree = repo.worktree_at(&wt.path);
                let git_dir = wt_tree.git_dir().context("resolving worktree git dir")?;
                let metadata = fs::metadata(&git_dir).context("Failed to read worktree git dir")?;
                let created = metadata.created().or_else(|_| {
                    fs::metadata(git_dir.join("commondir")).and_then(|m| m.modified())
                });
                if let Ok(created) = created
                    && let Ok(created_epoch) = created.duration_since(std::time::UNIX_EPOCH)
                {
                    let age = Duration::from_secs(now_secs.saturating_sub(created_epoch.as_secs()));
                    if age < min_age_duration {
                        if !dry_run {
                            eprintln!(
                                "{}",
                                info_message(format!("Skipped {label} (younger than {min_age})"))
                            );
                        }
                        skipped_young.push(label);
                        continue;
                    }
                }
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
            } else if is_current {
                deferred_current = Some(candidate);
            } else if try_remove(
                &candidate,
                &repo,
                &config,
                foreground,
                run_hooks,
                worktrees,
                &snapshot_arc,
            )
            .with_context(|| format!("removing worktree for {}", candidate.label))?
            {
                removed.push(candidate);
            }
            continue;
        }

        // Branch-only candidates: prunable (stale worktree) and orphan branches
        let (branch, suffix) = match &item.source {
            CheckSource::Prunable { branch } => (branch, " (stale)"),
            CheckSource::Orphan => (&item.integration_ref, " (branch only)"),
            CheckSource::Linked { .. } => unreachable!(),
        };

        // Age check for orphan branches via reflog creation timestamp
        if matches!(&item.source, CheckSource::Orphan) && min_age_duration > Duration::ZERO {
            let ref_name = format!("refs/heads/{branch}");
            if let Ok(stdout) = repo.run_command(&["reflog", "show", "--format=%ct", &ref_name])
                && let Some(created_epoch) = stdout
                    .trim()
                    .lines()
                    .last()
                    .and_then(|s| s.parse::<u64>().ok())
            {
                let age = Duration::from_secs(now_secs.saturating_sub(created_epoch));
                if age < min_age_duration {
                    if !dry_run {
                        eprintln!(
                            "{}",
                            info_message(format!("Skipped {branch} (younger than {min_age})"))
                        );
                    }
                    skipped_young.push(branch.clone());
                    continue;
                }
            }
        }

        let candidate = Candidate {
            check_idx: idx,
            label: branch.clone(),
            branch: Some(branch.clone()),
            path: None,
            kind: CandidateKind::BranchOnly,
        };
        if dry_run {
            let info = DryRunInfo {
                reason_desc: reason.description().to_string(),
                effective_target,
                suffix,
            };
            dry_run_info.push((candidate, info));
        } else if try_remove(
            &candidate,
            &repo,
            &config,
            foreground,
            run_hooks,
            worktrees,
            &snapshot_arc,
        )
        .with_context(|| format!("removing worktree for {}", candidate.label))?
        {
            removed.push(candidate);
        }
    }

    if dry_run {
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
            let names = skipped_young.join(", ");
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
        return Ok(());
    }

    // Remove deferred current worktree last (cd-to-primary happens here)
    if let Some(current) = deferred_current
        && try_remove(
            &current,
            &repo,
            &config,
            foreground,
            run_hooks,
            worktrees,
            &snapshot_arc,
        )
        .with_context(|| format!("removing worktree for {}", current.label))?
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
