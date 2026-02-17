//! Worktree relocation logic for `wt step relocate`.
//!
//! This module implements the algorithm for moving worktrees to their expected
//! paths based on the `worktree-path` template. It handles:
//!
//! - Simple relocations (target is empty)
//! - Swap/cycle scenarios (worktrees occupy each other's targets)
//! - Blocked targets (non-worktree paths, with optional `--clobber`)
//! - Main worktree special handling (can't use `git worktree move`)
//!
//! The algorithm uses explicit types to represent each stage of the pipeline:
//!
//! ```text
//! gather_candidates() → Vec<RelocationCandidate>
//!         ↓
//! validate_candidates() → Vec<ValidatedCandidate>
//!         ↓
//! RelocationExecutor::new() → executor with dependency graph
//!         ↓
//! executor.execute() → performs moves in topological order
//! ```

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Context;
use color_print::cformat;
use worktrunk::config::UserConfig;
use worktrunk::git::{Repository, WorktreeInfo};
use worktrunk::path::format_path_for_display;
use worktrunk::shell_exec::Cmd;
use worktrunk::styling::{
    eprintln, format_with_gutter, hint_message, info_message, progress_message, success_message,
    warning_message,
};

use super::commit::{CommitGenerator, StageMode};
use super::worktree::{compute_worktree_path, paths_match};

// ============================================================================
// Types representing each stage of the pipeline
// ============================================================================

/// A worktree that needs relocation (current path != expected path).
pub struct RelocationCandidate {
    pub wt: WorktreeInfo,
    pub expected_path: PathBuf,
}

impl RelocationCandidate {
    /// The branch name (guaranteed to exist for relocation candidates).
    pub fn branch(&self) -> &str {
        self.wt.branch.as_deref().unwrap()
    }
}

/// Result of gathering relocation candidates.
pub struct GatherResult {
    pub candidates: Vec<RelocationCandidate>,
    pub template_errors: usize,
}

/// A candidate that passed pre-checks (not locked, not dirty or committed).
pub struct ValidatedCandidate {
    pub wt: WorktreeInfo,
    pub expected_path: PathBuf,
    pub is_main: bool,
}

impl ValidatedCandidate {
    pub fn branch(&self) -> &str {
        self.wt.branch.as_deref().unwrap()
    }
}

/// Tracks a worktree temporarily moved to break a cycle.
struct TempRelocation {
    index: usize,
    temp_path: PathBuf,
    original_path: PathBuf,
}

/// Executes relocations in dependency order, handling cycles via temp moves.
pub struct RelocationExecutor {
    pending: Vec<ValidatedCandidate>,
    /// Maps canonical current path → index in pending (for cycle detection)
    current_locations: HashMap<PathBuf, usize>,
    /// Indices blocked by external factors
    blocked: HashSet<usize>,
    /// Indices already moved (directly or to temp)
    moved: HashSet<usize>,
    /// Worktrees moved to temp location, awaiting final move
    temp_relocated: Vec<TempRelocation>,
    /// Temp directory for cycle breaking
    temp_dir: PathBuf,
    /// Counters for summary
    pub skipped: usize,
    pub relocated: usize,
}

// ============================================================================
// Phase 1: Gather candidates
// ============================================================================

/// Find worktrees that are not at their expected paths.
///
/// Returns candidates for relocation plus a count of template errors encountered.
pub fn gather_candidates(
    repo: &Repository,
    config: &UserConfig,
    filter_branches: &[String],
) -> anyhow::Result<GatherResult> {
    // Get all worktrees, excluding prunable ones
    let worktrees: Vec<_> = repo
        .list_worktrees()?
        .into_iter()
        .filter(|wt| wt.prunable.is_none())
        .collect();

    // Filter to requested branches if any were specified
    let worktrees: Vec<_> = if filter_branches.is_empty() {
        worktrees
    } else {
        worktrees
            .into_iter()
            .filter(|wt| {
                wt.branch
                    .as_ref()
                    .is_some_and(|b| filter_branches.iter().any(|arg| arg == b))
            })
            .collect()
    };

    // Find mismatched worktrees
    let mut candidates = Vec::new();
    let mut template_errors = 0;

    for wt in worktrees {
        let Some(branch) = wt.branch.as_deref() else {
            continue; // Detached HEAD worktrees can't be relocated
        };

        match compute_worktree_path(repo, branch, config) {
            Ok(expected) => {
                // Check if paths differ (canonical comparison)
                let actual_canonical = wt.path.canonicalize().unwrap_or_else(|_| wt.path.clone());
                let expected_canonical =
                    expected.canonicalize().unwrap_or_else(|_| expected.clone());

                if actual_canonical != expected_canonical {
                    candidates.push(RelocationCandidate {
                        wt,
                        expected_path: expected,
                    });
                }
            }
            Err(e) => {
                // Template expansion failed - warn user so they can fix config
                let header = cformat!("Skipping <bold>{branch}</> due to template error:");
                let detail = format_with_gutter(&e.to_string(), None);
                eprintln!("{}\n{}", warning_message(header), detail);
                template_errors += 1;
            }
        }
    }

    Ok(GatherResult {
        candidates,
        template_errors,
    })
}

