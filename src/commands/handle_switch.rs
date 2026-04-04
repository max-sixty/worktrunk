//! Switch command handler.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use worktrunk::HookType;
use worktrunk::config::{UserConfig, expand_template, template_references_var, validate_template};
use worktrunk::git::{GitError, Repository, SwitchSuggestionCtx, current_or_recover};
use worktrunk::styling::{eprintln, info_message};

use super::command_approval::approve_hooks;
use super::command_executor::{CommandContext, build_hook_context};
use super::hooks::{HookFailureStrategy, execute_hook};
use super::worktree::{
    SwitchBranchInfo, SwitchPlan, SwitchResult, execute_switch, offer_bare_repo_worktree_path_fix,
    path_mismatch, plan_switch,
};
use crate::output::{
    execute_user_command, handle_switch_output, is_shell_integration_active,
    prompt_shell_integration,
};

/// Options for the switch command
pub struct SwitchOptions<'a> {
    pub branch: &'a str,
    pub create: bool,
    pub base: Option<&'a str>,
    pub execute: Option<&'a str>,
    pub execute_args: &'a [String],
    pub yes: bool,
    pub clobber: bool,
    /// Resolved from --cd/--no-cd flags: Some(true) = cd, Some(false) = no cd, None = use config
    pub change_dir: Option<bool>,
    pub verify: bool,
}

/// Run pre-switch hooks before branch resolution or worktree creation.
///
/// The hook context uses the **destination** branch argument as `{{ branch }}`,
/// so hooks receive the user's raw input before resolution.
///
/// Directional vars:
/// - `base` / `base_worktree_path`: current (source) branch and worktree
/// - `target` / `target_worktree_path`: destination branch and worktree (if it exists)
pub(crate) fn run_pre_switch_hooks(
    repo: &Repository,
    config: &UserConfig,
    target_branch: &str,
    yes: bool,
) -> anyhow::Result<()> {
    let current_wt = repo.current_worktree();
    let current_path = current_wt.path().to_path_buf();
    let pre_ctx = CommandContext::new(repo, config, Some(target_branch), &current_path, yes);

    let pre_switch_approved = approve_hooks(&pre_ctx, &[HookType::PreSwitch])?;
    if pre_switch_approved {
        // Base vars: source (where the user currently is)
        let base_branch = current_wt.branch().ok().flatten().unwrap_or_default();
        let base_path_str = worktrunk::path::to_posix_path(&current_path.to_string_lossy());

        let mut extra_vars: Vec<(&str, &str)> = vec![
            ("base", &base_branch),
            ("base_worktree_path", &base_path_str),
        ];

        // Target vars and Active overrides: destination worktree.
        // For existing worktrees: override bare vars (worktree_path, worktree_name,
        // worktree) to point to the destination (Active), not the source.
        let dest_path = repo.worktree_for_branch(target_branch).ok().flatten();
        let dest_name = dest_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(|s| s.to_string());
        let dest_path_str = dest_path.map(|p| worktrunk::path::to_posix_path(&p.to_string_lossy()));

        extra_vars.push(("target", target_branch));
        if let Some(ref p) = dest_path_str {
            // Existing destination: override bare vars to Active (destination)
            extra_vars.push(("target_worktree_path", p));
            extra_vars.push(("worktree_path", p));
            extra_vars.push(("worktree", p)); // deprecated alias
            if let Some(ref name) = dest_name {
                extra_vars.push(("worktree_name", name));
            }
        }
        // For creates (dest_path_str is None): worktree_path keeps its default
        // (the source worktree = cwd). The planned destination path is computed
        // later during plan_switch, after pre-switch hooks complete.

        execute_hook(
            &pre_ctx,
            HookType::PreSwitch,
            &extra_vars,
            HookFailureStrategy::FailFast,
            None,
            crate::output::pre_hook_display_path(pre_ctx.worktree_path),
        )?;
    }
    Ok(())
}

/// Hook types that apply after a switch operation.
///
/// Creates trigger pre-start + post-start + post-switch hooks;
/// existing worktrees trigger only post-switch.
fn switch_post_hook_types(is_create: bool) -> &'static [HookType] {
    if is_create {
        &[
            HookType::PreStart,
            HookType::PostStart,
            HookType::PostSwitch,
        ]
    } else {
        &[HookType::PostSwitch]
    }
}

