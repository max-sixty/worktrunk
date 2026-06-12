use std::path::Path;

use anyhow::Context;
use worktrunk::HookType;
use worktrunk::config::{MergeConfig, UserConfig};
use worktrunk::git::Repository;
use worktrunk::styling::{eprintln, info_message};

use super::command_approval::approve_commit_template_append;
use super::command_executor::FailureStrategy;
use super::commit::{CommitOptions, HookGate};
use super::context::CommandEnv;
use super::hook_plan::{ApprovedHookPlan, HookPlanBuilder, execute_planned_hook};
use super::hooks::HookAnnouncer;
use super::repository_ext::RepositoryCliExt;
use super::template_vars::TemplateVars;
use super::worktree::{
    FinishAfterMergeArgs, MergeOperations, PushKind, finish_after_merge, handle_no_ff_merge,
    handle_push,
};

/// Tri-state CLI overrides for the six `wt merge` boolean flags. `None` =
/// fall through to effective config; `Some(b)` = user explicitly chose.
pub struct MergeFlagOverrides {
    pub squash: Option<bool>,
    pub commit: Option<bool>,
    pub rebase: Option<bool>,
    pub remove: Option<bool>,
    pub ff: Option<bool>,
    pub verify: Option<bool>,
}

impl MergeFlagOverrides {
    pub fn from_cli(args: &crate::cli::MergeArgs) -> Self {
        Self {
            squash: crate::flag_pair(args.squash, args.no_squash),
            commit: crate::flag_pair(args.commit, args.no_commit),
            rebase: crate::flag_pair(args.rebase, args.no_rebase),
            remove: crate::flag_pair(args.remove, args.no_remove),
            ff: crate::flag_pair(args.ff, args.no_ff),
            verify: crate::flag_pair(args.verify, args.no_hooks || args.no_verify),
        }
    }

    /// Apply the override → effective-config → default-true chain.
    pub fn resolve(&self, config: &MergeConfig) -> ResolvedMergeFlags {
        ResolvedMergeFlags {
            squash: self.squash.unwrap_or(config.squash()),
            commit: self.commit.unwrap_or(config.commit()),
            rebase: self.rebase.unwrap_or(config.rebase()),
            remove: self.remove.unwrap_or(config.remove()),
            ff: self.ff.unwrap_or(config.ff()),
            verify: self.verify.unwrap_or(config.verify()),
        }
    }
}

pub struct ResolvedMergeFlags {
    pub squash: bool,
    pub commit: bool,
    pub rebase: bool,
    pub remove: bool,
    pub ff: bool,
    pub verify: bool,
}

/// Options for the merge command. `flags` carries tri-state CLI overrides for
/// the six boolean flags; `stage` is the same shape but for stage mode.
pub struct MergeOptions<'a> {
    pub target: Option<&'a str>,
    pub flags: MergeFlagOverrides,
    pub yes: bool,
    pub stage: Option<super::commit::StageMode>,
    pub format: crate::cli::SwitchFormat,
}

/// Build the frozen [`ApprovedHookPlan`] for the merge's covered hooks, gating
/// every project command once.
///
/// Every hook selects its commands from the invoking worktree's
/// `.config/wt.toml` — `repo`'s cwd, the feature worktree `wt merge` ran in.
/// The *anchor* — the executor's plan lookup key — is the worktree each hook
/// runs in:
///
/// - `pre-commit` / `post-commit` / `pre-merge` / `pre-remove` / `post-remove`
///   → the feature worktree.
/// - `post-merge` / `post-switch` → the merge destination.
///
/// `pre-commit`/`post-commit` execute via the unchanged commit/squash path
/// (no gate→exec state mutation precedes the commit), but are included here so
/// the single approval prompt is byte-identical to before; the TOCTOU-covered
/// hooks (`pre-merge` onward) execute *only* from the returned plan.
///
/// `Ok(None)` ⇒ the user declined; the caller proceeds without hooks.
#[allow(clippy::too_many_arguments)]
fn approve_merge_plan(
    repo: &Repository,
    config: &UserConfig,
    feature_root: &Path,
    destination_path: &Path,
    project_id: &str,
    commit: bool,
    verify: bool,
    will_remove: bool,
    squash_enabled: bool,
    yes: bool,
) -> anyhow::Result<Option<ApprovedHookPlan>> {
    let pid = Some(project_id);

    // `--no-hooks` (`!verify`) selects no hook, so the gate skips the config
    // read entirely.
    if !verify {
        return Ok(Some(ApprovedHookPlan::empty()));
    }

    // Every feature-worktree hook shares one anchor: the feature worktree's
    // canonical root, the exact path `finish_after_merge` records as
    // `RemoveResult::worktree_path` and `handle_merge` passes as the
    // `pre-merge` executor anchor, so every plan lookup is an exact match.
    // `pre-commit`/`post-commit` run via the unchanged `execute_hook` path and
    // are listed only so the single prompt is complete; their anchor is never
    // looked up.
    let mut feature_hooks = Vec::new();
    let will_create_commit = repo.current_worktree().is_dirty()? || squash_enabled;
    if commit && will_create_commit {
        feature_hooks.push(HookType::PreCommit);
        feature_hooks.push(HookType::PostCommit);
    }
    feature_hooks.push(HookType::PreMerge);
    if will_remove {
        feature_hooks.push(HookType::PreRemove);
        feature_hooks.push(HookType::PostRemove);
    }

    let project_config = repo.load_project_config()?;
    let mut builder = HookPlanBuilder::new(project_config.as_ref(), config, pid);
    builder.add(feature_root, &feature_hooks);
    // `post-merge` runs in the destination, and `post-switch` lands the user
    // there (the feature worktree is removed) — both still selected from the
    // invoking worktree's config.
    builder.add(destination_path, &[HookType::PostMerge]);
    if will_remove {
        builder.add(destination_path, &[HookType::PostSwitch]);
    }

    builder.finish().approve(pid, yes)
}

