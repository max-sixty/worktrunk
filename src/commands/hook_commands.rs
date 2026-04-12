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
use worktrunk::config::{Approvals, CommandConfig, ProjectConfig, UserConfig};
use worktrunk::git::{GitError, Repository};
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{
    INFO_SYMBOL, PROMPT_SYMBOL, eprintln, format_bash_with_gutter, format_heading, hint_message,
    info_message, success_message, warning_message,
};

use super::command_approval::approve_hooks_filtered;
use super::command_executor::build_hook_context;

use super::command_executor::CommandContext;
use super::context::CommandEnv;
use super::hooks::{
    HookCommandSpec, HookFailureStrategy, check_name_filter_matched, command_summary_name,
    count_sourced_commands, prepare_background_hooks, prepare_sourced_steps, run_hook_with_filter,
    spawn_hook_pipeline,
};
use super::project_config::collect_commands_for_hooks;

fn run_filtered_hook(
    ctx: &CommandContext,
    user_config: Option<&CommandConfig>,
    project_config: Option<&CommandConfig>,
    hook_type: HookType,
    extra_vars: &[(&str, &str)],
    name_filters: &[String],
    failure_strategy: HookFailureStrategy,
) -> anyhow::Result<()> {
    run_hook_with_filter(
        ctx,
        HookCommandSpec {
            user_config,
            project_config,
            hook_type,
            extra_vars,
            name_filters,
            display_path: crate::output::pre_hook_display_path(ctx.worktree_path),
        },
        failure_strategy,
    )
}

fn run_post_hook(
    ctx: &CommandContext,
    foreground: Option<bool>,
    user_config: Option<&CommandConfig>,
    project_config: Option<&CommandConfig>,
    hook_type: HookType,
    extra_vars: &[(&str, &str)],
    name_filters: &[String],
) -> anyhow::Result<()> {
    // Default to background execution; --foreground is for debugging.
    if !foreground.unwrap_or(false) {
        if !name_filters.is_empty() {
            let steps = prepare_sourced_steps(
                ctx,
                HookCommandSpec {
                    user_config,
                    project_config,
                    hook_type,
                    extra_vars,
                    name_filters,
                    display_path: None,
                },
            )?;
            check_name_filter_matched(
                name_filters,
                count_sourced_commands(&steps),
                user_config,
                project_config,
            )?;
            return spawn_hook_pipeline(ctx, steps);
        }

        // No name filter: prepare as pipeline steps and spawn per source.
        for steps in prepare_background_hooks(ctx, hook_type, extra_vars, None)? {
            spawn_hook_pipeline(ctx, steps)?;
        }
        return Ok(());
    }

    run_filtered_hook(
        ctx,
        user_config,
        project_config,
        hook_type,
        extra_vars,
        name_filters,
        HookFailureStrategy::Warn,
    )
}

