//! Pipeline runner for background hook execution.
//!
//! The parent `wt` process serializes a [`PipelineSpec`] to JSON and spawns
//! `wt hook run-pipeline` as a detached process (via `spawn_detached_exec`, which
//! pipes the JSON to stdin, redirects stdout/stderr to a log file, and puts
//! the process in its own process group). This module is that background
//! process.
//!
//! ## Lifecycle
//!
//! 1. Read and deserialize the spec from stdin.
//! 2. Open a [`Repository`] from the worktree path in the spec.
//! 3. Walk steps in order. For each step, expand templates and spawn shell
//!    children (see Execution model). Abort on the first serial step failure.
//! 4. Exit. Log files in `.git/wt/logs/` are the only artifacts.
//!
//! ## Execution model
//!
//! Each command — whether serial or concurrent — gets its own shell process
//! via [`ShellConfig`] (`sh` on Unix, Git Bash on Windows). Shell state
//! (`cd`, `export`, environment) does not carry across steps.
//!
//! **Serial steps** run one at a time. If a step exits non-zero, the
//! pipeline aborts — later steps don't run.
//!
//! **Concurrent groups** spawn all children at once, then wait for every
//! child before proceeding. If any child fails, the group is reported as
//! failed, but all children are allowed to finish. Template expansion for
//! concurrent commands happens sequentially before any child is spawned
//! (expansion may read git config, so order matters for `vars.*`).
//!
//! **Stdin**: every child receives the spec's context as JSON on stdin,
//! matching the foreground hook convention. Commands that don't read stdin
//! ignore it.
//!
//! ## Template freshness
//!
//! The spec carries two kinds of template input:
//!
//! - **Base context** (`branch`, `commit`, `worktree_path`, …) — snapshotted
//!   once when the parent builds the spec. A step that creates a new commit
//!   won't update `{{ commit }}` for later steps.
//!
//! - **`vars.*`** — read fresh from git config on every `expand_template`
//!   call. A step that runs `wt config state vars set key=val` makes
//!   `{{ vars.key }}` available to subsequent steps.
//!
//! This distinction exists because `vars.*` are the intended inter-step
//! communication channel (cheap git-config reads), while rebuilding the full
//! base context would spawn multiple git subprocesses per step.
//!
//! Template values are shell-escaped at expansion time (`shell_escape=true`)
//! since the expanded string is passed to a shell for interpretation.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::io::Read as _;
use std::path::Path;
use std::process::{Child, ExitStatus, Stdio};

use anyhow::Context;

use worktrunk::git::{Repository, WorktrunkError};
use worktrunk::shell_exec::ShellConfig;

use super::command_executor::{expand_shell_template, wait_first_error};
use super::pipeline_spec::{PipelineSpec, PipelineStepSpec};
use super::process::HookLog;

/// Run a serialized pipeline from stdin.
///
/// This is the entry point for `wt hook run-pipeline`.
/// The orchestrator is a long-lived background process spawned by
/// `spawn_detached_exec`; stdout/stderr are already redirected to a log file.
///
/// Each command's output is written to its own log file in `spec.log_dir`,
/// named `{branch}-{source}-{hook_type}-{name}.log`. The runner process's
/// own stdout/stderr captures only runner-level errors.
pub fn run_pipeline() -> anyhow::Result<()> {
    let mut contents = String::new();
    std::io::stdin()
        .read_to_string(&mut contents)
        .context("failed to read pipeline spec from stdin")?;

    let spec: PipelineSpec =
        serde_json::from_str(&contents).context("failed to deserialize pipeline spec")?;

    let repo =
        Repository::at(&spec.worktree_path).context("failed to open repository for pipeline")?;

    fs::create_dir_all(&spec.log_dir)
        .with_context(|| format!("failed to create log directory: {}", spec.log_dir.display()))?;

    let mut cmd_index = 0usize;

    for step in &spec.steps {
        match step {
            PipelineStepSpec::Single { template, name } => {
                let log_name = command_log_name(name.as_deref(), cmd_index);
                let log_file = create_command_log(&spec, &log_name)?;
                let step_ctx = step_context(&spec.context, name.as_deref());
                let label = name.as_deref().unwrap_or("pipeline step");
                let expanded = expand_shell_template(
                    template,
                    &step_ctx,
                    &repo,
                    label,
                    Some(worktrunk::config::ValidationScope::Hook(spec.hook_type)),
                )?;
                let step_json = serde_json::to_string(&*step_ctx)
                    .context("failed to serialize step context")?;
                let mut child =
                    spawn_shell_command(&expanded, &spec.worktree_path, &step_json, log_file)?;
                let status = child.wait().context("failed to wait for child process")?;
                if !status.success() {
                    return Err(failure_error(&status, &expanded));
                }
                cmd_index += 1;
            }
            PipelineStepSpec::Concurrent { commands } => {
                run_concurrent_group(commands, &spec, &repo, &mut cmd_index)?;
            }
        }
    }

    Ok(())
}

