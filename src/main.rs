use std::collections::HashSet;
use std::io::Write;

use anyhow::Context;
use clap::FromArgMatches;
use clap::error::ErrorKind as ClapErrorKind;
use color_print::{ceprintln, cformat};
use std::process;
use worktrunk::config::{UserConfig, set_config_path};
use worktrunk::git::{
    Repository, ResolvedWorktree, current_or_recover, cwd_removed_hint, exit_code, set_base_path,
};
use worktrunk::styling::{
    eprintln, error_message, format_with_gutter, hint_message, info_message, warning_message,
};

use commands::command_approval::approve_hooks;
use commands::command_executor::CommandContext;
use commands::list::progressive::RenderMode;
use commands::worktree::RemoveResult;

mod cli;
mod commands;
mod completion;
mod diagnostic;
mod display;
mod help;
pub(crate) mod help_pager;
mod invocation;
mod llm;
mod md_help;
mod output;
mod pager;
mod summary;
mod verbose_log;

// Re-export invocation utilities at crate level for use by other modules
pub(crate) use invocation::{
    binary_name, invocation_path, is_git_subcommand, was_invoked_with_explicit_path,
};

pub(crate) use crate::cli::OutputFormat;

#[cfg(unix)]
use commands::handle_picker;
use commands::repository_ext::RepositoryCliExt;
use commands::worktree::{BranchDeletionMode, handle_no_ff_merge, handle_push};
use commands::{
    MergeOptions, OperationMode, RebaseResult, RemoveTarget, SquashResult, SwitchOptions,
    add_approvals, clear_approvals, handle_claude_install, handle_claude_install_statusline,
    handle_claude_uninstall, handle_completions, handle_config_create, handle_config_show,
    handle_config_update, handle_configure_shell, handle_hints_clear, handle_hints_get,
    handle_hook_show, handle_init, handle_list, handle_logs_get, handle_merge,
    handle_opencode_install, handle_opencode_uninstall, handle_promote, handle_rebase,
    handle_show_theme, handle_squash, handle_state_clear, handle_state_clear_all, handle_state_get,
    handle_state_set, handle_state_show, handle_switch, handle_unconfigure_shell,
    handle_vars_clear, handle_vars_get, handle_vars_list, handle_vars_set, resolve_worktree_arg,
    run_hook, step_commit, step_copy_ignored, step_diff, step_eval, step_for_each, step_prune,
    step_relocate,
};
use output::handle_remove_output;

use cli::{
    ApprovalsCommand, CiStatusAction, Cli, Commands, ConfigCommand, ConfigPluginsClaudeCommand,
    ConfigPluginsCommand, ConfigPluginsOpencodeCommand, ConfigShellCommand, DefaultBranchAction,
    HintsAction, HookCommand, ListArgs, ListSubcommand, LogsAction, MarkerAction, MergeArgs,
    PreviousBranchAction, RemoveArgs, StateCommand, StepCommand, SwitchArgs, SwitchFormat,
    VarsAction,
};
use worktrunk::HookType;

/// Enhance clap errors with command-specific hints, then exit.
///
/// For unrecognized subcommands that match nested commands, suggests the full path.
fn enhance_and_exit_error(err: clap::Error) -> ! {
    // For unrecognized subcommands, check if they match a nested subcommand
    // e.g., `wt squash` -> suggest `wt step squash`
    if err.kind() == ClapErrorKind::InvalidSubcommand
        && let Some(unknown) = err.get(clap::error::ContextKind::InvalidSubcommand)
    {
        let cmd = cli::build_command();
        if let Some(suggestion) = cli::suggest_nested_subcommand(&cmd, &unknown.to_string()) {
            ceprintln!(
                "{}
  <yellow>tip:</>  perhaps <cyan,bold>{suggestion}</cyan,bold>?",
                err.render().ansi()
            );
            process::exit(2);
        }
    }

    // Note: `wt switch` without arguments now opens the interactive picker,
    // so this error enhancement is no longer triggered for that case.

    err.exit()
}

#[cfg(not(unix))]
fn print_windows_picker_unavailable() {
    eprintln!(
        "{}",
        error_message("Interactive picker is not available on Windows")
    );
    eprintln!(
        "{}",
        hint_message(cformat!("Specify a branch: <underline>wt switch BRANCH</>"))
    );
}

fn flag_pair(positive: bool, negative: bool) -> Option<bool> {
    match (positive, negative) {
        (true, _) => Some(true),
        (_, true) => Some(false),
        _ => None,
    }
}

fn run_non_toggle_hook(
    hook_type: HookType,
    yes: bool,
    dry_run: bool,
    names: &[String],
    vars: &[(String, String)],
) -> anyhow::Result<()> {
    run_hook(hook_type, yes, None, dry_run, names, vars)
}

fn run_toggleable_hook(
    hook_type: HookType,
    yes: bool,
    dry_run: bool,
    foreground: bool,
    names: &[String],
    vars: &[(String, String)],
) -> anyhow::Result<()> {
    run_hook(hook_type, yes, Some(foreground), dry_run, names, vars)
}

