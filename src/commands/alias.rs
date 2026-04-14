//! Alias command implementation.
//!
//! Aliases are user-defined commands configured in `[aliases]` sections of user
//! or project config. They share the foreground execution path with hooks via
//! `execute_pipeline_foreground` in `command_executor`, which handles pipeline
//! structure, template expansion, concurrent steps, and error handling.
//!
//! ## Execution model
//!
//! Aliases build `ForegroundStep`s from `CommandConfig::steps()`, preserving
//! pipeline structure (`Single` vs `Concurrent`). All commands use lazy template
//! expansion — `vars.*` references resolve from git config at execution time,
//! so prior steps that set vars via `wt config state vars set` are visible to
//! later steps.
//!
//! ## Why foreground and background execution differ
//!
//! Foreground execution (aliases + foreground hooks) uses `execute_shell_command`
//! which streams stdout/stderr to the terminal. Concurrency needs OS threads
//! (`thread::scope`), one per command. Background pipeline execution spawns
//! shell processes with stdout/stderr redirected to log files — no threads
//! needed. The two share preparation (`PreparedStep`) but not execution.
//!
//! ## Trust model
//!
//! User-config aliases are trusted (skip approval). Project-config aliases
//! require command approval. When both define the same alias, both run — user
//! first, then project. The CD directive file is passed through to child
//! processes so inner `wt` invocations can redirect the parent shell's cwd;
//! the EXEC directive file is scrubbed so alias bodies cannot inject
//! arbitrary shell into the interactive session.

use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, bail};
use clap::error::{ContextKind, ContextValue, ErrorKind};
use color_print::cformat;
use strsim::jaro_winkler;
use worktrunk::config::{
    CommandConfig, HookStep, ProjectConfig, UserConfig, append_aliases, template_references_var,
    validate_template_syntax,
};
use worktrunk::git::Repository;
use worktrunk::styling::{
    eprintln, format_bash_with_gutter, info_message, progress_message, warning_message,
};

use crate::commands::command_approval::approve_alias_commands;
use crate::commands::command_executor::{
    CommandContext, CommandOrigin, FailureStrategy, ForegroundStep, PreparedCommand, PreparedStep,
    build_hook_context, execute_pipeline_foreground, expand_shell_template,
};
use crate::commands::hooks::format_pipeline_summary_from_names;
use crate::output::DirectivePassthrough;

/// Built-in `wt step` subcommand names. Aliases with these names are
/// shadowed by the built-in and will never run.
const BUILTIN_STEP_COMMANDS: &[&str] = &[
    "commit",
    "copy-ignored",
    "diff",
    "eval",
    "for-each",
    "promote",
    "prune",
    "push",
    "rebase",
    "relocate",
    "squash",
];

/// Options parsed from the external subcommand args.
#[derive(Debug)]
pub struct AliasOptions {
    pub name: String,
    pub dry_run: bool,
    pub yes: bool,
    pub vars: Vec<(String, String)>,
}

impl AliasOptions {
    /// Parse alias options from the external subcommand args.
    ///
    /// First element is the alias name, remaining are flags:
    /// `--dry-run`, `--yes`/`-y`, `--var KEY=VALUE`, or `--KEY=VALUE`.
    ///
    /// Unknown `--key=value` flags are treated as template variable assignments,
    /// so `--env=staging` is equivalent to `--var env=staging`. The `=` is
    /// required — bare `--key` flags (without a value) are rejected. Use
    /// `--var KEY=VALUE` if a variable name collides with a built-in flag.
    ///
    /// Hyphens in variable names are canonicalized to underscores so users can
    /// write `--my-var=value` and reference `{{ my_var }}` in templates
    /// (minijinja parses `{{ my-var }}` as subtraction).
    pub fn parse(args: Vec<String>) -> anyhow::Result<Self> {
        let Some(name) = args.first().cloned() else {
            bail!("Missing alias name");
        };

        let mut dry_run = false;
        let mut yes = false;
        let mut vars = Vec::new();
        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--dry-run" => dry_run = true,
                "--yes" | "-y" => yes = true,
                "--var" => {
                    i += 1;
                    if i >= args.len() {
                        bail!("--var requires a KEY=VALUE argument");
                    }
                    let pair =
                        crate::cli::parse_key_val(&args[i]).map_err(|e| anyhow::anyhow!(e))?;
                    vars.push(pair);
                }
                arg if arg.starts_with("--var=") => {
                    let pair = crate::cli::parse_key_val(arg.strip_prefix("--var=").unwrap())
                        .map_err(|e| anyhow::anyhow!(e))?;
                    vars.push(pair);
                }
                arg if arg.starts_with("--") => {
                    let rest = &arg[2..];
                    if rest.contains('=') {
                        let pair =
                            crate::cli::parse_key_val(rest).map_err(|e| anyhow::anyhow!(e))?;
                        vars.push(pair);
                    } else {
                        bail!(
                            "Unknown flag '{arg}' for alias '{name}' (use --{rest}=VALUE to pass a variable)"
                        );
                    }
                }
                other => {
                    bail!("Unexpected argument '{other}' for alias '{name}'");
                }
            }
            i += 1;
        }

        Ok(Self {
            name,
            dry_run,
            yes,
            vars,
        })
    }
}

