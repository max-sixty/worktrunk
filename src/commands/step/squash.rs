//! `wt step squash` — squash commits into one (also used by `wt merge --squash`).

use anyhow::Context;
use color_print::cformat;
use worktrunk::HookType;
use worktrunk::config::UserConfig;
use worktrunk::git::{Repository, WorkingTree};
use worktrunk::styling::{
    eprintln, format_with_gutter, hint_message, info_message, println, progress_message,
    success_message,
};

use super::super::command_approval::{
    approve_commit_template_append, approve_or_skip, resolve_template_for_preview,
};
use super::super::command_executor::FailureStrategy;
use super::super::commit::{CommitGenerator, CommitOutcome, HookGate, StageMode};
use super::super::context::CommandEnv;
use super::super::hooks::{HookAnnouncer, execute_hook};
use super::super::repository_ext::warn_about_untracked_files;
use super::super::template_vars::TemplateVars;
use super::shared::print_dry_run;

/// Caller's stance on project commit-message guidance approval.
///
/// `wt step squash` runs its own gate (the user invokes squash directly, so
/// there's no upstream decision to attach to). `wt merge` resolves the append
/// through its own `approve_commit_template_append` gate up front and passes
/// the result here so `handle_squash` doesn't gate it a second time.
#[derive(Debug, Clone)]
pub enum PreApprovedGuidance {
    /// No pre-approval — `handle_squash` runs its own gate via
    /// `approve_commit_template_append` (only if the LLM is configured).
    RunOwnGate,
    /// Caller already resolved the guidance through an upstream batch.
    /// `None` means "guidance not configured or user declined".
    Resolved(Option<String>),
}

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
    pre_approved_guidance: PreApprovedGuidance,
) -> anyhow::Result<SquashResult> {
    // Load config once, run LLM setup prompt, then reuse config
    let mut config = UserConfig::load().context("Failed to load config")?;
    // One-time LLM setup prompt (errors logged internally; don't block commit)
    let _ = crate::output::prompt_commit_generation(&mut config);

    let env = CommandEnv::for_action(config)?;
    let repo = &env.repo;
    // Rewriting history under a half-finished operation is never what the user
    // meant, and mid-rebase HEAD is detached — so this runs ahead of the branch
    // check, which would otherwise blame the detached HEAD and point at
    // `git switch`, the one command that throws the operation away.
    repo.ensure_no_operation_in_progress("squash")?;
    // Squash auto-stages, and `git add -A` would resolve an unmerged path to
    // whatever is on disk — conflict markers included. `wt.stage` gates that
    // directly; refusing here too keeps the approval prompt below from asking
    // about hooks for a squash that cannot happen.
    let wt = repo.worktree_at(&env.worktree_path);
    wt.ensure_no_unmerged_paths("squash")?;
    // Squash requires being on a branch (can't squash in detached HEAD)
    let current_branch = env.require_branch("squash")?.to_string();
    let ctx = env.context(yes);
    let resolved = env.resolved();
    // Defer project-guidance approval until we know an LLM call will happen
    // (after the NoCommitsAhead / AlreadySingleCommit early-exits). When the
    // caller pre-approved via an upstream batch (e.g. `wt merge`), use that
    // value directly to avoid a second prompt mid-flow.
    let llm_configured = resolved.commit_generation.is_configured();
    let approve_guidance = || -> anyhow::Result<Option<String>> {
        match &pre_approved_guidance {
            PreApprovedGuidance::Resolved(value) => Ok(value.clone()),
            PreApprovedGuidance::RunOwnGate if llm_configured => {
                approve_commit_template_append(&ctx)
            }
            PreApprovedGuidance::RunOwnGate => Ok(None),
        }
    };

    // CLI flag overrides config value
    let stage_mode = stage.unwrap_or(resolved.commit.stage());

    // Check if any pre-commit hooks exist (needed for skip message and approval)
    let project_config = repo.load_project_config()?;
    let user_hooks = ctx.config.hooks(ctx.project_id().as_deref());
    let any_hooks_exist = user_hooks.get(HookType::PreCommit).is_some()
        || project_config
            .as_ref()
            .is_some_and(|c| c.hooks.get(HookType::PreCommit).is_some());

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
    // #3519: when the branch's history extends past the local target into the
    // target's upstream, measure the squash against the upstream — so commits
    // already published there are never folded into the squash commit.
    let span_target = repo
        .span_upstream(&integration_target)?
        .unwrap_or_else(|| integration_target.clone());
    let template_vars = TemplateVars::new().with_target(&integration_target);

    // Auto-stage changes before running pre-commit hooks so both beta and merge paths behave identically
    if stage_mode == StageMode::All {
        warn_about_untracked_files(&wt)?;
    }
    wt.stage(stage_mode)?;

    // Run pre-commit hooks (user first, then project).
    if hooks.run() {
        execute_hook(
            &ctx,
            HookType::PreCommit,
            &template_vars.as_extra_vars(),
            FailureStrategy::FailFast,
        )?;
    }

    // Resolve HEAD once, so the span, the message's commit list, and the
    // compare-and-swap that finally moves the branch all describe one tip.
    let head_sha = wt.run_command(&["rev-parse", "HEAD"])?.trim().to_string();

    // Get merge base with target branch (required for squash)
    let merge_base = repo
        .merge_base(&head_sha, &span_target)?
        .context("Cannot squash: no common ancestor with target branch")?;

    // Count commits since merge base
    let commit_count = repo.count_commits(&merge_base, &head_sha)?;

    // Check if there are staged changes in addition to commits
    let has_staged = wt.has_staged_changes()?;

    // Handle different scenarios
    if commit_count == 0 && !has_staged {
        // No commits and no staged changes - nothing to squash
        return Ok(SquashResult::NoCommitsAhead(span_target));
    }

    if commit_count == 1 && !has_staged {
        // Single commit, no staged changes - already squashed
        return Ok(SquashResult::AlreadySingleCommit);
    }

    // From here on, an LLM call may happen — gate the project append.
    let project_append = approve_guidance()?;
    let generator = CommitGenerator::new(&resolved.commit_generation, project_append.as_deref());

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

    // Either multiple commits OR single commit with staged changes - squash them
    // Get diff stats early for display in progress message
    let range = format!("{merge_base}..{head_sha}");

    let commit_text = if commit_count == 1 {
        "commit"
    } else {
        "commits"
    };

    // Get total stats (commits + any working tree changes)
    let total_stats = if has_staged {
        wt.prepare_staged_diff(&merge_base).stats_summary()
    } else {
        wt.prepare_commit_diff(&merge_base, &head_sha)
            .stats_summary()
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

    // Back up working-tree changes before the squash commit absorbs them
    if has_staged {
        let backup_message = format!("{} → {} (squash)", current_branch, span_target);
        let sha = wt.create_safety_backup(&backup_message)?;
        eprintln!("{}", hint_message(format!("Backup created @ {sha}")));
    }

    // Get commit subjects and bodies for the squash message
    let commit_details = repo.commit_message_details(&range)?;

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

    let commit_message = crate::llm::SquashInputs {
        target_branch: &span_target,
        merge_base: &merge_base,
        commit_details: &commit_details,
        current_branch: &current_branch,
        repo_name,
        config: &resolved.commit_generation,
        project_append: project_append.as_deref(),
        // `wt.stage` above already put everything in the real index.
        staging_index: None,
    }
    .generate_message()?;

    // Display the generated commit message
    let formatted_message = generator.format_message_for_display(&commit_message);
    eprintln!("{}", format_with_gutter(&formatted_message, None));

    // Squash the way `git rebase` rewrites history: detach HEAD onto the merge
    // base, commit there, and move the branch to the result. The branch keeps
    // its commits until the commit exists, so a commit that fails — a
    // `pre-commit` hook rejecting it, a broken signing setup — leaves the
    // branch and its history where they were. Committing through `git commit`
    // rather than plumbing is what keeps git's own hooks, `commit.cleanup`,
    // signing, and identity resolution behaving as they do for any other
    // commit; those hooks see a detached HEAD, as they do under `git rebase`.
    let tree = wt.run_command(&["write-tree"])?.trim().to_string();
    let base_tree = repo
        .run_command(&["rev-parse", &format!("{merge_base}^{{tree}}")])?
        .trim()
        .to_string();

    // One compare-and-swap moves the branch, refusing if it moved since
    // `head_sha` was read; ORIG_HEAD then keeps the pre-squash tip, as `git
    // reset` and `git rebase` leave it.
    let branch_ref = format!("refs/heads/{current_branch}");
    let move_branch = |new_sha: &str, reflog_message: &str| -> anyhow::Result<()> {
        wt.run_command(&[
            "update-ref",
            "-m",
            reflog_message,
            &branch_ref,
            new_sha,
            &head_sha,
        ])
        .with_context(|| cformat!("Failed to update <bold>{current_branch}</>"))?;
        wt.run_command(&["update-ref", "ORIG_HEAD", &head_sha])?;
        Ok(())
    };

    if tree == base_tree {
        // The commits cancel out, so the squash produces no commit and the
        // branch moves to the merge base, where `wt merge` finds nothing left
        // to integrate.
        move_branch(&merge_base, "wt squash: no net changes")?;
        eprintln!(
            "{}",
            info_message(format!(
                "No changes after squashing {commit_count} {commit_text}"
            ))
        );
        return Ok(SquashResult::NoNetChanges);
    }

    // `--no-deref` moves HEAD itself, leaving the branch, the index and the
    // working tree untouched; the compare-and-swap refuses if the branch moved
    // while the message was being generated.
    wt.run_command(&[
        "update-ref",
        "--no-deref",
        "-m",
        "wt squash: detach to build the squash commit",
        "HEAD",
        &merge_base,
        &head_sha,
    ])
    .context("Failed to detach HEAD onto the merge base")?;

    let commit_sha = match wt
        .run_command(&["commit", "-m", &commit_message])
        .context("Failed to create squash commit")
        .and_then(|_| wt.run_command(&["rev-parse", "HEAD"]))
    {
        Ok(sha) => sha.trim().to_string(),
        Err(err) => return Err(reattach_head(&wt, &branch_ref, err)),
    };

    let subject = commit_message.lines().next().unwrap_or_default();
    if let Err(err) = move_branch(&commit_sha, &format!("wt squash: {subject}")) {
        return Err(reattach_head(&wt, &branch_ref, err));
    }
    wt.run_command(&["symbolic-ref", "HEAD", &branch_ref])
        .with_context(|| cformat!("Failed to put HEAD back on <bold>{current_branch}</>"))?;

    // Full SHA for the JSON payload, abbreviated form for the success line.
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

/// Put HEAD back on the branch after a failure that struck while it was
/// detached for the squash commit, and return the failure that got us here.
///
/// The branch never moved, so a successful reattach restores the worktree
/// exactly as it was and leaves nothing to report beyond `err`.
fn reattach_head(wt: &WorkingTree<'_>, branch_ref: &str, err: anyhow::Error) -> anyhow::Error {
    match wt.run_command(&["symbolic-ref", "HEAD", branch_ref]) {
        Ok(_) => err,
        Err(reattach_err) => err.context(cformat!(
            "HEAD is left detached ({reattach_err:#}); to put it back, run <bold>git symbolic-ref HEAD {branch_ref}</>"
        )),
    }
}

/// Handle `wt step squash --show-prompt`
///
/// Builds and outputs the squash prompt without running the LLM or squashing.
pub fn step_show_squash_prompt(
    target: Option<&str>,
    stage: Option<StageMode>,
) -> anyhow::Result<()> {
    // `--show-prompt` never invokes the LLM, so the `yes` flag is irrelevant
    // — pass false; the guidance gate inside `preview_squash` is dry-run only.
    preview_squash(target, stage, false, false)
}

/// Handle `wt step squash --dry-run`
///
/// Renders the squash prompt, prints the LLM command, generates the message, and prints
/// it without staging, running hooks, or squashing.
pub fn step_dry_run_squash(
    target: Option<&str>,
    stage: Option<StageMode>,
    yes: bool,
) -> anyhow::Result<()> {
    preview_squash(target, stage, true, yes)
}

/// Shared implementation for `--show-prompt` and `--dry-run` on squash. `--show-prompt`
/// (`dry_run = false`) outputs only the rendered prompt; `--dry-run` additionally calls
/// the LLM and prints the command and the generated message.
fn preview_squash(
    target: Option<&str>,
    stage: Option<StageMode>,
    dry_run: bool,
    yes: bool,
) -> anyhow::Result<()> {
    let repo = Repository::current()?;
    let config = UserConfig::load().context("Failed to load config")?;
    let project_id = repo.project_identifier().ok();
    let commit_config = config.commit_generation(project_id.as_deref());

    let integration_target = repo.require_target_ref(target)?;
    // #3519: preview against the same upstream-aware span the real squash uses.
    // TODO(#3519 follow-up): unlike `handle_squash`, this path (--dry-run /
    // --show-prompt) has no test asserting it measures against the stale
    // target's upstream — a snapshot test pinning the preview output in that
    // topology would close the one unasserted consumer of `span_upstream`.
    let span_target = repo
        .span_upstream(&integration_target)?
        .unwrap_or(integration_target);

    let wt = repo.current_worktree();
    let current_branch = wt.branch()?.unwrap_or_else(|| "HEAD".to_string());

    let merge_base = repo
        .merge_base("HEAD", &span_target)?
        .context("Cannot generate squash message: no common ancestor with target branch")?;

    let range = format!("{}..HEAD", merge_base);
    let commit_details = repo.commit_message_details(&range)?;

    let repo_root = wt.root()?;
    let repo_name = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo");

    let env = CommandEnv::for_action(config)?;
    let ctx = env.context(yes);
    let project_append = resolve_template_for_preview(&ctx, &commit_config, dry_run)?;

    // `--dry-run` stages into a copy of the index so the previewed prompt spans
    // what a real squash would commit: `handle_squash` stages before generating
    // the message, and the prompt's diff is read from the index. `--show-prompt`
    // skips it, the cheap "what's already staged" path — the same split
    // `preview_commit` makes for `wt step commit`.
    let stage_mode = stage.unwrap_or(env.resolved().commit.stage());
    let temp_index = if dry_run && stage_mode != StageMode::None {
        let temp = wt.temp_index()?;
        temp.stage(stage_mode)?;
        Some(temp)
    } else {
        None
    };
    let staging_index = temp_index.as_ref();

    let inputs = crate::llm::SquashInputs {
        target_branch: &span_target,
        merge_base: &merge_base,
        commit_details: &commit_details,
        current_branch: &current_branch,
        repo_name,
        config: &commit_config,
        project_append: project_append.as_deref(),
        staging_index,
    };

    let prompt = inputs.prompt()?;
    if !dry_run {
        println!("{}", prompt);
        return Ok(());
    }
    let message = inputs.generate_message()?;
    print_dry_run(&prompt, &commit_config, &message)
}
