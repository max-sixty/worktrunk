use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use worktrunk::git::{GitError, GitResultExt};

/// Spawn a detached background process with output redirected to a log file
///
/// The process will be fully detached from the parent:
/// - On Unix: uses double-fork with setsid to create a daemon
/// - On Windows: uses CREATE_NEW_PROCESS_GROUP to detach from console
///
/// # Arguments
/// * `worktree_path` - Working directory for the command
/// * `command` - Shell command to execute
/// * `name` - Name identifier for the process (used in log filename)
///
/// # Returns
/// Path to the log file where output is being written
pub fn spawn_detached(
    worktree_path: &Path,
    command: &str,
    name: &str,
) -> Result<std::path::PathBuf, GitError> {
    // Resolve the actual .git directory path
    // In a worktree, .git is a file pointing to the real git directory
    let git_path = worktree_path.join(".git");
    let git_dir = if git_path.is_file() {
        // Read the gitdir path from the file
        let content = fs::read_to_string(&git_path).git_context("Failed to read .git file")?;
        // Format is "gitdir: /path/to/git/dir"
        let gitdir_path = content
            .trim()
            .strip_prefix("gitdir: ")
            .ok_or_else(|| GitError::CommandFailed("Invalid .git file format".to_string()))?;
        Path::new(gitdir_path).to_path_buf()
    } else {
        git_path
    };

    // Create log directory in the git directory
    let log_dir = git_dir.join("wt-logs");
    fs::create_dir_all(&log_dir).git_context("Failed to create log directory")?;

    // Generate log filename (no timestamp - overwrites on each run)
    let log_path = log_dir.join(format!("post-start-{}.log", name));

    // Create log file
    let log_file = fs::File::create(&log_path).git_context("Failed to create log file")?;

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
        .git_context("Failed to spawn detached process")?;

    // Wait for the outer shell to exit (immediate, doesn't block on background command)
    child
        .wait()
        .git_context("Failed to wait for detachment shell")?;

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
        .git_context("Failed to spawn detached process")?;

    Ok(())
}