/// Determine whether an alias requires project-config approval.
///
/// Returns the project-config commands for this alias, if any exist.
/// Project-config commands always need approval, regardless of whether
/// user config also defines the same alias — matching hook behavior.
fn alias_needs_approval(
    alias_name: &str,
    project_config: &Option<ProjectConfig>,
) -> Option<CommandConfig> {
    project_config
        .as_ref()
        .and_then(|pc| pc.aliases.get(alias_name))
        .cloned()
}

/// Synthesize clap's native `InvalidSubcommand` error for `wt step <name>`
/// and exit through `enhance_and_exit_error`, so the output matches what
/// `wt <typo>` produces at the top level. Suggestion candidates include both
/// the visible built-in `wt step` subcommands and the user's configured
/// aliases — `SuggestedSubcommand` takes arbitrary strings, so aliases show
/// up in the `tip:` line for typos like `wt step deplyo` → `'deploy'`.
///
/// Uses the same `jaro_winkler > 0.7` threshold as clap's internal
/// `did_you_mean` so the tip line reads identically to the top-level path.
fn unknown_step_command_exit(name: &str, alias_names: &[&str]) -> ! {
    let mut top = crate::cli::build_command();
    let step_cmd = top
        .find_subcommand_mut("step")
        .expect("`step` subcommand is defined in the CLI");
    // `render_usage` uses the command's `bin_name`, which clap only sets
    // after matching. When we synthesize the error ahead of that, the
    // subcommand has no bin_name and usage renders as `Usage: step
    // <COMMAND>` instead of `Usage: wt step [COMMAND]`. Set it to the same
    // display_name applied by `apply_help_template_recursive`.
    step_cmd.set_bin_name("wt step");
    let usage = step_cmd.render_usage();

    let mut candidates: Vec<&str> = step_cmd
        .get_subcommands()
        .filter(|c| !c.is_hide_set())
        .map(|c| c.get_name())
        .filter(|&n| n != "help")
        .collect();
    candidates.extend(alias_names);

    let mut scored: Vec<(f64, String)> = candidates
        .into_iter()
        .map(|candidate| (jaro_winkler(name, candidate), candidate.to_string()))
        .filter(|(score, _)| *score > 0.7)
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let suggestions: Vec<String> = scored.into_iter().map(|(_, n)| n).collect();

    let mut err = clap::Error::new(ErrorKind::InvalidSubcommand).with_cmd(step_cmd);
    err.insert(
        ContextKind::InvalidSubcommand,
        ContextValue::String(name.to_string()),
    );
    if !suggestions.is_empty() {
        err.insert(
            ContextKind::SuggestedSubcommand,
            ContextValue::Strings(suggestions),
        );
    }
    err.insert(ContextKind::Usage, ContextValue::StyledStr(usage));
    crate::enhance_and_exit_error(err)
}

/// Format the "Running alias …" announcement.
///
/// For pipelines or concurrent groups with named commands, includes a summary
/// of the structure (e.g., `Running alias deploy: install; build, lint`).
/// When all commands are unnamed, falls back to the bare form
/// (`Running alias deploy`) — aliases have no natural fallback label for
/// unnamed steps the way hooks use `user`/`project`.
///
/// Sibling of `format_command_label` in `commands/mod.rs`, which builds the
/// non-pipeline `Running {type} {name}` form for hooks. Both apply bold
/// styling to the alias/command name — keep them in sync if styling evolves.
fn format_alias_announcement(name: &str, cmd_config: &CommandConfig) -> String {
    let step_names: Vec<Vec<Option<&str>>> = cmd_config
        .steps()
        .iter()
        .map(|step| match step {
            HookStep::Single(cmd) => vec![cmd.name.as_deref()],
            HookStep::Concurrent(cmds) => cmds.iter().map(|c| c.name.as_deref()).collect(),
        })
        .collect();

    let summary =
        format_pipeline_summary_from_names(&step_names, |n| cformat!("<bold>{n}</>"), |_| None);

    if summary.is_empty() {
        cformat!("Running alias <bold>{name}</>")
    } else {
        cformat!("Running alias <bold>{name}</>: {summary}")
    }
}

