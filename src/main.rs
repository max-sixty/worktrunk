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
use commands::context::CommandEnv;
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
use commands::handle_select;
use commands::worktree::handle_push;
use commands::{
    MergeOptions, OperationMode, RebaseResult, SquashResult, SwitchOptions, add_approvals,
    clear_approvals, handle_completions, handle_config_create, handle_config_show,
    handle_config_update, handle_configure_shell, handle_hints_clear, handle_hints_get,
    handle_hook_show, handle_init, handle_list, handle_logs_get, handle_merge, handle_promote,
    handle_rebase, handle_remove, handle_remove_current, handle_show_theme, handle_squash,
    handle_state_clear, handle_state_clear_all, handle_state_get, handle_state_set,
    handle_state_show, handle_switch, handle_unconfigure_shell, resolve_worktree_arg, run_hook,
    step_commit, step_copy_ignored, step_diff, step_for_each, step_prune, step_relocate,
};
use output::handle_remove_output;

use cli::{
    ApprovalsCommand, CiStatusAction, Cli, Commands, ConfigCommand, ConfigShellCommand,
    DefaultBranchAction, HintsAction, HookCommand, ListSubcommand, LogsAction, MarkerAction,
    PreviousBranchAction, StateCommand, StepCommand,
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
    name: Option<&str>,
    vars: &[(String, String)],
) -> anyhow::Result<()> {
    run_hook(hook_type, yes, None, dry_run, name, vars)
}

fn run_toggleable_hook(
    hook_type: HookType,
    yes: bool,
    dry_run: bool,
    foreground: bool,
    name: Option<&str>,
    vars: &[(String, String)],
) -> anyhow::Result<()> {
    run_hook(hook_type, yes, Some(foreground), dry_run, name, vars)
}

fn warn_select_deprecated() {
    eprintln!(
        "{}",
        warning_message("wt select is deprecated; use wt switch instead")
    );
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
        } => run_non_toggle_hook(HookType::PreSwitch, yes, dry_run, name.as_deref(), &vars),
        HookCommand::PostCreate {
            name,
            yes,
            dry_run,
            vars,
        } => run_non_toggle_hook(HookType::PostCreate, yes, dry_run, name.as_deref(), &vars),
        HookCommand::PostStart {
            name,
            yes,
            dry_run,
            foreground,
            vars,
        } => run_toggleable_hook(
            HookType::PostStart,
            yes,
            dry_run,
            foreground,
            name.as_deref(),
            &vars,
        ),
        HookCommand::PostSwitch {
            name,
            yes,
            dry_run,
            foreground,
            vars,
        } => run_toggleable_hook(
            HookType::PostSwitch,
            yes,
            dry_run,
            foreground,
            name.as_deref(),
            &vars,
        ),
        HookCommand::PreCommit {
            name,
            yes,
            dry_run,
            vars,
        } => run_non_toggle_hook(HookType::PreCommit, yes, dry_run, name.as_deref(), &vars),
        HookCommand::PreMerge {
            name,
            yes,
            dry_run,
            vars,
        } => run_non_toggle_hook(HookType::PreMerge, yes, dry_run, name.as_deref(), &vars),
        HookCommand::PostMerge {
            name,
            yes,
            dry_run,
            vars,
        } => run_non_toggle_hook(HookType::PostMerge, yes, dry_run, name.as_deref(), &vars),
        HookCommand::PreRemove {
            name,
            yes,
            dry_run,
            vars,
        } => run_non_toggle_hook(HookType::PreRemove, yes, dry_run, name.as_deref(), &vars),
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
            name.as_deref(),
            &vars,
        ),
        HookCommand::Approvals { action } => match action {
            ApprovalsCommand::Add { all } => add_approvals(all),
            ApprovalsCommand::Clear { global } => clear_approvals(global),
        },
    }
}

