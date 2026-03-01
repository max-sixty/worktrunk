//! Command suggestion helpers for hint messages.
//!
//! Build copy-pasteable commands for user suggestions:
//!
//! ```
//! use worktrunk::styling::{suggest_command, hint_message};
//! use color_print::cformat;
//!
//! let branch = "feature";
//! let cmd = suggest_command("remove", &[branch], &["--force"]);
//! println!("{}", hint_message(cformat!("To force delete, run <bright-black>{cmd}</>")));
//! // → ↳ To force delete, run wt remove --force feature
//! ```
//!
//! Handles shell escaping and `--` separator for args starting with `-`:
//!
//! ```
//! use worktrunk::styling::suggest_command;
//!
//! // Branch starting with dash gets -- separator; flags stay before --
//! let cmd = suggest_command("remove", &["-bugfix"], &["--force"]);
//! assert_eq!(cmd, "wt remove --force -- -bugfix");
//!
//! // Spaces are quoted
//! let cmd = suggest_command("remove", &["my feature"], &[]);
//! assert_eq!(cmd, "wt remove 'my feature'");
//! ```
//!
//! For commands targeting a specific worktree directory, use
//! [`suggest_command_in_dir`]:
//!
//! ```
//! use std::path::Path;
//! use worktrunk::styling::suggest_command_in_dir;
//!
//! let cmd = suggest_command_in_dir(Path::new("/tmp/repo"), "config", &["update"], &[]);
//! assert_eq!(cmd, "wt -C /tmp/repo config update");
//! ```

use shell_escape::escape;
use std::borrow::Cow;
use std::path::Path;

/// Build a suggested command string for hints.
///
/// Returns a copy-pasteable command like `wt remove --force feature`.
///
/// # Arguments
///
/// * `subcommand` - The subcommand name (e.g., "remove", "switch", "merge")
/// * `args` - Positional arguments (branch names, paths, etc.)
/// * `flags` - Additional flags to suggest (e.g., "--force", "-D")
///
/// # Shell escaping
///
/// Arguments containing spaces, quotes, or special shell characters are
/// automatically escaped using POSIX single-quote style.
///
/// # Dash-prefixed arguments
///
/// If any positional argument starts with `-`, a `--` separator is inserted
/// before it to prevent shell/clap from interpreting it as a flag.
pub fn suggest_command(subcommand: &str, args: &[&str], flags: &[&str]) -> String {
    format_command(None, subcommand, args, flags)
}

/// Like [`suggest_command`], but prepends `-C <path>` for commands targeting a
/// specific worktree directory.
///
/// The path is formatted with tilde shortening when safe (no escaping needed)
/// and falls back to a quoted absolute path otherwise.
pub fn suggest_command_in_dir(
    working_dir: &Path,
    subcommand: &str,
    args: &[&str],
    flags: &[&str],
) -> String {
    format_command(Some(working_dir), subcommand, args, flags)
}

fn format_command(
    working_dir: Option<&Path>,
    subcommand: &str,
    args: &[&str],
    flags: &[&str],
) -> String {
    let mut parts = vec!["wt".to_string()];

    if let Some(dir) = working_dir {
        parts.push("-C".to_string());
        parts.push(crate::path::format_path_for_display(dir));
    }

    parts.push(subcommand.to_string());

    // Flags go before positional args (and before any -- separator)
    // so they're always parsed as flags, not positional arguments.
    parts.extend(flags.iter().map(|s| s.to_string()));

    // Check if any arg starts with dash (needs -- separator)
    let needs_separator = args.iter().any(|arg| arg.starts_with('-'));
    let mut separator_inserted = false;

    for arg in args {
        // Insert -- before the first dash-prefixed arg
        if needs_separator && arg.starts_with('-') && !separator_inserted {
            parts.push("--".to_string());
            separator_inserted = true;
        }
        parts.push(escape(Cow::Borrowed(*arg)).into_owned());
    }

    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_command() {
        assert_eq!(
            suggest_command("remove", &["feature"], &[]),
            "wt remove feature"
        );
    }

    #[test]
    fn test_command_with_flag() {
        assert_eq!(
            suggest_command("remove", &["feature"], &["--force"]),
            "wt remove --force feature"
        );
    }

    #[test]
    fn test_command_with_multiple_flags() {
        assert_eq!(
            suggest_command("remove", &["feature"], &["--force", "--no-delete-branch"]),
            "wt remove --force --no-delete-branch feature"
        );
    }

    #[test]
    fn test_branch_with_spaces() {
        assert_eq!(
            suggest_command("remove", &["my feature"], &[]),
            "wt remove 'my feature'"
        );
    }

    #[test]
    fn test_branch_with_special_chars() {
        assert_eq!(
            suggest_command("remove", &["feature$1"], &[]),
            "wt remove 'feature$1'"
        );
    }

    #[test]
    fn test_branch_starting_with_dash() {
        assert_eq!(
            suggest_command("remove", &["-bugfix"], &[]),
            "wt remove -- -bugfix"
        );
    }

    #[test]
    fn test_branch_starting_with_dash_and_flag() {
        assert_eq!(
            suggest_command("remove", &["-bugfix"], &["--force"]),
            "wt remove --force -- -bugfix"
        );
    }

    #[test]
    fn test_multiple_args() {
        assert_eq!(
            suggest_command("remove", &["feature", "bugfix"], &[]),
            "wt remove feature bugfix"
        );
    }

    #[test]
    fn test_mixed_args_one_starting_with_dash() {
        // When one arg starts with dash, -- is inserted before it
        assert_eq!(
            suggest_command("remove", &["feature", "-bugfix"], &[]),
            "wt remove feature -- -bugfix"
        );
    }

    #[test]
    fn test_multiple_dash_prefixed_args() {
        // All dash-prefixed args follow the -- separator
        assert_eq!(
            suggest_command("remove", &["-bugfix", "-feature"], &[]),
            "wt remove -- -bugfix -feature"
        );
    }

    #[test]
    fn test_branch_with_single_quote() {
        assert_eq!(
            suggest_command("remove", &["it's-a-branch"], &[]),
            "wt remove 'it'\\''s-a-branch'"
        );
    }

    #[test]
    fn test_no_args() {
        assert_eq!(suggest_command("list", &[], &[]), "wt list");
    }

    #[test]
    fn test_flag_only() {
        assert_eq!(suggest_command("list", &[], &["--full"]), "wt list --full");
    }

    #[test]
    fn test_in_dir_simple_path() {
        assert_eq!(
            suggest_command_in_dir(Path::new("/tmp/repo"), "config", &["update"], &[]),
            "wt -C /tmp/repo config update"
        );
    }

    #[test]
    fn test_in_dir_path_with_spaces() {
        assert_eq!(
            suggest_command_in_dir(Path::new("/tmp/my repo"), "config", &["update"], &[]),
            "wt -C '/tmp/my repo' config update"
        );
    }

    #[test]
    fn test_in_dir_with_flags_and_args() {
        assert_eq!(
            suggest_command_in_dir(Path::new("/tmp/repo"), "remove", &["feature"], &["--force"]),
            "wt -C /tmp/repo remove --force feature"
        );
    }
}
