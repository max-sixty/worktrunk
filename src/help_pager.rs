//! Help text pager integration for CLI help output.
//!
//! Provides pager support for `--help` output using Git's pager selection.
//!
//! # Difference from the diff preview pager
//!
//! This pager is INTERACTIVE (spawned with TTY access) so users can scroll
//! `--help` output. The diff preview pager in `src/commands/picker/pager.rs`
//! runs inside the picker's preview window — a non-TTY context — so it spawns
//! differently:
//!
//! - Help pager: top-level user command, needs a TTY for interactive scrolling
//! - Diff preview pager: supplies the diff on stdin, appends `--paging=never`
//!   for pagers that would otherwise launch their own `less` (delta, bat), and
//!   is bounded by `PAGER_TIMEOUT` — it is never given a
//!   TTY, so it can't hang the picker's event loop
//!
//! Both follow git's pager detection but spawn differently based on their usage context.
//!
//! # Cross-Platform Support
//!
//! On Windows, Git Bash enables standard pagers like `less`. If the configured
//! pager cannot execute, help prints directly.

use std::io::IsTerminal;

use anyhow::{Context, Result};
use worktrunk::git::{ErrorExt, WorktrunkError};
use worktrunk::shell_exec::Cmd;
use worktrunk::styling::print;

use crate::pager::git_pager;

/// Show help text through a pager with TTY access for interactive scrolling.
///
/// The `use_pager` flag controls whether to attempt pager display:
/// - `true` (--help): Uses pager when available and terminal is detected
/// - `false` (-h): Always prints directly to stdout, never uses pager
///
/// This follows git's convention where `-h` never opens a pager (muscle-memory safe)
/// while `--help` uses a pager for longer content.
///
/// Even when `use_pager=true`, falls back to direct output if:
/// - Paging disabled or unavailable (prints to stdout)
/// - stdout is not a TTY (prints to stdout)
/// - Pager cannot start or I/O fails (prints to stdout)
///
/// A running pager owns its exit status and may close stdin early when the user
/// quits. Neither causes the help text to be printed a second time.
///
/// Native child interruption or controller cancellation propagates to the
/// command's normal cleanup and signal handling instead of printing fallback help.
///
/// Help text goes to stdout — POSIX convention (`wt --help | less` should work
/// without redirection), matching `cargo`, `curl`, `python`, `git <cmd> -h`,
/// and `--version` (see #2072).
pub(crate) fn show_help_in_pager(help_text: &str, use_pager: bool) -> Result<()> {
    // Short help (-h) never uses a pager
    if !use_pager {
        tracing::debug!("Short help (-h) requested, printing directly to stdout");
        print!("{}", help_text);
        return Ok(());
    }

    // Only page when our output destination is a terminal.
    // If stdout is piped/redirected (e.g., `wt --help | grep foo`), print directly.
    if !std::io::stdout().is_terminal() {
        tracing::debug!("stdout is not a TTY, skipping pager");
        print!("{}", help_text);
        return Ok(());
    }

    let Some(pager_cmd) = git_pager(true) else {
        tracing::debug!("Paging disabled or unavailable, printing help directly to stdout");
        print!("{}", help_text);
        return Ok(());
    };

    tracing::debug!(pager_cmd = %pager_cmd, "Invoking pager: {}", pager_cmd);
    if !pipe_through_pager(&pager_cmd, help_text)? {
        print!("{}", help_text);
    }
    Ok(())
}

/// Return whether the pager handled help, or false to request stdout fallback.
/// A POSIX shell reserves 126/127 for a command it could not execute; other
/// completed statuses belong to the pager (including less -K quitting with 2).
/// Native signal failures propagate instead of requesting fallback.
fn pipe_through_pager(pager_cmd: &str, help_text: &str) -> Result<bool> {
    let less_flags = compute_less_flags(std::env::var("LESS").ok().as_deref());
    match Cmd::shell(pager_cmd)
        .stdin_bytes(help_text.as_bytes())
        .env("LESS", less_flags)
        .forward_signals()
        .stream()
    {
        Ok(()) => Ok(true),
        Err(error) => {
            if let Some(signal) = error.interrupt_signal() {
                return Err(WorktrunkError::Interrupted { signal, hint: None }.into());
            }
            match error.downcast_ref::<WorktrunkError>() {
                Some(WorktrunkError::ChildProcessExited {
                    physical_signal: Some(_),
                    ..
                }) => Err(error).with_context(|| format!("Help pager `{pager_cmd}` failed")),
                Some(WorktrunkError::ChildProcessExited { code, .. })
                    if !matches!(*code, 126 | 127) =>
                {
                    Ok(true)
                }
                _ => {
                    tracing::debug!(error = %error, "Pager failed, falling back to stdout: {}", error);
                    Ok(false)
                }
            }
        }
    }
}

