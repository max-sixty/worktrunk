use anstyle::Style;
use anyhow::Context;
use clap::FromArgMatches;
use std::process;
use worktrunk::config::{WorktrunkConfig, set_config_path};
use worktrunk::git::{Repository, exit_code, is_command_not_approved, set_base_path};
use worktrunk::path::format_path_for_display;
use worktrunk::styling::println;

mod cli;
mod commands;
mod completion;
mod display;
mod help_pager;
mod help_resolver;
mod llm;
mod md_help;
mod output;

pub use crate::cli::OutputFormat;

use commands::command_executor::CommandContext;
#[cfg(unix)]
use commands::handle_select;
use commands::worktree::{SwitchResult, handle_push};
use commands::{
    ConfigAction, RebaseResult, handle_config_create, handle_config_list,
    handle_config_refresh_cache, handle_config_status_set, handle_config_status_unset,
    handle_configure_shell, handle_init, handle_list, handle_merge, handle_rebase, handle_remove,
    handle_squash, handle_standalone_ask_approvals, handle_standalone_clear_approvals,
    handle_standalone_commit, handle_standalone_run_hook, handle_switch, handle_unconfigure_shell,
};
use output::{execute_user_command, handle_remove_output, handle_switch_output};

#[cfg(unix)]
use cli::BetaCommand;
use cli::{
    ApprovalsCommand, Cli, Commands, ConfigCommand, ConfigShellCommand, StatusAction, StepCommand,
};
use worktrunk::HookType;

/// Try to handle --help flag with pager before clap processes it
fn maybe_handle_help_with_pager() -> bool {
    use clap::ColorChoice;
    use clap::error::ErrorKind;

    let args: Vec<String> = std::env::args().collect();

    // Check for --help-md flag (output raw markdown without ANSI rendering)
    if args.iter().any(|a| a == "--help-md") {
        let mut cmd = cli::build_command();
        // Filter out --help-md and add --help for clap
        let filtered_args: Vec<String> = args
            .iter()
            .map(|a| {
                if a == "--help-md" {
                    "--help".to_string()
                } else {
                    a.clone()
                }
            })
            .collect();
        let target = help_resolver::resolve_target_command(&mut cmd, filtered_args);
        let help = target.render_long_help().to_string(); // Raw markdown, no ANSI
        println!("{}", help);
        process::exit(0);
    }

    let mut cmd = cli::build_command();
    cmd = cmd.color(ColorChoice::Always); // Force clap to always emit ANSI codes

    // DON'T render markdown yet - let clap generate help first

    match cmd.try_get_matches_from_mut(args) {
        Ok(_) => false, // Normal args, not help
        Err(err) => {
            match err.kind() {
                ErrorKind::DisplayHelp => {
                    // Re-resolve which subcommand's help user asked for
                    let target = help_resolver::resolve_target_command(&mut cmd, std::env::args());
                    let mut help = target.render_long_help().to_string(); // StyledStr -> string (contains raw markdown)

                    // NOW render markdown sections to ANSI
                    help = md_help::render_markdown_in_help(&help);

                    if let Err(e) = help_pager::show_help_in_pager(&help) {
                        log::debug!("Pager invocation failed: {}", e);
                        eprintln!("{}", help);
                    }
                    process::exit(0);
                }
                ErrorKind::DisplayVersion => {
                    // Version display
                    println!("{}", err);
                    process::exit(0);
                }
                _ => {
                    // Not help or version - this will be re-parsed by Cli::parse() below
                    // which will handle the error with proper ANSI formatting
                    false
                }
            }
        }
    }
}

