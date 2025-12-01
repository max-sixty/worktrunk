//! Zellij workspace integration for worktrunk.
//!
//! # Goals
//!
//! Enable a workspace-based workflow where each repository has a dedicated zellij
//! session, and each worktree has a "seat" (canonical pane) within that session.
//! When you run `wt switch feature`, instead of changing directories, it focuses
//! (or creates) the pane for that worktree.
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
//! │  wt switch foo  - Focus/create seat (when inside workspace)     │
//! └──────────────────────────┬──────────────────────────────────────┘
//!                            │
//!                            ▼
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    Library Layer (this module)                  │
//! │  detect_context()       - Where are we running?                 │
//! │  session_name_for_repo() - Deterministic session naming         │
//! │  send_pipe_message()    - CLI → Plugin communication            │
//! │  create_session()       - Launch zellij with layout             │
//! └──────────────────────────┬──────────────────────────────────────┘
//!                            │ zellij pipe --name wt
//!                            ▼
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    Plugin Layer (wt-bridge)                     │
//! │  Runs as WASM inside zellij via load_plugins                    │
//! │  Receives: "select|/path/to/worktree"                           │
//! │  Actions: focus_terminal_pane() or open_terminal()              │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Terminology
//!
//! - **Workspace**: A zellij session dedicated to one repository (named `wt:<hash>`)
//! - **Seat**: A terminal pane dedicated to one worktree within a workspace
//!
//! # Message Protocol
//!
//! The CLI communicates with the plugin via `zellij pipe`:
//!
//! ```text
//! zellij pipe --name wt -- "select|/path/to/worktree"
//! ```
//!
//! The plugin maintains a mapping from worktree paths to pane IDs. On receiving
//! a `select` message, it either focuses an existing pane or creates a new one.
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
//! The plugin layer requires manual testing inside zellij:
//!
//! ```bash
//! # 1. Install the plugin
//! wt ui setup
//!
//! # 2. Enter a workspace
//! wt ui
//!
//! # 3. Verify permissions were granted (check for dialog on first run)
//!
//! # 4. Test the pipe message interface
//! zellij pipe --name wt -- "select|/tmp"
//!
//! # Expected: A new pane opens in /tmp (or existing pane focuses)
//! ```
//!
//! If nothing happens, check `~/.config/zellij/config.kdl` contains the
//! `load_plugins` entry for wt-bridge.wasm.

use std::env;
use std::path::Path;
use std::process::Command;

/// Session name prefix for worktrunk-managed zellij sessions.
const SESSION_PREFIX: &str = "wt:";

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

/// Send a pipe message to the wt-bridge plugin.
///
/// Format: `zellij pipe --name wt '<action>|<data>'`
pub fn send_pipe_message(action: &str, data: &str) -> anyhow::Result<()> {
    let message = format!("{}|{}", action, data);

    let output = Command::new("zellij")
        .args(["pipe", "--name", "wt", &message])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to send pipe message: {}", stderr.trim());
    }

    Ok(())
}

/// Request the wt-bridge plugin to focus or create a seat for a worktree.
pub fn focus_or_create_seat(worktree_path: &Path) -> anyhow::Result<()> {
    // Git worktree paths come from validated git operations and should always be valid UTF-8.
    // If not, the pipe protocol can't handle it anyway.
    let path_str = worktree_path
        .to_str()
        .expect("worktree path from git should be valid UTF-8");

    send_pipe_message("select", path_str)
}

/// Focus a worktree seat if running inside a worktrunk workspace.
///
/// Returns `Ok(true)` if we're inside a workspace and handled the seat focus,
/// `Ok(false)` if we're outside a workspace (caller should use normal cd behavior),
/// or an error if seat focusing failed.
pub fn try_focus_seat(repo_root: &Path, worktree_path: &Path) -> anyhow::Result<bool> {
    let context = detect_context(repo_root);

    if context.is_in_workspace() {
        focus_or_create_seat(worktree_path)?;
        Ok(true)
    } else {
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
    // Message protocol tests
    // ─────────────────────────────────────────────────────────────────────────

    /// Verify the message format that would be sent to the plugin.
    /// This tests the format, not actual sending (which requires zellij).
    #[test]
    fn pipe_message_format() {
        // The format should be "action|data"
        let action = "select";
        let data = "/path/to/worktree";
        let message = format!("{}|{}", action, data);

        assert_eq!(message, "select|/path/to/worktree");

        // Verify it can be parsed back
        let (parsed_action, parsed_data) = message.split_once('|').unwrap();
        assert_eq!(parsed_action, action);
        assert_eq!(parsed_data, data);
    }

    #[test]
    fn pipe_message_handles_paths_with_spaces() {
        let path = "/home/user/my project/worktree";
        let message = format!("select|{}", path);

        let (_, parsed_path) = message.split_once('|').unwrap();
        assert_eq!(parsed_path, path);
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
