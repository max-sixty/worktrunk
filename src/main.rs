use anstyle::Style;
use anyhow::Context;
use clap::FromArgMatches;
use std::process;
use worktrunk::config::{WorktrunkConfig, set_config_path};
use worktrunk::git::{Repository, exit_code, is_command_not_approved, set_base_path};
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{SUCCESS_EMOJI, println};

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
    ConfigAction, handle_config_create, handle_config_list, handle_config_refresh_cache,
    handle_config_status_set, handle_config_status_unset, handle_configure_shell, handle_init,
    handle_list, handle_merge, handle_rebase, handle_remove, handle_squash,
    handle_standalone_ask_approvals, handle_standalone_clear_approvals, handle_standalone_commit,
    handle_standalone_run_hook, handle_switch,
};
use output::{execute_user_command, handle_remove_output, handle_switch_output};

use cli::{
    ApprovalsCommand, Cli, Commands, ConfigCommand, ConfigShellCommand, StandaloneCommand,
    StatusAction,
};

/// Try to handle --help flag with pager before clap processes it
fn maybe_handle_help_with_pager() -> bool {
    use clap::ColorChoice;
    use clap::error::ErrorKind;

    let mut cmd = cli::build_command();
    cmd = cmd.color(ColorChoice::Always); // Force clap to always emit ANSI codes

    // DON'T render markdown yet - let clap generate help first

    match cmd.try_get_matches_from_mut(std::env::args()) {
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
    termimad::crossterm::style::force_color_output(true);

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
            ConfigCommand::Shell { action } => match action {
                ConfigShellCommand::Init {
                    shell,
                    command_name,
                } => {
                    // Generate shell code to stdout
                    let mut cli_cmd = cli::build_command();
                    handle_init(shell, command_name, &mut cli_cmd)
                        .map_err(|e| anyhow::anyhow!("{}", e))
                }
                ConfigShellCommand::Install {
                    shell,
                    force,
                    command_name,
                } => {
                    // Auto-write to shell config files
                    handle_configure_shell(shell, force, command_name)
                        .map_err(|e| anyhow::anyhow!("{}", e))
                        .map(|results| {
                            use anstyle::{AnsiColor, Color};

                            // Count actual changes (not AlreadyExists)
                            let changes_count = results
                                .iter()
                                .filter(|r| !matches!(r.action, ConfigAction::AlreadyExists))
                                .count();

                            if changes_count == 0 {
                                // All shells already configured
                                let green =
                                    Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green)));
                                println!(
                                    "{SUCCESS_EMOJI} {green}All shells already configured{green:#}"
                                );
                                return;
                            }

                            // Show what was done (instant operations, no progress needed)
                            for result in &results {
                                use worktrunk::styling::format_bash_with_gutter;
                                let bold = Style::new().bold();
                                let shell = result.shell;
                                let path = format_path_for_display(&result.path);

                                println!(
                                    "{} {} {bold}{shell}{bold:#} {path}",
                                    result.action.emoji(),
                                    result.action.description(),
                                );
                                // Show config line with gutter
                                print!("{}", format_bash_with_gutter(&result.config_line, ""));
                            }

                            // Success summary
                            println!();
                            let green = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green)));
                            let plural = if changes_count == 1 { "" } else { "s" };
                            println!(
                                "{SUCCESS_EMOJI} {green}Configured {changes_count} shell{plural}{green:#}"
                            );

                            // Show hint about restarting shell
                            println!();
                            use worktrunk::styling::{HINT, HINT_EMOJI};
                            println!(
                                "{HINT_EMOJI} {HINT}Restart your shell or run: source <config-file>{HINT:#}"
                            );
                        })
                }
            },
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
        Commands::Standalone { action } => match action {
            StandaloneCommand::RunHook { hook_type, force } => {
                handle_standalone_run_hook(hook_type, force)
            }
            StandaloneCommand::Commit { force, verify } => handle_standalone_commit(force, !verify),
            StandaloneCommand::Squash {
                target,
                force,
                verify,
            } => handle_squash(target.as_deref(), force, !verify, false, false, true).map(|_| ()),
            StandaloneCommand::Push {
                target,
                allow_merge_commits,
            } => handle_push(target.as_deref(), allow_merge_commits, "Pushed to", None),
            StandaloneCommand::Rebase { target } => handle_rebase(target.as_deref()).map(|_| ()),
            #[cfg(unix)]
            StandaloneCommand::Select => handle_select(),
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
                handle_switch_output(&result, &resolved_branch, execute.is_some())?;

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
            commit,
            remove,
            verify,
            force,
            tracked_only,
        } => handle_merge(
            target.as_deref(),
            !squash,
            !commit,
            !remove,
            !verify,
            force,
            tracked_only,
        ),
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