fn main() {
    // Tell crossterm to always emit ANSI sequences
    crossterm::style::force_color_output(true);

    if completion::maybe_handle_env_completion() {
        return;
    }

    // Handle --help with pager before clap processes it
    if maybe_handle_help_with_pager() {
        return;
    }

    // TODO: Enhance error messages to show possible values for missing enum arguments
    // Currently `wt config shell init` doesn't show available shells, but `wt config shell init invalid` does.
    // Clap doesn't support this natively yet - see https://github.com/clap-rs/clap/issues/3320
    // When available, use built-in setting. Until then, could use try_parse() to intercept
    // MissingRequiredArgument errors and print custom messages with ValueEnum::value_variants().
    let cmd = cli::build_command();
    let matches = cmd.get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());

    // Initialize base path from -C flag if provided
    if let Some(path) = cli.directory {
        set_base_path(path);
    }

    // Initialize config path from --config flag if provided
    if let Some(path) = cli.config {
        set_config_path(path);
    }

    // Initialize output context based on --internal flag
    let output_mode = if cli.internal {
        output::OutputMode::Directive
    } else {
        output::OutputMode::Interactive
    };
    output::initialize(output_mode);

    // Configure logging based on --verbose flag or RUST_LOG env var
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or(if cli.verbose { "debug" } else { "off" }),
    )
    .format(|buf, record| {
        use anstyle::Style;
        use std::io::Write;

        let msg = record.args().to_string();

        // Map thread ID to a single character (a-z, then A-Z)
        let thread_id = format!("{:?}", std::thread::current().id());
        let thread_num = thread_id
            .strip_prefix("ThreadId(")
            .and_then(|s| s.strip_suffix(")"))
            .and_then(|s| s.parse::<usize>().ok())
            .map(|n| {
                if n <= 26 {
                    char::from(b'a' + (n - 1) as u8)
                } else if n <= 52 {
                    char::from(b'A' + (n - 27) as u8)
                } else {
                    '?'
                }
            })
            .unwrap_or('?');

        let dim = Style::new().dimmed();

        // Commands start with $, make only the command bold (not $ or [worktree])
        if let Some(rest) = msg.strip_prefix("$ ") {
            let bold = Style::new().bold();

            // Split: "git command [worktree]" -> ("git command", " [worktree]")
            if let Some(bracket_pos) = rest.find(" [") {
                let command = &rest[..bracket_pos];
                let worktree = &rest[bracket_pos..];
                writeln!(
                    buf,
                    "{dim}[{thread_num}]{dim:#} $ {bold}{command}{bold:#}{worktree}"
                )
            } else {
                writeln!(buf, "{dim}[{thread_num}]{dim:#} $ {bold}{rest}{bold:#}")
            }
        } else if msg.starts_with("  ! ") {
            // Error output - show in red
            use anstyle::{AnsiColor, Color};
            let red = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Red)));
            writeln!(buf, "{dim}[{thread_num}]{dim:#} {red}{msg}{red:#}")
        } else {
            // Regular output with thread ID
            writeln!(buf, "{dim}[{thread_num}]{dim:#} {msg}")
        }
    })
    .init();

    let result = match cli.command {
        Commands::Config { action } => match action {
            ConfigCommand::Shell { action } => {
                match action {
                    ConfigShellCommand::Init { shell } => {
                        // Generate shell code to stdout
                        handle_init(shell).map_err(|e| anyhow::anyhow!("{}", e))
                    }
                    ConfigShellCommand::Install { shell, force } => {
                        // Auto-write to shell config files and completions
                        handle_configure_shell(shell, force)
                        .map_err(|e| anyhow::anyhow!("{}", e))
                        .and_then(|scan_result| {
                            use worktrunk::styling::format_bash_with_gutter;

                            let changes_count = scan_result
                                .configured
                                .iter()
                                .filter(|r| !matches!(r.action, ConfigAction::AlreadyExists))
                                .count();

                            // Show configured shells grouped with their completions
                            let bold = Style::new().bold();
                            for result in &scan_result.configured {
                                let shell = result.shell;
                                let path = format_path_for_display(&result.path);
                                let message = format!(
                                    "{} shell extension for {bold}{shell}{bold:#} @ {bold}{path}{bold:#}",
                                    result.action.description()
                                );

                                // Use appropriate output function based on action
                                match result.action {
                                    ConfigAction::Added | ConfigAction::Created => {
                                        crate::output::success(message)?;
                                    }
                                    ConfigAction::AlreadyExists => {
                                        crate::output::info(message)?;
                                    }
                                    ConfigAction::WouldAdd | ConfigAction::WouldCreate => {
                                        crate::output::progress(message)?;
                                    }
                                }

                                // Show config line in preview (so user sees what will be added)
                                if matches!(
                                    result.action,
                                    ConfigAction::WouldAdd | ConfigAction::WouldCreate
                                ) {
                                    crate::output::gutter(format_bash_with_gutter(
                                        &result.config_line,
                                        "",
                                    ))?;
                                }

                                // Show completion result for this shell
                                // TODO: Inconsistent that shell extensions show gutter but completions don't.
                                // Completions are dynamic stubs (~30 lines) that call back to `wt` - not
                                // as meaningful to show, but the asymmetry is confusing.
                                if let Some(comp_result) = scan_result.completion_results.iter().find(|r| r.shell == shell) {
                                    let comp_path = format_path_for_display(&comp_result.path);
                                    let comp_message = format!(
                                        "{} completions for {bold}{shell}{bold:#} @ {bold}{comp_path}{bold:#}",
                                        comp_result.action.description()
                                    );
                                    match comp_result.action {
                                        ConfigAction::Added | ConfigAction::Created => {
                                            crate::output::success(comp_message)?;
                                        }
                                        ConfigAction::AlreadyExists => {
                                            crate::output::info(comp_message)?;
                                        }
                                        ConfigAction::WouldAdd | ConfigAction::WouldCreate => {
                                            crate::output::progress(comp_message)?;
                                        }
                                    }
                                } else if matches!(shell, worktrunk::shell::Shell::Zsh)
                                    && !matches!(result.action, ConfigAction::AlreadyExists)
                                {
                                    // Zsh completions are handled via lazy compdef in the init script
                                    // Only show this message for new installations
                                    crate::output::info(format!(
                                        "Completions for {bold}zsh{bold:#} via lazy compdef (no file needed)"
                                    ))?;
                                }
                            }

                            // Show skipped shells
                            for (shell, path) in &scan_result.skipped {
                                let path = format_path_for_display(path);
                                crate::output::hint(format!(
                                    "Skipped {bold}{shell}{bold:#}; {path} not found"
                                ))?;
                            }

                            // Exit with error if no shells configured
                            if scan_result.configured.is_empty() {
                                return Err(anyhow::anyhow!("No shell config files found"));
                            }

                            // Count total changes (config + completions)
                            let completion_changes = scan_result.completion_results
                                .iter()
                                .filter(|r| !matches!(r.action, ConfigAction::AlreadyExists))
                                .count();
                            let total_changes = changes_count + completion_changes;

                            // Summary
                            if total_changes > 0 {
                                crate::output::blank()?;
                                let plural = if total_changes == 1 { "" } else { "s" };
                                crate::output::success(format!(
                                    "Configured {total_changes} shell{plural}"
                                ))?;
                            } else {
                                crate::output::success("All shells already configured")?;
                            }

                            // Restart hint for current shell (only if config files changed, not just completions)
                            if changes_count > 0 {
                                let current_shell = std::env::var("SHELL")
                                    .ok()
                                    .and_then(|s| s.rsplit('/').next().map(String::from));

                                let current_shell_path =
                                    current_shell.as_ref().and_then(|shell_name| {
                                        scan_result
                                            .configured
                                            .iter()
                                            .filter(|r| {
                                                !matches!(r.action, ConfigAction::AlreadyExists)
                                            })
                                            .find(|r| {
                                                r.shell.to_string().eq_ignore_ascii_case(shell_name)
                                            })
                                            .map(|r| format_path_for_display(&r.path))
                                    });

                                if let Some(path) = current_shell_path {
                                    crate::output::hint(format!(
                                        "Restart shell or run: source {path}"
                                    ))?;
                                }
                            }
                            Ok(())
                        })
                    }
                    ConfigShellCommand::Uninstall { shell, force } => {
                        let explicit_shell = shell.is_some();
                        handle_unconfigure_shell(shell, force)
                            .map_err(|e| anyhow::anyhow!("{}", e))
                            .and_then(|scan_result| {
                                let shell_count = scan_result.results.len();
                                let completion_count = scan_result.completion_results.len();
                                let total_changes = shell_count + completion_count;

                                // Show shell extension results
                                for result in &scan_result.results {
                                    let bold = Style::new().bold();
                                    let shell = result.shell;
                                    let path = format_path_for_display(&result.path);

                                    crate::output::success(format!(
                                        "{} shell extension for {bold}{shell}{bold:#} @ {bold}{path}{bold:#}",
                                        result.action.description(),
                                    ))?;
                                }

                                // Show completion results
                                for result in &scan_result.completion_results {
                                    let bold = Style::new().bold();
                                    let shell = result.shell;
                                    let path = format_path_for_display(&result.path);

                                    crate::output::success(format!(
                                        "{} completions for {bold}{shell}{bold:#} @ {bold}{path}{bold:#}",
                                        result.action.description(),
                                    ))?;
                                }

                                // Show zsh lazy compdef message if zsh shell extension was removed
                                // but no completion file was removed (because zsh uses lazy compdef)
                                let bold = Style::new().bold();
                                let zsh_shell_removed = scan_result.results.iter().any(|r| {
                                    matches!(r.shell, worktrunk::shell::Shell::Zsh)
                                });
                                let zsh_completion_removed = scan_result.completion_results.iter().any(|r| {
                                    matches!(r.shell, worktrunk::shell::Shell::Zsh)
                                });
                                if zsh_shell_removed && !zsh_completion_removed {
                                    crate::output::info(format!(
                                        "Completions for {bold}zsh{bold:#} were via lazy compdef (no file to remove)"
                                    ))?;
                                }

                                // Show not found - warning if explicit shell, hint if auto-scan
                                for (shell, path) in &scan_result.not_found {
                                    let path = format_path_for_display(path);
                                    if explicit_shell {
                                        crate::output::warning(format!(
                                            "No shell integration found in {path}"
                                        ))?;
                                    } else {
                                        crate::output::hint(format!(
                                            "No {shell} integration in {path}"
                                        ))?;
                                    }
                                }

                                // Exit with info if nothing was found
                                if total_changes == 0 {
                                    if scan_result.not_found.is_empty() {
                                        crate::output::blank()?;
                                        crate::output::hint(
                                            "No shell integration found to remove",
                                        )?;
                                    }
                                    return Ok(());
                                }

                                // Summary
                                crate::output::blank()?;
                                let plural = if shell_count == 1 { "" } else { "s" };
                                crate::output::success(format!(
                                    "Removed integration from {shell_count} shell{plural}"
                                ))?;

                                // Hint about restarting shell (only if current shell was affected)
                                let current_shell = std::env::var("SHELL")
                                    .ok()
                                    .and_then(|s| s.rsplit('/').next().map(String::from));

                                let current_shell_affected = current_shell.as_ref().is_some_and(|shell_name| {
                                    scan_result.results.iter().any(|r| {
                                        r.shell.to_string().eq_ignore_ascii_case(shell_name)
                                    })
                                });

                                if current_shell_affected {
                                    crate::output::hint("Restart shell to complete uninstall")?;
                                }
                                Ok(())
                            })
                    }
                    ConfigShellCommand::Completions { shell } => {
                        // Generate completion script to stdout
                        completion::generate_completions(shell)
                            .map_err(|e| anyhow::anyhow!("Failed to generate completions: {}", e))
                    }
                }
            }
            ConfigCommand::Create => handle_config_create(),
            ConfigCommand::List => handle_config_list(),
            ConfigCommand::RefreshCache => handle_config_refresh_cache(),
            ConfigCommand::Status { action } => match action {
                StatusAction::Set { value, branch } => handle_config_status_set(value, branch),
                StatusAction::Unset { target } => handle_config_status_unset(target),
            },
            ConfigCommand::Approvals { action } => match action {
                ApprovalsCommand::Ask { force, all } => handle_standalone_ask_approvals(force, all),
                ApprovalsCommand::Clear { global } => handle_standalone_clear_approvals(global),
            },
        },
        Commands::Step { action } => match action {
            StepCommand::Commit {
                force,
                verify,
                stage,
            } => WorktrunkConfig::load()
                .context("Failed to load config")
                .and_then(|config| {
                    let stage_final = stage
                        .or_else(|| config.commit.and_then(|c| c.stage))
                        .unwrap_or_default();
                    handle_standalone_commit(force, !verify, stage_final)
                }),
            StepCommand::Squash {
                target,
                force,
                verify,
                stage,
            } => WorktrunkConfig::load()
                .context("Failed to load config")
                .and_then(|config| {
                    let stage_final = stage
                        .or_else(|| config.commit.and_then(|c| c.stage))
                        .unwrap_or_default();
                    let did_work =
                        handle_squash(target.as_deref(), force, !verify, false, stage_final)?;
                    if !did_work {
                        crate::output::info("Nothing to squash")?;
                    }
                    Ok(())
                }),
            StepCommand::Push {
                target,
                allow_merge_commits,
            } => handle_push(target.as_deref(), allow_merge_commits, "Pushed to", None),
            StepCommand::Rebase { target } => {
                handle_rebase(target.as_deref()).and_then(|result| match result {
                    RebaseResult::Rebased => Ok(()),
                    RebaseResult::UpToDate(branch) => {
                        crate::output::info(format!("Already up-to-date with {branch}"))?;
                        Ok(())
                    }
                })
            }
            StepCommand::PostCreate { force } => {
                handle_standalone_run_hook(HookType::PostCreate, force)
            }
            StepCommand::PostStart { force } => {
                handle_standalone_run_hook(HookType::PostStart, force)
            }
            StepCommand::PreCommit { force } => {
                handle_standalone_run_hook(HookType::PreCommit, force)
            }
            StepCommand::PreMerge { force } => {
                handle_standalone_run_hook(HookType::PreMerge, force)
            }
            StepCommand::PostMerge { force } => {
                handle_standalone_run_hook(HookType::PostMerge, force)
            }
        },
        #[cfg(unix)]
        Commands::Beta { action } => match action {
            BetaCommand::Select => handle_select(),
        },
        Commands::List {
            format,
            branches,
            remotes,
            full,
            progressive,
            no_progressive,
        } => {
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
                    let render_mode = RenderMode::detect(progressive_opt, cli.internal);
                    handle_list(format, show_branches, show_remotes, show_full, render_mode)
                })
        }
        Commands::Switch {
            branch,
            create,
            base,
            execute,
            force,
            verify,
        } => WorktrunkConfig::load()
            .context("Failed to load config")
            .and_then(|config| {
                // Execute switch operation (creates worktree, runs post-create hooks)
                let (result, resolved_branch) =
                    handle_switch(&branch, create, base.as_deref(), force, !verify, &config)?;

                // Show success message (temporal locality: immediately after worktree creation)
                // Pass cli.internal to indicate whether shell integration is active
                handle_switch_output(&result, &resolved_branch, execute.is_some(), cli.internal)?;

                // Now spawn post-start hooks (background processes, after success message)
                // Only run post-start commands when creating a NEW worktree, not when switching to existing
                // Note: If user declines post-start commands, continue anyway - they're optional
                if verify && let SwitchResult::Created { path, .. } = &result {
                    let repo = Repository::current();
                    let repo_root = repo.worktree_base()?;
                    let ctx = CommandContext::new(
                        &repo,
                        &config,
                        &resolved_branch,
                        path,
                        &repo_root,
                        force,
                    );
                    if let Err(e) = ctx.spawn_post_start_commands() {
                        // Only treat CommandNotApproved as non-fatal (user declined)
                        // Other errors should still fail
                        if !is_command_not_approved(&e) {
                            return Err(e);
                        }
                    }
                }

                // Execute user command after post-start hooks have been spawned
                if let Some(cmd) = execute {
                    execute_user_command(&cmd)?;
                }

                Ok(())
            }),
        Commands::Remove {
            worktrees,
            delete_branch,
            force_delete,
            background,
        } => (|| -> anyhow::Result<()> {
            // Validate conflicting flags
            if !delete_branch && force_delete {
                anyhow::bail!("Cannot use --force-delete with --no-delete-branch");
            }

            let repo = Repository::current();

            if worktrees.is_empty() {
                // No worktrees specified, remove current worktree
                let current_branch = repo.resolve_worktree_name("@")?;
                let result =
                    handle_remove(&current_branch, !delete_branch, force_delete, background)?;
                handle_remove_output(&result, None, false, background)
            } else {
                // When removing multiple worktrees, we need to handle the current worktree last
                // to avoid deleting the directory we're currently in
                let current_worktree = repo.worktree_root().ok();

                // Partition worktrees into current and others, storing resolved names
                let mut others = Vec::new();
                let mut current = None;

                for worktree_name in &worktrees {
                    // Resolve "@" to current branch (fail fast on errors like detached HEAD)
                    let resolved = repo.resolve_worktree_name(worktree_name)?;

                    // Check if this is the current worktree by comparing branch names
                    if let Ok(Some(worktree_path)) = repo.worktree_for_branch(&resolved) {
                        if Some(&worktree_path) == current_worktree.as_ref() {
                            current = Some(resolved);
                        } else {
                            others.push(resolved);
                        }
                    } else {
                        // Worktree doesn't exist or branch not found, will error when we try to remove
                        others.push(resolved);
                    }
                }

                // Remove others first, then current last
                // Progress messages shown by handle_remove_output for all cases
                for resolved in &others {
                    let result = handle_remove(resolved, !delete_branch, force_delete, background)?;
                    handle_remove_output(&result, Some(resolved), false, background)?;
                }

                // Remove current worktree last (if it was in the list)
                if let Some(resolved) = current {
                    let result =
                        handle_remove(&resolved, !delete_branch, force_delete, background)?;
                    handle_remove_output(&result, Some(&resolved), false, background)?;
                }

                Ok(())
            }
        })(),
        Commands::Merge {
            target,
            squash,
            no_squash,
            commit,
            no_commit,
            remove,
            no_remove,
            verify,
            no_verify,
            force,
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
                let remove_default = merge_config.and_then(|m| m.remove).unwrap_or(true);
                let verify_default = merge_config.and_then(|m| m.verify).unwrap_or(true);

                // CLI flags override config, config overrides defaults
                let squash_final = flag_pair(squash, no_squash).unwrap_or(squash_default);
                let commit_final = flag_pair(commit, no_commit).unwrap_or(commit_default);
                let remove_final = flag_pair(remove, no_remove).unwrap_or(remove_default);
                let verify_final = flag_pair(verify, no_verify).unwrap_or(verify_default);

                // Stage defaults from [commit] config section
                let stage_final = stage
                    .or_else(|| config.commit.and_then(|c| c.stage))
                    .unwrap_or_default();

                handle_merge(
                    target.as_deref(),
                    squash_final,
                    commit_final,
                    remove_final,
                    verify_final,
                    force,
                    stage_final,
                )
            }),
    };

    if let Err(e) = result {
        // Error messages are already formatted with emoji and colors
        // Route through output system to respect mode:
        // - Interactive mode: errors go to stdout
        // - Directive mode: errors go to stderr
        let _ = output::error(e.to_string());

        // Preserve exit code from child processes (especially for signals like SIGINT)
        let code = exit_code(&e).unwrap_or(1);
        process::exit(code);
    }
}
