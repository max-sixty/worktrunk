use anyhow::Context;
use clap::FromArgMatches;
use color_print::cformat;
use std::path::PathBuf;
use std::process;
use worktrunk::config::{WorktrunkConfig, set_config_path};
use worktrunk::git::{Repository, exit_code, is_command_not_approved, set_base_path};
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{format_with_gutter, println};

mod cli;
mod commands;
mod completion;
mod display;
pub(crate) mod help_pager;
mod llm;
mod md_help;
mod output;

pub use crate::cli::OutputFormat;

use commands::command_executor::CommandContext;
#[cfg(unix)]
use commands::handle_select;
use commands::worktree::{SwitchResult, handle_push};
use commands::{
    ConfigAction, RebaseResult, SquashResult, handle_cache_clear, handle_cache_refresh,
    handle_cache_show, handle_config_create, handle_config_show, handle_configure_shell,
    handle_init, handle_list, handle_merge, handle_rebase, handle_remove, handle_remove_by_path,
    handle_remove_current, handle_squash, handle_standalone_add_approvals,
    handle_standalone_clear_approvals, handle_standalone_commit, handle_standalone_run_hook,
    handle_switch, handle_unconfigure_shell, handle_var_clear, handle_var_get, handle_var_set,
    resolve_worktree_path_first,
};
use output::{execute_user_command, handle_remove_output, handle_switch_output};

use cli::{
    ApprovalsCommand, CacheCommand, Cli, Commands, ConfigCommand, ConfigShellCommand,
    ListSubcommand, StepCommand, VarCommand,
};
use worktrunk::HookType;

/// Custom help handling for pager support and markdown rendering.
///
/// We intercept help requests to provide:
/// 1. **Pager support**: Help is shown through `less` (like git)
/// 2. **Markdown rendering**: `## Headers` become green, code blocks are dimmed
///
/// Uses `Error::render()` to get clap's pre-formatted help, which already
/// respects `-h` (short) vs `--help` (long) distinction.
fn maybe_handle_help_with_pager() -> bool {
    use clap::ColorChoice;
    use clap::error::ErrorKind;

    let args: Vec<String> = std::env::args().collect();

    // Check for --help-md flag (output raw markdown without ANSI rendering)
    if args.iter().any(|a| a == "--help-md") {
        let mut cmd = cli::build_command();
        cmd = cmd.color(ColorChoice::Never); // No ANSI codes for raw markdown

        // Replace --help-md with --help for clap
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

        if let Err(err) = cmd.try_get_matches_from_mut(filtered_args)
            && matches!(
                err.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
            )
        {
            // err.render() returns the help text clap already built
            println!("{}", err.render());
            process::exit(0);
        }
        // Fall through if not a help request
    }

    let mut cmd = cli::build_command();
    cmd = cmd.color(ColorChoice::Always); // Force clap to emit ANSI codes

    match cmd.try_get_matches_from_mut(args) {
        Ok(_) => false, // Normal args, not help
        Err(err) => {
            match err.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
                    // err.render() returns a StyledStr containing ANSI codes.
                    // Use .ansi() to preserve them; .to_string() strips ANSI codes.
                    let mut help = err.render().ansi().to_string();

                    // Render markdown sections to ANSI
                    help = md_help::render_markdown_in_help(&help);

                    if let Err(e) = help_pager::show_help_in_pager(&help) {
                        log::debug!("Pager invocation failed: {}", e);
                        eprintln!("{}", help);
                    }
                    process::exit(0);
                }
                ErrorKind::DisplayVersion => {
                    // Print to stderr - stdout is reserved for data/scripts
                    // Use eprint! because clap's Error Display already includes a trailing newline
                    eprint!("{}", err);
                    process::exit(0);
                }
                _ => {
                    // Not help or version - will be re-parsed by Cli::parse()
                    false
                }
            }
        }
    }
}

