//! For-each command implementation
//!
//! Runs a command sequentially in each worktree by direct exec — no implicit
//! shell. Each post-`--` argv element is template-expanded and passed through
//! to the program. Users wanting shell features (pipes, redirects, `$VAR`)
//! pass `sh -c '<snippet>'` explicitly.
//!
//! # Design Notes
//!
//! The `step` subcommand grouping is not fully satisfying. Current state:
//!
//! - `commit`, `squash`, `rebase`, `push` — merge workflow steps (single-worktree)
//! - `for-each` — utility to run commands across all worktrees (multi-worktree)
//!
//! These don't naturally belong together. Options considered:
//!
//! 1. **Top-level `wt for-each`** — more discoverable, but adds top-level commands
//! 2. **Rename `step` to `ops`** — clearer grouping, but breaking change
//! 3. **New `wt run` subcommand** — but unclear what stays in `step`
//! 4. **Keep current structure** — document the awkwardness (this option)
//!
//! Historical note: `hook` subcommands (pre-commit, post-merge, etc.) were originally
//! under `step` but were moved to their own `wt hook` subcommand for clarity.
//!
//! For now, we keep `for-each` under `step` as a pragmatic choice.

use std::collections::HashMap;
use std::io::{Write as _, stderr};
use std::process::Stdio;

use color_print::cformat;
use worktrunk::config::{UserConfig, expand_template};
use worktrunk::git::{Repository, WorktreeInfo, WorktrunkError, interrupt_exit_code};
use worktrunk::shell_exec::Cmd;
use worktrunk::styling::{
    eprintln, error_message, format_with_gutter, progress_message, success_message, warning_message,
};

use crate::commands::command_executor::{CommandContext, build_hook_context};
use crate::commands::worktree_display_name;

