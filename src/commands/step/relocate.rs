//! `wt step relocate` — move worktrees to expected paths based on the `worktree-path` template.
//!
//! See `src/commands/relocate.rs` for the implementation details and algorithm.

use std::path::PathBuf;

use worktrunk::config::UserConfig;
use worktrunk::git::Repository;
use worktrunk::styling::println;

/// Move worktrees to their expected paths based on the `worktree-path` template.
///
/// See `src/commands/relocate.rs` for the implementation details and algorithm.
///
/// # Flags
///
/// | Flag | Purpose |
/// |------|---------|
/// | `--dry-run` | Show what would be moved without moving |
/// | `--commit` | Auto-commit dirty worktrees with LLM-generated messages before relocating |
/// | `--clobber` | Move non-worktree paths out of the way (`<path>.bak-<timestamp>`) |
/// | `[branches...]` | Specific branches to relocate (default: all mismatched) |
pub fn step_relocate(
    branches: Vec<String>,
    dry_run: bool,
    commit: bool,
    clobber: bool,
    format: crate::cli::SwitchFormat,
) -> anyhow::Result<()> {
    use super::super::relocate::{
        GatherResult, RelocationExecutor, ValidationResult, gather_candidates, show_all_skipped,
        show_dry_run_preview, show_no_relocations_needed, show_summary, validate_candidates,
    };

    let json_mode = format == crate::cli::SwitchFormat::Json;

    let repo = Repository::current()?;
    let config = UserConfig::load()?;
    let default_branch = repo.default_branch().unwrap_or_default();

    // Validate default branch early - needed for main worktree relocation
    if default_branch.is_empty() {
        anyhow::bail!(
            "Cannot determine default branch; set with: wt config state default-branch set main"
        );
    }
    let repo_path = repo.repo_path()?.to_path_buf();

    // Phase 1: Gather candidates (worktrees not at expected paths)
    let GatherResult {
        candidates,
        template_error_branches,
    } = gather_candidates(&repo, &config, &branches)?;

    let template_skips: Vec<super::super::relocate::SkippedEntry> = template_error_branches
        .iter()
        .map(|b| super::super::relocate::SkippedEntry {
            branch: b.clone(),
            reason: "template_error",
        })
        .collect();

    if candidates.is_empty() {
        if json_mode {
            print_relocate_json(&[], &template_skips, dry_run)?;
        } else {
            show_no_relocations_needed(template_error_branches.len());
        }
        return Ok(());
    }

    // Dry run: show preview and exit
    if dry_run {
        if json_mode {
            let planned: Vec<_> = candidates
                .iter()
                .map(|c| RelocatedEntryView {
                    branch: c.branch().to_string(),
                    from: c.wt.path.clone(),
                    to: c.expected_path.clone(),
                })
                .collect();
            print_relocate_json(&planned, &template_skips, true)?;
        } else {
            show_dry_run_preview(&candidates);
        }
        return Ok(());
    }

    // Phase 2: Validate candidates (check locked/dirty, optionally auto-commit)
    let ValidationResult {
        validated,
        skipped: validation_skipped,
    } = validate_candidates(&repo, &config, candidates, commit, &repo_path)?;

    if validated.is_empty() {
        if json_mode {
            let mut all_skipped = template_skips;
            all_skipped.extend(validation_skipped);
            print_relocate_json(&[], &all_skipped, false)?;
        } else {
            show_all_skipped(validation_skipped.len());
        }
        return Ok(());
    }

    // Phase 3 & 4: Create executor (classifies targets) and execute relocations
    let mut executor = RelocationExecutor::new(&repo, validated, clobber)?;
    let cwd = std::env::current_dir().ok();
    executor.execute(&default_branch, cwd.as_deref())?;

    if json_mode {
        let relocated_views: Vec<RelocatedEntryView> = executor
            .relocated_entries
            .iter()
            .map(|e| RelocatedEntryView {
                branch: e.branch.clone(),
                from: e.from.clone(),
                to: e.to.clone(),
            })
            .collect();
        let mut all_skipped = template_skips;
        all_skipped.extend(validation_skipped);
        all_skipped.extend(executor.skipped_entries);
        print_relocate_json(&relocated_views, &all_skipped, false)?;
    } else {
        let total_skipped = validation_skipped.len() + executor.skipped_count();
        show_summary(executor.relocated_count(), total_skipped);
    }

    Ok(())
}

/// Internal projection of a relocation event for JSON output.
struct RelocatedEntryView {
    branch: String,
    from: PathBuf,
    to: PathBuf,
}

fn print_relocate_json(
    entries: &[RelocatedEntryView],
    skipped: &[super::super::relocate::SkippedEntry],
    dry_run: bool,
) -> anyhow::Result<()> {
    let payload = serde_json::json!({
        "dry_run": dry_run,
        "entries": entries.iter().map(|e| serde_json::json!({
            "branch": e.branch,
            "from": e.from,
            "to": e.to,
        })).collect::<Vec<_>>(),
        "skipped": skipped.iter().map(|s| serde_json::json!({
            "branch": s.branch,
            "reason": s.reason,
        })).collect::<Vec<_>>(),
    });
    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}
