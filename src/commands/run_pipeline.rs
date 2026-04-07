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

use std::collections::HashMap;
use std::fs;
use std::io::Read as _;
use std::path::Path;
use std::process::{Child, Stdio};

use anyhow::{Context, bail};

use worktrunk::config::expand_template;
use worktrunk::git::Repository;
use worktrunk::shell_exec::ShellConfig;

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
                let expanded = expand_now(template, &spec, &repo, name.as_deref())?;
                let step_json = build_step_context_json(&spec.context, name.as_deref())?;
                let mut child =
                    spawn_shell_command(&expanded, &spec.worktree_path, &step_json, log_file)?;
                let status = child.wait().context("failed to wait for child process")?;
                if !status.success() {
                    bail!(
                        "command failed with {}: {}",
                        format_exit(status.code()),
                        expanded,
                    );
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

/// Expand a template using the spec's context and fresh vars from git config.
///
/// Injects per-step `hook_name` into the vars so each step sees its own name,
/// not the first step's name (the shared context has `hook_name` stripped).
fn expand_now(
    template: &str,
    spec: &PipelineSpec,
    repo: &Repository,
    name: Option<&str>,
) -> anyhow::Result<String> {
    let mut vars: HashMap<&str, &str> = spec
        .context
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    if let Some(n) = name {
        vars.insert("hook_name", n);
    }
    let label = name.unwrap_or("pipeline step");
    // shell_escape=true — values are interpolated into a string passed to a shell,
    // so they must be escaped to prevent word splitting and metachar injection.
    Ok(expand_template(template, &vars, true, repo, label)?)
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
fn run_concurrent_group(
    commands: &[super::pipeline_spec::PipelineCommandSpec],
    spec: &PipelineSpec,
    repo: &Repository,
    cmd_index: &mut usize,
) -> anyhow::Result<()> {
    let mut children = Vec::with_capacity(commands.len());

    for cmd in commands {
        let log_name = command_log_name(cmd.name.as_deref(), *cmd_index);
        let log_file = create_command_log(spec, &log_name)?;
        let expanded = expand_now(&cmd.template, spec, repo, cmd.name.as_deref())?;
        let cmd_json = build_step_context_json(&spec.context, cmd.name.as_deref())?;
        let child = spawn_shell_command(&expanded, &spec.worktree_path, &cmd_json, log_file)?;
        children.push((cmd.name.clone(), expanded, child));
        *cmd_index += 1;
    }

    let mut failures = Vec::new();
    for (name, expanded, mut child) in children {
        let status = child
            .wait()
            .with_context(|| format!("failed to wait for: {expanded}"))?;
        if !status.success() {
            let label = name.as_deref().unwrap_or(&expanded);
            failures.push(label.to_string());
        }
    }

    if !failures.is_empty() {
        bail!("concurrent group had failures: {}", failures.join(", "));
    }
    Ok(())
}

/// Build per-step context JSON, injecting `hook_name` when the step has a name.
///
/// The shared pipeline context has `hook_name` stripped (it varies per step).
/// This function adds it back for the specific step so commands receive the
/// correct `hook_name` on stdin.
fn build_step_context_json(
    base_context: &HashMap<String, String>,
    name: Option<&str>,
) -> anyhow::Result<String> {
    if let Some(n) = name {
        let mut ctx = base_context.clone();
        ctx.insert("hook_name".into(), n.into());
        serde_json::to_string(&ctx).context("failed to serialize step context")
    } else {
        serde_json::to_string(base_context).context("failed to serialize step context")
    }
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

fn format_exit(code: Option<i32>) -> String {
    code.map_or("signal".to_string(), |c| format!("exit code {c}"))
}
