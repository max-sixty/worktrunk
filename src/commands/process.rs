use anyhow::Context;
use std::fs;
use std::path::Path;
#[cfg(unix)]
use std::process::Command;
use std::process::Stdio;
use worktrunk::git::Repository;
use worktrunk::path::{format_path_for_display, sanitize_for_filename};

/// Get the separator needed before closing brace in POSIX shell command grouping.
/// Returns empty string if command already ends with newline or semicolon.
fn posix_command_separator(command: &str) -> &'static str {
    if command.ends_with('\n') || command.ends_with(';') {
        ""
    } else {
        ";"
    }
}

/// Spawn a detached background process with output redirected to a log file
///
/// The process will be fully detached from the parent:
/// - On Unix: uses process_group(0) to create a new process group (survives PTY closure)
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
    // Create log directory in the common git directory
    let log_dir = repo.wt_logs_dir();
    fs::create_dir_all(&log_dir).with_context(|| {
        format!(
            "Failed to create log directory {}",
            format_path_for_display(&log_dir)
        )
    })?;

    // Generate log filename (no timestamp - overwrites on each run)
    // Format: {branch}-{name}.log (e.g., "feature-post-start-npm.log", "bugfix-remove.log")
    let safe_branch = sanitize_for_filename(branch);
    let safe_name = sanitize_for_filename(name);
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
        spawn_detached_unix(worktree_path, command, log_file, context_json, name)?;
    }

    #[cfg(windows)]
    {
        spawn_detached_windows(worktree_path, command, log_file, context_json, name)?;
    }

    Ok(log_path)
}

#[cfg(unix)]
fn spawn_detached_unix(
    worktree_path: &Path,
    command: &str,
    log_file: fs::File,
    context_json: Option<&str>,
    name: &str,
) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;

    // Build the command, optionally piping JSON context to stdin
    let full_command = match context_json {
        Some(json) => {
            // Use printf to pipe JSON to the command's stdin
            // printf is more portable than echo for arbitrary content
            // Wrap command in braces to ensure proper grouping with &&, ||, etc.
            format!(
                "printf '%s' {} | {{ {}{} }}",
                shell_escape::escape(json.into()),
                command,
                posix_command_separator(command)
            )
        }
        None => command.to_string(),
    };

    let shell_cmd = format!("{} &", full_command);

    // Log only the operation identifier, not the full command (which may contain context_json
    // with user data that shouldn't appear in debug logs)
    log::debug!("spawn_detached: {} in {}", name, worktree_path.display());

    // Detachment via process_group(0): puts the spawned shell in its own process group.
    // When the controlling PTY closes, SIGHUP is sent to the foreground process group.
    // Since our process is in a different group, it doesn't receive the signal.
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(&shell_cmd)
        .current_dir(worktree_path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            log_file
                .try_clone()
                .context("Failed to clone log file handle")?,
        ))
        .stderr(Stdio::from(log_file))
        // Prevent hooks from writing to the directive file
        .env_remove(worktrunk::shell_exec::DIRECTIVE_FILE_ENV_VAR)
        .process_group(0) // New process group, not in PTY's foreground group
        .spawn()
        .context("Failed to spawn detached process")?;

    // Wait for sh to exit (immediate, doesn't block on background command)
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
    name: &str,
) -> anyhow::Result<()> {
    use std::os::windows::process::CommandExt;
    use worktrunk::shell_exec::ShellConfig;

    // Log only the operation identifier, not the full command (which may contain context_json
    // with user data that shouldn't appear in debug logs)
    log::debug!("spawn_detached: {} in {}", name, worktree_path.display());

    // CREATE_NEW_PROCESS_GROUP: Creates new process group (0x00000200)
    // DETACHED_PROCESS: Creates process without console (0x00000008)
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
    const DETACHED_PROCESS: u32 = 0x00000008;

    let shell = ShellConfig::get();

    // Build the command based on shell type
    let mut cmd = if shell.is_posix() {
        // Git Bash available - use same syntax as Unix
        let full_command = match context_json {
            Some(json) => {
                // Use printf to pipe JSON to the command's stdin (same as Unix)
                format!(
                    "printf '%s' {} | {{ {}{} }}",
                    shell_escape::escape(json.into()),
                    command,
                    posix_command_separator(command)
                )
            }
            None => command.to_string(),
        };
        shell.command(&full_command)
    } else {
        // PowerShell fallback
        let full_command = match context_json {
            Some(json) => {
                // PowerShell single-quote escaping:
                // - Single quotes prevent variable expansion ($) and are literal
                // - Backticks are literal in single quotes (NOT escape characters)
                // - Only single quotes need doubling (`'` → `''`)
                // See: https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_quoting_rules
                let escaped_json = json.replace('\'', "''");
                // Pipe JSON to the command via PowerShell script block
                format!("'{}' | & {{ {} }}", escaped_json, command)
            }
            None => command.to_string(),
        };
        shell.command(&full_command)
    };

    cmd.current_dir(worktree_path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            log_file
                .try_clone()
                .context("Failed to clone log file handle")?,
        ))
        .stderr(Stdio::from(log_file))
        // Prevent hooks from writing to the directive file
        .env_remove(worktrunk::shell_exec::DIRECTIVE_FILE_ENV_VAR)
        .creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS)
        .spawn()
        .context("Failed to spawn detached process")?;

    // Windows: Process is fully detached via DETACHED_PROCESS flag,
    // no need to wait (unlike Unix which waits for the outer shell)

    Ok(())
}

