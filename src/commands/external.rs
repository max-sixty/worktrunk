//! Git-style external subcommand dispatch.
//!
//! When the user runs `wt foo` and `foo` is not a built-in subcommand, clap
//! captures the invocation via the `Commands::External` variant. This module
//! looks for an executable named `wt-foo` on `PATH` and runs it with the
//! remaining arguments, mirroring how `git foo` finds `git-foo`.
//!
//! Behaviour:
//!
//! 1. If the name matches a nested subcommand (e.g. `squash` → `wt step squash`),
//!    print the suggestion and exit — preserves the existing hint behaviour.
//! 2. Otherwise, resolve `wt-<name>` via `which`. If found, run it with the
//!    remaining args, inheriting stdio, and propagate the exit code.
//! 3. If not found, print a git-style "not a wt command" error, with a
//!    best-match suggestion computed from the list of built-in subcommands.
//!
//! Built-in subcommands always take precedence — clap only dispatches
//! `Commands::External` when no built-in matched, so there is no way for an
//! external `wt-switch` to shadow `wt switch`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use clap::CommandFactory;
use color_print::cformat;
use strsim::levenshtein;
use worktrunk::git::WorktrunkError;
use worktrunk::styling::{eprintln, error_message, hint_message};

use crate::cli::{Cli, suggest_nested_subcommand};

/// Handle a `Commands::External` invocation.
///
/// `args[0]` is the subcommand name; `args[1..]` are the arguments to pass
/// through. `working_dir`, if set, is the value of the top-level `-C <path>`
/// flag — applied as the child's current directory so global `-C` works the
/// same for external subcommands as it does for built-ins.
///
/// On success (child exit code 0), returns `Ok(())`. On non-zero exit or when
/// the command isn't found, returns `WorktrunkError::AlreadyDisplayed` with
/// the appropriate exit code so `main` can propagate it without printing an
/// extra error line (the child, or this module, has already reported the
/// failure). Exit code 1 for "not found" matches git's behaviour.
pub(crate) fn handle_external_command(
    args: Vec<OsString>,
    working_dir: Option<PathBuf>,
) -> Result<()> {
    let mut iter = args.into_iter();
    let name_os = iter
        .next()
        .expect("clap guarantees at least one arg for external subcommands");
    let rest: Vec<OsString> = iter.collect();

    let name = name_os
        .to_str()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "subcommand name is not valid UTF-8: {}",
                name_os.to_string_lossy()
            )
        })?
        .to_owned();

    // Nested subcommand suggestion takes precedence: `wt squash` should still
    // hint at `wt step squash` rather than searching PATH for `wt-squash`.
    let cli_cmd = Cli::command();
    if let Some(suggestion) = suggest_nested_subcommand(&cli_cmd, &name) {
        eprintln!(
            "{}",
            error_message(cformat!("Unrecognized subcommand '<cyan,bold>{name}</>'"))
        );
        eprintln!(
            "{}",
            hint_message(cformat!("Perhaps <cyan,bold>{suggestion}</>?"))
        );
        eprintln!("{}", hint_message(help_hint()));
        return Err(WorktrunkError::AlreadyDisplayed { exit_code: 2 }.into());
    }

    let binary = format!("wt-{name}");
    let Ok(path) = which::which(&binary) else {
        print_not_found(&name, &cli_cmd);
        return Err(WorktrunkError::AlreadyDisplayed { exit_code: 1 }.into());
    };

    run_external(&path, &rest, working_dir.as_deref())
}

/// Spawn the external binary, inheriting stdio, and propagate its exit code.
fn run_external(path: &Path, args: &[OsString], working_dir: Option<&Path>) -> Result<()> {
    let mut cmd = Command::new(path);
    cmd.args(args);
    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }

    let status = cmd
        .status()
        .with_context(|| format!("failed to execute {}", path.display()))?;

    if status.success() {
        return Ok(());
    }

    // Propagate the exact exit code — including signal codes on Unix — so
    // `wt foo` behaves like running `wt-foo` directly. We use
    // `AlreadyDisplayed` (not `ChildProcessExited`) because the external
    // command has already reported its own failure to the user; `wt` should
    // just forward the exit code without adding a second error line.
    #[cfg(unix)]
    if let Some(sig) = std::os::unix::process::ExitStatusExt::signal(&status) {
        return Err(WorktrunkError::AlreadyDisplayed {
            exit_code: 128 + sig,
        }
        .into());
    }

    let code = status.code().unwrap_or(1);
    Err(WorktrunkError::AlreadyDisplayed { exit_code: code }.into())
}

