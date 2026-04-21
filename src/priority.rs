//! Lowering process priority for background work.
//!
//! Worktrunk runs a handful of operations — background `wt remove` cleanup,
//! stale-trash sweeps, and `step copy-ignored` — that are latency-insensitive
//! but can compete for CPU and disk bandwidth with the foreground session.
//! This module centralises the policy we apply to those operations and the
//! two forms in which we apply it.
//!
//! ## Policy
//!
//! - **macOS**: `taskpolicy -b` enters `PRIO_DARWIN_BG` — lowers CPU
//!   scheduling *and* throttles disk + network I/O (see `setpriority(2)`).
//!   `nice(1)`/`renice(8)` only touch CPU on Darwin, leaving the dominant
//!   cost of a bulk `rm -rf` or reflink-fallback copy on APFS un-throttled.
//! - **Linux**: `nice -n 19` for CPU plus best-effort `ionice -c 3` (idle
//!   class) for I/O. `ionice` is probed once via `which` — it ships in
//!   `util-linux` on every mainstream distro and is enabled in Alpine's
//!   busybox, so the fallback path is only hit on stripped-down environments
//!   (distroless, minimal busybox, etc.).
//! - **Other Unix / Windows**: no-op.
//!
//! ## Why shell out?
//!
//! `setpriority(2)` (with `PRIO_DARWIN_BG` on Darwin) and `setiopolicy_np(3)`
//! would be more direct, but both are unsafe FFI and the crate has
//! `#![forbid(unsafe_code)]`.
//!
//! ## Forms
//!
//! - [`lower_current_process`] — self-lower by pid. Used when the *current*
//!   worktrunk process (and any threads/children it later spawns) should run
//!   at lower priority. The policy is inherited across `fork`/`exec`.
//! - [`command`] — build a [`Command`] that starts its child under the
//!   policy, by wrapping it in `taskpolicy -b <cmd>` or
//!   `ionice … nice … <cmd>`. Used for detached background spawns where we
//!   want the wrapper tool itself to apply the policy and then exec the real
//!   work.
//!
//! ## Background-hook context signalling
//!
//! When wt spawns a background hook pipeline (detached `wt hook run-pipeline`),
//! it exports [`FOREGROUND_ENV_VAR`] = [`BACKGROUND_HOOK_VALUE`] (`-1`) into
//! that process's environment. The variable is inherited by every child the
//! pipeline spawns (shell, user command, any nested `wt` invocation). Commands
//! that want to yield priority only when they're running inside a background
//! hook — rather than always — check it via [`in_background_hook`]. This is
//! an experimental hook-vs-foreground signal; the variable name and value are
//! not yet a stable contract.

use std::ffi::OsStr;
use std::process::Command;
#[cfg(unix)]
use std::process::Stdio;
#[cfg(all(unix, not(target_os = "macos")))]
use std::sync::LazyLock;

/// Whether `ionice` is available on PATH. Probed once per process so we don't
/// stat `$PATH` on every call.
#[cfg(all(unix, not(target_os = "macos")))]
static HAS_IONICE: LazyLock<bool> = LazyLock::new(|| which::which("ionice").is_ok());

/// Environment variable wt sets on background hook pipelines so descendants
/// can tell whether they're running in the foreground or inside a background
/// hook. Experimental; see the [module docs](self) for context.
pub const FOREGROUND_ENV_VAR: &str = "WORKTRUNK_FOREGROUND";

/// Value written to [`FOREGROUND_ENV_VAR`] when the enclosing context is a
/// background hook pipeline. Positive and zero values are reserved for future
/// use (e.g., signalling foreground hook nesting levels); only `-1` currently
/// has an observer.
pub const BACKGROUND_HOOK_VALUE: &str = "-1";

/// Returns `true` when wt detects that the current process is running inside
/// a background hook pipeline. Checks [`FOREGROUND_ENV_VAR`] against
/// [`BACKGROUND_HOOK_VALUE`]; any other value (including unset) returns
/// `false` so interactive and foreground-hook invocations stay at normal
/// priority.
pub fn in_background_hook() -> bool {
    is_background_hook_value(std::env::var_os(FOREGROUND_ENV_VAR).as_deref())
}

/// Extracted comparison so tests can exercise the match without mutating
/// process-global environment state (forbidden per `tests/CLAUDE.md`).
fn is_background_hook_value(value: Option<&OsStr>) -> bool {
    value == Some(OsStr::new(BACKGROUND_HOOK_VALUE))
}