/// Build a per-step context, injecting `hook_name` when the step has a name.
///
/// The shared pipeline context has `hook_name` stripped (it varies per step).
/// Returns a `Cow` so unnamed steps borrow the base context without cloning.
fn step_context<'a>(
    base: &'a HashMap<String, String>,
    name: Option<&str>,
) -> Cow<'a, HashMap<String, String>> {
    match name {
        Some(n) => {
            let mut ctx = base.clone();
            ctx.insert("hook_name".into(), n.into());
            Cow::Owned(ctx)
        }
        None => Cow::Borrowed(base),
    }
}

/// Spawn a shell command with context JSON piped to stdin.
///
/// Uses `ShellConfig` for portable shell detection (Git Bash on Windows,
/// `sh` on Unix). stdout/stderr are redirected to `log_file` so each
/// command gets its own log. Returns the `Child` so the caller controls
/// when to wait.
fn spawn_shell_command(
    expanded: &str,
    worktree_path: &Path,
    context_json: &str,
    log_file: fs::File,
) -> anyhow::Result<Child> {
    let shell = ShellConfig::get()?;
    let log_err = log_file
        .try_clone()
        .context("failed to clone log file handle")?;
    let mut child = shell
        .command(expanded)
        .current_dir(worktree_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_err))
        .spawn()
        .with_context(|| format!("failed to spawn: {expanded}"))?;

    // Write context JSON to stdin, then drop to close the pipe.
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        // Ignore BrokenPipe — child may exit or close stdin early.
        let _ = stdin.write_all(context_json.as_bytes());
    }

    Ok(child)
}

/// Spawn all commands in a concurrent group, then wait for all.
///
/// Waits every spawned child before returning. If any failed, the first
/// failure (in spawn order) is returned, matching the serial-step bail
/// format. Per-command output already lives in each command's log file.
///
/// When `WORKTRUNK_TEST_SERIAL_CONCURRENT=1` is set, each command's child is
/// awaited before the next is spawned so output ordering is deterministic for
/// snapshot tests. The serial path bails on the first failure rather than
/// running every child to completion (the test hatch is for ordering, not
/// error semantics).
fn run_concurrent_group(
    commands: &[super::pipeline_spec::PipelineCommandSpec],
    spec: &PipelineSpec,
    repo: &Repository,
    cmd_index: &mut usize,
) -> anyhow::Result<()> {
    let serial = super::force_serial_concurrent();
    let mut children = Vec::with_capacity(if serial { 0 } else { commands.len() });

    for cmd in commands {
        let log_name = command_log_name(cmd.name.as_deref(), *cmd_index);
        let log_file = create_command_log(spec, &log_name)?;
        let cmd_ctx = step_context(&spec.context, cmd.name.as_deref());
        let label = cmd.name.as_deref().unwrap_or("pipeline step");
        let expanded = expand_shell_template(
            &cmd.template,
            &cmd_ctx,
            repo,
            label,
            Some(worktrunk::config::ValidationScope::Hook(spec.hook_type)),
        )?;
        let cmd_json =
            serde_json::to_string(&*cmd_ctx).context("failed to serialize step context")?;
        let mut child = spawn_shell_command(&expanded, &spec.worktree_path, &cmd_json, log_file)?;
        *cmd_index += 1;

        if serial {
            let status = child
                .wait()
                .with_context(|| format!("failed to wait for: {expanded}"))?;
            if !status.success() {
                return Err(failure_error(
                    &status,
                    cmd.name.as_deref().unwrap_or(&expanded),
                ));
            }
        } else {
            children.push((cmd.name.clone(), expanded, child));
        }
    }

    wait_first_error(children.into_iter().map(
        |(name, expanded, mut child)| -> anyhow::Result<()> {
            let status = child
                .wait()
                .with_context(|| format!("failed to wait for: {expanded}"))?;
            if !status.success() {
                return Err(failure_error(&status, name.as_deref().unwrap_or(&expanded)));
            }
            Ok(())
        },
    ))
}

