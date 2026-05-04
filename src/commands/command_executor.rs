use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use color_print::cformat;
use worktrunk::HookType;
use worktrunk::config::{
    Command, CommandConfig, HookStep, UserConfig, expand_template, format_hook_variables,
    template_references_var, validate_template_syntax,
};
use worktrunk::git::{ErrorExt, Repository, WorktrunkError};
use worktrunk::path::{format_path_for_display, to_posix_path};
use worktrunk::styling::{
    eprintln, error_message, format_bash_with_gutter, format_with_gutter, info_message,
    progress_message, verbosity,
};
use worktrunk::trace::Span;

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
    /// Label for template expansion errors and per-command announcement summary.
    /// For hooks: `"user:foo"` for named, `"user"` for unnamed. For aliases: alias name.
    pub label: String,
    /// Log label for command tracing (e.g. `"pre-merge user:foo"`). `None` skips logging.
    pub log_label: Option<String>,
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

/// Per-step announcement policy — replaces the per-command match on origin.
///
/// Hook steps render a per-command `Running {type} {label} @ {path}` line plus
/// the bash gutter. Alias steps suppress per-command rendering because the
/// caller emits a single summary line for the whole pipeline.
pub enum AnnouncePolicy {
    Hook {
        hook_type: HookType,
        display_path: Option<PathBuf>,
    },
    None,
}

/// Wraps a command failure (FailFast path) into the final error.
///
/// Receives the failing command, the message extracted from the inner error,
/// and the optional exit code (set when the inner error is
/// `WorktrunkError::ChildProcessExited`). Signal-derived errors bypass this
/// wrapper and short-circuit to `AlreadyDisplayed` — see
/// [`handle_command_error`] for the rationale.
///
/// The wrapper is invoked on the calling thread after concurrent children
/// have joined (`run_concurrent_group` folds outcomes serially), so no
/// `Send + Sync` bounds are required.
pub type ErrorWrapper = Box<dyn Fn(&PreparedCommand, String, Option<i32>) -> anyhow::Error>;

/// What kind of pipeline a sourced step belongs to.
///
/// Supplied at conversion time (`sourced_steps_to_foreground`) so a single
/// `SourcedStep` shape can be produced by both alias and hook resolution.
/// Drives the per-step trust model (EXEC passthrough), announce policy,
/// stdin handling, and error wrapping. Hook-only metadata
/// (`hook_type`, `display_path`) lives on the `Hook` variant — it's
/// per-pipeline, not per-step, so the per-step shape stays neutral.
#[derive(Clone)]
pub enum PipelineKind {
    Hook {
        hook_type: HookType,
        display_path: Option<PathBuf>,
    },
    Alias {
        name: String,
    },
}

/// A pipeline step ready for foreground execution, with rendering / error policy.
pub struct ForegroundStep {
    pub step: PreparedStep,
    /// Whether `Concurrent` steps actually run concurrently. When `false`,
    /// concurrent commands execute serially (deprecated pre-* table form).
    pub concurrent: bool,
    /// How to announce each command before execution.
    pub announce: AnnouncePolicy,
    /// Pipe `context_json` to the child's stdin (hooks); when `false`, inherit
    /// the parent's stdin so interactive children keep the controlling tty
    /// (aliases).
    pub pipe_stdin: bool,
    /// Merge the child's stdout onto wt's stderr (`true`, hooks) or pass it
    /// through unchanged (`false`, aliases). Hooks merge so their output stays
    /// ordered with wt's own stderr "Running …" lines; aliases pass through so
    /// `wt <alias> | …` remains usable in scripts (#2478).
    pub redirect_stdout_to_stderr: bool,
    /// Wraps a per-command failure into the final error returned to the caller.
    pub error_wrapper: ErrorWrapper,
    /// Per-step directive passthrough. Trust differs by source — user-source
    /// alias steps pass EXEC through (the body is the user's own config),
    /// while project-source steps and all hook steps scrub it. Per-step rather
    /// than per-pipeline so a merged user+project alias relaxes the user's
    /// own steps without leaking the project's body into the parent shell.
    pub directives: DirectivePassthrough,
}

