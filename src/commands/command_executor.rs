use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use color_print::cformat;
use worktrunk::HookType;
use worktrunk::config::{
    Command, CommandConfig, HookStep, UserConfig, expand_template, template_references_var,
    validate_template_syntax,
};
use worktrunk::git::{Repository, WorktrunkError, interrupt_exit_code};
use worktrunk::path::{format_path_for_display, to_posix_path};
use worktrunk::styling::{eprintln, error_message, format_bash_with_gutter, progress_message};

use super::format_command_label;
use super::hook_filter::HookSource;
use crate::output::concurrent::{ConcurrentCommand, run_concurrent_commands};
use crate::output::{DirectivePassthrough, execute_shell_command};

#[derive(Debug)]
pub struct PreparedCommand {
    pub name: Option<String>,
    pub expanded: String,
    pub context_json: String,
    /// Raw template for lazy expansion at execution time (when template references `vars.`).
    /// When `Some`, the `expanded` field is a placeholder — use `lazy_template` instead.
    pub lazy_template: Option<String>,
}

/// A step in a prepared pipeline, mirroring `HookStep`.
#[derive(Debug)]
pub enum PreparedStep {
    Single(PreparedCommand),
    Concurrent(Vec<PreparedCommand>),
}

impl PreparedStep {
    /// Flatten into a vec of commands (Single becomes a one-element vec).
    pub fn into_commands(self) -> Vec<PreparedCommand> {
        match self {
            Self::Single(cmd) => vec![cmd],
            Self::Concurrent(cmds) => cmds,
        }
    }
}

/// Where a foreground command originated — determines announcement format,
/// error wrapping, and log label.
#[derive(Clone, Debug)]
pub enum CommandOrigin {
    /// Hook command with source attribution.
    Hook {
        source: HookSource,
        hook_type: HookType,
        /// Path shown in announcement when commands run in a different directory
        /// than where the user invoked the command.
        display_path: Option<PathBuf>,
    },
    /// Alias command.
    Alias { name: String },
}

/// A pipeline step ready for foreground execution, with origin metadata.
pub struct ForegroundStep {
    pub step: PreparedStep,
    pub origin: CommandOrigin,
    /// Whether `Concurrent` steps actually run concurrently. When `false`,
    /// concurrent commands execute serially (deprecated pre-* table form).
    pub concurrent: bool,
}

/// Controls how foreground execution responds to command failures.
#[derive(Clone, Copy)]
pub enum FailureStrategy {
    /// Stop on first failure and surface the error to the caller.
    FailFast,
    /// Log warnings and continue executing remaining commands.
    Warn,
}

#[derive(Clone, Copy, Debug)]
pub struct CommandContext<'a> {
    pub repo: &'a Repository,
    pub config: &'a UserConfig,
    /// Current branch name, if on a branch (None in detached HEAD state).
    pub branch: Option<&'a str>,
    pub worktree_path: &'a Path,
    pub yes: bool,
}

impl<'a> CommandContext<'a> {
    pub fn new(
        repo: &'a Repository,
        config: &'a UserConfig,
        branch: Option<&'a str>,
        worktree_path: &'a Path,
        yes: bool,
    ) -> Self {
        Self {
            repo,
            config,
            branch,
            worktree_path,
            yes,
        }
    }

    /// Get branch name, using "HEAD" as fallback for detached HEAD state.
    pub fn branch_or_head(&self) -> &str {
        self.branch.unwrap_or("HEAD")
    }

    /// Get the project identifier for per-project config lookup.
    ///
    /// Uses the remote URL if available, otherwise the canonical repository path.
    /// Returns None only if the path is not valid UTF-8.
    pub fn project_id(&self) -> Option<String> {
        self.repo.project_identifier().ok()
    }

    /// Get the commit generation config, merging project-specific settings.
    pub fn commit_generation(&self) -> worktrunk::config::CommitGenerationConfig {
        self.config.commit_generation(self.project_id().as_deref())
    }
}