// ============================================================================
// Phase 2: Validate candidates
// ============================================================================

/// Result of validating candidates.
pub struct ValidationResult {
    pub validated: Vec<ValidatedCandidate>,
    pub skipped: usize,
}

/// Check each candidate for locked/dirty state and optionally auto-commit.
///
/// Returns validated candidates ready for relocation.
pub fn validate_candidates(
    repo: &Repository,
    config: &UserConfig,
    candidates: Vec<RelocationCandidate>,
    auto_commit: bool,
    repo_path: &Path,
) -> anyhow::Result<ValidationResult> {
    let mut validated = Vec::new();
    let mut skipped = 0;

    for candidate in candidates {
        let branch = candidate.branch();

        // Check locked - always skip (user must unlock manually)
        if let Some(reason) = &candidate.wt.locked {
            let reason_text = if reason.is_empty() {
                String::new()
            } else {
                format!(": {reason}")
            };
            eprintln!(
                "{}",
                warning_message(cformat!("Skipping <bold>{branch}</> (locked{reason_text})"))
            );
            skipped += 1;
            continue;
        }

        // Check dirty
        let worktree = repo.worktree_at(&candidate.wt.path);
        if worktree.is_dirty()? {
            if auto_commit {
                eprintln!(
                    "{}",
                    progress_message(cformat!("Committing changes in <bold>{branch}</>..."))
                );
                // Stage all changes
                worktree
                    .run_command(&["add", "-A"])
                    .context("Failed to stage changes")?;
                // Commit using shared pipeline
                let project_id = repo.project_identifier().ok();
                let commit_config = config.commit_generation(project_id.as_deref());
                CommitGenerator::new(&commit_config).commit_staged_changes(
                    &worktree,
                    false, // show_progress - already showing "Committing changes in..."
                    false, // show_no_squash_note
                    StageMode::None, // already staged above
                )?;
            } else {
                eprintln!(
                    "{}",
                    warning_message(cformat!("Skipping <bold>{branch}</> (uncommitted changes)"))
                );
                eprintln!(
                    "{}",
                    hint_message(cformat!(
                        "Use <bright-black>--commit</> to auto-commit changes before relocating"
                    ))
                );
                skipped += 1;
                continue;
            }
        }

        let is_main = paths_match(&candidate.wt.path, repo_path);
        validated.push(ValidatedCandidate {
            wt: candidate.wt,
            expected_path: candidate.expected_path,
            is_main,
        });
    }

    Ok(ValidationResult { validated, skipped })
}

// ============================================================================
// Phase 3 & 4: Execute relocations
// ============================================================================

