//! `wt step squash` — squash commits into one (also used by `wt merge --squash`).

use anyhow::Context;
use color_print::cformat;
use worktrunk::HookType;
use worktrunk::config::UserConfig;
use worktrunk::git::Repository;
use worktrunk::styling::{
    eprintln, format_with_gutter, hint_message, info_message, println, progress_message,
    success_message,
};

use super::super::command_approval::approve_or_skip;
use super::super::command_executor::FailureStrategy;
use super::super::commit::{CommitGenerator, CommitOutcome, HookGate, StageMode};
use super::super::context::CommandEnv;
use super::super::hooks::{self, HookAnnouncer, execute_hook};
use super::super::repository_ext::RepositoryCliExt;
use super::super::template_vars::TemplateVars;
use super::shared::print_dry_run;

/// Result of a squash operation
#[derive(Debug, Clone)]
pub enum SquashResult {
    /// Squash or commit occurred. Carries the resulting commit's SHA, message,
    /// and resolved stage mode so callers can render structured output.
    Squashed {
        sha: String,
        message: String,
        stage_mode: StageMode,
    },
    /// Nothing to squash: no commits ahead of target branch
    NoCommitsAhead(String),
    /// Nothing to squash: already a single commit
    AlreadySingleCommit,
    /// Squash attempted but resulted in no net changes (commits canceled out)
    NoNetChanges,
}