/// Approve switch hooks upfront and show "Commands declined" if needed.
///
/// Returns `true` if hooks are approved to run.
/// Returns `false` if hooks should be skipped (`!verify` or user declined).
pub(crate) fn approve_switch_hooks(
    repo: &Repository,
    config: &UserConfig,
    plan: &SwitchPlan,
    yes: bool,
    verify: bool,
) -> anyhow::Result<bool> {
    if !verify {
        return Ok(false);
    }

    let ctx = CommandContext::new(repo, config, plan.branch(), plan.worktree_path(), yes);
    let approved = approve_hooks(&ctx, switch_post_hook_types(plan.is_create()))?;

    if !approved {
        eprintln!(
            "{}",
            info_message(if plan.is_create() {
                "Commands declined, continuing worktree creation"
            } else {
                "Commands declined"
            })
        );
    }

    Ok(approved)
}

/// Compute extra template variables from a switch result.
///
/// Returns base branch context (`base`, `base_worktree_path`) for hooks and template expansion.
pub(crate) fn switch_extra_vars(result: &SwitchResult) -> Vec<(&str, &str)> {
    match result {
        SwitchResult::Created {
            base_branch,
            base_worktree_path,
            ..
        } => [
            base_branch.as_deref().map(|b| ("base", b)),
            base_worktree_path
                .as_deref()
                .map(|p| ("base_worktree_path", p)),
        ]
        .into_iter()
        .flatten()
        .collect(),
        SwitchResult::Existing { .. } | SwitchResult::AlreadyAt(_) => Vec::new(),
    }
}

/// Spawn post-switch (and post-start for creates) background hooks.
pub(crate) fn spawn_switch_background_hooks(
    repo: &Repository,
    config: &UserConfig,
    result: &SwitchResult,
    branch: Option<&str>,
    yes: bool,
    extra_vars: &[(&str, &str)],
    hooks_display_path: Option<&Path>,
) -> anyhow::Result<()> {
    let ctx = CommandContext::new(repo, config, branch, result.path(), yes);

    let mut flat_hooks = Vec::new();

    // Spawn each hook type's pipeline independently — pipelines carry a single
    // hook_type/source/context, so mixing types in one runner would give later
    // steps the wrong template variables (e.g., {{ hook_type }}).
    // Flat hooks are spawned as independent processes, so accumulating them
    // across hook types for a combined display message is fine.
    match super::hooks::prepare_background_hooks(
        &ctx,
        HookType::PostSwitch,
        extra_vars,
        hooks_display_path,
    )? {
        super::hooks::PreparedHooks::Flat(cmds) => flat_hooks.extend(cmds),
        super::hooks::PreparedHooks::Pipeline(steps) => {
            super::hooks::spawn_hook_pipeline(&ctx, steps)?;
        }
    }

    if matches!(result, SwitchResult::Created { .. }) {
        match super::hooks::prepare_background_hooks(
            &ctx,
            HookType::PostStart,
            extra_vars,
            hooks_display_path,
        )? {
            super::hooks::PreparedHooks::Flat(cmds) => flat_hooks.extend(cmds),
            super::hooks::PreparedHooks::Pipeline(steps) => {
                super::hooks::spawn_hook_pipeline(&ctx, steps)?;
            }
        }
    }

    super::hooks::spawn_background_hooks(&ctx, flat_hooks)
}

