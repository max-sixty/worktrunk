//! `wt config alias` subcommands.
//!
//! Introspection and preview for aliases configured in user config
//! (`~/.config/worktrunk/config.toml`) and project config (`.config/wt.toml`).
//! `show` prints the template text, source-labeled, with one gutter block per
//! alias entry and `# <name>` comment lines above named pipeline steps.
//! `dry-run` parses a per-invocation argument vector with the same parser
//! `wt <alias>` uses, then expands templates using the same context as
//! execution — so previews match what the real run will do. The two share a
//! layout; only the header verb differs (`:` vs ` would run:`).
//!
//! ## Why `dry-run` lives here rather than on the alias dispatch
//!
//! Previous versions exposed dry-run via `wt <alias> --dry-run`. That routed
//! through `AliasOptions::parse` and required every caller to handle the
//! "preview vs run" branch. Lifting it into a dedicated subcommand keeps the
//! alias-dispatch path single-purpose (always runs) and gives preview a
//! natural home alongside `show`.

use std::collections::{BTreeSet, HashMap};

use anyhow::Context;
use color_print::cformat;
use worktrunk::config::{
    ALIAS_ARGS_KEY, CommandConfig, ProjectConfig, UserConfig, append_aliases,
    referenced_vars_for_config, template_references_var, validate_template_syntax,
};
use worktrunk::git::Repository;
use worktrunk::styling::{format_bash_with_gutter, info_message, println};

use crate::commands::alias::{AliasOptions, AliasSource, TOP_LEVEL_BUILTINS};
use crate::commands::command_executor::{
    CommandContext, build_hook_context, expand_shell_template,
};
use crate::commands::did_you_mean;

/// Show the configured template(s) for an alias, tagged by source.
///
/// When the same name is defined in both user and project config, both
/// entries are printed (user first, matching runtime execution order).
pub fn handle_alias_show(name: String) -> anyhow::Result<()> {
    let repo = Repository::current()?;
    let user_config = UserConfig::load()?;
    let project_config = ProjectConfig::load(&repo, true)?;
    let entries = entries_for_name(&repo, &user_config, project_config.as_ref(), &name);

    if entries.is_empty() {
        return Err(unknown_alias_error(
            &repo,
            &user_config,
            project_config.as_ref(),
            &name,
        ));
    }

    warn_if_shadowed(&name);

    for (i, (cfg, source)) in entries.iter().enumerate() {
        if i > 0 {
            println!();
        }
        let bodies: Vec<String> = cfg.commands().map(|c| c.template.clone()).collect();
        println!("{}", format_entry(&name, cfg, *source, &bodies, None));
    }
    Ok(())
}

/// Emit a warning if `name` is a top-level built-in subcommand. Aliases with
/// these names are unreachable via `wt <name>` — clap matches the built-in
/// first. Reported in `show`/`dry-run` so the user finds out at the discovery
/// surface rather than silently during an invocation that never reaches the
/// alias.
fn warn_if_shadowed(name: &str) {
    if TOP_LEVEL_BUILTINS.contains(&name) {
        worktrunk::styling::eprintln!(
            "{}",
            worktrunk::styling::warning_message(cformat!(
                "Alias <bold>{name}</> is shadowed by built-in <bold>wt {name}</>"
            ))
        );
    }
}