/// Handle shared squash workflow (used by `wt step squash` and `wt merge`)
///
/// # Arguments
/// * `hooks` - Whether to run pre-commit hooks. `Run` triggers an internal approval
///   prompt; `NoHooksFlag` skips with a "(--no-hooks)" message; `Silent` skips silently
///   (used when the caller already declined approval upstream and announced it).
/// * `stage` - CLI-provided stage mode. If None, uses the effective config default.
/// * `announcer` - Post-commit hooks register on the caller's announcer; the
///   caller decides when to flush. Multi-phase callers (`wt merge --squash`
///   combining post-commit + post-remove + post-switch + post-merge) share
///   one announce line; standalone callers (`wt step squash`) construct an
///   announcer of their own and flush right after.
pub fn handle_squash(
    target: Option<&str>,
    yes: bool,
    hooks: HookGate,
    stage: Option<StageMode>,
    announcer: &mut HookAnnouncer<'_>,
) -> anyhow::Result<SquashResult> {
    // Load config once, run LLM setup prompt, then reuse config
    let mut config = UserConfig::load().context("Failed to load config")?;
    // One-time LLM setup prompt (errors logged internally; don't block commit)
    let _ = crate::output::prompt_commit_generation(&mut config);

    let env = CommandEnv::for_action(config)?;
    let repo = &env.repo;
    // Squash requires being on a branch (can't squash in detached HEAD)
    let current_branch = env.require_branch("squash")?.to_string();
    let ctx = env.context(yes);
    let resolved = env.resolved();
    let generator = CommitGenerator::new(&resolved.commit_generation);

    // CLI flag overrides config value
    let stage_mode = stage.unwrap_or(resolved.commit.stage());

    // Check if any pre-commit hooks exist (needed for skip message and approval)
    let project_config = repo.load_project_config()?;
    let user_hooks = ctx.config.hooks(ctx.project_id().as_deref());
    let (user_cfg, proj_cfg) =
        hooks::lookup_hook_configs(&user_hooks, project_config.as_ref(), HookType::PreCommit);
    let any_hooks_exist = user_cfg.is_some() || proj_cfg.is_some();

    // Resolve the hook gate: Run triggers an approval prompt and downgrades to Silent
    // on decline (approve_or_skip prints its own message). NoHooksFlag prints the skip
    // message itself; Silent stays quiet so the upstream caller's decline message isn't
    // followed by a spurious "(--no-hooks)" line.
    let hooks = match hooks {
        HookGate::Run => {
            if approve_or_skip(
                &ctx,
                &[HookType::PreCommit, HookType::PostCommit],
                "Commands declined, squashing without hooks",
            )? {
                HookGate::Run
            } else {
                HookGate::Silent
            }
        }
        HookGate::NoHooksFlag => {
            if any_hooks_exist {
                eprintln!("{}", info_message("Skipping pre-commit hooks (--no-hooks)"));
            }
            HookGate::NoHooksFlag
        }
        HookGate::Silent => HookGate::Silent,
    };

    // Get and validate target ref (any commit-ish for merge-base calculation)
    let integration_target = repo.require_target_ref(target)?;
    let template_vars = TemplateVars::new().with_target(&integration_target);

    // Auto-stage changes before running pre-commit hooks so both beta and merge paths behave identically
    match stage_mode {
        StageMode::All => {
            repo.warn_if_auto_staging_untracked()?;
            repo.run_command(&["add", "-A"])
                .context("Failed to stage changes")?;
        }
        StageMode::Tracked => {
            repo.run_command(&["add", "-u"])
                .context("Failed to stage tracked changes")?;
        }
        StageMode::None => {
            // Stage nothing - use what's already staged
        }
    }

    // Run pre-commit hooks (user first, then project).
    if hooks.run() {
        execute_hook(
            &ctx,
            HookType::PreCommit,
            &template_vars.as_extra_vars(),
            FailureStrategy::FailFast,
            crate::output::pre_hook_display_path(ctx.worktree_path),
        )?;
    }

    // Get merge base with target branch (required for squash)
    let merge_base = repo
        .merge_base("HEAD", &integration_target)?
        .context("Cannot squash: no common ancestor with target branch")?;

    // Count commits since merge base
    let commit_count = repo.count_commits(&merge_base, "HEAD")?;

    // Check if there are staged changes in addition to commits
    let wt = repo.current_worktree();
    let has_staged = wt.has_staged_changes()?;

    // Handle different scenarios
    if commit_count == 0 && !has_staged {
        // No commits and no staged changes - nothing to squash
        return Ok(SquashResult::NoCommitsAhead(integration_target));
    }

    if commit_count == 0 && has_staged {
        // Just staged changes, no commits - commit them directly (no squashing needed)
        let CommitOutcome {
            sha,
            message,
            stage_mode,
        } = generator.commit_staged_changes(&wt, true, true, stage_mode)?;
        return Ok(SquashResult::Squashed {
            sha,
            message,
            stage_mode,
        });
    }

    if commit_count == 1 && !has_staged {
        // Single commit, no staged changes - already squashed
        return Ok(SquashResult::AlreadySingleCommit);
    }

    // Either multiple commits OR single commit with staged changes - squash them
    // Get diff stats early for display in progress message
    let range = format!("{}..HEAD", merge_base);

    let commit_text = if commit_count == 1 {
        "commit"
    } else {
        "commits"
    };

    // Get total stats (commits + any working tree changes)
    let total_stats = if has_staged {
        repo.diff_stats_summary(&["diff", "--shortstat", &merge_base, "--cached"])
    } else {
        repo.diff_stats_summary(&["diff", "--shortstat", &range])
    };

    let with_changes = if has_staged {
        match stage_mode {
            StageMode::Tracked => " & tracked changes",
            _ => " & working tree changes",
        }
    } else {
        ""
    };

    // Build parenthesized content: stats only (stage mode is in message text)
    let parts = total_stats;

    let squash_progress = if parts.is_empty() {
        format!("Squashing {commit_count} {commit_text}{with_changes} into a single commit...")
    } else {
        // Gray parenthetical with separate cformat for closing paren (avoids optimizer)
        let parts_str = parts.join(", ");
        let paren_close = cformat!("<bright-black>)</>");
        cformat!(
            "Squashing {commit_count} {commit_text}{with_changes} into a single commit <bright-black>({parts_str}</>{paren_close}..."
        )
    };
    eprintln!("{}", progress_message(squash_progress));

    // Create safety backup before potentially destructive reset if there are working tree changes
    if has_staged {
        let backup_message = format!("{} → {} (squash)", current_branch, integration_target);
        let sha = wt.create_safety_backup(&backup_message)?;
        eprintln!("{}", hint_message(format!("Backup created @ {sha}")));
    }

    // Get commit subjects for the squash message
    let subjects = repo.commit_subjects(&range)?;

    // Generate squash commit message
    eprintln!(
        "{}",
        progress_message("Generating squash commit message...")
    );

    generator.emit_hint_if_needed();

    // Get current branch and repo name for template variables
    let repo_root = wt.root()?;
    let repo_name = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo");

    let commit_message = crate::llm::generate_squash_message(
        &integration_target,
        &merge_base,
        &subjects,
        &current_branch,
        repo_name,
        &resolved.commit_generation,
    )?;

    // Display the generated commit message
    let formatted_message = generator.format_message_for_display(&commit_message);
    eprintln!("{}", format_with_gutter(&formatted_message, None));

    // Reset to merge base (soft reset stages all changes, including any already-staged uncommitted changes)
    //
    // TOCTOU note: Between this reset and the commit below, an external process could
    // modify the staging area. This is extremely unlikely (requires precise timing) and
    // the consequence is minor (unexpected content in squash commit). The commit message
    // generated above accurately reflects the original commits being squashed, so any
    // discrepancy would be visible in the diff. Considered acceptable risk.
    repo.run_command(&["reset", "--soft", &merge_base])
        .context("Failed to reset to merge base")?;

    // Check if there are actually any changes to commit
    if !wt.has_staged_changes()? {
        eprintln!(
            "{}",
            info_message(format!(
                "No changes after squashing {commit_count} {commit_text}"
            ))
        );
        return Ok(SquashResult::NoNetChanges);
    }

    // Commit with the generated message
    repo.run_command(&["commit", "-m", &commit_message])
        .context("Failed to create squash commit")?;

    // Full SHA for the JSON payload, abbreviated form for the success line.
    let commit_sha = repo.run_command(&["rev-parse", "HEAD"])?.trim().to_string();
    let commit_hash = repo.short_sha(&commit_sha)?;

    // Show success immediately after completing the squash
    eprintln!(
        "{}",
        success_message(cformat!("Squashed @ <dim>{commit_hash}</>"))
    );

    // Register post-commit hooks onto the caller's announcer (respects --no-hooks).
    if hooks.run() {
        let extra_vars = template_vars.as_extra_vars();
        announcer.register(&ctx, HookType::PostCommit, &extra_vars, None)?;
    }

    Ok(SquashResult::Squashed {
        sha: commit_sha,
        message: commit_message,
        stage_mode,
    })
}

