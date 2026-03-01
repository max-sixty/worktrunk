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
pub fn step_eval(template: &str) -> anyhow::Result<()> {
    let repo = Repository::current()?;
    let config = UserConfig::load()?;

    let wt = repo.current_worktree();
    let branch = wt.branch()?;
    let worktree_path = wt.root()?;

    let ctx = CommandContext::new(&repo, &config, branch.as_deref(), &worktree_path, false);
    let context_map = build_hook_context(&ctx, &[]);

    let vars: HashMap<&str, &str> = context_map
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    // No shell escaping — output is literal for scripting
    let result = expand_template(template, &vars, false, &repo, "eval")?;

    println!("{result}");
    Ok(())
}
