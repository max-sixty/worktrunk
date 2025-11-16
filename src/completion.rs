use std::cell::RefCell;
use std::ffi::{OsStr, OsString};

use clap::{Command, CommandFactory};
use clap_complete::engine::{ArgValueCompleter, CompletionCandidate, ValueCompleter};
use clap_complete::env::CompleteEnv;

use crate::cli::Cli;
use worktrunk::git::Repository;

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

    let branches = load_branches();
    if branches.is_empty() {
        return Vec::new();
    }

    branches.into_iter().map(CompletionCandidate::new).collect()
}

fn load_branches() -> Vec<String> {
    Repository::current()
        .all_branches()
        .unwrap_or_else(|_| Vec::new())
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
    adjust_completion_command(Cli::command())
}

// Mark positional target args as `.last(true)` to allow them after all flags.
// This enables flexible argument ordering: `wt merge --no-squash main` instead of
// requiring the more restrictive `wt merge main --no-squash`.
fn adjust_completion_command(cmd: Command) -> Command {
    cmd.mut_subcommand("merge", |merge| {
        merge.mut_arg("target", |arg| arg.last(true))
    })
    .mut_subcommand("beta", |beta| {
        beta.mut_subcommand("push", |push| push.mut_arg("target", |arg| arg.last(true)))
            .mut_subcommand("squash", |squash| {
                squash.mut_arg("target", |arg| arg.last(true))
            })
            .mut_subcommand("rebase", |rebase| {
                rebase.mut_arg("target", |arg| arg.last(true))
            })
    })
}