fn warn_select_deprecated() {
    eprintln!(
        "{}",
        warning_message("wt select is deprecated; use wt switch instead")
    );
}

/// Resolve the `--no-hooks` / `--no-verify` pair: emit a deprecation warning
/// if the old flag was used, then return the effective verify value.
fn resolve_verify(verify: bool, no_verify_deprecated: bool) -> bool {
    if no_verify_deprecated {
        eprintln!(
            "{}",
            warning_message("--no-verify is deprecated; use --no-hooks instead")
        );
        false
    } else {
        verify
    }
}

fn handle_hook_command(action: HookCommand) -> anyhow::Result<()> {
    match action {
        HookCommand::Show {
            hook_type,
            expanded,
        } => handle_hook_show(hook_type.as_deref(), expanded),
        HookCommand::PreSwitch {
            name,
            yes,
            dry_run,
            vars,
        } => run_non_toggle_hook(HookType::PreSwitch, yes, dry_run, &name, &vars),
        HookCommand::PostSwitch {
            name,
            yes,
            dry_run,
            foreground,
            vars,
        } => run_toggleable_hook(HookType::PostSwitch, yes, dry_run, foreground, &name, &vars),
        HookCommand::PreStart {
            name,
            yes,
            dry_run,
            vars,
        } => run_non_toggle_hook(HookType::PreStart, yes, dry_run, &name, &vars),
        HookCommand::PostStart {
            name,
            yes,
            dry_run,
            foreground,
            vars,
        } => run_toggleable_hook(HookType::PostStart, yes, dry_run, foreground, &name, &vars),
        HookCommand::PreCommit {
            name,
            yes,
            dry_run,
            vars,
        } => run_non_toggle_hook(HookType::PreCommit, yes, dry_run, &name, &vars),
        HookCommand::PostCommit {
            name,
            yes,
            dry_run,
            foreground,
            vars,
        } => run_toggleable_hook(HookType::PostCommit, yes, dry_run, foreground, &name, &vars),
        HookCommand::PreMerge {
            name,
            yes,
            dry_run,
            vars,
        } => run_non_toggle_hook(HookType::PreMerge, yes, dry_run, &name, &vars),
        HookCommand::PostMerge {
            name,
            yes,
            dry_run,
            foreground,
            vars,
        } => run_toggleable_hook(HookType::PostMerge, yes, dry_run, foreground, &name, &vars),
        HookCommand::PreRemove {
            name,
            yes,
            dry_run,
            vars,
        } => run_non_toggle_hook(HookType::PreRemove, yes, dry_run, &name, &vars),
        HookCommand::PostRemove {
            name,
            yes,
            dry_run,
            foreground,
            vars,
        } => run_hook(
            HookType::PostRemove,
            yes,
            Some(foreground),
            dry_run,
            &name,
            &vars,
        ),
        HookCommand::RunPipeline => commands::run_pipeline(),
        HookCommand::Approvals { action } => match action {
            ApprovalsCommand::Add { all } => add_approvals(all),
            ApprovalsCommand::Clear { global } => clear_approvals(global),
        },
    }
}

fn handle_step_command(action: StepCommand) -> anyhow::Result<()> {
    match action {
        StepCommand::Commit(args) => {
            let verify = resolve_verify(args.verify, args.no_verify_deprecated);
            step_commit(args.branch, args.yes, verify, args.stage, args.show_prompt)
        }
        StepCommand::Squash(args) => {
            let verify = resolve_verify(args.verify, args.no_verify_deprecated);
            // Handle --show-prompt early: just build and output the prompt
            if args.show_prompt {
                commands::step_show_squash_prompt(args.target.as_deref())
            } else {
                // Approval is handled inside handle_squash (like step_commit)
                handle_squash(args.target.as_deref(), args.yes, verify, args.stage).map(|result| {
                    match result {
                        SquashResult::Squashed | SquashResult::NoNetChanges => {}
                        SquashResult::NoCommitsAhead(branch) => {
                            eprintln!(
                                "{}",
                                info_message(format!(
                                    "Nothing to squash; no commits ahead of {branch}"
                                ))
                            );
                        }
                        SquashResult::AlreadySingleCommit => {
                            eprintln!(
                                "{}",
                                info_message("Nothing to squash; already a single commit")
                            );
                        }
                    }
                })
            }
        }
        StepCommand::Push { target, no_ff, .. } => {
            if no_ff {
                let repo = Repository::current()?;
                let current_branch = repo.require_current_branch("step push --no-ff")?;
                handle_no_ff_merge(target.as_deref(), None, &current_branch)
            } else {
                handle_push(target.as_deref(), "Pushed to", None)
            }
        }
        StepCommand::Rebase { target } => {
            handle_rebase(target.as_deref()).map(|result| match result {
                RebaseResult::Rebased => (),
                RebaseResult::UpToDate(branch) => {
                    eprintln!(
                        "{}",
                        info_message(cformat!("Already up to date with <bold>{branch}</>"))
                    );
                }
            })
        }
        StepCommand::Diff { target, extra_args } => step_diff(target.as_deref(), &extra_args),
        StepCommand::CopyIgnored {
            from,
            to,
            dry_run,
            force,
        } => step_copy_ignored(from.as_deref(), to.as_deref(), dry_run, force),
        StepCommand::Eval { template, dry_run } => step_eval(&template, dry_run),
        StepCommand::ForEach { format, args } => step_for_each(args, format),
        StepCommand::Promote { branch } => {
            handle_promote(branch.as_deref()).map(|result| match result {
                commands::PromoteResult::Promoted => (),
                commands::PromoteResult::AlreadyInMain(branch) => {
                    eprintln!(
                        "{}",
                        info_message(cformat!(
                            "Branch <bold>{branch}</> is already in main worktree"
                        ))
                    );
                }
            })
        }
        StepCommand::Prune {
            dry_run,
            yes,
            min_age,
            foreground,
            format,
        } => step_prune(dry_run, yes, &min_age, foreground, format),
        StepCommand::Relocate {
            branches,
            dry_run,
            commit,
            clobber,
        } => step_relocate(branches, dry_run, commit, clobber),
        StepCommand::External(args) => {
            commands::AliasOptions::parse(args).and_then(commands::step_alias)
        }
    }
}