/// Compute LESS flags by appending our required flags to user's existing LESS setting.
///
/// Returns flags suitable for setting LESS env var when spawning less.
/// Ensures F (quit if one screen), R (colors), X (no termcap init) are always active.
fn compute_less_flags(user_less: Option<&str>) -> String {
    compute_less_flags_for(user_less, cfg!(windows))
}

/// Platform-parameterized core of [`compute_less_flags`], split out so both
/// branches are testable on any host.
///
/// On Windows we additionally pass `-K` (`--quit-on-intr`) so that pressing
/// Ctrl-C makes `less` exit cleanly — running its normal deinit that restores
/// the console input mode — instead of returning to its own prompt. `wt` has
/// no Windows console Ctrl-C handler (signal forwarding is Unix-only, see
/// `signal_forwarder.rs`), so a Ctrl-C otherwise terminates both `less` and
/// the blocked `wt` parent before the console mode is restored, leaving the
/// terminal wedged (#2968). `-K` routes Ctrl-C through the same clean-exit
/// path as quitting with `q`, which restores the terminal correctly.
///
/// `-K` is Windows-only on purpose: on Unix the pager receives the terminal's
/// Ctrl-C directly, and wt waits while a pager that handles it keeps running.
/// Many users rely on Ctrl-C returning `less` to its prompt rather than quitting.
fn compute_less_flags_for(user_less: Option<&str>, windows: bool) -> String {
    let base = user_less.unwrap_or_default();
    if windows {
        format!("{base} -FRX -K")
    } else {
        format!("{base} -FRX")
    }
}

#[cfg(test)]
mod tests {
    use super::compute_less_flags_for;

    #[cfg(unix)]
    #[test]
    fn test_pipe_through_pager_pipes_to_real_command() {
        assert!(super::pipe_through_pager("cat > /dev/null", "help text").unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn successful_pager_can_quit_before_consuming_input() {
        assert!(
            super::pipe_through_pager("head -c 50 > /dev/null", &"help text\n".repeat(100_000))
                .unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn completed_pager_owns_its_exit_status() {
        assert!(super::pipe_through_pager("cat > /dev/null; exit 1", "help text").unwrap());
        assert!(super::pipe_through_pager("exit 2", &"help text\n".repeat(100_000)).unwrap());
        assert!(super::pipe_through_pager("exit 7", "help text").unwrap());
        assert!(super::pipe_through_pager("exit 137", "help text").unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn pager_signal_remains_a_visible_failure() {
        let error = super::pipe_through_pager("kill -KILL $$", "help text").unwrap_err();
        assert!(matches!(
            error.downcast_ref::<worktrunk::git::WorktrunkError>(),
            Some(worktrunk::git::WorktrunkError::ChildProcessExited {
                physical_signal: Some(9),
                ..
            })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn pager_that_cannot_execute_triggers_fallback() {
        assert!(!super::pipe_through_pager("worktrunk-test-missing-pager", "help text").unwrap());
        let script = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(script.path(), "#!/bin/sh\n").unwrap();
        // NamedTempFile is not executable: the shell returns reserved status 126.
        let command = shell_escape::unix::escape(script.path().to_string_lossy());
        assert!(!super::pipe_through_pager(&command, "help text").unwrap());
    }

    #[test]
    fn test_compute_less_flags_empty() {
        // Leading space is fine - less ignores it
        assert_eq!(compute_less_flags_for(None, false), " -FRX");
        assert_eq!(compute_less_flags_for(Some(""), false), " -FRX");
    }

    #[test]
    fn test_compute_less_flags_short_options() {
        // Common case: user has -R (oh-my-zsh default)
        assert_eq!(compute_less_flags_for(Some("-R"), false), "-R -FRX");
        // User has multiple short flags
        assert_eq!(compute_less_flags_for(Some("-iMRS"), false), "-iMRS -FRX");
    }

    #[test]
    fn test_compute_less_flags_long_options() {
        // Issue #594: --mouse must not become --mouseFRX
        assert_eq!(
            compute_less_flags_for(Some("--mouse"), false),
            "--mouse -FRX"
        );
        // Multiple long options
        assert_eq!(
            compute_less_flags_for(Some("--mouse --shift=4"), false),
            "--mouse --shift=4 -FRX"
        );
    }

    #[test]
    fn test_compute_less_flags_mixed() {
        assert_eq!(
            compute_less_flags_for(Some("-R --mouse"), false),
            "-R --mouse -FRX"
        );
    }

    #[test]
    fn test_compute_less_flags_windows_appends_quit_on_intr() {
        // #2968: on Windows we add -K so Ctrl-C makes less exit cleanly and
        // restore the console, instead of wedging the terminal.
        assert_eq!(compute_less_flags_for(None, true), " -FRX -K");
        assert_eq!(compute_less_flags_for(Some("-R"), true), "-R -FRX -K");
        assert_eq!(
            compute_less_flags_for(Some("--mouse"), true),
            "--mouse -FRX -K"
        );
    }
}