impl RelocationExecutor {
    /// Create executor and classify targets (handling blockers with optional clobber).
    pub fn new(
        repo: &Repository,
        validated: Vec<ValidatedCandidate>,
        clobber: bool,
    ) -> anyhow::Result<Self> {
        let temp_dir = repo.git_common_dir().join("wt-relocate-tmp");

        // Build map of current locations for cycle detection
        let mut current_locations: HashMap<PathBuf, usize> = HashMap::new();
        for (i, candidate) in validated.iter().enumerate() {
            let canonical = candidate
                .wt
                .path
                .canonicalize()
                .unwrap_or_else(|_| candidate.wt.path.clone());
            current_locations.insert(canonical, i);
        }

        let mut blocked: HashSet<usize> = HashSet::new();
        let mut skipped = 0;

        // Classify targets and handle blockers
        for (i, candidate) in validated.iter().enumerate() {
            let expected_path = &candidate.expected_path;

            if !expected_path.exists() {
                continue; // Target is empty, no blocker
            }

            let canonical_target = expected_path
                .canonicalize()
                .unwrap_or_else(|_| expected_path.clone());

            if current_locations.contains_key(&canonical_target) {
                // Target is another worktree we're moving - handle via dependency graph
                continue;
            }

            // Target exists but is NOT a worktree we're moving
            let branch = candidate.branch();

            // SAFETY: Never clobber an existing worktree - that would corrupt git metadata
            if let Some((_, occupant_branch)) = repo.worktree_at_path(expected_path)? {
                let occupant_name = occupant_branch.as_deref().unwrap_or("(detached)");
                let msg = cformat!(
                    "Skipping <bold>{branch}</> (target is worktree for <bold>{occupant_name}</>)"
                );
                eprintln!("{}", warning_message(msg));
                let hint = cformat!("Relocate or remove <bright-black>{occupant_name}</> first");
                eprintln!("{}", hint_message(hint));
                blocked.insert(i);
                skipped += 1;
                continue;
            }

            if clobber {
                // Backup the blocker
                let timestamp_secs = worktrunk::utils::get_now() as i64;
                let datetime = chrono::DateTime::from_timestamp(timestamp_secs, 0)
                    .unwrap_or_else(chrono::Utc::now);
                let suffix = datetime.format("%Y%m%d-%H%M%S");
                let backup_path = expected_path.with_file_name(format!(
                    "{}.bak-{suffix}",
                    expected_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                ));
                let src = format_path_for_display(expected_path);
                let dest = format_path_for_display(&backup_path);
                eprintln!(
                    "{}",
                    progress_message(cformat!("Backing up {src} → {dest}"))
                );
                std::fs::rename(expected_path, &backup_path)
                    .with_context(|| format!("Failed to backup {}", expected_path.display()))?;
            } else {
                let blocked_path = format_path_for_display(expected_path);
                let msg = cformat!("Skipping <bold>{branch}</> (target blocked: {blocked_path})");
                eprintln!("{}", warning_message(msg));
                eprintln!(
                    "{}",
                    hint_message(cformat!(
                        "Use <bright-black>--clobber</> to backup blocking paths"
                    ))
                );
                blocked.insert(i);
                skipped += 1;
            }
        }

        Ok(Self {
            pending: validated,
            current_locations,
            blocked,
            moved: HashSet::new(),
            temp_relocated: Vec::new(),
            temp_dir,
            skipped,
            relocated: 0,
        })
    }

    /// Execute all relocations in dependency order.
    pub fn execute(
        &mut self,
        repo_path: &Path,
        default_branch: &str,
        cwd: Option<&Path>,
    ) -> anyhow::Result<()> {
        // Process until all pending are moved or in temp
        loop {
            let mut made_progress = false;

            // Find worktrees whose target is now empty
            for i in 0..self.pending.len() {
                if self.moved.contains(&i) || self.blocked.contains(&i) {
                    continue;
                }

                match self.is_target_empty(i) {
                    Some(true) => {
                        self.move_worktree(i, repo_path, default_branch, cwd)?;
                        made_progress = true;
                    }
                    Some(false) => {
                        // Target occupied by another pending worktree - wait for it to move
                    }
                    None => {
                        // Target unexpectedly blocked (TOCTOU race or same-target conflict)
                        let branch = self.pending[i].branch();
                        let blocked_path = format_path_for_display(&self.pending[i].expected_path);
                        let msg = cformat!(
                            "Skipping <bold>{branch}</> (target occupied: {blocked_path})"
                        );
                        eprintln!("{}", warning_message(msg));
                        self.blocked.insert(i);
                        self.skipped += 1;
                    }
                }
            }

            if made_progress {
                continue;
            }

            // No progress - break a cycle by moving one worktree to temp
            if !self.break_cycle()? {
                break; // All done
            }
        }

        // Move temp-relocated worktrees to final destinations
        self.finalize_temp_relocations()?;

        // Clean up temp directory if empty
        if self.temp_dir.exists() {
            let _ = std::fs::remove_dir(&self.temp_dir);
        }

        Ok(())
    }

