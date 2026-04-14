//! Git-style external subcommand dispatch.
//!
//! When the user runs `wt foo` and `foo` is not a built-in subcommand, clap
//! captures the invocation via the `Commands::External` variant. This module
//! looks for an executable named `wt-foo` on `PATH` and runs it with the
//! remaining arguments, mirroring how `git foo` finds `git-foo`.
//!
//! Behaviour:
//!
//! 1. Resolve `wt-<name>` via `which`. If found, run it with the remaining
//!    args, inheriting stdio, and propagate the exit code.
//! 2. If not found, synthesize clap's native `InvalidSubcommand` error and
//!    route it through `enhance_and_exit_error` so the output matches what
//!    clap would have produced without `external_subcommand` — same
//!    formatting, suggestions, Usage line, and nested-subcommand tip (e.g.
//!    `wt squash` → `perhaps wt step squash?`).
//!
//! Built-in subcommands always take precedence — clap only dispatches
//! `Commands::External` when no built-in matched, so there is no way for an
//! external `wt-switch` to shadow `wt switch`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use clap::error::{ContextKind, ContextValue, ErrorKind};
use strsim::jaro_winkler;
use worktrunk::git::WorktrunkError;

use crate::cli::build_command;
use crate::enhance_and_exit_error;

/// Handle a `Commands::External` invocation.
///
/// `args[0]` is the subcommand name; `args[1..]` are the arguments to pass
/// through. `working_dir`, if set, is the value of the top-level `-C <path>`
/// flag — applied as the child's current directory so global `-C` works the
/// same for external subcommands as it does for built-ins.
///
/// On success (child exit code 0), returns `Ok(())`. On non-zero exit, returns
/// `WorktrunkError::AlreadyDisplayed` with the child's exit code so `main`
/// can propagate it without printing an extra error line. When the command
/// isn't found on PATH, diverges via `enhance_and_exit_error` with clap's
/// standard exit code 2.
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

    // Try the PATH lookup first. If a `wt-<name>` binary exists, it takes
    // precedence over clap's "unrecognized subcommand" error path — but *not*
    // over built-ins, because clap only dispatches `External` when no built-in
    // matched. Nested-subcommand hints (`wt squash` → `wt step squash`) are
    // applied by `enhance_and_exit_error` when we fall through below, so a
    // name that matches a nested subcommand still gets its tip even though
    // we look at PATH first (nested names aren't expected to collide with
    // real `wt-*` binaries, and if they do the on-PATH binary wins — same as
    // git's behaviour).
    let binary = format!("wt-{name}");
    if let Ok(path) = which::which(&binary) {
        return run_external(&path, &rest, working_dir.as_deref());
    }

    // Not on PATH — emit clap's native `InvalidSubcommand` error. Routing
    // through `enhance_and_exit_error` keeps the rendering consistent with
    // every other clap error (same tip/Usage formatting) and layers the
    // wt-specific nested-subcommand hint on top.
    enhance_and_exit_error(unrecognized_subcommand_error(&name));
}

/// Build a `clap::Error` that mirrors what clap itself would have raised for
/// an unrecognized top-level subcommand if we weren't capturing via
/// `#[command(external_subcommand)]`. Populates `InvalidSubcommand`,
/// `SuggestedSubcommand`, and `Usage` context so clap's rich formatter
/// produces its native output (the "tip:" line and "Usage:" block come from
/// these context entries).
fn unrecognized_subcommand_error(name: &str) -> clap::Error {
    let mut cmd = build_command();
    let mut err = clap::Error::new(ErrorKind::InvalidSubcommand).with_cmd(&cmd);
    err.insert(
        ContextKind::InvalidSubcommand,
        ContextValue::String(name.to_string()),
    );
    let suggestions = similar_subcommands(name, &cmd);
    if !suggestions.is_empty() {
        err.insert(
            ContextKind::SuggestedSubcommand,
            ContextValue::Strings(suggestions),
        );
    }
    err.insert(
        ContextKind::Usage,
        ContextValue::StyledStr(cmd.render_usage()),
    );
    err
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

/// Return visible built-in subcommand names similar to `name`, sorted by
/// descending confidence. Mirrors clap's internal `did_you_mean` (Jaro–Winkler
/// similarity with a 0.7 threshold) so the `tip:` line reads the same as it
/// would have without `#[command(external_subcommand)]` intercepting the
/// error.
fn similar_subcommands(name: &str, cli_cmd: &clap::Command) -> Vec<String> {
    let mut scored: Vec<(f64, String)> = cli_cmd
        .get_subcommands()
        .filter(|c| !c.is_hide_set())
        .map(|c| c.get_name())
        .filter(|&candidate| candidate != "help")
        .map(|candidate| (jaro_winkler(name, candidate), candidate.to_string()))
        .filter(|(score, _)| *score > 0.7)
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().map(|(_, name)| name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similar_subcommands_finds_typo() {
        let cmd = build_command();
        let suggestions = similar_subcommands("siwtch", &cmd);
        assert_eq!(
            suggestions.first().map(String::as_str),
            Some("switch"),
            "got: {suggestions:?}"
        );
    }

    #[test]
    fn similar_subcommands_ignores_unrelated() {
        let cmd = build_command();
        assert!(similar_subcommands("zzzzzzzz", &cmd).is_empty());
    }

    #[test]
    fn similar_subcommands_skips_hidden() {
        // `select` is hidden (deprecated); it should not be suggested even
        // though an exact-match candidate exists.
        let cmd = build_command();
        assert!(!similar_subcommands("select", &cmd).contains(&"select".to_string()));
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
