//! Hook commands for `wt hook` subcommand.
//!
//! This module contains:
//! - `run_hook` - Execute a specific hook type
//! - `add_approvals` - Approve all project commands
//! - `clear_approvals` - Clear approved commands
//! - `handle_hook_show` - Display configured hooks

use std::collections::HashMap;
use std::fmt::Write as _;

use anyhow::Context;
use color_print::cformat;
use strum::IntoEnumIterator;
use worktrunk::HookType;
use worktrunk::config::{CommandConfig, ProjectConfig, UserConfig};
use worktrunk::git::{GitError, Repository};
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{
    INFO_SYMBOL, PROMPT_SYMBOL, eprintln, format_bash_with_gutter, format_heading, hint_message,
    info_message, success_message,
};

use super::command_approval::approve_hooks_filtered;
use super::command_executor::build_hook_context;

use super::command_executor::CommandContext;
use super::context::CommandEnv;
use super::hooks::execute_hook;
use super::hooks::{
    HookFailureStrategy, check_name_filter_matched, prepare_hook_commands, run_hook_with_filter,
    spawn_hook_commands_background,
};
use super::project_config::collect_commands_for_hooks;

/// Handle `wt hook` command
///
/// When explicitly invoking hooks, ALL hooks run (both user and project).
/// There's no skip flag - if you explicitly run hooks, all configured hooks run.
///
/// Works in detached HEAD state - `{{ branch }}` template variable will be "HEAD".
///
/// Custom variables from `--var KEY=VALUE` are merged into the template context,
/// allowing hooks to be tested with different values without being in that context.
///
/// The `foreground` parameter controls execution mode for hooks that normally run
/// in background (post-start, post-switch):
/// - `None` = use default behavior for this hook type
/// - `Some(true)` = run in foreground (for debugging)
/// - `Some(false)` = run in background (default for post-start/post-switch)
pub fn run_hook(
    hook_type: HookType,
    yes: bool,
    foreground: Option<bool>,
    name_filter: Option<&str>,
    custom_vars: &[(String, String)],
) -> anyhow::Result<()> {
    // Derive context from current environment (branch-optional for CI compatibility)
    let env = CommandEnv::for_action_branchless()?;
    let repo = &env.repo;
    let ctx = env.context(yes);

    // Load project config (optional - user hooks can run without project config)
    let project_config = repo.load_project_config()?;

    // "Approve at the Gate": approve project hooks upfront
    // Pass name_filter to only approve the targeted hook, not all hooks of this type
    let approved = approve_hooks_filtered(&ctx, &[hook_type], name_filter)?;
    // If declined, return early - the whole point of `wt hook` is to run hooks
    if !approved {
        eprintln!("{}", worktrunk::styling::info_message("Commands declined"));
        return Ok(());
    }

    // Build extra vars from command-line --var flags
    let custom_vars_refs: Vec<(&str, &str)> = custom_vars
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    /// Helper to require at least one hook is configured (for standalone `wt hook` command)
    fn require_hooks(
        user: Option<&CommandConfig>,
        project: Option<&CommandConfig>,
        hook_type: HookType,
    ) -> anyhow::Result<()> {
        if user.is_none() && project.is_none() {
            return Err(worktrunk::git::GitError::Other {
                message: format!("No {hook_type} hook configured; checked both user and project"),
            }
            .into());
        }
        Ok(())
    }

    // Get effective user hooks (global + per-project merged)
    let user_hooks = ctx.config.hooks(ctx.project_id().as_deref());

    // Execute the hook based on type
    match hook_type {
        HookType::PostCreate => {
            let user_config = user_hooks.post_create.as_ref();
            let project_config = project_config
                .as_ref()
                .and_then(|c| c.hooks.post_create.as_ref());
            require_hooks(user_config, project_config, hook_type)?;
            // Manual wt hook: user stays at cwd (no cd happens)
            run_hook_with_filter(
                &ctx,
                user_config,
                project_config,
                hook_type,
                &custom_vars_refs,
                HookFailureStrategy::FailFast,
                name_filter,
                crate::output::pre_hook_display_path(ctx.worktree_path),
            )
        }
        HookType::PostStart => {
            let user_config = user_hooks.post_start.as_ref();
            let project_config = project_config
                .as_ref()
                .and_then(|c| c.hooks.post_start.as_ref());
            require_hooks(user_config, project_config, hook_type)?;

            // Default to background (matching normal behavior during switch)
            // Use --foreground to run in foreground for debugging
            if !foreground.unwrap_or(false) {
                let commands = prepare_hook_commands(
                    &ctx,
                    user_config,
                    project_config,
                    hook_type,
                    &custom_vars_refs,
                    name_filter,
                    None,
                )?;
                check_name_filter_matched(
                    name_filter,
                    commands.len(),
                    user_config,
                    project_config,
                )?;
                spawn_hook_commands_background(&ctx, commands, hook_type)
            } else {
                run_hook_with_filter(
                    &ctx,
                    user_config,
                    project_config,
                    hook_type,
                    &custom_vars_refs,
                    HookFailureStrategy::Warn,
                    name_filter,
                    crate::output::pre_hook_display_path(ctx.worktree_path),
                )
            }
        }
        HookType::PostSwitch => {
            let user_config = user_hooks.post_switch.as_ref();
            let project_config = project_config
                .as_ref()
                .and_then(|c| c.hooks.post_switch.as_ref());
            require_hooks(user_config, project_config, hook_type)?;

            // Default to background (matching normal behavior during switch)
            // Use --foreground to run in foreground for debugging
            if !foreground.unwrap_or(false) {
                let commands = prepare_hook_commands(
                    &ctx,
                    user_config,
                    project_config,
                    hook_type,
                    &custom_vars_refs,
                    name_filter,
                    None,
                )?;
                check_name_filter_matched(
                    name_filter,
                    commands.len(),
                    user_config,
                    project_config,
                )?;
                spawn_hook_commands_background(&ctx, commands, hook_type)
            } else {
                run_hook_with_filter(
                    &ctx,
                    user_config,
                    project_config,
                    hook_type,
                    &custom_vars_refs,
                    HookFailureStrategy::Warn,
                    name_filter,
                    crate::output::pre_hook_display_path(ctx.worktree_path),
                )
            }
        }
        HookType::PreCommit => {
            let user_config = user_hooks.pre_commit.as_ref();
            let project_config = project_config
                .as_ref()
                .and_then(|c| c.hooks.pre_commit.as_ref());
            require_hooks(user_config, project_config, hook_type)?;
            // Pre-commit hook can optionally use target branch context
            // Custom vars take precedence (added last)
            let target_branch = repo.default_branch();
            let mut extra_vars: Vec<(&str, &str)> = target_branch
                .as_deref()
                .into_iter()
                .map(|t| ("target", t))
                .collect();
            extra_vars.extend(&custom_vars_refs);
            // Manual wt hook: user stays at cwd (no cd happens)
            run_hook_with_filter(
                &ctx,
                user_config,
                project_config,
                hook_type,
                &extra_vars,
                HookFailureStrategy::FailFast,
                name_filter,
                crate::output::pre_hook_display_path(ctx.worktree_path),
            )
        }
        HookType::PreMerge => {
            // Use current branch as target (matches approval prompt for wt hook)
            let mut vars = vec![("target", ctx.branch_or_head())];
            vars.extend(custom_vars_refs.iter().cloned());
            execute_hook(
                &ctx,
                HookType::PreMerge,
                &vars,
                HookFailureStrategy::FailFast,
                name_filter,
                crate::output::pre_hook_display_path(ctx.worktree_path),
            )
        }
        HookType::PostMerge => {
            // Manual wt hook: user stays at cwd (no cd happens)
            let mut vars = vec![("target", ctx.branch_or_head())];
            vars.extend(custom_vars_refs.iter().cloned());
            execute_hook(
                &ctx,
                HookType::PostMerge,
                &vars,
                HookFailureStrategy::Warn,
                name_filter,
                crate::output::pre_hook_display_path(ctx.worktree_path),
            )
        }
        HookType::PreRemove => {
            // Manual wt hook: user stays at cwd (no cd happens)
            execute_hook(
                &ctx,
                HookType::PreRemove,
                &custom_vars_refs,
                HookFailureStrategy::FailFast,
                name_filter,
                crate::output::pre_hook_display_path(ctx.worktree_path),
            )
        }
        HookType::PostRemove => {
            let user_config = user_hooks.post_remove.as_ref();
            let project_config = project_config
                .as_ref()
                .and_then(|c| c.hooks.post_remove.as_ref());
            require_hooks(user_config, project_config, hook_type)?;

            // Default to background (matching normal behavior during remove)
            // Use --foreground to run in foreground for debugging
            if !foreground.unwrap_or(false) {
                let commands = prepare_hook_commands(
                    &ctx,
                    user_config,
                    project_config,
                    hook_type,
                    &custom_vars_refs,
                    name_filter,
                    None,
                )?;
                check_name_filter_matched(
                    name_filter,
                    commands.len(),
                    user_config,
                    project_config,
                )?;
                spawn_hook_commands_background(&ctx, commands, hook_type)
            } else {
                run_hook_with_filter(
                    &ctx,
                    user_config,
                    project_config,
                    hook_type,
                    &custom_vars_refs,
                    HookFailureStrategy::Warn,
                    name_filter,
                    crate::output::pre_hook_display_path(ctx.worktree_path),
                )
            }
        }
    }
}

