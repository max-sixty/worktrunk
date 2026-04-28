//! Git-style custom subcommand dispatch.
//!
//! When the user runs `wt foo` and `foo` is not a built-in subcommand, clap
//! captures the invocation via the `Commands::Custom` variant. This module
//! resolves it in this order:
//!
//! 1. **Alias**: if `foo` is configured as an alias in user/project config,
//!    run it via the same path as `wt step foo`. User config wins over
//!    `wt-<name>` PATH binaries — aliases are how users customize wt, so the
//!    user's intent should take precedence.
//! 2. **PATH binary**: resolve `wt-<name>` via `which`. If found, run it with
//!    the remaining args, inheriting stdio, and propagate the exit code.
//!    Mirrors how `git foo` finds `git-foo`.
//! 3. Otherwise, synthesize clap's native `InvalidSubcommand` error (with
//!    aliases included in the "did you mean" candidates) and return it via
//!    `enhance_clap_error` so the output matches what clap would have produced
//!    without `external_subcommand` — same formatting, suggestions, Usage
//!    line, and nested-subcommand tip (e.g. `wt squash` →
//!    `perhaps wt step squash?`). Returning (rather than exiting) lets
//!    `finish_command` run its cleanup (diagnostic writes, ANSI reset for
//!    shell integration).
//!
//! Built-in subcommands always take precedence — clap only dispatches
//! `Commands::Custom` when no built-in matched, so there is no way for an
//! alias or `wt-switch` on PATH to shadow `wt switch`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use worktrunk::git::WorktrunkError;

use crate::cli::build_command;
use crate::commands::{
    alias_names_for_suggestions, build_invalid_subcommand_error, similar_subcommands, try_alias,
};
use crate::enhance_clap_error;

/// Handle a `Commands::Custom` invocation.
///
/// `args[0]` is the subcommand name; `args[1..]` are the arguments to pass
/// through. `working_dir`, if set, is the value of the top-level `-C <path>`
/// flag — applied as the child's current directory so global `-C` works the
/// same for custom subcommands as it does for built-ins. `yes` is the global
/// `--yes`/`-y` flag, passed to alias dispatch so it can skip approval prompts
/// for project-config aliases.
///
/// On success (child exit code 0), returns `Ok(())`. On non-zero exit, returns
/// `WorktrunkError::AlreadyDisplayed` with the child's exit code so `main`
/// can propagate it without printing an extra error line. When the command
/// isn't found on PATH, returns `AlreadyDisplayed` via `enhance_clap_error`
/// with clap's standard exit code 2.
pub(crate) fn handle_custom_command(
    args: Vec<OsString>,
    working_dir: Option<PathBuf>,
    yes: bool,
) -> Result<()> {
    let mut iter = args.into_iter();
    let name_os = iter
        .next()
        .expect("clap guarantees at least one arg for external_subcommand variants");
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

    // Try alias dispatch first so user/project config wins over PATH binaries
    // of the same name. Built-ins still take precedence — clap only routes
    // to `Commands::Custom` when no built-in matched. The alias arg parser
    // requires UTF-8, but we only run it after confirming the name is a
    // configured alias, so non-UTF-8 args meant for a custom subcommand binary
    // don't surface as alias parse errors.
    let alias_args: Option<Vec<String>> = rest
        .iter()
        .map(|a| a.to_str().map(|s| s.to_owned()))
        .collect();
    if let Some(alias_args) = alias_args
        && let Some(()) = try_alias(name.clone(), alias_args, yes)?
    {
        return Ok(());
    }

    // Fall through to `wt-<name>` PATH binary. Nested-subcommand hints
    // (`wt squash` → `wt step squash`) are applied by `enhance_clap_error`
    // when we fall through below, so a name that matches a nested subcommand
    // still gets its tip even though we look at PATH before erroring (nested
    // names aren't expected to collide with real `wt-*` binaries, and if they
    // do the on-PATH binary wins — same as git's behaviour).
    let binary = format!("wt-{name}");
    if let Ok(path) = which::which(&binary) {
        return run_custom(&path, &rest, working_dir.as_deref());
    }

    // Not an alias and not on PATH — emit clap's native `InvalidSubcommand`
    // error. Routing through `enhance_clap_error` keeps the rendering
    // consistent with every other clap error (same tip/Usage formatting) and
    // layers the wt-specific nested-subcommand hint on top. Returning
    // `AlreadyDisplayed` (rather than calling `process::exit`) lets
    // `finish_command` run its cleanup before wt exits.
    Err(enhance_clap_error(unrecognized_subcommand_error(&name)))
}