/// Run a command in each worktree sequentially.
///
/// Executes the given argv directly in every worktree, streaming output in
/// real-time. Continues on errors and reports a summary at the end.
///
/// All template variables from hooks are available; values are substituted
/// into argv elements without shell escaping. Context JSON is piped to stdin.
pub fn step_for_each(args: Vec<String>, format: crate::cli::SwitchFormat) -> anyhow::Result<()> {
    let json_mode = format == crate::cli::SwitchFormat::Json;
    let repo = Repository::current()?;
    // Filter out prunable worktrees (directory deleted) - can't run commands there
    let worktrees: Vec<&WorktreeInfo> = repo
        .list_worktrees()?
        .iter()
        .filter(|wt| !wt.is_prunable())
        .collect();
    let config = UserConfig::load()?;

    let mut failed: Vec<String> = Vec::new();
    let mut json_results: Vec<serde_json::Value> = Vec::new();
    // Set when a child dies from a signal (Ctrl-C / SIGTERM). We abort the
    // loop and propagate an equivalent exit code rather than visiting the
    // remaining worktrees — the user asked for the work to stop.
    let mut interrupted: Option<i32> = None;
    let total = worktrees.len();

    for &wt in &worktrees {
        let display_name = worktree_display_name(wt, &repo, &config);
        eprintln!(
            "{}",
            progress_message(format!("Running in {display_name}..."))
        );

        // Build full hook context for this worktree
        // Pass wt.branch directly (not the display string) so detached HEAD maps to None -> "HEAD"
        let ctx = CommandContext::new(&repo, &config, wt.branch.as_deref(), &wt.path, false);
        let context_map = build_hook_context(&ctx, &[], None)?;

        // Expand each argv element through the template engine without
        // shell-escaping — values are interpolated directly into the argv
        // element a program receives, not through `sh -c`.
        let vars: HashMap<&str, &str> = context_map
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let expanded: Vec<String> = args
            .iter()
            .map(|arg| expand_template(arg, &vars, false, &repo, "for-each argument"))
            .collect::<Result<_, _>>()?;

        // Build JSON context for stdin
        let context_json = serde_json::to_string(&context_map)
            .expect("HashMap<String, String> serialization should never fail");

        match run_argv(&wt.path, expanded, &context_json) {
            Ok(()) => {
                if json_mode {
                    json_results.push(serde_json::json!({
                        "branch": wt.branch,
                        "path": wt.path,
                        "exit_code": 0,
                        "success": true,
                    }));
                }
            }
            Err(err) => {
                let signal_exit = interrupt_exit_code(&err);
                let (exit_info, exit_code, error_msg, show_detail) =
                    if let Some(WorktrunkError::ChildProcessExited { code, message, .. }) =
                        err.downcast_ref::<WorktrunkError>()
                    {
                        (
                            format!(" (exit code {code})"),
                            serde_json::json!(code),
                            message.clone(),
                            false,
                        )
                    } else {
                        let msg = err.to_string();
                        (
                            " (spawn failed)".to_string(),
                            serde_json::json!(null),
                            msg,
                            true,
                        )
                    };
                eprintln!(
                    "{}",
                    error_message(cformat!("Failed in <bold>{display_name}</>{exit_info}"))
                );
                if show_detail {
                    eprintln!("{}", format_with_gutter(&error_msg, None));
                }
                failed.push(display_name.to_string());
                if json_mode {
                    json_results.push(serde_json::json!({
                        "branch": wt.branch,
                        "path": wt.path,
                        "exit_code": exit_code,
                        "success": false,
                        "error": error_msg,
                    }));
                }
                if let Some(code) = signal_exit {
                    interrupted = Some(code);
                    break;
                }
            }
        }
    }

    if let Some(exit_code) = interrupted {
        if json_mode {
            println!("{}", serde_json::to_string_pretty(&json_results)?);
        } else {
            eprintln!();
            eprintln!(
                "{}",
                warning_message("Interrupted — skipped remaining worktrees")
            );
        }
        return Err(WorktrunkError::AlreadyDisplayed { exit_code }.into());
    }

    if json_mode {
        println!("{}", serde_json::to_string_pretty(&json_results)?);
        if failed.is_empty() {
            return Ok(());
        } else {
            return Err(WorktrunkError::AlreadyDisplayed { exit_code: 1 }.into());
        }
    }

    // Summary
    eprintln!();
    if failed.is_empty() {
        eprintln!(
            "{}",
            success_message(format!(
                "Completed in {total} worktree{}",
                if total == 1 { "" } else { "s" }
            ))
        );
        Ok(())
    } else {
        eprintln!(
            "{}",
            warning_message(format!(
                "{} of {total} worktree{} failed",
                failed.len(),
                if total == 1 { "" } else { "s" }
            ))
        );
        let failed_list = failed.join("\n");
        eprintln!("{}", format_with_gutter(&failed_list, None));
        // Return silent error so main exits with code 1 without duplicate message
        Err(WorktrunkError::AlreadyDisplayed { exit_code: 1 }.into())
    }
}

/// Run argv directly (no shell) with streaming output, signal forwarding,
/// stdout→stderr redirect, and JSON context piped on stdin.
///
/// Mirrors the bookkeeping in `output::execute_shell_command` (flush, ANSI
/// reset, signal forwarding) but builds the command via `Cmd::new` so the
/// program is exec'd directly without `sh -c` interposition. Directive env
/// vars are scrubbed by `Cmd` for every spawn, so child commands run in
/// other worktrees can't perturb the parent shell's CD/exec state.
///
/// Child stdout is merged onto stderr so it interleaves cleanly with
/// for-each's decorated per-worktree headers and footers; the structured
/// `--format=json` output is the only stdout write, emitted once after all
/// children complete.
fn run_argv(
    working_dir: &std::path::Path,
    argv: Vec<String>,
    stdin_json: &str,
) -> anyhow::Result<()> {
    stderr().flush()?;
    eprint!("{}", anstyle::Reset);
    stderr().flush().ok();

    let mut iter = argv.into_iter();
    let program = iter
        .next()
        .expect("clap enforces at least one argv element");

    Cmd::new(program)
        .args(iter)
        .current_dir(working_dir)
        .stdout(Stdio::from(std::io::stderr()))
        .forward_signals()
        .stdin_bytes(stdin_json.as_bytes().to_vec())
        .stream()?;

    stderr().flush()?;
    Ok(())
}