/// Enhance clap errors with command-specific hints, then exit.
///
/// For `wt switch` missing the branch argument, adds hints about shortcuts.
fn enhance_and_exit_error(err: clap::Error) -> ! {
    use clap::error::ErrorKind;
    use color_print::ceprintln;

    // Enhance `wt switch` missing argument error with shortcut hints.
    // Safe in directive mode: hints go to stderr, only stdout is eval'd.
    if err.kind() == ErrorKind::MissingRequiredArgument && format!("{err}").contains("wt switch") {
        eprint!("{}", err.render().ansi());
        ceprintln!("<green,bold>Quick switches:</>");
        ceprintln!("  <cyan,bold>wt switch ^</>    default branch's worktree");
        ceprintln!("  <cyan,bold>wt switch -</>    previous worktree");
        ceprintln!("  <cyan,bold>wt switch @</>    current branch's worktree");
        ceprintln!("  <cyan,bold>wt select</>      interactive picker");
        process::exit(2);
    }

    err.exit()
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
                    ConfigShellCommand::Init { shell } => {
                        // Generate shell code to stdout
                        handle_init(shell).map_err(|e| anyhow::anyhow!("{}", e))
                    }
                    ConfigShellCommand::Install { shell, force } => {
                        // Auto-write to shell config files and completions
                        handle_configure_shell(shell, force)
                        .map_err(|e| anyhow::anyhow!("{}", e))
                        .and_then(|scan_result| {

                            // Count shells that became (more) configured
                            // A shell counts if any of its components changed (extension or completions)
                            let shells_configured_count = scan_result
                                .configured
                                .iter()
                                .filter(|ext_result| {
                                    let ext_changed =
                                        !matches!(ext_result.action, ConfigAction::AlreadyExists);
                                    let comp_changed = scan_result
                                        .completion_results
                                        .iter()
                                        .find(|c| c.shell == ext_result.shell)
                                        .is_some_and(|c| {
                                            !matches!(c.action, ConfigAction::AlreadyExists)
                                        });
                                    ext_changed || comp_changed
                                })
                                .count();

                            // Show configured shells grouped with their completions
                            for result in &scan_result.configured {
                                let shell = result.shell;
                                let path = format_path_for_display(&result.path);
                                // For bash/zsh, completions are inline in the init script
                                let what = if matches!(shell, worktrunk::shell::Shell::Bash | worktrunk::shell::Shell::Zsh) {
                                    "shell extension & completions"
                                } else {
                                    "shell extension"
                                };
                                let message = cformat!(
                                    "{} {what} for <bold>{shell}</> @ <bold>{path}</>",
                                    result.action.description()
                                );

                                // Use appropriate output function based on action
                                // Note: WouldAdd/WouldCreate are only returned in preview mode,
                                // which is handled internally by prompt_for_confirmation()
                                match result.action {
                                    ConfigAction::Added | ConfigAction::Created => {
                                        crate::output::success(message)?;
                                    }
                                    ConfigAction::AlreadyExists => {
                                        crate::output::info(message)?;
                                    }
                                    ConfigAction::WouldAdd | ConfigAction::WouldCreate => {
                                        unreachable!("Preview actions handled by confirmation prompt")
                                    }
                                }

                                // Show completion result for this shell
                                // TODO: Inconsistent that shell extensions show gutter but completions don't.
                                // Completions are dynamic stubs (~30 lines) that call back to `wt` - not
                                // as meaningful to show, but the asymmetry is confusing.
                                if let Some(comp_result) = scan_result.completion_results.iter().find(|r| r.shell == shell) {
                                    let comp_path = format_path_for_display(&comp_result.path);
                                    let comp_message = cformat!(
                                        "{} completions for <bold>{shell}</> @ <bold>{comp_path}</>",
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
                                            unreachable!("Preview actions handled by confirmation prompt")
                                        }
                                    }
                                }
                            }

                            // Show skipped shells
                            for (shell, path) in &scan_result.skipped {
                                let path = format_path_for_display(path);
                                crate::output::hint(cformat!(
                                    "Skipped <bright-black>{shell}</>; {path} not found"
                                ))?;
                            }

                            // Exit with error if no shells configured
                            if scan_result.configured.is_empty() {
                                return Err(worktrunk::git::GitError::Other {
                                    message: "No shell config files found".into(),
                                }
                                .into());
                            }

                            // Summary
                            if shells_configured_count > 0 {
                                crate::output::blank()?;
                                let plural = if shells_configured_count == 1 { "" } else { "s" };
                                crate::output::success(format!(
                                    "Configured {shells_configured_count} shell{plural}"
                                ))?;
                            } else {
                                // No action: all shells were already configured
                                crate::output::info("All shells already configured")?;
                            }

                            // Restart hint: only shown if the current shell's extension changed
                            // (completions are auto-sourced or sourced separately, no restart needed)
                            if shells_configured_count > 0 {
                                let current_shell = std::env::var("SHELL")
                                    .ok()
                                    .and_then(|s| s.rsplit('/').next().map(String::from));

                                // Find if current shell had its extension changed (not just completions)
                                let current_shell_result =
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
                                    });

                                if let Some(result) = current_shell_result {
                                    // Fish auto-sources from conf.d, so just say "Restart shell"
                                    // Bash/Zsh can source directly for immediate activation
                                    if matches!(result.shell, worktrunk::shell::Shell::Fish) {
                                        crate::output::hint("Restart shell to activate")?;
                                    } else {
                                        let path = format_path_for_display(&result.path);
                                        crate::output::hint(format!(
                                            "Restart shell or run: source {path}"
                                        ))?;
                                    }
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

                                    crate::output::success(cformat!(
                                        "{} {what} for <bold>{shell}</> @ <bold>{path}</>",
                                        result.action.description(),
                                    ))?;
                                }

                                // Show completion results
                                for result in &scan_result.completion_results {
                                    let shell = result.shell;
                                    let path = format_path_for_display(&result.path);

                                    crate::output::success(cformat!(
                                        "{} completions for <bold>{shell}</> @ <bold>{path}</>",
                                        result.action.description(),
                                    ))?;
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
                                        crate::output::warning(format!(
                                            "No {what} found in {path}"
                                        ))?;
                                    } else {
                                        crate::output::hint(cformat!(
                                            "No <bright-black>{shell}</> {what} in {path}"
                                        ))?;
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
                                        crate::output::warning(format!(
                                            "No completions found in {path}"
                                        ))?;
                                    } else {
                                        crate::output::hint(cformat!(
                                            "No <bright-black>{shell}</> completions in {path}"
                                        ))?;
                                    }
                                }

                                // Exit with info if nothing was found
                                let all_not_found = scan_result.not_found.len()
                                    + scan_result.completion_not_found.len();
                                if total_changes == 0 {
                                    if all_not_found == 0 {
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

                                let current_shell_affected =
                                    current_shell.as_ref().is_some_and(|shell_name| {
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
                }
            }
            ConfigCommand::Create => handle_config_create(),
            ConfigCommand::Show => handle_config_show(),
            ConfigCommand::Cache { action } => match action {
                CacheCommand::Show => handle_cache_show(),
                CacheCommand::Clear { cache_type } => handle_cache_clear(cache_type),
                CacheCommand::Refresh => handle_cache_refresh(),
            },
            ConfigCommand::Var { action } => match action {
                VarCommand::Get {
                    key,
                    refresh,
                    branch,
                } => handle_var_get(&key, refresh, branch),
                VarCommand::Set { key, value, branch } => handle_var_set(&key, value, branch),
                VarCommand::Clear { key, branch, all } => handle_var_clear(&key, branch, all),
            },
            ConfigCommand::Approvals { action } => match action {
                ApprovalsCommand::Add { force, all } => handle_standalone_add_approvals(force, all),
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
                    match handle_squash(target.as_deref(), force, !verify, false, stage_final)? {
                        SquashResult::Squashed | SquashResult::NoNetChanges => {}
                        SquashResult::NoCommitsAhead(branch) => {
                            crate::output::info(format!(
                                "Nothing to squash; no commits ahead of {branch}"
                            ))?;
                        }
                        SquashResult::AlreadySingleCommit => {
                            crate::output::info("Nothing to squash; already a single commit")?;
                        }
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
        Commands::Select => handle_select(cli.internal),
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
                        let render_mode = RenderMode::detect(progressive_opt, cli.internal);
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
        } => WorktrunkConfig::load()
            .context("Failed to load config")
            .and_then(|config| {
                // Validate conflicting flags
                if !delete_branch && force_delete {
                    return Err(worktrunk::git::GitError::Other {
                        message: "Cannot use --force-delete with --no-delete-branch".into(),
                    }
                    .into());
                }

                if worktrees.is_empty() {
                    // No worktrees specified, remove current worktree
                    // Uses path-based removal to handle detached HEAD state
                    let result = handle_remove_current(!delete_branch, force_delete, background)?;
                    handle_remove_output(&result, None, background)
                } else {
                    use worktrunk::git::ResolvedWorktree;

                    let repo = Repository::current();
                    // When removing multiple worktrees, we need to handle the current worktree last
                    // to avoid deleting the directory we're currently in
                    let current_worktree = repo.worktree_root().ok();

                    // Partition worktrees into current, others, and branch-only using path-first
                    // resolution, which checks expected path before falling back to branch lookup
                    let mut others = Vec::new();
                    let mut branch_only = Vec::new();
                    let mut current: Option<(PathBuf, Option<String>)> = None;

                    for worktree_name in &worktrees {
                        match resolve_worktree_path_first(&repo, worktree_name, &config)? {
                            ResolvedWorktree::Worktree { path, branch } => {
                                if Some(&path) == current_worktree.as_ref() {
                                    current = Some((path, branch));
                                } else {
                                    others.push((path, branch));
                                }
                            }
                            ResolvedWorktree::BranchOnly { branch } => {
                                branch_only.push(branch);
                            }
                        }
                    }

                    // Remove other worktrees first
                    for (path, branch) in &others {
                        if let Some(branch_name) = branch {
                            let result = handle_remove(
                                branch_name,
                                !delete_branch,
                                force_delete,
                                background,
                            )?;
                            handle_remove_output(&result, Some(branch_name), background)?;
                        } else {
                            // Non-current worktree is detached - remove by path (no branch to delete)
                            let result =
                                handle_remove_by_path(path, None, force_delete, background)?;
                            handle_remove_output(&result, None, background)?;
                        }
                    }

                    // Handle branch-only cases (no worktree)
                    for branch in &branch_only {
                        let result =
                            handle_remove(branch, !delete_branch, force_delete, background)?;
                        handle_remove_output(&result, Some(branch), background)?;
                    }

                    // Remove current worktree last (if it was in the list)
                    if let Some((_path, branch)) = current {
                        let result =
                            handle_remove_current(!delete_branch, force_delete, background)?;
                        handle_remove_output(&result, branch.as_deref(), background)?;
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

    // Emit shell script (directive mode) or no-op (interactive mode)
    // Must be called before error handling so cd happens even on failure
    // (matches shell wrapper behavior which evals script regardless of exit code)
    let _ = output::terminate_output();

    if let Err(e) = result {
        use worktrunk::styling::ERROR_EMOJI;

        // GitError and WorktrunkError produce styled output via Display
        if let Some(err) = e.downcast_ref::<worktrunk::git::GitError>() {
            let _ = output::print(err.to_string());
        } else if let Some(err) = e.downcast_ref::<worktrunk::git::WorktrunkError>() {
            let _ = output::print(err.to_string());
        } else {
            // Anyhow error - format with emoji, multi-line root cause gets gutter
            let msg = e.to_string();
            let root_cause = e.root_cause().to_string();
            let _ = output::print(cformat!("{ERROR_EMOJI} <red>{msg}</>"));
            if msg != root_cause && root_cause.contains('\n') {
                let _ = output::gutter(format_with_gutter(&root_cause, "", None));
            }
        }

        // Preserve exit code from child processes (especially for signals like SIGINT)
        let code = exit_code(&e).unwrap_or(1);
        process::exit(code);
    }
}