/// Run a configured alias by name.
///
/// Looks up the alias in merged config (project config + user config),
/// expands each command template, and executes them in order. Project-config
/// aliases require command approval before execution.
pub fn step_alias(opts: AliasOptions) -> anyhow::Result<()> {
    let repo = Repository::current()?;
    let user_config = UserConfig::load()?;
    let project_id = repo.project_identifier().ok();
    let project_config = ProjectConfig::load(&repo, true)?;

    // Merge aliases: user config first, then project config appends.
    // Matches hook merge semantics — both sources run, project commands
    // need approval regardless of whether user also defines the alias.
    let mut aliases = user_config.aliases(project_id.as_deref());
    if let Some(pc) = project_config.as_ref() {
        append_aliases(&mut aliases, &pc.aliases);
    }

    // Warn about aliases that shadow built-in step commands
    let shadowed: Vec<_> = aliases
        .keys()
        .filter(|k| BUILTIN_STEP_COMMANDS.contains(&k.as_str()))
        .collect();
    if !shadowed.is_empty() {
        let names = shadowed
            .iter()
            .map(|k| cformat!("<bold>{k}</>"))
            .collect::<Vec<_>>()
            .join(", ");
        let (noun, verb) = if shadowed.len() == 1 {
            ("Alias", "shadows a built-in step command")
        } else {
            ("Aliases", "shadow built-in step commands")
        };
        eprintln!(
            "{}",
            warning_message(format!("{noun} {names} {verb} and will never run"))
        );
    }

    let Some(cmd_config) = aliases.get(&opts.name) else {
        // Mirror clap's native `unrecognized subcommand` error so `wt step
        // <typo>` reads the same as `wt <typo>`. Aliases are fed into the
        // `SuggestedSubcommand` list so a typo like `wt step deplyo` still
        // gets `tip: ... 'deploy'` when `deploy` is user-defined. The
        // Aliases block in `wt step --help` is the full discovery surface —
        // the error just needs to point there via `Usage: wt step [COMMAND]`.
        let alias_names: Vec<&str> = aliases
            .keys()
            .filter(|k| !BUILTIN_STEP_COMMANDS.contains(&k.as_str()))
            .map(|k| k.as_str())
            .collect();
        unknown_step_command_exit(&opts.name, &alias_names);
    };

    // Check if this alias needs project-config approval (skip for --dry-run).
    // project_id is required for approval — re-derive with error propagation
    // rather than using the .ok() from above.
    if !opts.dry_run
        && let Some(project_commands) = alias_needs_approval(&opts.name, &project_config)
    {
        let project_id = repo
            .project_identifier()
            .context("Cannot determine project identifier for alias approval")?;
        let approved =
            approve_alias_commands(&project_commands, &opts.name, &project_id, opts.yes)?;
        if !approved {
            return Ok(());
        }
    }

    // Build hook context for template expansion
    let wt = repo.current_worktree();
    let wt_path = wt.root().context("Failed to get worktree root")?;
    let branch = wt.branch().ok().flatten();
    let ctx = CommandContext::new(&repo, &user_config, branch.as_deref(), &wt_path, false);

    let extra_refs: Vec<(&str, &str)> = opts
        .vars
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let context_map = build_hook_context(&ctx, &extra_refs)?;

    // Build JSON context for stdin
    let context_json = serde_json::to_string(&context_map)
        .expect("HashMap<String, String> serialization should never fail");

    if opts.dry_run {
        let expanded: Vec<_> = cmd_config
            .commands()
            .map(|cmd| render_for_dry_run(&cmd.template, &context_map, &repo, &opts.name))
            .collect::<anyhow::Result<_>>()?;
        eprintln!(
            "{}",
            info_message(cformat!(
                "Alias <bold>{}</> would run:\n{}",
                opts.name,
                expanded
                    .iter()
                    .map(|c| format_bash_with_gutter(c))
                    .collect::<Vec<_>>()
                    .join("\n")
            ))
        );
        return Ok(());
    }

    eprintln!(
        "{}",
        progress_message(format_alias_announcement(&opts.name, cmd_config))
    );

    // CD passed through, EXEC scrubbed (see `output::global` for rationale).
    let directives = DirectivePassthrough::inherit_from_env();

    // Build ForegroundSteps: all alias commands use lazy expansion so vars.*
    // references resolved from git config at execution time are visible to
    // later steps that set vars via `wt config state vars set`.
    let origin = CommandOrigin::Alias {
        name: opts.name.clone(),
    };
    let foreground_steps: Vec<ForegroundStep> = cmd_config
        .steps()
        .iter()
        .map(|step| {
            let prepared = match step {
                HookStep::Single(cmd) => {
                    PreparedStep::Single(alias_prepared_command(cmd, &context_json))
                }
                HookStep::Concurrent(cmds) => PreparedStep::Concurrent(
                    cmds.iter()
                        .map(|cmd| alias_prepared_command(cmd, &context_json))
                        .collect(),
                ),
            };
            ForegroundStep {
                step: prepared,
                origin: origin.clone(),
            }
        })
        .collect();

    execute_pipeline_foreground(
        &foreground_steps,
        &repo,
        &wt_path,
        &directives,
        FailureStrategy::FailFast,
        true, // aliases support concurrent execution
    )
}

