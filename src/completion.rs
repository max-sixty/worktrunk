use std::cell::RefCell;
use std::ffi::{OsStr, OsString};
use std::io::Write;

use clap::Command;
use clap_complete::engine::{ArgValueCompleter, CompletionCandidate, ValueCompleter};
use clap_complete::env::CompleteEnv;

use crate::cli;
use crate::display::format_relative_time_short;
use worktrunk::config::{ProjectConfig, UserConfig};
use worktrunk::git::{BranchCategory, HookType, Repository};

/// Deprecated args that should never appear in completions.
/// These are hidden from help AND completions, unlike other hidden args
/// that appear when completing `--`.
const DEPRECATED_ARGS: &[&str] = &["--no-background"];

/// Handle shell-initiated completion requests via `COMPLETE=$SHELL wt`
pub(crate) fn maybe_handle_env_completion() -> bool {
    let Some(shell_name) = std::env::var_os("COMPLETE") else {
        return false;
    };

    if shell_name.is_empty() || shell_name == "0" {
        return false;
    }

    let mut args: Vec<OsString> = std::env::args_os().collect();
    CONTEXT.with(|ctx| *ctx.borrow_mut() = Some(CompletionContext { args: args.clone() }));

    // Remove the binary name and find the `--` separator
    args.remove(0);
    let escape_index = args
        .iter()
        .position(|a| *a == "--")
        .map(|i| i + 1)
        .unwrap_or(args.len());
    args.drain(0..escape_index);

    let current_dir = std::env::current_dir().ok();

    // If no args after `--`, output the shell registration script
    if args.is_empty() {
        // Use CompleteEnv for registration script generation
        let all_args: Vec<OsString> = std::env::args_os().collect();
        let _ = CompleteEnv::with_factory(completion_command)
            .try_complete(all_args, current_dir.as_deref());
        CONTEXT.with(|ctx| ctx.borrow_mut().take());
        return true;
    }

    // Generate completions with filtering
    let mut cmd = completion_command();
    cmd.build();

    // Determine the index of the word being completed.
    // - Bash/Zsh: Pass `_CLAP_COMPLETE_INDEX` env var with the cursor position
    // - Fish: Appends the current token as the last argument, so index = len - 1
    let index: usize = std::env::var("_CLAP_COMPLETE_INDEX")
        .ok()
        .and_then(|i| i.parse().ok())
        .unwrap_or_else(|| args.len() - 1);

    // Check if the current word is exactly "-" (single dash)
    // If so, we want to show both short flags (-h) AND long flags (--help)
    // clap only returns matches for the prefix, so we call complete twice
    let current_word = args.get(index).map(|s| s.to_string_lossy());
    let include_long_flags = current_word.as_deref() == Some("-");

    let completions = match clap_complete::engine::complete(
        &mut cmd,
        args.clone(),
        index,
        current_dir.as_deref(),
    ) {
        Ok(c) => c,
        Err(_) => {
            CONTEXT.with(|ctx| ctx.borrow_mut().take());
            return true;
        }
    };

    // If single dash, also get completions for "--" and merge
    let completions = if include_long_flags {
        let mut merged = completions;
        let mut args_with_double_dash = args;
        if let Some(word) = args_with_double_dash.get_mut(index) {
            *word = OsString::from("--");
        }
        let mut cmd2 = completion_command();
        cmd2.build();
        if let Ok(long_completions) = clap_complete::engine::complete(
            &mut cmd2,
            args_with_double_dash,
            index,
            current_dir.as_deref(),
        ) {
            // Add long flags that aren't already present (avoid duplicates)
            for candidate in long_completions {
                let value = candidate.get_value();
                if !merged.iter().any(|c| c.get_value() == value) {
                    merged.push(candidate);
                }
            }
        }
        merged
    } else {
        completions
    };

    // Filter out deprecated args - they should never appear in completions
    let completions: Vec<_> = completions
        .into_iter()
        .filter(|c| {
            let value = c.get_value().to_string_lossy();
            !DEPRECATED_ARGS.contains(&value.as_ref())
        })
        .collect();

    // Write completions in the appropriate format for the shell
    let shell_name = shell_name.to_string_lossy();
    let ifs = std::env::var("_CLAP_IFS").ok();
    let separator = ifs.as_deref().unwrap_or("\n");

    // Shell-specific separator between value and description
    // zsh uses ":", fish/nushell use "\t", bash doesn't support descriptions
    let help_sep = match shell_name.as_ref() {
        "zsh" => Some(":"),
        "fish" | "nu" => Some("\t"),
        _ => None,
    };

    let mut stdout = std::io::stdout();
    for (i, candidate) in completions.iter().enumerate() {
        if i != 0 {
            let _ = write!(stdout, "{}", separator);
        }
        let value = candidate.get_value().to_string_lossy();
        match (help_sep, candidate.get_help()) {
            (Some(sep), Some(help)) => {
                let _ = write!(stdout, "{}{}{}", value, sep, help);
            }
            _ => {
                let _ = write!(stdout, "{}", value);
            }
        }
    }

    CONTEXT.with(|ctx| ctx.borrow_mut().take());
    true
}

