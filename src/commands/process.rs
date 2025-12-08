use anyhow::Context;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use worktrunk::git::Repository;
use worktrunk::path::format_path_for_display;

/// Spawn a detached background process with output redirected to a log file
///
/// The process will be fully detached from the parent:
/// - On Unix: uses double-fork with setsid to create a daemon
/// - On Windows: uses CREATE_NEW_PROCESS_GROUP to detach from console
///
/// Logs are centralized in the main worktree's `.git/wt-logs/` directory.
///
/// # Arguments
/// * `repo` - Repository instance for accessing git common directory
/// * `worktree_path` - Working directory for the command
/// * `command` - Shell command to execute
/// * `branch` - Branch name for log organization
/// * `name` - Operation identifier (e.g., "post-start-npm", "remove")
/// * `context_json` - Optional JSON context to pipe to command's stdin
///
/// # Returns
/// Path to the log file where output is being written
pub fn spawn_detached(
    repo: &Repository,
    worktree_path: &Path,
    command: &str,
    branch: &str,
    name: &str,
    context_json: Option<&str>,
) -> anyhow::Result<std::path::PathBuf> {
    // Get the git common directory (shared across all worktrees)
    let git_common_dir = repo.git_common_dir()?;

    // Create log directory in the common git directory
    let log_dir = git_common_dir.join("wt-logs");
    fs::create_dir_all(&log_dir).with_context(|| {
        format!(
            "Failed to create log directory {}",
            format_path_for_display(&log_dir)
        )
    })?;

    // Generate log filename (no timestamp - overwrites on each run)
    // Format: {branch}-{name}.log (e.g., "feature-post-start-npm.log", "bugfix-remove.log")
    // Sanitize branch and name: replace '/' with '-' to avoid creating subdirectories
    let safe_branch = branch.replace('/', "-");
    let safe_name = name.replace('/', "-");
    let log_path = log_dir.join(format!("{}-{}.log", safe_branch, safe_name));

    // Create log file
    let log_file = fs::File::create(&log_path).with_context(|| {
        format!(
            "Failed to create log file {}",
            format_path_for_display(&log_path)
        )
    })?;

    #[cfg(unix)]
    {
        spawn_detached_unix(worktree_path, command, log_file, context_json)?;
    }

    #[cfg(windows)]
    {
        spawn_detached_windows(worktree_path, command, log_file, context_json)?;
    }

    Ok(log_path)
}

#[cfg(unix)]
fn spawn_detached_unix(
    worktree_path: &Path,
    command: &str,
    log_file: fs::File,
    context_json: Option<&str>,
) -> anyhow::Result<()> {
    // Detachment using nohup and background execution (&):
    // - nohup makes the process immune to SIGHUP (continues after parent exits)
    // - sh -c allows complex shell commands with pipes, redirects, etc.
    // - & backgrounds the process immediately
    // - We wait for the outer shell to exit (happens immediately after backgrounding)
    // - This prevents zombie process accumulation under high concurrency
    // - Output redirected to log file for debugging

    // Build the command, optionally piping JSON context to stdin
    let full_command = match context_json {
        Some(json) => {
            // Use printf to pipe JSON to the command's stdin
            // printf is more portable than echo for arbitrary content
            // Wrap command in braces to ensure proper grouping with &&, ||, etc.
            // Only add semicolon if command doesn't already end with newline or semicolon
            // (shell syntax: `{ cmd\n }` is valid, but `{ cmd\n; }` is not)
            let separator = if command.ends_with('\n') || command.ends_with(';') {
                ""
            } else {
                ";"
            };
            format!(
                "printf '%s' {} | {{ {}{} }}",
                shell_escape::escape(json.into()),
                command,
                separator
            )
        }
        None => command.to_string(),
    };

    let mut child = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "nohup sh -c {} &",
            shell_escape::escape(full_command.into())
        ))
        .current_dir(worktree_path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            log_file
                .try_clone()
                .context("Failed to clone log file handle")?,
        ))
        .stderr(Stdio::from(log_file))
        .spawn()
        .context("Failed to spawn detached process")?;

    // Wait for the outer shell to exit (immediate, doesn't block on background command)
    child
        .wait()
        .context("Failed to wait for detachment shell")?;

    Ok(())
}

#[cfg(windows)]
fn spawn_detached_windows(
    worktree_path: &Path,
    command: &str,
    log_file: fs::File,
    context_json: Option<&str>,
) -> anyhow::Result<()> {
    use std::os::windows::process::CommandExt;

    // CREATE_NEW_PROCESS_GROUP: Creates new process group (0x00000200)
    // DETACHED_PROCESS: Creates process without console (0x00000008)
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
    const DETACHED_PROCESS: u32 = 0x00000008;

    // Build the command, optionally piping JSON context to stdin
    let full_command = match context_json {
        Some(json) => {
            // PowerShell single-quote escaping:
            // - Single quotes prevent variable expansion ($)
            // - Backticks need escaping even in single quotes (`` ` `` → ``` `` ```)
            // - Single quotes need doubling (`'` → `''`)
            let escaped_json = json.replace('`', "``").replace('\'', "''");
            // Pipe JSON to the command via PowerShell script block
            format!("'{}' | & {{ {} }}", escaped_json, command)
        }
        None => command.to_string(),
    };

    let mut cmd = Command::new("powershell");
    cmd.args(["-NoProfile", "-Command", &full_command]);

    cmd.current_dir(worktree_path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            log_file
                .try_clone()
                .context("Failed to clone log file handle")?,
        ))
        .stderr(Stdio::from(log_file))
        .creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS)
        .spawn()
        .context("Failed to spawn detached process")?;

    Ok(())
}