/// Handle `wt hook approvals add` command - approve all commands in the project
pub fn add_approvals(show_all: bool) -> anyhow::Result<()> {
    use super::command_approval::approve_command_batch;

    let repo = Repository::current()?;
    let project_id = repo.project_identifier()?;
    let config = UserConfig::load().context("Failed to load config")?;

    // Load project config (error if missing - this command requires it)
    let config_path = repo
        .current_worktree()
        .root()?
        .join(".config")
        .join("wt.toml");
    let project_config = repo
        .load_project_config()?
        .ok_or(GitError::ProjectConfigNotFound { config_path })?;

    // Collect all commands from the project config
    let all_hooks: Vec<_> = HookType::iter().collect();
    let commands = collect_commands_for_hooks(&project_config, &all_hooks);

    if commands.is_empty() {
        eprintln!("{}", info_message("No commands configured in project"));
        return Ok(());
    }

    // Filter to only unapproved commands (unless --all is specified)
    let commands_to_approve = if !show_all {
        let unapproved: Vec<_> = commands
            .into_iter()
            .filter(|cmd| !config.is_command_approved(&project_id, &cmd.command.template))
            .collect();

        if unapproved.is_empty() {
            eprintln!("{}", info_message("All commands already approved"));
            return Ok(());
        }

        unapproved
    } else {
        commands
    };

    // Call the approval prompt (yes=false to require interactive approval and save)
    // When show_all=true, we've already included all commands in commands_to_approve
    // When show_all=false, we've already filtered to unapproved commands
    // So we pass skip_approval_filter=true to prevent double-filtering
    let approved = approve_command_batch(&commands_to_approve, &project_id, &config, false, true)?;

    // Show result
    if approved {
        eprintln!("{}", success_message("Commands approved & saved to config"));
    } else {
        eprintln!("{}", info_message("Commands declined"));
    }

    Ok(())
}

