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

use crate::cli::SwitchFormat;
use crate::commands::command_executor::{CommandContext, build_hook_context};

/// Template name reported in errors, the `-v` expansion view, and JSON output.
const EVAL_NAME: &str = "eval";

/// Evaluate a template expression in the current worktree context.
///
/// In text mode, prints the expanded result to stdout with a trailing newline.
/// In JSON mode (`--format=json`), prints `{name, template, result}` instead.
/// All hook template variables and filters are available.
///
/// `eval` mutates nothing, so it has no `--dry-run`. Variable discovery lives
/// in the verbose lane instead: `-v` lists the available template variables on
/// stderr, above the labeled `source` / `result` expansion view that
/// `expand_template` renders at `-v`. The `-v` lane writes to stderr, so it
/// composes with either output format.
pub fn step_eval(template: &str, format: SwitchFormat) -> anyhow::Result<()> {
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
        eprintln!(
            "{}",
            info_message(cformat!("<bold>{EVAL_NAME}</> template variables:"))
        );
        eprintln!("{}", format_with_gutter(&listing, None));
    }

    let vars: HashMap<&str, &str> = context_map
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    let result = expand_template(template, &vars, ShellEscapeMode::Literal, &repo, EVAL_NAME)?;
    // `expand_template` emitted the `source` / `result` view to stderr under
    // `-v`; a trailing blank separates it from the result printed below.
    if verbosity() >= 1 {
        eprintln!();
    }

    match format {
        SwitchFormat::Text => println!("{result}"),
        SwitchFormat::Json => {
            let payload = serde_json::json!({
                "name": EVAL_NAME,
                "template": template,
                "result": result,
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }
    Ok(())
}
