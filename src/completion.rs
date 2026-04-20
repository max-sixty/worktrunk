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

    // Tab-completion output lands above the user's prompt — any stray stderr
    // warnings would display there. Silence config deprecation/unknown-field
    // warnings for the duration of this process.
    worktrunk::config::suppress_warnings();

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

    // If the subcommand word matches a custom-subcommand binary, forward the
    // completion request to it (e.g., `wt sync --<tab>` → `wt-sync --<tab>`).
    // args[0] is the binary name ("wt"), args[1] is the subcommand ("sync").
    if args.len() >= 3 {
        let subcommand = args[1].to_string_lossy();
        // Only forward to the custom binary if no built-in subcommand has this
        // name. Built-ins always take precedence at runtime, so completions
        // must agree.
        let binary = format!("wt-{subcommand}");
        if cli::build_command().find_subcommand(&*subcommand).is_none()
            && which::which(&binary).is_ok()
        {
            // Forward args[1..] to the custom binary
            if let Some(forwarded) = forward_completion_to_custom(&binary, &args[1..], &shell_name)
            {
                let _ = std::io::stdout().write_all(forwarded.as_bytes());
                CONTEXT.with(|ctx| ctx.borrow_mut().take());
                return true;
            }
        }
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

/// Alias name completion for `wt config alias <show|dry-run> <name>`.
///
/// Completes with the merged user + project alias name set. Returns all
/// candidates unfiltered — the outer `maybe_handle_env_completion` does
/// bash-specific prefix filtering; fish/zsh apply their own matching.
pub(crate) fn alias_name_completer() -> ArgValueCompleter {
    ArgValueCompleter::new(AliasNameCompleter)
}

#[derive(Clone, Copy)]
struct AliasNameCompleter;

impl ValueCompleter for AliasNameCompleter {
    fn complete(&self, current: &OsStr) -> Vec<CompletionCandidate> {
        if current.to_str().is_some_and(|s| s.starts_with('-')) {
            return Vec::new();
        }
        load_aliases_for_completion()
            .into_keys()
            .map(CompletionCandidate::new)
            .collect()
    }
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
            && let Some(config) = user_config.hooks.get(hook_type)
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
    let cmd = inject_hook_subcommands(cmd);
    let cmd = inject_custom_subcommands(cmd);
    hide_non_positional_options_for_completion(cmd)
}

/// Inject hook type names as subcommands of `hook` so both completion and
/// `wt hook --help` list them. Clap doesn't dispatch these at runtime — the
/// real parser uses the `external_subcommand` `Run(Vec<OsString>)` variant —
/// but the help renderer and completion engine both walk the `Command` tree
/// to produce their output, so injecting stubs into an augmented clone shows
/// the types without affecting argument dispatch.
///
/// Mirrors `inject_alias_subcommands` which does the equivalent for
/// top-level alias names.
pub(crate) fn inject_hook_subcommands(cmd: Command) -> Command {
    cmd.mut_subcommand("hook", |mut hook| {
        for &name in cli::HOOK_TYPE_NAMES {
            // Skip if a real subcommand already uses this name (won't happen
            // today, but keeps the injection idempotent).
            if hook.get_subcommands().any(|s| s.get_name() == name) {
                continue;
            }
            hook = hook.subcommand(build_hook_completion_command(name));
        }
        hook
    })
}

/// Build a completion stub `clap::Command` for a hook type. Same shape as
/// `build_alias_completion_command` — declares the known flags (so they show
/// up in `wt hook pre-merge --<Tab>` completions) and wires the name completer
/// for the first positional (hook command name filter).
fn build_hook_completion_command(name: &'static str) -> Command {
    let about: &'static str = Box::leak(format!("Run {name} hooks").into_boxed_str());
    Command::new(name)
        .about(about)
        .arg(
            clap::Arg::new("dry-run")
                .long("dry-run")
                .action(clap::ArgAction::SetTrue)
                .help("Show what would run without executing"),
        )
        .arg(
            clap::Arg::new("foreground")
                .long("foreground")
                .action(clap::ArgAction::SetTrue)
                .help("Run in foreground (block until complete)"),
        )
        .arg(
            clap::Arg::new("yes")
                .short('y')
                .long("yes")
                .action(clap::ArgAction::SetTrue)
                .help("Skip approval prompts for project hooks"),
        )
        .arg(
            clap::Arg::new("var")
                .long("var")
                .value_name("KEY=VALUE")
                .num_args(1)
                .action(clap::ArgAction::Append)
                .help("Set template variable (deprecated — prefer --KEY=VALUE)"),
        )
        .arg(
            clap::Arg::new("name")
                .num_args(0..)
                .add(hook_command_name_completer())
                .help("Filter by command name(s)"),
        )
}

/// Inject configured aliases as subcommands at both the top level and under
/// `step` so they appear in completions for `wt <Tab>` and `wt step <Tab>`.
///
/// Aliases are loaded from user config and project config (same merge order as
/// `step_alias`). Aliases that shadow a built-in at a given level are skipped
/// for that level only — `commit` is shadowed under `step` but offered at the
/// top level, since `wt commit` runs the alias.
fn inject_alias_subcommands(cmd: Command) -> Command {
    let aliases = load_aliases_for_completion();
    if aliases.is_empty() {
        return cmd;
    }

    let mut cmd = cmd;
    // Top-level injection: skip aliases that match a top-level built-in.
    for (name, cmd_config) in &aliases {
        if cmd.get_subcommands().any(|s| s.get_name() == name.as_str()) {
            continue;
        }
        cmd = cmd.subcommand(build_alias_completion_command(name, cmd_config));
    }
    // Step-level injection: keep historical `wt step <alias>` completions.
    cmd.mut_subcommand("step", |mut step| {
        for (name, cmd_config) in aliases {
            if step
                .get_subcommands()
                .any(|s| s.get_name() == name.as_str())
            {
                continue;
            }
            step = step.subcommand(build_alias_completion_command(&name, &cmd_config));
        }
        step
    })
}

/// Build a completion stub `clap::Command` for an alias. Leaks strings since
/// completion is a short-lived subprocess that exits after printing candidates.
fn build_alias_completion_command(name: &str, cmd_config: &CommandConfig) -> Command {
    // Use the first command's template for the help text
    let first_template = cmd_config
        .commands()
        .next()
        .map(|c| c.template.as_str())
        .unwrap_or("");
    let help = truncate_template(first_template);
    let name: &'static str = Box::leak(name.to_string().into_boxed_str());
    let about: &'static str = Box::leak(format!("alias: {help}").into_boxed_str());
    Command::new(name)
        .about(about)
        .arg(clap::Arg::new("dry-run").long("dry-run"))
        .arg(clap::Arg::new("yes").short('y').long("yes"))
        .arg(
            clap::Arg::new("var")
                .long("var")
                .num_args(1)
                .action(clap::ArgAction::Append),
        )
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
        if let Ok(Some(project_config)) = ProjectConfig::load(&repo, false) {
            append_aliases(&mut aliases, &project_config.aliases);
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

/// Forward a completion request to a custom `wt-*` binary.
///
/// Rebuilds the args as if the user invoked `wt-sync <rest>` directly, passing
/// the `COMPLETE` env var so the custom binary generates completions.
fn forward_completion_to_custom(binary: &str, args: &[OsString], shell: &OsStr) -> Option<String> {
    // Build args for the custom binary: [binary_name, rest_args...]
    let mut child_args: Vec<OsString> = vec![OsString::from(binary)];
    child_args.extend_from_slice(&args[1..]);

    // Adjust the completion index: subtract 1 since we removed the subcommand name
    let index = std::env::var("_CLAP_COMPLETE_INDEX")
        .ok()
        .and_then(|i| i.parse::<usize>().ok())
        .map(|i| i.saturating_sub(1));

    let mut cmd = std::process::Command::new(binary);
    cmd.arg("--");
    cmd.args(&child_args);
    cmd.env("COMPLETE", shell);
    cmd.env(
        "_CLAP_IFS",
        std::env::var("_CLAP_IFS").unwrap_or_else(|_| "\n".to_string()),
    );
    if let Some(idx) = index {
        cmd.env("_CLAP_COMPLETE_INDEX", idx.to_string());
    }

    // Capture stdout from the custom binary. Using std::process::Command
    // directly (not shell_exec::Cmd) because this runs during completion —
    // a short-lived subprocess where logging/tracing is unwanted.
    let result = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?
        .wait_with_output()
        .ok()?;
    if result.status.success() {
        String::from_utf8(result.stdout).ok()
    } else {
        None
    }
}

/// Discover `wt-*` executables on PATH and inject them as subcommands for completion.
///
/// This mirrors git's approach: `wt sync` dispatches to `wt-sync`, and completions
/// should show `sync` as a subcommand. Custom subcommands that shadow built-in
/// commands are skipped (built-ins always take precedence at runtime).
fn inject_custom_subcommands(cmd: Command) -> Command {
    inject_custom_subcommand_list(cmd, discover_custom_subcommands())
}

/// Add discovered custom subcommands to the completion command tree.
///
/// Each one gets a stub `Command` with `allow_external_subcommands(true)` so
/// any trailing args are accepted without error. Built-in subcommands are never shadowed.
fn inject_custom_subcommand_list(mut cmd: Command, customs: Vec<String>) -> Command {
    for name in customs {
        if cmd.find_subcommand(&name).is_some() {
            continue;
        }
        // Leak is fine: completion is a short-lived subprocess that exits after
        // printing candidates (same pattern as inject_alias_subcommands).
        let name: &'static str = Box::leak(name.into_boxed_str());
        let about: &'static str = Box::leak(format!("custom: wt-{name}").into_boxed_str());
        let sub = Command::new(name)
            .about(about)
            .allow_external_subcommands(true);
        cmd = cmd.subcommand(sub);
    }
    cmd
}

/// Find `wt-*` executables on PATH, returning their subcommand names (without the `wt-` prefix).
fn discover_custom_subcommands() -> Vec<String> {
    let Some(path_var) = std::env::var_os("PATH") else {
        return Vec::new();
    };
    discover_custom_subcommands_in(&path_var)
}

/// Find `wt-*` executables in the given PATH value, returning subcommand names
/// (without the `wt-` prefix). On Windows, executable extensions (.exe, .cmd, etc.)
/// are stripped so that `wt-sync.exe` produces the subcommand name `sync`.
fn discover_custom_subcommands_in(path_var: &OsStr) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();

    for dir in std::env::split_paths(path_var) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str() else {
                continue;
            };
            let Some(subcommand) = name.strip_prefix("wt-") else {
                continue;
            };
            // Strip executable extensions on Windows (.exe, .cmd, etc.)
            #[cfg(windows)]
            let subcommand = std::path::Path::new(subcommand)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(subcommand);

            if subcommand.is_empty() || !seen.insert(subcommand.to_string()) {
                continue;
            }

            // Verify it's executable (on Unix)
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = entry.metadata()
                    && meta.permissions().mode() & 0o111 == 0
                {
                    continue;
                }
            }
            result.push(subcommand.to_string());
        }
    }

    result.sort();
    result
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

    #[test]
    fn test_discover_empty_path() {
        let result = discover_custom_subcommands_in(OsStr::new(""));
        assert!(result.is_empty());
    }

    #[test]
    fn test_discover_nonexistent_dir() {
        let result =
            discover_custom_subcommands_in(OsStr::new("/nonexistent/path/xxxxxxxx_wt_test"));
        assert!(result.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn test_discover_finds_wt_executables() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();

        for name in ["wt-alpha", "wt-beta"] {
            let path = dir.path().join(name);
            std::fs::write(&path, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let result = discover_custom_subcommands_in(dir.path().as_os_str());
        assert_eq!(result, vec!["alpha", "beta"]);
    }

    #[cfg(unix)]
    #[test]
    fn test_discover_skips_non_executable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();

        let exec = dir.path().join("wt-exec");
        std::fs::write(&exec, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&exec, std::fs::Permissions::from_mode(0o755)).unwrap();

        let noexec = dir.path().join("wt-noexec");
        std::fs::write(&noexec, "data").unwrap();
        std::fs::set_permissions(&noexec, std::fs::Permissions::from_mode(0o644)).unwrap();

        let result = discover_custom_subcommands_in(dir.path().as_os_str());
        assert_eq!(result, vec!["exec"]);
    }

    #[cfg(unix)]
    #[test]
    fn test_discover_deduplicates_across_dirs() {
        use std::os::unix::fs::PermissionsExt;
        let dir1 = tempfile::tempdir().unwrap();
        let dir2 = tempfile::tempdir().unwrap();

        for dir in [dir1.path(), dir2.path()] {
            let path = dir.join("wt-dup");
            std::fs::write(&path, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let path = std::env::join_paths([dir1.path(), dir2.path()]).unwrap();
        let result = discover_custom_subcommands_in(&path);
        assert_eq!(result, vec!["dup"]);
    }

    #[cfg(unix)]
    #[test]
    fn test_discover_skips_bare_prefix_and_non_matching() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();

        // "wt-" with no suffix should be skipped
        let empty = dir.path().join("wt-");
        std::fs::write(&empty, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&empty, std::fs::Permissions::from_mode(0o755)).unwrap();

        // Non-matching name should be skipped
        let other = dir.path().join("other-tool");
        std::fs::write(&other, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o755)).unwrap();

        let result = discover_custom_subcommands_in(dir.path().as_os_str());
        assert!(result.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn test_discover_results_are_sorted() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();

        // Create in reverse alphabetical order
        for name in ["wt-zebra", "wt-apple", "wt-mango"] {
            let path = dir.path().join(name);
            std::fs::write(&path, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let result = discover_custom_subcommands_in(dir.path().as_os_str());
        assert_eq!(result, vec!["apple", "mango", "zebra"]);
    }

    #[test]
    fn test_inject_custom_adds_subcommands() {
        let cmd = Command::new("wt")
            .subcommand(Command::new("switch"))
            .subcommand(Command::new("list"));

        let cmd = inject_custom_subcommand_list(cmd, vec!["sync".into(), "deploy".into()]);

        assert!(cmd.find_subcommand("sync").is_some());
        assert!(cmd.find_subcommand("deploy").is_some());
        assert!(cmd.find_subcommand("switch").is_some());
        assert!(cmd.find_subcommand("list").is_some());
    }

    #[test]
    fn test_inject_custom_skips_builtins() {
        let cmd = Command::new("wt").subcommand(Command::new("switch").about("built-in switch"));

        let cmd = inject_custom_subcommand_list(cmd, vec!["switch".into(), "sync".into()]);

        // "switch" should still be the built-in, not the custom subcommand
        let switch = cmd.find_subcommand("switch").unwrap();
        assert_eq!(switch.get_about().unwrap().to_string(), "built-in switch");
        // "sync" should be added as a custom subcommand
        let sync = cmd.find_subcommand("sync").unwrap();
        assert!(sync.get_about().unwrap().to_string().contains("custom"));
    }

    #[test]
    fn test_inject_custom_empty_list() {
        let cmd = Command::new("wt").subcommand(Command::new("switch"));
        let cmd = inject_custom_subcommand_list(cmd, vec![]);
        assert_eq!(cmd.get_subcommands().count(), 1);
    }

    #[test]
    fn test_inject_custom_allows_trailing_args() {
        let cmd = Command::new("wt");
        let cmd = inject_custom_subcommand_list(cmd, vec!["sync".into()]);

        let sync = cmd.find_subcommand("sync").unwrap();
        assert!(sync.is_allow_external_subcommands_set());
    }

    #[test]
    fn test_forward_to_nonexistent_binary() {
        let result = forward_completion_to_custom(
            "/nonexistent/binary/xxxxxxxx_wt_test",
            &[OsString::from("test")],
            OsStr::new("bash"),
        );
        assert!(result.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn test_forward_to_custom_binary() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("wt-fake");
        std::fs::write(&script, "#!/bin/sh\nprintf '%s\\n%s' '--all' '--verbose'\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let result = forward_completion_to_custom(
            script.to_str().unwrap(),
            &[OsString::from("fake"), OsString::from("--")],
            OsStr::new("bash"),
        );
        assert!(result.is_some());
        let output = result.unwrap();
        assert!(output.contains("--all"));
        assert!(output.contains("--verbose"));
    }

    #[cfg(unix)]
    #[test]
    fn test_forward_to_failing_binary() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("wt-fail");
        std::fs::write(&script, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let result = forward_completion_to_custom(
            script.to_str().unwrap(),
            &[OsString::from("fail")],
            OsStr::new("bash"),
        );
        assert!(result.is_none());
    }
}
