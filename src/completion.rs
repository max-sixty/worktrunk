use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io::Write;

use clap::Command;
use clap_complete::engine::{ArgValueCompleter, CompletionCandidate, ValueCompleter};
use clap_complete::env::CompleteEnv;

use crate::cli;
use crate::display::format_relative_time_short;
use worktrunk::config::{CommandConfig, ProjectConfig, UserConfig, append_aliases};
use worktrunk::git::{BranchCategory, HookType, Repository};

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
    // - Fish/Nushell: Append the current token as the last argument, so index = len - 1
    let index: usize = std::env::var("_CLAP_COMPLETE_INDEX")
        .ok()
        .and_then(|i| i.parse().ok())
        .unwrap_or_else(|| args.len() - 1);

    // Check if the current word is exactly "-" (single dash)
    // If so, we want to show both short flags (-h) AND long flags (--help)
    // clap only returns matches for the prefix, so we call complete twice
    let current_word = args.get(index).map(|s| s.to_string_lossy().into_owned());
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

    // Bash does not filter COMPREPLY by prefix — its programmable completion
    // (-F) passes the array as-is. Fish/zsh apply their own matching (substring,
    // fuzzy), so they receive all candidates. For bash, we must filter here.
    let shell_name = shell_name.to_string_lossy();
    let completions = if shell_name.as_ref() == "bash" {
        let prefix = current_word.as_deref().unwrap_or("").to_owned();
        if prefix.is_empty() {
            completions
        } else {
            completions
                .into_iter()
                .filter(|c| c.get_value().to_string_lossy().starts_with(&*prefix))
                .collect()
        }
    } else {
        completions
    };

    // Write completions in the appropriate format for the shell
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

/// Branch completion for positional arguments (switch).
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

