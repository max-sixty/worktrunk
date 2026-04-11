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
use std::path::Path;
use std::process::Stdio;

use color_print::cformat;
use worktrunk::config::{UserConfig, expand_template};
use worktrunk::git::Repository;
use worktrunk::git::WorktrunkError;
use worktrunk::shell_exec::ShellConfig;
use worktrunk::styling::{
    eprintln, error_message, format_with_gutter, progress_message, success_message, warning_message,
};

use crate::commands::command_executor::{CommandContext, build_hook_context};
use crate::commands::worktree_display_name;

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
    let worktrees: Vec<_> = repo
        .list_worktrees()?
        .into_iter()
        .filter(|wt| !wt.is_prunable())
        .collect();
    let config = UserConfig::load()?;

    let mut failed: Vec<String> = Vec::new();
    let mut json_results: Vec<serde_json::Value> = Vec::new();
    let total = worktrees.len();

    // Join args into a template string (will be expanded per-worktree)
    let command_template = args.join(" ");

    for wt in &worktrees {
        let display_name = worktree_display_name(wt, &repo, &config);
        eprintln!(
            "{}",
            progress_message(format!("Running in {display_name}..."))
        );

        // Build full hook context for this worktree
        // Pass wt.branch directly (not the display string) so detached HEAD maps to None -> "HEAD"
        let ctx = CommandContext::new(&repo, &config, wt.branch.as_deref(), &wt.path, false);
        let context_map = build_hook_context(&ctx, &[])?;

        // Convert to &str references for expand_template
        let vars: HashMap<&str, &str> = context_map
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        // Expand template with full context (shell-escaped)
        let command = expand_template(&command_template, &vars, true, &repo, "for-each command")?;

        // Build JSON context for stdin
        let context_json = serde_json::to_string(&context_map)
            .expect("HashMap<String, String> serialization should never fail");

        // Execute command: stream both stdout and stderr in real-time
        // Pipe context JSON to stdin for scripts that want structured data.
        // `for-each` scrubs the directive file env var (see `run_command_streaming`
        // docs) so commands run in each worktree cannot influence the parent
        // shell's working directory.
        match run_command_streaming(&command, &wt.path, Some(&context_json), None) {
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
                match &err {
                    CommandError::SpawnFailed(e) => {
                        eprintln!(
                            "{}",
                            error_message(cformat!(
                                "Failed in <bold>{display_name}</> (spawn failed)"
                            ))
                        );
                        eprintln!("{}", format_with_gutter(e, None));
                    }
                    CommandError::ExitCode(code) => {
                        let exit_info = code
                            .map(|c| format!(" (exit code {c})"))
                            .unwrap_or_default();
                        eprintln!(
                            "{}",
                            error_message(cformat!("Failed in <bold>{display_name}</>{exit_info}"))
                        );
                    }
                }
                failed.push(display_name.to_string());
                if json_mode {
                    json_results.push(serde_json::json!({
                        "branch": wt.branch,
                        "path": wt.path,
                        "exit_code": err.exit_code(),
                        "success": false,
                        "error": err.to_string(),
                    }));
                }
            }
        }
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

/// Error from running a command in a worktree
pub(crate) enum CommandError {
    /// Command failed to spawn (e.g., command not found, permission denied)
    SpawnFailed(String),
    /// Command exited with non-zero status
    ExitCode(Option<i32>),
}

impl CommandError {
    fn exit_code(&self) -> Option<i32> {
        match self {
            CommandError::SpawnFailed(_) => None,
            CommandError::ExitCode(code) => *code,
        }
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandError::SpawnFailed(e) => write!(f, "spawn failed: {e}"),
            CommandError::ExitCode(Some(c)) => write!(f, "exit code {c}"),
            CommandError::ExitCode(None) => write!(f, "killed by signal"),
        }
    }
}

/// Run a shell command, streaming both stdout and stderr in real-time.
///
/// Returns `Ok(())` on success, or `Err(CommandError)` on failure.
/// Both stdout and stderr stream to the terminal (stderr) in real-time.
/// If `stdin_content` is provided, it's piped to the command's stdin.
///
/// `directive_file` controls whether the child process can write shell
/// integration directives back to the parent shell:
///
/// - `None` — the `WORKTRUNK_DIRECTIVE_FILE` env var is removed from the
///   child's environment. Inner `wt` invocations will print the "shell
///   integration not installed" hint and drop any `cd` directives. This is
///   the default for sandboxed contexts like `wt step for-each`.
/// - `Some(path)` — the env var is set to `path`, so inner `wt` invocations
///   (and any child they spawn) can write directives that the parent shell
///   wrapper will source after `wt` exits. `wt step alias` uses this to let
///   aliases wrapping `wt switch --create` land the user in the new worktree.
///
/// # TODO: Streaming vs Gutter Tradeoff
///
/// Currently stderr streams directly without gutter formatting, same as hooks.
/// This means error output appears inline rather than in a visual gutter block.
/// Options to consider:
/// - Tee stderr (stream + capture) for gutter display on failure
/// - Add `--gutter` flag to capture and format output
/// - Accept current behavior as consistent with hooks
pub(crate) fn run_command_streaming(
    command: &str,
    working_dir: &Path,
    stdin_content: Option<&str>,
    directive_file: Option<&Path>,
) -> Result<(), CommandError> {
    let shell = ShellConfig::get().map_err(|e| CommandError::SpawnFailed(e.to_string()))?;

    log::debug!("$ {} (streaming)", command);

    let stdin_mode = if stdin_content.is_some() {
        Stdio::piped()
    } else {
        Stdio::inherit() // Allow interactive commands when no stdin content
    };

    let mut cmd = shell.command(command);
    cmd.current_dir(working_dir)
        .stdin(stdin_mode)
        // Redirect stdout to stderr to keep stdout reserved for data output
        // Note: Stdio::from(Stderr) works since Rust 1.74 (impl From<Stderr> for Stdio)
        .stdout(Stdio::from(std::io::stderr()))
        // Stream stderr to terminal in real-time
        .stderr(Stdio::inherit());

    match directive_file {
        // Propagate the parent's directive file so inner `wt` calls can write
        // shell integration directives (e.g. `wt switch --create` inside a
        // user alias body).
        Some(path) => {
            cmd.env(worktrunk::shell_exec::DIRECTIVE_FILE_ENV_VAR, path);
        }
        // Default: prevent subprocesses from writing to the directive file.
        None => {
            cmd.env_remove(worktrunk::shell_exec::DIRECTIVE_FILE_ENV_VAR);
        }
    }

    let mut child = cmd
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_error_display_and_exit_code() {
        let spawn = CommandError::SpawnFailed("no such file".into());
        assert_eq!(spawn.to_string(), "spawn failed: no such file");
        assert_eq!(spawn.exit_code(), None);

        let exit = CommandError::ExitCode(Some(42));
        assert_eq!(exit.to_string(), "exit code 42");
        assert_eq!(exit.exit_code(), Some(42));

        let signal = CommandError::ExitCode(None);
        assert_eq!(signal.to_string(), "killed by signal");
        assert_eq!(signal.exit_code(), None);
    }
}
