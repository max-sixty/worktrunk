//! Hook commands for `wt hook` subcommand.
//!
//! This module contains:
//! - `run_hook` - Execute a specific hook type
//! - `add_approvals` - Approve all project commands
//! - `clear_approvals` - Clear approved commands
//! - `handle_hook_show` - Display configured hooks

use anyhow::Context;
use color_print::cformat;
use std::fmt::Write as _;
use worktrunk::HookType;
use worktrunk::config::{CommandConfig, ProjectConfig, WorktrunkConfig};
use worktrunk::git::{GitError, Repository};
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{
    INFO_SYMBOL, PROMPT_SYMBOL, format_bash_with_gutter, format_heading, hint_message,
    info_message, success_message,
};

use super::command_executor::CommandContext;
use super::context::CommandEnv;
use super::hooks::{
    HookFailureStrategy, check_name_filter_matched, prepare_hook_commands, run_hook_with_filter,
    spawn_hook_commands_background,
};
use super::merge::{
    execute_post_merge_commands, execute_pre_remove_commands, run_pre_merge_commands,
};
use super::project_config::collect_commands_for_hooks;

/// Handle `wt hook` command
///
/// When explicitly invoking hooks, ALL hooks run (both user and project).
/// There's no skip flag - if you explicitly run hooks, all configured hooks run.
///
/// Works in detached HEAD state - `{{ branch }}` template variable will be "HEAD".
pub fn run_hook(hook_type: HookType, yes: bool, name_filter: Option<&str>) -> anyhow::Result<()> {
    use super::command_approval::approve_hooks_filtered;

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
        crate::output::print(worktrunk::styling::info_message("Commands declined"))?;
        return Ok(());
    }

    // TODO: Add support for custom variable overrides (e.g., --var key=value)
    // This would allow testing hooks with different contexts without being in that context

    // Helper to get user hook config
    macro_rules! user_hook {
        ($field:ident) => {
            ctx.config.hooks.$field.as_ref()
        };
    }

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

    // Execute the hook based on type
    match hook_type {
        HookType::PostCreate => {
            let user_config = user_hook!(post_create);
            let project_config = project_config
                .as_ref()
                .and_then(|c| c.hooks.post_create.as_ref());
            require_hooks(user_config, project_config, hook_type)?;
            run_hook_with_filter(
                &ctx,
                user_config,
                project_config,
                hook_type,
                &[],
                HookFailureStrategy::FailFast,
                name_filter,
                None,
            )
        }
        HookType::PostStart => {
            // post-start hooks spawn in background (matching their normal behavior during switch)
            let user_config = user_hook!(post_start);
            let project_config = project_config
                .as_ref()
                .and_then(|c| c.hooks.post_start.as_ref());
            require_hooks(user_config, project_config, hook_type)?;
            let commands = prepare_hook_commands(
                &ctx,
                user_config,
                project_config,
                hook_type,
                &[],
                name_filter,
                None,
            )?;
            check_name_filter_matched(name_filter, commands.len(), user_config, project_config)?;
            spawn_hook_commands_background(&ctx, commands, hook_type)
        }
        HookType::PostSwitch => {
            // post-switch hooks spawn in background (matching their normal behavior during switch)
            let user_config = user_hook!(post_switch);
            let project_config = project_config
                .as_ref()
                .and_then(|c| c.hooks.post_switch.as_ref());
            require_hooks(user_config, project_config, hook_type)?;
            let commands = prepare_hook_commands(
                &ctx,
                user_config,
                project_config,
                hook_type,
                &[],
                name_filter,
                None,
            )?;
            check_name_filter_matched(name_filter, commands.len(), user_config, project_config)?;
            spawn_hook_commands_background(&ctx, commands, hook_type)
        }
        HookType::PreCommit => {
            let user_config = user_hook!(pre_commit);
            let project_config = project_config
                .as_ref()
                .and_then(|c| c.hooks.pre_commit.as_ref());
            require_hooks(user_config, project_config, hook_type)?;
            // Pre-commit hook can optionally use target branch context
            let target_branch = repo.default_branch().ok();
            let extra_vars: Vec<(&str, &str)> = target_branch
                .as_deref()
                .into_iter()
                .map(|t| ("target", t))
                .collect();
            run_hook_with_filter(
                &ctx,
                user_config,
                project_config,
                hook_type,
                &extra_vars,
                HookFailureStrategy::FailFast,
                name_filter,
                None,
            )
        }
        HookType::PreMerge => {
            // pre-merge, post-merge, pre-remove use functions from merge.rs
            // which already handle user hooks (approval already happened at gate)
            // Use current branch as target (matches approval prompt for wt hook)
            let project_cfg = project_config.unwrap_or_default();
            run_pre_merge_commands(&project_cfg, &ctx, ctx.branch_or_head(), name_filter)
        }
        HookType::PostMerge => {
            // Use current branch as target (matches approval prompt for wt hook)
            // No display_path - user is already in the directory where hooks run
            execute_post_merge_commands(&ctx, ctx.branch_or_head(), name_filter, None)
        }
        HookType::PreRemove => execute_pre_remove_commands(&ctx, name_filter),
    }
}

