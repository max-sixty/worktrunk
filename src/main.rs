use anyhow::Context;
use clap::FromArgMatches;
use color_print::cformat;
use std::process;
use worktrunk::config::{WorktrunkConfig, expand_template, set_config_path};
use worktrunk::git::{Repository, exit_code, set_base_path};
use worktrunk::path::format_path_for_display;
use worktrunk::shell::extract_filename_from_path;
use worktrunk::styling::{
    error_message, format_with_gutter, hint_message, info_message, success_message, warning_message,
};

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

use commands::command_executor::{CommandContext, build_hook_context};
#[cfg(unix)]
use commands::handle_select;
use commands::worktree::{SwitchResult, handle_push};
use commands::{
    MergeOptions, RebaseResult, ResolutionContext, SquashResult, add_approvals, approve_hooks,
    clear_approvals, execute_switch, handle_completions, handle_config_create, handle_config_show,
    handle_configure_shell, handle_hints_clear, handle_hints_get, handle_hook_show, handle_init,
    handle_list, handle_merge, handle_rebase, handle_remove, handle_remove_current,
    handle_show_theme, handle_squash, handle_state_clear, handle_state_clear_all, handle_state_get,
    handle_state_set, handle_state_show, handle_unconfigure_shell, plan_switch,
    resolve_worktree_arg, run_hook, step_commit, step_copy_ignored, step_for_each,
};
use output::{execute_user_command, handle_remove_output, handle_switch_output};

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
    use clap::error::ErrorKind;
    use color_print::ceprintln;

    // For unrecognized subcommands, check if they match a nested subcommand
    // e.g., `wt squash` -> suggest `wt step squash`
    if err.kind() == ErrorKind::InvalidSubcommand
        && let Some(unknown) = err.get(clap::error::ContextKind::InvalidSubcommand)
    {
        let cmd = cli::build_command();
        if let Some(suggestion) = cli::suggest_nested_subcommand(&cmd, unknown.to_string().as_str())
        {
            eprint!("{}", err.render().ansi());
            ceprintln!("\n  <yellow>tip:</>  did you mean <cyan,bold>{suggestion}</cyan,bold>?");
            process::exit(2);
        }
    }

    // Enhance `wt switch` missing argument error with shortcut hints.
    // Hints go to stderr, which is safe since stdout is reserved for data output.
    // Check for both "wt switch" and "wt.exe switch" (Windows)
    let err_str = format!("{err}");
    let is_switch_missing_arg = err.kind() == ErrorKind::MissingRequiredArgument
        && (err_str.contains("wt switch") || err_str.contains("wt.exe switch"));
    if is_switch_missing_arg {
        eprint!("{}", err.render().ansi());
        eprintln!();
        ceprintln!("<green,bold>Quick switches:</>");
        ceprintln!("  <cyan,bold>wt switch ^</>    default branch's worktree");
        ceprintln!("  <cyan,bold>wt switch -</>    previous worktree");
        ceprintln!("  <cyan,bold>wt select</>      interactive picker");
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

    // Initialize verbose level for output module
    output::set_verbose_level(cli.verbose);

    // Configure logging based on --verbose flag or RUST_LOG env var
    // When -vv is set, also write logs to .git/wt-logs/verbose.log
    if cli.verbose >= 2 {
        verbose_log::init();
    }

    // Capture verbose level and command line before cli is partially consumed
    let verbose_level = cli.verbose;
    let command_line = std::env::args().collect::<Vec<_>>().join(" ");

    // -vv takes precedence over RUST_LOG: use Builder::new() to ignore env var
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
            use std::io::Write;

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
                            .and_then(|scan_result| {
                                // For --dry-run, preview was already shown by handler
                                if dry_run {
                                    return Ok(());
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

                                    crate::output::print(success_message(cformat!(
                                        "{} {what} for <bold>{shell}</> @ <bold>{path}</>",
                                        result.action.description(),
                                    )))?;
                                }

                                // Show completion results
                                for result in &scan_result.completion_results {
                                    let shell = result.shell;
                                    let path = format_path_for_display(&result.path);

                                    crate::output::print(success_message(cformat!(
                                        "{} completions for <bold>{shell}</> @ <bold>{path}</>",
                                        result.action.description(),
                                    )))?;
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
                                        crate::output::print(warning_message(format!(
                                            "No {what} found in {path}"
                                        )))?;
                                    } else {
                                        crate::output::print(hint_message(cformat!(
                                            "No <bright-black>{shell}</> {what} in {path}"
                                        )))?;
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
                                        crate::output::print(warning_message(format!(
                                            "No completions found in {path}"
                                        )))?;
                                    } else {
                                        crate::output::print(hint_message(cformat!(
                                            "No <bright-black>{shell}</> completions in {path}"
                                        )))?;
                                    }
                                }

                                // Exit with info if nothing was found
                                let all_not_found = scan_result.not_found.len()
                                    + scan_result.completion_not_found.len();
                                if total_changes == 0 {
                                    if all_not_found == 0 {
                                        crate::output::blank()?;
                                        crate::output::print(hint_message(
                                            "No shell integration found to remove",
                                        ))?;
                                    }
                                    return Ok(());
                                }

                                // Summary
                                crate::output::blank()?;
                                let plural = if shell_count == 1 { "" } else { "s" };
                                crate::output::print(success_message(format!(
                                    "Removed integration from {shell_count} shell{plural}"
                                )))?;

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
                                    crate::output::print(hint_message(
                                        "Restart shell to complete uninstall",
                                    ))?;
                                }
                                Ok(())
                            })
                    }
                    ConfigShellCommand::ShowTheme => {
                        handle_show_theme().map_err(|e| anyhow::anyhow!("{}", e))
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
                    Some(LogsAction::Get) | None => handle_state_get("logs", None),
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
            } => WorktrunkConfig::load()
                .context("Failed to load config")
                .and_then(|config| {
                    let stage_final = stage
                        .or_else(|| config.commit.and_then(|c| c.stage))
                        .unwrap_or_default();
                    step_commit(yes, !verify, stage_final, show_prompt)
                }),
            StepCommand::Squash {
                target,
                yes,
                verify,
                stage,
                show_prompt,
            } => WorktrunkConfig::load()
                .context("Failed to load config")
                .and_then(|config| {
                    let stage_final = stage
                        .or_else(|| config.commit.and_then(|c| c.stage))
                        .unwrap_or_default();

                    // Handle --show-prompt early: just build and output the prompt
                    if show_prompt {
                        return commands::step_show_squash_prompt(
                            target.as_deref(),
                            &config.commit_generation,
                        );
                    }

                    // "Approve at the Gate": approve pre-commit hooks upfront (unless --no-verify)
                    // Shadow verify: if user declines approval, skip hooks but continue squash
                    let verify = if verify {
                        use commands::command_approval::approve_hooks;
                        use commands::context::CommandEnv;
                        let env = CommandEnv::for_action("squash")?;
                        let ctx = env.context(yes);
                        let approved = approve_hooks(&ctx, &[HookType::PreCommit])?;
                        if !approved {
                            crate::output::print(info_message(
                                "Commands declined, squashing without hooks",
                            ))?;
                        }
                        approved
                    } else {
                        false
                    };

                    match handle_squash(target.as_deref(), yes, !verify, stage_final)? {
                        SquashResult::Squashed | SquashResult::NoNetChanges => {}
                        SquashResult::NoCommitsAhead(branch) => {
                            crate::output::print(info_message(format!(
                                "Nothing to squash; no commits ahead of {branch}"
                            )))?;
                        }
                        SquashResult::AlreadySingleCommit => {
                            crate::output::print(info_message(
                                "Nothing to squash; already a single commit",
                            ))?;
                        }
                    }
                    Ok(())
                }),
            StepCommand::Push { target } => handle_push(target.as_deref(), "Pushed to", None),
            StepCommand::Rebase { target } => {
                handle_rebase(target.as_deref()).and_then(|result| match result {
                    RebaseResult::Rebased => Ok(()),
                    RebaseResult::UpToDate(branch) => {
                        crate::output::print(info_message(cformat!(
                            "Already up to date with <bold>{branch}</>"
                        )))?;
                        Ok(())
                    }
                })
            }
            StepCommand::CopyIgnored { from, to, dry_run } => {
                step_copy_ignored(from.as_deref(), to.as_deref(), dry_run)
            }
            StepCommand::ForEach { args } => step_for_each(args),
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
                    let _ = output::print(warning_message(
                        "--no-background is deprecated; use --foreground instead",
                    ));
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
                    let _ = output::print(warning_message(
                        "--no-background is deprecated; use --foreground instead",
                    ));
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
            HookCommand::Approvals { action } => match action {
                ApprovalsCommand::Add { all } => add_approvals(all),
                ApprovalsCommand::Clear { global } => clear_approvals(global),
            },
        },
        #[cfg(unix)]
        Commands::Select { branches, remotes } => {
            WorktrunkConfig::load()
                .context("Failed to load config")
                .and_then(|config| {
                    // Get config values from [list] config (shared with wt list)
                    let (show_branches_config, show_remotes_config) = config
                        .list
                        .as_ref()
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
            let _ = output::print(error_message("wt select is not available on Windows"));
            let _ = output::print(hint_message(cformat!(
                "To see all worktrees, run <bright-black>wt list</>; to switch directly, run <bright-black>wt switch BRANCH</>"
            )));
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
            Some(ListSubcommand::Statusline { claude_code }) => {
                commands::statusline::run(claude_code)
            }
            None => {
                use commands::list::progressive::RenderMode;

                // Load config and merge with CLI flags (CLI flags take precedence)
                WorktrunkConfig::load()
                    .context("Failed to load config")
                    .and_then(|config| {
                        // Get config values from global list config
                        let (show_branches_config, show_remotes_config, show_full_config) = config
                            .list
                            .as_ref()
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
        } => WorktrunkConfig::load()
            .context("Failed to load config")
            .and_then(|mut config| {
                let repo = Repository::current().context("Failed to switch worktree")?;

                // Validate FIRST (before approval) - fails fast if branch doesn't exist, etc.
                let plan = plan_switch(&repo, &branch, create, base.as_deref(), clobber, &config)?;

                // "Approve at the Gate": collect and approve hooks upfront
                // This ensures approval happens once at the command entry point
                // If user declines, skip hooks but continue with worktree operation
                let approved = if verify {
                    let repo_root = repo.repo_path().context("Failed to switch worktree")?;
                    let ctx = CommandContext::new(
                        &repo,
                        &config,
                        Some(plan.branch()),
                        plan.worktree_path(),
                        &repo_root,
                        yes,
                    );
                    // Approve different hooks based on whether we're creating or switching
                    if plan.is_create() {
                        approve_hooks(
                            &ctx,
                            &[
                                HookType::PostCreate,
                                HookType::PostStart,
                                HookType::PostSwitch,
                            ],
                        )?
                    } else {
                        // When switching to existing, only post-switch needs approval
                        approve_hooks(&ctx, &[HookType::PostSwitch])?
                    }
                } else {
                    true // --no-verify: skip all hooks
                };

                // Skip hooks if --no-verify or user declined approval
                let skip_hooks = !verify || !approved;

                // Show message if user declined approval
                if !approved {
                    crate::output::print(info_message(if plan.is_create() {
                        "Commands declined, continuing worktree creation"
                    } else {
                        "Commands declined"
                    }))?;
                }

                // Execute the validated plan
                let (result, branch_info) = execute_switch(&repo, plan, &config, yes, skip_hooks)?;

                // Show success message (temporal locality: immediately after worktree operation)
                // Returns path to display in hooks when user's shell won't be in the worktree
                // Also shows worktree-path hint on first --create (before shell integration warning)
                let hooks_display_path =
                    handle_switch_output(&result, &branch_info, execute.as_deref())?;

                // Offer shell integration if not already installed/active
                // (only shows prompt/hint when shell integration isn't working)
                // With --execute: show hints only (don't interrupt with prompt)
                // Best-effort: don't fail switch if offer fails
                if !output::is_shell_integration_active() {
                    let skip_prompt = execute.is_some();
                    let _ =
                        output::prompt_shell_integration(&mut config, &binary_name(), skip_prompt);
                }

                // Build extra vars for base branch context (used by both hooks and --execute)
                // "base" is the branch we branched from when creating a new worktree.
                // For existing worktrees, there's no base concept.
                let (base_branch, base_worktree_path): (Option<&str>, Option<&str>) = match &result
                {
                    SwitchResult::Created {
                        base_branch,
                        base_worktree_path,
                        ..
                    } => (base_branch.as_deref(), base_worktree_path.as_deref()),
                    SwitchResult::Existing(_) | SwitchResult::AlreadyAt(_) => (None, None),
                };
                let extra_vars: Vec<(&str, &str)> = [
                    base_branch.map(|b| ("base", b)),
                    base_worktree_path.map(|p| ("base_worktree_path", p)),
                ]
                .into_iter()
                .flatten()
                .collect();

                // Spawn background hooks after success message
                // - post-switch: runs on ALL switches (shows "@ path" when shell won't be there)
                // - post-start: runs only when creating a NEW worktree
                if !skip_hooks {
                    let repo = Repository::current()?;
                    let repo_root = repo.repo_path().context("Failed to switch worktree")?;
                    let ctx = CommandContext::new(
                        &repo,
                        &config,
                        Some(&branch_info.branch),
                        result.path(),
                        &repo_root,
                        yes,
                    );

                    // Post-switch runs first (immediate "I'm here" signal)
                    ctx.spawn_post_switch_commands(&extra_vars, hooks_display_path.as_deref())?;

                    // Post-start runs only on creation (setup tasks)
                    if matches!(&result, SwitchResult::Created { .. }) {
                        ctx.spawn_post_start_commands(&extra_vars, hooks_display_path.as_deref())?;
                    }
                }

                // Execute user command after post-start hooks have been spawned
                // Note: execute_args requires execute via clap's `requires` attribute
                if let Some(cmd) = execute {
                    // Build template context for expansion (includes base vars when creating)
                    let repo = Repository::current()?;
                    let repo_root = repo.repo_path().context("Failed to get repo root")?;
                    let ctx = CommandContext::new(
                        &repo,
                        &config,
                        Some(&branch_info.branch),
                        result.path(),
                        &repo_root,
                        yes,
                    );
                    let template_vars = build_hook_context(&ctx, &extra_vars);
                    let vars: std::collections::HashMap<&str, &str> = template_vars
                        .iter()
                        .map(|(k, v)| (k.as_str(), v.as_str()))
                        .collect();

                    // Expand template variables in command (shell_escape: true for safety)
                    let expanded_cmd = expand_template(&cmd, &vars, true, &repo).map_err(|e| {
                        anyhow::anyhow!("Failed to expand --execute template: {}", e)
                    })?;

                    // Append any trailing args (after --) to the execute command
                    // Each arg is also expanded, then shell-escaped
                    let full_cmd = if execute_args.is_empty() {
                        expanded_cmd
                    } else {
                        let expanded_args: Result<Vec<_>, _> = execute_args
                            .iter()
                            .map(|arg| {
                                expand_template(arg, &vars, false, &repo).map_err(|e| {
                                    anyhow::anyhow!("Failed to expand argument template: {}", e)
                                })
                            })
                            .collect();
                        let escaped_args: Vec<_> = expanded_args?
                            .iter()
                            .map(|arg| shlex::try_quote(arg).unwrap_or(arg.into()).into_owned())
                            .collect();
                        format!("{} {}", expanded_cmd, escaped_args.join(" "))
                    };
                    execute_user_command(&full_cmd)?;
                }

                Ok(())
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
        } => WorktrunkConfig::load()
            .context("Failed to load config")
            .and_then(|config| {
                // Handle deprecated --no-background flag
                if no_background {
                    output::print(warning_message(
                        "--no-background is deprecated; use --foreground instead",
                    ))?;
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
                    use commands::context::CommandEnv;
                    let env = CommandEnv::for_action_branchless()?;
                    let ctx = env.context(yes);
                    let approved =
                        approve_hooks(&ctx, &[HookType::PreRemove, HookType::PostSwitch])?;
                    if !approved {
                        crate::output::print(info_message(
                            "Commands declined, continuing removal",
                        ))?;
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
                    use commands::worktree::RemoveResult;
                    use worktrunk::git::ResolvedWorktree;

                    // Multi-worktree removal: validate ALL first, then approve, then execute
                    // This supports partial success - some may fail validation while others succeed.
                    let current_worktree = repo
                        .current_worktree()
                        .root()
                        .ok()
                        .and_then(|p| dunce::canonicalize(&p).ok());

                    // Dedupe inputs to avoid redundant planning/execution
                    let branches: Vec<_> = {
                        let mut seen = std::collections::HashSet::new();
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
                    let mut record_error = |e: anyhow::Error| -> anyhow::Result<()> {
                        output::print(e.to_string())?;
                        all_errors.push(e);
                        Ok(())
                    };

                    for branch_name in &branches {
                        // Resolve the target
                        let resolved = match resolve_worktree_arg(
                            &repo,
                            branch_name,
                            &config,
                            ResolutionContext::Remove,
                        ) {
                            Ok(r) => r,
                            Err(e) => {
                                record_error(e)?;
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
                                        Err(e) => record_error(e)?,
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
                                    Err(e) => record_error(e)?,
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
                                    Err(e) => record_error(e)?,
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
        } => WorktrunkConfig::load()
            .context("Failed to load config")
            .and_then(|config| {
                // Convert paired flags to Option<bool>
                fn flag_pair(positive: bool, negative: bool) -> Option<bool> {
                    match (positive, negative) {
                        (true, _) => Some(true),
                        (_, true) => Some(false),
                        _ => None,
                    }
                }

                // Get config defaults (positive form: true = do it)
                let merge_config = config.merge.as_ref();
                let squash_default = merge_config.and_then(|m| m.squash).unwrap_or(true);
                let commit_default = merge_config.and_then(|m| m.commit).unwrap_or(true);
                let rebase_default = merge_config.and_then(|m| m.rebase).unwrap_or(true);
                let remove_default = merge_config.and_then(|m| m.remove).unwrap_or(true);
                let verify_default = merge_config.and_then(|m| m.verify).unwrap_or(true);

                // CLI flags override config, config overrides defaults
                let squash_final = flag_pair(squash, no_squash).unwrap_or(squash_default);
                let commit_final = flag_pair(commit, no_commit).unwrap_or(commit_default);
                let rebase_final = flag_pair(rebase, no_rebase).unwrap_or(rebase_default);
                let remove_final = flag_pair(remove, no_remove).unwrap_or(remove_default);
                let verify_final = flag_pair(verify, no_verify).unwrap_or(verify_default);

                // Stage defaults from [commit] config section
                let stage_final = stage
                    .or_else(|| config.commit.and_then(|c| c.stage))
                    .unwrap_or_default();

                handle_merge(MergeOptions {
                    target: target.as_deref(),
                    squash: squash_final,
                    commit: commit_final,
                    rebase: rebase_final,
                    remove: remove_final,
                    verify: verify_final,
                    yes,
                    stage_mode: stage_final,
                })
            }),
    };

    if let Err(e) = result {
        // GitError, WorktrunkError, and HookErrorWithHint produce styled output via Display
        if let Some(err) = e.downcast_ref::<worktrunk::git::GitError>() {
            let _ = output::print(err.to_string());
        } else if let Some(err) = e.downcast_ref::<worktrunk::git::WorktrunkError>() {
            let _ = output::print(err.to_string());
        } else if let Some(err) = e.downcast_ref::<worktrunk::git::HookErrorWithHint>() {
            let _ = output::print(err.to_string());
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
                    let _ = output::print(error_message(&msg));
                    let chain_text = chain.join("\n");
                    let _ = output::print(format_with_gutter(&chain_text, None));
                } else if msg.contains('\n') || msg.contains('\r') {
                    // Multiline error without context - this shouldn't happen if all
                    // errors have proper context. Catch in debug builds, log in release.
                    debug_assert!(false, "Multiline error without context: {msg}");
                    log::warn!("Multiline error without context: {msg}");
                    // Normalize line endings for display
                    let normalized = msg.replace("\r\n", "\n").replace('\r', "\n");
                    let _ = output::print(error_message("Command failed"));
                    let _ = output::print(format_with_gutter(&normalized, None));
                } else {
                    // Single-line error without context: inline with emoji
                    let _ = output::print(error_message(&msg));
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