fn handle_state_command(action: StateCommand) -> anyhow::Result<()> {
    match action {
        StateCommand::DefaultBranch { action } => match action {
            Some(DefaultBranchAction::Get) | None => {
                handle_state_get("default-branch", None, SwitchFormat::Text)
            }
            Some(DefaultBranchAction::Set { branch }) => {
                handle_state_set("default-branch", branch, None)
            }
            Some(DefaultBranchAction::Clear) => handle_state_clear("default-branch", None, false),
        },
        StateCommand::PreviousBranch { action } => match action {
            Some(PreviousBranchAction::Get) | None => {
                handle_state_get("previous-branch", None, SwitchFormat::Text)
            }
            Some(PreviousBranchAction::Set { branch }) => {
                handle_state_set("previous-branch", branch, None)
            }
            Some(PreviousBranchAction::Clear) => handle_state_clear("previous-branch", None, false),
        },
        StateCommand::CiStatus { action } => match action {
            Some(CiStatusAction::Get { branch, format }) => {
                handle_state_get("ci-status", branch, format)
            }
            None => handle_state_get("ci-status", None, SwitchFormat::Text),
            Some(CiStatusAction::Clear { branch, all }) => {
                handle_state_clear("ci-status", branch, all)
            }
        },
        StateCommand::Marker { action } => match action {
            Some(MarkerAction::Get { branch, format }) => {
                handle_state_get("marker", branch, format)
            }
            None => handle_state_get("marker", None, SwitchFormat::Text),
            Some(MarkerAction::Set { value, branch }) => handle_state_set("marker", value, branch),
            Some(MarkerAction::Clear { branch, all }) => handle_state_clear("marker", branch, all),
        },
        StateCommand::Logs { action } => match action {
            Some(LogsAction::Get {
                hook,
                branch,
                format,
            }) => handle_logs_get(hook, branch, format),
            None => handle_logs_get(None, None, SwitchFormat::Text),
            Some(LogsAction::Clear) => handle_state_clear("logs", None, false),
        },
        StateCommand::Hints { action } => match action {
            Some(HintsAction::Get { format }) => handle_hints_get(format),
            None => handle_hints_get(SwitchFormat::Text),
            Some(HintsAction::Clear { name }) => handle_hints_clear(name),
        },
        StateCommand::Vars { action } => match action {
            VarsAction::Get { key, branch } => handle_vars_get(&key, branch),
            VarsAction::Set {
                assignment: (key, value),
                branch,
            } => handle_vars_set(&key, &value, branch),
            VarsAction::List { branch, format } => handle_vars_list(branch, format),
            VarsAction::Clear { key, all, branch } => {
                handle_vars_clear(key.as_deref(), all, branch)
            }
        },
        StateCommand::Get { format } => handle_state_show(format),
        StateCommand::Clear => handle_state_clear_all(),
    }
}