fn handle_step_command(action: StepCommand) -> anyhow::Result<()> {
    match action {
        StepCommand::Commit {
            yes,
            verify,
            stage,
            show_prompt,
        } => step_commit(yes, verify, stage, show_prompt),
        StepCommand::Squash {
            target,
            yes,
            verify,
            stage,
            show_prompt,
        } => {
            // Handle --show-prompt early: just build and output the prompt
            if show_prompt {
                commands::step_show_squash_prompt(target.as_deref())
            } else {
                // Approval is handled inside handle_squash (like step_commit)
                handle_squash(target.as_deref(), yes, verify, stage).map(|result| match result {
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
                })
            }
        }
        StepCommand::Push { target } => handle_push(target.as_deref(), "Pushed to", None),
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
        StepCommand::ForEach { args } => step_for_each(args),
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
        } => step_prune(dry_run, yes, &min_age, foreground),
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
            Some(DefaultBranchAction::Get) | None => handle_state_get("default-branch", None),
            Some(DefaultBranchAction::Set { branch }) => {
                handle_state_set("default-branch", branch, None)
            }
            Some(DefaultBranchAction::Clear) => handle_state_clear("default-branch", None, false),
        },
        StateCommand::PreviousBranch { action } => match action {
            Some(PreviousBranchAction::Get) | None => handle_state_get("previous-branch", None),
            Some(PreviousBranchAction::Set { branch }) => {
                handle_state_set("previous-branch", branch, None)
            }
            Some(PreviousBranchAction::Clear) => handle_state_clear("previous-branch", None, false),
        },
        StateCommand::CiStatus { action } => match action {
            Some(CiStatusAction::Get { branch }) => handle_state_get("ci-status", branch),
            None => handle_state_get("ci-status", None),
            Some(CiStatusAction::Clear { branch, all }) => {
                handle_state_clear("ci-status", branch, all)
            }
        },
        StateCommand::Marker { action } => match action {
            Some(MarkerAction::Get { branch }) => handle_state_get("marker", branch),
            None => handle_state_get("marker", None),
            Some(MarkerAction::Set { value, branch }) => handle_state_set("marker", value, branch),
            Some(MarkerAction::Clear { branch, all }) => handle_state_clear("marker", branch, all),
        },
        StateCommand::Logs { action } => match action {
            Some(LogsAction::Get { hook, branch }) => handle_logs_get(hook, branch),
            None => handle_logs_get(None, None),
            Some(LogsAction::Clear) => handle_state_clear("logs", None, false),
        },
        StateCommand::Hints { action } => match action {
            Some(HintsAction::Get) | None => handle_hints_get(),
            Some(HintsAction::Clear { name }) => handle_hints_clear(name),
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
        ConfigCommand::Show { full } => handle_config_show(full),
        ConfigCommand::Update { yes } => handle_config_update(yes),
        ConfigCommand::State { action } => handle_state_command(action),
    }
}

fn handle_list_command(
    subcommand: Option<ListSubcommand>,
    format: OutputFormat,
    branches: bool,
    remotes: bool,
    full: bool,
    progressive: bool,
    no_progressive: bool,
) -> anyhow::Result<()> {
    match subcommand {
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
            let render_mode = RenderMode::detect(flag_pair(progressive, no_progressive));
            handle_list(repo, format, branches, remotes, full, render_mode)
        }
    }
}

#[cfg(unix)]
fn handle_select_command(branches: bool, remotes: bool) -> anyhow::Result<()> {
    // Deprecated: show warning and delegate to handle_select
    warn_select_deprecated();
    handle_select(branches, remotes, true)
}

#[cfg(not(unix))]
fn handle_select_command(_branches: bool, _remotes: bool) -> anyhow::Result<()> {
    warn_select_deprecated();
    print_windows_picker_unavailable();
    std::process::exit(1);
}

struct SwitchCommandArgs {
    branch: Option<String>,
    branches: bool,
    remotes: bool,
    create: bool,
    base: Option<String>,
    execute: Option<String>,
    execute_args: Vec<String>,
    yes: bool,
    clobber: bool,
    no_cd: bool,
    verify: bool,
}

fn handle_switch_command(spec: SwitchCommandArgs) -> anyhow::Result<()> {
    UserConfig::load()
        .context("Failed to load config")
        .and_then(|mut config| {
            // No branch argument: open interactive picker
            let Some(branch) = spec.branch else {
                #[cfg(unix)]
                {
                    return handle_select(spec.branches, spec.remotes, !spec.no_cd);
                }

                #[cfg(not(unix))]
                {
                    // Suppress unused variable warnings on Windows
                    let _ = (spec.branches, spec.remotes);

                    print_windows_picker_unavailable();
                    std::process::exit(2);
                }
            };

            handle_switch(
                SwitchOptions {
                    branch: &branch,
                    create: spec.create,
                    base: spec.base.as_deref(),
                    execute: spec.execute.as_deref(),
                    execute_args: &spec.execute_args,
                    yes: spec.yes,
                    clobber: spec.clobber,
                    change_dir: !spec.no_cd,
                    verify: spec.verify,
                },
                &mut config,
                &binary_name(),
            )
        })
}