/// Controls how foreground execution responds to command failures.
#[derive(Clone, Copy)]
pub enum FailureStrategy {
    /// Stop on first failure and surface the error to the caller.
    FailFast,
    /// Log warnings and continue executing remaining commands.
    Warn,
}

impl FailureStrategy {
    /// Default strategy for a hook type: `pre-*` block (fail-fast),
    /// `post-*` warn-and-continue.
    pub fn default_for(hook_type: HookType) -> Self {
        if hook_type.is_pre() {
            Self::FailFast
        } else {
            Self::Warn
        }
    }
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
///
/// `referenced`, when `Some`, restricts the map to keys named in the set —
/// vars the body doesn't reference are not computed. Aliases pass their
/// `referenced_vars_for_config` set (extended via `alias_context_filter`)
/// so unused git lookups (`var_commit` rev-parse, `var_default_branch` cold
/// detection, `branch().upstream()`) are skipped, and the verbose
/// `template variables:` table only lists vars the body actually references.
/// Hooks pass `None` so every standard var stays available — the child
/// receives the full context as JSON on stdin and may consume keys that
/// don't appear in the inline `{{ }}` template (e.g. via `jq`).
pub fn build_hook_context(
    ctx: &CommandContext<'_>,
    extra_vars: &[(&str, &str)],
    referenced: Option<&BTreeSet<String>>,
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

    let want = |key: &str| referenced.is_none_or(|r| r.contains(key));

    // Cheap vars (already in scope, no I/O) are populated unconditionally —
    // skipping them saves no work and would just turn the verbose table's
    // `(unused)` label into noise. Only the expensive blocks below
    // (subprocesses, git config / remote lookups) honor `want`.
    let mut map = HashMap::new();
    map.insert("repo".into(), repo_name.into());
    map.insert("branch".into(), ctx.branch_or_head().into());
    map.insert("worktree_name".into(), worktree_name.into());
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
    if want("default_branch") {
        let _span = Span::new("var_default_branch");
        if let Some(default_branch) = ctx.repo.default_branch() {
            map.insert("default_branch".into(), default_branch);
        }
    }

    // Primary worktree path (where established files live)
    if want("primary_worktree_path") || want("main_worktree_path") {
        let _span = Span::new("var_primary_worktree");
        if let Ok(Some(path)) = ctx.repo.primary_worktree() {
            let path_str = to_posix_path(&path.to_string_lossy());
            if want("primary_worktree_path") {
                map.insert("primary_worktree_path".into(), path_str.clone());
            }
            // Deprecated alias
            if want("main_worktree_path") {
                map.insert("main_worktree_path".into(), path_str);
            }
        }
    }

    // Resolve commit from the Active branch, not HEAD at discovery path.
    // This ensures {{ commit }} follows the Active branch even when the
    // CommandContext points to a different worktree than where we're running.
    // Detached HEAD (`ctx.branch == None`) must read HEAD from
    // `ctx.worktree_path`, not the running worktree: `wt step for-each`
    // iterates over sibling worktrees, and a sibling on detached HEAD has a
    // different HEAD than the worktree `wt` runs in. Branched contexts go
    // through `rev-parse <branch>`, which is repo-wide.
    if want("commit") || want("short_commit") {
        let _span = Span::new("var_commit");
        let commit = match ctx.branch {
            Some(branch) => ctx
                .repo
                .run_command(&["rev-parse", branch])
                .ok()
                .map(|s| s.trim().to_owned()),
            None => ctx
                .repo
                .worktree_at(ctx.worktree_path)
                .head_sha()
                .ok()
                .flatten(),
        };
        if let Some(commit) = commit {
            if want("short_commit")
                && let Ok(short) = ctx.repo.short_sha(&commit)
            {
                map.insert("short_commit".into(), short);
            }
            if want("commit") {
                map.insert("commit".into(), commit);
            }
        }
    }

    if want("remote") || want("remote_url") || want("upstream") {
        let _span = Span::new("var_remote");
        if let Ok(remote) = ctx.repo.primary_remote() {
            if want("remote") {
                map.insert("remote".into(), remote.to_string());
            }
            // Add remote URL for conditional hook execution (e.g., GitLab vs GitHub)
            if want("remote_url")
                && let Some(url) = ctx.repo.remote_url(&remote)
            {
                map.insert("remote_url".into(), url);
            }
            if want("upstream")
                && let Some(branch) = ctx.branch
                && let Ok(Some(upstream)) = ctx.repo.branch(branch).upstream()
            {
                map.insert("upstream".into(), upstream);
            }
        }
    }

    // Execution directory — always where the hook command runs, even when
    // worktree_path points to an Active identity that doesn't exist on disk.
    map.insert(
        "cwd".into(),
        to_posix_path(&ctx.worktree_path.to_string_lossy()),
    );

    // Caller-set bindings (e.g., merge target, switch base, alias args).
    // Aliases pre-filter via `AliasOptions::parse`, hooks pass everything;
    // either way the value is already computed, so insert unconditionally.
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

/// Resolve the shell string to execute for a prepared command.
///
/// Commands carrying a `lazy_template` are re-expanded against their
/// `context_json` at execution time so they see fresh git-config state
/// (`vars.*` set by earlier steps). Commands without one were already
/// expanded at prep time — return the cached `expanded` string.
fn resolve_command_str(cmd: &PreparedCommand, repo: &Repository) -> Result<String> {
    match &cmd.lazy_template {
        Some(template) => {
            let context: HashMap<String, String> = serde_json::from_str(&cmd.context_json)
                .context("failed to deserialize context_json")?;
            expand_shell_template(template, &context, repo, &cmd.label)
        }
        None => Ok(cmd.expanded.clone()),
    }
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
/// template resolution, and policy-driven error handling.
///
/// Each `ForegroundStep` carries a `concurrent` flag. When true, `Concurrent`
/// steps spawn threads via `thread::scope`. When false (deprecated pre-*
/// single-table form), `Concurrent` steps execute serially. Pipeline configs
/// (`[[hook]]` blocks), aliases, and post-* hooks set `concurrent: true`.
pub fn execute_pipeline_foreground(
    steps: &[ForegroundStep],
    repo: &Repository,
    wt_path: &Path,
    failure_strategy: FailureStrategy,
) -> anyhow::Result<()> {
    for fg_step in steps {
        match &fg_step.step {
            PreparedStep::Single(cmd) => {
                run_one_command(cmd, fg_step, repo, wt_path, failure_strategy)?;
            }
            PreparedStep::Concurrent(cmds) => {
                if !fg_step.concurrent {
                    for cmd in cmds {
                        run_one_command(cmd, fg_step, repo, wt_path, failure_strategy)?;
                    }
                } else {
                    run_concurrent_group(cmds, fg_step, repo, wt_path, failure_strategy)?;
                }
            }
        }
    }
    Ok(())
}

/// Run every command in a concurrent group via the prefixed-line executor.
///
/// Announces each command up front (per the step's `AnnouncePolicy` — hooks
/// render per-command announcements, aliases only announce the outer group),
/// expands all templates sequentially (template expansion reads git config;
/// racing on reads would produce inconsistent state), then dispatches to
/// `run_concurrent_commands` which streams each child's output prefixed by
/// its label and waits for all to complete before folding outcomes.
fn run_concurrent_group(
    cmds: &[PreparedCommand],
    fg_step: &ForegroundStep,
    repo: &Repository,
    wt_path: &Path,
    failure_strategy: FailureStrategy,
) -> anyhow::Result<()> {
    let directives = &fg_step.directives;
    for cmd in cmds {
        announce_command(cmd, &fg_step.announce);
    }

    let expanded: Vec<String> = cmds
        .iter()
        .map(|cmd| {
            let _span = Span::new(format!("template_render:{}", cmd.label));
            resolve_command_str(cmd, repo)
        })
        .collect::<Result<_>>()?;

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
            log_label: cmd.log_label.as_deref(),
            directives,
        })
        .collect();

    let outcomes = run_concurrent_commands(&specs)?;

    let mut first_failure: Option<anyhow::Error> = None;
    for (outcome, cmd) in outcomes.into_iter().zip(cmds) {
        let Err(err) = outcome else { continue };
        match handle_command_error(err, cmd, &fg_step.error_wrapper, failure_strategy) {
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
    fg_step: &ForegroundStep,
    repo: &Repository,
    wt_path: &Path,
    failure_strategy: FailureStrategy,
) -> anyhow::Result<()> {
    let directives = &fg_step.directives;
    announce_command(cmd, &fg_step.announce);

    let command_str = {
        let _span = Span::new(format!("template_render:{}", cmd.label));
        resolve_command_str(cmd, repo)?
    };

    // Hooks get a documented JSON context on stdin; aliases inherit stdin so
    // interactive children (e.g. `wt switch`'s picker) keep their controlling
    // terminal. Piping JSON into an interactive alias body steals the tty.
    let stdin_json = if fg_step.pipe_stdin {
        Some(cmd.context_json.as_str())
    } else {
        None
    };
    let result = execute_shell_command(
        wt_path,
        &command_str,
        stdin_json,
        cmd.log_label.as_deref(),
        directives.clone(),
        fg_step.redirect_stdout_to_stderr,
    );

    match result {
        Ok(()) => Ok(()),
        Err(err) => handle_command_error(err, cmd, &fg_step.error_wrapper, failure_strategy),
    }
}

/// Announce a command before execution, formatted per the step's policy.
///
/// Hook policies emit a per-command "Running …" line plus the bash gutter.
/// Alias policies (`AnnouncePolicy::None`) emit nothing — the alias caller
/// renders a single pipeline summary externally.
fn announce_command(cmd: &PreparedCommand, policy: &AnnouncePolicy) {
    let AnnouncePolicy::Hook {
        hook_type,
        display_path,
    } = policy
    else {
        return;
    };

    let full_label = match &cmd.name {
        Some(_) => format_command_label(&hook_type.to_string(), Some(&cmd.label)),
        None => format!("Running {hook_type} {} hook", cmd.label),
    };
    let message = match display_path.as_deref() {
        Some(path) => {
            let path_display = format_path_for_display(path);
            cformat!("{full_label} @ <bold>{path_display}</>")
        }
        None => full_label,
    };
    if verbosity() >= 1 {
        let ctx: HashMap<String, String> = serde_json::from_str(&cmd.context_json)
            .expect("context_json is always serialized from a HashMap<String, String>");
        let vars = format_hook_variables(*hook_type, &ctx);
        eprintln!("{}", info_message("template variables:"));
        eprintln!("{}", format_with_gutter(&vars, None));
    }
    eprintln!("{}", progress_message(message));
    eprintln!("{}", format_bash_with_gutter(&cmd.expanded));
}

/// Build the standard `ErrorWrapper` for hook steps.
///
/// Wraps non-signal failures in `WorktrunkError::HookCommandFailed`. Signal
/// errors short-circuit upstream (see [`handle_command_error`]).
pub fn hook_error_wrapper(hook_type: HookType) -> ErrorWrapper {
    Box::new(move |cmd, err_msg, exit_code| {
        WorktrunkError::HookCommandFailed {
            hook_type,
            command_name: cmd.name.clone(),
            error: err_msg,
            exit_code,
        }
        .into()
    })
}

/// Build the standard `ErrorWrapper` for alias steps.
///
/// Children that report a child exit code surface as `AlreadyDisplayed` so
/// `wt` propagates the alias's exit status. Anything else (template errors,
/// spawn failures) is wrapped with the alias name.
pub fn alias_error_wrapper(alias_name: String) -> ErrorWrapper {
    Box::new(move |_cmd, err_msg, exit_code| match exit_code {
        Some(code) => WorktrunkError::AlreadyDisplayed { exit_code: code }.into(),
        None => anyhow::anyhow!("Failed to run alias '{}': {}", alias_name, err_msg),
    })
}

/// Handle a command execution error via the step's `ErrorWrapper`.
///
/// Signal-derived child exits (SIGINT/SIGTERM) bypass the wrapper and
/// `failure_strategy`: the error is returned as `AlreadyDisplayed` with the
/// `128 + signal` exit code so the enclosing loop aborts. This enforces the
/// project-wide Ctrl-C cancellation policy — see the "Signal Handling"
/// section of the root `CLAUDE.md` for the rationale.
fn handle_command_error(
    err: anyhow::Error,
    cmd: &PreparedCommand,
    error_wrapper: &ErrorWrapper,
    failure_strategy: FailureStrategy,
) -> anyhow::Result<()> {
    if let Some(exit_code) = err.interrupt_exit_code() {
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
        FailureStrategy::FailFast => Err(error_wrapper(cmd, err_msg, exit_code)),
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
    let mut base_context = build_hook_context(ctx, extra_vars, None)?;

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

    let make_prepared = |cmd: Command, json: String, lazy: Option<String>| -> PreparedCommand {
        let label = command_summary_name(cmd.name.as_deref(), source);
        let log_label = format!("{hook_type} {label}");
        PreparedCommand {
            name: cmd.name,
            expanded: cmd.expanded,
            context_json: json,
            lazy_template: lazy,
            label,
            log_label: Some(log_label),
        }
    };

    let mut result = Vec::new();
    for (step, &size) in steps.iter().zip(&step_sizes) {
        let chunk: Vec<_> = expanded_iter.by_ref().take(size).collect();
        match step {
            HookStep::Single(_) => {
                let (cmd, json, lazy) = chunk.into_iter().next().unwrap();
                result.push(PreparedStep::Single(make_prepared(cmd, json, lazy)));
            }
            HookStep::Concurrent(_) => {
                let prepared = chunk
                    .into_iter()
                    .map(|(cmd, json, lazy)| make_prepared(cmd, json, lazy))
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
        let label = command_summary_name(name, HookSource::User);
        let log_label = format!("pre-merge {label}");
        PreparedCommand {
            name: name.map(String::from),
            expanded: "echo test".to_string(),
            context_json: "{}".to_string(),
            lazy_template: None,
            label,
            log_label: Some(log_label),
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
        let wrapper = hook_error_wrapper(HookType::PreMerge);
        let result = handle_command_error(err, &cmd, &wrapper, FailureStrategy::FailFast);
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
        // WorktrunkError that isn't ChildProcessExited
        let err: anyhow::Error = WorktrunkError::CommandNotApproved.into();
        let cmd = make_cmd(Some("build"));
        let wrapper = hook_error_wrapper(HookType::PreMerge);
        let result = handle_command_error(err, &cmd, &wrapper, FailureStrategy::FailFast);
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
        let wrapper = alias_error_wrapper("deploy".into());
        let result = handle_command_error(err, &cmd, &wrapper, FailureStrategy::FailFast);
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
        let wrapper = alias_error_wrapper("deploy".into());
        let result = handle_command_error(err, &cmd, &wrapper, FailureStrategy::FailFast);
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
        let wrapper = hook_error_wrapper(HookType::PostStart);
        let result = handle_command_error(err, &cmd, &wrapper, FailureStrategy::Warn);
        assert!(result.is_ok());
    }

    #[test]
    fn test_handle_command_error_warn_unnamed() {
        // Covers the `cmd.name = None` branch of the Warn arm.
        let err = anyhow::anyhow!("unexpected failure");
        let cmd = make_cmd(None);
        let wrapper = hook_error_wrapper(HookType::PostStart);
        let result = handle_command_error(err, &cmd, &wrapper, FailureStrategy::Warn);
        assert!(result.is_ok());
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
