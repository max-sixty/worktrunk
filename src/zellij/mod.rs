//! Zellij workspace integration for worktrunk.
//!
//! # Goals
//!
//! Enable a workspace-based workflow where each repository has a dedicated zellij
//! session, and each worktree has its own tab within that session.
//! When you run `wt switch feature`, instead of changing directories, it focuses
//! (or creates) the tab for that worktree.
//!
//! # Architecture
//!
//! The integration has three layers:
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                         CLI Commands                            │
//! │  wt ui          - Enter/create workspace                        │
//! │  wt ui setup    - Install plugin                                │
//! │  wt ui status   - Show context                                  │
//! │  wt switch foo  - Focus/create tab (when inside workspace)      │
//! └──────────────────────────┬──────────────────────────────────────┘
//!                            │
//!                            ▼
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    Library Layer (this module)                  │
//! │  detect_context()       - Where are we running?                 │
//! │  session_name_for_repo() - Deterministic session naming         │
//! │  focus_or_create_tab()  - Tab management via plugin protocol    │
//! │  create_session()       - Launch zellij with layout             │
//! └──────────────────────────┬──────────────────────────────────────┘
//!                            │ zellij pipe --name wt
//!                            ▼
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    Plugin Layer (wt-bridge)                     │
//! │  Runs as WASM inside zellij via load_plugins                    │
//! │  Tracks: worktree_path -> tab_index mapping                     │
//! │  Protocol:                                                      │
//! │    "sync|path" → "synced" (register current tab with path)      │
//! │    "select|name|path" → "focused" or "not_found:unique_name"    │
//! │    "register|name|path" → "registered"                          │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Why a Plugin?
//!
//! The plugin solves the "main/main collision" problem: when two repos both have
//! a `main` branch, we get two tabs named `main`. Using `go-to-tab-name "main"`
//! is ambiguous. The plugin tracks tabs by worktree path and routes by tab INDEX,
//! which is unambiguous.
//!
//! # Terminology
//!
//! - **Workspace**: A zellij session dedicated to one repository (named `wt:<hash>`)
//! - **Tab**: A named tab dedicated to one worktree within a workspace
//!
//! # Context Detection
//!
//! The library detects four contexts via environment variables:
//!
//! 1. **Outside** - Not in any zellij session
//! 2. **InsideWorkspace** - In the correct worktrunk session for this repo
//! 3. **InsideOtherWorkspace** - In a worktrunk session for a different repo
//! 4. **InsideOtherSession** - In a non-worktrunk zellij session
//!
//! # Testing
//!
//! The library layer is tested via unit tests (see tests module below).
//! Manual testing inside zellij:
//!
//! ```bash
//! # 1. Install the plugin
//! wt ui setup
//!
//! # 2. Enter a workspace
//! wt ui
//!
//! # 3. Test tab switching
//! wt switch feature  # Creates or focuses "feature" tab
//! wt switch main     # Creates or focuses "main" tab
//! ```

use std::env;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};

/// Session name prefix for worktrunk-managed zellij sessions.
const SESSION_PREFIX: &str = "wt:";

/// Pipe name for wt-bridge plugin communication.
const PIPE_NAME: &str = "wt";

/// Plugin path for targeting pipe messages.
const PLUGIN_PATH: &str = "file:~/.config/zellij/plugins/wt-bridge.wasm";

/// Environment variable set by zellij when running inside a session.
const ZELLIJ_ENV: &str = "ZELLIJ";

/// Environment variable containing the current zellij session name.
const ZELLIJ_SESSION_NAME_ENV: &str = "ZELLIJ_SESSION_NAME";

/// The current zellij context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZellijContext {
    /// Not running inside any zellij session.
    Outside,

    /// Inside a worktrunk-managed session for this repository.
    InsideWorkspace {
        /// The session name (e.g., "wt:a1b2c3d")
        session_name: String,
    },

    /// Inside a worktrunk session, but for a different repository.
    InsideOtherWorkspace {
        /// The session name of the current session
        current_session: String,
        /// The expected session name for this repository
        expected_session: String,
    },

    /// Inside a non-worktrunk zellij session.
    InsideOtherSession {
        /// The session name of the non-worktrunk session
        session_name: String,
    },
}

