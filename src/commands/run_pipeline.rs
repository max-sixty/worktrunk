//! Pipeline runner for background hook execution.
//!
//! The parent `wt` process serializes a [`PipelineSpec`] to JSON and spawns
//! `wt hook run-pipeline` as a detached process (via `spawn_detached_exec`, which
//! supplies the JSON on stdin, redirects stdout/stderr to a log file, and puts
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
//! **Concurrent groups** spawn each child as soon as its own template is
//! expanded, then wait for every child before proceeding. If any child fails,
//! the group is reported as failed, but all children are allowed to finish.
//! Expansion runs in a single sequential loop in command order — each command
//! is expanded immediately before its own child is spawned (expansion may read
//! git config, so order matters for `vars.*`), so a later command's expansion
//! can run after an earlier command's child has already started.
//!
//! **Stdin**: every child gets a closed stdin — this runner is detached, so
//! there is no terminal to hand over and nothing to read. The foreground path
//! inherits wt's stdin instead, so a step there can prompt (see
//! `execute_shell_command` in `output/handlers.rs`). Every template variable
//! reaches a step either way, through `{{ }}` expansion.
//!
//! ## Template freshness
//!
//! Each prepared command carries two kinds of template input:
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

use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus, Stdio};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use worktrunk::HookType;
use worktrunk::git::{Repository, WorktrunkError};
use worktrunk::shell_exec::{ShellConfig, scrub_git_discovery_env_vars};
use worktrunk::trace::CommandTrace;

use super::command_executor::{
    PreparedCommand, PreparedStep, expand_shell_template, hook_error_wrapper, wait_first_error,
};
use super::process::HookLog;
use worktrunk::config::HookSource;

/// Serialized specification for a background hook pipeline.
///
/// The envelope carries pipeline-wide execution metadata. Its steps use the
/// same prepared command model as foreground execution, so selection and
/// context preparation have one representation on both sides of the process
/// boundary.
#[derive(Serialize, Deserialize)]
pub(super) struct PipelineSpec {
    pub worktree_path: PathBuf,
    pub branch: String,
    pub hook_type: HookType,
    pub source: HookSource,
    pub steps: Vec<PreparedStep>,
}

/// Run a serialized pipeline from stdin.
///
/// This is the entry point for `wt hook run-pipeline`.
/// The orchestrator is a long-lived background process spawned by
/// `spawn_detached_exec`; stdout/stderr are already redirected to a log file.
///
/// Each command's output is written to its own repository log file,
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

    let log_dir = repo.wt_logs_dir();
    fs::create_dir_all(&log_dir)
        .with_context(|| format!("failed to create log directory: {}", log_dir.display()))?;

    let error_wrapper = hook_error_wrapper(spec.hook_type, spec.source, true);
    let mut cmd_index = 0usize;

    for step in &spec.steps {
        match step {
            PreparedStep::Single(cmd) => {
                let log_name = command_log_name(cmd.name.as_deref(), cmd_index);
                let log_file = create_command_log(&spec, &log_dir, &log_name)?;
                let expanded =
                    expand_shell_template(&cmd.template, &cmd.context, &repo, &cmd.template_name)?;
                let (mut child, mut trace) =
                    spawn_shell_command(&expanded, &spec.worktree_path, log_file)
                        .map_err(|error| error_wrapper(cmd, error))?;
                let status = wait_resolving(&mut child, &mut trace, &expanded)?;
                if !status.success() {
                    return Err(error_wrapper(
                        cmd,
                        WorktrunkError::from_child_status(&status, None).into(),
                    ));
                }
                cmd_index += 1;
            }
            PreparedStep::Concurrent(commands) => {
                run_concurrent_group(commands, &spec, &repo, &log_dir, &mut cmd_index)?;
            }
        }
    }

    Ok(())
}

