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
        if *HAS_IONICE {
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
    #[cfg(not(unix))]
    {
        Command::new(program)
    }
}
