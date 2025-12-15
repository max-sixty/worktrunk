use anyhow::Context;
use std::fs;
use std::path::Path;
#[cfg(unix)]
use std::process::Command;
use std::process::Stdio;
use worktrunk::git::Repository;
use worktrunk::path::format_path_for_display;

/// Sanitize a string for use as a filename on all platforms.
/// Replaces characters that are illegal in Windows filenames or are path separators.
/// Also handles Windows reserved device names (CON, PRN, AUX, NUL, COM1-9, LPT1-9).
fn sanitize_for_filename(s: &str) -> String {
    // Replace illegal characters
    let sanitized: String = s
        .chars()
        .map(|c| match c {
            '/' | '\\' | '<' | '>' | ':' | '"' | '|' | '?' | '*' => '-',
            _ => c,
        })
        .collect();

    // Check for Windows reserved device names (case-insensitive)
    // These cannot be used as filenames on Windows, even with extensions
    let upper = sanitized.to_uppercase();
    let is_reserved = matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.chars().nth(3).is_some_and(|c| matches!(c, '1'..='9')));

    if is_reserved {
        format!("_{}", sanitized)
    } else {
        sanitized
    }
}

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
            format!(
                "printf '%s' {} | {{ {}{} }}",
                shell_escape::escape(json.into()),
                command,
                posix_command_separator(command)
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
    use worktrunk::shell_exec::ShellConfig;

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
        .creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS)
        .spawn()
        .context("Failed to spawn detached process")?;

    // Windows: Process is fully detached via DETACHED_PROCESS flag,
    // no need to wait (unlike Unix which waits for the outer shell)

    Ok(())
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
}