/// Handle `wt hook approvals clear` command - clear approved commands
pub fn clear_approvals(global: bool) -> anyhow::Result<()> {
    let mut config = UserConfig::load().context("Failed to load config")?;

    if global {
        // Clear all approvals for all projects (preserving other per-project settings)
        let projects_with_approvals: Vec<String> = config
            .projects
            .iter()
            .filter(|(_, p)| !p.approved_commands.is_empty())
            .map(|(id, _)| id.clone())
            .collect();

        if projects_with_approvals.is_empty() {
            eprintln!("{}", info_message("No approvals to clear"));
            return Ok(());
        }

        let project_count = projects_with_approvals.len();

        // Clear only approved_commands, preserve other settings
        for project_id in &projects_with_approvals {
            if let Some(project_config) = config.projects.get_mut(project_id) {
                project_config.approved_commands.clear();
                // Remove project entry only if it has no other settings
                if project_config.is_empty() {
                    config.projects.remove(project_id);
                }
            }
        }
        config.save().context("Failed to save config")?;

        eprintln!(
            "{}",
            success_message(format!(
                "Cleared approvals for {project_count} project{}",
                if project_count == 1 { "" } else { "s" }
            ))
        );
    } else {
        // Clear approvals for current project (default)
        let repo = Repository::current()?;
        let project_id = repo.project_identifier()?;

        // Check if project has any approvals (not just if it exists)
        let had_approvals = config
            .projects
            .get(&project_id)
            .is_some_and(|p| !p.approved_commands.is_empty());

        if !had_approvals {
            eprintln!("{}", info_message("No approvals to clear for this project"));
            return Ok(());
        }

        // Count approvals before removing
        let approval_count = config
            .projects
            .get(&project_id)
            .map(|p| p.approved_commands.len())
            .unwrap_or(0);

        config
            .revoke_project(&project_id, None)
            .context("Failed to clear project approvals")?;

        eprintln!(
            "{}",
            success_message(format!(
                "Cleared {approval_count} approval{} for this project",
                if approval_count == 1 { "" } else { "s" }
            ))
        );
    }

    Ok(())
}

