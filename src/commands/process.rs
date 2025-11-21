use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use worktrunk::git::{GitError, Repository};

/// Spawn a detached background process with output redirected to a log file
///
/// The process will be fully detached from the parent:
/// - On Unix: uses double-fork with setsid to create a daemon
/// - On Windows: uses CREATE_NEW_PROCESS_GROUP to detach from console
///
/// Logs are centralized in the primary worktree's `.git/wt-logs/` directory.
///
/// # Arguments
/// * `repo` - Repository instance for accessing git common directory
/// * `worktree_path` - Working directory for the command
/// * `command` - Shell command to execute
/// * `branch` - Branch name for log organization
/// * `name` - Operation identifier (e.g., "post-start-npm", "remove")
///
/// # Returns
/// Path to the log file where output is being written
pub fn spawn_detached(
    repo: &Repository,
    worktree_path: &Path,
    command: &str,
    branch: &str,
    name: &str,
) -> Result<std::path::PathBuf, GitError> {
    // Get the git common directory (shared across all worktrees)
    let git_common_dir = repo.git_common_dir()?;

    // Create log directory in the common git directory
    let log_dir = git_common_dir.join("wt-logs");
    fs::create_dir_all(&log_dir)
        .map_err(|e| GitError::CommandFailed(format!("Failed to create log directory\n   {e}")))?;

    // Generate log filename (no timestamp - overwrites on each run)
    // Format: {branch}-{name}.log (e.g., "feature-post-start-npm.log", "bugfix-remove.log")
    // Sanitize branch name: replace '/' with '-' to avoid creating subdirectories
    let safe_branch = branch.replace('/', "-");
    let log_path = log_dir.join(format!("{}-{}.log", safe_branch, name));

    // Create log file
    let log_file = fs::File::create(&log_path)
        .map_err(|e| GitError::CommandFailed(format!("Failed to create log file\n   {e}")))?;

    #[cfg(unix)]
    {
        spawn_detached_unix(worktree_path, command, log_file)?;
    }

    #[cfg(windows)]
    {
        spawn_detached_windows(worktree_path, command, log_file)?;
    }

    Ok(log_path)
}

#[cfg(unix)]
fn spawn_detached_unix(
    worktree_path: &Path,
    command: &str,
    log_file: fs::File,
) -> Result<(), GitError> {
    // Detachment using nohup and background execution (&):
    // - nohup makes the process immune to SIGHUP (continues after parent exits)
    // - sh -c allows complex shell commands with pipes, redirects, etc.
    // - & backgrounds the process immediately
    // - We wait for the outer shell to exit (happens immediately after backgrounding)
    // - This prevents zombie process accumulation under high concurrency
    // - Output redirected to log file for debugging
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "nohup sh -c {} &",
            shell_escape::escape(command.into())
        ))
        .current_dir(worktree_path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file.try_clone().map_err(|e| {
            GitError::CommandFailed(format!("Failed to clone log file handle: {}", e))
        })?))
        .stderr(Stdio::from(log_file))
        .spawn()
        .map_err(|e| {
            GitError::CommandFailed(format!("Failed to spawn detached process\n   {e}"))
        })?;

    // Wait for the outer shell to exit (immediate, doesn't block on background command)
    child.wait().map_err(|e| {
        GitError::CommandFailed(format!("Failed to wait for detachment shell\n   {e}"))
    })?;

    Ok(())
}

#[cfg(windows)]
fn spawn_detached_windows(
    worktree_path: &Path,
    command: &str,
    log_file: fs::File,
) -> Result<(), GitError> {
    use std::os::windows::process::CommandExt;

    // CREATE_NEW_PROCESS_GROUP: Creates new process group (0x00000200)
    // DETACHED_PROCESS: Creates process without console (0x00000008)
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
    const DETACHED_PROCESS: u32 = 0x00000008;

    Command::new("cmd")
        .args(["/C", command])
        .current_dir(worktree_path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file.try_clone().map_err(|e| {
            GitError::CommandFailed(format!("Failed to clone log file handle: {}", e))
        })?))
        .stderr(Stdio::from(log_file))
        .creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS)
        .spawn()
        .map_err(|e| {
            GitError::CommandFailed(format!("Failed to spawn detached process\n   {e}"))
        })?;

    Ok(())
}