/// Handle `wt hook approvals add` command - approve all commands in the project
pub fn add_approvals(show_all: bool) -> anyhow::Result<()> {
    use super::command_approval::approve_command_batch;
    use worktrunk::config::WorktrunkConfig;

    let repo = Repository::current();
    let project_id = repo.project_identifier()?;
    let config = WorktrunkConfig::load().context("Failed to load config")?;

    // Load project config (error if missing - this command requires it)
    let config_path = repo.worktree_root()?.join(".config").join("wt.toml");
    let project_config = repo
        .load_project_config()?
        .ok_or(GitError::ProjectConfigNotFound { config_path })?;

    // Collect all commands from the project config
    let all_hooks = [
        HookType::PostCreate,
        HookType::PostStart,
        HookType::PreCommit,
        HookType::PreMerge,
        HookType::PostMerge,
    ];
    let commands = collect_commands_for_hooks(&project_config, &all_hooks);

    if commands.is_empty() {
        crate::output::print(info_message("No commands configured in project"))?;
        return Ok(());
    }

    // Filter to only unapproved commands (unless --all is specified)
    let commands_to_approve = if !show_all {
        let unapproved: Vec<_> = commands
            .into_iter()
            .filter(|cmd| !config.is_command_approved(project_id, &cmd.command.template))
            .collect();

        if unapproved.is_empty() {
            crate::output::print(info_message("All commands already approved"))?;
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
    let approved = approve_command_batch(&commands_to_approve, project_id, &config, false, true)?;

    // Show result
    if approved {
        crate::output::print(success_message("Commands approved & saved to config"))?;
    } else {
        crate::output::print(info_message("Commands declined"))?;
    }

    Ok(())
}

/// Handle `wt hook approvals clear` command - clear approved commands
pub fn clear_approvals(global: bool) -> anyhow::Result<()> {
    use worktrunk::config::WorktrunkConfig;

    let mut config = WorktrunkConfig::load().context("Failed to load config")?;

    if global {
        // Clear all approvals for all projects
        let project_count = config.projects.len();

        if project_count == 0 {
            crate::output::print(info_message("No approvals to clear"))?;
            return Ok(());
        }

        config.projects.clear();
        config.save().context("Failed to save config")?;

        crate::output::print(success_message(format!(
            "Cleared approvals for {project_count} project{}",
            if project_count == 1 { "" } else { "s" }
        )))?;
    } else {
        // Clear approvals for current project (default)
        let repo = Repository::current();
        let project_id = repo.project_identifier()?;

        // Check if project has any approvals
        let had_approvals = config.projects.contains_key(project_id);

        if !had_approvals {
            crate::output::print(info_message("No approvals to clear for this project"))?;
            return Ok(());
        }

        // Count approvals before removing
        let approval_count = config
            .projects
            .get(project_id)
            .map(|p| p.approved_commands.len())
            .unwrap_or(0);

        config
            .revoke_project(project_id, None)
            .context("Failed to clear project approvals")?;

        crate::output::print(success_message(format!(
            "Cleared {approval_count} approval{} for this project",
            if approval_count == 1 { "" } else { "s" }
        )))?;
    }

    Ok(())
}

/// Handle `wt hook show` command - display configured hooks
pub fn handle_hook_show(hook_type_filter: Option<&str>, expanded: bool) -> anyhow::Result<()> {
    use crate::help_pager::show_help_in_pager;

    let repo = Repository::current();
    let config = WorktrunkConfig::load().context("Failed to load user config")?;
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
        project_id,
        filter,
        ctx.as_ref(),
    )?;

    // Display through pager (fall back to stderr if pager unavailable)
    if show_help_in_pager(&output).is_err() {
        worktrunk::styling::eprintln!("{}", output);
    }

    Ok(())
}

/// Render user hooks section
fn render_user_hooks(
    out: &mut String,
    config: &WorktrunkConfig,
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

    // Collect all user hooks
    let hooks = [
        (HookType::PostCreate, &config.hooks.post_create),
        (HookType::PostStart, &config.hooks.post_start),
        (HookType::PostSwitch, &config.hooks.post_switch),
        (HookType::PreCommit, &config.hooks.pre_commit),
        (HookType::PreMerge, &config.hooks.pre_merge),
        (HookType::PostMerge, &config.hooks.post_merge),
        (HookType::PreRemove, &config.hooks.pre_remove),
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
    user_config: &WorktrunkConfig,
    project_id: Option<&str>,
    filter: Option<HookType>,
    ctx: Option<&CommandContext>,
) -> anyhow::Result<()> {
    let repo_root = repo.worktree_root()?;
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
    approval_context: Option<(&WorktrunkConfig, Option<&str>)>,
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
    use super::command_executor::build_hook_context;

    // Build extra vars based on hook type (same logic as run_hook approval)
    let default_branch = ctx.repo.default_branch().ok();
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
    let vars: std::collections::HashMap<&str, &str> = template_ctx
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    // Use the standard template expansion (shell-escaped)
    worktrunk::config::expand_template(template, &vars, true)
        .unwrap_or_else(|_| template.to_string())
}
