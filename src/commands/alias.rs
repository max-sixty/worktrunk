//! Alias command implementation.
//!
//! Aliases are user-defined commands configured in `[aliases]` sections of user
//! or project config. They share execution infrastructure with hooks:
//! `execute_shell_command` (signal forwarding, ANSI reset, `Cmd` tracing),
//! `CommandConfig` (pipeline steps), template expansion, and the approval system.
//!
//! ## Execution model
//!
//! Aliases iterate `CommandConfig::steps()`, preserving pipeline structure:
//! - `HookStep::Single` — serial execution, fail-fast
//! - `HookStep::Concurrent` — commands spawn via `thread::scope`, all run to
//!   completion, first error propagated
//!
//! In pipelines, templates referencing `vars.*` use lazy expansion — deferred
//! until execution time so prior steps can set vars via git config.
//!
//! ## Trust model
//!
//! User-config aliases are trusted (skip approval). Project-config aliases
//! require command approval. When both define the same alias, both run — user
//! first, then project. The directive file is passed through to child processes
//! (same trust profile as foreground hooks).

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, bail};
use color_print::cformat;
use worktrunk::config::{
    CommandConfig, HookStep, ProjectConfig, UserConfig, append_aliases, expand_template,
    template_references_var,
};
use worktrunk::git::{Repository, WorktrunkError};
use worktrunk::shell_exec::DIRECTIVE_FILE_ENV_VAR;
use worktrunk::styling::{
    eprintln, format_bash_with_gutter, info_message, progress_message, warning_message,
};

use crate::commands::command_approval::approve_alias_commands;
use crate::commands::command_executor::{
    CommandContext, build_hook_context, expand_shell_template, wait_first_error,
};
use crate::output::execute_shell_command;

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
                    let pair = parse_var(&args[i])?;
                    vars.push(pair);
                }
                arg if arg.starts_with("--var=") => {
                    let pair = parse_var(arg.strip_prefix("--var=").unwrap())?;
                    vars.push(pair);
                }
                arg if arg.starts_with("--") => {
                    let rest = &arg[2..];
                    if let Some((key, value)) = rest.split_once('=') {
                        if key.is_empty() {
                            bail!("Variable name must not be empty (got '--={value}')");
                        }
                        vars.push((key.to_string(), value.to_string()));
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

fn parse_var(s: &str) -> anyhow::Result<(String, String)> {
    let (key, value) = s.split_once('=').context("--var value must be KEY=VALUE")?;
    if key.is_empty() {
        bail!("--var key must not be empty (got '={value}')");
    }
    Ok((key.to_string(), value.to_string()))
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
        .and_then(|pc| pc.aliases.as_ref())
        .and_then(|a| a.get(alias_name))
        .cloned()
}

/// Find the closest match for `input` among `candidates` using Jaro similarity.
///
/// Returns `Some(match)` if a candidate is sufficiently similar (threshold 0.7),
/// `None` otherwise. Uses `jaro` (not `jaro_winkler`) with the same threshold
/// as clap — see clap GH #4660 for why.
fn find_closest_match<'a>(input: &str, candidates: &[&'a str]) -> Option<&'a str> {
    candidates
        .iter()
        .map(|c| (*c, strsim::jaro(input, c)))
        .filter(|(_, score)| *score > 0.7)
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(name, _)| name)
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
    if let Some(project_aliases) = project_config.as_ref().and_then(|pc| pc.aliases.as_ref()) {
        append_aliases(&mut aliases, project_aliases);
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
        // Check for typos against both built-in commands and aliases
        let mut all_candidates: Vec<&str> = BUILTIN_STEP_COMMANDS.to_vec();
        // Only include non-shadowed aliases as candidates
        let available_aliases: Vec<_> = aliases
            .keys()
            .filter(|k| !BUILTIN_STEP_COMMANDS.contains(&k.as_str()))
            .map(|k| k.as_str())
            .collect();
        all_candidates.extend(&available_aliases);

        if let Some(closest) = find_closest_match(&opts.name, &all_candidates) {
            bail!(
                "{}",
                cformat!(
                    "Unknown step command <bold>{}</> — perhaps <bold>{closest}</>?",
                    opts.name,
                ),
            );
        }
        if available_aliases.is_empty() {
            bail!(
                "{}",
                cformat!(
                    "Unknown step command <bold>{}</> (no aliases configured)",
                    opts.name,
                ),
            );
        }
        bail!(
            "{}",
            cformat!(
                "Unknown alias <bold>{}</> (available: {})",
                opts.name,
                available_aliases.join(", "),
            ),
        );
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

    // Convert to &str references for expand_template
    let vars: HashMap<&str, &str> = context_map
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    // Build JSON context for stdin
    let context_json = serde_json::to_string(&context_map)
        .expect("HashMap<String, String> serialization should never fail");

    if opts.dry_run {
        let expanded: Vec<_> = cmd_config
            .commands()
            .map(|cmd| expand_template(&cmd.template, &vars, true, &repo, &opts.name))
            .collect::<Result<_, _>>()?;
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
        progress_message(cformat!("Running alias <bold>{}</>", opts.name))
    );

    // Pass the parent shell's directive file through so inner `wt` invocations
    // (e.g. `wt switch --create`) can write shell directives that the parent
    // shell wrapper will source after `wt` exits. The Cmd builder scrubs the
    // env var by default; `.directive_file()` re-adds it for trusted contexts.
    let parent_directive_file: Option<PathBuf> =
        std::env::var_os(DIRECTIVE_FILE_ENV_VAR).map(PathBuf::from);

    let exec = AliasExecCtx {
        vars: &vars,
        repo: &repo,
        alias_name: &opts.name,
        wt_path: &wt_path,
        context_json: &context_json,
        directive_file: parent_directive_file.as_deref(),
        is_pipeline: cmd_config.is_pipeline(),
    };

    for step in cmd_config.steps() {
        match step {
            HookStep::Single(cmd) => exec.run(cmd)?,
            HookStep::Concurrent(cmds) => {
                std::thread::scope(|s| {
                    let handles: Vec<_> =
                        cmds.iter().map(|cmd| s.spawn(|| exec.run(cmd))).collect();
                    wait_first_error(
                        handles
                            .into_iter()
                            .map(|h| h.join().expect("alias command thread panicked")),
                    )
                })?;
            }
        }
    }

    Ok(())
}