/// Hook command name completion for `wt hook <hook-type> <name>`.
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

        // Return all candidates without prefix filtering — let the shell apply its
        // own matching (substring in fish, fuzzy in zsh, prefix in bash). The
        // bash-specific prefix filter in maybe_handle_env_completion() handles bash.

        // Get the hook type from the command line context
        let hook_type = CONTEXT.with(|ctx| {
            ctx.borrow().as_ref().and_then(|ctx| {
                for hook in &[
                    "pre-start",
                    "post-start",
                    "pre-commit",
                    "post-commit",
                    "pre-merge",
                    "post-merge",
                    "pre-remove",
                ] {
                    if ctx.contains(hook) {
                        return Some(*hook);
                    }
                }
                // Deprecated alias: post-create → pre-start
                if ctx.contains("post-create") {
                    return Some("pre-start");
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

        let add_named_commands =
            |candidates: &mut Vec<_>, config: &worktrunk::config::CommandConfig| {
                candidates.extend(
                    config
                        .commands()
                        .filter_map(|cmd| cmd.name.as_ref())
                        .map(|name| CompletionCandidate::new(name.clone())),
                );
            };

        // Load user config and add user hook names
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

        // Return all candidates without prefix filtering — let the shell apply its
        // own matching (substring in fish, fuzzy in zsh, prefix in bash). Pre-filtering
        // here prevents shells from using their native matching strategies.

        if self.suppress_with_create && suppress_switch_branch_completion() {
            return Vec::new();
        }

        let branches = match Repository::current().and_then(|repo| repo.branches_for_completion()) {
            Ok(b) => b,
            Err(_) => return Vec::new(),
        };

        if branches.is_empty() {
            return Vec::new();
        }

        // If remote-only branches aren't already excluded, drop them when the total
        // count is large. Shells like bash/zsh prompt "do you wish to see all N
        // possibilities?" which makes completion unusable in repos with many remotes.
        // Threshold of 100 aligns with bash's default `completion-query-items`.
        let exclude_remote_only = self.exclude_remote_only
            || (!self.worktree_only
                && branches.len() > 100
                && branches
                    .iter()
                    .any(|b| matches!(b.category, BranchCategory::Remote(_))));

        branches
            .into_iter()
            .filter(|branch| {
                if self.worktree_only {
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
                    BranchCategory::Remote(remotes) => {
                        format!("⇣ {} {}", time_str, remotes.join(", "))
                    }
                };
                CompletionCandidate::new(branch.name).help(Some(help.into()))
            })
            .collect()
    }
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
    let cmd = inject_alias_subcommands(cmd);
    hide_non_positional_options_for_completion(cmd)
}

/// Inject configured aliases as subcommands of `step` so they appear in completions.
///
/// Aliases are loaded from user config and project config (same merge order as
/// `step_alias`). Aliases that shadow built-in step commands are skipped.
fn inject_alias_subcommands(cmd: Command) -> Command {
    let aliases = load_aliases_for_completion();
    if aliases.is_empty() {
        return cmd;
    }

    cmd.mut_subcommand("step", |mut step| {
        for (name, cmd_config) in aliases {
            // Skip aliases that shadow built-in step commands
            if step
                .get_subcommands()
                .any(|s| s.get_name() == name.as_str())
            {
                continue;
            }
            // Use the first command's template for the help text
            let first_template = cmd_config
                .commands()
                .next()
                .map(|c| c.template.as_str())
                .unwrap_or("");
            let help = truncate_template(first_template);
            // clap::Command::new() requires Into<Str>, and Str only implements
            // From<&'static str> (not From<String>). Leak is fine: completion is
            // a short-lived subprocess that exits after printing candidates.
            let name: &'static str = Box::leak(name.into_boxed_str());
            let about: &'static str = Box::leak(format!("alias: {help}").into_boxed_str());
            let sub = Command::new(name)
                .about(about)
                .arg(clap::Arg::new("dry-run").long("dry-run"))
                .arg(clap::Arg::new("yes").short('y').long("yes"))
                .arg(
                    clap::Arg::new("var")
                        .long("var")
                        .num_args(1)
                        .action(clap::ArgAction::Append),
                );
            step = step.subcommand(sub);
        }
        step
    })
}

/// Load aliases from user and project config for completion.
///
/// Merges user and project aliases with append semantics (matching hooks).
fn load_aliases_for_completion() -> BTreeMap<String, CommandConfig> {
    let mut aliases = BTreeMap::new();

    if let Ok(repo) = Repository::current() {
        // User config first
        if let Ok(user_config) = UserConfig::load() {
            let project_id = repo.project_identifier().ok();
            aliases.extend(user_config.aliases(project_id.as_deref()));
        }
        // Project config appends
        if let Ok(Some(project_config)) = ProjectConfig::load(&repo, false)
            && let Some(ref project_aliases) = project_config.aliases
        {
            append_aliases(&mut aliases, project_aliases);
        }
    } else if let Ok(user_config) = UserConfig::load() {
        aliases.extend(user_config.aliases(None));
    }

    aliases
}

/// Truncate a template string for use as completion help text.
fn truncate_template(template: &str) -> &str {
    let s = template.trim();
    let first_line = s.lines().next().unwrap_or(s);
    if first_line.len() > 60 {
        // Find the last char boundary at or before byte 57
        let mut end = 57;
        while end > 0 && !first_line.is_char_boundary(end) {
            end -= 1;
        }
        &first_line[..end]
    } else {
        first_line
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_template() {
        // Short template — returned as-is
        assert_eq!(truncate_template("echo hello"), "echo hello");

        // Multiline — only first line
        assert_eq!(truncate_template("line one\nline two"), "line one");

        // Leading/trailing whitespace trimmed
        assert_eq!(truncate_template("  spaced  \n"), "spaced");

        // Exactly 60 chars — no truncation
        let s60 = "a".repeat(60);
        assert_eq!(truncate_template(&s60), s60.as_str());

        // 61 chars — truncated to 57
        let s61 = "b".repeat(61);
        assert_eq!(truncate_template(&s61), &"b".repeat(57));

        // Multi-byte chars where byte 57 falls mid-character.
        // 'a' (1 byte) × 56 + '€' (3 bytes) × 2 = 62 bytes, > 60.
        // Byte 57 is the second byte of the first '€', so the loop backs up to 56.
        let multi = "a".repeat(56) + "€€";
        let result = truncate_template(&multi);
        assert_eq!(result.len(), 56);
        assert_eq!(result, "a".repeat(56));
    }
}