/// Build shell command for background worktree removal
///
/// `branch_to_delete` is the branch to delete after removing the worktree.
/// Pass `None` for detached HEAD or when branch should be retained.
/// This decision is computed upfront (checking if branch is merged) before spawning the background process.
///
/// `force_worktree` adds `--force` to `git worktree remove`, allowing removal
/// even when the worktree contains untracked files (like build artifacts).
pub fn build_remove_command(
    worktree_path: &std::path::Path,
    branch_to_delete: Option<&str>,
    force_worktree: bool,
) -> String {
    use shell_escape::escape;

    let worktree_path_str = worktree_path.to_string_lossy();
    let worktree_escaped = escape(worktree_path_str.as_ref().into());

    // TODO: This delay is a timing-based workaround, not a principled fix.
    // The race: after wt exits, the shell wrapper reads the directive file and
    // runs `cd`. But fish (and other shells) may call getcwd() before the cd
    // completes (e.g., for prompt updates), and if the background removal has
    // already deleted the directory, we get "shell-init: error retrieving current
    // directory". A 1s delay is very conservative (shell cd takes ~1-5ms), but
    // deterministic solutions (shell-spawned background, marker file sync) add
    // significant complexity for marginal benefit.
    let delay = "sleep 1";

    // Stop fsmonitor daemon first (best effort - ignore errors)
    // This prevents zombie daemons from accumulating when using builtin fsmonitor
    let stop_fsmonitor = format!(
        "git -C {} fsmonitor--daemon stop 2>/dev/null || true",
        worktree_escaped
    );

    let force_flag = if force_worktree { " --force" } else { "" };

    match branch_to_delete {
        Some(branch_name) => {
            let branch_escaped = escape(branch_name.into());
            format!(
                "{} && {} && git worktree remove{} {} && git branch -D {}",
                delay, stop_fsmonitor, force_flag, worktree_escaped, branch_escaped
            )
        }
        None => {
            format!(
                "{} && {} && git worktree remove{} {}",
                delay, stop_fsmonitor, force_flag, worktree_escaped
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_for_filename() {
        // Path separators
        assert_eq!(sanitize_for_filename("feature/branch"), "feature-branch");
        assert_eq!(sanitize_for_filename("feature\\branch"), "feature-branch");

        // Windows-illegal characters
        assert_eq!(sanitize_for_filename("bug:123"), "bug-123");
        assert_eq!(sanitize_for_filename("fix<angle>"), "fix-angle-");
        assert_eq!(sanitize_for_filename("fix|pipe"), "fix-pipe");
        assert_eq!(sanitize_for_filename("fix?question"), "fix-question");
        assert_eq!(sanitize_for_filename("fix*wildcard"), "fix-wildcard");
        assert_eq!(sanitize_for_filename("fix\"quotes\""), "fix-quotes-");

        // Multiple special characters
        assert_eq!(
            sanitize_for_filename("a/b\\c<d>e:f\"g|h?i*j"),
            "a-b-c-d-e-f-g-h-i-j"
        );

        // Already safe
        assert_eq!(sanitize_for_filename("normal-branch"), "normal-branch");
        assert_eq!(
            sanitize_for_filename("branch_with_underscore"),
            "branch_with_underscore"
        );

        // Windows reserved device names (must be prefixed to avoid conflicts)
        assert_eq!(sanitize_for_filename("CON"), "_CON");
        assert_eq!(sanitize_for_filename("con"), "_con");
        assert_eq!(sanitize_for_filename("PRN"), "_PRN");
        assert_eq!(sanitize_for_filename("AUX"), "_AUX");
        assert_eq!(sanitize_for_filename("NUL"), "_NUL");
        assert_eq!(sanitize_for_filename("COM1"), "_COM1");
        assert_eq!(sanitize_for_filename("com9"), "_com9");
        assert_eq!(sanitize_for_filename("LPT1"), "_LPT1");
        assert_eq!(sanitize_for_filename("lpt9"), "_lpt9");

        // COM0/LPT0 are NOT reserved (only 1-9 are)
        assert_eq!(sanitize_for_filename("COM0"), "COM0");
        assert_eq!(sanitize_for_filename("LPT0"), "LPT0");

        // Longer names are fine
        assert_eq!(sanitize_for_filename("CONSOLE"), "CONSOLE");
        assert_eq!(sanitize_for_filename("COM10"), "COM10");
    }

    #[test]
    fn test_posix_command_separator() {
        // Commands ending with newline don't need separator
        assert_eq!(posix_command_separator("echo hello\n"), "");

        // Commands ending with semicolon don't need separator
        assert_eq!(posix_command_separator("echo hello;"), "");

        // Commands without trailing newline/semicolon need separator
        assert_eq!(posix_command_separator("echo hello"), ";");

        // Empty command needs separator
        assert_eq!(posix_command_separator(""), ";");

        // Commands with internal newlines but not trailing
        assert_eq!(posix_command_separator("echo\nhello"), ";");

        // Commands with internal semicolons but not trailing
        assert_eq!(posix_command_separator("echo; hello"), ";");
    }

    #[test]
    fn test_build_remove_command() {
        use std::path::PathBuf;

        let path = PathBuf::from("/tmp/test-worktree");

        // Without branch deletion, without force
        let cmd = build_remove_command(&path, None, false);
        assert!(cmd.contains("git worktree remove"));
        assert!(cmd.contains("/tmp/test-worktree"));
        assert!(!cmd.contains("branch -D"));
        assert!(!cmd.contains("--force"));

        // With branch deletion, without force
        let cmd = build_remove_command(&path, Some("feature-branch"), false);
        assert!(cmd.contains("git worktree remove"));
        assert!(cmd.contains("git branch -D"));
        assert!(cmd.contains("feature-branch"));
        assert!(!cmd.contains("--force"));

        // With force flag
        let cmd = build_remove_command(&path, None, true);
        assert!(cmd.contains("git worktree remove --force"));

        // With branch deletion and force
        let cmd = build_remove_command(&path, Some("feature-branch"), true);
        assert!(cmd.contains("git worktree remove --force"));
        assert!(cmd.contains("git branch -D"));

        // Shell escaping for special characters
        let special_path = PathBuf::from("/tmp/test worktree");
        let cmd = build_remove_command(&special_path, Some("feature/branch"), false);
        assert!(cmd.contains("worktree remove"));
    }
}