impl ZellijContext {
    /// Returns true if we're inside the worktrunk workspace for this repo.
    pub fn is_in_workspace(&self) -> bool {
        matches!(self, ZellijContext::InsideWorkspace { .. })
    }
}

/// Check if zellij is available on the system.
pub fn is_zellij_available() -> bool {
    Command::new("zellij")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Detect the current zellij context for a repository.
///
/// # Arguments
/// * `repo_root` - The canonicalized path to the repository root (from `repo.worktree_base()`)
///
/// # Returns
/// The current zellij context indicating whether we're outside zellij,
/// inside the correct workspace, or inside some other session.
pub fn detect_context(repo_root: &Path) -> ZellijContext {
    // Check if we're inside zellij at all
    if env::var(ZELLIJ_ENV).is_err() {
        return ZellijContext::Outside;
    }

    // Get the current session name
    let current_session = match env::var(ZELLIJ_SESSION_NAME_ENV) {
        Ok(name) => name,
        Err(_) => {
            // Inside zellij but can't determine session name - treat as other session
            return ZellijContext::InsideOtherSession {
                session_name: "<unknown>".to_string(),
            };
        }
    };

    // Calculate the expected session name for this repo
    let expected_session = session_name_for_repo(repo_root);

    // Check if it's a worktrunk session
    if !current_session.starts_with(SESSION_PREFIX) {
        return ZellijContext::InsideOtherSession {
            session_name: current_session,
        };
    }

    // It's a worktrunk session - check if it's for this repo
    if current_session == expected_session {
        ZellijContext::InsideWorkspace {
            session_name: current_session,
        }
    } else {
        ZellijContext::InsideOtherWorkspace {
            current_session,
            expected_session,
        }
    }
}

/// Generate a session name for a repository.
///
/// Format: `wt:<short_hash>` where short_hash is the first 7 characters
/// of a hash of the canonicalized repository root path.
pub fn session_name_for_repo(repo_root: &Path) -> String {
    let hash = short_hash(repo_root);
    format!("{}{}", SESSION_PREFIX, hash)
}

/// Generate a short hash of a path for use in session names.
///
/// Uses the first 7 characters of a blake3 hash (or falls back to a simple hash).
fn short_hash(path: &Path) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    let hash = hasher.finish();

    // Format as hex and take first 7 characters
    format!("{:016x}", hash)[..7].to_string()
}

/// Session status from `zellij list-sessions`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    /// Session is running and can be attached.
    Running,
    /// Session exited and needs resurrection (may fail).
    Exited,
    /// Session does not exist.
    NotFound,
}

/// Check the status of a zellij session.
pub fn session_status(session_name: &str) -> SessionStatus {
    let output = match Command::new("zellij").args(["list-sessions"]).output() {
        Ok(o) => o,
        Err(_) => return SessionStatus::NotFound,
    };

    let stdout = String::from_utf8_lossy(&output.stdout);

    for line in stdout.lines() {
        // Format: "session-name [Created Xs ago] (EXITED - attach to resurrect)"
        // or: "session-name [Created Xs ago]"
        if line.contains(session_name) {
            if line.contains("EXITED") {
                return SessionStatus::Exited;
            } else {
                return SessionStatus::Running;
            }
        }
    }

    SessionStatus::NotFound
}

/// Check if a zellij session with the given name exists (running or exited).
pub fn session_exists(session_name: &str) -> bool {
    session_status(session_name) != SessionStatus::NotFound
}

/// Delete a zellij session.
pub fn delete_session(session_name: &str) -> anyhow::Result<()> {
    let output = Command::new("zellij")
        .args(["delete-session", session_name])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to delete session: {}", stderr.trim());
    }

    Ok(())
}