/// Preview an alias invocation: parse the args, build the template context,
/// and print the rendered command(s) without executing.
///
/// Lazy semantics are preserved: templates referencing `vars.*` are shown
/// raw (after syntax validation) because those values resolve from git
/// config at execution time, potentially written by earlier pipeline steps.
/// Other templates expand against the current context.
pub fn handle_alias_dry_run(name: String, args: Vec<String>) -> anyhow::Result<()> {
    let repo = Repository::current()?;
    let user_config = UserConfig::load()?;
    let project_config = ProjectConfig::load(&repo, true)?;
    let entries = entries_for_name(&repo, &user_config, project_config.as_ref(), &name);

    if entries.is_empty() {
        return Err(unknown_alias_error(
            &repo,
            &user_config,
            project_config.as_ref(),
            &name,
        ));
    }

    // Reuse the real parser so previews stay aligned with runtime parsing —
    // including `--KEY=VALUE` routing and positional forwarding. When both
    // user and project configs define the alias, union the referenced vars
    // so a flag binds if any entry's template references it.
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    for (cfg, _) in &entries {
        referenced.extend(referenced_vars_for_config(cfg)?);
    }
    let mut parse_args = Vec::with_capacity(1 + args.len());
    parse_args.push(name.clone());
    parse_args.extend(args);
    let (opts, warnings) = AliasOptions::parse(parse_args, &referenced)?;
    warn_if_shadowed(&name);
    for warning in &warnings {
        worktrunk::styling::eprintln!("{}", worktrunk::styling::warning_message(warning));
    }

    let wt = repo.current_worktree();
    let wt_path = wt.root().context("Failed to get worktree root")?;
    let branch = wt.branch().ok().flatten();
    let ctx = CommandContext::new(&repo, &user_config, branch.as_deref(), &wt_path, false);
    let extra_refs: Vec<(&str, &str)> = opts
        .vars
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let mut context_map = build_hook_context(&ctx, &extra_refs)?;
    context_map.insert(
        ALIAS_ARGS_KEY.to_string(),
        serde_json::to_string(&opts.positional_args)
            .expect("Vec<String> serialization should never fail"),
    );

    let routing = format_routing_summary(&opts);

    for (i, (cfg, source)) in entries.iter().enumerate() {
        if i > 0 {
            println!();
        }
        let bodies: Vec<String> = cfg
            .commands()
            .map(|c| render_preview(&c.template, &context_map, &repo, &name))
            .collect::<anyhow::Result<_>>()?;
        println!(
            "{}",
            format_entry_with_routing(
                &name,
                cfg,
                *source,
                &bodies,
                Some("would run"),
                routing.as_deref()
            )
        );
    }
    Ok(())
}

