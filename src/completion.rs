use std::cell::RefCell;
use std::ffi::{OsStr, OsString};
use std::io;

use clap::Command;
use clap_complete::engine::{ArgValueCompleter, CompletionCandidate, ValueCompleter};
use clap_complete::env::{CompleteEnv, EnvCompleter};

use crate::cli;
use crate::display::format_relative_time_short;
use worktrunk::git::{BranchCategory, Repository};
use worktrunk::shell::Shell;

/// Handle shell-initiated completion requests via `COMPLETE=$SHELL wt`
pub fn maybe_handle_env_completion() -> bool {
    if std::env::var_os("COMPLETE").is_none() {
        return false;
    }

    let args: Vec<OsString> = std::env::args_os().collect();
    CONTEXT.with(|ctx| *ctx.borrow_mut() = Some(CompletionContext { args: args.clone() }));

    let current_dir = std::env::current_dir().ok();
    let handled = CompleteEnv::with_factory(completion_command)
        .try_complete(args, current_dir.as_deref())
        .unwrap_or_else(|err| err.exit());

    CONTEXT.with(|ctx| ctx.borrow_mut().take());
    handled
}

/// Generate completion script for explicit `wt config shell completions <shell>` command
pub fn generate_completions(shell: Shell) -> io::Result<()> {
    generate_completions_to_writer(shell, &mut io::stdout())
}

/// Generate completion script to a writer (for writing to files)
pub fn generate_completions_to_writer(shell: Shell, writer: &mut dyn io::Write) -> io::Result<()> {
    let completer = std::env::current_exe()?;
    let completer = completer.to_string_lossy();
    let var = "COMPLETE";

    // Use the shell-specific EnvCompleter to write the registration script
    // Parameters: var, name (shell context), bin (user display - same as name), completer (binary path)
    match shell {
        Shell::Bash => {
            clap_complete::env::Bash.write_registration(var, "wt", "wt", &completer, writer)
        }
        Shell::Zsh => {
            clap_complete::env::Zsh.write_registration(var, "wt", "wt", &completer, writer)
        }
        Shell::Fish => {
            clap_complete::env::Fish.write_registration(var, "wt", "wt", &completer, writer)
        }
    }
}

/// Branch completion without additional context filtering (e.g., --base, merge target).
pub fn branch_value_completer() -> ArgValueCompleter {
    ArgValueCompleter::new(BranchCompleter {
        suppress_with_create: false,
    })
}

/// Branch completion for positional arguments that represent worktrees (switch/remove).
pub fn worktree_branch_completer() -> ArgValueCompleter {
    ArgValueCompleter::new(BranchCompleter {
        suppress_with_create: true,
    })
}

#[derive(Clone, Copy)]
struct BranchCompleter {
    suppress_with_create: bool,
}

impl ValueCompleter for BranchCompleter {
    fn complete(&self, _current: &OsStr) -> Vec<CompletionCandidate> {
        complete_branches(self.suppress_with_create)
    }
}

fn complete_branches(suppress_with_create: bool) -> Vec<CompletionCandidate> {
    if suppress_with_create && suppress_switch_branch_completion() {
        return Vec::new();
    }

    let branches = match Repository::current().branches_for_completion() {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };

    if branches.is_empty() {
        return Vec::new();
    }

    branches
        .into_iter()
        .map(|branch| {
            let time_str = format_relative_time_short(branch.timestamp);
            let help = match branch.category {
                BranchCategory::Worktree | BranchCategory::Local => time_str,
                BranchCategory::Remote(remote) => format!("{}, {}", time_str, remote),
            };
            CompletionCandidate::new(branch.name).help(Some(help.into()))
        })
        .collect()
}

fn suppress_switch_branch_completion() -> bool {
    CONTEXT.with(|ctx| {
        ctx.borrow()
            .as_ref()
            .is_some_and(|ctx| ctx.contains("--create") || ctx.contains("-c"))
    })
}

struct CompletionContext {
    args: Vec<OsString>,
}

impl CompletionContext {
    fn contains(&self, needle: &str) -> bool {
        self.args
            .iter()
            .any(|arg| arg.to_string_lossy().as_ref() == needle)
    }
}

// Thread-local context tracking is required because clap's ValueCompleter::complete()
// receives only the current argument being completed, not the full command line.
// We need access to all arguments to detect `--create` / `-c` flags and suppress
// branch completion when creating a new worktree (since the branch doesn't exist yet).
thread_local! {
    static CONTEXT: RefCell<Option<CompletionContext>> = const { RefCell::new(None) };
}

fn completion_command() -> Command {
    let cmd = cli::build_command();
    let cmd = adjust_completion_command(cmd);
    hide_non_positional_options_for_completion(cmd)
}

/// Hide non-positional options so they're filtered out when positional/subcommand
/// completions exist, but still shown when completing `--<TAB>`.
///
/// This exploits clap_complete's behavior: if any non-hidden candidates exist,
/// hidden ones are dropped. When all candidates are hidden, they're kept.
fn hide_non_positional_options_for_completion(cmd: Command) -> Command {
    // Disable built-in help/version flags for completion only
    let cmd = cmd
        .disable_help_flag(true)
        .disable_help_subcommand(true)
        .disable_version_flag(true);

    fn recurse(cmd: Command) -> Command {
        // Hide every non-positional arg on this Command
        let cmd = cmd.mut_args(|arg| {
            if arg.is_positional() {
                arg
            } else {
                arg.hide(true)
            }
        });

        // Recurse into subcommands
        cmd.mut_subcommands(recurse)
    }

    recurse(cmd)
}

// Mark positional args as `.last(true)` to allow them after all flags.
// This enables flexible argument ordering like:
// - `wt switch --create --execute=cmd --base=main feature` instead of `wt switch feature --create --execute=cmd --base=main`
// - `wt merge --no-squash main` instead of `wt merge main --no-squash`
// - `wt remove --no-delete-branch feature` instead of `wt remove feature --no-delete-branch`
fn adjust_completion_command(cmd: Command) -> Command {
    cmd.mut_subcommand("switch", |switch| {
        switch.mut_arg("branch", |arg| arg.last(true))
    })
    .mut_subcommand("remove", |remove| {
        remove.mut_arg("worktrees", |arg| arg.last(true))
    })
    .mut_subcommand("merge", |merge| {
        merge.mut_arg("target", |arg| arg.last(true))
    })
    .mut_subcommand("step", |step| {
        step.mut_subcommand("push", |push| push.mut_arg("target", |arg| arg.last(true)))
            .mut_subcommand("squash", |squash| {
                squash.mut_arg("target", |arg| arg.last(true))
            })
            .mut_subcommand("rebase", |rebase| {
                rebase.mut_arg("target", |arg| arg.last(true))
            })
    })
}