/// Handle the switch command.
pub fn handle_switch(
    opts: SwitchOptions<'_>,
    config: &mut UserConfig,
    binary_name: &str,
) -> anyhow::Result<()> {
    let SwitchOptions {
        branch,
        create,
        base,
        execute,
        execute_args,
        yes,
        clobber,
        change_dir: change_dir_flag,
        verify,
    } = opts;

    let (repo, is_recovered) = current_or_recover().context("Failed to switch worktree")?;

    // Resolve change_dir: explicit CLI flags > project config > global config > default (true)
    // Now that we have the repo, we can resolve project-specific config.
    let change_dir = change_dir_flag.unwrap_or_else(|| {
        let project_id = repo.project_identifier().ok();
        config.resolved(project_id.as_deref()).switch.cd()
    });

    // Build switch suggestion context for enriching error hints with --execute/trailing args.
    // Without this, errors like "branch already exists" would suggest `wt switch <branch>`
    // instead of the full `wt switch <branch> --execute=<cmd> -- <args>`.
    let suggestion_ctx = execute.map(|exec| {
        let escaped = shell_escape::escape(exec.into());
        SwitchSuggestionCtx {
            extra_flags: vec![format!("--execute={escaped}")],
            trailing_args: execute_args.to_vec(),
        }
    });

    // Run pre-switch hooks before branch resolution or worktree creation.
    // {{ branch }} receives the raw user input (before resolution).
    // Skip when recovered — the source worktree is gone, nothing to run hooks against.
    if verify && !is_recovered {
        run_pre_switch_hooks(&repo, config, branch, yes)?;
    }

    // Offer to fix worktree-path for bare repos with hidden directory names (.git, .bare).
    offer_bare_repo_worktree_path_fix(&repo, config)?;

    // Validate and resolve the target branch.
    let plan = plan_switch(&repo, branch, create, base, clobber, config).map_err(|err| {
        match suggestion_ctx {
            Some(ref ctx) => match err.downcast::<GitError>() {
                Ok(git_err) => GitError::WithSwitchSuggestion {
                    source: Box::new(git_err),
                    ctx: ctx.clone(),
                }
                .into(),
                Err(err) => err,
            },
            None => err,
        }
    })?;

    // "Approve at the Gate": collect and approve hooks upfront
    // This ensures approval happens once at the command entry point
    // If user declines, skip hooks but continue with worktree operation
    let hooks_approved = approve_switch_hooks(&repo, config, &plan, yes, verify)?;

    // Pre-flight: validate all templates before mutation (worktree creation).
    // Catches syntax errors and undefined variables early so a broken template
    // doesn't leave behind a half-created worktree that blocks re-running.
    validate_switch_templates(&repo, config, &plan, execute, execute_args, hooks_approved)?;

    // Capture source (base) worktree identity BEFORE the switch, so post-switch
    // hooks can reference where the user came from via {{ base }} / {{ base_worktree_path }}.
    let source_branch = repo
        .current_worktree()
        .branch()
        .ok()
        .flatten()
        .unwrap_or_default();
    let source_path = repo
        .current_worktree()
        .root()
        .ok()
        .map(|p| worktrunk::path::to_posix_path(&p.to_string_lossy()))
        .unwrap_or_default();

    // Execute the validated plan
    let (result, branch_info) = execute_switch(&repo, plan, config, yes, hooks_approved)?;

    // Early exit for benchmarking time-to-first-output
    if std::env::var_os("WORKTRUNK_FIRST_OUTPUT").is_some() {
        return Ok(());
    }

    // Compute path mismatch lazily (deferred from plan_switch for existing worktrees).
    // Skip for detached HEAD worktrees (branch is None) — no branch to compute expected path from.
    let branch_info = match &result {
        SwitchResult::Existing { path } | SwitchResult::AlreadyAt(path) => {
            let expected_path = branch_info
                .branch
                .as_deref()
                .and_then(|b| path_mismatch(&repo, b, path, config));
            SwitchBranchInfo {
                expected_path,
                ..branch_info
            }
        }
        _ => branch_info,
    };

    // Show success message (temporal locality: immediately after worktree operation)
    // Returns path to display in hooks when user's shell won't be in the worktree
    // Also shows worktree-path hint on first --create (before shell integration warning)
    //
    // When recovered from a deleted worktree, current_dir() and current_worktree().root()
    // both fail — fall back to repo_path() (the main worktree root).
    let fallback_path = repo.repo_path()?.to_path_buf();
    let cwd = std::env::current_dir().unwrap_or(fallback_path.clone());
    let source_root = repo.current_worktree().root().unwrap_or(fallback_path);
    let hooks_display_path =
        handle_switch_output(&result, &branch_info, change_dir, Some(&source_root), &cwd)?;

    // Offer shell integration if not already installed/active
    // (only shows prompt/hint when shell integration isn't working)
    // With --execute: show hints only (don't interrupt with prompt)
    // Skip when change_dir is false — user opted out of cd, so shell integration is irrelevant
    // Best-effort: don't fail switch if offer fails
    if change_dir && !is_shell_integration_active() {
        let skip_prompt = execute.is_some();
        let _ = prompt_shell_integration(config, binary_name, skip_prompt);
    }

    // Build extra vars for base/target context (used by both hooks and --execute).
    // "base" is the source worktree the user switched from (all switches),
    // or the branch they branched from (creates).
    let mut extra_vars = switch_extra_vars(&result);
    // For existing switches, add source worktree as base
    if matches!(
        result,
        SwitchResult::Existing { .. } | SwitchResult::AlreadyAt(_)
    ) {
        if !source_branch.is_empty() {
            extra_vars.push(("base", &source_branch));
        }
        if !source_path.is_empty() {
            extra_vars.push(("base_worktree_path", &source_path));
        }
    }

    // Spawn background hooks after success message
    // - post-switch: runs on ALL switches (shows "@ path" when shell won't be there)
    // - post-start: runs only when creating a NEW worktree
    // Batch hooks into a single message when both types are present
    if hooks_approved {
        spawn_switch_background_hooks(
            &repo,
            config,
            &result,
            branch_info.branch.as_deref(),
            yes,
            &extra_vars,
            hooks_display_path.as_deref(),
        )?;
    }

    // Execute user command after post-start hooks have been spawned
    // Note: execute_args requires execute via clap's `requires` attribute
    if let Some(cmd) = execute {
        // Build template context for expansion (includes base vars when creating)
        let ctx = CommandContext::new(
            &repo,
            config,
            branch_info.branch.as_deref(),
            result.path(),
            yes,
        );
        let template_vars = build_hook_context(&ctx, &extra_vars)?;
        let vars: HashMap<&str, &str> = template_vars
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        // Expand template variables in command (shell_escape: true for safety)
        let expanded_cmd = expand_template(cmd, &vars, true, &repo, "--execute command")?;

        // Append any trailing args (after --) to the execute command
        // Each arg is also expanded, then shell-escaped
        let full_cmd = if execute_args.is_empty() {
            expanded_cmd
        } else {
            let expanded_args: Result<Vec<_>, _> = execute_args
                .iter()
                .map(|arg| expand_template(arg, &vars, false, &repo, "--execute argument"))
                .collect();
            let escaped_args: Vec<_> = expanded_args?
                .iter()
                .map(|arg| shell_escape::escape(arg.into()).into_owned())
                .collect();
            format!("{} {}", expanded_cmd, escaped_args.join(" "))
        };
        execute_user_command(&full_cmd, hooks_display_path.as_deref())?;
    }

    Ok(())
}

