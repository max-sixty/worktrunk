//! `wt config alias` subcommands.
//!
//! Introspection and preview for aliases configured in user config
//! (`~/.config/worktrunk/config.toml`) and project config (`.config/wt.toml`).
//! `show <name>` prints the template text, source-labeled, with one gutter
//! block per alias entry and `# <name>` comment lines above named pipeline
//! steps. `show` with no name prints that block for every configured alias in
//! name order — equivalent to `wt config alias show <name>` run for each.
//! `dry-run` parses a per-invocation argument vector with the same parser
//! `wt <alias>` uses, then expands templates using the same context as
//! execution — so previews match what the real run will do. `show <name>` and
//! `dry-run` share a layout; only the header verb differs (`:` vs
//! ` would run:`).
//!
//! ## Why `dry-run` lives here rather than on the alias dispatch
//!
//! Previous versions exposed dry-run via `wt <alias> --dry-run`. That routed
//! through `AliasOptions::parse` and required every caller to handle the
//! "preview vs run" branch. Lifting it into a dedicated subcommand keeps the
//! alias-dispatch path single-purpose (always runs) and gives preview a
//! natural home alongside `show`.

use std::collections::BTreeSet;
use std::io::Write;

use anyhow::Context;
use color_print::cformat;
use worktrunk::config::{
    ALIAS_ARGS_KEY, CommandConfig, ProjectConfig, UserConfig, referenced_vars_for_config,
};
use worktrunk::git::{Repository, WorktrunkError};
use worktrunk::styling::{format_bash_with_gutter, info_message, println};

use crate::commands::alias::{
    AliasOptions, TOP_LEVEL_BUILTINS, load_aliases, load_aliases_for_listing,
};
use crate::commands::build_invalid_subcommand_error;
use crate::commands::command_executor::{
    CommandContext, build_hook_context, render_template_preview,
};
use crate::commands::did_you_mean;
use crate::commands::hooks::HookSource;

/// Show the configured template(s) for an alias — or, with no name, every
/// configured alias's template(s).
///
/// When the same name is defined in both user and project config, both
/// entries are printed (user first, matching runtime execution order).
pub fn handle_alias_show(name: Option<String>) -> anyhow::Result<()> {
    let Some(name) = name else {
        return list_aliases();
    };

    let repo = Repository::current()?;
    let user_config = repo.user_config();
    let project_config = repo.project_config()?;
    let entries = entries_for_name(&repo, user_config, project_config, &name);

    if entries.is_empty() {
        return Err(unknown_alias_error(
            &repo,
            user_config,
            project_config,
            &name,
            "show",
        ));
    }

    warn_if_shadowed(&name);

    for (cfg, source) in &entries {
        let bodies: Vec<String> = cfg.commands().map(|c| c.template.clone()).collect();
        println!("{}", format_entry(&name, cfg, *source, &bodies, None));
    }
    Ok(())
}