fn handle_config_shell_command(action: ConfigShellCommand) -> anyhow::Result<()> {
    match action {
        ConfigShellCommand::Init { shell, cmd } => {
            // Generate shell code to stdout
            let cmd = cmd.unwrap_or_else(binary_name);
            handle_init(shell, cmd).map_err(|e| anyhow::anyhow!("{}", e))
        }
        ConfigShellCommand::Install {
            shell,
            yes,
            dry_run,
            cmd,
        } => {
            // Auto-write to shell config files and completions
            let cmd = cmd.unwrap_or_else(binary_name);
            handle_configure_shell(shell, yes, dry_run, cmd)
                .map_err(|e| anyhow::anyhow!("{}", e))
                .and_then(|scan_result| {
                    // Exit with error if no shells configured
                    // Show skipped shells first so user knows what was tried
                    if scan_result.configured.is_empty() {
                        crate::output::print_skipped_shells(&scan_result.skipped)?;
                        return Err(worktrunk::git::GitError::Other {
                            message: "No shell config files found".into(),
                        }
                        .into());
                    }
                    // For --dry-run, preview was already shown by handler
                    if dry_run {
                        return Ok(());
                    }
                    crate::output::print_shell_install_result(&scan_result)
                })
        }
        ConfigShellCommand::Uninstall {
            shell,
            yes,
            dry_run,
        } => {
            let explicit_shell = shell.is_some();
            handle_unconfigure_shell(shell, yes, dry_run, &binary_name())
                .map_err(|e| anyhow::anyhow!("{}", e))
                .map(|result| {
                    if !dry_run {
                        crate::output::print_shell_uninstall_result(&result, explicit_shell);
                    }
                })
        }
        ConfigShellCommand::ShowTheme => {
            handle_show_theme();
            Ok(())
        }
        ConfigShellCommand::Completions { shell } => handle_completions(shell),
    }
}

fn handle_config_command(action: ConfigCommand) -> anyhow::Result<()> {
    match action {
        ConfigCommand::Shell { action } => handle_config_shell_command(action),
        ConfigCommand::Create { project } => handle_config_create(project),
        ConfigCommand::Show { full, format } => handle_config_show(full, format),
        ConfigCommand::Update { yes } => handle_config_update(yes),
        ConfigCommand::Plugins { action } => handle_plugins_command(action),
        ConfigCommand::State { action } => handle_state_command(action),
    }
}

fn handle_plugins_command(action: ConfigPluginsCommand) -> anyhow::Result<()> {
    match action {
        ConfigPluginsCommand::Claude { action } => match action {
            ConfigPluginsClaudeCommand::Install { yes } => handle_claude_install(yes),
            ConfigPluginsClaudeCommand::Uninstall { yes } => handle_claude_uninstall(yes),
            ConfigPluginsClaudeCommand::InstallStatusline { yes } => {
                handle_claude_install_statusline(yes)
            }
        },
        ConfigPluginsCommand::Opencode { action } => match action {
            ConfigPluginsOpencodeCommand::Install { yes } => handle_opencode_install(yes),
            ConfigPluginsOpencodeCommand::Uninstall { yes } => handle_opencode_uninstall(yes),
        },
    }
}

fn handle_list_command(args: ListArgs) -> anyhow::Result<()> {
    match args.subcommand {
        Some(ListSubcommand::Statusline {
            format,
            claude_code,
        }) => {
            // Hidden --claude-code flag only applies when format is default (Table)
            // Explicit --format=json takes precedence over --claude-code
            let effective_format = if claude_code && matches!(format, OutputFormat::Table) {
                OutputFormat::ClaudeCode
            } else {
                format
            };
            commands::statusline::run(effective_format)
        }
        None => {
            let (repo, _recovered) = current_or_recover()?;
            let render_mode = RenderMode::detect(flag_pair(args.progressive, args.no_progressive));
            handle_list(
                repo,
                args.format,
                args.branches,
                args.remotes,
                args.full,
                render_mode,
            )
        }
    }
}

#[cfg(unix)]
fn handle_select_command(branches: bool, remotes: bool) -> anyhow::Result<()> {
    // Deprecated: show warning and delegate to handle_picker
    warn_select_deprecated();
    handle_picker(branches, remotes, None)
}

#[cfg(not(unix))]
fn handle_select_command(_branches: bool, _remotes: bool) -> anyhow::Result<()> {
    use worktrunk::git::WorktrunkError;
    warn_select_deprecated();
    print_windows_picker_unavailable();
    Err(WorktrunkError::AlreadyDisplayed { exit_code: 1 }.into())
}

fn handle_switch_command(args: SwitchArgs) -> anyhow::Result<()> {
    let verify = resolve_verify(args.verify, args.no_verify_deprecated);
    UserConfig::load()
        .context("Failed to load config")
        .and_then(|mut config| {
            // No branch argument: open interactive picker
            let change_dir_flag = flag_pair(args.cd, args.no_cd);

            let Some(branch) = args.branch else {
                #[cfg(unix)]
                {
                    return handle_picker(args.branches, args.remotes, change_dir_flag);
                }

                #[cfg(not(unix))]
                {
                    use worktrunk::git::WorktrunkError;
                    // Suppress unused variable warnings on Windows
                    let _ = (args.branches, args.remotes, change_dir_flag);

                    print_windows_picker_unavailable();
                    return Err(WorktrunkError::AlreadyDisplayed { exit_code: 2 }.into());
                }
            };

            handle_switch(
                SwitchOptions {
                    branch: &branch,
                    create: args.create,
                    base: args.base.as_deref(),
                    execute: args.execute.as_deref(),
                    execute_args: &args.execute_args,
                    yes: args.yes,
                    clobber: args.clobber,
                    change_dir: change_dir_flag,
                    verify,
                    format: args.format,
                },
                &mut config,
                &binary_name(),
            )
        })
}