/// Validate all templates that will be expanded after worktree creation.
///
/// Catches syntax errors and undefined variable references *before* the
/// irreversible worktree creation, so a broken template doesn't leave behind
/// a worktree that blocks re-running the command.
///
/// This is a best-effort pre-flight check: it catches definite errors (syntax,
/// unknown variables) but cannot catch failures from conditional variables that
/// are absent at expansion time (e.g., `upstream` when no tracking is configured).
/// Such late failures propagate as normal errors — no panics.
///
/// ## Why only switch needs pre-flight validation
///
/// Switch is the only command where template failure after mutation creates a
/// **blocking half-state**: `wt switch -c <branch>` creates a worktree, then if
/// hook/--execute expansion fails, the worktree exists and the same command
/// can't be re-run (branch already exists). Other commands don't have this
/// problem:
///
/// - **Pre-operation hooks** (pre-merge, pre-remove, pre-commit) run before the
///   irreversible operation, so template errors abort cleanly.
/// - **Post-operation hooks** (post-merge, post-remove) run after the operation
///   completed successfully — template failure is a missed notification, not a
///   blocking state. The user can fix the template and run `wt hook` manually.
///
/// Validates:
/// - `--execute` command template (if present)
/// - `--execute` trailing arg templates (if present)
/// - Hook templates (post-create, post-start, post-switch) from user and project config
fn validate_switch_templates(
    repo: &Repository,
    config: &UserConfig,
    plan: &SwitchPlan,
    execute: Option<&str>,
    execute_args: &[String],
    hooks_approved: bool,
) -> anyhow::Result<()> {
    // Validate --execute template and trailing args
    if let Some(cmd) = execute {
        validate_template(cmd, repo, "--execute command")?;
        for arg in execute_args {
            validate_template(arg, repo, "--execute argument")?;
        }
    }

    // Validate hook templates only when hooks will actually run
    if !hooks_approved {
        return Ok(());
    }

    let project_config = repo.load_project_config()?;
    let user_hooks = config.hooks(repo.project_identifier().ok().as_deref());

    for &hook_type in switch_post_hook_types(plan.is_create()) {
        let (user_cfg, proj_cfg) =
            super::hooks::lookup_hook_configs(&user_hooks, project_config.as_ref(), hook_type);
        for (source, cfg) in [("user", user_cfg), ("project", proj_cfg)] {
            if let Some(cfg) = cfg {
                for cmd in cfg.commands() {
                    // Skip full validation for lazy templates ({{ vars.X }}) —
                    // they're expanded at runtime after prior pipeline steps set
                    // the vars. Syntax is still checked by expand_commands.
                    if template_references_var(&cmd.template, "vars") {
                        continue;
                    }
                    let name = match &cmd.name {
                        Some(n) => format!("{source} {hook_type}:{n}"),
                        None => format!("{source} {hook_type} hook"),
                    };
                    validate_template(&cmd.template, repo, &name)?;
                }
            }
        }
    }

    Ok(())
}
