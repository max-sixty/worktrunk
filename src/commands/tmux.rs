//! Tmux integration for launching worktree operations in new windows/sessions.

use anyhow::{Result, bail};
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
        .chars()
        .map(|c| if matches!(c, '/' | '.' | ':') { '-' } else { c })
        .collect()
}

/// Options for spawning a worktree switch in tmux.
pub struct TmuxSwitchOptions<'a> {
    pub branch: &'a str,
    pub create: bool,
    pub base: Option<&'a str>,
    pub execute: Option<&'a str>,
    pub execute_args: &'a [String],
    pub yes: bool,
    pub clobber: bool,
    pub verify: bool,
    pub detach: bool,
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

/// Environment variables to skip when passing to tmux session.
const SKIP_ENV_VARS: &[&str] = &[
    "PWD",
    "OLDPWD",
    "_",
    "SHLVL",
    "TMUX",
    "TMUX_PANE",
    "TERM_SESSION_ID",
    "SHELL_SESSION_ID",
    "WORKTRUNK_DIRECTIVE_FILE",
    "COMP_LINE",
    "COMP_POINT",
];

/// Write environment variables to a temp file and return the path.
/// The file contains export statements that can be sourced.
fn write_env_file() -> Option<std::path::PathBuf> {
    use std::io::Write;

    let path = std::env::temp_dir().join(format!("wt-env-{}", std::process::id()));
    let mut file = std::fs::File::create(&path).ok()?;

    for (key, value) in env::vars() {
        if SKIP_ENV_VARS.contains(&key.as_str()) || key.starts_with("__") {
            continue;
        }
        if let Ok(escaped) = shlex::try_quote(&value) {
            writeln!(file, "export {}={}", key, escaped).ok()?;
        }
    }

    Some(path)
}

/// Capture the visible output from a tmux pane, cleaning up whitespace.
fn capture_pane(session: &str) -> String {
    Command::new("tmux")
        .args(["capture-pane", "-t", session, "-p"])
        .output()
        .map(|o| {
            let raw = String::from_utf8_lossy(&o.stdout);
            // Filter out blank lines and tmux's "Pane is dead" message
            raw.lines()
                .filter(|line| {
                    let trimmed = line.trim();
                    !trimmed.is_empty() && !trimmed.starts_with("Pane is dead")
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// Result of spawning a tmux session/window.
pub enum TmuxSpawnResult {
    /// Created detached session with this name
    Detached(String),
    /// Detached session failed quickly - includes captured output
    DetachedFailed { name: String, output: String },
    /// Created new window (already in tmux)
    Window(String),
    /// Switched to existing window (was inside tmux)
    SwitchedWindow(String),
}

/// Spawn worktree switch in a new tmux window/session.
///
/// # Returns
/// * `TmuxSpawnResult::Window` - If already in tmux, created new window
/// * `TmuxSpawnResult::Detached` - If not in tmux and detach=true
/// * Never returns if not in tmux and detach=false (exec replaces process)
#[cfg(unix)]
pub fn spawn_switch_in_tmux(opts: &TmuxSwitchOptions<'_>) -> Result<TmuxSpawnResult> {
    use std::os::unix::process::CommandExt;

    if !is_available() {
        bail!("tmux not found. Install tmux or run without --tmux");
    }

    let name = sanitize_name(opts.branch);

    // Build the wt command to run inside tmux
    let mut wt_args = vec!["switch".to_string()];
    if opts.create {
        wt_args.push("--create".to_string());
    }
    wt_args.push(opts.branch.to_string());
    if let Some(b) = opts.base {
        wt_args.push("--base".to_string());
        wt_args.push(b.to_string());
    }
    if let Some(cmd) = opts.execute {
        wt_args.push("--execute".to_string());
        wt_args.push(cmd.to_string());
    }
    if opts.yes {
        wt_args.push("--yes".to_string());
    }
    if opts.clobber {
        wt_args.push("--clobber".to_string());
    }
    if !opts.verify {
        wt_args.push("--no-verify".to_string());
    }
    // Add execute_args after -- separator
    if !opts.execute_args.is_empty() {
        wt_args.push("--".to_string());
        wt_args.extend(opts.execute_args.iter().cloned());
    }

    // Shell-escape each argument
    let escaped_args: Vec<String> = wt_args
        .iter()
        .map(|arg| shlex::try_quote(arg).unwrap_or(arg.into()).into_owned())
        .collect();

    // Write env vars to temp file for sourcing in tmux
    let env_file = write_env_file();

    // Command to run: source env file, then wt switch, then keep shell open
    let wt_command = match &env_file {
        Some(path) => format!(
            "source {} && rm -f {} && wt {} && exec $SHELL",
            path.display(),
            path.display(),
            escaped_args.join(" ")
        ),
        None => format!("wt {} && exec $SHELL", escaped_args.join(" ")),
    };

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
    } else if opts.detach {
        // Not in tmux, detach mode: create detached session
        // Use remain-on-exit so we can capture output if command fails quickly
        let mut cmd = Command::new("tmux");
        cmd.args([
            "new-session",
            "-d",
            "-s",
            &name,
            "-x",
            "200", // Wide enough to not wrap output
            "-y",
            "50",
            &wt_command,
        ]);
        shell_exec::run(&mut cmd, None)?;

        // Set remain-on-exit so pane stays if command fails
        let mut cmd = Command::new("tmux");
        cmd.args(["set-option", "-t", &name, "remain-on-exit", "on"]);
        let _ = cmd.output(); // Ignore errors

        // Wait briefly and check if the command is still running
        std::thread::sleep(std::time::Duration::from_secs(2));

        // Check if the pane is dead (command exited)
        let pane_dead = Command::new("tmux")
            .args(["list-panes", "-t", &name, "-F", "#{pane_dead}"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "1")
            .unwrap_or(false);

        if pane_dead {
            // Command exited quickly - likely an error
            let output = capture_pane(&name);

            // Kill the dead session
            let mut cmd = Command::new("tmux");
            cmd.args(["kill-session", "-t", &name]);
            let _ = cmd.output();

            Ok(TmuxSpawnResult::DetachedFailed {
                name: name.clone(),
                output,
            })
        } else {
            // Command still running - turn off remain-on-exit for normal behavior
            let mut cmd = Command::new("tmux");
            cmd.args(["set-option", "-t", &name, "remain-on-exit", "off"]);
            let _ = cmd.output();

            Ok(TmuxSpawnResult::Detached(name))
        }
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
pub fn spawn_switch_in_tmux(_opts: &TmuxSwitchOptions<'_>) -> Result<TmuxSpawnResult> {
    bail!("tmux is not available on Windows")
}