/// Show every configured alias's full definition — the same header + gutter
/// block `wt config alias show <name>` prints, emitted for each alias in name
/// order (and for each source when a name is defined in both, user first,
/// matching runtime execution order). `wt --help` shows a compact names-only
/// list (`render_aliases_help_section`) and points here for these.
///
/// Tolerates running outside a repository (user-config aliases still list,
/// project-config ones are skipped) and outside a config (prints a note).
/// Warnings are suppressed: this is a discovery surface, so a deprecated
/// `wt.toml` shouldn't make `wt config alias show` noisy — `wt config update`
/// is where deprecations get reported. A name shadowed by a top-level
/// built-in still gets the same stderr warning `wt config alias show <name>`
/// emits, once per name.
fn list_aliases() -> anyhow::Result<()> {
    worktrunk::config::suppress_warnings();
    let entries = load_aliases_for_listing();
    if entries.is_empty() {
        println!("{}", info_message("No aliases configured"));
        return Ok(());
    }

    // `entries` is sorted by (name, source), so a name's entries are adjacent.
    // Warn once per shadowed name — matching `show <name>`, which warns on the
    // name, not per entry. The `Aliases:` names are invoked as `wt <name>`, so
    // shadowing is judged from the top-level perspective (an alias named `list`
    // is unreachable; one named `commit` is not — `wt commit` runs it).
    let mut prev_name: Option<&str> = None;
    for (name, _, _) in &entries {
        if prev_name != Some(name.as_str()) {
            warn_if_shadowed(name);
            prev_name = Some(name);
        }
    }

    let mut out = String::new();
    for (name, cfg, source) in &entries {
        let bodies: Vec<String> = cfg.commands().map(|c| c.template.clone()).collect();
        out.push_str(&format_entry(name, cfg, *source, &bodies, None));
        out.push('\n');
    }
    crate::help_pager::show_help_in_pager(&out, true);
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
/// Rendering goes through [`render_template_preview`], which mirrors
/// execution-time semantics: templates referencing `vars.*` are shown raw
/// (after syntax validation) because those values resolve from git config
/// when the step runs, potentially written by earlier pipeline steps. Other
/// templates expand against the current context.
pub fn handle_alias_dry_run(name: String, args: Vec<String>) -> anyhow::Result<()> {
    let repo = Repository::current()?;
    let user_config = repo.user_config();
    let project_config = repo.project_config()?;
    let entries = entries_for_name(&repo, user_config, project_config, &name);

    if entries.is_empty() {
        return Err(unknown_alias_error(
            &repo,
            user_config,
            project_config,
            &name,
            "dry-run",
        ));
    }

    // Reuse the real parser so previews stay aligned with runtime parsing —
    // including `--KEY=VALUE` routing and positional forwarding. When both
    // user and project configs define the alias, union the referenced vars
    // so a flag binds if any entry's template references it.
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    for (cfg, _) in &entries {
        referenced.extend(referenced_vars_for_config(cfg, &name)?);
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
    let ctx = CommandContext::new(&repo, user_config, branch.as_deref(), &wt_path, false);
    let extra_refs: Vec<(&str, &str)> = opts
        .vars
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let mut context_map = build_hook_context(&ctx, &extra_refs, None)?;
    context_map.insert(
        ALIAS_ARGS_KEY.to_string(),
        serde_json::to_string(&opts.positional_args)
            .expect("Vec<String> serialization should never fail"),
    );

    let routing = format_routing_summary(&opts);

    for (cfg, source) in &entries {
        let bodies: Vec<String> = cfg
            .commands()
            .map(|c| render_template_preview(&c.template, &context_map, &repo, &name))
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

/// Resolve `name` against user + project config, preserving runtime execution
/// order (user first, then project).
fn entries_for_name(
    repo: &Repository,
    user_config: &UserConfig,
    project_config: Option<&ProjectConfig>,
    name: &str,
) -> Vec<(CommandConfig, HookSource)> {
    let project_id = repo.project_identifier().ok();
    let mut entries = Vec::new();
    if let Some(cfg) = user_config.aliases(project_id.as_deref()).get(name) {
        entries.push((cfg.clone(), HookSource::User));
    }
    if let Some(pc) = project_config
        && let Some(cfg) = pc.aliases.get(name)
    {
        entries.push((cfg.clone(), HookSource::Project));
    }
    entries
}

/// Print an "unrecognized alias 'X'" error matching clap's `InvalidSubcommand`
/// layout — same `error:` / `tip:` / `Usage:` block and exit code 2 as
/// `wt <typo>` and `wt step <typo>`. `sub` is `"show"` or `"dry-run"`,
/// anchoring the error on the real clap subcommand so the Usage line reads
/// `Usage: wt config alias <sub> <NAME>`.
///
/// Built as a real `clap::Error` with `ErrorKind::InvalidSubcommand`, rendered
/// by clap, then string-substituted to say "alias" instead of "subcommand" —
/// the positional is an alias name, not a subcommand, so the tighter wording
/// reads more honestly at this surface. Going through clap's rendering gets
/// NO_COLOR / TTY detection, singular-vs-plural "similar" phrasing, and
/// styling correct automatically; modifying the final string is cheaper than
/// reimplementing those.
///
/// Substitutions are scoped to clap's fixed phrases, never the bare word
/// "subcommand" — otherwise an alias or typo containing the literal string
/// `subcommand` (e.g. `my-subcommand`) would be mangled when echoed into
/// the error. Plural `subcommands` is rewritten before singular `subcommand`
/// because `"similar subcommand"` is a prefix of `"similar subcommands"`.
///
/// Returns `AlreadyDisplayed { exit_code: 2 }` rather than calling
/// `process::exit`, so `main`'s `finish_command` still runs `terminate_output`
/// (ANSI reset for shell integration) and `diagnostic::write_if_verbose`.
fn unknown_alias_error(
    repo: &Repository,
    user_config: &UserConfig,
    project_config: Option<&ProjectConfig>,
    name: &str,
    sub: &str,
) -> anyhow::Error {
    let aliases = load_aliases(Some(repo), user_config, project_config);
    let suggestions = did_you_mean(name, aliases.into_keys());

    let mut top = crate::cli::build_command();
    let sub_cmd = top
        .find_subcommand_mut("config")
        .expect("`config` subcommand is defined in the CLI")
        .find_subcommand_mut("alias")
        .expect("`config alias` subcommand is defined in the CLI")
        .find_subcommand_mut(sub)
        .unwrap_or_else(|| panic!("`config alias {sub}` subcommand is defined in the CLI"));
    // `render_usage` needs `bin_name`; clap only sets it on match, so when we
    // synthesize the error ahead of that, set it to the display_name
    // `apply_help_template_recursive` would apply.
    sub_cmd.set_bin_name(format!("wt config alias {sub}"));
    let err = build_invalid_subcommand_error(sub_cmd, name, suggestions);

    let rewritten = err
        .render()
        .ansi()
        .to_string()
        .replace("unrecognized subcommand", "unrecognized alias")
        .replace("similar subcommands", "similar aliases")
        .replace("similar subcommand", "similar alias");

    let mut stream = anstream::AutoStream::auto(std::io::stderr());
    let _ = write!(stream, "{rewritten}");
    WorktrunkError::AlreadyDisplayed { exit_code: 2 }.into()
}

/// Format one alias entry: `○ Alias <name> (<source>)[ <verb>]:` header
/// followed by a single gutter block of the command bodies. Each named step
/// gets a `# <name>` comment line above its body; anonymous steps render the
/// body alone. Joining into one block matches the old `--dry-run` layout and
/// keeps `show`/`dry-run` visually aligned — the only difference is the verb.
fn format_entry(
    name: &str,
    cfg: &CommandConfig,
    source: HookSource,
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
    source: HookSource,
    bodies: &[String],
    verb: Option<&str>,
    routing: Option<&str>,
) -> String {
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
        "Alias <bold>{name}</> ({source}){suffix}\n{}",
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
        let out = format_entry("greet", &cfg, HookSource::User, &bodies, None);
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
        let out = format_entry("deploy", &cfg, HookSource::Project, &bodies, None);
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
            HookSource::Project,
            &bodies,
            Some("would run"),
        );
        insta::assert_snapshot!(out.ansi_strip());
    }
}
