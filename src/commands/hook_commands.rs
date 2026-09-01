//! Hook commands for `wt hook` subcommand.
//!
//! This module contains:
//! - `run_hook` - Execute a specific hook type
//! - `handle_hook_show` - Display configured hooks

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
    info_message, println, warning_message,
};

use crate::output::print_json;

use super::command_approval::approve_hooks_filtered;
use super::command_executor::{
    CommandContext, FailureStrategy, PreparedStep, prepare_steps, render_template_preview,
};
use super::context::CommandEnv;
use super::hook_filter::HookSource;
use super::hooks::{HookAnnouncer, prepare_and_check, run_hooks_foreground};
use super::project_config::command_label;
use super::template_vars::TemplateVars;

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
        return run_hooks_foreground(
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
    let mut announcer = HookAnnouncer::new(ctx.repo, false);
    if name_filters.is_empty() {
        announcer.register(ctx, hook_type, extra_vars, None)?;
    } else {
        let flat = prepare_and_check(
            ctx,
            user_config,
            project_config,
            hook_type,
            extra_vars,
            name_filters,
        )?;
        // `flat` is non-empty: a filter that matches nothing errors above.
        announcer.add_groups(ctx, hook_type, None, vec![flat]);
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
/// (execution + dry-run) and [`hook_command_rows`] (`hook show --expanded`) use
/// this function. Returns a `TemplateVars` so callers can extend with
/// additional bindings (e.g. CLI shorthand) before materializing.
fn build_manual_hook_template_vars(ctx: &CommandContext, hook_type: HookType) -> TemplateVars {
    let branch = ctx.branch_or_head();
    let worktree_path = ctx.worktree_path;
    match hook_type {
        // Merge/commit hooks: target = merge target (default branch for commit,
        // current for merge). Only this arm needs the default branch, and
        // resolving it can cost a `git ls-remote` on a fresh clone — so it is
        // fetched here rather than up front.
        HookType::PreCommit | HookType::PostCommit => ctx
            .repo
            .default_branch()
            .map_or_else(TemplateVars::new, |t| TemplateVars::new().with_target(&t)),
        HookType::PreMerge | HookType::PostMerge => TemplateVars::new()
            .with_target(branch)
            .with_target_worktree_path(worktree_path),
        // Switch hooks: base = current (we're "switching from" here)
        HookType::PreSwitch | HookType::PreCreate | HookType::PostCreate | HookType::PostSwitch => {
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
    hook_type: HookType,
) -> anyhow::Result<std::collections::BTreeSet<String>> {
    let mut out = std::collections::BTreeSet::new();
    if let Some(cfg) = user_config {
        out.extend(referenced_vars_for_config(
            cfg,
            &format!("user {hook_type} hook"),
        )?);
    }
    if let Some(cfg) = project_config {
        out.extend(referenced_vars_for_config(
            cfg,
            &format!("project {hook_type} hook"),
        )?);
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
    // Derive context from the current environment; `config` isn't in hand here,
    // so let the repository supply its cached load.
    let env = CommandEnv::for_action_loading_config()?;
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
    let user_config = user_hooks.get(hook_type);
    let proj_config = project_config.as_ref().and_then(|c| c.hooks.get(hook_type));
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
    let referenced = referenced_vars_union(user_config, proj_config, hook_type)?;
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
    // Splice `args` into the template context as a JSON-encoded sequence.
    // `expand_template` rehydrates it as `ShellArgs` so bare `{{ args }}`
    // renders space-joined with per-element shell escaping. Mirrors
    // `run_alias` at `src/commands/alias.rs`.
    let args_json =
        serde_json::to_string(&args).expect("Vec<String> serialization should never fail");
    let template_vars = build_manual_hook_template_vars(&ctx, hook_type);
    let mut extra_vars = template_vars.as_extra_vars();
    extra_vars.extend(custom_vars_refs.iter().copied());
    // Forward positional CLI args as `{{ args }}` (empty sequence when
    // nothing was forwarded). `expand_template` rehydrates this JSON into a
    // `ShellArgs` sequence that renders space-joined, per-element escaped.
    extra_vars.push((ALIAS_ARGS_KEY, &args_json));

    if dry_run {
        let steps = prepare_and_check(
            &ctx,
            user_config,
            proj_config,
            hook_type,
            &extra_vars,
            name_filters,
        )?;

        for sourced in steps {
            for cmd in sourced.step.into_commands() {
                let preview =
                    render_template_preview(&cmd.template, &cmd.context, repo, &cmd.template_name)?;
                let label = if cmd.name.is_some() {
                    cformat!("{hook_type} <bold>{}</> would run:", cmd.label)
                } else {
                    cformat!("{hook_type} <bold>{}</> hook would run:", cmd.label)
                };
                // Dry-run preview is the command's answer (the hook that would
                // run), so it goes to stdout — see /writing-user-outputs.
                println!(
                    "{}",
                    info_message(cformat!("{label}\n{}", format_bash_with_gutter(&preview)))
                );
            }
        }
        return Ok(());
    }

    // pre-* hooks block (fail-fast); post-* hooks default to background.
    if hook_type.is_pre() {
        run_hooks_foreground(
            &ctx,
            user_config,
            proj_config,
            hook_type,
            &extra_vars,
            name_filters,
            FailureStrategy::FailFast,
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
pub fn handle_hook_show(
    hook_type_filter: Option<&str>,
    expanded: bool,
    format: crate::cli::SwitchFormat,
) -> anyhow::Result<()> {
    use crate::help_pager::show_help_in_pager;

    let repo = Repository::current().context("Failed to show hooks")?;
    let config: &UserConfig = repo.user_config();
    let project_config: Option<&ProjectConfig> = repo
        .project_config()
        .context("Failed to load project config")?;
    let approvals = Approvals::load().context("Failed to load approvals")?;
    let project_id = repo.project_identifier().ok();

    // Parse hook type filter if provided. clap's value parser already
    // validated the string (canonical name or deprecated `-create` alias);
    // `parse_hook_type` maps both to the canonical `HookType`.
    let filter: Option<HookType> = hook_type_filter
        .map(crate::cli::parse_hook_type)
        .transpose()?;

    // Build context for template expansion (only used if --expanded).
    // Need to keep CommandEnv alive for the lifetime of ctx. Detached HEAD is
    // fine: `{{ branch }}` expands to "HEAD" there.
    let env = if expanded {
        Some(CommandEnv::for_action_loading_config()?)
    } else {
        None
    };
    let ctx = env.as_ref().map(|e| e.context(false));

    if format == crate::cli::SwitchFormat::Json {
        return emit_hook_show_json(
            config,
            project_config,
            &approvals,
            project_id.as_deref(),
            filter,
            ctx.as_ref(),
        );
    }

    let mut output = String::new();

    // Render user hooks
    render_user_hooks(
        &mut output,
        config,
        &approvals,
        project_id.as_deref(),
        filter,
        ctx.as_ref(),
    )?;
    output.push('\n');

    // Render project hooks
    render_project_hooks(
        &mut output,
        &repo,
        project_config,
        &approvals,
        project_id.as_deref(),
        filter,
        ctx.as_ref(),
    )?;

    show_help_in_pager(&output, true);

    Ok(())
}

/// Emit configured hooks as a JSON array of structured records.
///
/// Each record carries the hook type, source (user or project), optional name,
/// raw template, project approval status, and — when `--expanded` was passed —
/// the rendered command preview. `handle_hook_show` builds `ctx` only under
/// `--expanded`, and each row carries whether it was expanded.
fn emit_hook_show_json(
    user_config: &UserConfig,
    project_config: Option<&ProjectConfig>,
    approvals: &Approvals,
    project_id: Option<&str>,
    filter: Option<HookType>,
    ctx: Option<&CommandContext>,
) -> anyhow::Result<()> {
    let mut entries: Vec<serde_json::Value> = Vec::new();

    let mut emit =
        |hook_type: HookType, source: HookSource, cfg: &CommandConfig| -> anyhow::Result<()> {
            for row in hook_command_rows(cfg, ctx, hook_type, source)? {
                let mut obj = serde_json::json!({
                    "type": hook_type.to_string(),
                    "source": source.to_string(),
                    "name": row.name,
                    "template": row.template,
                    "needs_approval": needs_approval(source, approvals, project_id, &row.template),
                });

                if let Some(expanded) = row.expanded {
                    obj["expanded"] = serde_json::Value::String(expanded);
                }

                entries.push(obj);
            }
            Ok(())
        };

    // User hooks (merge global + per-project so the listing matches what runs)
    let user_hooks = user_config.hooks(project_id);
    for hook_type in HookType::iter() {
        if let Some(f) = filter
            && f != hook_type
        {
            continue;
        }
        if let Some(cfg) = user_hooks.get(hook_type) {
            emit(hook_type, HookSource::User, cfg)?;
        }
    }

    // Project hooks
    if let Some(project) = project_config {
        for hook_type in HookType::iter() {
            if let Some(f) = filter
                && f != hook_type
            {
                continue;
            }
            if let Some(cfg) = project.hooks.get(hook_type) {
                emit(hook_type, HookSource::Project, cfg)?;
            }
        }
    }

    print_json(&entries)?;
    Ok(())
}

/// Render user hooks section
fn render_user_hooks(
    out: &mut String,
    config: &UserConfig,
    approvals: &Approvals,
    project_id: Option<&str>,
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

    // Merge global and per-project user hooks so display matches what
    // actually runs (the execution path also uses `config.hooks(project_id)`).
    let user_hooks = config.hooks(project_id);
    let hooks: Vec<_> = HookType::iter()
        .filter_map(|ht| user_hooks.get(ht).map(|cfg| (ht, cfg)))
        .collect();

    render_hook_section(
        out,
        &hooks,
        HookSource::User,
        approvals,
        project_id,
        filter,
        ctx,
    )
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

    // Collect all project hooks
    let hooks: Vec<_> = HookType::iter()
        .filter_map(|ht| config.hooks.get(ht).map(|cfg| (ht, cfg)))
        .collect();

    render_hook_section(
        out,
        &hooks,
        HookSource::Project,
        approvals,
        project_id,
        filter,
        ctx,
    )
}

/// Render a section's body: every hook that survives `filter`, or
/// `(none configured)` when that leaves the section with nothing.
///
/// The fallback keys off what was printed, not off what the config declared —
/// a hook type carrying an empty command list (`post-switch = []`) has an entry
/// but no commands to show, and a section holding only those is empty.
fn render_hook_section(
    out: &mut String,
    hooks: &[(HookType, &CommandConfig)],
    source: HookSource,
    approvals: &Approvals,
    project_id: Option<&str>,
    filter: Option<HookType>,
    ctx: Option<&CommandContext>,
) -> anyhow::Result<()> {
    let mut has_any = false;
    for (hook_type, config) in hooks {
        if let Some(f) = filter
            && f != *hook_type
        {
            continue;
        }

        has_any |=
            render_hook_commands(out, *hook_type, config, source, approvals, project_id, ctx)?;
    }

    if !has_any {
        writeln!(out, "{}", hint_message("(none configured)"))?;
    }

    Ok(())
}

/// Render commands for a single hook type, reporting whether it wrote any.
fn render_hook_commands(
    out: &mut String,
    hook_type: HookType,
    config: &CommandConfig,
    source: HookSource,
    approvals: &Approvals,
    project_id: Option<&str>,
    ctx: Option<&CommandContext>,
) -> anyhow::Result<bool> {
    let mut wrote_any = false;
    for row in hook_command_rows(config, ctx, hook_type, source)? {
        wrote_any = true;
        let label = command_label(hook_type, row.name.as_deref());

        let needs_approval = needs_approval(source, approvals, project_id, &row.template);

        // Use ❯ for needs approval, ○ for approved/user hooks
        let (emoji, suffix) = if needs_approval {
            (PROMPT_SYMBOL, cformat!(" <dim>(requires approval)</>"))
        } else {
            (INFO_SYMBOL, String::new())
        };

        writeln!(out, "{emoji} {label}{suffix}")?;
        let shown = row.expanded.as_deref().unwrap_or(&row.template);
        writeln!(out, "{}", format_bash_with_gutter(shown))?;
    }

    Ok(wrote_any)
}

/// Whether a listed command still needs the user's approval to run.
///
/// Only project commands do — user config is the user's own. A repo with no
/// project identifier has nothing to key approvals by, so nothing is approved
/// and nothing is flagged.
fn needs_approval(
    source: HookSource,
    approvals: &Approvals,
    project_id: Option<&str>,
    template: &str,
) -> bool {
    match source {
        HookSource::User => false,
        HookSource::Project => {
            project_id.is_some_and(|id| !approvals.is_command_approved(id, template))
        }
    }
}

/// One command in a `wt hook show` listing.
struct HookCommandRow {
    name: Option<String>,
    template: String,
    /// The command as it would run, under `--expanded`. `None` without it, so
    /// the listing prints the raw template and the JSON omits the field —
    /// neither has to re-derive which mode it is in.
    expanded: Option<String>,
}

/// The rows for one hook config, expanded when `ctx` is present —
/// `handle_hook_show` builds one only under `--expanded`.
///
/// Expansion runs the config through [`prepare_steps`], the same function that
/// builds what actually executes, so every context key the execution path
/// gains reaches this preview with no second edit here. Rendering then goes
/// through [`render_template_preview`], shared with `wt hook <type>
/// --dry-run`, which renders each `{{ vars.<key> }}` as itself while the rest
/// of the template expands — those values resolve from git config when the
/// step runs, possibly written by an earlier step.
///
/// A manual invocation has no source or destination worktree, so the
/// directional vars come from [`build_manual_hook_template_vars`], exactly as
/// `run_hook` builds them. `args` is left unset, and `prepare_steps` defaults it
/// to the empty sequence — a listing has no CLI args to forward, which is what
/// that default encodes.
///
/// A template that cannot expand renders as `# <error>` above its raw text
/// rather than propagating — `wt hook show` lists configuration, so one broken
/// template must not blank the rest of the listing.
fn hook_command_rows(
    config: &CommandConfig,
    ctx: Option<&CommandContext>,
    hook_type: HookType,
    source: HookSource,
) -> anyhow::Result<Vec<HookCommandRow>> {
    // The emptiness check spares a config with no commands the git
    // subprocesses `prepare_steps` spawns to build a context nothing reads.
    if let Some(ctx) = ctx
        && config.commands().next().is_some()
    {
        let template_vars = build_manual_hook_template_vars(ctx, hook_type);
        let extra_vars = template_vars.as_extra_vars();

        return Ok(prepare_steps(config, ctx, &extra_vars, hook_type, source)?
            .into_unvalidated()
            .into_iter()
            .flat_map(PreparedStep::into_commands)
            .map(|cmd| {
                let template = cmd.template;
                let display =
                    render_template_preview(&template, &cmd.context, ctx.repo, &cmd.template_name)
                        .unwrap_or_else(|err| format!("# {err}\n{template}"));
                HookCommandRow {
                    name: cmd.name,
                    template,
                    expanded: Some(display),
                }
            })
            .collect());
    }

    Ok(config
        .commands()
        .map(|cmd| HookCommandRow {
            name: cmd.name.clone(),
            template: cmd.template.clone(),
            expanded: None,
        })
        .collect())
}