/// Spawn a shell command with its output redirected to a log file.
///
/// Uses `ShellConfig` for portable shell detection (Git Bash on Windows,
/// `sh` on Unix). stdout/stderr are redirected to `log_file` so each
/// command gets its own log. Returns the `Child` so the caller controls
/// when to wait.
///
/// Stdin is closed: a detached step has no terminal, so a read returns EOF
/// rather than blocking on one that isn't there.
fn spawn_shell_command(
    expanded: &str,
    worktree_path: &Path,
    log_file: fs::File,
) -> anyhow::Result<(Child, CommandTrace)> {
    let shell = ShellConfig::get()?;
    let log_err = log_file
        .try_clone()
        .context("failed to clone log file handle")?;
    // Start the trace just before spawning; the caller resolves it once the
    // child is waited on (see `wait_resolving`).
    let mut trace = CommandTrace::new(None, expanded);
    let mut command = shell.command(expanded);
    command
        .current_dir(worktree_path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_err));
    // Background hooks, like foreground ones, discover their repo from the
    // worktree cwd, not an inherited GIT_DIR/GIT_WORK_TREE (issue #3373). This
    // runner only ever executes hook pipelines, so the scrub is unconditional.
    scrub_git_discovery_env_vars(&mut command);
    let child = match worktrunk::shell_exec::spawn(&mut command) {
        Ok(child) => child,
        Err(e) => {
            trace.fail(&e);
            return Err(e.into());
        }
    };

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
    commands: &[PreparedCommand],
    spec: &PipelineSpec,
    repo: &Repository,
    log_dir: &Path,
    cmd_index: &mut usize,
) -> anyhow::Result<()> {
    let error_wrapper = hook_error_wrapper(spec.hook_type, spec.source, true);
    let serial = super::force_serial_concurrent();
    let mut children = Vec::with_capacity(if serial { 0 } else { commands.len() });

    // Spawn (and, in serial mode, run) each command. Wrapped so that a mid-loop
    // setup or expansion error tears down
    // the children already spawned this group rather than dropping them with
    // unresolved trace guards (and as unreaped orphans).
    let spawn_result = (|| -> anyhow::Result<()> {
        for cmd in commands {
            let log_name = command_log_name(cmd.name.as_deref(), *cmd_index);
            let log_file = create_command_log(spec, log_dir, &log_name)?;
            let expanded =
                expand_shell_template(&cmd.template, &cmd.context, repo, &cmd.template_name)?;
            let spawned = spawn_shell_command(&expanded, &spec.worktree_path, log_file)
                .map_err(|error| error_wrapper(cmd, error));
            *cmd_index += 1;

            if serial {
                let (mut child, mut trace) = spawned?;
                let status = wait_resolving(&mut child, &mut trace, &expanded)?;
                if !status.success() {
                    return Err(error_wrapper(
                        cmd,
                        WorktrunkError::from_child_status(&status, None).into(),
                    ));
                }
            } else {
                children.push((cmd, expanded, spawned));
            }
        }
        Ok(())
    })();

    if let Err(e) = spawn_result {
        for (_, _, spawned) in children {
            if let Ok((mut child, mut trace)) = spawned {
                let _ = child.kill();
                let _ = child.wait();
                trace.complete(false);
            }
        }
        return Err(e);
    }

    wait_first_error(
        children
            .into_iter()
            .map(|(cmd, expanded, spawned)| -> anyhow::Result<()> {
                let (mut child, mut trace) = spawned?;
                let status = wait_resolving(&mut child, &mut trace, &expanded)?;
                if !status.success() {
                    return Err(error_wrapper(
                        cmd,
                        WorktrunkError::from_child_status(&status, None).into(),
                    ));
                }
                Ok(())
            }),
    )
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

/// Create a per-command log file in the repository's log directory.
///
/// Caller must ensure `log_dir` exists (created once at pipeline startup).
fn create_command_log(spec: &PipelineSpec, log_dir: &Path, name: &str) -> anyhow::Result<fs::File> {
    let hook_log = HookLog::hook(spec.source, spec.hook_type, name);
    let path = hook_log.path(log_dir, &spec.branch);
    fs::File::create(&path)
        .with_context(|| format!("failed to create log file: {}", path.display()))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use worktrunk::git::ErrorExt;

    fn failure_error(
        status: &ExitStatus,
        hook_type: HookType,
        name: Option<&str>,
    ) -> anyhow::Error {
        let cmd = PreparedCommand {
            name: name.map(str::to_owned),
            template: "exit 7".into(),
            context: Default::default(),
            template_name: "user hook".into(),
            label: name.map_or_else(|| "user".into(), |name| format!("user:{name}")),
        };
        hook_error_wrapper(hook_type, HookSource::User, true)(
            &cmd,
            WorktrunkError::from_child_status(status, None).into(),
        )
    }

    fn downcast_child_exit(err: &anyhow::Error) -> (i32, Option<i32>, String) {
        match err.chain().find_map(|error| {
            error
                .downcast_ref::<WorktrunkError>()
                .filter(|error| matches!(error, WorktrunkError::ChildProcessExited { .. }))
        }) {
            Some(WorktrunkError::ChildProcessExited {
                code,
                physical_signal,
                ..
            }) => (*code, *physical_signal, format!("{err:#}")),
            _ => panic!("expected ChildProcessExited, got {err:?}"),
        }
    }

    #[test]
    fn pipeline_spawn_failure_retains_working_directory_and_io_cause() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing-worktree");
        let log = fs::File::create(dir.path().join("command.log")).unwrap();
        let error = spawn_shell_command("true", &missing, log).err().unwrap();
        let cause = error.root_cause().downcast_ref::<std::io::Error>().unwrap();
        assert_eq!(cause.kind(), std::io::ErrorKind::NotFound);
        assert_eq!(cause.raw_os_error(), Some(2));
        assert_eq!(error.exit_code(), None);
        let detail = error.display_message();
        assert!(detail.contains(&worktrunk::path::format_path_for_display(&missing)));
        assert_eq!(detail.matches(&cause.to_string()).count(), 1, "{detail}");
    }

    #[test]
    fn signal_exit_reports_named_signal_and_shell_exit_code() {
        let cases = [
            (
                15,
                143,
                "pre-merge user:my-step failed: killed by signal 15",
            ),
            (2, 130, "pre-merge user:my-step failed: killed by signal 2"),
            (9, 137, "pre-merge user:my-step failed: killed by signal 9"),
        ];
        for (sig, expected_code, expected_msg) in cases {
            let status = ExitStatus::from_raw(sig);
            let err = failure_error(&status, HookType::PreMerge, Some("my-step"));
            let (code, signal, message) = downcast_child_exit(&err);
            assert_eq!(signal, Some(sig), "signal field for {sig}");
            assert_eq!(code, expected_code, "exit code for {sig}");
            assert_eq!(message, expected_msg, "message for {sig}");
            assert_eq!(
                err.interrupt_signal(),
                matches!(sig, 2 | 15).then_some(sig),
                "interrupt_signal for {sig}"
            );
        }
    }

    #[test]
    fn non_signal_exit_preserves_child_code() {
        // Non-signal exit: raw value is (code << 8) on Unix.
        let status = ExitStatus::from_raw(2 << 8);
        let err = failure_error(&status, HookType::PreMerge, Some("my-step"));
        let (code, signal, message) = downcast_child_exit(&err);
        assert_eq!(signal, None);
        assert_eq!(code, 2);
        assert_eq!(message, "pre-merge user:my-step failed: exit code 2");
        // Non-signal errors must NOT trip the interrupt abort path.
        assert_eq!(err.interrupt_signal(), None);
    }
}