/// Build hook context as a HashMap for JSON serialization and template expansion.
///
/// The resulting HashMap is passed to hook commands as JSON on stdin,
/// and used directly for template variable expansion.
pub fn build_hook_context(
    ctx: &CommandContext<'_>,
    extra_vars: &[(&str, &str)],
) -> Result<HashMap<String, String>> {
    let repo_root = ctx.repo.repo_path()?;
    let repo_name = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    // Convert paths to POSIX format for Git Bash compatibility on Windows.
    // This avoids shell escaping of `:` and `\` characters in Windows paths.
    let worktree = to_posix_path(&ctx.worktree_path.to_string_lossy());
    let worktree_name = ctx
        .worktree_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    let repo_path = to_posix_path(&repo_root.to_string_lossy());

    let mut map = HashMap::new();
    map.insert("repo".into(), repo_name.into());
    map.insert("branch".into(), ctx.branch_or_head().into());
    map.insert("worktree_name".into(), worktree_name.into());

    // Canonical path variables
    map.insert("repo_path".into(), repo_path.clone());
    map.insert("worktree_path".into(), worktree.clone());

    // Deprecated aliases (kept for backward compatibility)
    map.insert("main_worktree".into(), repo_name.into());
    map.insert("repo_root".into(), repo_path);
    map.insert("worktree".into(), worktree);

    if let Some(parsed_remote) = ctx.repo.primary_remote_parsed_url() {
        map.insert("owner".into(), parsed_remote.owner().to_string());
    }

    // Default branch
    if let Some(default_branch) = ctx.repo.default_branch() {
        map.insert("default_branch".into(), default_branch);
    }

    // Primary worktree path (where established files live)
    if let Ok(Some(path)) = ctx.repo.primary_worktree() {
        let path_str = to_posix_path(&path.to_string_lossy());
        map.insert("primary_worktree_path".into(), path_str.clone());
        // Deprecated alias
        map.insert("main_worktree_path".into(), path_str);
    }

    // Resolve commit from the Active branch, not HEAD at discovery path.
    // This ensures {{ commit }} follows the Active branch even when the
    // CommandContext points to a different worktree than where we're running.
    let commit_ref = ctx.branch.unwrap_or("HEAD");
    if let Ok(commit) = ctx.repo.run_command(&["rev-parse", commit_ref]) {
        let commit = commit.trim();
        map.insert("commit".into(), commit.into());
        if commit.len() >= 7 {
            map.insert("short_commit".into(), commit[..7].into());
        }
    }

    if let Ok(remote) = ctx.repo.primary_remote() {
        map.insert("remote".into(), remote.to_string());
        // Add remote URL for conditional hook execution (e.g., GitLab vs GitHub)
        if let Some(url) = ctx.repo.remote_url(&remote) {
            map.insert("remote_url".into(), url);
        }
        if let Some(branch) = ctx.branch
            && let Ok(Some(upstream)) = ctx.repo.branch(branch).upstream()
        {
            map.insert("upstream".into(), upstream);
        }
    }

    // Execution directory — always where the hook command runs, even when
    // worktree_path points to an Active identity that doesn't exist on disk.
    map.insert(
        "cwd".into(),
        to_posix_path(&ctx.worktree_path.to_string_lossy()),
    );

    // Add extra vars (e.g., target branch for merge, base for switch)
    for (k, v) in extra_vars {
        map.insert((*k).into(), (*v).into());
    }

    Ok(map)
}

/// Drain a sequence of command results, returning the first error.
///
/// All items are consumed before returning, so callers can be sure every
/// spawned child or joined thread has completed even when one item already
/// errored. Used by alias and pipeline concurrent groups, which both want
/// "wait all, return first error" semantics around different concurrency
/// primitives (in-process threads vs OS subprocesses).
pub fn wait_first_error<E>(
    results: impl IntoIterator<Item = std::result::Result<(), E>>,
) -> std::result::Result<(), E> {
    let mut first = None;
    for r in results {
        if let Err(e) = r
            && first.is_none()
        {
            first = Some(e);
        }
    }
    first.map_or(Ok(()), Err)
}

/// Expand a shell-command template against a context map.
///
/// Builds the `&str` vars map required by `expand_template` and fixes
/// `shell_escape=true` since every caller interpolates the result into a
/// shell string. Used by the three execution paths — foreground hooks,
/// background pipelines, and aliases — that defer `vars.*` expansion until
/// just before the command runs so prior steps can set vars via git config.
pub fn expand_shell_template(
    template: &str,
    context: &HashMap<String, String>,
    repo: &Repository,
    label: &str,
) -> Result<String> {
    let vars: HashMap<&str, &str> = context
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    Ok(expand_template(template, &vars, true, repo, label)?)
}