/// Derive the log file name for a command.
///
/// Named commands use their name; unnamed commands use `cmd-{index}`.
fn command_log_name(name: Option<&str>, index: usize) -> String {
    match name {
        Some(n) => n.to_string(),
        None => format!("cmd-{index}"),
    }
}

/// Create a per-command log file in the spec's log directory.
///
/// Caller must ensure `spec.log_dir` exists (created once at pipeline startup).
fn create_command_log(spec: &PipelineSpec, name: &str) -> anyhow::Result<fs::File> {
    let hook_log = HookLog::hook(spec.source, spec.hook_type, name);
    let path = hook_log.path(&spec.log_dir, &spec.branch);
    fs::File::create(&path)
        .with_context(|| format!("failed to create log file: {}", path.display()))
}

/// Build the `anyhow::Error` for a failed pipeline step.
///
/// Signal-killed children surface as `WorktrunkError::ChildProcessExited`
/// with `signal: Some(sig)` and `code: 128 + sig`, matching the foreground
/// convention established by `shell_exec`. That lets `exit_code()` and
/// `interrupt_exit_code()` work consistently and the `wt hook run-pipeline`
/// process exits 130 on SIGINT and 143 on SIGTERM — the expectation the
/// "Signal Handling" section of the project `CLAUDE.md` sets for every
/// command loop.
///
/// Non-signal failures carry the child's exit code verbatim so log readers
/// (and any future observer of the background process) see the real code
/// instead of a generic `1`.
///
/// On non-Unix (`status.signal()` unavailable), the function falls through
/// to the exit-code path; `status.code()` is always `Some` on Windows.
fn failure_error(status: &ExitStatus, label: &str) -> anyhow::Error {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            let message = format!(
                "pipeline step terminated by {}: {label}",
                format_signal(sig)
            );
            return WorktrunkError::ChildProcessExited {
                code: 128 + sig,
                message,
                signal: Some(sig),
            }
            .into();
        }
    }
    let code = status.code().unwrap_or(1);
    let message = format!("command failed with exit code {code}: {label}");
    WorktrunkError::ChildProcessExited {
        code,
        message,
        signal: None,
    }
    .into()
}

/// Render a signal number as `signal N (SIGNAME)`, or `signal N` if nix
/// doesn't recognize it (platform-specific or real-time signals).
#[cfg(unix)]
fn format_signal(sig: i32) -> String {
    match nix::sys::signal::Signal::try_from(sig) {
        Ok(signal) => format!("signal {sig} ({signal})"),
        Err(_) => format!("signal {sig}"),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use worktrunk::git::interrupt_exit_code;

    fn downcast_child_exit(err: &anyhow::Error) -> (i32, Option<i32>, String) {
        match err.downcast_ref::<WorktrunkError>() {
            Some(WorktrunkError::ChildProcessExited {
                code,
                message,
                signal,
            }) => (*code, *signal, message.clone()),
            _ => panic!("expected ChildProcessExited, got {err:?}"),
        }
    }

    #[test]
    fn signal_exit_reports_named_signal_and_shell_exit_code() {
        let cases = [
            (
                15,
                143,
                "pipeline step terminated by signal 15 (SIGTERM): my-step",
            ),
            (
                2,
                130,
                "pipeline step terminated by signal 2 (SIGINT): my-step",
            ),
            (
                9,
                137,
                "pipeline step terminated by signal 9 (SIGKILL): my-step",
            ),
        ];
        for (sig, expected_code, expected_msg) in cases {
            let status = ExitStatus::from_raw(sig);
            let err = failure_error(&status, "my-step");
            let (code, signal, message) = downcast_child_exit(&err);
            assert_eq!(signal, Some(sig), "signal field for {sig}");
            assert_eq!(code, expected_code, "exit code for {sig}");
            assert_eq!(message, expected_msg, "message for {sig}");
            assert_eq!(
                interrupt_exit_code(&err),
                Some(expected_code),
                "interrupt_exit_code for {sig}",
            );
        }
    }

    #[test]
    fn non_signal_exit_preserves_child_code() {
        // Non-signal exit: raw value is (code << 8) on Unix.
        let status = ExitStatus::from_raw(2 << 8);
        let err = failure_error(&status, "my-step");
        let (code, signal, message) = downcast_child_exit(&err);
        assert_eq!(signal, None);
        assert_eq!(code, 2);
        assert_eq!(message, "command failed with exit code 2: my-step");
        // Non-signal errors must NOT trip the interrupt abort path.
        assert_eq!(interrupt_exit_code(&err), None);
    }
}