/// Validated removal plans, categorized for ordered execution.
///
/// Multi-worktree removal validates all targets upfront, then executes in order:
/// other worktrees first, branch-only cases next, current worktree last.
struct RemovePlans {
    others: Vec<RemoveResult>,
    branch_only: Vec<RemoveResult>,
    current: Option<RemoveResult>,
    errors: Vec<anyhow::Error>,
}

impl RemovePlans {
    fn has_valid_plans(&self) -> bool {
        !self.others.is_empty() || !self.branch_only.is_empty() || self.current.is_some()
    }

    fn record_error(&mut self, e: anyhow::Error) {
        eprintln!("{}", e);
        self.errors.push(e);
    }
}

/// Validate all removal targets, returning categorized plans.
///
/// Resolves each branch name, determines whether it's the current worktree,
/// another worktree, or branch-only, and prepares the removal plan.
/// Errors are collected (not fatal) to support partial success.
fn validate_remove_targets(
    repo: &Repository,
    branches: Vec<String>,
    config: &UserConfig,
    keep_branch: bool,
    force_delete: bool,
    force: bool,
) -> RemovePlans {
    let current_worktree = repo
        .current_worktree()
        .root()
        .ok()
        .and_then(|p| dunce::canonicalize(&p).ok());

    // Dedupe inputs to avoid redundant planning/execution
    let branches: Vec<_> = {
        let mut seen = HashSet::new();
        branches
            .into_iter()
            .filter(|b| seen.insert(b.clone()))
            .collect()
    };

    let deletion_mode = BranchDeletionMode::from_flags(keep_branch, force_delete);
    let worktrees = repo.list_worktrees().ok();

    let mut plans = RemovePlans {
        others: Vec::new(),
        branch_only: Vec::new(),
        current: None,
        errors: Vec::new(),
    };

    for branch_name in &branches {
        let resolved = match resolve_worktree_arg(repo, branch_name, config, OperationMode::Remove)
        {
            Ok(r) => r,
            Err(e) => {
                plans.record_error(e);
                continue;
            }
        };

        match resolved {
            ResolvedWorktree::Worktree { path, branch } => {
                // Use canonical paths to avoid symlink/normalization mismatches
                let path_canonical = dunce::canonicalize(&path).unwrap_or(path);
                let is_current = current_worktree.as_ref() == Some(&path_canonical);

                if is_current {
                    match repo.prepare_worktree_removal(
                        RemoveTarget::Current,
                        deletion_mode,
                        force,
                        config,
                        None,
                        worktrees.as_deref(),
                    ) {
                        Ok(result) => plans.current = Some(result),
                        Err(e) => plans.record_error(e),
                    }
                    continue;
                }

                // Non-current worktree: remove by branch name, or by path for
                // detached worktrees (which have no branch).
                let target = if let Some(ref branch_name) = branch {
                    RemoveTarget::Branch(branch_name)
                } else {
                    RemoveTarget::Path(&path_canonical)
                };
                match repo.prepare_worktree_removal(
                    target,
                    deletion_mode,
                    force,
                    config,
                    None,
                    worktrees.as_deref(),
                ) {
                    Ok(result) => plans.others.push(result),
                    Err(e) => plans.record_error(e),
                }
            }
            ResolvedWorktree::BranchOnly { branch } => {
                match repo.prepare_worktree_removal(
                    RemoveTarget::Branch(&branch),
                    deletion_mode,
                    force,
                    config,
                    None,
                    worktrees.as_deref(),
                ) {
                    Ok(result) => plans.branch_only.push(result),
                    Err(e) => plans.record_error(e),
                }
            }
        }
    }

    plans
}