/// Render a command template for `--dry-run` display.
///
/// Mirrors execution-time lazy semantics: templates referencing `vars.*` may
/// read values set by earlier pipeline steps via git config, and at dry-run
/// time those values haven't been written yet (even if git config happens to
/// hold a stale value from a previous run, the execution path would overwrite
/// it). For those templates, syntax-validate (catching typos like
/// `{{ vars..foo }}`) and show the raw template. Other templates expand
/// eagerly against the initial context just like before.
fn render_for_dry_run(
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

/// Build a PreparedCommand for an alias, deferring template expansion to execution time.
fn alias_prepared_command(cmd: &worktrunk::config::Command, context_json: &str) -> PreparedCommand {
    PreparedCommand {
        name: cmd.name.clone(),
        expanded: cmd.template.clone(),
        context_json: context_json.to_string(),
        lazy_template: Some(cmd.template.clone()),
    }
}

/// Where an alias came from. When the same name is defined in both configs,
/// the listing shows both entries in runtime order (user first, then project)
/// rather than merging them, so users see the real commands from each source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum AliasSource {
    User,
    Project,
}

/// Splice the Aliases section into clap-rendered `wt step` help, if any
/// aliases are configured.
///
/// Called from the help path in `help.rs` — which covers `wt step`
/// (via `arg_required_else_help`), `wt step -h`, and `wt step --help`,
/// all of which flow through clap's `DisplayHelp` error. Tolerates
/// running outside a repository: user-config aliases still list,
/// project-config aliases just get skipped.
pub(crate) fn augment_step_help(help: &str) -> String {
    // Help must not emit deprecation/unknown-field warnings or write `.new`
    // migration files as a side effect of rendering the alias list.
    worktrunk::config::suppress_warnings();

    let aliases = load_aliases_for_listing();
    if aliases.is_empty() {
        return help.to_string();
    }

    // Place the Aliases section right after Commands so it sits next to the
    // built-ins it extends. Clap has no template-level hook for inserting
    // between sections, so we splice around the `Options:` heading in the
    // rendered output. The search prefix is derived from the same style
    // clap uses (our `help_styles().get_header()`), so if the header
    // styling changes both sides move together.
    let aliases_section = render_aliases_section(&aliases);
    let options_heading = format!(
        "{}Options:",
        crate::cli::help_styles().get_header().render()
    );
    match help.find(&options_heading) {
        Some(pos) => format!("{}{aliases_section}\n\n{}", &help[..pos], &help[pos..]),
        None => {
            // Clap's styling changed; fall back to appending so we don't
            // silently drop the aliases list.
            format!("{help}\n{aliases_section}")
        }
    }
}