/// Build best-effort directional vars for manual `wt hook` invocation.
///
/// When hooks run during real operations (switch, merge, remove), each call site
/// builds precise extra_vars from the actual source/destination context. When
/// invoked manually via `wt hook <type>`, we only have the current worktree —
/// so we provide reasonable defaults: the current branch as both base and target,
/// and the current worktree path for directional path vars.
///
/// This is the single source of truth for manual hook context — both `run_hook`
/// (execution + dry-run) and `expand_command_template` (hook show --expanded)
/// use this function.
fn build_manual_hook_extra_vars<'a>(
    ctx: &'a CommandContext,
    hook_type: HookType,
    custom_vars: &'a [(&'a str, &'a str)],
    default_branch: Option<&'a str>,
    worktree_path_str: &'a str,
) -> Vec<(&'a str, &'a str)> {
    let branch = ctx.branch_or_head();
    let mut vars: Vec<(&str, &str)> = match hook_type {
        // Merge/commit hooks: target = merge target (default branch for commit, current for merge)
        HookType::PreCommit | HookType::PostCommit => {
            default_branch.into_iter().map(|t| ("target", t)).collect()
        }
        HookType::PreMerge | HookType::PostMerge => {
            vec![
                ("target", branch),
                ("target_worktree_path", worktree_path_str),
            ]
        }
        // Switch hooks: base = current (we're "switching from" here)
        HookType::PreSwitch | HookType::PreStart | HookType::PostStart | HookType::PostSwitch => {
            vec![
                ("base", branch),
                ("base_worktree_path", worktree_path_str),
                ("target", branch),
                ("target_worktree_path", worktree_path_str),
            ]
        }
        // Remove hooks: target = where user ends up (current worktree is the best guess)
        HookType::PreRemove | HookType::PostRemove => {
            vec![
                ("target", branch),
                ("target_worktree_path", worktree_path_str),
            ]
        }
    };
    vars.extend(custom_vars.iter().copied());
    vars
}

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
    dry_run: bool,
    name_filters: &[String],
    custom_vars: &[(String, String)],
) -> anyhow::Result<()> {
    // Derive context from current environment (branch-optional for CI compatibility)
    let env = CommandEnv::for_action_branchless()?;
    let repo = &env.repo;
    let ctx = env.context(yes);

    // Load project config (optional - user hooks can run without project config)
    let project_config = repo.load_project_config()?;

    if !dry_run {
        // "Approve at the Gate": approve project hooks upfront
        // Pass name_filters to only approve the targeted hooks, not all hooks of this type
        let approved = approve_hooks_filtered(&ctx, &[hook_type], name_filters)?;
        // If declined, return early - the whole point of `wt hook` is to run hooks
        if !approved {
            eprintln!("{}", worktrunk::styling::info_message("Commands declined"));
            return Ok(());
        }
    }

    // Build extra vars from command-line --var flags
    let custom_vars_refs: Vec<(&str, &str)> = custom_vars
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    // Get effective user hooks (global + per-project merged)
    let user_hooks = ctx.config.hooks(ctx.project_id().as_deref());
    let (user_config, proj_config) = (
        user_hooks.get(hook_type),
        project_config.as_ref().and_then(|c| c.hooks.get(hook_type)),
    );
    // No hooks configured: warn and exit successfully. Running hooks that
    // don't exist is a no-op, so scripts can invoke `wt hook <type>`
    // unconditionally without special-casing empty configuration.
    if user_config.is_none() && proj_config.is_none() {
        eprintln!(
            "{}",
            warning_message(format!("No {hook_type} hooks configured"))
        );
        return Ok(());
    }

    // Build extra vars per hook type (shared by dry-run and execution paths)
    let default_branch = repo.default_branch();
    let worktree_path_str = worktrunk::path::to_posix_path(&ctx.worktree_path.to_string_lossy());
    let extra_vars = build_manual_hook_extra_vars(
        &ctx,
        hook_type,
        &custom_vars_refs,
        default_branch.as_deref(),
        &worktree_path_str,
    );

    if dry_run {
        let steps = prepare_sourced_steps(
            &ctx,
            HookCommandSpec {
                user_config,
                project_config: proj_config,
                hook_type,
                extra_vars: &extra_vars,
                name_filters,
                display_path: None,
            },
        )?;
        check_name_filter_matched(
            name_filters,
            count_sourced_commands(&steps),
            user_config,
            proj_config,
        )?;

        for sourced in steps {
            for cmd in sourced.step.into_commands() {
                let summary = command_summary_name(cmd.name.as_deref(), sourced.source);
                let label = if cmd.name.is_some() {
                    cformat!("{hook_type} <bold>{summary}</> would run:")
                } else {
                    cformat!("{hook_type} <bold>{summary}</> hook would run:")
                };
                eprintln!(
                    "{}",
                    info_message(cformat!(
                        "{label}\n{}",
                        format_bash_with_gutter(&cmd.expanded)
                    ))
                );
            }
        }
        return Ok(());
    }

    // Execute the hook based on type
    // pre-* hooks are blocking (fail-fast), post-* hooks run in background
    match hook_type {
        HookType::PreSwitch
        | HookType::PreStart
        | HookType::PreRemove
        | HookType::PreCommit
        | HookType::PreMerge => run_filtered_hook(
            &ctx,
            user_config,
            proj_config,
            hook_type,
            &extra_vars,
            name_filters,
            HookFailureStrategy::FailFast,
        ),
        HookType::PostStart
        | HookType::PostSwitch
        | HookType::PostCommit
        | HookType::PostMerge
        | HookType::PostRemove => run_post_hook(
            &ctx,
            foreground,
            user_config,
            proj_config,
            hook_type,
            &extra_vars,
            name_filters,
        ),
    }
}

