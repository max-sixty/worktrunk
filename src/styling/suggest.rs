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
//! // → ↳ To force delete, run wt remove feature --force
//! ```
//!
//! Handles shell escaping and `--` separator for args starting with `-`:
//!
//! ```
//! use worktrunk::styling::suggest_command;
//!
//! // Branch starting with dash gets -- separator
//! let cmd = suggest_command("remove", &["-bugfix"], &["--force"]);
//! assert_eq!(cmd, "wt remove -- -bugfix --force");
//!
//! // Spaces are quoted
//! let cmd = suggest_command("remove", &["my feature"], &[]);
//! assert_eq!(cmd, "wt remove 'my feature'");
//! ```

use shell_escape::escape;
use std::borrow::Cow;

/// Build a suggested command string for hints.
///
/// Returns a copy-pasteable command like `wt remove feature --force`.
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
///
/// # Design note: Global flags
///
/// Intentionally excludes global flags like `-C` (working directory).
/// Suggestions show the operation, not the invocation context.
/// Users may have changed directory since the error occurred.
///
/// If we later find users frequently need `-C` in suggestions (e.g., scripting
/// contexts), we could add an optional `global_flags` parameter.
pub fn suggest_command(subcommand: &str, args: &[&str], flags: &[&str]) -> String {
    let mut parts = vec!["wt".to_string(), subcommand.to_string()];

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

    parts.extend(flags.iter().map(|s| s.to_string()));
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
            "wt remove feature --force"
        );
    }

    #[test]
    fn test_command_with_multiple_flags() {
        assert_eq!(
            suggest_command("remove", &["feature"], &["--force", "--no-delete-branch"]),
            "wt remove feature --force --no-delete-branch"
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
            "wt remove -- -bugfix --force"
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
}