/// Branch completion without additional context filtering (e.g., --base, merge target).
pub(crate) fn branch_value_completer() -> ArgValueCompleter {
    ArgValueCompleter::new(BranchCompleter {
        suppress_with_create: false,
        exclude_remote_only: false,
        worktree_only: false,
    })
}

/// Branch completion for positional arguments (switch, select).
/// Suppresses completions when --create flag is present.
pub(crate) fn worktree_branch_completer() -> ArgValueCompleter {
    ArgValueCompleter::new(BranchCompleter {
        suppress_with_create: true,
        exclude_remote_only: false,
        worktree_only: false,
    })
}

/// Branch completion for remove command - excludes remote-only branches.
pub(crate) fn local_branches_completer() -> ArgValueCompleter {
    ArgValueCompleter::new(BranchCompleter {
        suppress_with_create: false,
        exclude_remote_only: true,
        worktree_only: false,
    })
}

/// Branch completion for commands that only operate on worktrees (e.g., copy-ignored).
pub(crate) fn worktree_only_completer() -> ArgValueCompleter {
    ArgValueCompleter::new(BranchCompleter {
        suppress_with_create: false,
        exclude_remote_only: false,
        worktree_only: true,
    })
}

/// Hook command name completion for `wt step <hook-type> <name>`.
/// Completes with command names from the project config for the hook type being invoked.
pub(crate) fn hook_command_name_completer() -> ArgValueCompleter {
    ArgValueCompleter::new(HookCommandCompleter)
}

#[derive(Clone, Copy)]
struct HookCommandCompleter;

impl ValueCompleter for HookCommandCompleter {
    fn complete(&self, current: &OsStr) -> Vec<CompletionCandidate> {
        // If user is typing an option (starts with -), don't suggest command names
        if current.to_str().is_some_and(|s| s.starts_with('-')) {
            return Vec::new();
        }

        let prefix = current.to_string_lossy();
        complete_hook_commands()
            .into_iter()
            .filter(|candidate| {
                candidate
                    .get_value()
                    .to_string_lossy()
                    .starts_with(&*prefix)
            })
            .collect()
    }
}

fn complete_hook_commands() -> Vec<CompletionCandidate> {
    // Get the hook type from the command line context
    let hook_type = CONTEXT.with(|ctx| {
        ctx.borrow().as_ref().and_then(|ctx| {
            // Look for the hook subcommand in the args
            for hook in &[
                "post-create",
                "post-start",
                "pre-commit",
                "pre-merge",
                "post-merge",
                "pre-remove",
            ] {
                if ctx.contains(hook) {
                    return Some(*hook);
                }
            }
            None
        })
    });

    let Some(hook_type_str) = hook_type else {
        return Vec::new();
    };
    let Ok(hook_type) = hook_type_str.parse::<HookType>() else {
        return Vec::new();
    };

    let mut candidates = Vec::new();

    // Helper to extract named commands from a hook config
    let add_named_commands =
        |candidates: &mut Vec<_>, config: &worktrunk::config::CommandConfig| {
            candidates.extend(
                config
                    .commands()
                    .iter()
                    .filter_map(|cmd| cmd.name.as_ref())
                    .map(|name| CompletionCandidate::new(name.clone())),
            );
        };

    // Load user config and add user hook names
    // Uses overrides.hooks for completion (global hooks from user config file)
    if let Ok(user_config) = UserConfig::load()
        && let Some(config) = user_config.configs.hooks.get(hook_type)
    {
        add_named_commands(&mut candidates, config);
    }

    // Load project config and add project hook names
    // Pass write_hints=false to avoid side effects during completion
    if let Ok(repo) = Repository::current()
        && let Ok(Some(project_config)) = ProjectConfig::load(&repo, false)
        && let Some(config) = project_config.hooks.get(hook_type)
    {
        add_named_commands(&mut candidates, config);
    }

    candidates
}