/// Attach to an existing zellij session.
///
/// This replaces the current process with the zellij attach command.
pub fn attach_session(session_name: &str) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;

    let err = Command::new("zellij").args(["attach", session_name]).exec();

    // exec() only returns on error
    Err(anyhow::anyhow!("Failed to attach to session: {}", err))
}

/// Create a new zellij session with the given name and working directory.
///
/// If `layout` is provided, the session will use that layout file.
/// This replaces the current process with the zellij command.
pub fn create_session(session_name: &str, cwd: &Path, layout: Option<&Path>) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;

    let mut cmd = Command::new("zellij");

    // Use --new-session-with-layout to GUARANTEE a new session is created
    // (--layout alone might try to attach if session name exists)
    if let Some(layout_path) = layout {
        cmd.args([
            "--new-session-with-layout",
            layout_path.to_str().unwrap(),
            "--session",
            session_name,
        ]);
    } else {
        cmd.args(["--session", session_name]);
    }

    cmd.current_dir(cwd);

    let err = cmd.exec();

    // exec() only returns on error
    Err(anyhow::anyhow!("Failed to create session: {}", err))
}

/// Timeout for waiting for plugin response.
///
/// Plugin responds immediately after permission grant. This timeout only triggers
/// if permissions haven't been granted yet (plugin queues but never responds).
/// 3s chosen as: fast enough to fail quickly, slow enough for system lag.
const PIPE_TIMEOUT_SECS: u64 = 3;

/// Send a message to the wt-bridge plugin and read the response.
///
/// Uses `zellij pipe` for bidirectional communication with the plugin.
/// The plugin responds via `cli_pipe_output`.
///
/// Includes a timeout to prevent hanging if the plugin hasn't been granted
/// permissions yet (in which case it queues messages but never responds).
fn pipe_message(payload: &str) -> anyhow::Result<String> {
    use std::io::Write;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    let mut child = Command::new("zellij")
        .args(["pipe", "--plugin", PLUGIN_PATH, "--name", PIPE_NAME])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    // Write payload to stdin and close it
    if let Some(mut stdin) = child.stdin.take() {
        writeln!(stdin, "{}", payload)?;
        // stdin is dropped here, closing it
    }

    // Read response from stdout with timeout (plugin may not respond if permissions not granted)
    let stdout = child.stdout.take().expect("stdout was piped");

    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut response = String::new();
        let result = reader.read_line(&mut response);
        let _ = tx.send((result, response));
    });

    let timeout = Duration::from_secs(PIPE_TIMEOUT_SECS);
    let (read_result, response) = rx.recv_timeout(timeout).map_err(|_| {
        // Kill child process; reader thread will unblock when stdout closes
        let _ = child.kill();
        anyhow::anyhow!(
            "Timed out waiting for wt-bridge plugin response after {}s. \
             The plugin may not have been granted permissions yet. \
             Run: zellij action launch-or-focus-plugin \"file:~/.config/zellij/plugins/wt-bridge.wasm\" --floating\n\
             Then grant permissions and try again.",
            PIPE_TIMEOUT_SECS
        )
    })?;

    let bytes_read = read_result?;

    let status = child.wait()?;
    if !status.success() {
        let stderr_msg = match child.stderr.take() {
            Some(s) => {
                let mut buf = String::new();
                BufReader::new(s).read_line(&mut buf).unwrap_or(0);
                buf
            }
            None => String::new(),
        };
        let stderr_trimmed = stderr_msg.trim();
        if stderr_trimmed.is_empty() {
            anyhow::bail!("Pipe command failed (no error details)");
        } else {
            anyhow::bail!("Pipe command failed: {}", stderr_trimmed);
        }
    }

    if bytes_read == 0 {
        anyhow::bail!(
            "No response from wt-bridge plugin. \
             Is the plugin loaded? Run 'wt ui setup' to install."
        );
    }

    Ok(response.trim_end().to_string())
}