/// Handle `wt hook show` command - display configured hooks
pub fn handle_hook_show(hook_type_filter: Option<&str>, expanded: bool) -> anyhow::Result<()> {
    use crate::help_pager::show_help_in_pager;

    let repo = Repository::current()?;
    let config = UserConfig::load().context("Failed to load user config")?;
    let project_config = repo.load_project_config()?;
    let project_id = repo.project_identifier().ok();

    // Parse hook type filter if provided
    let filter: Option<HookType> = hook_type_filter.map(|s| match s {
        "post-create" => HookType::PostCreate,
        "post-start" => HookType::PostStart,
        "post-switch" => HookType::PostSwitch,
        "pre-commit" => HookType::PreCommit,
        "pre-merge" => HookType::PreMerge,
        "post-merge" => HookType::PostMerge,
        "pre-remove" => HookType::PreRemove,
        "post-remove" => HookType::PostRemove,
        _ => unreachable!("clap validates hook type"),
    });

    // Build context for template expansion (only used if --expanded)
    // Need to keep CommandEnv alive for the lifetime of ctx
    // Uses branchless mode - template expansion uses "HEAD" in detached HEAD state
    let env = if expanded {
        Some(CommandEnv::for_action_branchless()?)
    } else {
        None
    };
    let ctx = env.as_ref().map(|e| e.context(false));

    let mut output = String::new();

    // Render user hooks
    render_user_hooks(&mut output, &config, filter, ctx.as_ref())?;
    output.push('\n');

    // Render project hooks
    render_project_hooks(
        &mut output,
        &repo,
        project_config.as_ref(),
        &config,
        project_id.as_deref(),
        filter,
        ctx.as_ref(),
    )?;

    // Display through pager (fall back to stderr if pager unavailable)
    if show_help_in_pager(&output, true).is_err() {
        worktrunk::styling::eprintln!("{}", output);
    }

    Ok(())
}

/// Render user hooks section
fn render_user_hooks(
    out: &mut String,
    config: &UserConfig,
    filter: Option<HookType>,
    ctx: Option<&CommandContext>,
) -> anyhow::Result<()> {
    let config_path = worktrunk::config::get_config_path();

    writeln!(
        out,
        "{}",
        format_heading(
            "USER HOOKS",
            Some(
                &config_path
                    .as_ref()
                    .map(|p| format_path_for_display(p))
                    .unwrap_or_else(|| "(not found)".to_string())
            )
        )
    )?;

    // Collect all user hooks (global hooks from the user config file)
    // Note: uses overrides.hooks for display, not the merged hooks() accessor
    let user_hooks = &config.configs.hooks;
    let hooks = [
        (HookType::PostCreate, &user_hooks.post_create),
        (HookType::PostStart, &user_hooks.post_start),
        (HookType::PostSwitch, &user_hooks.post_switch),
        (HookType::PreCommit, &user_hooks.pre_commit),
        (HookType::PreMerge, &user_hooks.pre_merge),
        (HookType::PostMerge, &user_hooks.post_merge),
        (HookType::PreRemove, &user_hooks.pre_remove),
        (HookType::PostRemove, &user_hooks.post_remove),
    ];

    let mut has_any = false;
    for (hook_type, hook_config) in hooks {
        // Apply filter if specified
        if let Some(f) = filter
            && f != hook_type
        {
            continue;
        }

        if let Some(cfg) = hook_config {
            has_any = true;
            render_hook_commands(out, hook_type, cfg, None, ctx)?;
        }
    }

    if !has_any {
        writeln!(out, "{}", hint_message("(none configured)"))?;
    }

    Ok(())
}

