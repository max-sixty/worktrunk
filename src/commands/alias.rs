//! Alias command implementation
//!
//! Runs user-defined command aliases configured in `[aliases]` sections
//! of user config or project config. Aliases are command templates that
//! support the same template variables as hooks.

use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, bail};
use color_print::cformat;
use worktrunk::config::{ProjectConfig, UserConfig, expand_template};
use worktrunk::git::{Repository, WorktrunkError};
use worktrunk::styling::{eprintln, format_with_gutter, info_message, progress_message};

use crate::commands::command_executor::{CommandContext, build_hook_context};
use crate::commands::for_each::{CommandError, run_command_streaming};

/// Options parsed from the external subcommand args.
#[derive(Debug)]
pub struct AliasOptions {
    pub name: String,
    pub dry_run: bool,
    pub vars: Vec<(String, String)>,
}

impl AliasOptions {
    /// Parse alias options from the external subcommand args.
    ///
    /// First element is the alias name, remaining are flags:
    /// `--dry-run` and `--var KEY=VALUE`.
    pub fn parse(args: Vec<String>) -> anyhow::Result<Self> {
        let Some(name) = args.first().cloned() else {
            bail!("Missing alias name");
        };

        let mut dry_run = false;
        let mut vars = Vec::new();
        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--dry-run" => dry_run = true,
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
                    bail!("Unknown flag '{other}' for alias '{name}'");
                }
            }
            i += 1;
        }

        Ok(Self {
            name,
            dry_run,
            vars,
        })
    }
}

fn parse_var(s: &str) -> anyhow::Result<(String, String)> {
    let (key, value) = s.split_once('=').context("--var value must be KEY=VALUE")?;
    Ok((key.to_string(), value.to_string()))
}

/// Run a configured alias by name.
///
/// Looks up the alias in merged config (project config + user config),
/// expands the template, and executes it.
pub fn step_alias(opts: AliasOptions) -> anyhow::Result<()> {
    let repo = Repository::current()?;
    let user_config = UserConfig::load()?;
    let project_id = repo.project_identifier().ok();
    let project_config = ProjectConfig::load(&repo, true)?;

    // Merge aliases: project config first, then user config overrides
    let mut aliases: BTreeMap<String, String> = project_config
        .as_ref()
        .and_then(|pc| pc.aliases.clone())
        .unwrap_or_default();
    aliases.extend(user_config.aliases(project_id.as_deref()));

    let Some(template) = aliases.get(&opts.name) else {
        if aliases.is_empty() {
            bail!(
                "Unknown step command '{}' (no aliases configured)",
                opts.name,
            );
        } else {
            let available: Vec<_> = aliases.keys().map(|k| k.as_str()).collect();
            bail!(
                "Unknown alias '{}' (available: {})",
                opts.name,
                available.join(", "),
            );
        }
    };

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

    let command = expand_template(template, &vars, true, &repo, &opts.name)?;

    if opts.dry_run {
        eprintln!(
            "{}",
            info_message(cformat!(
                "Alias <bold>{}</> would run:\n{}",
                opts.name,
                format_with_gutter(&command, None)
            ))
        );
        return Ok(());
    }

    eprintln!(
        "{}",
        progress_message(cformat!("Running alias <bold>{}</>", opts.name))
    );

    // Build JSON context for stdin
    let context_json = serde_json::to_string(&context_map)
        .expect("HashMap<String, String> serialization should never fail");

    match run_command_streaming(&command, &wt_path, Some(&context_json)) {
        Ok(()) => Ok(()),
        Err(CommandError::SpawnFailed(err)) => {
            bail!("Failed to run alias '{}': {}", opts.name, err);
        }
        Err(CommandError::ExitCode(exit_code)) => Err(WorktrunkError::AlreadyDisplayed {
            exit_code: exit_code.unwrap_or(1),
        }
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> anyhow::Result<AliasOptions> {
        AliasOptions::parse(args.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn test_parse_name_only() {
        let opts = parse(&["deploy"]).unwrap();
        assert_eq!(opts.name, "deploy");
        assert!(!opts.dry_run);
        assert!(opts.vars.is_empty());
    }

    #[test]
    fn test_parse_dry_run() {
        let opts = parse(&["deploy", "--dry-run"]).unwrap();
        assert!(opts.dry_run);
    }

    #[test]
    fn test_parse_var_separate() {
        let opts = parse(&["deploy", "--var", "key=value"]).unwrap();
        assert_eq!(opts.vars, vec![("key".into(), "value".into())]);
    }

    #[test]
    fn test_parse_var_equals() {
        let opts = parse(&["deploy", "--var=key=value"]).unwrap();
        assert_eq!(opts.vars, vec![("key".into(), "value".into())]);
    }

    #[test]
    fn test_parse_var_value_with_equals() {
        let opts = parse(&["deploy", "--var", "url=http://host?a=1"]).unwrap();
        assert_eq!(opts.vars[0], ("url".into(), "http://host?a=1".into()));
    }

    #[test]
    fn test_parse_multiple_vars() {
        let opts = parse(&["deploy", "--var", "a=1", "--var", "b=2", "--dry-run"]).unwrap();
        assert_eq!(opts.vars.len(), 2);
        assert!(opts.dry_run);
    }

    #[test]
    fn test_parse_missing_name() {
        let err = parse(&[]).unwrap_err();
        assert!(err.to_string().contains("Missing alias name"));
    }

    #[test]
    fn test_parse_var_missing_value() {
        let err = parse(&["deploy", "--var"]).unwrap_err();
        assert!(err.to_string().contains("--var requires a KEY=VALUE"));
    }

    #[test]
    fn test_parse_var_no_equals() {
        let err = parse(&["deploy", "--var", "noequals"]).unwrap_err();
        assert!(err.to_string().contains("KEY=VALUE"));
    }

    #[test]
    fn test_parse_unknown_flag() {
        let err = parse(&["deploy", "--verbose"]).unwrap_err();
        assert!(err.to_string().contains("Unknown flag '--verbose'"));
    }
}