/// Create a new tab with the given name and working directory.
///
/// Uses `zellij action new-tab` to create the tab, then registers it with
/// the wt-bridge plugin so it can be tracked by path.
fn create_and_register_tab(name: &str, cwd: &Path) -> anyhow::Result<()> {
    let cwd_str = cwd
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Worktree path is not valid UTF-8: {:?}", cwd))?;

    // Create the tab with zellij action (plugin API doesn't support cwd)
    let output = Command::new("zellij")
        .args(["action", "new-tab", "--name", name, "--cwd", cwd_str])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to create tab '{}': {}", name, stderr.trim());
    }

    // Register the new tab with the plugin for path tracking
    let register_payload = format!("register|{}|{}", name, cwd_str);
    pipe_message(&register_payload)?;

    Ok(())
}

/// Sync the current tab with the plugin (register if not tracked).
///
/// This ensures the currently active tab is associated with the given worktree path.
/// Critical for the first switch: the layout creates an initial tab without registering it,
/// so we must sync before the first `select` or the plugin won't know about the starting tab.
fn sync_current_tab(current_worktree: &Path) -> anyhow::Result<()> {
    let path_str = current_worktree.to_str().ok_or_else(|| {
        anyhow::anyhow!("Worktree path is not valid UTF-8: {:?}", current_worktree)
    })?;

    let sync_payload = format!("sync|{}", path_str);
    log::debug!("[zellij] sync_current_tab: sending {:?}", sync_payload);
    let response = pipe_message(&sync_payload)?;
    log::debug!("[zellij] sync_current_tab: response {:?}", response);

    if response == "synced" {
        Ok(())
    } else {
        anyhow::bail!(
            "Unexpected sync response from wt-bridge plugin: {}",
            response
        )
    }
}

/// Focus or create a tab for a worktree.
///
/// Uses the wt-bridge plugin protocol:
/// 1. Sync the current tab with the plugin (register if not tracked)
/// 2. Send "select|{name}|{path}" to plugin
/// 3. If response is "focused", the plugin switched to the tab by index
/// 4. If response is "not_found:{unique_name}", create a new tab with that name
///
/// The plugin handles the "main/main collision" problem by tracking tabs by path
/// and routing by index rather than by name.
pub fn focus_or_create_tab(current_worktree: &Path, target_worktree: &Path) -> anyhow::Result<()> {
    log::debug!(
        "[zellij] focus_or_create_tab: current={:?} target={:?}",
        current_worktree,
        target_worktree
    );

    // First, sync the current tab to ensure it's tracked
    sync_current_tab(current_worktree)?;

    let path_str = target_worktree.to_str().ok_or_else(|| {
        anyhow::anyhow!("Worktree path is not valid UTF-8: {:?}", target_worktree)
    })?;

    // Extract the display name from the worktree directory name (typically the branch name)
    let display_name = target_worktree
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("worktree");

    // Ask plugin to select the tab by path
    let select_payload = format!("select|{}|{}", display_name, path_str);
    log::debug!("[zellij] focus_or_create_tab: sending {:?}", select_payload);
    let response = pipe_message(&select_payload)?;
    log::debug!("[zellij] focus_or_create_tab: response {:?}", response);

    if response == "focused" {
        // Plugin found the tab and focused it by index
        Ok(())
    } else if let Some(unique_name) = response.strip_prefix("not_found:") {
        // Tab doesn't exist - create with the unique name provided by plugin
        // (may have hash suffix if display_name collides with existing tabs)
        log::debug!(
            "[zellij] focus_or_create_tab: creating new tab {:?}",
            unique_name
        );
        create_and_register_tab(unique_name, target_worktree)
    } else {
        anyhow::bail!("Unexpected response from wt-bridge plugin: {}", response)
    }
}