/// Shared state for executing alias commands within a pipeline.
struct AliasExecCtx<'a> {
    vars: &'a HashMap<&'a str, &'a str>,
    repo: &'a Repository,
    alias_name: &'a str,
    wt_path: &'a std::path::Path,
    context_json: &'a str,
    directive_file: Option<&'a std::path::Path>,
    is_pipeline: bool,
}

impl AliasExecCtx<'_> {
    /// Expand and execute a single alias command.
    ///
    /// In pipelines, templates referencing `vars.*` are deferred to execution
    /// time so that vars set by earlier steps are available.
    fn run(&self, cmd: &worktrunk::config::Command) -> anyhow::Result<()> {
        let command = if self.is_pipeline && template_references_var(&cmd.template, "vars") {
            let fresh_context: HashMap<String, String> = serde_json::from_str(self.context_json)
                .context("failed to deserialize context_json")?;
            expand_shell_template(&cmd.template, &fresh_context, self.repo, self.alias_name)?
        } else {
            expand_template(&cmd.template, self.vars, true, self.repo, self.alias_name)?
        };
        if let Err(err) = execute_shell_command(
            self.wt_path,
            &command,
            Some(self.context_json),
            None,
            self.directive_file,
        ) {
            if let Some(WorktrunkError::ChildProcessExited { code, .. }) =
                err.downcast_ref::<WorktrunkError>()
            {
                return Err(WorktrunkError::AlreadyDisplayed { exit_code: *code }.into());
            }
            bail!("Failed to run alias '{}': {}", self.alias_name, err);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> anyhow::Result<AliasOptions> {
        AliasOptions::parse(args.iter().map(|s| s.to_string()).collect())
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
    }

    #[test]
    fn test_parse_errors() {
        use insta::assert_snapshot;
        assert_snapshot!(parse(&[]).unwrap_err(), @"Missing alias name");
        assert_snapshot!(parse(&["deploy", "--var"]).unwrap_err(), @"--var requires a KEY=VALUE argument");
        assert_snapshot!(parse(&["deploy", "--var", "noequals"]).unwrap_err(), @"--var value must be KEY=VALUE");
        assert_snapshot!(parse(&["deploy", "--verbose"]).unwrap_err(), @"Unknown flag '--verbose' for alias 'deploy' (use --verbose=VALUE to pass a variable)");
        assert_snapshot!(parse(&["deploy", "arg1"]).unwrap_err(), @"Unexpected argument 'arg1' for alias 'deploy'");
        assert_snapshot!(parse(&["deploy", "--var", "=value"]).unwrap_err(), @"--var key must not be empty (got '=value')");
        assert_snapshot!(parse(&["deploy", "--=value"]).unwrap_err(), @"Variable name must not be empty (got '--=value')");
    }

    #[test]
    fn test_find_closest_match() {
        assert_eq!(
            find_closest_match("deplyo", &["deploy", "hello"]),
            Some("deploy")
        );
        assert_eq!(
            find_closest_match("comit", &["commit", "squash", "push", "rebase"]),
            Some("commit")
        );
        assert_eq!(find_closest_match("zzz", &["deploy", "hello"]), None);
        assert_eq!(find_closest_match("deploy", &[]), None);
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
}
