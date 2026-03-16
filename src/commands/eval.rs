//! Eval command implementation
//!
//! Evaluates a template expression in the current worktree context and prints
//! the result to stdout. Designed for scripting — output is raw (no shell
//! escaping, no decoration).

use std::collections::HashMap;

use worktrunk::config::{UserConfig, expand_template};
use worktrunk::git::Repository;

use crate::commands::command_executor::{CommandContext, build_hook_context};

/// Evaluate a template expression in the current worktree context.
///
/// Prints the expanded result to stdout with a trailing newline. All hook
/// template variables and filters are available.
///
/// With `dry_run`, prints the template variables and the expanded result
/// With `dry_run`, prints the template variables and the expanded result
/// to stderr — useful for debugging templates.
pub fn step_eval(template: &str, dry_run: bool) -> anyhow::Result<()> {
    let repo = Repository::current()?;
    let config = UserConfig::load()?;

    let wt = repo.current_worktree();
    let branch = wt.branch()?;
    let worktree_path = wt.root()?;

    let ctx = CommandContext::new(&repo, &config, branch.as_deref(), &worktree_path, false);
    let context_map = build_hook_context(&ctx, &[])?;

    let vars: HashMap<&str, &str> = context_map
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    // No shell escaping — output is literal for scripting
    let result = expand_template(template, &vars, false, &repo, "eval")?;

    if dry_run {
        let mut keys: Vec<&str> = context_map.keys().map(|k| k.as_str()).collect();
        keys.sort();
        for key in keys {
            eprintln!("{}={}", key, context_map[key]);
        }
        eprintln!("---");
        eprintln!("Result: {result}");
    } else {
        println!("{result}");
    }
    Ok(())
}