/// Focus a worktree tab if running inside a worktrunk workspace.
///
/// Returns `Ok(true)` if we're inside a workspace and handled the tab focus,
/// `Ok(false)` if we're outside a workspace (caller should use normal cd behavior),
/// or an error if tab focusing failed.
///
/// # Arguments
/// * `repo_root` - The repository root (for context detection)
/// * `current_worktree` - The worktree we're currently in (will be synced with plugin)
/// * `target_worktree` - The worktree to switch to
pub fn try_focus_tab(
    repo_root: &Path,
    current_worktree: &Path,
    target_worktree: &Path,
) -> anyhow::Result<bool> {
    let context = detect_context(repo_root);
    log::debug!(
        "[zellij] try_focus_tab: repo_root={:?} context={:?}",
        repo_root,
        context
    );

    if context.is_in_workspace() {
        focus_or_create_tab(current_worktree, target_worktree)?;
        Ok(true)
    } else {
        log::debug!("[zellij] try_focus_tab: not in workspace, skipping");
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ─────────────────────────────────────────────────────────────────────────
    // Session naming tests
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn session_name_is_deterministic() {
        let path = PathBuf::from("/home/user/repos/myproject");
        let name1 = session_name_for_repo(&path);
        let name2 = session_name_for_repo(&path);
        assert_eq!(name1, name2, "same path should produce same session name");
    }

    #[test]
    fn session_name_has_correct_format() {
        let path = PathBuf::from("/home/user/repos/myproject");
        let name = session_name_for_repo(&path);

        assert!(name.starts_with("wt:"), "must start with 'wt:' prefix");
        assert_eq!(name.len(), 10, "must be 'wt:' + 7 hex chars");

        // Verify the hash part is valid hex
        let hash_part = &name[3..];
        assert!(
            hash_part.chars().all(|c| c.is_ascii_hexdigit()),
            "hash part must be hex: {}",
            hash_part
        );
    }

    #[test]
    fn different_paths_produce_different_names() {
        let paths = [
            PathBuf::from("/home/user/repos/project1"),
            PathBuf::from("/home/user/repos/project2"),
            PathBuf::from("/home/user/repos/project1/subdir"), // even similar paths differ
        ];

        let names: Vec<_> = paths.iter().map(|p| session_name_for_repo(p)).collect();

        // All names should be unique
        for (i, name1) in names.iter().enumerate() {
            for (j, name2) in names.iter().enumerate() {
                if i != j {
                    assert_ne!(
                        name1, name2,
                        "paths {:?} and {:?} should produce different names",
                        paths[i], paths[j]
                    );
                }
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Context detection tests
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn context_outside_when_no_zellij_env() {
        // Most test environments run outside zellij
        if env::var(ZELLIJ_ENV).is_err() {
            let path = PathBuf::from("/test/repo");
            assert_eq!(detect_context(&path), ZellijContext::Outside);
        }
    }

    #[test]
    fn context_is_in_workspace_method() {
        // Test the is_in_workspace() helper
        assert!(
            ZellijContext::InsideWorkspace {
                session_name: "wt:abc1234".to_string()
            }
            .is_in_workspace()
        );

        assert!(!ZellijContext::Outside.is_in_workspace());

        assert!(
            !ZellijContext::InsideOtherSession {
                session_name: "my-session".to_string()
            }
            .is_in_workspace()
        );

        assert!(
            !ZellijContext::InsideOtherWorkspace {
                current_session: "wt:abc1234".to_string(),
                expected_session: "wt:def5678".to_string(),
            }
            .is_in_workspace()
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Hash function tests
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn short_hash_is_7_chars() {
        let paths = [
            PathBuf::from("/a"),
            PathBuf::from("/very/long/path/to/some/deep/directory"),
            PathBuf::from(""),
        ];

        for path in paths {
            let hash = short_hash(&path);
            assert_eq!(hash.len(), 7, "hash for {:?} should be 7 chars", path);
        }
    }

    #[test]
    fn short_hash_is_hex() {
        let path = PathBuf::from("/some/path");
        let hash = short_hash(&path);

        assert!(
            hash.chars().all(|c| c.is_ascii_hexdigit()),
            "hash should be hex: {}",
            hash
        );
    }
}
