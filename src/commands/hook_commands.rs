//! Hook commands for `wt hook` subcommand.
//!
//! This module contains:
//! - `run_hook` - Execute a specific hook type
//! - `handle_hook_show` - Display configured hooks

use std::collections::HashMap;
use std::fmt::Write as _;

use anyhow::Context;
use color_print::cformat;
use strum::IntoEnumIterator;
use worktrunk::HookType;
use worktrunk::config::{
    ALIAS_ARGS_KEY, Approvals, CommandConfig, ProjectConfig, UserConfig, referenced_vars_for_config,
};
use worktrunk::git::Repository;
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{
    INFO_SYMBOL, PROMPT_SYMBOL, eprintln, format_bash_with_gutter, format_heading, hint_message,
    info_message, warning_message,
};

use super::command_approval::approve_hooks_filtered;
use super::command_executor::build_hook_context;

use super::command_executor::CommandContext;
use super::command_executor::FailureStrategy;
use super::context::CommandEnv;
use super::hooks::{
    HookAnnouncer, HookCommandSpec, lookup_hook_configs, prepare_and_check, run_hooks_foreground,
};
use super::template_vars::TemplateVars;

fn run_filtered_hook(
    ctx: &CommandContext,
    user_config: Option<&CommandConfig>,
    project_config: Option<&CommandConfig>,
    hook_type: HookType,
    extra_vars: &[(&str, &str)],
    name_filters: &[String],
    failure_strategy: FailureStrategy,
) -> anyhow::Result<()> {
    run_hooks_foreground(
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
    // --foreground is for debugging; default is background.
    if foreground.unwrap_or(false) {
        return run_filtered_hook(
            ctx,
            user_config,
            project_config,
            hook_type,
            extra_vars,
            name_filters,
            FailureStrategy::Warn,
        );
    }

    // Filter path merges user + project matches into one pipeline (the user
    // cherry-picked specific names across sources). The default path keeps
    // sources independent so a user hook failure doesn't abort project hooks.
    let mut announcer = HookAnnouncer::new(ctx.repo, ctx.config, false);
    if name_filters.is_empty() {
        announcer.register(ctx, hook_type, extra_vars, None)?;
    } else {
        let flat = prepare_and_check(
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
        if flat.is_empty() {
            return Ok(());
        }
        announcer.extend(std::iter::once((*ctx, flat)));
    }
    announcer.flush()
}

/// Build best-effort directional vars for manual `wt hook` invocation.
///
/// When hooks run during real operations (switch, merge, remove), each call site
/// builds precise vars from the actual source/destination context. When invoked
/// manually via `wt hook <type>`, we only have the current worktree — so we
/// provide reasonable defaults: the current branch as both base and target, and
/// the current worktree path for directional path vars.
///
/// This is the single source of truth for manual hook context — both `run_hook`
/// (execution + dry-run) and `expand_command_template` (hook show --expanded)
/// use this function. Returns a `TemplateVars` so callers can extend with
/// additional bindings (e.g. CLI shorthand) before materializing.
fn build_manual_hook_template_vars(
    ctx: &CommandContext,
    hook_type: HookType,
    default_branch: Option<&str>,
) -> TemplateVars {
    let branch = ctx.branch_or_head();
    let worktree_path = ctx.worktree_path;
    match hook_type {
        // Merge/commit hooks: target = merge target (default branch for commit, current for merge)
        HookType::PreCommit | HookType::PostCommit => {
            default_branch.map_or_else(TemplateVars::new, |t| TemplateVars::new().with_target(t))
        }
        HookType::PreMerge | HookType::PostMerge => TemplateVars::new()
            .with_target(branch)
            .with_target_worktree_path(worktree_path),
        // Switch hooks: base = current (we're "switching from" here)
        HookType::PreSwitch | HookType::PreStart | HookType::PostStart | HookType::PostSwitch => {
            TemplateVars::new()
                .with_base(branch, worktree_path)
                .with_target(branch)
                .with_target_worktree_path(worktree_path)
        }
        // Remove hooks: target = where user ends up (current worktree is the best guess)
        HookType::PreRemove | HookType::PostRemove => TemplateVars::new()
            .with_target(branch)
            .with_target_worktree_path(worktree_path),
    }
}

/// Parse a raw `KEY=VALUE` shorthand token into a canonicalized
/// `(canonical_key, original_key, value)` triple.
///
/// Canonicalization replaces `-` with `_` in the key to match the template
/// naming convention (minijinja parses `{{ my-var }}` as subtraction), the
/// same rule `parse_key_val` applies to `--var`. The original key is preserved
/// for reconstructing `--KEY=VALUE` when forwarding to `{{ args }}`.
fn parse_shorthand_token(raw: &str) -> anyhow::Result<(String, String, String)> {
    let (key, value) = raw
        .split_once('=')
        .ok_or_else(|| anyhow::anyhow!("invalid shorthand (missing `=`): {raw}"))?;
    if key.is_empty() {
        anyhow::bail!("invalid shorthand (empty key): {raw}");
    }
    Ok((key.replace('-', "_"), key.to_string(), value.to_string()))
}

/// Union of top-level template variable names referenced across every command
/// in both configs for this hook type. Matches alias pipeline semantics:
/// referenced in any step is a binding candidate for the whole invocation.
fn referenced_vars_union(
    user_config: Option<&CommandConfig>,
    project_config: Option<&CommandConfig>,
) -> anyhow::Result<std::collections::BTreeSet<String>> {
    let mut out = std::collections::BTreeSet::new();
    if let Some(cfg) = user_config {
        out.extend(referenced_vars_for_config(cfg)?);
    }
    if let Some(cfg) = project_config {
        out.extend(referenced_vars_for_config(cfg)?);
    }
    Ok(out)
}

/// CLI-origin arguments to a manual `wt hook <type>` invocation. Bundled so
/// the call sites in `main.rs` don't balloon past clippy's
/// `too_many_arguments` threshold as the shorthand/forwarding surface grows.
pub struct HookCliArgs<'a> {
    /// Positional name filters: `wt hook pre-merge test build` → `["test", "build"]`.
    pub name_filters: &'a [String],
    /// Explicit `--var KEY=VALUE` bindings (deprecated force-bind).
    pub explicit_vars: &'a [(String, String)],
    /// Raw `KEY=VALUE` tokens from the `--KEY=VALUE` shorthand. Smart-routed:
    /// bind if any hook template references KEY, else forward to `{{ args }}`.
    pub shorthand_vars: &'a [String],
    /// Tokens after `--` that forward to `{{ args }}` verbatim.
    pub forwarded_args: &'a [String],
}

/// Handle `wt hook` command
///
/// When explicitly invoking hooks, ALL hooks run (both user and project).
/// There's no skip flag - if you explicitly run hooks, all configured hooks run.
///
/// Works in detached HEAD state - `{{ branch }}` template variable will be "HEAD".
///
/// Template variables come from three sources in [`HookCliArgs`], routed per
/// alias semantics:
/// - `shorthand_vars` (`--KEY=VALUE`): binds `{{ KEY }}` if any hook template
///   references it; otherwise forwards `--KEY=VALUE` into `{{ args }}`.
/// - `forwarded_args` (tokens after `--`): forwards into `{{ args }}` verbatim.
/// - `explicit_vars` (`--var KEY=VALUE`): deprecated force-bind. Always binds,
///   regardless of whether any template references the key.
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
    cli: HookCliArgs<'_>,
) -> anyhow::Result<()> {
    let HookCliArgs {
        name_filters,
        explicit_vars,
        shorthand_vars,
        forwarded_args,
    } = cli;
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

    // Get effective user hooks (global + per-project merged)
    let user_hooks = ctx.config.hooks(ctx.project_id().as_deref());
    let (user_config, proj_config) =
        lookup_hook_configs(&user_hooks, project_config.as_ref(), hook_type);
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

    // Smart-route shorthand: bind when the template references the key,
    // forward otherwise. Mirrors `AliasOptions::parse` for the alias path.
    let referenced = referenced_vars_union(user_config, proj_config)?;
    let mut bindings: Vec<(String, String)> = Vec::new();
    let mut args: Vec<String> = Vec::new();
    for raw in shorthand_vars {
        let (canon_key, orig_key, value) = parse_shorthand_token(raw)?;
        if referenced.contains(&canon_key) {
            bindings.push((canon_key, value));
        } else {
            args.push(format!("--{orig_key}={value}"));
        }
    }
    args.extend(forwarded_args.iter().cloned());

    // Explicit `--var KEY=VALUE` is deprecated — prefer `--KEY=VALUE`. It
    // still force-binds (useful when a template references the key only
    // conditionally, e.g. `{% if override %}`), so keep the binding.
    if !explicit_vars.is_empty() {
        eprintln!(
            "{}",
            warning_message(
                "--var is deprecated; use --KEY=VALUE shorthand (binds automatically when any hook template references KEY)",
            )
        );
        bindings.extend(explicit_vars.iter().cloned());
    }

    let custom_vars_refs: Vec<(&str, &str)> = bindings
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    // Build extra vars per hook type (shared by dry-run and execution paths)
    let default_branch = repo.default_branch();
    // Splice `args` into the template context as a JSON-encoded sequence.
    // `expand_template` rehydrates it as `ShellArgs` so bare `{{ args }}`
    // renders space-joined with per-element shell escaping. Mirrors
    // `run_alias` at `src/commands/alias.rs`.
    let args_json =
        serde_json::to_string(&args).expect("Vec<String> serialization should never fail");
    let template_vars = build_manual_hook_template_vars(&ctx, hook_type, default_branch.as_deref());
    let mut extra_vars = template_vars.as_extra_vars();
    extra_vars.extend(custom_vars_refs.iter().copied());
    // Forward positional CLI args as `{{ args }}` (empty sequence when
    // nothing was forwarded). `expand_template` rehydrates this JSON into a
    // `ShellArgs` sequence that renders space-joined, per-element escaped.
    extra_vars.push((ALIAS_ARGS_KEY, &args_json));

    if dry_run {
        let steps = prepare_and_check(
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

        for sourced in steps {
            for cmd in sourced.step.into_commands() {
                let label = if cmd.name.is_some() {
                    cformat!("{hook_type} <bold>{}</> would run:", cmd.label)
                } else {
                    cformat!("{hook_type} <bold>{}</> hook would run:", cmd.label)
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

    // pre-* hooks block (fail-fast); post-* hooks default to background.
    if hook_type.is_pre() {
        run_filtered_hook(
            &ctx,
            user_config,
            proj_config,
            hook_type,
            &extra_vars,
            name_filters,
            FailureStrategy::default_for(hook_type),
        )
    } else {
        run_post_hook(
            &ctx,
            foreground,
            user_config,
            proj_config,
            hook_type,
            &extra_vars,
            name_filters,
        )
    }
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

    // Display through pager; fall back to direct stdout if pager unavailable
    if show_help_in_pager(&output, true).is_err() {
        worktrunk::styling::println!("{}", output);
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
    let template_vars = build_manual_hook_template_vars(ctx, hook_type, default_branch.as_deref());
    let extra_vars = template_vars.as_extra_vars();
    let mut template_ctx = build_hook_context(ctx, &extra_vars)?;
    template_ctx.insert("hook_type".into(), hook_type.to_string());
    if let Some(name) = hook_name {
        template_ctx.insert("hook_name".into(), name.into());
    }
    // Preview has no CLI args to forward. Inject an empty JSON sequence
    // so templates that reference `{{ args }}` render cleanly rather than
    // erroring with "undefined value" at the preview site.
    template_ctx.insert(ALIAS_ARGS_KEY.into(), "[]".into());
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