/// Format the list of aliases as a styled help section.
///
/// Matches clap's "Commands:" / "Options:" styling (bold+green heading,
/// bold+cyan names) so the Aliases section blends in with the rest of
/// `-h` output. Returns the block without leading or trailing blank lines —
/// the caller positions it.
///
/// When a name is defined in both user and project config, two rows are
/// shown (user first, then project, matching runtime order). Both rows
/// carry a source marker so the reader can tell which pipeline is which.
fn render_aliases_section(entries: &[(String, CommandConfig, AliasSource)]) -> String {
    use std::fmt::Write as _;

    // Names appearing in both sources need source markers to be distinguishable.
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for (name, _, _) in entries {
        *counts.entry(name.as_str()).or_insert(0) += 1;
    }

    let mut out = String::new();
    let _ = writeln!(out, "{}", cformat!("<bold><green>Aliases:</></>"));
    let name_width = entries.iter().map(|(n, _, _)| n.len()).max().unwrap_or(0);
    let mut first = true;
    for (name, cfg, source) in entries {
        if !first {
            out.push('\n');
        }
        first = false;
        let padding = " ".repeat(name_width - name.len());
        let summary = format_alias_summary(cfg);
        // Shadowed-by-builtin is a warning (yellow) and takes precedence over
        // the source marker so the row doesn't pile up suffixes.
        let suffix = if BUILTIN_STEP_COMMANDS.contains(&name.as_str()) {
            cformat!(" <yellow>(shadowed by built-in)</>")
        } else if counts.get(name.as_str()).copied().unwrap_or(0) > 1 {
            match source {
                AliasSource::User => cformat!(" <dim>(user)</>"),
                AliasSource::Project => cformat!(" <dim>(project)</>"),
            }
        } else {
            String::new()
        };
        let _ = write!(
            out,
            "  {name_styled}{padding}  {summary}{suffix}",
            name_styled = cformat!("<bold><cyan>{name}</></>"),
        );
    }
    out
}

/// Load aliases for display as a flat list sorted by name, with source tagged.
///
/// Duplicate names (same alias in both user and project) appear twice — once
/// per source, user first, matching runtime execution order. Showing each
/// separately preserves the individual command text; merging them would
/// reduce to an uninformative step count when both are unnamed singles.
///
/// The caller (`augment_step_help`) latches `suppress_warnings()` before
/// reaching here so the standard `UserConfig::load()` stays quiet: no
/// deprecation warnings, no `.new` file writes, no approved-commands copy.
/// Project config is parsed directly from TOML rather than via
/// `ProjectConfig::load` because the `aliases` table has no deprecated forms
/// — skipping the migration avoids the unrelated warnings entirely.
///
/// Tolerates missing or unloadable config: this is a discovery surface, not
/// an execution surface, so we'd rather show the built-in commands than
/// error out when a repo isn't detected or a config file is malformed.
/// `step_alias` surfaces those errors at execution time.
fn load_aliases_for_listing() -> Vec<(String, CommandConfig, AliasSource)> {
    let repo = Repository::current().ok();
    let project_id = repo.as_ref().and_then(|r| r.project_identifier().ok());

    let user_aliases = UserConfig::load()
        .ok()
        .map(|uc| uc.aliases(project_id.as_deref()))
        .unwrap_or_default();

    let project_aliases = repo
        .as_ref()
        .and_then(load_project_aliases_silent)
        .unwrap_or_default();

    let mut entries: Vec<(String, CommandConfig, AliasSource)> = user_aliases
        .into_iter()
        .map(|(n, c)| (n, c, AliasSource::User))
        .chain(
            project_aliases
                .into_iter()
                .map(|(n, c)| (n, c, AliasSource::Project)),
        )
        .collect();

    // Sort by name; for ties, user before project (derived Ord on AliasSource)
    // so duplicates display in runtime execution order.
    entries.sort_by(|a, b| a.0.cmp(&b.0).then(a.2.cmp(&b.2)));
    entries
}

/// Parse `.config/wt.toml` directly, extracting just `aliases`, without
/// triggering `ProjectConfig::load`'s deprecation warning and hint-writing
/// side effects. See `load_aliases_for_listing` for why.
fn load_project_aliases_silent(repo: &Repository) -> Option<BTreeMap<String, CommandConfig>> {
    let path = repo.project_config_path().ok().flatten()?;
    if !path.exists() {
        return None;
    }
    let contents = std::fs::read_to_string(&path).ok()?;
    let config: ProjectConfig = toml::from_str(&contents).ok()?;
    Some(config.aliases)
}

