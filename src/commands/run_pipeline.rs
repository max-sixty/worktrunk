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
//! 4. On abort, record the failure for the next foreground `wt` to report
//!    (see [`super::hook_failure`]) — nobody is watching this process's stderr.
//! 5. Exit. Log files in `.git/wt/logs/` are the only artifacts.
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
//! **Concurrent groups** spawn each child as soon as its own template is
//! expanded, then wait for every child before proceeding. If any child fails,
//! the group is reported as failed, but all children are allowed to finish.
//! Expansion runs in a single sequential loop in command order — each command
//! is expanded immediately before its own child is spawned (expansion may read
//! git config, so order matters for `vars.*`), so a later command's expansion
//! can run after an earlier command's child has already started.
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
use std::fs;
use std::io::Read as _;
use std::path::Path;
use std::process::{Child, ExitStatus, Stdio};

use anyhow::Context;

use worktrunk::config::TemplateContext;
use worktrunk::git::{ErrorExt as _, Repository, WorktrunkError};
use worktrunk::shell_exec::{ShellConfig, scrub_git_discovery_env_vars};
use worktrunk::trace::CommandTrace;

use super::command_executor::{expand_shell_template, wait_first_error};
use super::hook_failure;
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

    for (step_index, step) in spec.steps.iter().enumerate() {
        if let Err(failure) = run_step(step, &spec, &repo, &mut cmd_index) {
            record_failure(&spec, &repo, step_index, &failure);
            return Err(failure.error);
        }
    }

    Ok(())
}

/// A pipeline abort, carrying what the deferred report needs to name the
/// command responsible.
///
/// Setup failures — template expansion, log creation, spawn — carry no exit
/// code, which is how the report tells "the step ran and exited 1" from "the
/// step never ran".
struct StepFailure {
    /// Display label of the command that aborted the pipeline: its hook name,
    /// or the command itself when the step is unnamed.
    label: String,
    /// Log-file stem for that command (`name`, or `cmd-{index}`), used to point
    /// the report at its output. `None` when no log file was created.
    log_name: Option<String>,
    error: anyhow::Error,
}

impl StepFailure {
    fn new(error: anyhow::Error, label: impl Into<String>, log_name: Option<&str>) -> Self {
        Self {
            label: label.into(),
            log_name: log_name.map(str::to_owned),
            error,
        }
    }
}

/// Run one pipeline step to completion.
///
/// Split out of [`run_pipeline`] so every exit from a step — including the
/// setup `?`s — funnels through one `Err` the caller can record before
/// propagating.
fn run_step(
    step: &PipelineStepSpec,
    spec: &PipelineSpec,
    repo: &Repository,
    cmd_index: &mut usize,
) -> Result<(), StepFailure> {
    match step {
        PipelineStepSpec::Single {
            template,
            template_name,
            name,
        } => {
            let log_name = command_log_name(name.as_deref(), *cmd_index);
            // Before the log file exists there is nothing to point at, so
            // setup failures up to that point report `log_name: None`.
            let label = name.as_deref().unwrap_or(template_name).to_string();
            let log_file = create_command_log(spec, &log_name)
                .map_err(|e| StepFailure::new(e, &label, None))?;
            let step_ctx = step_context(&spec.context, name.as_deref());
            let expanded = expand_shell_template(template, &step_ctx, repo, template_name)
                .map_err(|e| StepFailure::new(e, &label, Some(&log_name)))?;
            let label = name.as_deref().unwrap_or(&expanded).to_string();
            let step_json = step_ctx.to_json();
            let (mut child, mut trace) =
                spawn_shell_command(&expanded, &spec.worktree_path, &step_json, log_file)
                    .map_err(|e| StepFailure::new(e, &label, Some(&log_name)))?;
            let status = wait_resolving(&mut child, &mut trace, &expanded)
                .map_err(|e| StepFailure::new(e, &label, Some(&log_name)))?;
            if !status.success() {
                return Err(StepFailure::new(
                    failure_error(&status, &label),
                    &label,
                    Some(&log_name),
                ));
            }
            *cmd_index += 1;
            Ok(())
        }
        PipelineStepSpec::Concurrent { commands } => {
            run_concurrent_group(commands, spec, repo, cmd_index)
        }
    }
}