struct RemoveCommandArgs {
    branches: Vec<String>,
    delete_branch: bool,
    force_delete: bool,
    foreground: bool,
    verify: bool,
    yes: bool,
    force: bool,
}

fn handle_remove_command(spec: RemoveCommandArgs) -> anyhow::Result<()> {
    UserConfig::load()
        .context("Failed to load config")
        .and_then(|config| {
            // Validate conflicting flags
            if !spec.delete_branch && spec.force_delete {
                return Err(worktrunk::git::GitError::Other {
                    message: "Cannot use --force-delete with --no-delete-branch".into(),
                }
                .into());
            }

            let repo = Repository::current().context("Failed to remove worktree")?;

            // Helper: approve remove hooks using current worktree context
            // Returns true if hooks should run (user approved)
            let approve_remove = |yes: bool| -> anyhow::Result<bool> {
                let env = CommandEnv::for_action_branchless()?;
                let ctx = env.context(yes);
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

            let branches = spec.branches;

            if branches.is_empty() {
                // Single worktree removal: validate FIRST, then approve, then execute
                let result = handle_remove_current(
                    !spec.delete_branch,
                    spec.force_delete,
                    spec.force,
                    &config,
                )
                .context("Failed to remove worktree")?;

                // Early exit for benchmarking time-to-first-output
                if std::env::var_os("WORKTRUNK_FIRST_OUTPUT").is_some() {
                    return Ok(());
                }

                // "Approve at the Gate": approval happens AFTER validation passes
                let run_hooks = spec.verify && approve_remove(spec.yes)?;

                handle_remove_output(&result, spec.foreground, run_hooks, false)
            } else {
                // Multi-worktree removal: validate ALL first, then approve, then execute
                // This supports partial success - some may fail validation while others succeed.
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

                // Phase 1: Validate all targets (resolution + preparation)
                // Store successful plans for execution after approval
                let mut plans_others: Vec<RemoveResult> = Vec::new();
                let mut plans_branch_only: Vec<RemoveResult> = Vec::new();
                let mut plan_current: Option<RemoveResult> = None;
                let mut all_errors: Vec<anyhow::Error> = Vec::new();

                // Helper: record error and continue
                let mut record_error = |e: anyhow::Error| {
                    eprintln!("{}", e);
                    all_errors.push(e);
                };

                for branch_name in &branches {
                    // Resolve the target
                    let resolved = match resolve_worktree_arg(
                        &repo,
                        branch_name,
                        &config,
                        OperationMode::Remove,
                    ) {
                        Ok(r) => r,
                        Err(e) => {
                            record_error(e);
                            continue;
                        }
                    };

                    match resolved {
                        ResolvedWorktree::Worktree { path, branch } => {
                            // Use canonical paths to avoid symlink/normalization mismatches
                            let path_canonical = dunce::canonicalize(&path).unwrap_or(path);
                            let is_current = current_worktree.as_ref() == Some(&path_canonical);

                            if is_current {
                                // Current worktree - use handle_remove_current for detached HEAD
                                match handle_remove_current(
                                    !spec.delete_branch,
                                    spec.force_delete,
                                    spec.force,
                                    &config,
                                ) {
                                    Ok(result) => plan_current = Some(result),
                                    Err(e) => record_error(e),
                                }
                                continue;
                            }

                            // Non-current worktree - branch is always Some because:
                            // - "@" resolves to current worktree (handled by is_current branch above)
                            // - Other names resolve via resolve_worktree_arg which always sets branch: Some(...)
                            let branch_for_remove = branch.as_ref().unwrap();

                            match handle_remove(
                                branch_for_remove,
                                !spec.delete_branch,
                                spec.force_delete,
                                spec.force,
                                &config,
                            ) {
                                Ok(result) => plans_others.push(result),
                                Err(e) => record_error(e),
                            }
                        }
                        ResolvedWorktree::BranchOnly { branch } => {
                            match handle_remove(
                                &branch,
                                !spec.delete_branch,
                                spec.force_delete,
                                spec.force,
                                &config,
                            ) {
                                Ok(result) => plans_branch_only.push(result),
                                Err(e) => record_error(e),
                            }
                        }
                    }
                }

                // If no valid plans, bail early (all failed validation)
                let has_valid_plans = !plans_others.is_empty()
                    || !plans_branch_only.is_empty()
                    || plan_current.is_some();
                if !has_valid_plans {
                    anyhow::bail!("");
                }

                // Early exit for benchmarking time-to-first-output
                if std::env::var_os("WORKTRUNK_FIRST_OUTPUT").is_some() {
                    return Ok(());
                }

                // Phase 2: Approve hooks (only if we have valid plans)
                // TODO(pre-remove-context): Approval context uses current worktree,
                // but hooks execute in each target worktree.
                let run_hooks = spec.verify && approve_remove(spec.yes)?;

                // Phase 3: Execute all validated plans
                // Remove other worktrees first
                for result in plans_others {
                    handle_remove_output(&result, spec.foreground, run_hooks, false)?;
                }

                // Handle branch-only cases
                for result in plans_branch_only {
                    handle_remove_output(&result, spec.foreground, run_hooks, false)?;
                }

                // Remove current worktree last (if it was in the list)
                if let Some(result) = plan_current {
                    handle_remove_output(&result, spec.foreground, run_hooks, false)?;
                }

                // Exit with failure if any validation errors occurred
                if !all_errors.is_empty() {
                    anyhow::bail!("");
                }

                Ok(())
            }
        })
}

fn main() {
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

    // Tell crossterm to always emit ANSI sequences
    crossterm::style::force_color_output(true);

    if completion::maybe_handle_env_completion() {
        return;
    }

    // Handle --help with pager before clap processes it
    if help::maybe_handle_help_with_pager() {
        return;
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
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());

    // Initialize base path from -C flag if provided
    if let Some(path) = cli.directory {
        set_base_path(path);
    }

    // Initialize config path from --config flag if provided
    if let Some(path) = cli.config {
        set_config_path(path);
    }

    // Configure logging based on --verbose flag or RUST_LOG env var
    // When -vv is set, also write logs to .git/wt-logs/verbose.log
    if cli.verbose >= 2 {
        verbose_log::init();
    }

    // Capture verbose level and command line before cli is partially consumed
    let verbose_level = cli.verbose;
    let command_line = std::env::args().collect::<Vec<_>>().join(" ");

    // Initialize command log for always-on logging of hooks and LLM commands.
    // Directory and file are created lazily on first log_command() call.
    if let Ok(repo) = worktrunk::git::Repository::current() {
        worktrunk::command_log::init(&repo.wt_logs_dir(), &command_line);
    }

    // Set global verbosity level for styled verbose output
    output::set_verbosity(verbose_level);

    // -vv enables debug logging via env_logger; -v uses styled output (not logging)
    // Otherwise, respect RUST_LOG (defaulting to off)
    let mut builder = if cli.verbose >= 2 {
        let mut b = env_logger::Builder::new();
        b.filter_level(log::LevelFilter::Debug);
        b
    } else {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("off"))
    };

    builder
        .format(|buf, record| {
            let msg = record.args().to_string();

            // Map thread ID to a single character (a-z, then A-Z)
            let thread_id = format!("{:?}", std::thread::current().id());
            let thread_num = thread_id
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
                .unwrap_or('?');

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

    let Some(command) = cli.command else {
        // No subcommand provided - print help to stderr (stdout is eval'd by shell wrapper)
        let mut cmd = cli::build_command();
        let help = cmd.render_help().ansi().to_string();
        eprintln!("{help}");
        return;
    };

    let result = match command {
        Commands::Config { action } => handle_config_command(action),
        Commands::Step { action } => handle_step_command(action),
        Commands::Hook { action } => handle_hook_command(action),
        Commands::Select { branches, remotes } => handle_select_command(branches, remotes),
        Commands::List {
            subcommand,
            format,
            branches,
            remotes,
            full,
            progressive,
            no_progressive,
        } => handle_list_command(
            subcommand,
            format,
            branches,
            remotes,
            full,
            progressive,
            no_progressive,
        ),
        Commands::Switch {
            branch,
            branches,
            remotes,
            create,
            base,
            execute,
            execute_args,
            yes,
            clobber,
            no_cd,
            verify,
        } => handle_switch_command(SwitchCommandArgs {
            branch,
            branches,
            remotes,
            create,
            base,
            execute,
            execute_args,
            yes,
            clobber,
            no_cd,
            verify,
        }),
        Commands::Remove {
            branches,
            delete_branch,
            force_delete,
            foreground,
            verify,
            yes,
            force,
        } => handle_remove_command(RemoveCommandArgs {
            branches,
            delete_branch,
            force_delete,
            foreground,
            verify,
            yes,
            force,
        }),
        Commands::Merge {
            target,
            squash,
            no_squash,
            commit,
            no_commit,
            rebase,
            no_rebase,
            remove,
            no_remove,
            verify,
            no_verify,
            yes,
            stage,
        } => {
            // Pass CLI flags as options; handle_merge determines effective defaults
            // using per-project config merged with global config
            handle_merge(MergeOptions {
                target: target.as_deref(),
                squash: flag_pair(squash, no_squash),
                commit: flag_pair(commit, no_commit),
                rebase: flag_pair(rebase, no_rebase),
                remove: flag_pair(remove, no_remove),
                verify: flag_pair(verify, no_verify),
                yes,
                stage,
            })
        }
    };

    if let Err(e) = result {
        // GitError, WorktrunkError, and HookErrorWithHint produce styled output via Display.
        // Some variants (AlreadyDisplayed, CommandNotApproved) have empty Display impls —
        // skip eprintln! for those to avoid phantom blank lines.
        if let Some(err) = e.downcast_ref::<worktrunk::git::GitError>() {
            eprintln!("{}", err);
        } else if let Some(err) = e.downcast_ref::<worktrunk::git::WorktrunkError>() {
            let display = err.to_string();
            if !display.is_empty() {
                eprintln!("{display}");
            }
        } else if let Some(err) = e.downcast_ref::<worktrunk::git::HookErrorWithHint>() {
            eprintln!("{}", err);
        } else if let Some(err) = e.downcast_ref::<worktrunk::config::TemplateExpandError>() {
            eprintln!("{}", err);
        } else {
            // Anyhow error formatting:
            // - With context: show context as header, root cause in gutter
            // - Simple error: inline with emoji
            // - Empty error: skip (errors already printed elsewhere)
            let msg = e.to_string();
            if !msg.is_empty() {
                // Collect the error chain (skipping the first which is in msg)
                let chain: Vec<String> = e.chain().skip(1).map(|e| e.to_string()).collect();
                if !chain.is_empty() {
                    // Has context: msg is context, chain contains intermediate + root cause
                    eprintln!("{}", error_message(&msg));
                    let chain_text = chain.join("\n");
                    eprintln!("{}", format_with_gutter(&chain_text, None));
                } else if msg.contains('\n') || msg.contains('\r') {
                    // Multiline error without context - this shouldn't happen if all
                    // errors have proper context. Catch in debug builds, log in release.
                    debug_assert!(false, "Multiline error without context: {msg}");
                    log::warn!("Multiline error without context: {msg}");
                    // Normalize line endings for display
                    let normalized = msg.replace("\r\n", "\n").replace('\r', "\n");
                    eprintln!("{}", error_message("Command failed"));
                    eprintln!("{}", format_with_gutter(&normalized, None));
                } else {
                    // Single-line error without context: inline with emoji
                    eprintln!("{}", error_message(&msg));
                }
            }
        }

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

        // Preserve exit code from child processes (especially for signals like SIGINT)
        let code = exit_code(&e).unwrap_or(1);

        // Write diagnostic if -vv was used (error case)
        diagnostic::write_if_verbose(verbose_level, &command_line, Some(&e.to_string()));

        // Reset ANSI state before exiting
        let _ = output::terminate_output();
        process::exit(code);
    }

    // Write diagnostic if -vv was used (success case)
    diagnostic::write_if_verbose(verbose_level, &command_line, None);

    // Reset ANSI state before returning to shell (success case)
    let _ = output::terminate_output();
}
