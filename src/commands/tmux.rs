//! Tmux integration for launching worktree operations in new windows/sessions.

use anyhow::{bail, Result};
use std::env;
use std::process::Command;

use worktrunk::shell_exec;

/// Check if tmux is available in PATH.
pub fn is_available() -> bool {
    which::which("tmux").is_ok()
}

/// Check if we're inside a tmux session.
pub fn is_inside_tmux() -> bool {
    env::var("TMUX").is_ok()
}

/// Sanitize branch name for tmux session/window name.
///
/// Tmux doesn't allow certain characters in session/window names.
fn sanitize_name(branch: &str) -> String {
    branch
        .replace('/', "-")
        .replace('.', "-")
        .replace(':', "-")
}

/// Check if a tmux session with the given name exists.
fn session_exists(name: &str) -> bool {
    Command::new("tmux")
        .args(["has-session", "-t", name])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Check if a window with the given name exists in the current tmux session.
fn window_exists(name: &str) -> bool {
    Command::new("tmux")
        .args(["list-windows", "-F", "#{window_name}"])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .any(|line| line == name)
        })
        .unwrap_or(false)
}

/// Result of spawning a tmux session/window.
pub enum TmuxSpawnResult {
    /// Created detached session with this name
    Detached(String),
    /// Created new window (already in tmux)
    Window(String),
    /// Switched to existing window (was inside tmux)
    SwitchedWindow(String),
}

/// Spawn worktree switch in a new tmux window/session.
///
/// # Arguments
/// * `branch` - Branch name to switch to/create
/// * `create` - Whether to create a new branch
/// * `base` - Optional base branch for creation
/// * `execute` - Optional command to run after switch
/// * `execute_args` - Additional arguments for execute command
/// * `yes` - Skip approval prompts
/// * `clobber` - Remove stale paths at target
/// * `verify` - Run hooks (false means --no-verify)
/// * `detach` - Create detached session instead of attaching
///
/// # Returns
/// * `TmuxSpawnResult::Window` - If already in tmux, created new window
/// * `TmuxSpawnResult::Detached` - If not in tmux and detach=true
/// * Never returns if not in tmux and detach=false (exec replaces process)
#[cfg(unix)]
pub fn spawn_switch_in_tmux(
    branch: &str,
    create: bool,
    base: Option<&str>,
    execute: Option<&str>,
    execute_args: &[String],
    yes: bool,
    clobber: bool,
    verify: bool,
    detach: bool,
) -> Result<TmuxSpawnResult> {
    use std::os::unix::process::CommandExt;

    if !is_available() {
        bail!("tmux not found. Install tmux or run without --tmux");
    }

    let name = sanitize_name(branch);

    // Build the wt command to run inside tmux
    let mut wt_args = vec!["switch".to_string()];
    if create {
        wt_args.push("--create".to_string());
    }
    wt_args.push(branch.to_string());
    if let Some(b) = base {
        wt_args.push("--base".to_string());
        wt_args.push(b.to_string());
    }
    if let Some(cmd) = execute {
        wt_args.push("--execute".to_string());
        wt_args.push(cmd.to_string());
    }
    if yes {
        wt_args.push("--yes".to_string());
    }
    if clobber {
        wt_args.push("--clobber".to_string());
    }
    if !verify {
        wt_args.push("--no-verify".to_string());
    }
    // Add execute_args after -- separator
    if !execute_args.is_empty() {
        wt_args.push("--".to_string());
        wt_args.extend(execute_args.iter().cloned());
    }

    // Shell-escape each argument
    let escaped_args: Vec<String> = wt_args
        .iter()
        .map(|arg| {
            shlex::try_quote(arg)
                .unwrap_or(arg.into())
                .into_owned()
        })
        .collect();

    // Command to run: wt switch ... && exec $SHELL (keep shell open after completion)
    let wt_command = format!("wt {} && exec $SHELL", escaped_args.join(" "));

    if is_inside_tmux() {
        // Already in tmux: check if window exists
        if window_exists(&name) {
            // Switch to existing window
            let mut cmd = Command::new("tmux");
            cmd.args(["select-window", "-t", &name]);
            shell_exec::run(&mut cmd, None)?;
            Ok(TmuxSpawnResult::SwitchedWindow(name))
        } else {
            // Create new window and switch to it
            let mut cmd = Command::new("tmux");
            cmd.args(["new-window", "-n", &name, &wt_command]);
            shell_exec::run(&mut cmd, None)?;
            Ok(TmuxSpawnResult::Window(name))
        }
    } else if session_exists(&name) {
        // Session exists: attach to it (exec replaces process)
        let mut cmd = Command::new("tmux");
        cmd.args(["attach-session", "-t", &name]);
        let err = cmd.exec();
        Err(err.into())
    } else if detach {
        // Not in tmux, detach mode: create detached session
        let mut cmd = Command::new("tmux");
        cmd.args(["new-session", "-d", "-s", &name, &wt_command]);
        shell_exec::run(&mut cmd, None)?;
        Ok(TmuxSpawnResult::Detached(name))
    } else {
        // Not in tmux, attach mode: exec into tmux (replaces process)
        let mut cmd = Command::new("tmux");
        cmd.args(["new-session", "-s", &name, &wt_command]);
        // exec() replaces the current process with tmux, so this only returns on error
        let err = cmd.exec();
        Err(err.into())
    }
}

/// Windows stub - tmux is not available on Windows.
#[cfg(not(unix))]
pub fn spawn_switch_in_tmux(
    _branch: &str,
    _create: bool,
    _base: Option<&str>,
    _execute: Option<&str>,
    _execute_args: &[String],
    _yes: bool,
    _clobber: bool,
    _verify: bool,
    _detach: bool,
) -> Result<TmuxSpawnResult> {
    bail!("tmux is not available on Windows")
}