/// Record an aborted pipeline for the next foreground `wt` to report.
///
/// Nobody is watching this process's stderr — it is a log file — so without
/// this the abort and the steps it skipped are invisible until someone reads
/// `runner.log`. See [`super::hook_failure`].
fn record_failure(
    spec: &PipelineSpec,
    repo: &Repository,
    step_index: usize,
    failure: &StepFailure,
) {
    if is_cancellation(&failure.error) {
        return;
    }
    let skipped: Vec<String> = spec.steps[step_index + 1..]
        .iter()
        .flat_map(step_labels)
        .collect();
    let log = failure.log_name.as_ref().map(|name| {
        HookLog::hook(spec.source, spec.hook_type, name).path(&spec.log_dir, &spec.branch)
    });
    hook_failure::record(
        &repo.wt_dir(),
        &hook_failure::HookFailure {
            hook_type: spec.hook_type.to_string(),
            source: spec.source.to_string(),
            branch: spec.branch.clone(),
            failed: failure.label.clone(),
            exit: failure.error.exit_code(),
            skipped,
            log,
        },
    );
}

/// Whether the abort was a user-initiated cancellation rather than a hook
/// failure.
///
/// Only SIGINT and SIGTERM qualify — the two signals the project's Ctrl-C
/// policy treats as "the user asked for this", which exit quietly rather than
/// reporting. Any other signal (a crash, an OOM kill) is a real failure the
/// user should hear about, even though `interrupt_signal()` also names it.
#[cfg(unix)]
fn is_cancellation(error: &anyhow::Error) -> bool {
    use nix::sys::signal::Signal;
    matches!(
        error.interrupt_signal(),
        Some(sig) if sig == Signal::SIGINT as i32 || sig == Signal::SIGTERM as i32
    )
}

#[cfg(not(unix))]
fn is_cancellation(_error: &anyhow::Error) -> bool {
    false
}

/// Display labels for every command in a step, in order.
///
/// Used to name the steps an abort skipped. Unnamed commands (list-form hooks)
/// have no name to show, so the raw template stands in — that is what the user
/// wrote in their config.
fn step_labels(step: &PipelineStepSpec) -> Vec<String> {
    match step {
        PipelineStepSpec::Single { name, template, .. } => {
            vec![name.clone().unwrap_or_else(|| template.clone())]
        }
        PipelineStepSpec::Concurrent { commands } => commands
            .iter()
            .map(|c| c.name.clone().unwrap_or_else(|| c.template.clone()))
            .collect(),
    }
}

/// Build a per-step context, injecting `hook_name` when the step has a name.
///
/// The shared pipeline context has `hook_name` stripped (it varies per step).
/// Returns a `Cow` so unnamed steps borrow the base context without cloning.
fn step_context<'a>(base: &'a TemplateContext, name: Option<&str>) -> Cow<'a, TemplateContext> {
    match name {
        Some(n) => {
            let mut ctx = base.clone();
            ctx.insert("hook_name", n);
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
) -> anyhow::Result<(Child, CommandTrace)> {
    let shell = ShellConfig::get()?;
    let log_err = log_file
        .try_clone()
        .context("failed to clone log file handle")?;
    // Start the trace just before spawning; the caller resolves it once the
    // child is waited on (see `wait_resolving`). The step is fed its own
    // `context_json` on stdin, so mark it stdin-reading — the same command
    // across worktrees isn't a duplicate (different per-worktree input).
    let mut trace = CommandTrace::new(None, expanded).reads_stdin(true);
    let mut command = shell.command(expanded);
    command
        .current_dir(worktree_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_err));
    // Background hooks, like foreground ones, discover their repo from the
    // worktree cwd, not an inherited GIT_DIR/GIT_WORK_TREE (issue #3373). This
    // runner only ever executes hook pipelines, so the scrub is unconditional.
    scrub_git_discovery_env_vars(&mut command);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            trace.fail(&e);
            return Err(e).with_context(|| format!("failed to spawn: {expanded}"));
        }
    };

    // Write context JSON to stdin, then drop to close the pipe.
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        // Ignore BrokenPipe — child may exit or close stdin early.
        let _ = stdin.write_all(context_json.as_bytes());
    }

    Ok((child, trace))
}

