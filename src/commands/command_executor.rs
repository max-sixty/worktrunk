use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use anyhow::Result;
use color_print::cformat;
use worktrunk::HookType;
use worktrunk::config::{
    Command, CommandConfig, HookStep, UserConfig, expand_template, format_hook_variables,
    template_references_var, validate_template_syntax,
};
use worktrunk::git::{ErrorExt, Repository, WorktrunkError};
use worktrunk::path::{format_path_for_display, to_posix_path};
use worktrunk::shell_exec::ShellEscapeMode;
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
    /// Raw template, rendered against `context` when the command runs.
    /// Syntax is validated at preparation; rendering is deferred so `vars.*`
    /// set by earlier pipeline steps are read fresh from git config.
    pub template: String,
    /// Template variables, frozen at preparation. Serialized to JSON only at
    /// the process boundary (child stdin, background pipeline spec).
    pub context: HashMap<String, String>,
    /// Name used in template expansion errors: `"user:foo"` for named hook
    /// commands, `"user pre-merge hook"` for unnamed ones, the alias name for
    /// aliases.
    pub template_name: String,
    /// Label for the per-command announcement summary and render span.
    /// For hooks: `"user:foo"` for named, `"user"` for unnamed. For aliases: alias name.
    pub label: String,
}

impl PreparedCommand {
    /// The JSON form of `context` piped to the child's stdin.
    pub fn context_json(&self) -> String {
        serde_json::to_string(&self.context)
            .expect("HashMap<String, String> serialization should never fail")
    }
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

impl PipelineKind {
    /// Log label for command tracing: hooks log as `"{hook_type} {label}"`
    /// (e.g. `"pre-merge user:foo"`); aliases skip per-command logging.
    fn log_label(&self, cmd: &PreparedCommand) -> Option<String> {
        match self {
            Self::Hook { hook_type, .. } => Some(format!("{hook_type} {}", cmd.label)),
            Self::Alias { .. } => None,
        }
    }

    /// Whether this pipeline is a hook (vs an alias). Hooks scrub inherited
    /// git-discovery env vars from their children so a hook's `git` commands
    /// resolve against the worktree wt targets, not an inherited `GIT_DIR`
    /// (issue #3373).
    fn is_hook(&self) -> bool {
        matches!(self, Self::Hook { .. })
    }
}

/// A pipeline step ready for foreground execution, with rendering / error policy.
pub struct ForegroundStep {
    pub step: PreparedStep,
    /// Which pipeline this step belongs to. Drives how each command is
    /// announced before execution: hooks render a per-command "Running …"
    /// line plus the bash gutter; aliases stay silent (the caller emits one
    /// pipeline summary line).
    pub announce: PipelineKind,
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
    /// The repository, rooted at the worktree this operation acts on.
    ///
    /// For hooks this field is load-bearing: `ctx.repo.load_project_config()` is
    /// how a hook gets its `.config/wt.toml`, so whichever worktree `repo` is
    /// rooted at decides which file is read. The per-hook mapping (and why each
    /// construction site picks the root it does) is the spec in the
    /// [`super::hooks`] module docs.
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
                .run_command(&["rev-parse", "--verify", "--end-of-options", branch])
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
/// [`ShellEscapeMode::Posix`]: every caller (foreground hooks, background
/// pipelines, aliases) interpolates the result into a command line run
/// through `Cmd::shell` (`sh`/Git Bash), which is always POSIX — unlike the
/// `--execute` payload, this never reaches a PowerShell wrapper. Every
/// execution path renders just before the command runs so prior steps can
/// set `vars.*` via git config.
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
    Ok(expand_template(
        template,
        &vars,
        ShellEscapeMode::Posix,
        repo,
        label,
    )?)
}

/// Resolve the shell string to execute for a prepared command.
///
/// Templates render at execution time against the frozen `context`, so they
/// see fresh git-config state (`vars.*` set by earlier steps).
fn resolve_command_str(cmd: &PreparedCommand, repo: &Repository) -> Result<String> {
    expand_shell_template(&cmd.template, &cmd.context, repo, &cmd.template_name)
}

/// Render a template for dry-run / preview display. Mirrors execution-time
/// semantics: a template referencing `vars.*` is shown raw after a syntax
/// check — its values resolve from git config when the step runs, possibly
/// written by earlier pipeline steps — while everything else renders against
/// `context`. Expansion is side-effect-free, so previewing never perturbs the
/// real run.
pub fn render_template_preview(
    template: &str,
    context: &HashMap<String, String>,
    repo: &Repository,
    name: &str,
) -> Result<String> {
    if template_references_var(template, "vars") {
        validate_template_syntax(template, name)?;
        Ok(template.to_string())
    } else {
        expand_shell_template(template, context, repo, name)
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
/// Handles serial/concurrent step execution, per-command announcement,
/// execution-time template rendering, and policy-driven error handling.
///
/// `Single` steps run one at a time; `Concurrent` steps run all their
/// commands in parallel via the prefixed-line executor.
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
                run_concurrent_group(cmds, fg_step, repo, wt_path, failure_strategy)?;
            }
        }
    }
    Ok(())
}

