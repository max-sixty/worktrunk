use std::ffi::{OsStr, OsString};

use clap::Subcommand;

use super::config::ApprovalsCommand;

/// Hook subcommands that accept `--var KEY=VALUE` (and the `--KEY=VALUE` shorthand).
///
/// Excludes `show`, `run-pipeline`, and the deprecated `approvals` (now under
/// `wt config approvals`), which don't expand template variables. `post-create`
/// is the deprecated alias for `pre-start`.
const HOOK_SUBCOMMANDS_WITH_VARS: &[&str] = &[
    "pre-switch",
    "post-switch",
    "pre-start",
    "post-start",
    "post-create",
    "pre-commit",
    "post-commit",
    "pre-merge",
    "post-merge",
    "pre-remove",
    "post-remove",
];

/// Long flags on hook subcommands that must never be rewritten as template
/// variables. Includes hook-specific flags and global flags (`-C` is a short
/// flag so it's excluded; the `--` prefix check handles that automatically).
const KNOWN_HOOK_LONG_FLAGS: &[&str] = &[
    "--yes",
    "--dry-run",
    "--foreground",
    "--var",
    "--help",
    "--config",
    "--verbose",
];

/// Rewrite `wt hook <type> --KEY=VALUE` into `wt hook <type> --var KEY=VALUE`
/// for unknown `--key=value` flags, so hook invocations can use the same
/// `--KEY=VALUE` shorthand that `wt <alias>` supports. (Aliases route via
/// template references; hooks have no such discriminator, so the rewrite
/// here is an unconditional "unknown `--KEY=VALUE` means var".)
///
/// Only args after a `hook <type>` prefix are touched, and only when `<type>`
/// is a hook that accepts `--var`.
///
/// Use the long `--var KEY=VALUE` form when a variable name collides with a
/// built-in flag like `--config` or `--yes`.
pub(crate) fn rewrite_var_shorthand(args: Vec<OsString>) -> Vec<OsString> {
    // Locate `hook` in argv, skipping the program name and guarding against
    // `hook` appearing as the value of `-C` or `--config`.
    let Some(hook_idx) = args.iter().enumerate().find_map(|(i, arg)| {
        if i == 0 || arg.as_os_str() != OsStr::new("hook") {
            return None;
        }
        let prev = args[i - 1].as_os_str();
        if prev == OsStr::new("-C") || prev == OsStr::new("--config") {
            return None;
        }
        Some(i)
    }) else {
        return args;
    };

    let Some(sub_str) = args.get(hook_idx + 1).and_then(|s| s.to_str()) else {
        return args;
    };
    if !HOOK_SUBCOMMANDS_WITH_VARS.contains(&sub_str) {
        return args;
    }

    let rewrite_start = hook_idx + 2;
    let mut out: Vec<OsString> = Vec::with_capacity(args.len());
    out.extend(args[..rewrite_start].iter().cloned());
    let mut past_double_dash = false;
    for token in &args[rewrite_start..] {
        if token == "--" {
            past_double_dash = true;
            out.push(token.clone());
            continue;
        }
        if !past_double_dash
            && let Some(s) = token.to_str()
            && let Some(rest) = s.strip_prefix("--")
            && let Some((key, value)) = rest.split_once('=')
            && !key.is_empty()
        {
            let flag_name = format!("--{key}");
            if !KNOWN_HOOK_LONG_FLAGS.contains(&flag_name.as_str()) {
                out.push(OsString::from("--var"));
                out.push(OsString::from(format!("{key}={value}")));
                continue;
            }
        }
        out.push(token.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewrite(args: &[&str]) -> Vec<String> {
        rewrite_var_shorthand(args.iter().map(OsString::from).collect())
            .into_iter()
            .map(|s| s.into_string().unwrap())
            .collect()
    }

    #[test]
    fn test_rewrite_var_shorthand() {
        // Shorthand gets rewritten
        assert_eq!(
            rewrite(&["wt", "hook", "pre-start", "--branch=feature/test"]),
            vec!["wt", "hook", "pre-start", "--var", "branch=feature/test"]
        );

        // Multiple shorthand args
        assert_eq!(
            rewrite(&[
                "wt",
                "hook",
                "post-merge",
                "--branch=main",
                "--target=develop"
            ]),
            vec![
                "wt",
                "hook",
                "post-merge",
                "--var",
                "branch=main",
                "--var",
                "target=develop"
            ]
        );

        // Shorthand mixed with known flags
        assert_eq!(
            rewrite(&[
                "wt",
                "hook",
                "pre-merge",
                "--yes",
                "--branch=feature",
                "--dry-run"
            ]),
            vec![
                "wt",
                "hook",
                "pre-merge",
                "--yes",
                "--var",
                "branch=feature",
                "--dry-run"
            ]
        );

        // Shorthand mixed with name filter positional
        assert_eq!(
            rewrite(&["wt", "hook", "pre-merge", "test", "--branch=feature"]),
            vec!["wt", "hook", "pre-merge", "test", "--var", "branch=feature"]
        );

        // Deprecated post-create alias still works
        assert_eq!(
            rewrite(&["wt", "hook", "post-create", "--branch=x"]),
            vec!["wt", "hook", "post-create", "--var", "branch=x"]
        );

        // Value containing equals sign
        assert_eq!(
            rewrite(&["wt", "hook", "pre-start", "--url=http://host?a=1"]),
            vec!["wt", "hook", "pre-start", "--var", "url=http://host?a=1"]
        );

        // Empty value
        assert_eq!(
            rewrite(&["wt", "hook", "pre-start", "--branch="]),
            vec!["wt", "hook", "pre-start", "--var", "branch="]
        );
    }

    #[test]
    fn test_rewrite_preserves_known_flags() {
        // --var=KEY=VAL is untouched (still handled by clap's parser)
        assert_eq!(
            rewrite(&["wt", "hook", "pre-start", "--var=branch=feature"]),
            vec!["wt", "hook", "pre-start", "--var=branch=feature"]
        );

        // --config=path is a global flag, not a variable shorthand
        assert_eq!(
            rewrite(&["wt", "hook", "pre-start", "--config=/tmp/config.toml"]),
            vec!["wt", "hook", "pre-start", "--config=/tmp/config.toml"]
        );

        // --dry-run, --yes, --foreground pass through
        assert_eq!(
            rewrite(&[
                "wt",
                "hook",
                "post-merge",
                "--yes",
                "--dry-run",
                "--foreground"
            ]),
            vec![
                "wt",
                "hook",
                "post-merge",
                "--yes",
                "--dry-run",
                "--foreground"
            ]
        );
    }

    #[test]
    fn test_rewrite_leaves_non_hook_subcommands_alone() {
        // `wt hook show` doesn't accept --var, so pass through unchanged
        assert_eq!(
            rewrite(&["wt", "hook", "show", "--expanded"]),
            vec!["wt", "hook", "show", "--expanded"]
        );

        // `wt hook approvals add --all` passes through unchanged
        assert_eq!(
            rewrite(&["wt", "hook", "approvals", "add", "--all"]),
            vec!["wt", "hook", "approvals", "add", "--all"]
        );

        // `wt switch --foo=bar` is not a hook command, pass through
        assert_eq!(
            rewrite(&["wt", "switch", "--foo=bar"]),
            vec!["wt", "switch", "--foo=bar"]
        );

        // `wt` alone
        assert_eq!(rewrite(&["wt"]), vec!["wt"]);
    }

    #[test]
    fn test_rewrite_skips_hook_in_flag_value_position() {
        // `wt -C hook pre-start` — here `hook` is a path, not the subcommand
        assert_eq!(
            rewrite(&["wt", "-C", "hook", "pre-start", "--branch=x"]),
            vec!["wt", "-C", "hook", "pre-start", "--branch=x"]
        );

        // `wt --config hook pre-start` — same guard
        assert_eq!(
            rewrite(&["wt", "--config", "hook", "pre-start", "--branch=x"]),
            vec!["wt", "--config", "hook", "pre-start", "--branch=x"]
        );
    }

    #[test]
    fn test_rewrite_handles_global_flags_before_hook() {
        // Global flags before `hook` shouldn't interfere
        assert_eq!(
            rewrite(&["wt", "-v", "hook", "pre-start", "--branch=x"]),
            vec!["wt", "-v", "hook", "pre-start", "--var", "branch=x"]
        );
    }

    #[test]
    fn test_rewrite_ignores_bare_hyphen_args() {
        // Bare `--`, single dash, and short flags pass through
        assert_eq!(
            rewrite(&["wt", "hook", "pre-start", "-y", "--branch=x"]),
            vec!["wt", "hook", "pre-start", "-y", "--var", "branch=x"]
        );

        // `--` with no key portion
        assert_eq!(
            rewrite(&["wt", "hook", "pre-start", "--=val"]),
            vec!["wt", "hook", "pre-start", "--=val"]
        );

        // `--` stops rewriting — args after it are positional
        assert_eq!(
            rewrite(&["wt", "hook", "pre-start", "--", "--branch=x"]),
            vec!["wt", "hook", "pre-start", "--", "--branch=x"]
        );
    }

    /// Verify HOOK_SUBCOMMANDS_WITH_VARS stays in sync with HookCommand variants
    /// that accept `--var`. If a hook subcommand is added/removed without updating
    /// the list, this test catches the drift.
    ///
    /// `post-create` is a deprecated alias for `pre-start` — it appears in the
    /// list (users can still type `wt hook post-create`) but not as a separate
    /// clap subcommand, so it's excluded from the reverse check.
    #[test]
    fn test_hook_subcommands_with_vars_matches_clap() {
        use crate::cli::Cli;
        use clap::CommandFactory;

        let app = Cli::command();
        let hook_cmd = app
            .get_subcommands()
            .find(|c| c.get_name() == "hook")
            .expect("hook subcommand exists");

        let subs_with_var: Vec<&str> = hook_cmd
            .get_subcommands()
            .filter(|c| c.get_arguments().any(|a| a.get_id() == "vars"))
            .map(|c| c.get_name())
            .collect();

        for name in &subs_with_var {
            assert!(
                HOOK_SUBCOMMANDS_WITH_VARS.contains(name),
                "Hook subcommand '{name}' accepts --var but is missing from \
                 HOOK_SUBCOMMANDS_WITH_VARS. Add it so --KEY=VALUE shorthand works."
            );
        }

        // Deprecated aliases live in the list but not as separate clap subcommands
        let deprecated_aliases: &[&str] = &["post-create"];
        for name in HOOK_SUBCOMMANDS_WITH_VARS {
            if deprecated_aliases.contains(name) {
                continue;
            }
            assert!(
                subs_with_var.contains(name),
                "HOOK_SUBCOMMANDS_WITH_VARS contains '{name}' but that subcommand \
                 doesn't accept --var. Remove it from the list."
            );
        }
    }

    /// Verify KNOWN_HOOK_LONG_FLAGS stays in sync with actual clap flags on
    /// hook subcommands. An unlisted flag would be silently rewritten to
    /// `--var`, which clap then rejects — but the error message would be
    /// confusing rather than helpful.
    #[test]
    fn test_known_hook_long_flags_matches_clap() {
        use crate::cli::Cli;
        use clap::CommandFactory;

        let app = Cli::command();
        let hook_cmd = app
            .get_subcommands()
            .find(|c| c.get_name() == "hook")
            .expect("hook subcommand exists");

        // Collect all long flags from hook subcommands that accept --var
        let mut clap_flags: std::collections::HashSet<String> = std::collections::HashSet::new();
        for sub in hook_cmd.get_subcommands() {
            if !sub.get_arguments().any(|a| a.get_id() == "vars") {
                continue;
            }
            for arg in sub.get_arguments() {
                if let Some(long) = arg.get_long() {
                    clap_flags.insert(format!("--{long}"));
                }
            }
        }
        // Also include global flags that appear after `hook <type>`
        for arg in app.get_arguments() {
            if let Some(long) = arg.get_long() {
                clap_flags.insert(format!("--{long}"));
            }
        }

        for flag in &clap_flags {
            assert!(
                KNOWN_HOOK_LONG_FLAGS.contains(&flag.as_str()),
                "Hook subcommand flag '{flag}' is missing from KNOWN_HOOK_LONG_FLAGS. \
                 Add it so --KEY=VALUE shorthand doesn't rewrite it."
            );
        }
    }
}

// Ordering: worktree lifecycle phases (switch → start → commit → merge →
// remove), with each phase's `pre-` immediately before its `post-`. `show`
// first (read-only introspection). Hidden commands last.
/// Run configured hooks
#[derive(Subcommand)]
pub enum HookCommand {
    /// Show configured hooks
    ///
    /// Lists user and project hooks. Project hooks show approval status (❓ = needs approval).
    Show {
        /// Hook type to show (default: all)
        #[arg(value_parser = ["pre-switch", "post-switch", "pre-start", "post-start", "pre-commit", "post-commit", "pre-merge", "post-merge", "pre-remove", "post-remove"])]
        hook_type: Option<String>,

        /// Show expanded commands with current variables
        #[arg(long)]
        expanded: bool,
    },

    /// Run pre-switch hooks
    ///
    /// Blocking — waits for completion before continuing.
    PreSwitch {
        /// Filter by command name(s)
        ///
        /// Supports `user:name` or `project:name` to filter by source.
        /// `user:` alone runs all user hooks; `project:` alone runs all project hooks.
        #[arg(add = crate::completion::hook_command_name_completer())]
        name: Vec<String>,

        /// Show what would run without executing
        #[arg(long)]
        dry_run: bool,

        /// Set template variable (KEY=VALUE)
        #[arg(long = "var", value_name = "KEY=VALUE", value_parser = super::parse_key_val, action = clap::ArgAction::Append)]
        vars: Vec<(String, String)>,
    },

    /// Run post-switch hooks
    ///
    /// Background by default. Use `--foreground` to run in foreground for debugging.
    PostSwitch {
        /// Filter by command name(s)
        ///
        /// Supports `user:name` or `project:name` to filter by source.
        /// `user:` alone runs all user hooks; `project:` alone runs all project hooks.
        #[arg(add = crate::completion::hook_command_name_completer())]
        name: Vec<String>,

        /// Show what would run without executing
        #[arg(long)]
        dry_run: bool,

        /// Run in foreground (block until complete)
        #[arg(long)]
        foreground: bool,

        /// Set template variable (KEY=VALUE)
        #[arg(long = "var", value_name = "KEY=VALUE", value_parser = super::parse_key_val, action = clap::ArgAction::Append)]
        vars: Vec<(String, String)>,
    },

    /// Run pre-start hooks
    ///
    /// Blocking — waits for completion before continuing.
    #[command(alias = "post-create")]
    PreStart {
        /// Filter by command name(s)
        ///
        /// Supports `user:name` or `project:name` to filter by source.
        /// `user:` alone runs all user hooks; `project:` alone runs all project hooks.
        #[arg(add = crate::completion::hook_command_name_completer())]
        name: Vec<String>,

        /// Show what would run without executing
        #[arg(long)]
        dry_run: bool,

        /// Set template variable (KEY=VALUE)
        #[arg(long = "var", value_name = "KEY=VALUE", value_parser = super::parse_key_val, action = clap::ArgAction::Append)]
        vars: Vec<(String, String)>,
    },

    /// Run post-start hooks
    ///
    /// Background by default. Use `--foreground` to run in foreground for debugging.
    PostStart {
        /// Filter by command name(s)
        ///
        /// Supports `user:name` or `project:name` to filter by source.
        /// `user:` alone runs all user hooks; `project:` alone runs all project hooks.
        #[arg(add = crate::completion::hook_command_name_completer())]
        name: Vec<String>,

        /// Show what would run without executing
        #[arg(long)]
        dry_run: bool,

        /// Run in foreground (block until complete)
        #[arg(long)]
        foreground: bool,

        /// Set template variable (KEY=VALUE)
        #[arg(long = "var", value_name = "KEY=VALUE", value_parser = super::parse_key_val, action = clap::ArgAction::Append)]
        vars: Vec<(String, String)>,
    },

    /// Run pre-commit hooks
    PreCommit {
        /// Filter by command name(s)
        ///
        /// Supports `user:name` or `project:name` to filter by source.
        /// `user:` alone runs all user hooks; `project:` alone runs all project hooks.
        #[arg(add = crate::completion::hook_command_name_completer())]
        name: Vec<String>,

        /// Show what would run without executing
        #[arg(long)]
        dry_run: bool,

        /// Set template variable (KEY=VALUE)
        #[arg(long = "var", value_name = "KEY=VALUE", value_parser = super::parse_key_val, action = clap::ArgAction::Append)]
        vars: Vec<(String, String)>,
    },

    /// Run post-commit hooks
    ///
    /// Background by default. Use `--foreground` to run in foreground for debugging.
    PostCommit {
        /// Filter by command name(s)
        ///
        /// Supports `user:name` or `project:name` to filter by source.
        /// `user:` alone runs all user hooks; `project:` alone runs all project hooks.
        #[arg(add = crate::completion::hook_command_name_completer())]
        name: Vec<String>,

        /// Show what would run without executing
        #[arg(long)]
        dry_run: bool,

        /// Run in foreground (block until complete)
        #[arg(long)]
        foreground: bool,

        /// Set template variable (KEY=VALUE)
        #[arg(long = "var", value_name = "KEY=VALUE", value_parser = super::parse_key_val, action = clap::ArgAction::Append)]
        vars: Vec<(String, String)>,
    },

    /// Run pre-merge hooks
    PreMerge {
        /// Filter by command name(s)
        ///
        /// Supports `user:name` or `project:name` to filter by source.
        /// `user:` alone runs all user hooks; `project:` alone runs all project hooks.
        #[arg(add = crate::completion::hook_command_name_completer())]
        name: Vec<String>,

        /// Show what would run without executing
        #[arg(long)]
        dry_run: bool,

        /// Set template variable (KEY=VALUE)
        #[arg(long = "var", value_name = "KEY=VALUE", value_parser = super::parse_key_val, action = clap::ArgAction::Append)]
        vars: Vec<(String, String)>,
    },

    /// Run post-merge hooks
    ///
    /// Background by default. Use `--foreground` to run in foreground for debugging.
    PostMerge {
        /// Filter by command name(s)
        ///
        /// Supports `user:name` or `project:name` to filter by source.
        /// `user:` alone runs all user hooks; `project:` alone runs all project hooks.
        #[arg(add = crate::completion::hook_command_name_completer())]
        name: Vec<String>,

        /// Show what would run without executing
        #[arg(long)]
        dry_run: bool,

        /// Run in foreground (block until complete)
        #[arg(long)]
        foreground: bool,

        /// Set template variable (KEY=VALUE)
        #[arg(long = "var", value_name = "KEY=VALUE", value_parser = super::parse_key_val, action = clap::ArgAction::Append)]
        vars: Vec<(String, String)>,
    },

    /// Run pre-remove hooks
    PreRemove {
        /// Filter by command name(s)
        ///
        /// Supports `user:name` or `project:name` to filter by source.
        /// `user:` alone runs all user hooks; `project:` alone runs all project hooks.
        #[arg(add = crate::completion::hook_command_name_completer())]
        name: Vec<String>,

        /// Show what would run without executing
        #[arg(long)]
        dry_run: bool,

        /// Set template variable (KEY=VALUE)
        #[arg(long = "var", value_name = "KEY=VALUE", value_parser = super::parse_key_val, action = clap::ArgAction::Append)]
        vars: Vec<(String, String)>,
    },

    /// Run post-remove hooks
    ///
    /// Background by default. Use `--foreground` to run in foreground for debugging.
    PostRemove {
        /// Filter by command name(s)
        ///
        /// Supports `user:name` or `project:name` to filter by source.
        /// `user:` alone runs all user hooks; `project:` alone runs all project hooks.
        #[arg(add = crate::completion::hook_command_name_completer())]
        name: Vec<String>,

        /// Show what would run without executing
        #[arg(long)]
        dry_run: bool,

        /// Run in foreground (block until complete)
        #[arg(long)]
        foreground: bool,

        /// Set template variable (KEY=VALUE)
        #[arg(long = "var", value_name = "KEY=VALUE", value_parser = super::parse_key_val, action = clap::ArgAction::Append)]
        vars: Vec<(String, String)>,
    },

    /// Internal: run a serialized pipeline from stdin
    #[command(hide = true, name = "run-pipeline")]
    RunPipeline,

    /// Deprecated: use `wt config approvals` instead
    #[command(hide = true)]
    Approvals {
        #[command(subcommand)]
        action: ApprovalsCommand,
    },
}