/// Handle `wt step squash --show-prompt`
///
/// Builds and outputs the squash prompt without running the LLM or squashing.
pub fn step_show_squash_prompt(target: Option<&str>) -> anyhow::Result<()> {
    preview_squash(target, false)
}

/// Handle `wt step squash --dry-run`
///
/// Renders the squash prompt, prints the LLM command, generates the message, and prints
/// it without resetting, running hooks, or committing.
pub fn step_dry_run_squash(target: Option<&str>) -> anyhow::Result<()> {
    preview_squash(target, true)
}

/// Shared implementation for `--show-prompt` and `--dry-run` on squash. `--show-prompt`
/// (`dry_run = false`) outputs only the rendered prompt; `--dry-run` additionally calls
/// the LLM and prints the command and the generated message.
fn preview_squash(target: Option<&str>, dry_run: bool) -> anyhow::Result<()> {
    let repo = Repository::current()?;
    let config = UserConfig::load().context("Failed to load config")?;
    let project_id = repo.project_identifier().ok();
    let commit_config = config.commit_generation(project_id.as_deref());

    let integration_target = repo.require_target_ref(target)?;

    let wt = repo.current_worktree();
    let current_branch = wt.branch()?.unwrap_or_else(|| "HEAD".to_string());

    let merge_base = repo
        .merge_base("HEAD", &integration_target)?
        .context("Cannot generate squash message: no common ancestor with target branch")?;

    let range = format!("{}..HEAD", merge_base);
    let subjects = repo.commit_subjects(&range)?;

    let repo_root = wt.root()?;
    let repo_name = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo");

    let prompt = crate::llm::build_squash_prompt(
        &integration_target,
        &merge_base,
        &subjects,
        &current_branch,
        repo_name,
        &commit_config,
    )?;
    if !dry_run {
        println!("{}", prompt);
        return Ok(());
    }
    let message = crate::llm::generate_squash_message(
        &integration_target,
        &merge_base,
        &subjects,
        &current_branch,
        repo_name,
        &commit_config,
    )?;
    print_dry_run(&prompt, &commit_config, &message)
}
