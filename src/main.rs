use anstyle::Style;
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use std::process;
use worktrunk::config::WorktrunkConfig;
use worktrunk::git::{GitError, GitResultExt, Repository};
use worktrunk::styling::{SUCCESS_EMOJI, println};

mod commands;
mod display;
mod llm;
mod output;

use commands::{
    ConfigAction, Shell, handle_complete, handle_completion, handle_config_help,
    handle_config_init, handle_config_list, handle_configure_shell, handle_dev_run_hook,
    handle_init, handle_list, handle_merge, handle_push, handle_remove, handle_switch,
};
use output::{handle_remove_output, handle_switch_output};

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum OutputFormat {
    /// Human-readable table format
    Table,
    /// JSON format
    Json,
}

#[derive(Parser)]
#[command(name = "wt")]
#[command(about = "Git worktree management", long_about = None)]
#[command(version = env!("VERGEN_GIT_DESCRIBE"))]
#[command(disable_help_subcommand = true)]
struct Cli {
    /// Enable verbose output (show git commands and debug info)
    #[arg(long, short = 'v', global = true)]
    verbose: bool,

    /// Use internal mode (outputs directives for shell wrapper)
    #[arg(long, global = true, hide = true)]
    internal: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Initialize global configuration file with examples
    Init,
    /// List all configuration files and their locations
    List,
    /// Show setup guide for AI-generated commit messages
    Help,
    /// Configure shell by writing to config files
    Shell {
        /// Specific shell to configure (default: all shells with existing config files)
        #[arg(long, value_enum)]
        shell: Option<Shell>,

        /// Skip confirmation prompt
        #[arg(short, long)]
        force: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum HookType {
    PostCreate,
    PostStart,
    PreCommit,
    PreSquash,
    PreMerge,
    PostMerge,
}

impl HookType {
    /// Returns the kebab-case name for display and error messages
    pub fn as_str(self) -> &'static str {
        match self {
            HookType::PostCreate => "post-create",
            HookType::PostStart => "post-start",
            HookType::PreCommit => "pre-commit",
            HookType::PreSquash => "pre-squash",
            HookType::PreMerge => "pre-merge",
            HookType::PostMerge => "post-merge",
        }
    }
}

#[derive(Subcommand)]
enum DevCommand {
    /// Run a project hook for testing
    RunHook {
        /// Hook type to run
        hook_type: HookType,

        /// Skip command approval prompts
        #[arg(short, long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum Commands {
    /// Generate shell integration code
    Init {
        /// Shell to generate code for
        shell: Shell,
    },

    /// Manage configuration
    Config {
        #[command(subcommand)]
        action: ConfigCommand,
    },

    /// Development and testing utilities
    #[command(hide = true)]
    Dev {
        #[command(subcommand)]
        action: DevCommand,
    },

    /// List all worktrees
    List {
        /// Output format
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,

        /// Also display branches that don't have worktrees
        #[arg(long)]
        branches: bool,
    },

    /// Switch to a worktree
    Switch {
        /// Branch name or worktree path
        branch: String,

        /// Create a new branch
        #[arg(short = 'c', long)]
        create: bool,

        /// Base branch to create from (only with --create)
        #[arg(short = 'b', long)]
        base: Option<String>,

        /// Execute command after switching
        #[arg(short = 'x', long)]
        execute: Option<String>,

        /// Skip confirmation prompt
        #[arg(short = 'f', long)]
        force: bool,

        /// Skip all project hooks (post-create, post-start)
        #[arg(long)]
        no_hooks: bool,
    },

    /// Finish current worktree, returning to primary if current
    Remove {
        /// Worktree names or branches to remove (defaults to current worktree if none specified)
        worktrees: Vec<String>,
    },

    /// Push changes between worktrees
    Push {
        /// Target branch (defaults to default branch)
        target: Option<String>,

        /// Allow pushing merge commits (non-linear history)
        #[arg(long)]
        allow_merge_commits: bool,
    },

    /// Merge worktree into target branch
    #[command(long_about = "Merge worktree into target branch

LIFECYCLE

The merge operation follows a strict order designed for fail-fast execution:

1. Validate branches
   Verifies current branch exists (not detached HEAD) and determines target branch
   (defaults to repository's default branch).

2. Run pre-merge commands
   Runs commands from project config's [pre-merge-command] before any git operations.
   These receive {target} placeholder for the target branch. Commands run sequentially
   and any failure aborts the merge immediately. Skip with --no-verify.

3. Auto-commit uncommitted changes
   If working tree has uncommitted changes, stages all changes (git add -A) and commits
   with LLM-generated message.

4. Squash commits
   If --squash specified, counts commits since merge base with target branch. When
   multiple commits exist, squashes them into one with LLM-generated message.

5. Rebase onto target
   Rebases current branch onto target branch. Detects conflicts and aborts if found.

6. Push to target
   Fast-forward pushes to target branch. Rejects non-fast-forward pushes (ensures
   linear history).

7. Clean up worktree
   Removes current worktree and switches primary worktree to target branch if needed.
   Skip removal with --keep.

EXAMPLES

Basic merge to main:
  wt merge

Squash commits before merging:
  wt merge --squash

Keep worktree after merging:
  wt merge --keep

Skip pre-merge commands:
  wt merge --no-verify")]
    Merge {
        /// Target branch to merge into (defaults to default branch)
        target: Option<String>,

        /// Squash all commits into one before merging
        #[arg(short, long)]
        squash: bool,

        /// Keep worktree after merging (don't remove)
        #[arg(short, long)]
        keep: bool,

        /// Skip all project hooks (pre-merge-command)
        #[arg(long)]
        no_hooks: bool,

        /// Skip approval prompts for commands
        #[arg(short, long)]
        force: bool,
    },

    /// Generate shell completion script (deprecated - use init instead)
    #[command(hide = true)]
    Completion {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },

    /// Internal completion helper (hidden)
    #[command(hide = true)]
    Complete {
        /// Arguments to complete
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

fn main() {
    let cli = Cli::parse();

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
        Commands::Init { shell } => {
            let mut cli_cmd = Cli::command();
            handle_init(shell, &mut cli_cmd).git_err()
        }
        Commands::Config { action } => match action {
            ConfigCommand::Init => handle_config_init(),
            ConfigCommand::List => handle_config_list(),
            ConfigCommand::Help => handle_config_help(),
            ConfigCommand::Shell { shell, force } => {
                handle_configure_shell(shell, force)
                    .map(|results| {
                        use anstyle::{AnsiColor, Color};

                        // Count actual changes (not AlreadyExists)
                        let changes_count = results
                            .iter()
                            .filter(|r| !matches!(r.action, ConfigAction::AlreadyExists))
                            .count();

                        if changes_count == 0 {
                            // All shells already configured
                            let green = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green)));
                            println!("{SUCCESS_EMOJI} {green}All shells already configured{green:#}");
                            return;
                        }

                        // Show what was done (instant operations, no progress needed)
                        for result in &results {
                            use worktrunk::styling::format_bash_with_gutter;
                            let bold = Style::new().bold();
                            let shell = result.shell;
                            let path = result.path.display();
                            println!(
                                "{} {bold}{shell}{bold:#} {path}",
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
                    .git_err()
            }
        },
        Commands::Dev { action } => match action {
            DevCommand::RunHook { hook_type, force } => handle_dev_run_hook(hook_type, force),
        },
        Commands::List { format, branches } => handle_list(format, branches),
        Commands::Switch {
            branch,
            create,
            base,
            execute,
            force,
            no_hooks,
        } => WorktrunkConfig::load()
            .git_context("Failed to load config")
            .and_then(|config| {
                handle_switch(&branch, create, base.as_deref(), force, no_hooks, &config)
                    .and_then(|result| handle_switch_output(&result, &branch, execute.as_deref()))
            }),
        Commands::Remove { worktrees } => {
            if worktrees.is_empty() {
                // No worktrees specified, remove current worktree
                handle_remove(None).and_then(|result| handle_remove_output(&result))
            } else {
                // When removing multiple worktrees, we need to handle the current worktree last
                // to avoid deleting the directory we're currently in
                (|| -> Result<(), GitError> {
                    let repo = Repository::current();
                    let current_worktree = repo.worktree_root().ok();

                    // Partition worktrees into current and others
                    let mut others = Vec::new();
                    let mut current = None;

                    for worktree_name in worktrees.iter() {
                        // Check if this is the current worktree by comparing branch names
                        if let Ok(Some(worktree_path)) = repo.worktree_for_branch(worktree_name) {
                            if Some(&worktree_path) == current_worktree.as_ref() {
                                current = Some(worktree_name);
                            } else {
                                others.push(worktree_name);
                            }
                        } else {
                            // Worktree doesn't exist or branch not found, will error when we try to remove
                            others.push(worktree_name);
                        }
                    }

                    // Remove others first
                    for worktree in others.iter() {
                        // Show progress before starting removal
                        use anstyle::Style;
                        use worktrunk::styling::CYAN;
                        let bold = Style::new().bold();
                        output::progress(format!(
                            "🔄 {CYAN}Removing worktree for {bold}{worktree}{bold:#}...{CYAN:#}"
                        ))?;

                        let result = handle_remove(Some(worktree.as_str()))?;
                        handle_remove_output(&result)?;
                    }

                    // Remove current worktree last (if it was in the list)
                    if let Some(current_name) = current {
                        // Show progress before starting removal
                        use anstyle::Style;
                        use worktrunk::styling::CYAN;
                        let bold = Style::new().bold();
                        output::progress(format!(
                            "🔄 {CYAN}Removing worktree for {bold}{current_name}{bold:#}...{CYAN:#}"
                        ))?;

                        let result = handle_remove(Some(current_name.as_str()))?;
                        handle_remove_output(&result)?;
                    }

                    Ok(())
                })()
            }
        }
        Commands::Push {
            target,
            allow_merge_commits,
        } => handle_push(target.as_deref(), allow_merge_commits, "Pushed to"),
        Commands::Merge {
            target,
            squash,
            keep,
            no_hooks,
            force,
        } => handle_merge(target.as_deref(), squash, keep, no_hooks, force),
        Commands::Completion { shell } => {
            let mut cli_cmd = Cli::command();
            handle_completion(shell, &mut cli_cmd);
            Ok(())
        }
        Commands::Complete { args } => handle_complete(args),
    };

    if let Err(e) = result {
        // Error messages are already formatted with emoji and colors
        eprintln!("{}", e);
        process::exit(1);
    }
}