/// Wait for a pipeline child, resolving its [`CommandTrace`], and surface a
/// wait-IO failure with context. Returns the child's exit status; the caller
/// decides whether a non-zero status is a pipeline failure.
fn wait_resolving(
    child: &mut Child,
    trace: &mut CommandTrace,
    expanded: &str,
) -> anyhow::Result<ExitStatus> {
    match child.wait() {
        Ok(status) => {
            trace.complete(status.success());
            Ok(status)
        }
        Err(e) => {
            trace.fail(&e);
            Err(e).with_context(|| format!("failed to wait for: {expanded}"))
        }
    }
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
) -> Result<(), StepFailure> {
    let serial = super::force_serial_concurrent();
    let mut children: Vec<(Option<String>, String, String, Child, CommandTrace)> =
        Vec::with_capacity(if serial { 0 } else { commands.len() });

    // Spawn (and, in serial mode, run) each command. Wrapped so that a mid-loop
    // error — a setup `?` or a spawn failure for a later command — tears down
    // the children already spawned this group rather than dropping them with
    // unresolved trace guards (and as unreaped orphans).
    let spawn_result = (|| -> Result<(), StepFailure> {
        for cmd in commands {
            let log_name = command_log_name(cmd.name.as_deref(), *cmd_index);
            let label = cmd
                .name
                .as_deref()
                .unwrap_or(&cmd.template_name)
                .to_string();
            let log_file = create_command_log(spec, &log_name)
                .map_err(|e| StepFailure::new(e, &label, None))?;
            let cmd_ctx = step_context(&spec.context, cmd.name.as_deref());
            let expanded = expand_shell_template(&cmd.template, &cmd_ctx, repo, &cmd.template_name)
                .map_err(|e| StepFailure::new(e, &label, Some(&log_name)))?;
            let label = cmd.name.as_deref().unwrap_or(&expanded).to_string();
            let cmd_json = cmd_ctx.to_json();
            let (mut child, mut trace) =
                spawn_shell_command(&expanded, &spec.worktree_path, &cmd_json, log_file)
                    .map_err(|e| StepFailure::new(e, &label, Some(&log_name)))?;
            *cmd_index += 1;

            if serial {
                let status = wait_resolving(&mut child, &mut trace, &expanded)
                    .map_err(|e| StepFailure::new(e, &label, Some(&log_name)))?;
                if !status.success() {
                    return Err(StepFailure::new(
                        failure_error(&status, &label),
                        &label,
                        Some(&log_name),
                    ));
                }
            } else {
                children.push((cmd.name.clone(), log_name, expanded, child, trace));
            }
        }
        Ok(())
    })();

    if let Err(e) = spawn_result {
        for (_, _, _, mut child, mut trace) in children {
            let _ = child.kill();
            let _ = child.wait();
            trace.complete(false);
        }
        return Err(e);
    }

    wait_first_error(children.into_iter().map(
        |(name, log_name, expanded, mut child, mut trace)| -> Result<(), StepFailure> {
            let label = name.as_deref().unwrap_or(&expanded).to_string();
            let status = wait_resolving(&mut child, &mut trace, &expanded)
                .map_err(|e| StepFailure::new(e, &label, Some(&log_name)))?;
            if !status.success() {
                return Err(StepFailure::new(
                    failure_error(&status, &label),
                    &label,
                    Some(&log_name),
                ));
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
/// `interrupt_signal()` work consistently and the `wt hook run-pipeline`
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
    use worktrunk::git::ErrorExt;

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
                err.interrupt_signal(),
                Some(sig),
                "interrupt_signal for {sig}"
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
        assert_eq!(err.interrupt_signal(), None);
    }
}
