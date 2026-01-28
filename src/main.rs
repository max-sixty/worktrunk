use std::collections::HashSet;
use std::io::Write;

use anyhow::Context;
use clap::FromArgMatches;
use clap::error::ErrorKind as ClapErrorKind;
use color_print::{ceprintln, cformat};
use std::process;
use worktrunk::config::{UserConfig, set_config_path};
use worktrunk::git::{Repository, ResolvedWorktree, exit_code, set_base_path};
use worktrunk::path::format_path_for_display;
use worktrunk::shell::extract_filename_from_path;
use worktrunk::styling::{
    eprintln, error_message, format_with_gutter, hint_message, info_message, success_message,
    warning_message,
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
    handle_configure_shell, handle_hints_clear, handle_hints_get, handle_hook_show, handle_init,
    handle_list, handle_logs_get, handle_merge, handle_rebase, handle_remove,
    handle_remove_current, handle_show_theme, handle_squash, handle_state_clear,
    handle_state_clear_all, handle_state_get, handle_state_set, handle_state_show, handle_switch,
    handle_unconfigure_shell, resolve_worktree_arg, run_hook, step_commit, step_copy_ignored,
    step_for_each, step_relocate,
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
/// For `wt switch` missing the branch argument, adds hints about shortcuts.
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
  <yellow>tip:</>  did you mean <cyan,bold>{suggestion}</cyan,bold>?",
                err.render().ansi()
            );
            process::exit(2);
        }
    }

    // Enhance `wt switch` missing argument error with shortcut hints.
    // Hints go to stderr, which is safe since stdout is reserved for data output.
    // Check for both "wt switch" and "wt.exe switch" (Windows)
    let err_str = format!("{err}");
    let is_switch_missing_arg = err.kind() == ClapErrorKind::MissingRequiredArgument
        && (err_str.contains("wt switch") || err_str.contains("wt.exe switch"));
    if is_switch_missing_arg {
        ceprintln!(
            "{}

<green,bold>Quick switches:</>
  <cyan,bold>wt switch ^</>    # default branch's worktree
  <cyan,bold>wt switch -</>    # previous worktree
  <cyan,bold>wt select</>      # interactive picker",
            err.render().ansi()
        );
        process::exit(2);
    }

    err.exit()
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
        Commands::Config { action } => match action {
            ConfigCommand::Shell { action } => {
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
                            .map(|scan_result| {
                                // For --dry-run, preview was already shown by handler
                                if dry_run {
                                    return;
                                }

                                // Count unique shells, not file results (fish may have 2 files: functions/ and legacy conf.d/)
                                let mut shells: Vec<_> =
                                    scan_result.results.iter().map(|r| r.shell).collect();
                                shells.sort_by_key(|s| s.to_string());
                                shells.dedup();
                                let shell_count = shells.len();
                                let completion_count = scan_result.completion_results.len();
                                let total_changes = shell_count + completion_count;

                                // Show shell extension results
                                for result in &scan_result.results {
                                    let shell = result.shell;
                                    let path = format_path_for_display(&result.path);
                                    // For bash/zsh, completions are inline in the init script
                                    let what = if matches!(
                                        shell,
                                        worktrunk::shell::Shell::Bash
                                            | worktrunk::shell::Shell::Zsh
                                    ) {
                                        "shell extension & completions"
                                    } else {
                                        "shell extension"
                                    };

                                    eprintln!(
                                        "{}",
                                        success_message(cformat!(
                                            "{} {what} for <bold>{shell}</> @ <bold>{path}</>",
                                            result.action.description(),
                                        ))
                                    );
                                }

                                // Show completion results
                                for result in &scan_result.completion_results {
                                    let shell = result.shell;
                                    let path = format_path_for_display(&result.path);

                                    eprintln!(
                                        "{}",
                                        success_message(cformat!(
                                            "{} completions for <bold>{shell}</> @ <bold>{path}</>",
                                            result.action.description(),
                                        ))
                                    );
                                }

                                // Show not found - warning if explicit shell, hint if auto-scan
                                for (shell, path) in &scan_result.not_found {
                                    let path = format_path_for_display(path);
                                    // Use consistent terminology matching install/uninstall messages
                                    let what = if matches!(
                                        shell,
                                        worktrunk::shell::Shell::Bash
                                            | worktrunk::shell::Shell::Zsh
                                    ) {
                                        "shell extension & completions"
                                    } else {
                                        "shell extension"
                                    };
                                    if explicit_shell {
                                        eprintln!(
                                            "{}",
                                            warning_message(format!("No {what} found in {path}"))
                                        );
                                    } else {
                                        eprintln!(
                                            "{}",
                                            hint_message(cformat!(
                                                "No <bright-black>{shell}</> {what} in {path}"
                                            ))
                                        );
                                    }
                                }

                                // Show completion files not found (only fish has separate completion files)
                                // Only show this if the shell extension was ALSO not found - if we removed
                                // the shell extension, no need to warn about missing completions
                                for (shell, path) in &scan_result.completion_not_found {
                                    let shell_was_removed =
                                        scan_result.results.iter().any(|r| r.shell == *shell);
                                    if shell_was_removed {
                                        continue; // Shell extension was removed, don't warn about completions
                                    }
                                    let path = format_path_for_display(path);
                                    if explicit_shell {
                                        eprintln!(
                                            "{}",
                                            warning_message(format!(
                                                "No completions found in {path}"
                                            ))
                                        );
                                    } else {
                                        eprintln!(
                                            "{}",
                                            hint_message(cformat!(
                                                "No <bright-black>{shell}</> completions in {path}"
                                            ))
                                        );
                                    }
                                }

                                // Exit with info if nothing was found
                                let all_not_found = scan_result.not_found.len()
                                    + scan_result.completion_not_found.len();
                                if total_changes == 0 {
                                    if all_not_found == 0 {
                                        eprintln!();
                                        eprintln!(
                                            "{}",
                                            hint_message("No shell integration found to remove")
                                        );
                                    }
                                    return;
                                }

                                // Summary
                                eprintln!();
                                let plural = if shell_count == 1 { "" } else { "s" };
                                eprintln!(
                                    "{}",
                                    success_message(format!(
                                        "Removed integration from {shell_count} shell{plural}"
                                    ))
                                );

                                // Hint about restarting shell (only if current shell was affected)
                                let current_shell = std::env::var("SHELL")
                                    .ok()
                                    .and_then(|s| extract_filename_from_path(&s).map(String::from));

                                let current_shell_affected =
                                    current_shell.as_ref().is_some_and(|shell_name| {
                                        scan_result.results.iter().any(|r| {
                                            r.shell.to_string().eq_ignore_ascii_case(shell_name)
                                        })
                                    });

                                if current_shell_affected {
                                    eprintln!(
                                        "{}",
                                        hint_message("Restart shell to complete uninstall")
                                    );
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
            ConfigCommand::Create { project } => handle_config_create(project),
            ConfigCommand::Show { full } => handle_config_show(full),
            ConfigCommand::State { action } => match action {
                StateCommand::DefaultBranch { action } => match action {
                    Some(DefaultBranchAction::Get) | None => {
                        handle_state_get("default-branch", None)
                    }
                    Some(DefaultBranchAction::Set { branch }) => {
                        handle_state_set("default-branch", branch, None)
                    }
                    Some(DefaultBranchAction::Clear) => {
                        handle_state_clear("default-branch", None, false)
                    }
                },
                StateCommand::PreviousBranch { action } => match action {
                    Some(PreviousBranchAction::Get) | None => {
                        handle_state_get("previous-branch", None)
                    }
                    Some(PreviousBranchAction::Set { branch }) => {
                        handle_state_set("previous-branch", branch, None)
                    }
                    Some(PreviousBranchAction::Clear) => {
                        handle_state_clear("previous-branch", None, false)
                    }
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
                    Some(MarkerAction::Set { value, branch }) => {
                        handle_state_set("marker", value, branch)
                    }
                    Some(MarkerAction::Clear { branch, all }) => {
                        handle_state_clear("marker", branch, all)
                    }
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
            },
        },
        Commands::Step { action } => match action {
            StepCommand::Commit {
                yes,
                verify,
                stage,
                show_prompt,
            } => step_commit(yes, !verify, stage, show_prompt),
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
                    handle_squash(target.as_deref(), yes, !verify, stage).map(|result| match result
                    {
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
            StepCommand::CopyIgnored { from, to, dry_run } => {
                step_copy_ignored(from.as_deref(), to.as_deref(), dry_run)
            }
            StepCommand::ForEach { args } => step_for_each(args),
            StepCommand::Relocate {
                branches,
                dry_run,
                commit,
                clobber,
            } => step_relocate(branches, dry_run, commit, clobber),
        },
        Commands::Hook { action } => match action {
            HookCommand::Show {
                hook_type,
                expanded,
            } => handle_hook_show(hook_type.as_deref(), expanded),
            HookCommand::PostCreate { name, yes, vars } => {
                run_hook(HookType::PostCreate, yes, None, name.as_deref(), &vars)
            }
            HookCommand::PostStart {
                name,
                yes,
                foreground,
                no_background,
                vars,
            } => {
                if no_background {
                    eprintln!(
                        "{}",
                        warning_message("--no-background is deprecated; use --foreground instead")
                    );
                }
                run_hook(
                    HookType::PostStart,
                    yes,
                    Some(foreground || no_background),
                    name.as_deref(),
                    &vars,
                )
            }
            HookCommand::PostSwitch {
                name,
                yes,
                foreground,
                no_background,
                vars,
            } => {
                if no_background {
                    eprintln!(
                        "{}",
                        warning_message("--no-background is deprecated; use --foreground instead")
                    );
                }
                run_hook(
                    HookType::PostSwitch,
                    yes,
                    Some(foreground || no_background),
                    name.as_deref(),
                    &vars,
                )
            }
            HookCommand::PreCommit { name, yes, vars } => {
                run_hook(HookType::PreCommit, yes, None, name.as_deref(), &vars)
            }
            HookCommand::PreMerge { name, yes, vars } => {
                run_hook(HookType::PreMerge, yes, None, name.as_deref(), &vars)
            }
            HookCommand::PostMerge { name, yes, vars } => {
                run_hook(HookType::PostMerge, yes, None, name.as_deref(), &vars)
            }
            HookCommand::PreRemove { name, yes, vars } => {
                run_hook(HookType::PreRemove, yes, None, name.as_deref(), &vars)
            }
            HookCommand::PostRemove {
                name,
                yes,
                foreground,
                vars,
            } => run_hook(
                HookType::PostRemove,
                yes,
                Some(foreground),
                name.as_deref(),
                &vars,
            ),
            HookCommand::Approvals { action } => match action {
                ApprovalsCommand::Add { all } => add_approvals(all),
                ApprovalsCommand::Clear { global } => clear_approvals(global),
            },
        },
        #[cfg(unix)]
        Commands::Select { branches, remotes } => {
            UserConfig::load()
                .context("Failed to load config")
                .and_then(|config| {
                    // Get project ID for per-project config lookup
                    let project_id = Repository::current()
                        .ok()
                        .and_then(|r| r.project_identifier().ok());

                    // Get effective list config (merges project-specific with global)
                    let (show_branches_config, show_remotes_config) = config
                        .list(project_id.as_deref())
                        .map(|l| (l.branches.unwrap_or(false), l.remotes.unwrap_or(false)))
                        .unwrap_or((false, false));

                    // CLI flags override config
                    let show_branches = branches || show_branches_config;
                    let show_remotes = remotes || show_remotes_config;

                    handle_select(show_branches, show_remotes, &config)
                })
        }
        #[cfg(not(unix))]
        Commands::Select { .. } => {
            eprintln!("{}", error_message("wt select is not available on Windows"));
            eprintln!(
                "{}",
                hint_message(cformat!(
                    "To see all worktrees, run <bright-black>wt list</>; to switch directly, run <bright-black>wt switch BRANCH</>"
                ))
            );
            std::process::exit(1);
        }
        Commands::List {
            subcommand,
            format,
            branches,
            remotes,
            full,
            progressive,
            no_progressive,
        } => match subcommand {
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
                // Load config and merge with CLI flags (CLI flags take precedence)
                UserConfig::load()
                    .context("Failed to load config")
                    .and_then(|config| {
                        // Get project ID for per-project config lookup
                        let project_id = Repository::current()
                            .ok()
                            .and_then(|r| r.project_identifier().ok());

                        // Get effective list config (merges project-specific with global)
                        let (show_branches_config, show_remotes_config, show_full_config) = config
                            .list(project_id.as_deref())
                            .map(|l| {
                                (
                                    l.branches.unwrap_or(false),
                                    l.remotes.unwrap_or(false),
                                    l.full.unwrap_or(false),
                                )
                            })
                            .unwrap_or((false, false, false));

                        // CLI flags override config
                        let show_branches = branches || show_branches_config;
                        let show_remotes = remotes || show_remotes_config;
                        let show_full = full || show_full_config;

                        // Convert two bools to Option<bool>: Some(true), Some(false), or None
                        let progressive_opt = match (progressive, no_progressive) {
                            (true, _) => Some(true),
                            (_, true) => Some(false),
                            _ => None,
                        };
                        let render_mode = RenderMode::detect(progressive_opt);
                        handle_list(
                            format,
                            show_branches,
                            show_remotes,
                            show_full,
                            render_mode,
                            &config,
                        )
                    })
            }
        },
        Commands::Switch {
            branch,
            create,
            base,
            execute,
            execute_args,
            yes,
            clobber,
            verify,
        } => UserConfig::load()
            .context("Failed to load config")
            .and_then(|mut config| {
                handle_switch(
                    SwitchOptions {
                        branch: &branch,
                        create,
                        base: base.as_deref(),
                        execute: execute.as_deref(),
                        execute_args: &execute_args,
                        yes,
                        clobber,
                        verify,
                    },
                    &mut config,
                    &binary_name(),
                )
            }),
        Commands::Remove {
            branches,
            delete_branch,
            force_delete,
            foreground,
            no_background,
            verify,
            yes,
            force,
        } => UserConfig::load()
            .context("Failed to load config")
            .and_then(|config| {
                // Handle deprecated --no-background flag
                if no_background {
                    eprintln!(
                        "{}",
                        warning_message("--no-background is deprecated; use --foreground instead")
                    );
                }
                let background = !(foreground || no_background);

                // Validate conflicting flags
                if !delete_branch && force_delete {
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

                if branches.is_empty() {
                    // Single worktree removal: validate FIRST, then approve, then execute
                    let result =
                        handle_remove_current(!delete_branch, force_delete, force, &config)
                            .context("Failed to remove worktree")?;

                    // "Approve at the Gate": approval happens AFTER validation passes
                    let run_hooks = verify && approve_remove(yes)?;

                    handle_remove_output(&result, background, run_hooks)
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
                                        !delete_branch,
                                        force_delete,
                                        force,
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
                                    !delete_branch,
                                    force_delete,
                                    force,
                                    &config,
                                ) {
                                    Ok(result) => plans_others.push(result),
                                    Err(e) => record_error(e),
                                }
                            }
                            ResolvedWorktree::BranchOnly { branch } => {
                                match handle_remove(
                                    &branch,
                                    !delete_branch,
                                    force_delete,
                                    force,
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

                    // Phase 2: Approve hooks (only if we have valid plans)
                    // TODO(pre-remove-context): Approval context uses current worktree,
                    // but hooks execute in each target worktree.
                    let run_hooks = verify && approve_remove(yes)?;

                    // Phase 3: Execute all validated plans
                    // Remove other worktrees first
                    for result in plans_others {
                        handle_remove_output(&result, background, run_hooks)?;
                    }

                    // Handle branch-only cases
                    for result in plans_branch_only {
                        handle_remove_output(&result, background, run_hooks)?;
                    }

                    // Remove current worktree last (if it was in the list)
                    if let Some(result) = plan_current {
                        handle_remove_output(&result, background, run_hooks)?;
                    }

                    // Exit with failure if any validation errors occurred
                    if !all_errors.is_empty() {
                        anyhow::bail!("");
                    }

                    Ok(())
                }
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
            // Convert paired flags to Option<bool>
            fn flag_pair(positive: bool, negative: bool) -> Option<bool> {
                match (positive, negative) {
                    (true, _) => Some(true),
                    (_, true) => Some(false),
                    _ => None,
                }
            }

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
        // GitError, WorktrunkError, and HookErrorWithHint produce styled output via Display
        if let Some(err) = e.downcast_ref::<worktrunk::git::GitError>() {
            eprintln!("{}", err);
        } else if let Some(err) = e.downcast_ref::<worktrunk::git::WorktrunkError>() {
            eprintln!("{}", err);
        } else if let Some(err) = e.downcast_ref::<worktrunk::git::HookErrorWithHint>() {
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