/// Short summary name: "user:name" for named commands, "user" otherwise.
pub(crate) fn command_summary_name(name: Option<&str>, source: HookSource) -> String {
    match name {
        Some(n) => format!("{source}:{n}"),
        None => source.to_string(),
    }
}

/// Execute a pipeline of prepared steps in the foreground.
///
/// This is the canonical foreground execution path for both hooks and aliases.
/// Handles serial/concurrent step execution, per-command announcement, lazy
/// template resolution, and origin-aware error handling.
///
/// Each `ForegroundStep` carries a `concurrent` flag. When true, `Concurrent`
/// steps spawn threads via `thread::scope`. When false (deprecated pre-*
/// single-table form), `Concurrent` steps execute serially. Pipeline configs
/// (`[[hook]]` blocks), aliases, and post-* hooks set `concurrent: true`.
///
/// TODO(unify-hook-alias): this function centralized dispatch but left four
/// per-origin branch points. Follow-ups to collapse them:
///   1. Unify the error type: one `CommandFailed { origin_label, exit_code,
///      message }` lets `handle_command_error` drop its origin match.
///   2. Unify announcement policy (decide per-command vs single-summary for
///      both); `announce_command`'s dispatch disappears.
///   3. Push log/expansion labels onto `PreparedCommand` at prep time so
///      `command_log_label` / `expansion_label` go away.
///   4. Longer term: treat hooks as aliases bound to triggers so `hooks.rs`
///      becomes a trigger-dispatcher + config loader over the alias machinery.
pub fn execute_pipeline_foreground(
    steps: &[ForegroundStep],
    repo: &Repository,
    wt_path: &Path,
    directives: &DirectivePassthrough,
    failure_strategy: FailureStrategy,
) -> anyhow::Result<()> {
    for fg_step in steps {
        match &fg_step.step {
            PreparedStep::Single(cmd) => {
                run_one_command(
                    cmd,
                    &fg_step.origin,
                    repo,
                    wt_path,
                    directives,
                    failure_strategy,
                )?;
            }
            PreparedStep::Concurrent(cmds) => {
                if !fg_step.concurrent {
                    for cmd in cmds {
                        run_one_command(
                            cmd,
                            &fg_step.origin,
                            repo,
                            wt_path,
                            directives,
                            failure_strategy,
                        )?;
                    }
                } else {
                    run_concurrent_group(
                        cmds,
                        &fg_step.origin,
                        repo,
                        wt_path,
                        directives,
                        failure_strategy,
                    )?;
                }
            }
        }
    }
    Ok(())
}