/// Render project hooks section
fn render_project_hooks(
    out: &mut String,
    repo: &Repository,
    project_config: Option<&ProjectConfig>,
    user_config: &UserConfig,
    project_id: Option<&str>,
    filter: Option<HookType>,
    ctx: Option<&CommandContext>,
) -> anyhow::Result<()> {
    let repo_root = repo.current_worktree().root()?;
    let config_path = repo_root.join(".config").join("wt.toml");

    writeln!(
        out,
        "{}",
        format_heading(
            "PROJECT HOOKS",
            Some(&format_path_for_display(&config_path))
        )
    )?;

    let Some(config) = project_config else {
        writeln!(out, "{}", hint_message("(not found)"))?;
        return Ok(());
    };

    // Collect all project hooks
    let hooks = [
        (HookType::PostCreate, &config.hooks.post_create),
        (HookType::PostStart, &config.hooks.post_start),
        (HookType::PostSwitch, &config.hooks.post_switch),
        (HookType::PreCommit, &config.hooks.pre_commit),
        (HookType::PreMerge, &config.hooks.pre_merge),
        (HookType::PostMerge, &config.hooks.post_merge),
        (HookType::PreRemove, &config.hooks.pre_remove),
        (HookType::PostRemove, &config.hooks.post_remove),
    ];

    let mut has_any = false;
    for (hook_type, hook_config) in hooks {
        // Apply filter if specified
        if let Some(f) = filter
            && f != hook_type
        {
            continue;
        }

        if let Some(cfg) = hook_config {
            has_any = true;
            render_hook_commands(out, hook_type, cfg, Some((user_config, project_id)), ctx)?;
        }
    }

    if !has_any {
        writeln!(out, "{}", hint_message("(none configured)"))?;
    }

    Ok(())
}

/// Render commands for a single hook type
fn render_hook_commands(
    out: &mut String,
    hook_type: HookType,
    config: &CommandConfig,
    // For project hooks: (user_config, project_id) to check approval status
    approval_context: Option<(&UserConfig, Option<&str>)>,
    ctx: Option<&CommandContext>,
) -> anyhow::Result<()> {
    let commands = config.commands();
    if commands.is_empty() {
        return Ok(());
    }

    for cmd in commands {
        // Build label: "hook-type name:" or "hook-type:"
        let label = match &cmd.name {
            Some(name) => cformat!("{hook_type} <bold>{name}</>:"),
            None => format!("{hook_type}:"),
        };

        // Check approval status for project hooks
        let needs_approval = if let Some((user_config, Some(project_id))) = approval_context {
            !user_config.is_command_approved(project_id, &cmd.template)
        } else {
            false
        };

        // Use ❯ for needs approval, ○ for approved/user hooks
        let (emoji, suffix) = if needs_approval {
            (PROMPT_SYMBOL, cformat!(" <dim>(requires approval)</>"))
        } else {
            (INFO_SYMBOL, String::new())
        };

        writeln!(out, "{emoji} {label}{suffix}")?;

        // Show template or expanded command
        let command_text = if let Some(command_ctx) = ctx {
            // Expand template with current context
            expand_command_template(&cmd.template, command_ctx, hook_type)
        } else {
            cmd.template.clone()
        };

        writeln!(out, "{}", format_bash_with_gutter(&command_text))?;
    }

    Ok(())
}

/// Expand a command template with context variables
fn expand_command_template(template: &str, ctx: &CommandContext, hook_type: HookType) -> String {
    // Build extra vars based on hook type (same logic as run_hook approval)
    let default_branch = ctx.repo.default_branch();
    let extra_vars: Vec<(&str, &str)> = match hook_type {
        HookType::PreCommit => {
            // Pre-commit uses default branch as target (for comparison context)
            default_branch
                .as_deref()
                .into_iter()
                .map(|t| ("target", t))
                .collect()
        }
        HookType::PreMerge | HookType::PostMerge => {
            // Pre-merge and post-merge use current branch as target
            vec![("target", ctx.branch_or_head())]
        }
        _ => Vec::new(),
    };
    let template_ctx = build_hook_context(ctx, &extra_vars);
    let vars: HashMap<&str, &str> = template_ctx
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    // Use the standard template expansion (shell-escaped)
    // On any error, show both the template and error message
    worktrunk::config::expand_template(template, &vars, true, ctx.repo, "hook preview")
        .unwrap_or_else(|err| format!("# Template error: {}\n{}", err, template))
}