#[derive(Clone, Copy)]
struct BranchCompleter {
    suppress_with_create: bool,
    exclude_remote_only: bool,
    worktree_only: bool,
}

impl ValueCompleter for BranchCompleter {
    fn complete(&self, current: &OsStr) -> Vec<CompletionCandidate> {
        // If user is typing an option (starts with -), don't suggest branches
        if current.to_str().is_some_and(|s| s.starts_with('-')) {
            return Vec::new();
        }

        // Filter branches by prefix - clap doesn't filter ArgValueCompleter results
        let prefix = current.to_string_lossy();
        complete_branches(
            self.suppress_with_create,
            self.exclude_remote_only,
            self.worktree_only,
        )
        .into_iter()
        .filter(|candidate| {
            candidate
                .get_value()
                .to_string_lossy()
                .starts_with(&*prefix)
        })
        .collect()
    }
}

fn complete_branches(
    suppress_with_create: bool,
    exclude_remote_only: bool,
    worktree_only: bool,
) -> Vec<CompletionCandidate> {
    if suppress_with_create && suppress_switch_branch_completion() {
        return Vec::new();
    }

    let branches = match Repository::current().and_then(|repo| repo.branches_for_completion()) {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };

    if branches.is_empty() {
        return Vec::new();
    }

    branches
        .into_iter()
        .filter(|branch| {
            if worktree_only {
                matches!(branch.category, BranchCategory::Worktree)
            } else if exclude_remote_only {
                !matches!(branch.category, BranchCategory::Remote(_))
            } else {
                true
            }
        })
        .map(|branch| {
            let time_str = format_relative_time_short(branch.timestamp);
            let help = match branch.category {
                BranchCategory::Worktree => format!("+ {}", time_str),
                BranchCategory::Local => format!("/ {}", time_str),
                BranchCategory::Remote(remotes) => format!("⇣ {} {}", time_str, remotes.join(", ")),
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
    hide_non_positional_options_for_completion(cmd)
}

/// Hide non-positional options so they're filtered out when positional/subcommand
/// completions exist, but still shown when completing `--<TAB>`.
///
/// This exploits clap_complete's behavior: if any non-hidden candidates exist,
/// hidden ones are dropped. When all candidates are hidden, they're kept.
fn hide_non_positional_options_for_completion(cmd: Command) -> Command {
    fn process_command(cmd: Command, is_root: bool) -> Command {
        // Disable built-in help flag (not visible to mut_args) and add custom replacement
        let cmd = cmd.disable_help_flag(true).arg(
            clap::Arg::new("help")
                .short('h')
                .long("help")
                .action(clap::ArgAction::Help)
                .help("Print help (see more with '--help')"),
        );

        // Only root command has --version
        let cmd = if is_root {
            cmd.disable_version_flag(true).arg(
                clap::Arg::new("version")
                    .short('V')
                    .long("version")
                    .action(clap::ArgAction::Version)
                    .help("Print version"),
            )
        } else {
            cmd
        };

        // Hide non-positional args that aren't already hidden.
        // Args originally marked hide=true stay hidden always.
        // Args we hide here will appear when completing `--` (all-hidden = all shown).
        let cmd = cmd.mut_args(|arg| {
            if arg.is_positional() || arg.is_hide_set() {
                arg
            } else {
                arg.hide(true)
            }
        });

        cmd.mut_subcommands(|sub| process_command(sub, false))
    }

    process_command(cmd, true)
}