pub fn handle_merge(opts: MergeOptions<'_>) -> anyhow::Result<()> {
    let json_mode = opts.format == crate::cli::SwitchFormat::Json;
    let MergeOptions {
        target,
        flags,
        yes,
        stage,
        ..
    } = opts;

    // Load config once, run LLM setup prompt if committing, then reuse config
    let mut config = UserConfig::load().context("Failed to load config")?;
    if flags.commit.unwrap_or(true) {
        // One-time LLM setup prompt (errors logged internally; don't block merge)
        let _ = crate::output::prompt_commit_generation(&mut config);
    }

    let env = CommandEnv::for_action(config)?;
    let repo = &env.repo;
    let config = &env.config;
    // Merge requires being on a branch (can't merge from detached HEAD)
    let current_branch = env.require_branch("merge")?.to_string();

    // Get effective settings (project-specific merged with global, defaults applied)
    let resolved = env.resolved();

    let ResolvedMergeFlags {
        squash,
        commit,
        rebase,
        remove,
        ff,
        verify,
    } = flags.resolve(&resolved.merge);
    let stage_mode = stage.unwrap_or(resolved.commit.stage());

    // Cache current worktree for multiple queries
    let current_wt = repo.current_worktree();

    // Validate --no-commit: requires clean working tree
    if !commit {
        let dirty_files = current_wt.dirty_files()?;
        if !dirty_files.is_empty() {
            return Err(worktrunk::git::GitError::UncommittedChanges {
                action: Some("merge with --no-commit".into()),
                branch: Some(current_branch),
                force_hint: false,
                dirty_files,
            }
            .into());
        }
    }

    // --no-commit implies --no-squash
    let squash_enabled = squash && commit;

    // Get and validate target branch (must be a branch since we're updating it)
    let target_branch = repo.require_target_branch(target)?;
    // Worktree for target is optional: if present we use it for safety checks and as destination.
    let target_worktree_path = repo.worktree_for_branch(&target_branch)?;
    // Where `post-merge` / `post-remove` / `post-switch` run: the target
    // branch's worktree if it exists, else the primary worktree. Mirrors
    // `finish_after_merge`'s destination resolution. (Config is resolved from
    // the invoking worktree, not here — see `approve_merge_plan`.)
    let destination_path = match &target_worktree_path {
        Some(path) => path.clone(),
        None => repo.home_path()?,
    };

    // Quick check for command approval: will removal be attempted?
    // The authoritative guard is prepare_merge_removal (shared with wt remove),
    // but we need a lightweight answer here to decide whether to include
    // pre-remove/post-remove hooks in the batch approval prompt.
    let on_target = current_branch == target_branch;
    let remove_requested = remove && !on_target;

    // Build and approve the frozen hook plan once, at the gate. Every covered
    // hook (`pre-merge` / `post-merge` / `pre-remove` / `post-remove` /
    // `post-switch`) executes only from this immutable value — re-reading the
    // (by-then-rebased / merged) on-disk config is structurally impossible.
    let project_id = repo.project_identifier()?;
    // One anchor for every feature-worktree hook: the canonical root, the same
    // value `finish_after_merge` records as `RemoveResult::worktree_path`.
    let feature_root = current_wt.root()?;
    let plan = approve_merge_plan(
        repo,
        config,
        &feature_root,
        &destination_path,
        &project_id,
        commit,
        verify,
        remove_requested,
        squash_enabled,
        yes,
    )?;
    let approved = plan.is_some();
    let plan = plan.unwrap_or_else(ApprovedHookPlan::empty);

    // Commit-phase gate uses the original `verify` (before the shadow below) so it can
    // distinguish --no-hooks from declined-approval; CommitOptions and handle_squash
    // need that distinction to suppress a duplicate "(--no-hooks)" line.
    let commit_hooks = HookGate::from_approval(verify, approved);

    // If commands were declined, skip hooks but continue with merge.
    // Shadow verify to gate all subsequent hook execution (pre-merge, post-merge,
    // pre-remove, post-switch) on approval.
    let verify = if approved {
        verify
    } else {
        eprintln!(
            "{}",
            info_message("Commands declined, continuing merge without hooks")
        );
        false
    };

    // One announcer for the whole command's background hooks: post-commit
    // (from auto-commit or squash), post-remove + post-switch (from worktree
    // removal), and post-merge share a single `◎ Running …` line flushed at
    // the end.
    let mut announcer = HookAnnouncer::new(repo, false);

    // The project commit-append is gated independently of hook approval:
    // declining it drops only the append, never the (possibly already-approved)
    // hooks. Mirrors the standalone `wt step commit` path via the shared gate.
    let will_create_commit = current_wt.is_dirty()? || squash_enabled;
    let llm_configured = env
        .config
        .commit_generation(Some(&project_id))
        .is_configured();
    let project_append = if commit && will_create_commit && llm_configured {
        approve_commit_template_append(&env.context(yes))?
    } else {
        None
    };
    let guidance = super::step::PreApprovedGuidance::Resolved(project_append);

    // Handle uncommitted changes (skip if --no-commit) - track whether commit occurred
    let committed = if commit && current_wt.is_dirty()? {
        if squash_enabled {
            false // Squash path handles staging and committing
        } else {
            let ctx = env.context(yes);
            let mut options = CommitOptions::new(&ctx);
            options.target_branch = Some(&target_branch);
            options.hooks = commit_hooks;
            options.stage_mode = stage_mode;
            options.warn_about_untracked = stage_mode == super::commit::StageMode::All;
            options.show_no_squash_note = true;
            options.guidance = guidance.clone();

            let _ = options.commit(&mut announcer)?;
            true // Committed directly
        }
    } else {
        false // No dirty changes or --no-commit
    };

    // Squash commits if enabled - track whether squashing occurred.
    // Pass `commit_hooks` (not the shadowed `verify`) so handle_squash gets the
    // --no-hooks vs declined-approval distinction.
    let squashed = if squash_enabled {
        matches!(
            super::step::handle_squash(
                Some(&target_branch),
                yes,
                commit_hooks,
                Some(stage_mode),
                &mut announcer,
                guidance,
            )?,
            super::step::SquashResult::Squashed { .. }
        )
    } else {
        false
    };

    // Rebase onto target - track whether rebasing occurred
    let rebased = if rebase {
        // Auto-rebase onto target
        matches!(
            super::step::handle_rebase(Some(&target_branch))?,
            super::step::RebaseResult::Rebased { .. }
        )
    } else {
        // --no-rebase: verify already rebased, fail if not
        if !repo.is_rebased_onto(&target_branch)? {
            return Err(worktrunk::git::GitError::NotRebased { target_branch }.into());
        }
        false // Already rebased, no rebase occurred
    };

    // Run pre-merge checks unless --no-hooks was specified
    // Do this after commit/squash/rebase to validate the final state that will be pushed
    if verify {
        let ctx = env.context(yes);
        let mut vars = TemplateVars::new().with_target(&target_branch);
        if let Some(p) = target_worktree_path.as_deref() {
            vars = vars.with_target_worktree_path(p);
        }
        execute_planned_hook(
            &plan,
            &feature_root,
            &ctx,
            HookType::PreMerge,
            &vars.as_extra_vars(),
            FailureStrategy::FailFast,
            crate::output::pre_hook_display_path(ctx.worktree_path),
        )?;
    }

    // Merge to target branch
    let operations = Some(MergeOperations {
        committed,
        squashed,
        rebased,
    });
    if !ff {
        // Create a merge commit on the target branch via commit-tree + update-ref
        let _ = handle_no_ff_merge(Some(&target_branch), operations, &current_branch)?;
    } else {
        // Fast-forward push to target branch
        let _ = handle_push(Some(&target_branch), PushKind::MergeFastForward, operations)?;
    }

    let removed = finish_after_merge(
        repo,
        config,
        &env,
        &mut announcer,
        FinishAfterMergeArgs {
            current_branch: &current_branch,
            target_branch: &target_branch,
            target_worktree_path: target_worktree_path.as_deref(),
            remove,
            verify,
            yes,
            plan: &plan,
        },
    )?;

    announcer.flush()?;

    if json_mode {
        let output = serde_json::json!({
            "branch": current_branch,
            "target": target_branch,
            "committed": committed,
            "squashed": squashed,
            "rebased": rebased,
            "removed": removed,
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    }

    Ok(())
}