/// Entry point for the `wt remove` command.
///
/// # Command flow
///
/// 1. **Validate** all target worktrees up front via `prepare_worktree_removal`
///    (clean check, branch-deletion-safety check, force-flag handling).
/// 2. **Approve hooks** (`pre-remove`, `post-remove`, `post-switch`) if
///    running interactively and any hooks are configured.
/// 3. **Dispatch to `handle_remove_output`** per target. For each, the output
///    handler runs `pre-remove` hooks in the worktree, then either:
///    - **Foreground** (`--foreground`): stop fsmonitor → rename into
///      `.git/wt/trash/<name>-<timestamp>/` → prune metadata → delete branch
///      → synchronous `remove_dir_all` on the staged directory.
///    - **Background** (default): stop fsmonitor → rename + prune +
///      synchronous branch delete → spawn detached `rm -rf` on the staged
///      directory. Cross-filesystem or locked worktrees fall back to
///      `git worktree remove` in the detached process.
/// 4. **Post-remove hooks** run in the background after dispatch.
/// 5. **Sweep stale trash** (fire-and-forget, after primary output): entries
///    in `.git/wt/trash/` older than 24 hours are removed by a detached
///    `rm -rf`. Runs last so it never delays the user-visible progress or
///    success message. See [`commands::process::sweep_stale_trash`].
fn handle_remove_command(args: RemoveArgs) -> anyhow::Result<()> {
    let json_mode = args.format == SwitchFormat::Json;
    let verify = resolve_verify(args.verify, args.no_verify_deprecated);
    UserConfig::load()
        .context("Failed to load config")
        .and_then(|config| {
            // Validate conflicting flags
            if !args.delete_branch && args.force_delete {
                return Err(worktrunk::git::GitError::Other {
                    message: "Cannot use --force-delete with --no-delete-branch".into(),
                }
                .into());
            }

            let repo = Repository::current().context("Failed to remove worktree")?;

            // Resolve current worktree context for hook approval
            let current_wt = repo.current_worktree();
            let approve_worktree_path = current_wt.root()?;
            let approve_branch = current_wt
                .branch()
                .context("Failed to determine current branch")?;

            // Helper: approve remove hooks using current worktree context
            // Returns true if hooks should run (user approved)
            let approve_remove = |yes: bool| -> anyhow::Result<bool> {
                let ctx = CommandContext::new(
                    &repo,
                    &config,
                    approve_branch.as_deref(),
                    &approve_worktree_path,
                    yes,
                );
                let approved = approve_hooks(
                    &ctx,
                    &[
                        HookType::PreRemove,
                        HookType::PostRemove,
                        HookType::PostSwitch,
                    ],
                )?;
                if !approved {
                    eprintln!("{}", info_message("Commands declined, continuing removal"));
                }
                Ok(approved)
            };

            let branches = args.branches;

            if branches.is_empty() {
                // Single worktree removal: validate FIRST, then approve, then execute
                let result = repo
                    .prepare_worktree_removal(
                        RemoveTarget::Current,
                        BranchDeletionMode::from_flags(!args.delete_branch, args.force_delete),
                        args.force,
                        &config,
                        None,
                        None,
                    )
                    .context("Failed to remove worktree")?;

                // Early exit for benchmarking time-to-first-output
                if std::env::var_os("WORKTRUNK_FIRST_OUTPUT").is_some() {
                    return Ok(());
                }

                // "Approve at the Gate": approval happens AFTER validation passes
                let run_hooks = verify && approve_remove(args.yes)?;

                handle_remove_output(&result, args.foreground, run_hooks, false, false)?;
                if json_mode {
                    let json = serde_json::json!([result.to_json()]);
                    println!("{}", serde_json::to_string_pretty(&json)?);
                }
                // Fire-and-forget cleanup of stale `.git/wt/trash/` entries —
                // runs after primary output so it never delays the user-visible
                // progress/success message.
                commands::process::sweep_stale_trash(&repo);
                Ok(())
            } else {
                // Multi-worktree removal: validate ALL first, then approve, then execute
                let plans = validate_remove_targets(
                    &repo,
                    branches,
                    &config,
                    !args.delete_branch,
                    args.force_delete,
                    args.force,
                );

                if !plans.has_valid_plans() {
                    anyhow::bail!("");
                }

                // Early exit for benchmarking time-to-first-output
                if std::env::var_os("WORKTRUNK_FIRST_OUTPUT").is_some() {
                    return Ok(());
                }

                // Approve hooks (only if we have valid plans)
                // TODO(pre-remove-context): Approval context uses current worktree,
                // but hooks execute in each target worktree.
                let run_hooks = verify && approve_remove(args.yes)?;

                // Execute all validated plans: others first, branch-only next, current last
                let show_branch =
                    plans.others.len() + plans.branch_only.len() + plans.current.iter().len() > 1;
                for result in &plans.others {
                    handle_remove_output(result, args.foreground, run_hooks, false, show_branch)?;
                }
                for result in &plans.branch_only {
                    handle_remove_output(result, args.foreground, run_hooks, false, show_branch)?;
                }
                if let Some(ref result) = plans.current {
                    handle_remove_output(result, args.foreground, run_hooks, false, show_branch)?;
                }

                if json_mode {
                    let json_items: Vec<serde_json::Value> = plans
                        .others
                        .iter()
                        .chain(&plans.branch_only)
                        .chain(plans.current.as_ref())
                        .map(RemoveResult::to_json)
                        .collect();
                    println!("{}", serde_json::to_string_pretty(&json_items)?);
                }

                // Fire-and-forget cleanup of stale `.git/wt/trash/` entries —
                // runs after primary output so it never delays the user-visible
                // progress/success messages.
                commands::process::sweep_stale_trash(&repo);

                if !plans.errors.is_empty() {
                    anyhow::bail!("");
                }

                Ok(())
            }
        })
}

fn init_rayon_thread_pool() {
    // Configure Rayon's global thread pool for mixed I/O workloads.
    // The `wt list` command runs git operations (CPU + disk I/O) and network
    // requests (CI status, URL health checks) in parallel. Using 2x CPU cores
    // allows threads blocked on I/O to overlap with compute work.
    //
    // Override with RAYON_NUM_THREADS=N for benchmarking.
    let num_threads = if std::env::var_os("RAYON_NUM_THREADS").is_some() {
        0 // Let Rayon handle the env var (includes validation)
    } else {
        std::thread::available_parallelism()
            .map(|n| n.get() * 2)
            .unwrap_or(8)
    };
    let _ = rayon::ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .build_global();
}