/// Summarize how each CLI token routed, as `# ` comment lines suitable for the
/// top of a dry-run body. Returns `None` when nothing would have been bound or
/// forwarded — the common no-args case stays clean.
fn format_routing_summary(opts: &AliasOptions) -> Option<String> {
    if opts.vars.is_empty() && opts.positional_args.is_empty() {
        return None;
    }
    let mut lines = String::new();
    if !opts.vars.is_empty() {
        let bound = opts
            .vars
            .iter()
            .map(|(k, v)| format!("{k}={}", shell_escape::unix::escape(v.into())))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push_str(&format!("# bound: {bound}\n"));
    }
    if !opts.positional_args.is_empty() {
        let args = opts
            .positional_args
            .iter()
            .map(|a| shell_escape::unix::escape(a.into()).into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        lines.push_str(&format!("# args: {args}\n"));
    }
    Some(lines)
}

/// Render a single command template for preview. Mirrors execution-time lazy
/// semantics — see the module-level docstring.
fn render_preview(
    template: &str,
    context: &HashMap<String, String>,
    repo: &Repository,
    alias_name: &str,
) -> anyhow::Result<String> {
    if template_references_var(template, "vars") {
        validate_template_syntax(template, alias_name)
            .map_err(|e| anyhow::anyhow!("syntax error in alias {alias_name}: {e}"))?;
        Ok(template.to_string())
    } else {
        Ok(expand_shell_template(template, context, repo, alias_name)?)
    }
}

/// Resolve `name` against user + project config, preserving runtime execution
/// order (user first, then project).
fn entries_for_name(
    repo: &Repository,
    user_config: &UserConfig,
    project_config: Option<&ProjectConfig>,
    name: &str,
) -> Vec<(CommandConfig, AliasSource)> {
    let project_id = repo.project_identifier().ok();
    let mut entries = Vec::new();
    if let Some(cfg) = user_config.aliases(project_id.as_deref()).get(name) {
        entries.push((cfg.clone(), AliasSource::User));
    }
    if let Some(pc) = project_config
        && let Some(cfg) = pc.aliases.get(name)
    {
        entries.push((cfg.clone(), AliasSource::Project));
    }
    entries
}

/// Build an anyhow error for an unknown alias, with a clap-style "did you mean"
/// tail pulled from the merged alias name set.
///
/// Uses `anyhow::Error::context` so the top-level handler formats the first
/// line as a header and the suggestion list in the error gutter.
fn unknown_alias_error(
    repo: &Repository,
    user_config: &UserConfig,
    project_config: Option<&ProjectConfig>,
    name: &str,
) -> anyhow::Error {
    let project_id = repo.project_identifier().ok();
    let mut merged = user_config.aliases(project_id.as_deref());
    if let Some(pc) = project_config {
        append_aliases(&mut merged, &pc.aliases);
    }
    let suggestions = did_you_mean(name, merged.into_keys());
    let header = format!("unknown alias '{name}'");
    if suggestions.is_empty() {
        anyhow::anyhow!(header)
    } else {
        let mut detail = String::from("a similar alias exists:");
        for s in &suggestions {
            detail.push_str(&format!("\n  {s}"));
        }
        anyhow::Error::msg(detail).context(header)
    }
}

/// Format one alias entry: `○ Alias <name> (<source>)[ <verb>]:` header
/// followed by a single gutter block of the command bodies. Each named step
/// gets a `# <name>` comment line above its body; anonymous steps render the
/// body alone. Joining into one block matches the old `--dry-run` layout and
/// keeps `show`/`dry-run` visually aligned — the only difference is the verb.
fn format_entry(
    name: &str,
    cfg: &CommandConfig,
    source: AliasSource,
    bodies: &[String],
    verb: Option<&str>,
) -> String {
    format_entry_with_routing(name, cfg, source, bodies, verb, None)
}

/// As `format_entry`, with optional routing comment lines prepended to the
/// body. Used by `dry-run` to surface `--KEY` bindings and forwarded args.
fn format_entry_with_routing(
    name: &str,
    cfg: &CommandConfig,
    source: AliasSource,
    bodies: &[String],
    verb: Option<&str>,
    routing: Option<&str>,
) -> String {
    let label = source.label();
    let suffix = match verb {
        Some(v) => format!(" {v}:"),
        None => ":".to_string(),
    };
    let mut body = String::new();
    if let Some(routing) = routing {
        body.push_str(routing);
    }
    for (cmd, rendered) in cfg.commands().zip(bodies) {
        if !body.is_empty() && !body.ends_with('\n') {
            body.push('\n');
        }
        if let Some(step_name) = &cmd.name {
            body.push_str(&format!("# {step_name}\n"));
        }
        body.push_str(rendered);
    }
    info_message(cformat!(
        "Alias <bold>{name}</> ({label}){suffix}\n{}",
        format_bash_with_gutter(&body)
    ))
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ansi_str::AnsiStr;

    fn cfg_from_toml(toml_str: &str) -> CommandConfig {
        #[derive(serde::Deserialize)]
        struct Wrap {
            cmd: CommandConfig,
        }
        toml::from_str::<Wrap>(toml_str).unwrap().cmd
    }

    #[test]
    fn test_format_entry_show_single() {
        let cfg = cfg_from_toml(r#"cmd = "echo {{ branch }}""#);
        let bodies: Vec<String> = cfg.commands().map(|c| c.template.clone()).collect();
        let out = format_entry("greet", &cfg, AliasSource::User, &bodies, None);
        insta::assert_snapshot!(out.ansi_strip());
    }

    #[test]
    fn test_format_entry_show_pipeline() {
        let cfg = cfg_from_toml(
            r#"
cmd = [
    { install = "npm install" },
    { build = "npm run build", lint = "npm run lint" },
]
"#,
        );
        let bodies: Vec<String> = cfg.commands().map(|c| c.template.clone()).collect();
        let out = format_entry("deploy", &cfg, AliasSource::Project, &bodies, None);
        insta::assert_snapshot!(out.ansi_strip());
    }

    #[test]
    fn test_format_entry_dry_run_pipeline() {
        // The verb only changes the header suffix — body layout is identical.
        let cfg = cfg_from_toml(
            r#"
cmd = [
    { install = "npm install" },
    { build = "npm run build", lint = "npm run lint" },
]
"#,
        );
        let bodies: Vec<String> = cfg.commands().map(|c| c.template.clone()).collect();
        let out = format_entry(
            "deploy",
            &cfg,
            AliasSource::Project,
            &bodies,
            Some("would run"),
        );
        insta::assert_snapshot!(out.ansi_strip());
    }
}
