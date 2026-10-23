//! `wt step relocate` — move worktrees to expected paths based on the `worktree-path` template.
//!
//! See `src/commands/relocate.rs` for the implementation details and algorithm.

use anyhow::Context;
use color_print::cformat;

use worktrunk::config::UserConfig;
use worktrunk::git::Repository;

use crate::output::print_json;

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
/// | `--clobber` | Move non-worktree paths out of the way (`<path>.bak.<timestamp>`) |
/// | `[branches...]` | Specific branches to relocate (default: all mismatched) |
pub fn step_relocate(
    branches: Vec<String>,
    dry_run: bool,
    commit: bool,
    clobber: bool,
    format: crate::cli::SwitchFormat,
) -> anyhow::Result<()> {
    use super::super::relocate::{
        GatherResult, RelocatedEntry, RelocationExecutor, ValidationResult, gather_candidates,
        show_all_skipped, show_dry_run_preview, show_no_relocations_needed, show_summary,
        validate_candidates,
    };

    let json_mode = format == crate::cli::SwitchFormat::Json;

    let repo = Repository::current()?;
    let config = UserConfig::load().context("Failed to load config")?;
    let default_branch = repo.default_branch().unwrap_or_default();

    // Validate default branch early - needed for main worktree relocation
    if default_branch.is_empty() {
        anyhow::bail!(cformat!(
            "Cannot determine default branch; to configure one, run <bold>wt config state default-branch set BRANCH</>"
        ));
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
            return print_relocations(&[], &template_skips, &[], dry_run);
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
                .map(|c| RelocatedEntry {
                    branch: c.branch().to_string(),
                    from: c.wt.path.clone(),
                    to: c.expected_path.clone(),
                })
                .collect();
            return print_relocations(&planned, &template_skips, &[], true);
        } else {
            show_dry_run_preview(&candidates);
        }
        return Ok(());
    }

    // Phase 2: Validate candidates (check locked/dirty, optionally auto-commit)
    // Approve the project commit append once for the whole batch so per-worktree
    // commits share a single approval prompt rather than one per worktree.
    // Skip approval when the LLM isn't configured for this project — the
    // fallback message generator doesn't render the prompt template.
    let project_append = if commit {
        use super::super::command_approval::approve_commit_template_append;
        use super::super::command_executor::CommandContext;
        let project_id = repo.project_identifier().ok();
        let commit_config = config.commit_generation(project_id.as_ref());
        if commit_config.is_configured() {
            let ctx = CommandContext::new(&repo, &config, None, &repo_path, false);
            approve_commit_template_append(&ctx)?
        } else {
            None
        }
    } else {
        None
    };
    let ValidationResult {
        validated,
        skipped: validation_skipped,
        commits,
    } = match validate_candidates(
        &repo,
        &config,
        candidates,
        commit,
        &repo_path,
        project_append.as_deref(),
    ) {
        Ok(validation) => validation,
        Err(failure) => {
            if json_mode && let Some(validation) = failure.result {
                print_relocations(
                    &[],
                    template_skips.iter().chain(&validation.skipped),
                    &validation.commits,
                    false,
                )?;
            }
            return Err(failure.error);
        }
    };

    if validated.is_empty() {
        if json_mode {
            return print_relocations(
                &[],
                template_skips.iter().chain(&validation_skipped),
                &commits,
                false,
            );
        } else {
            // Include template-error branches — the JSON path folds them into
            // `all_skipped`, so the human count must match.
            show_all_skipped(validation_skipped.len() + template_skips.len());
        }
        return Ok(());
    }

    // Phase 3 & 4: Create executor (classifies targets) and execute relocations
    let mut executor = match RelocationExecutor::new(&repo, validated, clobber) {
        Ok(executor) => executor,
        Err(error) => {
            if json_mode {
                print_relocations(
                    &[],
                    template_skips.iter().chain(&validation_skipped),
                    &commits,
                    false,
                )?;
            }
            return Err(error);
        }
    };
    // Where the user's shell stands — inherited from the parent `wt` when this
    // runs inside an alias or hook body, whose process cwd is the worktree
    // root (#3723).
    let cwd = worktrunk::shell_exec::shell_cwd();
    let execution = executor.execute(&default_branch, cwd.as_deref());

    if json_mode {
        print_relocations(
            &executor.relocated_entries,
            template_skips
                .iter()
                .chain(&validation_skipped)
                .chain(&executor.skipped_entries),
            &commits,
            false,
        )?;
        return execution;
    } else {
        execution?;
        // Template-error branches count as skips too — the JSON path folds
        // them into `all_skipped`, so keep the human total consistent.
        let total_skipped =
            template_skips.len() + validation_skipped.len() + executor.skipped_count();
        show_summary(executor.relocated_count(), total_skipped);
    }

    Ok(())
}

fn print_relocations<'a>(
    entries: &[super::super::relocate::RelocatedEntry],
    skipped: impl IntoIterator<Item = &'a super::super::relocate::SkippedEntry>,
    commits: &[super::super::relocate::RelocationCommit],
    dry_run: bool,
) -> anyhow::Result<()> {
    #[derive(serde::Serialize)]
    struct Output<'a> {
        dry_run: bool,
        entries: &'a [super::super::relocate::RelocatedEntry],
        skipped: Vec<&'a super::super::relocate::SkippedEntry>,
        commits: &'a [super::super::relocate::RelocationCommit],
    }
    print_json(&Output {
        dry_run,
        entries,
        skipped: skipped.into_iter().collect(),
        commits,
    })
}