/// Run every command in a concurrent group via the prefixed-line executor.
///
/// Expands all templates sequentially before spawning any thread (template
/// expansion reads git config; racing on reads would produce inconsistent
/// state), announces each command (per the step's `PipelineKind` — hooks
/// render per-command announcements, aliases only announce the outer group),
/// then dispatches to `run_concurrent_commands` which streams each child's
/// output prefixed by its label and waits for all to complete before folding
/// outcomes.
fn run_concurrent_group(
    cmds: &[PreparedCommand],
    fg_step: &ForegroundStep,
    repo: &Repository,
    wt_path: &Path,
    failure_strategy: FailureStrategy,
) -> anyhow::Result<()> {
    let directives = &fg_step.directives;

    let expanded: Vec<String> = cmds
        .iter()
        .map(|cmd| {
            let _span = Span::new(format!("template_render:{}", cmd.label));
            resolve_command_str(cmd, repo)
        })
        .collect::<Result<_>>()?;

    for (cmd, command_str) in cmds.iter().zip(&expanded) {
        announce_command(cmd, &fg_step.announce, command_str);
    }

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

    let context_jsons: Vec<String> = cmds.iter().map(PreparedCommand::context_json).collect();
    let log_labels: Vec<Option<String>> = cmds
        .iter()
        .map(|cmd| fg_step.announce.log_label(cmd))
        .collect();

    let scrub_git_discovery = fg_step.announce.is_hook();
    let specs: Vec<ConcurrentCommand<'_>> = (0..cmds.len())
        .map(|i| ConcurrentCommand {
            label: labels[i],
            expanded: &expanded[i],
            working_dir: wt_path,
            context_json: &context_jsons[i],
            log_label: log_labels[i].as_deref(),
            directives,
            scrub_git_discovery,
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

/// Execute a single prepared command: expand, announce, run, handle errors.
fn run_one_command(
    cmd: &PreparedCommand,
    fg_step: &ForegroundStep,
    repo: &Repository,
    wt_path: &Path,
    failure_strategy: FailureStrategy,
) -> anyhow::Result<()> {
    let directives = &fg_step.directives;

    let command_str = {
        let _span = Span::new(format!("template_render:{}", cmd.label));
        resolve_command_str(cmd, repo)?
    };
    announce_command(cmd, &fg_step.announce, &command_str);

    // Hooks get a documented JSON context on stdin; aliases inherit stdin so
    // interactive children (e.g. `wt switch`'s picker) keep their controlling
    // terminal. Piping JSON into an interactive alias body steals the tty.
    let stdin_json = fg_step.pipe_stdin.then(|| cmd.context_json());
    let log_label = fg_step.announce.log_label(cmd);
    let result = execute_shell_command(
        wt_path,
        &command_str,
        stdin_json.as_deref(),
        log_label.as_deref(),
        directives.clone(),
        fg_step.redirect_stdout_to_stderr,
        fg_step.announce.is_hook(),
    );

    match result {
        Ok(()) => Ok(()),
        Err(err) => handle_command_error(err, cmd, &fg_step.error_wrapper, failure_strategy),
    }
}

/// Announce a command before execution, formatted per the step's pipeline kind.
///
/// Hook pipelines emit a per-command "Running …" line plus a bash gutter
/// showing `command_str` — the rendered command about to run. Alias pipelines
/// emit nothing — the alias caller renders a single pipeline summary
/// externally.
fn announce_command(cmd: &PreparedCommand, kind: &PipelineKind, command_str: &str) {
    let PipelineKind::Hook {
        hook_type,
        display_path,
    } = kind
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
        let vars = format_hook_variables(*hook_type, &cmd.context);
        eprintln!(
            "{}",
            info_message(cformat!("<bold>{hook_type}</> template variables:"))
        );
        eprintln!("{}", format_with_gutter(&vars, None));
    }
    eprintln!("{}", progress_message(message));
    eprintln!("{}", format_bash_with_gutter(command_str));
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

/// Walk a config's pipeline structure, preparing each command.
///
/// Shared by hook and alias preparation: preserves the `Single` vs
/// `Concurrent` grouping from the config while `prepare` supplies the
/// per-command [`PreparedCommand`] (context construction and label naming
/// differ between hooks and aliases).
pub fn map_config_steps(
    config: &CommandConfig,
    mut prepare: impl FnMut(&Command) -> anyhow::Result<PreparedCommand>,
) -> anyhow::Result<Vec<PreparedStep>> {
    config
        .steps()
        .iter()
        .map(|step| match step {
            HookStep::Single(cmd) => Ok(PreparedStep::Single(prepare(cmd)?)),
            HookStep::Concurrent(cmds) => Ok(PreparedStep::Concurrent(
                cmds.iter().map(&mut prepare).collect::<Result<_>>()?,
            )),
        })
        .collect()
}

/// Prepare hook pipeline steps for execution, preserving serial/concurrent
/// structure. All hook preparation goes through this function (both
/// foreground and background paths).
///
/// Each command freezes its context as JSON and keeps its raw template;
/// rendering happens when the command runs. Syntax errors abort here — before
/// the first step runs — while semantic errors (undefined variable, filter
/// failure) surface at the failing step.
pub fn prepare_steps(
    command_config: &CommandConfig,
    ctx: &CommandContext<'_>,
    extra_vars: &[(&str, &str)],
    hook_type: HookType,
    source: HookSource,
) -> anyhow::Result<Vec<PreparedStep>> {
    // Built once per pipeline — build_hook_context spawns git subprocesses.
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

    map_config_steps(command_config, |cmd| {
        // hook_name is per-command: available as template variable and in JSON context
        let mut cmd_context = base_context.clone();
        if let Some(ref name) = cmd.name {
            cmd_context.insert("hook_name".into(), name.clone());
        }

        let template_name = match &cmd.name {
            Some(name) => format!("{source}:{name}"),
            None => format!("{source} {hook_type} hook"),
        };
        validate_template_syntax(&cmd.template, &template_name)?;

        Ok(PreparedCommand {
            name: cmd.name.clone(),
            template: cmd.template.clone(),
            context: cmd_context,
            template_name,
            label: command_summary_name(cmd.name.as_deref(), source),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_cmd(name: Option<&str>) -> PreparedCommand {
        let label = command_summary_name(name, HookSource::User);
        PreparedCommand {
            name: name.map(String::from),
            template: "echo test".to_string(),
            context: HashMap::new(),
            template_name: label.clone(),
            label,
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
        let wrapper = hook_error_wrapper(HookType::PostCreate);
        let result = handle_command_error(err, &cmd, &wrapper, FailureStrategy::Warn);
        assert!(result.is_ok());
    }

    #[test]
    fn test_handle_command_error_warn_signal_aborts() {
        let err: anyhow::Error = WorktrunkError::ChildProcessExited {
            code: 143,
            message: "terminated".into(),
            signal: Some(15),
        }
        .into();
        let cmd = make_cmd(Some("cleanup"));
        let wrapper = hook_error_wrapper(HookType::PostCreate);
        let result = handle_command_error(err, &cmd, &wrapper, FailureStrategy::Warn);
        let err = result.unwrap_err();
        let wt_err = err.downcast_ref::<WorktrunkError>().unwrap();
        assert!(matches!(
            wt_err,
            WorktrunkError::AlreadyDisplayed { exit_code: 143 }
        ));
    }

    #[test]
    fn test_handle_command_error_warn_unnamed() {
        // Covers the `cmd.name = None` branch of the Warn arm.
        let err = anyhow::anyhow!("unexpected failure");
        let cmd = make_cmd(None);
        let wrapper = hook_error_wrapper(HookType::PostCreate);
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