fn parse_cli() -> Option<Cli> {
    if completion::maybe_handle_env_completion() {
        return None;
    }

    // Handle --help with pager before clap processes it
    if help::maybe_handle_help_with_pager() {
        return None;
    }

    // TODO: Enhance error messages to show possible values for missing enum arguments
    // Currently `wt config shell init` doesn't show available shells, but `wt config shell init invalid` does.
    // Clap doesn't support this natively yet - see https://github.com/clap-rs/clap/issues/3320
    // When available, use built-in setting. Until then, could use try_parse() to intercept
    // MissingRequiredArgument errors and print custom messages with ValueEnum::value_variants().
    let cmd = cli::build_command();
    let matches = cmd.try_get_matches().unwrap_or_else(|e| {
        enhance_and_exit_error(e);
    });
    Some(Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit()))
}

fn apply_global_options(directory: Option<std::path::PathBuf>, config: Option<std::path::PathBuf>) {
    // Initialize base path from -C flag if provided
    if let Some(path) = directory {
        set_base_path(path);
    }

    // Initialize config path from --config flag if provided
    if let Some(path) = config {
        set_config_path(path);
    }
}

fn init_command_log(command_line: &str) {
    // Initialize command log for always-on logging of hooks and LLM commands.
    // Directory and file are created lazily on first log_command() call.
    if let Ok(repo) = worktrunk::git::Repository::current() {
        worktrunk::command_log::init(&repo.wt_logs_dir(), command_line);
    }
}

fn thread_label() -> char {
    let thread_id = format!("{:?}", std::thread::current().id());
    thread_id
        .strip_prefix("ThreadId(")
        .and_then(|s| s.strip_suffix(")"))
        .and_then(|s| s.parse::<usize>().ok())
        .map(|n| {
            if n == 0 {
                '0'
            } else if n <= 26 {
                char::from(b'a' + (n - 1) as u8)
            } else if n <= 52 {
                char::from(b'A' + (n - 27) as u8)
            } else {
                '?'
            }
        })
        .unwrap_or('?')
}

fn init_logging(verbose_level: u8) {
    // Configure logging based on --verbose flag or RUST_LOG env var
    // When -vv is set, also write logs to .git/wt/logs/verbose.log
    if verbose_level >= 2 {
        verbose_log::init();
    }

    // Set global verbosity level for styled verbose output
    output::set_verbosity(verbose_level);

    let mut builder = if verbose_level >= 2 {
        let mut b = env_logger::Builder::new();
        b.filter_level(log::LevelFilter::Debug);
        b
    } else {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("off"))
    };

    builder
        .format(|buf, record| {
            let msg = record.args().to_string();
            let thread_num = thread_label();

            // Write plain text to log file (no ANSI codes)
            verbose_log::write_line(&format!("[{thread_num}] {msg}"));

            // Commands start with $, make only the command bold (not $ or [worktree])
            if let Some(rest) = msg.strip_prefix("$ ") {
                // Split: "git command [worktree]" -> ("git command", " [worktree]")
                if let Some(bracket_pos) = rest.find(" [") {
                    let command = &rest[..bracket_pos];
                    let worktree = &rest[bracket_pos..];
                    writeln!(
                        buf,
                        "{}",
                        cformat!("<dim>[{thread_num}]</> $ <bold>{command}</>{worktree}")
                    )
                } else {
                    writeln!(
                        buf,
                        "{}",
                        cformat!("<dim>[{thread_num}]</> $ <bold>{rest}</>")
                    )
                }
            } else if msg.starts_with("  ! ") {
                // Error output - show in red
                writeln!(buf, "{}", cformat!("<dim>[{thread_num}]</> <red>{msg}</>"))
            } else {
                // Regular output with thread ID
                writeln!(buf, "{}", cformat!("<dim>[{thread_num}]</> {msg}"))
            }
        })
        .init();
}

fn handle_merge_command(args: MergeArgs) -> anyhow::Result<()> {
    if args.no_verify {
        eprintln!(
            "{}",
            warning_message("--no-verify is deprecated; use --no-hooks instead")
        );
    }
    handle_merge(MergeOptions {
        target: args.target.as_deref(),
        squash: flag_pair(args.squash, args.no_squash),
        commit: flag_pair(args.commit, args.no_commit),
        rebase: flag_pair(args.rebase, args.no_rebase),
        remove: flag_pair(args.remove, args.no_remove),
        ff: flag_pair(args.ff, args.no_ff),
        verify: flag_pair(args.verify, args.no_hooks || args.no_verify),
        yes: args.yes,
        stage: args.stage,
        format: args.format,
    })
}

