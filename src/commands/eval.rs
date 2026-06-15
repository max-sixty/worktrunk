//! Eval command implementation
//!
//! Evaluates a template expression in the current worktree context and prints
//! the result to stdout.

use std::collections::HashMap;

use color_print::cformat;
use worktrunk::config::{UserConfig, expand_template};
use worktrunk::git::Repository;
use worktrunk::shell_exec::ShellEscapeMode;
use worktrunk::styling::{eprintln, format_with_gutter, info_message, println, verbosity};

use crate::commands::command_executor::{CommandContext, build_hook_context};

/// Evaluate a template expression in the current worktree context.
///
/// Prints the expanded result to stdout with a trailing newline. All hook
/// template variables and filters are available.
///
/// `eval` mutates nothing, so it has no `--dry-run`. Variable discovery lives
/// in the verbose lane instead: `-v` lists the available template variables on
/// stderr, above the `{{ template }} → result` expansion view that
/// `expand_template` renders at `-v`.
pub fn step_eval(template: &str) -> anyhow::Result<()> {
    let repo = Repository::current()?;
    let config = UserConfig::load()?;

    let wt = repo.current_worktree();
    let branch = wt.branch()?;
    let worktree_path = wt.root()?;

    let ctx = CommandContext::new(&repo, &config, branch.as_deref(), &worktree_path, false);
    let context_map = build_hook_context(&ctx, &[], None)?;

    if verbosity() >= 1 {
        let width = context_map.keys().map(String::len).max().unwrap_or(0);
        let mut keys: Vec<&str> = context_map.keys().map(String::as_str).collect();
        keys.sort();
        let listing = keys
            .iter()
            .map(|key| {
                let pad = " ".repeat(width - key.len());
                cformat!("<bold>{key}</>{pad} = {}", context_map[*key])
            })
            .collect::<Vec<_>>()
            .join("\n");
        eprintln!("{}", info_message("Available template variables"));
        eprintln!("{}", format_with_gutter(&listing, None));
    }

    let vars: HashMap<&str, &str> = context_map
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    let result = expand_template(template, &vars, ShellEscapeMode::Literal, &repo, "eval")?;
    println!("{result}");
    Ok(())
}