/// Print a git-style "not a wt command" error, with an optional typo suggestion.
fn print_not_found(name: &str, cli_cmd: &clap::Command) {
    eprintln!(
        "{}",
        error_message(cformat!("'<cyan,bold>{name}</>' is not a wt command"))
    );
    if let Some(suggestion) = closest_subcommand(name, cli_cmd) {
        eprintln!(
            "{}",
            hint_message(cformat!(
                "The most similar command is <cyan,bold>{suggestion}</>"
            ))
        );
    }
    eprintln!("{}", hint_message(help_hint()));
}

/// The "For more information, try `wt --help`" tail shared by both
/// unrecognized-subcommand branches. Mirrors the suggestion clap emitted
/// before we took over this error path.
fn help_hint() -> String {
    cformat!("For more information, try '<cyan,bold>wt --help</>'.")
}

/// Return the closest visible built-in subcommand name by Levenshtein distance,
/// or `None` if nothing is reasonably close.
fn closest_subcommand(name: &str, cli_cmd: &clap::Command) -> Option<String> {
    // Threshold chosen to mirror clap's internal `did_you_mean`: allow up to
    // a third of the input length in edits, but always tolerate at least one.
    let max_distance = (name.len() / 3).max(1);

    cli_cmd
        .get_subcommands()
        .filter(|c| !c.is_hide_set())
        .map(|c| c.get_name())
        .filter(|&candidate| candidate != "help")
        .map(|candidate| (candidate, levenshtein(name, candidate)))
        .filter(|&(_, dist)| dist <= max_distance)
        .min_by_key(|&(_, dist)| dist)
        .map(|(candidate, _)| candidate.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closest_subcommand_finds_typo() {
        let cmd = Cli::command();
        assert_eq!(
            closest_subcommand("siwtch", &cmd).as_deref(),
            Some("switch")
        );
    }

    #[test]
    fn closest_subcommand_ignores_unrelated() {
        let cmd = Cli::command();
        assert_eq!(closest_subcommand("zzzzzzzz", &cmd), None);
    }

    #[test]
    fn closest_subcommand_skips_hidden() {
        // `select` is hidden (deprecated); it should not be suggested even
        // though an exact-match candidate exists.
        let cmd = Cli::command();
        assert_eq!(closest_subcommand("select", &cmd), None);
    }

    #[cfg(unix)]
    #[test]
    fn handle_external_command_rejects_non_utf8_name() {
        use std::os::unix::ffi::OsStringExt;

        // clap routes the subcommand name through `OsString`, so a caller
        // with a non-UTF-8 argv could in principle reach this path. We
        // construct the same `Vec<OsString>` shape directly.
        let bad_name = OsString::from_vec(vec![0xFF, 0xFE]);
        let err = handle_external_command(vec![bad_name], None).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("not valid UTF-8"),
            "unexpected error message: {msg}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn run_external_propagates_signal_exit_code() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("create tempdir");
        let script = dir.path().join("wt-signal-test");
        std::fs::write(&script, "#!/bin/sh\nkill -TERM $$\n").expect("write script");
        let mut perms = std::fs::metadata(&script)
            .expect("stat script")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).expect("chmod script");

        let err = run_external(&script, &[], None).expect_err("child killed by SIGTERM");
        let wt_err = err
            .downcast_ref::<WorktrunkError>()
            .expect("signal should surface as WorktrunkError::AlreadyDisplayed");
        match wt_err {
            WorktrunkError::AlreadyDisplayed { exit_code } => {
                // SIGTERM = 15, and the shell-style convention is 128 + signal.
                assert_eq!(*exit_code, 128 + 15);
            }
            other => panic!("unexpected WorktrunkError variant: {other:?}"),
        }
    }
}