/// Run every command in a concurrent group via the prefixed-line executor.
///
/// Announces each command up front (origin-aware — hooks render per-command
/// announcements, aliases only announce the outer group), expands all
/// templates sequentially (template expansion reads git config; racing on
/// reads would produce inconsistent state), then dispatches to
/// `run_concurrent_commands` which streams each child's output prefixed by
/// its label and waits for all to complete before folding outcomes.
fn run_concurrent_group(
    cmds: &[PreparedCommand],
    origin: &CommandOrigin,
    repo: &Repository,
    wt_path: &Path,
    directives: &DirectivePassthrough,
    failure_strategy: FailureStrategy,
) -> anyhow::Result<()> {
    for cmd in cmds {
        announce_command(cmd, origin);
    }

    // Commands with `lazy_template` are re-expanded at execution time so
    // they see fresh git-config state (e.g., `vars.*` set by earlier steps).
    // Commands without it were already expanded at prep time — use `expanded`.
    let mut expanded: Vec<String> = Vec::with_capacity(cmds.len());
    for cmd in cmds {
        if let Some(template) = &cmd.lazy_template {
            let label = expansion_label(cmd, origin);
            let context: HashMap<String, String> = serde_json::from_str(&cmd.context_json)
                .context("failed to deserialize context_json")?;
            expanded.push(expand_shell_template(template, &context, repo, &label)?);
        } else {
            expanded.push(cmd.expanded.clone());
        }
    }

    let log_labels: Vec<Option<String>> = cmds
        .iter()
        .map(|cmd| command_log_label(cmd, origin))
        .collect();

    // Both alias tables and hook tables produce named commands (TOML keys
    // become `name`), so `cmd.name` is always `Some` here.
    let labels: Vec<&str> = cmds
        .iter()
        .map(|cmd| {
            cmd.name
                .as_deref()
                .expect("concurrent group commands are always named")
        })
        .collect();

    let specs: Vec<ConcurrentCommand<'_>> = cmds
        .iter()
        .enumerate()
        .map(|(i, cmd)| ConcurrentCommand {
            label: labels[i],
            expanded: &expanded[i],
            working_dir: wt_path,
            context_json: &cmd.context_json,
            log_label: log_labels[i].as_deref(),
            directives,
        })
        .collect();

    let outcomes = run_concurrent_commands(&specs)?;

    let mut first_failure: Option<anyhow::Error> = None;
    for (outcome, cmd) in outcomes.into_iter().zip(cmds) {
        let Err(err) = outcome else { continue };
        match handle_command_error(err, cmd, origin, failure_strategy) {
            Ok(()) => {}
            Err(e) => {
                if first_failure.is_none() {
                    first_failure = Some(e);
                }
            }
        }
    }
    match first_failure {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// Execute a single prepared command: announce, expand, run, handle errors.
fn run_one_command(
    cmd: &PreparedCommand,
    origin: &CommandOrigin,
    repo: &Repository,
    wt_path: &Path,
    directives: &DirectivePassthrough,
    failure_strategy: FailureStrategy,
) -> anyhow::Result<()> {
    announce_command(cmd, origin);

    let lazy_expanded;
    let command_str = if let Some(template) = &cmd.lazy_template {
        let label = expansion_label(cmd, origin);
        let context: HashMap<String, String> = serde_json::from_str(&cmd.context_json)
            .context("failed to deserialize context_json")?;
        lazy_expanded = expand_shell_template(template, &context, repo, &label)?;
        &lazy_expanded
    } else {
        &cmd.expanded
    };

    let log_label = command_log_label(cmd, origin);
    let result = execute_shell_command(
        wt_path,
        command_str,
        Some(&cmd.context_json),
        log_label.as_deref(),
        directives.clone(),
    );

    match result {
        Ok(()) => Ok(()),
        Err(err) => handle_command_error(err, cmd, origin, failure_strategy),
    }
}

/// Announce a command before execution, formatted per origin.
///
/// Hooks get per-command announcements with the expanded command in a gutter.
/// Aliases show a single summary line before the pipeline (in the caller),
/// so no per-command announcement here.
fn announce_command(cmd: &PreparedCommand, origin: &CommandOrigin) {
    match origin {
        CommandOrigin::Hook {
            source,
            hook_type,
            display_path,
        } => {
            let summary = command_summary_name(cmd.name.as_deref(), *source);
            let full_label = match &cmd.name {
                Some(_) => format_command_label(&hook_type.to_string(), Some(&summary)),
                None => format!("Running {hook_type} {summary} hook"),
            };
            let message = match display_path.as_deref() {
                Some(path) => {
                    let path_display = format_path_for_display(path);
                    cformat!("{full_label} @ <bold>{path_display}</>")
                }
                None => full_label,
            };
            eprintln!("{}", progress_message(message));
            eprintln!("{}", format_bash_with_gutter(&cmd.expanded));
        }
        CommandOrigin::Alias { .. } => {}
    }
}

/// Log label for command tracing: "pre-merge user:foo" for hooks, None for aliases.
fn command_log_label(cmd: &PreparedCommand, origin: &CommandOrigin) -> Option<String> {
    match origin {
        CommandOrigin::Hook {
            source, hook_type, ..
        } => {
            let summary = command_summary_name(cmd.name.as_deref(), *source);
            Some(format!("{hook_type} {summary}"))
        }
        CommandOrigin::Alias { .. } => None,
    }
}

/// Label used for template expansion error messages.
fn expansion_label(cmd: &PreparedCommand, origin: &CommandOrigin) -> String {
    match origin {
        CommandOrigin::Hook { source, .. } => command_summary_name(cmd.name.as_deref(), *source),
        CommandOrigin::Alias { name } => name.clone(),
    }
}

/// Handle a command execution error per origin and failure strategy.
///
/// Signal-derived child exits (SIGINT/SIGTERM) bypass both `origin` wrapping
/// and `failure_strategy`: the error is returned as `AlreadyDisplayed` with
/// the `128 + signal` exit code so the enclosing loop aborts. This enforces
/// the project-wide Ctrl-C cancellation policy — see the "Signal Handling"
/// section of the root `CLAUDE.md` for the rationale.
fn handle_command_error(
    err: anyhow::Error,
    cmd: &PreparedCommand,
    origin: &CommandOrigin,
    failure_strategy: FailureStrategy,
) -> anyhow::Result<()> {
    if let Some(exit_code) = interrupt_exit_code(&err) {
        return Err(WorktrunkError::AlreadyDisplayed { exit_code }.into());
    }

    let (err_msg, exit_code) = if let Some(wt_err) = err.downcast_ref::<WorktrunkError>() {
        match wt_err {
            WorktrunkError::ChildProcessExited { message, code, .. } => {
                (message.clone(), Some(*code))
            }
            _ => (err.to_string(), None),
        }
    } else {
        (err.to_string(), None)
    };

    match failure_strategy {
        FailureStrategy::FailFast => match origin {
            CommandOrigin::Hook { hook_type, .. } => Err(WorktrunkError::HookCommandFailed {
                hook_type: *hook_type,
                command_name: cmd.name.clone(),
                error: err_msg,
                exit_code,
            }
            .into()),
            CommandOrigin::Alias { name } => {
                if let Some(code) = exit_code {
                    Err(WorktrunkError::AlreadyDisplayed { exit_code: code }.into())
                } else {
                    bail!("Failed to run alias '{}': {}", name, err_msg)
                }
            }
        },
        FailureStrategy::Warn => {
            let message = match &cmd.name {
                Some(name) => cformat!("Command <bold>{name}</> failed: {err_msg}"),
                None => format!("Command failed: {err_msg}"),
            };
            eprintln!("{}", error_message(message));
            Ok(())
        }
    }
}

/// Expand commands from a CommandConfig without approval.
///
/// When `lazy_enabled` is true, commands referencing `vars.` are validated but not
/// expanded — they carry a `lazy_template` for deferred expansion at execution time.
/// Only enable for pipeline steps where ordering guarantees vars are set by prior steps.
fn expand_commands(
    commands: &[Command],
    ctx: &CommandContext<'_>,
    extra_vars: &[(&str, &str)],
    hook_type: HookType,
    source: HookSource,
    lazy_enabled: bool,
) -> anyhow::Result<Vec<(Command, String, Option<String>)>> {
    let mut base_context = build_hook_context(ctx, extra_vars)?;

    // hook_type is always available as a template variable and in JSON context
    base_context.insert("hook_type".into(), hook_type.to_string());
    // `{{ args }}` is always available in hook scope. Default to an empty
    // JSON sequence (rendered via ShellArgs rehydration) so templates can
    // use `{{ args }}` unconditionally. Manual `wt hook <type>` overrides
    // via extra_vars earlier in the chain; internal invocations (merge,
    // switch, etc.) leave the default in place.
    base_context
        .entry(worktrunk::config::ALIAS_ARGS_KEY.to_string())
        .or_insert_with(|| "[]".to_string());

    let mut result = Vec::new();

    for cmd in commands {
        // hook_name is per-command: available as template variable and in JSON context
        let mut cmd_context = base_context.clone();
        if let Some(ref name) = cmd.name {
            cmd_context.insert("hook_name".into(), name.clone());
        }

        let template_name = match &cmd.name {
            Some(name) => format!("{}:{}", source, name),
            None => format!("{} {} hook", source, hook_type),
        };

        let lazy = lazy_enabled && template_references_var(&cmd.template, "vars");

        let (expanded_str, lazy_template) = if lazy {
            // Parse-only validation: catch syntax errors upfront without rendering.
            // Full rendering (validate_template) would fail on {{ vars.X }} because
            // vars aren't set yet — that's the whole point of lazy expansion.
            validate_template_syntax(&cmd.template, &template_name)
                .map_err(|e| anyhow::anyhow!("syntax error in {template_name}: {e}"))?;
            let tpl = cmd.template.clone();
            (tpl.clone(), Some(tpl))
        } else {
            (
                expand_shell_template(&cmd.template, &cmd_context, ctx.repo, &template_name)?,
                None,
            )
        };

        let context_json = serde_json::to_string(&cmd_context)
            .expect("HashMap<String, String> serialization should never fail");

        result.push((
            Command::with_expansion(cmd.name.clone(), cmd.template.clone(), expanded_str),
            context_json,
            lazy_template,
        ));
    }

    Ok(result)
}

/// Prepare pipeline steps for execution, preserving serial/concurrent structure.
///
/// Returns `Vec<PreparedStep>` that preserves the pipeline structure from
/// the config — `Single` vs `Concurrent` grouping. All hook preparation
/// goes through this function (both foreground and background paths).
pub fn prepare_steps(
    command_config: &CommandConfig,
    ctx: &CommandContext<'_>,
    extra_vars: &[(&str, &str)],
    hook_type: HookType,
    source: HookSource,
) -> anyhow::Result<Vec<PreparedStep>> {
    let steps = command_config.steps();

    // Collect step sizes so we can re-partition after a single expand_commands call.
    // This avoids calling build_hook_context (which spawns git subprocesses) per step.
    let step_sizes: Vec<usize> = steps
        .iter()
        .map(|s| match s {
            HookStep::Single(_) => 1,
            HookStep::Concurrent(cmds) => cmds.len(),
        })
        .collect();

    let all_commands: Vec<Command> = command_config.commands().cloned().collect();
    let all_expanded = expand_commands(&all_commands, ctx, extra_vars, hook_type, source, true)?;
    let mut expanded_iter = all_expanded.into_iter();

    let mut result = Vec::new();
    for (step, &size) in steps.iter().zip(&step_sizes) {
        let chunk: Vec<_> = expanded_iter.by_ref().take(size).collect();
        match step {
            HookStep::Single(_) => {
                let (cmd, json, lazy) = chunk.into_iter().next().unwrap();
                result.push(PreparedStep::Single(PreparedCommand {
                    name: cmd.name,
                    expanded: cmd.expanded,
                    context_json: json,
                    lazy_template: lazy,
                }));
            }
            HookStep::Concurrent(_) => {
                let prepared = chunk
                    .into_iter()
                    .map(|(cmd, json, lazy)| PreparedCommand {
                        name: cmd.name,
                        expanded: cmd.expanded,
                        context_json: json,
                        lazy_template: lazy,
                    })
                    .collect();
                result.push(PreparedStep::Concurrent(prepared));
            }
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_cmd(name: Option<&str>) -> PreparedCommand {
        PreparedCommand {
            name: name.map(String::from),
            expanded: "echo test".to_string(),
            context_json: "{}".to_string(),
            lazy_template: None,
        }
    }

    #[test]
    fn test_handle_command_error_hook_failfast_child_process_exited() {
        let err: anyhow::Error = WorktrunkError::ChildProcessExited {
            code: 42,
            message: "command failed".into(),
            signal: None,
        }
        .into();
        let cmd = make_cmd(Some("lint"));
        let origin = CommandOrigin::Hook {
            source: HookSource::User,
            hook_type: HookType::PreMerge,
            display_path: None,
        };
        let result = handle_command_error(err, &cmd, &origin, FailureStrategy::FailFast);
        let err = result.unwrap_err();
        let wt_err = err.downcast_ref::<WorktrunkError>().unwrap();
        assert!(matches!(
            wt_err,
            WorktrunkError::HookCommandFailed {
                exit_code: Some(42),
                ..
            }
        ));
    }

    #[test]
    fn test_handle_command_error_hook_failfast_non_child_worktrunk_error() {
        // WorktrunkError that isn't ChildProcessExited (line 439 coverage)
        let err: anyhow::Error = WorktrunkError::CommandNotApproved.into();
        let cmd = make_cmd(Some("build"));
        let origin = CommandOrigin::Hook {
            source: HookSource::User,
            hook_type: HookType::PreMerge,
            display_path: None,
        };
        let result = handle_command_error(err, &cmd, &origin, FailureStrategy::FailFast);
        let err = result.unwrap_err();
        let wt_err = err.downcast_ref::<WorktrunkError>().unwrap();
        assert!(matches!(
            wt_err,
            WorktrunkError::HookCommandFailed {
                exit_code: None,
                ..
            }
        ));
    }

    #[test]
    fn test_handle_command_error_hook_failfast_other_error() {
        let err = anyhow::anyhow!("something else");
        let cmd = make_cmd(None);
        let origin = CommandOrigin::Hook {
            source: HookSource::Project,
            hook_type: HookType::PreCommit,
            display_path: None,
        };
        let result = handle_command_error(err, &cmd, &origin, FailureStrategy::FailFast);
        let err = result.unwrap_err();
        let wt_err = err.downcast_ref::<WorktrunkError>().unwrap();
        assert!(matches!(
            wt_err,
            WorktrunkError::HookCommandFailed {
                exit_code: None,
                ..
            }
        ));
    }

    #[test]
    fn test_handle_command_error_alias_failfast_child_process_exited() {
        let err: anyhow::Error = WorktrunkError::ChildProcessExited {
            code: 1,
            message: "exit 1".into(),
            signal: None,
        }
        .into();
        let cmd = make_cmd(None);
        let origin = CommandOrigin::Alias {
            name: "deploy".into(),
        };
        let result = handle_command_error(err, &cmd, &origin, FailureStrategy::FailFast);
        let err = result.unwrap_err();
        let wt_err = err.downcast_ref::<WorktrunkError>().unwrap();
        assert!(matches!(
            wt_err,
            WorktrunkError::AlreadyDisplayed { exit_code: 1 }
        ));
    }

    #[test]
    fn test_handle_command_error_alias_failfast_other_error() {
        let err = anyhow::anyhow!("template error");
        let cmd = make_cmd(None);
        let origin = CommandOrigin::Alias {
            name: "deploy".into(),
        };
        let result = handle_command_error(err, &cmd, &origin, FailureStrategy::FailFast);
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Failed to run alias 'deploy'"));
        assert!(err_msg.contains("template error"));
    }

    #[test]
    fn test_handle_command_error_warn_continues() {
        let err: anyhow::Error = WorktrunkError::ChildProcessExited {
            code: 1,
            message: "lint failed".into(),
            signal: None,
        }
        .into();
        let cmd = make_cmd(Some("lint"));
        let origin = CommandOrigin::Hook {
            source: HookSource::User,
            hook_type: HookType::PostStart,
            display_path: None,
        };
        let result = handle_command_error(err, &cmd, &origin, FailureStrategy::Warn);
        assert!(result.is_ok());
    }

    #[test]
    fn test_handle_command_error_warn_unnamed() {
        let err = anyhow::anyhow!("unexpected failure");
        let cmd = make_cmd(None);
        let origin = CommandOrigin::Hook {
            source: HookSource::User,
            hook_type: HookType::PostStart,
            display_path: None,
        };
        let result = handle_command_error(err, &cmd, &origin, FailureStrategy::Warn);
        assert!(result.is_ok());
    }

    #[test]
    fn test_command_log_label() {
        let cmd = make_cmd(Some("lint"));
        let hook_origin = CommandOrigin::Hook {
            source: HookSource::User,
            hook_type: HookType::PreMerge,
            display_path: None,
        };
        assert_eq!(
            command_log_label(&cmd, &hook_origin),
            Some("pre-merge user:lint".to_string())
        );

        let alias_origin = CommandOrigin::Alias {
            name: "deploy".into(),
        };
        assert_eq!(command_log_label(&cmd, &alias_origin), None);
    }

    #[test]
    fn test_expansion_label() {
        let cmd = make_cmd(Some("build"));
        let hook_origin = CommandOrigin::Hook {
            source: HookSource::Project,
            hook_type: HookType::PreStart,
            display_path: None,
        };
        assert_eq!(expansion_label(&cmd, &hook_origin), "project:build");

        let alias_origin = CommandOrigin::Alias { name: "ci".into() };
        assert_eq!(expansion_label(&cmd, &alias_origin), "ci");
    }

    #[test]
    fn test_template_references_var_for_vars() {
        // Real vars references
        assert!(template_references_var("{{ vars.container }}", "vars"));
        assert!(template_references_var("{{vars.container}}", "vars"));
        assert!(template_references_var(
            "docker run --name {{ vars.name }}",
            "vars"
        ));
        assert!(template_references_var(
            "{% if vars.key %}yes{% endif %}",
            "vars"
        ));

        // Literal text — not a template reference
        assert!(!template_references_var(
            "echo hello > template_vars.txt",
            "vars"
        ));
        assert!(!template_references_var("no vars references here", "vars"));
    }
}