fn dispatch_command(command: Commands) -> anyhow::Result<()> {
    match command {
        Commands::Config { action } => handle_config_command(action),
        Commands::Step { action } => handle_step_command(action),
        Commands::Hook { action } => handle_hook_command(action),
        Commands::Select { branches, remotes } => handle_select_command(branches, remotes),
        Commands::List(args) => handle_list_command(args),
        Commands::Switch(args) => handle_switch_command(args),
        Commands::Remove(args) => handle_remove_command(args),
        Commands::Merge(args) => handle_merge_command(args),
    }
}

fn print_command_error(error: &anyhow::Error) {
    // GitError, WorktrunkError, and HookErrorWithHint produce styled output via Display.
    // Some variants (AlreadyDisplayed, CommandNotApproved) have empty Display impls —
    // skip eprintln! for those to avoid phantom blank lines.
    if let Some(err) = error.downcast_ref::<worktrunk::git::GitError>() {
        eprintln!("{}", err);
    } else if let Some(err) = error.downcast_ref::<worktrunk::git::WorktrunkError>() {
        let display = err.to_string();
        if !display.is_empty() {
            eprintln!("{display}");
        }
    } else if let Some(err) = error.downcast_ref::<worktrunk::git::HookErrorWithHint>() {
        eprintln!("{}", err);
    } else if let Some(err) = error.downcast_ref::<worktrunk::config::TemplateExpandError>() {
        eprintln!("{}", err);
    } else {
        // Anyhow error formatting:
        // - With context: show context as header, root cause in gutter
        // - Simple error: inline with emoji
        // - Empty error: skip (errors already printed elsewhere)
        let msg = error.to_string();
        if !msg.is_empty() {
            let chain: Vec<String> = error.chain().skip(1).map(|e| e.to_string()).collect();
            if !chain.is_empty() {
                eprintln!("{}", error_message(&msg));
                let chain_text = chain.join("\n");
                eprintln!("{}", format_with_gutter(&chain_text, None));
            } else if msg.contains('\n') || msg.contains('\r') {
                debug_assert!(false, "Multiline error without context: {msg}");
                log::warn!("Multiline error without context: {msg}");
                let normalized = msg.replace("\r\n", "\n").replace('\r', "\n");
                eprintln!("{}", error_message("Command failed"));
                eprintln!("{}", format_with_gutter(&normalized, None));
            } else {
                eprintln!("{}", error_message(&msg));
            }
        }
    }
}

fn print_cwd_removed_hint_if_needed() {
    // If the CWD has been deleted, hint the user about recovery options.
    // Check both: (1) explicit flag set by merge/remove when it knows the CWD
    // worktree was removed (reliable on all platforms), and (2) OS-level detection
    // for cases not covered by the flag (e.g., external worktree removal).
    let cwd_gone = output::was_cwd_removed() || std::env::current_dir().is_err();
    if cwd_gone {
        if let Some(hint) = cwd_removed_hint() {
            eprintln!("{}", hint_message(hint));
        } else {
            eprintln!("{}", info_message("Current directory was removed"));
        }
    }
}

fn finish_command(verbose_level: u8, command_line: &str, error: Option<&anyhow::Error>) {
    let error_text = error.map(|err| err.to_string());
    diagnostic::write_if_verbose(verbose_level, command_line, error_text.as_deref());
    let _ = output::terminate_output();
}

fn handle_command_failure(error: anyhow::Error, verbose_level: u8, command_line: &str) -> ! {
    print_command_error(&error);
    print_cwd_removed_hint_if_needed();

    // Preserve exit code from child processes (especially for signals like SIGINT)
    let code = exit_code(&error).unwrap_or(1);
    finish_command(verbose_level, command_line, Some(&error));
    process::exit(code);
}

fn print_help_to_stderr() {
    // No subcommand provided - print help to stderr (stdout is eval'd by shell wrapper)
    let mut cmd = cli::build_command();
    let help = cmd.render_help().ansi().to_string();
    eprintln!("{help}");
}

fn main() {
    // Capture the startup working directory before anything else. This is
    // used by shell_exec to resolve relative `GIT_*` path variables inherited
    // from a parent `git` (e.g. when invoked via `git wt ...` with
    // `alias.wt = "!wt"`) against a stable reference, rather than against
    // each child command's `current_dir`. See issue #1914.
    worktrunk::shell_exec::init_startup_cwd();

    init_rayon_thread_pool();

    // Tell crossterm to always emit ANSI sequences
    crossterm::style::force_color_output(true);

    let Some(cli) = parse_cli() else {
        return;
    };

    let Cli {
        directory,
        config,
        verbose,
        command,
    } = cli;
    apply_global_options(directory, config);

    let command_line = std::env::args().collect::<Vec<_>>().join(" ");
    init_command_log(&command_line);
    init_logging(verbose);

    let Some(command) = command else {
        print_help_to_stderr();
        return;
    };

    match dispatch_command(command) {
        Ok(()) => finish_command(verbose, &command_line, None),
        Err(error) => handle_command_failure(error, verbose, &command_line),
    }
}