    /// Check if target path is empty (not occupied by a pending worktree).
    ///
    /// Returns:
    /// - `Some(true)` if target doesn't exist or occupant has moved
    /// - `Some(false)` if target is occupied by another pending worktree
    /// - `None` if target is unexpectedly blocked (not in our tracking)
    fn is_target_empty(&self, idx: usize) -> Option<bool> {
        let expected = &self.pending[idx].expected_path;

        if !expected.exists() {
            return Some(true);
        }

        let canonical = expected.canonicalize().unwrap_or_else(|_| expected.clone());

        // Check if it's a worktree we're tracking
        self.current_locations
            .get(&canonical)
            .map(|occupant_idx| self.moved.contains(occupant_idx))
    }

    /// Move a single worktree to its expected path.
    fn move_worktree(
        &mut self,
        idx: usize,
        repo_path: &Path,
        default_branch: &str,
        cwd: Option<&Path>,
    ) -> anyhow::Result<()> {
        // Extract data we need before any mutable borrows
        let branch = self.pending[idx].branch().to_string();
        let is_main = self.pending[idx].is_main;
        let src_path = self.pending[idx].wt.path.clone();
        let dest_path = self.pending[idx].expected_path.clone();

        let src_display = format_path_for_display(&src_path);
        let dest_display = format_path_for_display(&dest_path);

        if is_main {
            self.move_main_worktree(idx, repo_path, default_branch)?;
        } else {
            Cmd::new("git")
                .args(["worktree", "move"])
                .arg(src_path.to_string_lossy())
                .arg(dest_path.to_string_lossy())
                .context(&branch)
                .run()
                .context("Failed to move worktree")?;
        }

        let msg = cformat!("Relocated <bold>{branch}</>: {src_display} → {dest_display}");
        eprintln!("{}", success_message(msg));

        // Update shell if user is inside this worktree
        if let Some(cwd_path) = cwd
            && cwd_path.starts_with(&src_path)
        {
            let relative = cwd_path.strip_prefix(&src_path).unwrap_or(Path::new(""));
            crate::output::change_directory(dest_path.join(relative))?;
        }

        self.moved.insert(idx);
        self.relocated += 1;
        Ok(())
    }

    /// Main worktree can't use `git worktree move`; must create new + switch.
    fn move_main_worktree(
        &mut self,
        idx: usize,
        repo_path: &Path,
        default_branch: &str,
    ) -> anyhow::Result<()> {
        let candidate = &self.pending[idx];
        let branch = candidate.branch();

        let msg = cformat!("Switching main worktree to <bold>{default_branch}</>...");
        eprintln!("{}", progress_message(msg));

        Cmd::new("git")
            .args(["checkout", default_branch])
            .current_dir(repo_path)
            .context("main")
            .run()
            .context("Failed to checkout default branch")?;

        // Try to create worktree; if it fails, rollback to original branch
        let add_result = Cmd::new("git")
            .args(["worktree", "add"])
            .arg(candidate.expected_path.to_string_lossy())
            .arg(branch)
            .context(branch)
            .run();

        if let Err(e) = add_result {
            // Rollback: checkout the original branch to restore user context
            let rollback_msg = cformat!("Worktree creation failed, restoring <bold>{branch}</>...");
            eprintln!("{}", warning_message(rollback_msg));

            let _ = Cmd::new("git")
                .args(["checkout", branch])
                .current_dir(repo_path)
                .context("main")
                .run(); // Best-effort rollback

            return Err(e).context("Failed to create worktree for main relocation");
        }

        Ok(())
    }