/// One-line summary of an alias's command(s) suitable for a help listing.
///
/// Single-command aliases show the template's first line (with `…` if the
/// template spans multiple lines). Pipelines show the shared named-step
/// summary used by the "Running alias" announcement.
fn format_alias_summary(cfg: &CommandConfig) -> String {
    // `is_pipeline()` is `steps.len() > 1`, but a single-step concurrent
    // alias (one `HookStep::Concurrent` holding several commands) would
    // fall into the else branch and hide all but the first command. Count
    // actual commands instead.
    if cfg.commands().count() > 1 {
        let step_names: Vec<Vec<Option<&str>>> = cfg
            .steps()
            .iter()
            .map(|step| match step {
                HookStep::Single(cmd) => vec![cmd.name.as_deref()],
                HookStep::Concurrent(cmds) => cmds.iter().map(|c| c.name.as_deref()).collect(),
            })
            .collect();
        let summary = format_pipeline_summary_from_names(&step_names, |n| n.to_string(), |_| None);
        if summary.is_empty() {
            format!("<{} steps>", cfg.commands().count())
        } else {
            summary
        }
    } else {
        let cmd = cfg
            .commands()
            .next()
            .expect("CommandConfig always contains at least one command");
        let first = cmd.template.lines().next().unwrap_or("").trim_end();
        if cmd.template.lines().count() > 1 {
            format!("{first}…")
        } else {
            first.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ansi_str::AnsiStr;

    fn parse(args: &[&str]) -> anyhow::Result<AliasOptions> {
        AliasOptions::parse(args.iter().map(|s| s.to_string()).collect())
    }

    /// Parse a TOML snippet of the form `cmd = ...` into a CommandConfig.
    /// CommandConfig has no public multi-step constructor, so round-tripping
    /// through TOML is the simplest way to build pipeline fixtures.
    fn cfg_from_toml(toml_str: &str) -> CommandConfig {
        #[derive(serde::Deserialize)]
        struct Wrap {
            cmd: CommandConfig,
        }
        toml::from_str::<Wrap>(toml_str).unwrap().cmd
    }

    #[test]
    fn test_format_alias_announcement_single_unnamed() {
        let cfg = cfg_from_toml(r#"cmd = "echo hi""#);
        let msg = format_alias_announcement("deploy", &cfg);
        insta::assert_snapshot!(msg.ansi_strip(), @"Running alias deploy");
    }

    #[test]
    fn test_format_alias_announcement_pipeline_all_unnamed() {
        // Pipeline of unnamed strings → no summary suffix.
        let cfg = cfg_from_toml(r#"cmd = ["echo a", "echo b"]"#);
        let msg = format_alias_announcement("deploy", &cfg);
        insta::assert_snapshot!(msg.ansi_strip(), @"Running alias deploy");
    }

    #[test]
    fn test_format_alias_announcement_concurrent_named() {
        // Single concurrent step (named table form).
        let cfg = cfg_from_toml(
            r#"
[cmd]
build = "cargo build"
test = "cargo test"
"#,
        );
        let msg = format_alias_announcement("check", &cfg);
        insta::assert_snapshot!(msg.ansi_strip(), @"Running alias check: build, test");
    }

    #[test]
    fn test_format_alias_announcement_pipeline_named() {
        // Pipeline: serial named step then concurrent named step.
        let cfg = cfg_from_toml(
            r#"
cmd = [
    { install = "npm install" },
    { build = "npm run build", lint = "npm run lint" },
]
"#,
        );
        let msg = format_alias_announcement("deploy", &cfg);
        insta::assert_snapshot!(msg.ansi_strip(), @"Running alias deploy: install; build, lint");
    }

    #[test]
    fn test_format_alias_announcement_mixed_named_unnamed() {
        // Pipeline mixing anonymous strings and named steps. Unnamed entries
        // are skipped from the summary (aliases have no fallback label).
        let cfg = cfg_from_toml(
            r#"
cmd = [
    "echo first",
    { build = "cargo build", test = "cargo test" },
]
"#,
        );
        let msg = format_alias_announcement("ci", &cfg);
        insta::assert_snapshot!(msg.ansi_strip(), @"Running alias ci: build, test");
    }

    #[test]
    fn test_parse() {
        use insta::assert_debug_snapshot;
        assert_debug_snapshot!(parse(&["deploy"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            dry_run: false,
            yes: false,
            vars: [],
        }
        "#);
        assert_debug_snapshot!(parse(&["deploy", "--dry-run"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            dry_run: true,
            yes: false,
            vars: [],
        }
        "#);
        assert_debug_snapshot!(parse(&["deploy", "--yes"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            dry_run: false,
            yes: true,
            vars: [],
        }
        "#);
        assert_debug_snapshot!(parse(&["deploy", "-y"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            dry_run: false,
            yes: true,
            vars: [],
        }
        "#);
        assert_debug_snapshot!(parse(&["deploy", "--var", "key=value"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            dry_run: false,
            yes: false,
            vars: [
                (
                    "key",
                    "value",
                ),
            ],
        }
        "#);
        // --var=key=value (equals form)
        assert_debug_snapshot!(parse(&["deploy", "--var=key=value"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            dry_run: false,
            yes: false,
            vars: [
                (
                    "key",
                    "value",
                ),
            ],
        }
        "#);
        // Value containing equals sign
        assert_debug_snapshot!(parse(&["deploy", "--var", "url=http://host?a=1"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            dry_run: false,
            yes: false,
            vars: [
                (
                    "url",
                    "http://host?a=1",
                ),
            ],
        }
        "#);
        // Multiple vars + flags
        assert_debug_snapshot!(parse(&["deploy", "--var", "a=1", "--var", "b=2", "--dry-run"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            dry_run: true,
            yes: false,
            vars: [
                (
                    "a",
                    "1",
                ),
                (
                    "b",
                    "2",
                ),
            ],
        }
        "#);
        // Empty value accepted
        assert_debug_snapshot!(parse(&["deploy", "--var", "key="]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            dry_run: false,
            yes: false,
            vars: [
                (
                    "key",
                    "",
                ),
            ],
        }
        "#);
        // --key=value shorthand
        assert_debug_snapshot!(parse(&["deploy", "--env=staging"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            dry_run: false,
            yes: false,
            vars: [
                (
                    "env",
                    "staging",
                ),
            ],
        }
        "#);
        // --key=value with equals in value
        assert_debug_snapshot!(parse(&["deploy", "--url=http://host?a=1"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            dry_run: false,
            yes: false,
            vars: [
                (
                    "url",
                    "http://host?a=1",
                ),
            ],
        }
        "#);
        // --key=value mixed with --var and flags
        assert_debug_snapshot!(parse(&["deploy", "--env=prod", "--var", "region=us-east", "--dry-run"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            dry_run: true,
            yes: false,
            vars: [
                (
                    "env",
                    "prod",
                ),
                (
                    "region",
                    "us-east",
                ),
            ],
        }
        "#);
        // --key= (empty value)
        assert_debug_snapshot!(parse(&["deploy", "--env="]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            dry_run: false,
            yes: false,
            vars: [
                (
                    "env",
                    "",
                ),
            ],
        }
        "#);
        // Hyphens in shorthand key are canonicalized to underscores
        assert_debug_snapshot!(parse(&["deploy", "--my-var=value"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            dry_run: false,
            yes: false,
            vars: [
                (
                    "my_var",
                    "value",
                ),
            ],
        }
        "#);
        // Hyphens in --var KEY=VALUE are canonicalized too
        assert_debug_snapshot!(parse(&["deploy", "--var", "my-var=value"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            dry_run: false,
            yes: false,
            vars: [
                (
                    "my_var",
                    "value",
                ),
            ],
        }
        "#);
        // Hyphens in --var=KEY=VALUE form
        assert_debug_snapshot!(parse(&["deploy", "--var=my-var=value"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            dry_run: false,
            yes: false,
            vars: [
                (
                    "my_var",
                    "value",
                ),
            ],
        }
        "#);
        // Already-underscored keys pass through unchanged
        assert_debug_snapshot!(parse(&["deploy", "--my_var=value"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            dry_run: false,
            yes: false,
            vars: [
                (
                    "my_var",
                    "value",
                ),
            ],
        }
        "#);
        // Multiple hyphens in a single key
        assert_debug_snapshot!(parse(&["deploy", "--long-var-name=x"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            dry_run: false,
            yes: false,
            vars: [
                (
                    "long_var_name",
                    "x",
                ),
            ],
        }
        "#);
        // Hyphens in value are preserved (only key is canonicalized)
        assert_debug_snapshot!(parse(&["deploy", "--region=us-east-1"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            dry_run: false,
            yes: false,
            vars: [
                (
                    "region",
                    "us-east-1",
                ),
            ],
        }
        "#);
    }

    #[test]
    fn test_parse_errors() {
        use insta::assert_snapshot;
        assert_snapshot!(parse(&[]).unwrap_err(), @"Missing alias name");
        assert_snapshot!(parse(&["deploy", "--var"]).unwrap_err(), @"--var requires a KEY=VALUE argument");
        assert_snapshot!(parse(&["deploy", "--var", "noequals"]).unwrap_err(), @"invalid KEY=VALUE: no `=` found in `noequals`");
        assert_snapshot!(parse(&["deploy", "--verbose"]).unwrap_err(), @"Unknown flag '--verbose' for alias 'deploy' (use --verbose=VALUE to pass a variable)");
        assert_snapshot!(parse(&["deploy", "arg1"]).unwrap_err(), @"Unexpected argument 'arg1' for alias 'deploy'");
        assert_snapshot!(parse(&["deploy", "--var", "=value"]).unwrap_err(), @"invalid KEY=VALUE: key cannot be empty");
        assert_snapshot!(parse(&["deploy", "--=value"]).unwrap_err(), @"invalid KEY=VALUE: key cannot be empty");
    }

    /// Verify BUILTIN_STEP_COMMANDS stays in sync with the actual StepCommand variants.
    ///
    /// If a new step subcommand is added without updating BUILTIN_STEP_COMMANDS,
    /// this test fails — preventing aliases from silently conflicting with built-ins.
    #[test]
    fn test_builtin_step_commands_matches_clap() {
        use crate::cli::Cli;
        use clap::CommandFactory;

        let app = Cli::command();
        let step_cmd = app
            .get_subcommands()
            .find(|c| c.get_name() == "step")
            .expect("step subcommand exists");

        let clap_names: Vec<&str> = step_cmd.get_subcommands().map(|s| s.get_name()).collect();

        // Every clap subcommand should be in BUILTIN_STEP_COMMANDS
        for name in &clap_names {
            assert!(
                BUILTIN_STEP_COMMANDS.contains(name),
                "Step subcommand '{name}' is missing from BUILTIN_STEP_COMMANDS. \
                 Add it to prevent aliases from silently conflicting with the built-in."
            );
        }

        // Every BUILTIN_STEP_COMMANDS entry should still be a real subcommand
        for name in BUILTIN_STEP_COMMANDS {
            assert!(
                clap_names.contains(name),
                "BUILTIN_STEP_COMMANDS contains '{name}' but no such step subcommand exists. \
                 Remove it from the list."
            );
        }
    }

    #[test]
    fn test_format_alias_summary_single_command() {
        let cfg = cfg_from_toml(r#"cmd = "echo hello""#);
        assert_eq!(format_alias_summary(&cfg), "echo hello");
    }

    #[test]
    fn test_format_alias_summary_multiline_gets_ellipsis() {
        let cfg = cfg_from_toml(
            r#"cmd = """
git fetch --all --prune
git rebase @{u}
""""#,
        );
        assert_eq!(format_alias_summary(&cfg), "git fetch --all --prune…");
    }

    #[test]
    fn test_format_alias_summary_pipeline_named() {
        let cfg = cfg_from_toml(
            r#"
cmd = [
    { install = "npm install" },
    { build = "npm run build", lint = "npm run lint" },
]
"#,
        );
        assert_eq!(format_alias_summary(&cfg), "install; build, lint");
    }

    #[test]
    fn test_format_alias_summary_concurrent_named() {
        // Single-step concurrent form: `[aliases.check]\nbuild=…\ntest=…`
        // — one step, multiple commands. Must use the pipeline formatter,
        // not fall back to "show first command's template".
        let cfg = cfg_from_toml(
            r#"
[cmd]
build = "cargo build"
test = "cargo test"
"#,
        );
        assert_eq!(format_alias_summary(&cfg), "build, test");
    }

    #[test]
    fn test_format_alias_summary_pipeline_all_unnamed() {
        // Anonymous pipeline entries fall back to a step count.
        let cfg = cfg_from_toml(r#"cmd = ["echo a", "echo b"]"#);
        assert_eq!(format_alias_summary(&cfg), "<2 steps>");
    }

    #[test]
    fn test_render_aliases_section_source_annotations() {
        // Names unique to one source have no annotation. Names defined in
        // both sources show two rows (user first, matching runtime order)
        // and each row carries a source marker so the reader can tell them
        // apart. Shadowed-by-builtin takes precedence over the source marker.
        let entries = vec![
            (
                "only-user".to_string(),
                cfg_from_toml(r#"cmd = "echo u""#),
                AliasSource::User,
            ),
            (
                "only-project".to_string(),
                cfg_from_toml(r#"cmd = "echo p""#),
                AliasSource::Project,
            ),
            (
                "shared".to_string(),
                cfg_from_toml(r#"cmd = "echo from-user""#),
                AliasSource::User,
            ),
            (
                "shared".to_string(),
                cfg_from_toml(r#"cmd = "echo from-project""#),
                AliasSource::Project,
            ),
        ];
        // Caller passes pre-sorted entries; mirror that here.
        let mut sorted = entries;
        sorted.sort_by(|a, b| a.0.cmp(&b.0).then(a.2.cmp(&b.2)));
        let rendered = render_aliases_section(&sorted);
        let rendered = rendered.ansi_strip();
        insta::assert_snapshot!(rendered, @r"
        Aliases:
          only-project  echo p
          only-user     echo u
          shared        echo from-user (user)
          shared        echo from-project (project)
        ");
    }
}