/// Handle `wt hook approvals add` command - approve all commands in the project
pub fn add_approvals(show_all: bool) -> anyhow::Result<()> {
    use super::command_approval::approve_command_batch;

    let repo = Repository::current()?;
    let project_id = repo.project_identifier()?;
    let approvals = Approvals::load().context("Failed to load approvals")?;

    // Load project config (error if missing - this command requires it)
    let config_path = repo
        .project_config_path()?
        .context("Cannot determine project config location — no worktree found")?;
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
            .filter(|cmd| !approvals.is_command_approved(&project_id, &cmd.command.template))
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
    let approved =
        approve_command_batch(&commands_to_approve, &project_id, &approvals, false, true)?;

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
    let mut approvals = Approvals::load().context("Failed to load approvals")?;

    if global {
        // Count projects with approvals before clearing
        let project_count = approvals
            .projects()
            .filter(|(_, cmds)| !cmds.is_empty())
            .count();

        if project_count == 0 {
            eprintln!("{}", info_message("No approvals to clear"));
            return Ok(());
        }

        approvals
            .clear_all(None)
            .context("Failed to clear approvals")?;

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

        // Count approvals before clearing
        let approval_count = approvals
            .projects()
            .find(|(id, _)| *id == project_id)
            .map(|(_, cmds)| cmds.len())
            .unwrap_or(0);

        if approval_count == 0 {
            eprintln!("{}", info_message("No approvals to clear for this project"));
            return Ok(());
        }

        approvals
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

    let repo = Repository::current().context("Failed to show hooks")?;
    let config = UserConfig::load().context("Failed to load user config")?;
    let approvals = Approvals::load().context("Failed to load approvals")?;
    let project_config = repo.load_project_config()?;
    let project_id = repo.project_identifier().ok();

    // Parse hook type filter if provided
    let filter: Option<HookType> = hook_type_filter.map(|s| match s {
        "pre-switch" => HookType::PreSwitch,
        "pre-start" | "post-create" => HookType::PreStart,
        "post-start" => HookType::PostStart,
        "post-switch" => HookType::PostSwitch,
        "pre-commit" => HookType::PreCommit,
        "post-commit" => HookType::PostCommit,
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
        &approvals,
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
    let config_path = worktrunk::config::config_path();

    writeln!(
        out,
        "{}",
        format_heading(
            "USER HOOKS",
            Some(
                &config_path
                    .as_ref()
                    .map(|p| format!("@ {}", format_path_for_display(p)))
                    .unwrap_or_else(|| "(not found)".to_string())
            )
        )
    )?;

    // Collect all user hooks (global hooks from the user config file)
    // Note: uses overrides.hooks for display, not the merged hooks() accessor.
    // get() handles the post-create → pre-start deprecation merge.
    let user_hooks = &config.hooks;
    let hooks: Vec<_> = HookType::iter()
        .filter_map(|ht| user_hooks.get(ht).map(|cfg| (ht, cfg)))
        .collect();

    let mut has_any = false;
    for (hook_type, cfg) in &hooks {
        // Apply filter if specified
        if let Some(f) = filter
            && f != *hook_type
        {
            continue;
        }

        has_any = true;
        render_hook_commands(out, *hook_type, cfg, None, ctx)?;
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
    approvals: &Approvals,
    project_id: Option<&str>,
    filter: Option<HookType>,
    ctx: Option<&CommandContext>,
) -> anyhow::Result<()> {
    let config_path = repo
        .project_config_path()?
        .context("Cannot determine project config location — no worktree found")?;

    writeln!(
        out,
        "{}",
        format_heading(
            "PROJECT HOOKS",
            Some(&format!("@ {}", format_path_for_display(&config_path)))
        )
    )?;

    let Some(config) = project_config else {
        writeln!(out, "{}", hint_message("(not found)"))?;
        return Ok(());
    };

    // Collect all project hooks (get() handles post-create → pre-start merge)
    let hooks: Vec<_> = HookType::iter()
        .filter_map(|ht| config.hooks.get(ht).map(|cfg| (ht, cfg)))
        .collect();

    let mut has_any = false;
    for (hook_type, cfg) in &hooks {
        // Apply filter if specified
        if let Some(f) = filter
            && f != *hook_type
        {
            continue;
        }

        has_any = true;
        render_hook_commands(out, *hook_type, cfg, Some((approvals, project_id)), ctx)?;
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
    // For project hooks: (approvals, project_id) to check approval status
    approval_context: Option<(&Approvals, Option<&str>)>,
    ctx: Option<&CommandContext>,
) -> anyhow::Result<()> {
    let commands: Vec<_> = config.commands().collect();
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
        let needs_approval = if let Some((approvals, Some(project_id))) = approval_context {
            !approvals.is_command_approved(project_id, &cmd.template)
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
            expand_command_template(&cmd.template, command_ctx, hook_type, cmd.name.as_deref())?
        } else {
            cmd.template.clone()
        };

        writeln!(out, "{}", format_bash_with_gutter(&command_text))?;
    }

    Ok(())
}

/// Expand a command template with context variables
fn expand_command_template(
    template: &str,
    ctx: &CommandContext,
    hook_type: HookType,
    hook_name: Option<&str>,
) -> anyhow::Result<String> {
    let default_branch = ctx.repo.default_branch();
    let worktree_path_str = worktrunk::path::to_posix_path(&ctx.worktree_path.to_string_lossy());
    let extra_vars = build_manual_hook_extra_vars(
        ctx,
        hook_type,
        &[],
        default_branch.as_deref(),
        &worktree_path_str,
    );
    let mut template_ctx = build_hook_context(ctx, &extra_vars)?;
    template_ctx.insert("hook_type".into(), hook_type.to_string());
    if let Some(name) = hook_name {
        template_ctx.insert("hook_name".into(), name.into());
    }
    let vars: HashMap<&str, &str> = template_ctx
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    // Use the standard template expansion (shell-escaped)
    // On any error, show both the template and error message
    Ok(
        worktrunk::config::expand_template(template, &vars, true, ctx.repo, "hook preview")
            .unwrap_or_else(|err| format!("# {}\n{}", err.message, template)),
    )
}