/// Build a `clap::Error` that mirrors what clap itself would have raised for
/// an unrecognized top-level subcommand if we weren't capturing via
/// `#[command(external_subcommand)]`. Populates `InvalidSubcommand`,
/// `SuggestedSubcommand`, and `Usage` context so clap's rich formatter
/// produces its native output (the "tip:" line and "Usage:" block come from
/// these context entries).
///
/// Configured aliases are mixed into the candidate pool so a typo like
/// `wt deplyo` produces `tip: ... 'deploy'` when `deploy` is user-defined,
/// matching the discovery surface of `wt --help`.
fn unrecognized_subcommand_error(name: &str) -> clap::Error {
    let mut cmd = build_command();
    let alias_names = alias_names_for_suggestions();
    let suggestions = similar_subcommands(name, &cmd, &alias_names);
    build_invalid_subcommand_error(&mut cmd, name, suggestions)
}

/// Spawn the custom binary, inheriting stdio, and propagate its exit code.
fn run_custom(path: &Path, args: &[OsString], working_dir: Option<&Path>) -> Result<()> {
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
    // `AlreadyDisplayed` (not `ChildProcessExited`) because the custom
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similar_subcommands_finds_typo() {
        let cmd = build_command();
        let suggestions = similar_subcommands("siwtch", &cmd, &[]);
        assert_eq!(
            suggestions.first().map(String::as_str),
            Some("switch"),
            "got: {suggestions:?}"
        );
    }

    #[test]
    fn similar_subcommands_ignores_unrelated() {
        let cmd = build_command();
        assert!(similar_subcommands("zzzzzzzz", &cmd, &[]).is_empty());
    }

    #[test]
    fn similar_subcommands_skips_hidden() {
        // `select` is hidden (deprecated); it should not be suggested even
        // though an exact-match candidate exists.
        let cmd = build_command();
        assert!(!similar_subcommands("select", &cmd, &[]).contains(&"select".to_string()));
    }

    #[test]
    fn similar_subcommands_includes_aliases() {
        // Alias names mix into the candidate pool so a typo close to a
        // user-defined alias shows up in the `tip:` line.
        let cmd = build_command();
        let aliases = vec!["deploy".to_string(), "release".to_string()];
        let suggestions = similar_subcommands("deplyo", &cmd, &aliases);
        assert_eq!(
            suggestions.first().map(String::as_str),
            Some("deploy"),
            "got: {suggestions:?}"
        );
    }

    #[test]
    fn similar_subcommands_dedupes_alias_matching_builtin() {
        // An alias whose name shadows a built-in (e.g. `list`) should appear
        // only once in suggestions, not duplicated.
        let cmd = build_command();
        let aliases = vec!["list".to_string()];
        let suggestions = similar_subcommands("list", &cmd, &aliases);
        let count = suggestions.iter().filter(|n| *n == "list").count();
        assert_eq!(count, 1, "got: {suggestions:?}");
    }

    #[cfg(unix)]
    #[test]
    fn handle_custom_command_rejects_non_utf8_name() {
        use std::os::unix::ffi::OsStringExt;

        // clap routes the subcommand name through `OsString`, so a caller
        // with a non-UTF-8 argv could in principle reach this path. We
        // construct the same `Vec<OsString>` shape directly.
        let bad_name = OsString::from_vec(vec![0xFF, 0xFE]);
        let err = handle_custom_command(vec![bad_name], None, false).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("not valid UTF-8"),
            "unexpected error message: {msg}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn run_custom_propagates_signal_exit_code() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("create tempdir");
        let script = dir.path().join("wt-signal-test");
        std::fs::write(&script, "#!/bin/sh\nkill -TERM $$\n").expect("write script");
        let mut perms = std::fs::metadata(&script)
            .expect("stat script")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).expect("chmod script");

        let err = run_custom(&script, &[], None).expect_err("child killed by SIGTERM");
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
