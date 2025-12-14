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

use std::collections::HashMap;
use std::io::Write;
use std::process::Stdio;

use color_print::cformat;
use worktrunk::config::{WorktrunkConfig, expand_template};
use worktrunk::git::Repository;
use worktrunk::git::WorktrunkError;
use worktrunk::shell_exec::ShellConfig;
use worktrunk::styling::{
    error_message, format_with_gutter, progress_message, success_message, warning_message,
};

use crate::commands::command_executor::{CommandContext, build_hook_context};
use crate::commands::worktree_display_name;
use crate::output;

/// Run a command in each worktree sequentially.
///
/// Executes the given command in every worktree, streaming output
/// in real-time. Continues on errors and reports a summary at the end.
///
/// All template variables from hooks are available, and context JSON is piped to stdin.
pub fn step_for_each(args: Vec<String>) -> anyhow::Result<()> {
    let repo = Repository::current();
    let worktrees = repo.list_worktrees()?;
    let config = WorktrunkConfig::load()?;

    let mut failed: Vec<String> = Vec::new();
    let total = worktrees.worktrees.len();

    // Join args into a template string (will be expanded per-worktree)
    let command_template = args.join(" ");

    // Get repo root for context
    let repo_root = repo.worktree_base()?;

    for wt in &worktrees.worktrees {
        let display_name = worktree_display_name(wt, &repo, &config);
        output::print(progress_message(format!("Running in {display_name}...")))?;

        // Open repository at worktree path to get worktree-specific context (commit, etc.)
        let wt_repo = Repository::at(&wt.path);

        // Build full hook context for this worktree
        // Pass wt.branch directly (not the display string) so detached HEAD maps to None -> "HEAD"
        let ctx = CommandContext::new(
            &wt_repo,
            &config,
            wt.branch.as_deref(),
            &wt.path,
            &repo_root,
            false,
        );
        let context_map = build_hook_context(&ctx, &[]);

        // Convert to &str references for expand_template
        let vars: HashMap<&str, &str> = context_map
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        // Expand template with full context (shell-escaped)
        let command = expand_template(&command_template, &vars, true)
            .map_err(|e| anyhow::anyhow!("Template expansion failed: {e}"))?;

        // Build JSON context for stdin
        let context_json = serde_json::to_string(&context_map)
            .expect("HashMap<String, String> serialization should never fail");

        // Flush output before running command to ensure message ordering
        output::flush()?;

        // Execute command: stream both stdout and stderr in real-time
        // Pipe context JSON to stdin for scripts that want structured data
        match run_command_streaming(&command, &wt.path, Some(&context_json)) {
            Ok(()) => {}
            Err(CommandError::SpawnFailed(err)) => {
                output::print(error_message(cformat!(
                    "Failed in <bold>{display_name}</> (spawn failed)"
                )))?;
                output::gutter(format_with_gutter(&err, "", None))?;
                failed.push(display_name.to_string());
            }
            Err(CommandError::ExitCode(exit_code)) => {
                // stderr already streamed to terminal; just show failure message
                let exit_info = exit_code
                    .map(|code| format!(" (exit code {code})"))
                    .unwrap_or_default();
                output::print(error_message(cformat!(
                    "Failed in <bold>{display_name}</>{exit_info}"
                )))?;
                failed.push(display_name.to_string());
            }
        }
    }

    // Summary
    output::blank()?;
    if failed.is_empty() {
        output::print(success_message(format!(
            "Completed in {total} worktree{}",
            if total == 1 { "" } else { "s" }
        )))?;
        Ok(())
    } else {
        output::print(warning_message(format!(
            "{} of {total} worktree{} failed",
            failed.len(),
            if total == 1 { "" } else { "s" }
        )))?;
        let failed_list = failed.join("\n");
        output::gutter(format_with_gutter(&failed_list, "", None))?;
        // Return silent error so main exits with code 1 without duplicate message
        Err(WorktrunkError::AlreadyDisplayed { exit_code: 1 }.into())
    }
}

/// Error from running a command in a worktree
enum CommandError {
    /// Command failed to spawn (e.g., command not found, permission denied)
    SpawnFailed(String),
    /// Command exited with non-zero status
    ExitCode(Option<i32>),
}

/// Run a shell command, streaming both stdout and stderr in real-time.
///
/// Returns `Ok(())` on success, or `Err(CommandError)` on failure.
/// Both stdout and stderr stream to the terminal (stderr) in real-time.
/// If `stdin_content` is provided, it's piped to the command's stdin.
///
/// # TODO: Streaming vs Gutter Tradeoff
///
/// Currently stderr streams directly without gutter formatting, same as hooks.
/// This means error output appears inline rather than in a visual gutter block.
/// Options to consider:
/// - Tee stderr (stream + capture) for gutter display on failure
/// - Add `--gutter` flag to capture and format output
/// - Accept current behavior as consistent with hooks
fn run_command_streaming(
    command: &str,
    working_dir: &std::path::Path,
    stdin_content: Option<&str>,
) -> Result<(), CommandError> {
    let shell = ShellConfig::get();

    let stdin_mode = if stdin_content.is_some() {
        Stdio::piped()
    } else {
        Stdio::inherit() // Allow interactive commands when no stdin content
    };

    let mut child = shell
        .command(command)
        .current_dir(working_dir)
        .stdin(stdin_mode)
        // Redirect stdout to stderr to keep stdout clean for directive scripts
        // Note: Stdio::from(Stderr) works since Rust 1.74 (impl From<Stderr> for Stdio)
        .stdout(Stdio::from(std::io::stderr()))
        // Stream stderr to terminal in real-time
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| CommandError::SpawnFailed(e.to_string()))?;

    // Write stdin content if provided (JSON context for scripts)
    if let Some(content) = stdin_content
        && let Some(mut stdin) = child.stdin.take()
    {
        // Ignore write errors - command may not read stdin
        let _ = stdin.write_all(content.as_bytes());
        // stdin is dropped here, closing the pipe
    }

    let status = child
        .wait()
        .map_err(|e| CommandError::SpawnFailed(e.to_string()))?;

    if status.success() {
        Ok(())
    } else {
        Err(CommandError::ExitCode(status.code()))
    }
}
