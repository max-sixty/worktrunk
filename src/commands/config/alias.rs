//! `wt config alias` subcommands.
//!
//! Introspection and preview for aliases configured in user config
//! (`~/.config/worktrunk/config.toml`) and project config (`.config/wt.toml`).
//! `show` prints the template text, source-labeled and annotated with the
//! pipeline structure the announcement uses at runtime. `dry-run` parses a
//! per-invocation argument vector with the same parser `wt <alias>` uses, then
//! expands templates using the same context as execution — so previews match
//! what the real run will do.
//!
//! ## Why `dry-run` lives here rather than on the alias dispatch
//!
//! Previous versions exposed dry-run via `wt <alias> --dry-run`. That routed
//! through `AliasOptions::parse` and required every caller to handle the
//! "preview vs run" branch. Lifting it into a dedicated subcommand keeps the
//! alias-dispatch path single-purpose (always runs) and gives preview a
//! natural home alongside `show`.

use std::collections::HashMap;

use anyhow::Context;
use color_print::cformat;
use worktrunk::config::{
    ALIAS_ARGS_KEY, Command, CommandConfig, ProjectConfig, UserConfig, append_aliases,
    template_references_var, validate_template_syntax,
};
use worktrunk::git::Repository;
use worktrunk::styling::{format_bash_with_gutter, format_heading, println};

use crate::commands::alias::{AliasOptions, AliasSource};
use crate::commands::command_executor::{
    CommandContext, build_hook_context, expand_shell_template,
};
use crate::commands::did_you_mean;
use crate::commands::hooks::{format_pipeline_summary_from_names, step_names_from_config};

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

    for (i, (cfg, source)) in entries.iter().enumerate() {
        if i > 0 {
            println!();
        }
        let bodies: Vec<String> = cfg.commands().map(|c| c.template.clone()).collect();
        println!("{}", format_entry(&name, cfg, *source, &bodies));
    }
    Ok(())
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
    // including `--var KEY=VALUE`, `--KEY=VALUE`, and positional forwarding.
    let mut parse_args = Vec::with_capacity(1 + args.len());
    parse_args.push(name.clone());
    parse_args.extend(args);
    let opts = AliasOptions::parse(parse_args)?;

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

    for (i, (cfg, source)) in entries.iter().enumerate() {
        if i > 0 {
            println!();
        }
        let bodies: Vec<String> = cfg
            .commands()
            .map(|c| render_preview(&c.template, &context_map, &repo, &name))
            .collect::<anyhow::Result<_>>()?;
        println!("{}", format_entry(&name, cfg, *source, &bodies));
    }
    Ok(())
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

/// Format one heading + pipeline summary + body block.
///
/// `bodies[i]` is the text to show for `cfg.commands().nth(i)`. `show` passes
/// the raw template; `dry-run` passes the rendered result. Keeping the layout
/// in one place guarantees the two views stay visually aligned.
fn format_entry(name: &str, cfg: &CommandConfig, source: AliasSource, bodies: &[String]) -> String {
    let mut out = String::new();
    out.push_str(&format_heading(
        &format!("{name} ({})", source.label()),
        None,
    ));
    out.push('\n');
    out.push_str(&pipeline_summary_line(cfg));
    for (cmd, body) in cfg.commands().zip(bodies) {
        out.push('\n');
        out.push_str(&format_bash_with_gutter(&command_display(cmd, body)));
    }
    out
}

/// Display form for a single command: `# <name>` comment line above the body
/// when the command is named, body alone when anonymous. Shared between `show`
/// (body is the raw template) and `dry-run` (body is the rendered template).
fn command_display(cmd: &Command, body: &str) -> String {
    match &cmd.name {
        Some(name) => format!("# {name}\n{body}"),
        None => body.to_string(),
    }
}

/// One-line pipeline summary — same shape used by "Running alias" at runtime.
/// When no steps are named, falls back to a bracketed hint so the line stays
/// non-empty and alignment with `show`/`dry-run` output is consistent.
fn pipeline_summary_line(cfg: &CommandConfig) -> String {
    let step_names = step_names_from_config(cfg);
    let summary = format_pipeline_summary_from_names(&step_names, |n| n.to_string(), |_| None);
    if summary.is_empty() {
        let count = cfg.commands().count();
        if count == 1 {
            cformat!("<dim>(single command)</>")
        } else {
            cformat!("<dim>pipeline: {count} unnamed steps</>")
        }
    } else {
        cformat!("<dim>pipeline: {summary}</>")
    }
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
    fn test_pipeline_summary_line() {
        use insta::assert_snapshot;

        // Single unnamed command
        let cfg = cfg_from_toml(r#"cmd = "echo hi""#);
        assert_snapshot!(pipeline_summary_line(&cfg).ansi_strip(), @"(single command)");

        // Single concurrent step with named commands
        let cfg = cfg_from_toml(
            r#"
[cmd]
build = "cargo build"
test = "cargo test"
"#,
        );
        assert_snapshot!(
            pipeline_summary_line(&cfg).ansi_strip(),
            @"pipeline: build, test"
        );

        // Pipeline with named + concurrent steps
        let cfg = cfg_from_toml(
            r#"
cmd = [
    { install = "npm install" },
    { build = "npm run build", lint = "npm run lint" },
]
"#,
        );
        assert_snapshot!(
            pipeline_summary_line(&cfg).ansi_strip(),
            @"pipeline: install; build, lint"
        );

        // Pipeline of all-unnamed commands falls back to a count
        let cfg = cfg_from_toml(r#"cmd = ["echo a", "echo b"]"#);
        assert_snapshot!(
            pipeline_summary_line(&cfg).ansi_strip(),
            @"pipeline: 2 unnamed steps"
        );
    }

    #[test]
    fn test_format_entry_single_command() {
        let cfg = cfg_from_toml(r#"cmd = "echo {{ branch }}""#);
        let bodies: Vec<String> = cfg.commands().map(|c| c.template.clone()).collect();
        let out = format_entry("greet", &cfg, AliasSource::User, &bodies);
        insta::assert_snapshot!(out.ansi_strip());
    }

    #[test]
    fn test_format_entry_pipeline() {
        let cfg = cfg_from_toml(
            r#"
cmd = [
    { install = "npm install" },
    { build = "npm run build", lint = "npm run lint" },
]
"#,
        );
        let bodies: Vec<String> = cfg.commands().map(|c| c.template.clone()).collect();
        let out = format_entry("deploy", &cfg, AliasSource::Project, &bodies);
        insta::assert_snapshot!(out.ansi_strip());
    }
}
