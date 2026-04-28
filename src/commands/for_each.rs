//! For-each command implementation
//!
//! Runs a command sequentially in each worktree with template expansion.
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

use color_print::cformat;
use worktrunk::config::UserConfig;
use worktrunk::git::{Repository, WorktreeInfo, WorktrunkError, interrupt_exit_code};
use worktrunk::styling::{
    eprintln, error_message, format_with_gutter, progress_message, success_message, warning_message,
};

use crate::commands::command_executor::{
    CommandContext, build_hook_context, expand_shell_template,
};
use crate::commands::worktree_display_name;
use crate::output::{DirectivePassthrough, execute_shell_command};

/// Run a command in each worktree sequentially.
///
/// Executes the given command in every worktree, streaming output
/// in real-time. Continues on errors and reports a summary at the end.
///
/// All template variables from hooks are available, and context JSON is piped to stdin.
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

    // Join args into a template string (will be expanded per-worktree)
    let command_template = args.join(" ");

    for &wt in &worktrees {
        let display_name = worktree_display_name(wt, &repo, &config);
        eprintln!(
            "{}",
            progress_message(format!("Running in {display_name}..."))
        );

        // Build full hook context for this worktree
        // Pass wt.branch directly (not the display string) so detached HEAD maps to None -> "HEAD"
        let ctx = CommandContext::new(&repo, &config, wt.branch.as_deref(), &wt.path, false);
        let context_map = build_hook_context(&ctx, &[])?;

        // Expand template with full context (shell-escaped)
        let command =
            expand_shell_template(&command_template, &context_map, &repo, "for-each command")?;

        // Build JSON context for stdin
        let context_json = serde_json::to_string(&context_map)
            .expect("HashMap<String, String> serialization should never fail");

        // Execute command: stream both stdout and stderr in real-time.
        // Pipe context JSON to stdin for scripts that want structured data.
        // Directive files are scrubbed — commands run in other worktrees should
        // not influence the parent shell's working directory.
        match execute_shell_command(
            &wt.path,
            &command,
            Some(&context_json),
            None,
            DirectivePassthrough::none(),
        ) {
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
