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
//! 4. Exit. The log file in `.git/wt/logs/` is the only artifact.
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
use std::io::Read as _;
use std::path::Path;
use std::process::{Child, Stdio};

use anyhow::{Context, bail};

use worktrunk::config::expand_template;
use worktrunk::git::Repository;
use worktrunk::shell_exec::ShellConfig;

use super::pipeline_spec::{PipelineSpec, PipelineStepSpec};

/// Run a serialized pipeline from stdin.
///
/// This is the entry point for `wt hook run-pipeline`.
/// The orchestrator is a long-lived background process spawned by
/// `spawn_detached_exec`; stdout/stderr are already redirected to a log file.
pub fn run_pipeline() -> anyhow::Result<()> {
    let mut contents = String::new();
    std::io::stdin()
        .read_to_string(&mut contents)
        .context("failed to read pipeline spec from stdin")?;

    let spec: PipelineSpec =
        serde_json::from_str(&contents).context("failed to deserialize pipeline spec")?;

    let repo =
        Repository::at(&spec.worktree_path).context("failed to open repository for pipeline")?;

    let context_json =
        serde_json::to_string(&spec.context).context("failed to serialize pipeline context")?;

    for step in &spec.steps {
        match step {
            PipelineStepSpec::Single { template, name } => {
                let expanded = expand_now(template, &spec, &repo, name.as_deref())?;
                let mut child = spawn_shell_command(&expanded, &spec.worktree_path, &context_json)?;
                let status = child.wait().context("failed to wait for child process")?;
                if !status.success() {
                    bail!(
                        "command failed with {}: {}",
                        format_exit(status.code()),
                        expanded,
                    );
                }
            }
            PipelineStepSpec::Concurrent { commands } => {
                run_concurrent_group(commands, &spec, &repo, &context_json)?;
            }
        }
    }

    Ok(())
}

/// Expand a template using the spec's context and fresh vars from git config.
fn expand_now(
    template: &str,
    spec: &PipelineSpec,
    repo: &Repository,
    name: Option<&str>,
) -> anyhow::Result<String> {
    let vars: HashMap<&str, &str> = spec
        .context
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let label = name.unwrap_or("pipeline step");
    // shell_escape=true — values are interpolated into a string passed to a shell,
    // so they must be escaped to prevent word splitting and metachar injection.
    Ok(expand_template(template, &vars, true, repo, label)?)
}

/// Spawn a shell command with context JSON piped to stdin.
///
/// Uses `ShellConfig` for portable shell detection (Git Bash on Windows,
/// `sh` on Unix). Returns the `Child` so the caller controls when to wait.
fn spawn_shell_command(
    expanded: &str,
    worktree_path: &Path,
    context_json: &str,
) -> anyhow::Result<Child> {
    let shell = ShellConfig::get()?;
    let mut child = shell
        .command(expanded)
        .current_dir(worktree_path)
        .stdin(Stdio::piped())
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
    context_json: &str,
) -> anyhow::Result<()> {
    let mut children = Vec::with_capacity(commands.len());

    for cmd in commands {
        let expanded = expand_now(&cmd.template, spec, repo, cmd.name.as_deref())?;
        let child = spawn_shell_command(&expanded, &spec.worktree_path, context_json)?;
        children.push((cmd.name.clone(), expanded, child));
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

fn format_exit(code: Option<i32>) -> String {
    code.map_or("signal".to_string(), |c| format!("exit code {c}"))
}