    /// Break a cycle by moving one worktree to a temp location.
    ///
    /// Returns `true` if a worktree was moved to temp, `false` if no cycles remain.
    fn break_cycle(&mut self) -> anyhow::Result<bool> {
        // Find a non-main worktree to temp-move (git worktree move can't move main)
        let cycle_idx = (0..self.pending.len())
            .filter(|&i| !self.moved.contains(&i) && !self.blocked.contains(&i))
            .find(|&i| !self.pending[i].is_main);

        // Fallback to any remaining (shouldn't happen in practice)
        let cycle_idx = cycle_idx.or_else(|| {
            (0..self.pending.len())
                .find(|&i| !self.moved.contains(&i) && !self.blocked.contains(&i))
        });

        let Some(i) = cycle_idx else {
            return Ok(false);
        };

        let candidate = &self.pending[i];
        let branch = candidate.branch();

        // Create temp directory if needed
        std::fs::create_dir_all(&self.temp_dir)?;

        // Sanitize branch name for temp path (feature/foo -> feature-foo)
        let safe_branch = worktrunk::path::sanitize_for_filename(branch);
        let temp_path = self.temp_dir.join(&safe_branch);

        let msg = cformat!("Moving <bold>{branch}</> to temporary location...");
        eprintln!("{}", progress_message(msg));

        Cmd::new("git")
            .args(["worktree", "move"])
            .arg(candidate.wt.path.to_string_lossy())
            .arg(temp_path.to_string_lossy())
            .context(branch)
            .run()
            .context("Failed to move worktree to temp")?;

        // Update current_locations to reflect the move
        let old_canonical = candidate
            .wt
            .path
            .canonicalize()
            .unwrap_or_else(|_| candidate.wt.path.clone());
        self.current_locations.remove(&old_canonical);

        self.temp_relocated.push(TempRelocation {
            index: i,
            temp_path,
            original_path: candidate.wt.path.clone(),
        });
        self.moved.insert(i);

        Ok(true)
    }

    /// Move worktrees from temp locations to their final destinations.
    fn finalize_temp_relocations(&mut self) -> anyhow::Result<()> {
        for temp in std::mem::take(&mut self.temp_relocated) {
            let candidate = &self.pending[temp.index];
            let branch = candidate.branch();

            let src_display = format_path_for_display(&temp.original_path);
            let dest_display = format_path_for_display(&candidate.expected_path);

            Cmd::new("git")
                .args(["worktree", "move"])
                .arg(temp.temp_path.to_string_lossy())
                .arg(candidate.expected_path.to_string_lossy())
                .context(branch)
                .run()
                .context("Failed to move worktree from temp to final location")?;

            let msg = cformat!("Relocated <bold>{branch}</>: {src_display} → {dest_display}");
            eprintln!("{}", success_message(msg));

            self.relocated += 1;
        }

        Ok(())
    }
}

// ============================================================================
// Display helpers
// ============================================================================

/// Show dry-run preview of relocations.
pub fn show_dry_run_preview(candidates: &[RelocationCandidate]) {
    eprintln!(
        "{}",
        info_message(format!(
            "{} worktree{} would be relocated:",
            candidates.len(),
            if candidates.len() == 1 { "" } else { "s" }
        ))
    );

    let preview_lines: Vec<String> = candidates
        .iter()
        .map(|c| {
            let branch = c.branch();
            let src_display = format_path_for_display(&c.wt.path);
            let dest_display = format_path_for_display(&c.expected_path);
            cformat!("<bold>{branch}</>: {src_display} → {dest_display}")
        })
        .collect();
    eprintln!("{}", format_with_gutter(&preview_lines.join("\n"), None));
}

/// Show summary of relocations performed.
pub fn show_summary(relocated: usize, skipped: usize) {
    if relocated > 0 || skipped > 0 {
        eprintln!();
        let plural = |n: usize| if n == 1 { "worktree" } else { "worktrees" };
        if skipped == 0 {
            let msg = format!("Relocated {relocated} {}", plural(relocated));
            eprintln!("{}", success_message(msg));
        } else {
            let msg = format!(
                "Relocated {relocated} {}, skipped {skipped} {}",
                plural(relocated),
                plural(skipped)
            );
            eprintln!("{}", info_message(msg));
        }
    }
}

/// Show message when no relocations are needed.
pub fn show_no_relocations_needed(template_errors: usize) {
    if template_errors == 0 {
        eprintln!("{}", info_message("All worktrees are at expected paths"));
    } else {
        eprintln!(
            "{}",
            info_message(format!(
                "No relocations performed; {} skipped due to template error{}",
                template_errors,
                if template_errors == 1 { "" } else { "s" }
            ))
        );
    }
}

/// Show message when all candidates were skipped during validation.
pub fn show_all_skipped(skipped: usize) {
    if skipped > 0 {
        eprintln!();
        eprintln!(
            "{}",
            info_message(format!(
                "Skipped {skipped} worktree{}",
                if skipped == 1 { "" } else { "s" }
            ))
        );
    }
}
