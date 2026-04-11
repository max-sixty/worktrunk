//! Alias command implementation
//!
//! Runs user-defined command aliases configured in `[aliases]` sections
//! of user config or project config. Aliases are command templates that
//! support the same template variables as hooks.
//!
//! Project-config aliases require command approval (same as project hooks).
//! User-config aliases are trusted and skip approval. When an alias exists
//! in both configs, both run — user first, then project (with approval).

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, bail};
use color_print::cformat;
use worktrunk::config::{
    CommandConfig, ProjectConfig, UserConfig, append_aliases, expand_template,
};
use worktrunk::git::{Repository, WorktrunkError};
use worktrunk::shell_exec::DIRECTIVE_FILE_ENV_VAR;
use worktrunk::styling::{
    eprintln, format_bash_with_gutter, info_message, progress_message, warning_message,
};

use crate::commands::command_approval::approve_alias_commands;
use crate::commands::command_executor::{CommandContext, build_hook_context};
use crate::commands::for_each::{CommandError, run_command_streaming};

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
    /// `--dry-run`, `--yes`/`-y`, and `--var KEY=VALUE`.
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

    let commands: Vec<_> = cmd_config.commands().collect();

    if opts.dry_run {
        let expanded: Vec<_> = commands
            .iter()
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

    // Pass the parent shell's directive file through to the alias subprocess
    // so inner `wt` invocations (e.g. `wt switch --create` inside an alias
    // body) can write shell integration directives like `cd '/path'` that the
    // parent shell wrapper will source after `wt` exits. Without this the
    // inner `wt` would see a scrubbed env var, print the "shell integration
    // not installed" hint, and drop the `cd`.
    //
    // This is a deliberate relaxation of the usual env scrub: aliases are
    // explicit, named, user-authorised commands (user-config aliases are
    // trusted; project-config aliases require approval), and an alias body is
    // already arbitrary shell that can `cd`/`rm`/`exec` anything locally, so
    // letting it ask the parent shell to `cd` is strictly less powerful than
    // what the body can already do.
    //
    // TODO: unify hook and alias execution so both pass the directive file
    // through. Hooks currently scrub it (see `process.rs` and the `None`
    // branch in `for_each::run_command_streaming`), so an inner `wt switch`
    // inside a hook body still drops its `cd`. Foreground `pre-*` hooks have
    // the same trust profile as aliases and could pass through too;
    // background `post-*` hooks outlive the parent shell, so any unification
    // needs to keep scrubbing in the detached spawn paths.
    let parent_directive_file: Option<PathBuf> =
        std::env::var_os(DIRECTIVE_FILE_ENV_VAR).map(PathBuf::from);

    for cmd in commands {
        let command = expand_template(&cmd.template, &vars, true, &repo, &opts.name)?;
        match run_command_streaming(
            &command,
            &wt_path,
            Some(&context_json),
            parent_directive_file.as_deref(),
        ) {
            Ok(()) => {}
            Err(CommandError::SpawnFailed(err)) => {
                bail!("Failed to run alias '{}': {}", opts.name, err);
            }
            Err(CommandError::ExitCode(exit_code)) => {
                return Err(WorktrunkError::AlreadyDisplayed {
                    exit_code: exit_code.unwrap_or(1),
                }
                .into());
            }
        }
    }

    Ok(())
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
    }

    #[test]
    fn test_parse_errors() {
        use insta::assert_snapshot;
        assert_snapshot!(parse(&[]).unwrap_err(), @"Missing alias name");
        assert_snapshot!(parse(&["deploy", "--var"]).unwrap_err(), @"--var requires a KEY=VALUE argument");
        assert_snapshot!(parse(&["deploy", "--var", "noequals"]).unwrap_err(), @"--var value must be KEY=VALUE");
        assert_snapshot!(parse(&["deploy", "--verbose"]).unwrap_err(), @"Unexpected argument '--verbose' for alias 'deploy'");
        assert_snapshot!(parse(&["deploy", "arg1"]).unwrap_err(), @"Unexpected argument 'arg1' for alias 'deploy'");
        assert_snapshot!(parse(&["deploy", "--var", "=value"]).unwrap_err(), @"--var key must not be empty (got '=value')");
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