/// Lower the current process's scheduling and I/O priority.
///
/// Non-fatal: if a helper binary is missing or fails, we proceed at normal
/// priority. No-op on non-Unix. See the [module docs](self) for the policy
/// applied on each platform.
pub fn lower_current_process() {
    #[cfg(unix)]
    {
        let pid = std::process::id().to_string();
        let quiet = |mut cmd: Command| {
            let _ = cmd
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        };

        #[cfg(target_os = "macos")]
        {
            let mut cmd = Command::new("/usr/sbin/taskpolicy");
            cmd.args(["-b", "-p", &pid]);
            quiet(cmd);
        }
        #[cfg(not(target_os = "macos"))]
        {
            let mut renice = Command::new("renice");
            renice.args(["-n", "19", "-p", &pid]);
            quiet(renice);
            if *HAS_IONICE {
                let mut ionice = Command::new("ionice");
                ionice.args(["-c", "3", "-p", &pid]);
                quiet(ionice);
            }
        }
    }
}

/// Build a [`Command`] that runs `program` at lowered priority when `lower`
/// is set, or at normal priority when not.
///
/// The wrapper tool (`taskpolicy` on macOS, `ionice`/`nice` on Linux) applies
/// the policy and then execs `program`, so policy is inherited by the child
/// and its descendants. `taskpolicy` takes `program` as a positional arg (no
/// `--` separator accepted); safe because callers pass `sh` or an absolute
/// path. See the [module docs](self) for the full policy.
pub fn command(program: impl AsRef<OsStr>, lower: bool) -> Command {
    if !lower {
        return Command::new(program);
    }
    #[cfg(target_os = "macos")]
    {
        let mut cmd = Command::new("/usr/sbin/taskpolicy");
        cmd.arg("-b").arg(program);
        cmd
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        linux_low_priority_command(program.as_ref(), *HAS_IONICE)
    }
    #[cfg(not(unix))]
    {
        Command::new(program)
    }
}

/// Linux wrap: `ionice -c 3 -- nice -n 19 -- <program>` if `has_ionice`,
/// else `nice -n 19 -- <program>`. Extracted so both branches are testable
/// without depending on whether the runner has `ionice` installed.
#[cfg(all(unix, not(target_os = "macos")))]
fn linux_low_priority_command(program: &OsStr, has_ionice: bool) -> Command {
    if has_ionice {
        let mut cmd = Command::new("ionice");
        cmd.args(["-c", "3", "--", "nice", "-n", "19", "--"])
            .arg(program);
        cmd
    } else {
        let mut cmd = Command::new("nice");
        cmd.arg("-n").arg("19").arg("--").arg(program);
        cmd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args_of(cmd: &Command) -> Vec<&str> {
        cmd.get_args().map(|a| a.to_str().unwrap()).collect()
    }

    #[test]
    fn foreground_env_var_name_and_value() {
        assert_eq!(FOREGROUND_ENV_VAR, "WORKTRUNK_FOREGROUND");
        assert_eq!(BACKGROUND_HOOK_VALUE, "-1");
    }

    #[test]
    fn background_hook_value_matches_sentinel_only() {
        // Only the exact sentinel counts as "inside a background hook".
        // Unset, empty, and other numeric-looking values all stay foreground
        // so interactive and `pre-*` hook callers run at normal priority.
        assert!(is_background_hook_value(Some(OsStr::new("-1"))));
        assert!(!is_background_hook_value(None));
        assert!(!is_background_hook_value(Some(OsStr::new(""))));
        assert!(!is_background_hook_value(Some(OsStr::new("0"))));
        assert!(!is_background_hook_value(Some(OsStr::new("1"))));
        assert!(!is_background_hook_value(Some(OsStr::new("-2"))));
        assert!(!is_background_hook_value(Some(OsStr::new(" -1"))));
    }

    #[test]
    fn command_no_lower_returns_bare() {
        let cmd = command("echo", false);
        assert_eq!(cmd.get_program(), "echo");
        assert!(args_of(&cmd).is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn command_lower_wraps_in_taskpolicy() {
        let cmd = command("echo", true);
        assert_eq!(cmd.get_program(), "/usr/sbin/taskpolicy");
        assert_eq!(args_of(&cmd), ["-b", "echo"]);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn linux_wrap_with_ionice() {
        let cmd = linux_low_priority_command(OsStr::new("echo"), true);
        assert_eq!(cmd.get_program(), "ionice");
        assert_eq!(
            args_of(&cmd),
            ["-c", "3", "--", "nice", "-n", "19", "--", "echo"]
        );
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn linux_wrap_without_ionice() {
        let cmd = linux_low_priority_command(OsStr::new("echo"), false);
        assert_eq!(cmd.get_program(), "nice");
        assert_eq!(args_of(&cmd), ["-n", "19", "--", "echo"]);
    }

    #[cfg(not(unix))]
    #[test]
    fn command_lower_noop_on_non_unix() {
        let cmd = command("echo", true);
        assert_eq!(cmd.get_program(), "echo");
        assert!(args_of(&cmd).is_empty());
    }

    #[test]
    fn lower_current_process_does_not_panic() {
        // Exercises the shell-out path; failures are silently swallowed so
        // this is effectively a smoke test that the cfg arms compile and run.
        lower_current_process();
    }
}
