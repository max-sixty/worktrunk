//! Prune command: batch removal of integrated branches and prunable worktrees.

mod collect;
mod execute;
mod filter;

use anyhow::Result;
use worktrunk::config::UserConfig;
use worktrunk::git::{IntegrationReason, Repository};

use std::path::PathBuf;

/// Options for the prune command.
#[derive(Debug)]
pub(crate) struct PruneOptions {
    /// Override integration target (defaults to default branch)
    pub target: Option<String>,
    /// Include unmerged branches (requires --force)
    pub force: bool,
    /// Only remove branches matching this pattern
    pub pattern: Option<String>,
    /// Exclude branches matching these patterns
    pub exclude: Vec<String>,
    /// Show what would be removed without removing
    pub dry_run: bool,
    /// Skip confirmation prompt
    pub yes: bool,
}

/// Reason why a candidate can be pruned.
#[derive(Debug, Clone)]
pub enum PruneReason {
    /// Branch is integrated into target
    Integrated(IntegrationReason, String),
    /// Worktree directory is missing (prunable)
    Prunable,
}

/// A candidate for pruning.
#[derive(Debug)]
pub struct PruneCandidate {
    /// Branch name
    pub branch: String,
    /// Worktree path (None if directory is missing)
    #[allow(dead_code)]
    worktree_path: Option<PathBuf>,
    /// Reason this candidate can be pruned
    pub reason: PruneReason,
    /// Cached integration reason for execute phase
    integration_reason: Option<IntegrationReason>,
}

/// Main entry point for the prune command.
pub(crate) fn handle_prune(opts: PruneOptions, config: &UserConfig) -> Result<()> {
    let repo = Repository::current()?;

    // Phase 1: Collect all integrated and prunable candidates
    let candidates = collect::collect_candidates(&repo, &opts)?;

    // Phase 2: Apply safety filters and user criteria
    let candidates = filter::apply_filters(candidates, &opts, &repo)?;

    // Phase 3: Show confirmation
    if !show_confirmation(&candidates, &opts)? {
        return Ok(());
    }

    // Phase 4: Execute removals
    let result = execute::execute_prune(candidates, config)?;

    // Phase 5: Report results
    report_results(&result);

    // Exit with error code if any failures
    if !result.failed.is_empty() {
        std::process::exit(1);
    }

    Ok(())
}

/// Show confirmation prompt or dry-run output.
fn show_confirmation(candidates: &[PruneCandidate], opts: &PruneOptions) -> Result<bool> {
    use std::io::IsTerminal;
    use std::io::{self, Write};
    use worktrunk::git::GitError;
    use worktrunk::styling::{eprintln, info_message, prompt_message, stderr};

    if opts.dry_run {
        show_dry_run_output(candidates);
        return Ok(false); // Don't proceed
    }

    if candidates.is_empty() {
        eprintln!(
            "{}",
            info_message("No integrated branches or prunable worktrees found")
        );
        return Ok(false);
    }

    // Show what will be removed
    let integrated: Vec<_> = candidates
        .iter()
        .filter(|c| matches!(c.reason, PruneReason::Integrated(..)))
        .collect();
    let prunable: Vec<_> = candidates
        .iter()
        .filter(|c| matches!(c.reason, PruneReason::Prunable))
        .collect();

    if !integrated.is_empty() {
        eprintln!("\nIntegrated branches:");
        for c in integrated {
            if let PruneReason::Integrated(reason, target) = &c.reason {
                eprintln!("  {} → {} ({})", c.branch, target, reason_display(reason));
            }
        }
    }

    if !prunable.is_empty() {
        eprintln!("\nPrunable worktrees:");
        for c in prunable {
            eprintln!("  {} (directory missing)", c.branch);
        }
    }

    if opts.yes {
        return Ok(true);
    }

    // Check if stdin is a TTY before attempting to prompt
    if !io::stdin().is_terminal() {
        return Err(GitError::NotInteractive.into());
    }

    eprintln!();
    use color_print::cformat;
    eprint!(
        "{} ",
        prompt_message(cformat!(
            "Remove {} items? <bold>[y/N]</>",
            candidates.len()
        ))
    );
    stderr().flush()?;

    let mut response = String::new();
    io::stdin().read_line(&mut response)?;
    eprintln!(); // End the prompt line

    let response = response.trim().to_lowercase();
    Ok(response == "y" || response == "yes")
}

/// Show dry-run output.
fn show_dry_run_output(candidates: &[PruneCandidate]) {
    use worktrunk::styling::eprintln;

    if candidates.is_empty() {
        eprintln!("No integrated branches or prunable worktrees found");
        return;
    }

    eprintln!("Would remove {} items:", candidates.len());

    for c in candidates {
        match &c.reason {
            PruneReason::Integrated(reason, target) => {
                eprintln!("  {} → {} ({})", c.branch, target, reason_display(reason));
            }
            PruneReason::Prunable => {
                eprintln!("  {} (directory missing)", c.branch);
            }
        }
    }
}

/// Display integration reason in human-readable form.
fn reason_display(reason: &IntegrationReason) -> &'static str {
    match reason {
        IntegrationReason::Ancestor => "ancestor",
        IntegrationReason::SameCommit => "same commit",
        IntegrationReason::TreesMatch => "trees match",
        IntegrationReason::NoAddedChanges => "no added changes",
        IntegrationReason::MergeAddsNothing => "merge adds nothing",
    }
}

/// Report final results.
fn report_results(result: &execute::PruneResult) {
    use color_print::cformat;
    use worktrunk::styling::{eprintln, error_message, warning_message};

    eprintln!();
    if result.failed.is_empty() {
        eprintln!(
            "{}",
            cformat!("<green>✓</> Removed {} items", result.removed.len())
        );
    } else {
        eprintln!(
            "{}",
            warning_message(format!(
                "Removed {} of {} items ({} failed)",
                result.removed.len(),
                result.removed.len() + result.failed.len(),
                result.failed.len()
            ))
        );

        eprintln!("\nFailed removals:");
        for (branch, error) in &result.failed {
            eprintln!("{}", error_message(format!("  {}: {}", branch, error)));
        }
    }
}
